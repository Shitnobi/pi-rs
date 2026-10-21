//! Parity-harness entry for the `agent-session-retry` protocol: the
//! auto-retry slice of `pi_session::AgentSession` (TS `dist/core/
//! agent-session.js` — `_prepareRetry`/`_omitRecoveryAttempt`/
//! `abortRetry`/`isRetrying`), driven end-to-end over a real `Agent` with
//! a scripted provider. TS mirror: parity/ts-harness.js
//! `agent-session-retry` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "settings"?: { "retry"?: {...} },
//!               "model"?: object|null, "entries": [...],
//!               "systemPrompt": string },
//!   "sleepMode"?: "immediate"|"pending",
//!   "script": [ { "op": "prompt"|"promptStart", "text" } |
//!               { "op": "waitEvent", "eventType" } |
//!               { "op": "abortRetry" } | { "op": "abortTap" } |
//!               { "op": "abort" } | { "op": "waitForIdle" } |
//!               { "op": "awaitPrompt" } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ]
//! }
//! ```
//!
//! Result: `{ results, sessionEvents, entries, sleepLog,
//! promptsSentToAgent, isRetryingAtEnd, extensionEvents, extensionErrors }`.
//!
//! **Sleep seam:** the injected recording sleeper stands in for TS's
//! patched `setTimeout` — "immediate" records `{ms}` and resolves at once
//! (fast-forward); "pending" records `{ms}` and holds the sleep until the
//! retry abort handle fires, mirroring the TS patch's never-firing timer.
//!
//! **Concurrency model:** `promptStart` fires a prompt without awaiting
//! it (TS pushes the floating promise). The Rust session lives behind an
//! `Arc<tokio::sync::Mutex>` so the spawned prompt task can hold it for
//! the run's duration while `waitEvent` polls the shared event log and
//! `abortRetry`/`abortTap` reach the session through the shared abort
//! taps (TS: `session.abortRetry()` / `void session.abort()`).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{Aborted, Sleeper};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions, Clock, FixedClock};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, SessionSettingsStore,
};

const HARNESS_CWD: &str = "/proj";

struct RetryHarnessSettings {
    retry: RetrySettings,
}

impl SessionSettingsStore for RetryHarnessSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
}

/// Recording fake `_extensionRunner` (no handlers; TS mirror has
/// `hasHandlers: () => false`).
struct RetryExtensionHost {
    events: Mutex<Vec<Value>>,
    errors: Mutex<Vec<Value>>,
}

