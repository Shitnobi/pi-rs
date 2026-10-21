//! Parity-harness entry for the `agent-session-tree` protocol: the
//! tree-navigation slice of `pi_session::AgentSession` (TS `navigateTree` /
//! `abortBranchSummary` / `getUserMessagesForForking`) over a scripted
//! provider (the same script drives the branch-summary summarizer call).
//! TS mirror: parity/ts-harness.js `agent-session-tree` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "model"?: object|null, "entries": [...],
//!               "systemPrompt": string,
//!               "settings"?: { "retry"?: {...}, "branchSummary"?: {...} } },
//!   "script": [ { "op": "navigateTree", "targetId": string,
//!                 "options"?: { "summarize"?: bool,
//!                               "customInstructions"?: string,
//!                               "replaceInstructions"?: bool,
//!                               "label"?: string } } |
//!               { "op": "abortBranchSummary" } |
//!               { "op": "getUserMessagesForForking" } |
//!               { "op": "setStreaming", "value": bool } |
//!               { "op": "isCompacting" } | { "op": "waitForIdle" } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ],
//!   "extensionHandlers"?: { "session_before_tree": [ { "cancel"?: bool,
//!     "summary"?: {...}, "customInstructions"?: string,
//!     "replaceInstructions"?: bool, "label"?: string,
//!     "abortBranchSummary"?: bool } ] }
//! }
//! ```
//!
//! Result: `{ results, sessionEvents, entries, extensionEvents,
//! promptsSentToAgent, isCompactingAtEnd }`.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{Clock, FixedClock, Sleeper};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::{BranchSummarySettings, RetrySettings};
use pi_session::agent_session::tree::NavigateTreeOptions;
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";
const DEFAULT_DOCS_PATH: &str = "/pi-docs";

/// Settings fake: `retry` + `branchSummary` seeds, TS in-memory defaults
/// elsewhere.
struct TreeHarnessSettings {
    retry: RetrySettings,
    branch_summary: BranchSummarySettings,
}

impl SessionSettingsStore for TreeHarnessSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
    fn branch_summary_settings(&self) -> BranchSummarySettings {
        self.branch_summary.clone()
    }
}

struct TreeHarnessAuth;

impl ModelAuthStore for TreeHarnessAuth {
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
        format!("No API key found for the provider. See {}/providers.md", DEFAULT_DOCS_PATH)
    }
}

/// Recording fake `_extensionRunner` with optional `session_before_tree`
/// handlers (mirrors the TS fake: per-handler field merging plus the
/// `abortBranchSummary` tap).
struct TreeHarnessHost {
    handlers: Vec<(String, Vec<Value>)>,
    events: Mutex<Vec<Value>>,
    /// Settable abort tap (TS: the fake runner closure captures `session`,
    /// which does not exist until after the session is constructed).
    branch_summary_abort_tap: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl TreeHarnessHost {
    fn set_abort_tap(&self, tap: Arc<dyn Fn() + Send + Sync>) {
        *self.branch_summary_abort_tap.lock().expect("abort tap") = Some(tap);
    }
    fn fire_abort_tap(&self) {
        if let Some(tap) = self.branch_summary_abort_tap.lock().expect("abort tap").as_ref() {
            tap();
        }
    }
}

impl ExtensionHost for TreeHarnessHost {
    fn has_handlers(&self, event: &str) -> bool {
        self.handlers.iter().any(|(type_, list)| type_ == event && !list.is_empty())
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.emit_session_before_tree(event).await;
        })
    }
    fn emit_session_before_tree<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
        self.events.lock().expect("extension log").push(event);
        let mut result: Option<Value> = None;
        if let Some((_, handlers)) = self.handlers.iter().find(|(type_, _)| *type_ == event_type) {
            for handler in handlers {
                if handler.get("abortBranchSummary").and_then(Value::as_bool) == Some(true) {
                    self.fire_abort_tap();
                }
                let mut merged = result.clone().unwrap_or(json!({}));
                let object = merged.as_object_mut().expect("merged object");
                // TS fake: `cancel` merges only when truthy (as `true`);
                // the other keys merge when present.
                if handler.get("cancel").and_then(Value::as_bool) == Some(true) {
                    object.insert("cancel".to_string(), json!(true));
                }
                for key in ["summary", "customInstructions", "replaceInstructions", "label"] {
                    if let Some(value) = handler.get(key) {
                        object.insert(key.to_string(), value.clone());
                    }
                }
                result = Some(Value::Object(std::mem::take(object)));
            }
        }
        Box::pin(async move { result })
    }
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        self.events.lock().expect("extension log").push(event);
        Box::pin(async { None })
    }
    fn emit_error(&self, _error: Value) {}
    fn invalidate(&self, _message: &str) {}
}

/// The patched-`setTimeout` mirror for the summarizer retry backoff
/// (fast-forward).
struct ImmediateSleeper;

impl Sleeper for ImmediateSleeper {
    fn sleep<'a>(&'a self, _ms: u64, signal: &'a pi_agent::host::AbortHandle) -> BoxFuture<'a, Result<(), pi_agent::host::Aborted>> {
        Box::pin(async move {
            if signal.is_aborted() {
                return Err(pi_agent::host::Aborted);
            }
            Ok(())
        })
    }
}

