//! Parity-harness entry for the `agent-session-compaction` protocol: the
//! compaction slice of `pi_session::AgentSession` (TS `dist/core/
//! agent-session.js` — `_checkCompaction`/`_runAutoCompaction`/`compact`/
//! `abortCompaction`), driven end-to-end over a real `Agent` with a
//! scripted provider. The same provider script drives both agent turns
//! and the summarizer model call (`Agent::stream_fn` is the injected
//! fake). TS mirror: parity/ts-harness.js `agent-session-compaction`
//! entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "settings"?: { "retry"?: {...}, "compaction"?: {...} },
//!               "model"?: object|null, "entries": [...],
//!               "systemPrompt": string },
//!   "scenario": "overflow-retry"|"overflow-complete"|"threshold"|
//!               "manual"|"already-compacted"|"too-small",
//!   "sleepMode"?: "immediate"|"pending",
//!   "script": [ { "op": "prompt"|"promptStart", "text" } |
//!               { "op": "waitEvent", "eventType" } |
//!               { "op": "awaitPrompt" } | { "op": "abort" } |
//!               { "op": "abortTap" } | { "op": "abortCompaction" } |
//!               { "op": "compact", "customInstructions"?: string } |
//!               { "op": "waitForIdle" } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ],
//!   "extensionHandlers"?: { "session_before_compact":
//!       [{ "cancel"?: bool, "compaction"?: {...}, "abortCompaction"?: bool }],
//!       "session_compact_failed": [{}] }
//! }
//! ```
//!
//! Result: `{ results, compactionResult, error, sessionEvents, entries,
//! extensionEvents, extensionErrors, promptsSentToAgent, sleepLog,
//! isCompactingAtEnd }`.
//!
//! **Docs paths:** the no-model guidance message embeds `getDocsPath()`;
//! the Rust side uses `/pi-docs` and normalizes it to `<docs>` in thrown
//! messages (the TS entry substitutes its real docs path the same way).
//!
//! **Model-keyed compaction overrides** (`compaction.modelOverrides`)
//! are not exercised by the harness settings fake; the ordinary
//! `compaction` fields map straight through.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{Aborted, Sleeper};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions, Clock, FixedClock};
use pi_core::compaction::{default_compaction_settings, CompactionSettings};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";
const DEFAULT_DOCS_PATH: &str = "/pi-docs";

const SCENARIOS: [&str; 6] = [
    "overflow-retry",
    "overflow-complete",
    "threshold",
    "manual",
    "already-compacted",
    "too-small",
];

struct CompactionHarnessSettings {
    retry: RetrySettings,
    compaction: CompactionSettings,
}

impl SessionSettingsStore for CompactionHarnessSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
    fn compaction_settings(&self, _model: Option<(&str, &str)>) -> CompactionSettings {
        self.compaction.clone()
    }
}

fn provider_login_help(docs_path: &str) -> String {
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_string(),
        format!("  {}/providers.md", docs_path),
        format!("  {}/models.md", docs_path),
    ]
    .join("\n")
}

struct CompactionModelAuth {
    docs_path: String,
}

impl ModelAuthStore for CompactionModelAuth {
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
        format!(
            "No model selected.\n\n{}\n\nThen use /model to select a model.",
            provider_login_help(&self.docs_path)
        )
    }
    fn no_api_key_message(&self, provider: &str) -> String {
        let display = if provider == "unknown" { "the selected model" } else { provider };
        format!("No API key found for {}.\n\n{}", display, provider_login_help(&self.docs_path))
    }
}

