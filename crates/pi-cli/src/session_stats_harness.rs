//! Parity-harness entry for the `agent-session-stats` protocol: the
//! stats/exports slice of `pi_session::AgentSession` (TS `getSessionStats` /
//! `getContextUsage` / `exportToJsonl` / `summarizeForBugReport`) over a
//! scripted provider. TS mirror: parity/ts-harness.js
//! `agent-session-stats` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "model"?: object|null, "entries": [...],
//!               "systemPrompt": string,
//!               "settings"?: { "retry"?: {...} },
//!               "auth"?: { "auth"?: { "baseUrl"?, "apiKey"?, "headers"? },
//!                          "env"? },
//!               "physicalModels"?: [ { "provider": string, "id": string,
//!                                      "model": object } ] },
//!   "script": [ { "op": "getSessionStats" } |
//!               { "op": "getContextUsage" } |
//!               { "op": "exportToJsonl", "path"?: string } |
//!               { "op": "summarizeForBugReport", "hint"?: string } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ]
//! }
//! ```
//!
//! Result: `{ results, promptsSentToAgent, entries }`. `exportToJsonl`
//! results carry `{ path, content }` (the file is read back then removed —
//! idempotent on both sides).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{Clock, FixedClock};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, RequestAuth,
    SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";

/// Settings fake: `retry` seed, TS in-memory defaults elsewhere.
struct StatsHarnessSettings {
    retry: RetrySettings,
}

impl SessionSettingsStore for StatsHarnessSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
}

struct StatsHarnessAuth {
    auth: Option<RequestAuth>,
}

impl ModelAuthStore for StatsHarnessAuth {
    fn get_auth(&self, _model: &Value) -> Option<RequestAuth> {
        self.auth.clone()
    }
    fn has_configured_auth(&self, _provider: &str) -> bool {
        true
    }
    fn check_auth(&self, _provider: &str) -> bool {
        true
    }
    fn is_using_oauth(&self, _provider: &str) -> bool {
        false
    }
    fn no_model_selected_message(&self) -> String {
        "No model selected.".to_string()
    }
    fn no_api_key_message(&self, _provider: &str) -> String {
        "No API key found for the provider.".to_string()
    }
}

/// Recording fake `_extensionRunner` (no handlers — the stats slice never
/// emits through it).
struct StatsHarnessHost;

impl ExtensionHost for StatsHarnessHost {
    fn has_handlers(&self, _event: &str) -> bool {
        false
    }
    fn emit<'a>(&'a self, _event: Value) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
    fn emit_message_end<'a>(&'a self, _event: Value) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    fn emit_error(&self, _error: Value) {}
}

/// The patched-`setTimeout` mirror for retry backoff (fast-forward).
struct ImmediateSleeper;

impl pi_agent::host::Sleeper for ImmediateSleeper {
    fn sleep<'a>(
        &'a self,
        _ms: u64,
        signal: &'a pi_agent::host::AbortHandle,
    ) -> BoxFuture<'a, Result<(), pi_agent::host::Aborted>> {
        Box::pin(async move {
            if signal.is_aborted() {
                return Err(pi_agent::host::Aborted);
            }
            Ok(())
        })
    }
}