impl ExtensionHost for RetryExtensionHost {
    fn has_handlers(&self, _event: &str) -> bool {
        false
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        self.events.lock().expect("extension log").push(event);
        Box::pin(async {})
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

/// The patched-`setTimeout` mirror: records `{ms}` per retry sleep.
/// "pending" holds the sleep until the retry abort handle fires (TS: the
/// patched timer never runs; only the abort listener rejects).
struct RecordingSleeper {
    log: Arc<Mutex<Vec<Value>>>,
    pending: bool,
}

impl Sleeper for RecordingSleeper {
    fn sleep<'a>(&'a self, ms: u64, signal: &'a pi_agent::host::AbortHandle) -> BoxFuture<'a, Result<(), Aborted>> {
        let log = Arc::clone(&self.log);
        let pending = self.pending;
        Box::pin(async move {
            // TS sleep(): the aborted check precedes setTimeout.
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

pub fn run_agent_session_retry_harness(args: &Value) -> Result<Value, String> {
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

    // TS `SettingsManager.inMemory({retry}).getRetrySettings()` defaults.
    let retry = RetrySettings {
        enabled: retry_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        max_retries: retry_in.get("maxRetries").and_then(Value::as_f64).unwrap_or(3.0),
        base_delay_ms: retry_in.get("baseDelayMs").and_then(Value::as_f64).unwrap_or(2000.0),
        max_agent_delay_ms: retry_in
            .get("maxAgentDelayMs")
            .and_then(Value::as_f64)
            .unwrap_or(pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
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
    let uuid_v7 = {
        let counter = Arc::clone(&uuid_counter);
        Arc::new(move || {
            let mut n = counter.lock().expect("uuid counter");
            *n += 1;
            format!("uid{:05}", n)
        })
    };
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

    let host = Arc::new(RetryExtensionHost {
        events: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
    });
    let sleep_log: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sleeper: Arc<dyn Sleeper> = Arc::new(RecordingSleeper {
        log: Arc::clone(&sleep_log),
        pending: sleep_mode == "pending",
    });

    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let abort_retry_tap;
    let abort_tap;
    let session = {
        let mut session = AgentSession::new(AgentSessionConfig {
            agent,
            session_manager,
            settings: Arc::new(RetryHarnessSettings { retry }),
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
            model_auth: None,
            skills: Vec::new(),
            prompt_templates: Vec::new(),
            system_prompt: Some(system_prompt),
            model_lookup: None,
            scoped_models: Vec::new(),
            model_routes: None,
        physical_model_lookup: None,
            sleeper: Some(Arc::clone(&sleeper)),
        });
        {
            let sink = Arc::clone(&session_events);
            session.subscribe(Box::new(move |event: &Value| {
                sink.lock().expect("session events").push(event.clone());
            }));
        }
        abort_retry_tap = session.abort_retry_tap();
        abort_tap = session.abort_tap();
        session
    };

    // The session lives behind a mutex so `promptStart` can hold it for a
    // whole run while waitEvent/abortRetry/abortTap proceed (TS: the
    // floating prompt promise).
    let session = Arc::new(tokio::sync::Mutex::new(session));

    pi_agent::run_current_thread(async move {
        let mut pending: Vec<tokio::sync::oneshot::Receiver<Result<(), String>>> = Vec::new();
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(Arc::clone(&session), op, &session_events, &abort_retry_tap, &abort_tap, &mut pending).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }

        let is_retrying_at_end = session.lock().await.is_retrying();
        Ok(json!({
            "result": {
                "results": results,
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "entries": session.lock().await.session_manager.get_entries(),
                "sleepLog": sleep_log.lock().expect("sleep log").clone(),
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
                "isRetryingAtEnd": is_retrying_at_end,
                "extensionEvents": host.events.lock().expect("extension log").clone(),
                "extensionErrors": host.errors.lock().expect("extension errors").clone(),
            }
        }))
    })
}

/// Run one script op; `Err` becomes the op's `{thrown}` entry.
async fn run_op(
    session: Arc<tokio::sync::Mutex<AgentSession>>,
    op: &Value,
    session_events: &Arc<Mutex<Vec<Value>>>,
    abort_retry_tap: &Arc<dyn Fn() + Send + Sync>,
    abort_tap: &Arc<dyn Fn() + Send + Sync>,
    pending: &mut Vec<tokio::sync::oneshot::Receiver<Result<(), String>>>,
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
        "abortRetry" => {
            abort_retry_tap();
            Ok(Value::Null)
        }
        "abortTap" => {
            // TS `void session.abort()` — the synchronous prefix.
            abort_tap();
            Ok(Value::Null)
        }
        "abort" => {
            if !pending.is_empty() {
                return Err("cannot abort while a prompt is pending".to_string());
            }
            let mut guard = session.lock().await;
            guard.abort().await;
            Ok(Value::Null)
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
        "awaitPrompt" => {
            let rx = pending.pop().ok_or_else(|| "no pending prompt".to_string())?;
            match rx.await {
                Ok(Ok(())) => Ok(Value::Null),
                Ok(Err(message)) => Err(message),
                // The sender is dropped only on task panic.
                Err(_) => Err("no pending prompt".to_string()),
            }
        }
        other => Err(format!("unknown agent-session-retry op: {}", other)),
    }
}