pub fn run_agent_session_tree_harness(args: &Value) -> Result<Value, String> {
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
    let branch_in = settings_in.get("branchSummary").cloned().unwrap_or(json!({}));
    if !branch_in.is_object() {
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
    let extension_handlers_in = args.get("extensionHandlers").cloned().unwrap_or(json!({}));
    if !extension_handlers_in.is_object() {
        return Err("invalid extensionHandlers".to_string());
    }
    let mut handlers: Vec<(String, Vec<Value>)> = Vec::new();
    if let Some(object) = extension_handlers_in.as_object() {
        for (type_, list) in object {
            let Value::Array(list) = list else { continue };
            handlers.push((type_.clone(), list.clone()));
        }
    }

    // TS `SettingsManager.inMemory({retry, branchSummary})` defaults.
    let retry = RetrySettings {
        enabled: retry_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        max_retries: retry_in.get("maxRetries").and_then(Value::as_f64).unwrap_or(3.0),
        base_delay_ms: retry_in.get("baseDelayMs").and_then(Value::as_f64).unwrap_or(2000.0),
        max_agent_delay_ms: pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
    };
    let branch_summary = BranchSummarySettings {
        reserve_tokens: branch_in
            .get("reserveTokens")
            .and_then(Value::as_f64)
            .unwrap_or(16384.0),
        skip_prompt: branch_in.get("skipPrompt").and_then(Value::as_bool).unwrap_or(false),
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
    let uuid_v7: pi_core::session_manager::UuidGen = Arc::new(|| "summarization-session".to_string());
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
        Arc::new(move |model: &Value, context: &Value, _options: &Value| {
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
            let sink = Arc::clone(&sink);
            let model = model.clone();
            let context = context.clone();
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
                sink.lock().expect("prompts sent").push(
                    context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())),
                );
                let _ = model;
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

    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    let host = Arc::new(TreeHarnessHost {
        handlers,
        events: Mutex::new(Vec::new()),
        branch_summary_abort_tap: Mutex::new(None),
    });

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(TreeHarnessSettings { retry, branch_summary }) as Arc<dyn SessionSettingsStore>,
        extension_host: Arc::clone(&host) as Arc<dyn ExtensionHost>,
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
        model_auth: Some(Arc::new(TreeHarnessAuth)),
        skills: Vec::new(),
        prompt_templates: Vec::new(),
        system_prompt: Some(system_prompt),
        model_lookup: None,
        scoped_models: Vec::new(),
        model_routes: None,
        physical_model_lookup: None,
        sleeper: Some(Arc::new(ImmediateSleeper)),
    });
    // The host's abort tap needs the session (TS: the fake runner closure
    // captures `session`).
    host.set_abort_tap(session.branch_summary_abort_tap());
    {
        let sink = Arc::clone(&session_events);
        session.subscribe(Box::new(move |event: &Value| {
            sink.lock().expect("session events").push(event.clone());
        }));
    }

    pi_agent::run_current_thread(async move {
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(&mut session, op).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }
        let is_compacting_at_end = session.is_compacting();
        let entries = session.session_manager.get_entries();
        Ok(json!({
            "result": {
                "results": results,
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "entries": entries,
                "extensionEvents": host.events.lock().expect("extension events").clone(),
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
                "isCompactingAtEnd": is_compacting_at_end,
            }
        }))
    })
}

async fn run_op(session: &mut AgentSession, op: &Value) -> Result<Value, String> {
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    match name {
        "navigateTree" => {
            let target_id = obj
                .get("targetId")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing targetId".to_string())?
                .to_string();
            let options_in = obj.get("options").filter(|o| !o.is_null());
            let mut options = NavigateTreeOptions::default();
            if let Some(options_in) = options_in {
                let Value::Object(options_in) = options_in else {
                    return Err("invalid options".to_string());
                };
                for (key, value) in options_in {
                    if value.is_null() {
                        continue;
                    }
                    match key.as_str() {
                        "summarize" | "replaceInstructions" => {
                            let Some(bool) = value.as_bool() else {
                                return Err("invalid options".to_string());
                            };
                            if key == "summarize" {
                                options.summarize = Some(bool);
                            } else {
                                options.replace_instructions = Some(bool);
                            }
                        }
                        "customInstructions" | "label" => {
                            let Some(text) = value.as_str() else {
                                return Err("invalid options".to_string());
                            };
                            if key == "customInstructions" {
                                options.custom_instructions = Some(text.to_string());
                            } else {
                                options.label = Some(text.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(session.navigate_tree(&target_id, options).await?)
        }
        "abortBranchSummary" => {
            session.abort_branch_summary();
            Ok(Value::Null)
        }
        "getUserMessagesForForking" => Ok(Value::Array(session.get_user_messages_for_forking())),
        "setStreaming" => {
            let value =
                obj.get("value").and_then(Value::as_bool).ok_or_else(|| "missing value".to_string())?;
            session.set_agent_run_active(value);
            Ok(Value::Null)
        }
        "isCompacting" => Ok(json!(session.is_compacting())),
        "waitForIdle" => {
            session.wait_for_idle().await;
            Ok(Value::Null)
        }
        _ => Err(format!("unknown agent-session-tree op: {}", name)),
    }
}