pub fn run_agent_session_stats_harness(args: &Value, process_cwd: &str) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;
    let config = args.get("config").cloned().unwrap_or(json!({}));
    if !config.is_object() {
        return Err("invalid config".to_string());
    }
    let system_prompt = match config.get("systemPrompt") {
        Some(Value::String(prompt)) => prompt.clone(),
        _ => return Err("missing systemPrompt".to_string()),
    };
    let entries = config
        .get("entries")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .filter(|e| e.first().and_then(|h| h.get("type")).and_then(Value::as_str) == Some("session"))
        .ok_or_else(|| "missing entries".to_string())?
        .clone();
    if let Some(model) = config.get("model") {
        if !model.is_null() && !model.is_object() {
            return Err("invalid model".to_string());
        }
    }
    let settings_in = config.get("settings").cloned().unwrap_or(json!({}));
    if !settings_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let retry_in = settings_in.get("retry").cloned().unwrap_or(json!({}));
    if !retry_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let script = args
        .get("script")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing script".to_string())?
        .clone();
    let provider_turns = args
        .get("providerTurns")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing providerTurns".to_string())?
        .clone();
    let auth_in = config.get("auth").cloned();
    if let Some(auth) = &auth_in {
        if !auth.is_null() && !auth.is_object() {
            return Err("invalid auth".to_string());
        }
    }
    let physical_models = config
        .get("physicalModels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for model in &physical_models {
        if !model.is_object() {
            return Err("invalid physicalModels".to_string());
        }
    }

    let retry = RetrySettings {
        enabled: retry_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        max_retries: retry_in.get("maxRetries").and_then(Value::as_f64).unwrap_or(3.0),
        base_delay_ms: retry_in.get("baseDelayMs").and_then(Value::as_f64).unwrap_or(2000.0),
        max_agent_delay_ms: pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
    };

    let now_ms_i = now_ms as i64;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(vec![now_ms], vec![]));
    let uuid_counter = Arc::new(Mutex::new(0usize));
    let uuid_v4 = {
        let counter = Arc::clone(&uuid_counter);
        Arc::new(move || {
            let mut n = counter.lock().expect("uuid counter");
            *n += 1;
            format!("uid{:05}", n)
        })
    };
    let uuid_v7: pi_core::session_manager::UuidGen =
        Arc::new(|| "stats-session".to_string());
    let now_gen = Arc::new(move || now_ms_i);
    let gen = Generators::with_gens(uuid_v7, uuid_v4, now_gen);

    let session_manager = SessionManager::in_memory(HARNESS_CWD, None, entries, &gen)?;
    let projected = session_manager
        .build_session_projection()
        .messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect::<Vec<Value>>();

    let prompts_sent: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let provider_cursor = Arc::new(Mutex::new(0usize));
    let stream_fn: Arc<pi_agent::LoopStreamFn> = {
        let turns = provider_turns;
        let cursor = Arc::clone(&provider_cursor);
        let sink = Arc::clone(&prompts_sent);
        Arc::new(move |model: &Value, context: &Value, options: &Value| {
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
            let sink = Arc::clone(&sink);
            let model = model.clone();
            let context = context.clone();
            let options = options.clone();
            Box::pin(async move {
                let index = {
                    let mut cursor = cursor.lock().expect("provider cursor");
                    let value = *cursor;
                    *cursor += 1;
                    value
                };
                let turn = match turns.get(index) {
                    Some(turn) => turn.clone(),
                    None => return Err("provider script exhausted".to_string()),
                };
                sink.lock().expect("prompts sent").push(json!({
                    "messages": context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())),
                    "model": model,
                    "options": options,
                }));
                Ok(pi_agent::provider::StreamOutput {
                    events: turn
                        .get("events")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into(),
                    result: turn.get("result").cloned().unwrap_or(Value::Null),
                })
            }) as BoxFuture<'static, Result<pi_agent::provider::StreamOutput, String>>
        })
    };

    let agent = Agent::new(AgentOptions {
        messages: projected,
        tools: Vec::new(),
        system_prompt: None,
        model: config.get("model").filter(|m| !m.is_null()).cloned().unwrap_or(Value::Null),
        thinking_level: "off".to_string(),
        steering_mode: Default::default(),
        follow_up_mode: Default::default(),
        stream_fn,
        tool_execution: Default::default(),
        clock: Arc::clone(&clock),
    });

    let request_auth = auth_in.as_ref().and_then(|auth| {
        if auth.is_null() {
            return None;
        }
        let inner = auth.get("auth").cloned().unwrap_or(json!({}));
        Some(RequestAuth {
            base_url: inner.get("baseUrl").and_then(Value::as_str).map(str::to_string),
            api_key: inner.get("apiKey").and_then(Value::as_str).map(str::to_string),
            headers: inner.get("headers").cloned(),
            env: auth.get("env").cloned().filter(|env| !env.is_null()),
        })
    });
    let physical_lookup: Option<Arc<dyn Fn(&str, &str) -> Option<Value> + Send + Sync>> = {
        let models = physical_models;
        Some(Arc::new(move |provider: &str, id: &str| {
            models
                .iter()
                .find(|m| {
                    m.get("provider").and_then(Value::as_str) == Some(provider)
                        && m.get("id").and_then(Value::as_str) == Some(id)
                })
                .and_then(|m| m.get("model").cloned())
        }))
    };

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(StatsHarnessSettings { retry }) as Arc<dyn SessionSettingsStore>,
        extension_host: Arc::new(StatsHarnessHost) as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: None,
        cwd: HARNESS_CWD.to_string(),
        custom_tools: Vec::new(),
        initial_active_tool_names: None,
        uses_default_tools: false,
        default_tool_modifiers: Vec::new(),
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: Some(Vec::new()),
        model_auth: Some(Arc::new(StatsHarnessAuth { auth: request_auth })),
        skills: Vec::new(),
        prompt_templates: Vec::new(),
        system_prompt: Some(system_prompt),
        model_lookup: None,
        scoped_models: Vec::new(),
        model_routes: None,
        physical_model_lookup: physical_lookup,
        sleeper: Some(Arc::new(ImmediateSleeper)),
    });

    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = Arc::clone(&session_events);
        session.subscribe(Box::new(move |event: &Value| {
            sink.lock().expect("session events").push(event.clone());
        }));
    }

    pi_agent::run_current_thread(async move {
        let signal = pi_agent::host::AbortHandle::new();
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(&mut session, op, process_cwd, &signal).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }
        let entries = session.session_manager.get_entries();
        Ok(json!({
            "result": {
                "results": results,
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
                "entries": entries,
            }
        }))
    })
}

async fn run_op(
    session: &mut AgentSession,
    op: &Value,
    process_cwd: &str,
    signal: &pi_agent::host::AbortHandle,
) -> Result<Value, String> {
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    match name {
        "getSessionStats" => session.get_session_stats(),
        "getContextUsage" => Ok(session.get_context_usage()?.unwrap_or(Value::Null)),
        "exportToJsonl" => {
            let path = obj.get("path").and_then(Value::as_str).map(str::to_string);
            let resolved = session.export_to_jsonl(path.as_deref(), process_cwd);
            let content = std::fs::read_to_string(&resolved)
                .map_err(|e| format!("failed to read export: {}", e))?;
            let _ = std::fs::remove_file(&resolved);
            Ok(json!({ "path": resolved, "content": content }))
        }
        "summarizeForBugReport" => {
            let hint = obj.get("hint").and_then(Value::as_str).map(str::to_string);
            let text = session.summarize_for_bug_report(hint.as_deref(), signal).await?;
            Ok(json!(text))
        }
        _ => Err(format!("unknown agent-session-stats op: {}", name)),
    }
}