/// Recording fake `_extensionRunner` with handler support. Handler lists
/// come from `extensionHandlers`; `session_before_compact` handlers may
/// `cancel`, supply a `compaction` result, or call `abortCompaction()`
/// mid-hook (through the late-bound tap installed after the session is
/// built). TS mirror: the `fakeRunner` in the TS entry.
struct CompactionExtensionHost {
    events: Mutex<Vec<Value>>,
    errors: Mutex<Vec<Value>>,
    handlers: Value,
    abort_tap: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl CompactionExtensionHost {
    fn handler_list(&self, event_type: &str) -> Vec<Value> {
        self.handlers
            .get(event_type)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

impl ExtensionHost for CompactionExtensionHost {
    fn has_handlers(&self, event: &str) -> bool {
        !self.handler_list(event).is_empty()
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        self.events.lock().expect("extension log").push(event);
        Box::pin(async {})
    }
    fn emit_session_before_compact<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        self.events.lock().expect("extension log").push(event);
        let mut result = serde_json::Map::new();
        for handler in self.handler_list("session_before_compact") {
            if handler.get("abortCompaction").and_then(Value::as_bool) == Some(true) {
                if let Some(tap) = self.abort_tap.lock().expect("abort tap").as_ref() {
                    tap();
                }
            }
            if handler.get("cancel").and_then(Value::as_bool) == Some(true) {
                result.insert("cancel".to_string(), json!(true));
            }
            if let Some(compaction) = handler.get("compaction") {
                result.insert("compaction".to_string(), compaction.clone());
            }
        }
        let result =
            if result.is_empty() { None } else { Some(Value::Object(result)) };
        Box::pin(async move { result })
    }
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        self.events.lock().expect("extension log").push(event);
        Box::pin(async { None })
    }
    fn emit_error(&self, error: Value) {
        self.errors.lock().expect("extension errors").push(error);
    }
    fn invalidate(&self, _message: &str) {}
}

/// The patched-`setTimeout` mirror (see `session_retry_harness.rs`).
struct RecordingSleeper {
    log: Arc<Mutex<Vec<Value>>>,
    pending: bool,
}

impl Sleeper for RecordingSleeper {
    fn sleep<'a>(&'a self, ms: u64, signal: &'a pi_agent::host::AbortHandle) -> BoxFuture<'a, Result<(), Aborted>> {
        let log = Arc::clone(&self.log);
        let pending = self.pending;
        Box::pin(async move {
            if signal.is_aborted() {
                return Err(Aborted);
            }
            log.lock().expect("sleep log").push(json!({ "ms": ms }));
            if !pending {
                return Ok(());
            }
            signal.aborted().await;
            Err(Aborted)
        })
    }
}

pub fn run_agent_session_compaction_harness(args: &Value) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;
    let config = args.get("config").cloned().unwrap_or(json!({}));
    if !config.is_object() {
        return Err("invalid config".to_string());
    }
    let settings_in = config.get("settings").cloned().unwrap_or(json!({}));
    if !settings_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let retry_in = settings_in.get("retry").cloned().unwrap_or(json!({}));
    if !retry_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let compaction_in = settings_in.get("compaction").cloned().unwrap_or(json!({}));
    if !compaction_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let system_prompt = match config.get("systemPrompt") {
        Some(Value::String(prompt)) => prompt.clone(),
        _ => return Err("invalid systemPrompt".to_string()),
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
    let scenario = args
        .get("scenario")
        .and_then(Value::as_str)
        .ok_or_else(|| "invalid scenario".to_string())?
        .to_string();
    if !SCENARIOS.contains(&scenario.as_str()) {
        return Err("invalid scenario".to_string());
    }
    let sleep_mode = args.get("sleepMode").and_then(Value::as_str).unwrap_or("immediate").to_string();
    if !matches!(sleep_mode.as_str(), "immediate" | "pending") {
        return Err("invalid sleepMode".to_string());
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
    let extension_handlers = args.get("extensionHandlers").cloned().unwrap_or(json!({}));
    if !extension_handlers.is_object() {
        return Err("invalid extensionHandlers".to_string());
    }

    // TS `SettingsManager.inMemory({retry, compaction})` defaults.
    let retry = RetrySettings {
        enabled: retry_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        max_retries: retry_in.get("maxRetries").and_then(Value::as_f64).unwrap_or(3.0),
        base_delay_ms: retry_in.get("baseDelayMs").and_then(Value::as_f64).unwrap_or(2000.0),
        max_agent_delay_ms: retry_in
            .get("maxAgentDelayMs")
            .and_then(Value::as_f64)
            .unwrap_or(pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
    };
    let default_compaction = default_compaction_settings();
    let compaction = CompactionSettings {
        enabled: compaction_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        reserve_tokens: compaction_in
            .get("reserveTokens")
            .and_then(Value::as_f64)
            .unwrap_or(default_compaction.reserve_tokens),
        keep_recent_tokens: compaction_in
            .get("keepRecentTokens")
            .and_then(Value::as_f64)
            .unwrap_or(default_compaction.keep_recent_tokens),
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
    // TS `uuidv7()` (the uuid package) is random and never consults the
    // parity override; only SessionManager's generateId (uuidv4) is pinned.
    // The v7 seam feeds just the summarizer sessionId, which is unobservable.
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
        Arc::new(move |_model: &Value, context: &Value, _options: &Value| {
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
            let sink = Arc::clone(&sink);
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
                sink.lock()
                    .expect("prompts sent")
                    .push(context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())));
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

    let host = Arc::new(CompactionExtensionHost {
        events: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
        handlers: extension_handlers,
        abort_tap: Mutex::new(None),
    });
    let sleep_log: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sleeper: Arc<dyn Sleeper> = Arc::new(RecordingSleeper {
        log: Arc::clone(&sleep_log),
        pending: sleep_mode == "pending",
    });

    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let compaction_abort_tap;
    let session = {
        let mut session = AgentSession::new(AgentSessionConfig {
            agent,
            session_manager,
            settings: Arc::new(CompactionHarnessSettings { retry, compaction }),
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
            model_auth: Some(Arc::new(CompactionModelAuth { docs_path: DEFAULT_DOCS_PATH.to_string() })),
            skills: Vec::new(),
            prompt_templates: Vec::new(),
            system_prompt: Some(system_prompt),
            model_lookup: None,
            sleeper: Some(Arc::clone(&sleeper)),
        });
        {
            let sink = Arc::clone(&session_events);
            session.subscribe(Box::new(move |event: &Value| {
                sink.lock().expect("session events").push(event.clone());
            }));
        }
        compaction_abort_tap = session.compaction_abort_tap();
        *host.abort_tap.lock().expect("abort tap") = Some(compaction_abort_tap.clone());
        session
    };

    // The session lives behind a mutex so `promptStart` can hold it for a
    // whole run while waitEvent/abortTap/abortCompaction proceed (TS: the
    // floating prompt promise).
    let session = Arc::new(tokio::sync::Mutex::new(session));

    pi_agent::run_current_thread(async move {
        let docs_path = DEFAULT_DOCS_PATH.to_string();
        let normalize = |message: &str| message.replace(&docs_path, "<docs>");
        let mut pending: Vec<tokio::sync::oneshot::Receiver<Result<(), String>>> = Vec::new();
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        let mut compaction_result: Value = Value::Null;
        let mut error: Option<String> = None;
        for op in &script {
            let outcome = run_op(
                Arc::clone(&session),
                op,
                &session_events,
                &compaction_abort_tap,
                &mut pending,
                &mut compaction_result,
                &mut error,
            )
            .await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": normalize(&message) }),
            });
        }

        let is_compacting_at_end = session.lock().await.is_compacting();
        Ok(json!({
            "result": {
                "results": results,
                "compactionResult": compaction_result,
                "error": match &error {
                    Some(message) => json!(normalize(message)),
                    None => Value::Null,
                },
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "entries": session.lock().await.session_manager.get_entries(),
                "extensionEvents": host.events.lock().expect("extension log").clone(),
                "extensionErrors": host.errors.lock().expect("extension errors").clone(),
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
                "sleepLog": sleep_log.lock().expect("sleep log").clone(),
                "isCompactingAtEnd": is_compacting_at_end,
            }
        }))
    })
}

/// Run one script op; `Err` becomes the op's `{thrown}` entry (docs paths
/// normalized).
async fn run_op(
    session: Arc<tokio::sync::Mutex<AgentSession>>,
    op: &Value,
    session_events: &Arc<Mutex<Vec<Value>>>,
    compaction_abort_tap: &Arc<dyn Fn() + Send + Sync>,
    pending: &mut Vec<tokio::sync::oneshot::Receiver<Result<(), String>>>,
    compaction_result: &mut Value,
    error: &mut Option<String>,
) -> Result<Value, String> {
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    match name {
        "prompt" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing text".to_string())?;
            if !pending.is_empty() {
                return Err("cannot prompt while a prompt is pending".to_string());
            }
            let mut guard = session.lock().await;
            guard.prompt(text, Default::default()).await?;
            Ok(Value::Null)
        }
        "promptStart" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing text".to_string())?
                .to_string();
            let session = Arc::clone(&session);
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let mut guard = session.lock().await;
                let result = guard.prompt(&text, Default::default()).await;
                drop(guard);
                let _ = tx.send(result);
            });
            pending.push(rx);
            Ok(Value::Null)
        }
        "waitEvent" => {
            let event_type = obj
                .get("eventType")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing eventType".to_string())?
                .to_string();
            loop {
                let found = session_events
                    .lock()
                    .expect("session events")
                    .iter()
                    .any(|event| event.get("type").and_then(Value::as_str) == Some(event_type.as_str()));
                if found {
                    break;
                }
                // Let the spawned prompt run advance (TS: setImmediate).
                tokio::task::yield_now().await;
                tokio::time::sleep(std::time::Duration::from_millis(0)).await;
            }
            Ok(Value::Null)
        }
        "awaitPrompt" => {
            let rx = pending.pop().ok_or_else(|| "no pending prompt".to_string())?;
            match rx.await {
                Ok(Ok(())) => Ok(Value::Null),
                Ok(Err(message)) => Err(message),
                // The sender is dropped only on task panic.
                Err(_) => Err("no pending prompt".to_string()),
            }
        }
        "abort" => {
            if !pending.is_empty() {
                return Err("cannot abort while a prompt is pending".to_string());
            }
            let mut guard = session.lock().await;
            guard.abort().await;
            Ok(Value::Null)
        }
        "abortTap" => {
            // TS `void session.abort()` — the synchronous prefix.
            let tap = session.lock().await.abort_tap();
            tap();
            Ok(Value::Null)
        }
        "abortCompaction" => {
            // TS `session.abortCompaction()` — reached through the shared
            // tap so a pending `promptStart` run (which holds the session
            // mutex) is not blocked.
            compaction_abort_tap();
            Ok(Value::Null)
        }
        "compact" => {
            if !pending.is_empty() {
                return Err("cannot compact while a prompt is pending".to_string());
            }
            let custom_instructions = match obj.get("customInstructions") {
                Some(Value::String(text)) => Some(text.clone()),
                Some(other) if !other.is_null() => {
                    return Err("invalid customInstructions".to_string())
                }
                _ => None,
            };
            let mut guard = session.lock().await;
            match guard.compact(custom_instructions.as_deref()).await {
                Ok(result) => {
                    *compaction_result = result;
                    *error = None;
                    Ok(Value::Null)
                }
                Err(message) => {
                    *compaction_result = Value::Null;
                    *error = Some(message.clone());
                    Err(message)
                }
            }
        }
        "waitForIdle" => {
            if !pending.is_empty() {
                return Err("cannot waitForIdle while a prompt is pending".to_string());
            }
            let guard = session.lock().await;
            let wait = guard.wait_for_idle();
            drop(guard);
            wait.await;
            Ok(Value::Null)
        }
        other => Err(format!("unknown agent-session-compaction op: {}", other)),
    }
}
