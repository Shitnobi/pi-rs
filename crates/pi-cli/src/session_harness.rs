//! Parity-harness entry for the `agent-session-events` protocol: the event
//! slice of `pi_session::AgentSession` (TS `packages/coding-agent`
//! `class AgentSession` — `_handleAgentEvent`, queue bookkeeping,
//! `sendCustomMessage`, `_emitAgentSettled`, `_findPersistedMessageEntryId`).
//!
//! Wire protocol (TS mirror in parity/ts-harness.js, `agent-session-events`
//! entry): bare JSON args on stdin, `{"result": {...}}` on stdout; per-case
//! exceptions surface in-band as `{"error": msg}` with byte-identical
//! messages on both sides.
//!
//! args:
//! ```json
//! {
//!   "messages":   [...],               // identity pool (see below)
//!   "nowMs":      number,              // pinned Date.now / clock / now_iso
//!   "stateCount": number,              // pool.slice(0, stateCount) → agent.state.messages
//!   "entries":    [...],               // session entries, header FIRST; "message"
//!                                       //   entries carry messageIndex instead of message
//!   "model":      object | null,       // agent.state.model (default DEFAULT_MODEL)
//!   "settings":   { "retry": { "enabled"?, "maxRetries"? } },
//!   "extensionConfig": { "messageEndReplacement"?: message },
//!   "nestedRecords": { "<toolCallId>": { "calls"?, "usage"? } },
//!   "ops":        [ { "op": ... }, ... ]
//! }
//! ```
//!
//! **Message identity (plan §2 trap 1):** TS keys `_entryIdsByMessage`
//! (WeakMap), `_boundaryDispatchedMessages` (WeakSet) and the
//! `entry.message === message` branch scan by object identity. The harness
//! declares a pool of messages and references them by index; the TS side
//! wires pool objects into events, agent state, and seeded entries, so both
//! sides observe the same identity graph through the arena port.
//!
//! Case conventions (divergence guards):
//! - `entries[0]` must be the `session` header (no `newSession()` id).
//! - Entry ids must not look like `uuid-N` (appended entries get those).
//! - `messageEndReplacement` targets and `nestedRecords` toolResult messages
//!   must not be referenced by a seeded entry's `messageIndex` (in-place
//!   mutation would be visible through the TS entry's live reference only).
//! - `sendCustomMessage` always passes boolean `display`; `details` is an
//!   object or absent (never null).
//! - `triggerTurn: true` needs the run pipeline (tranche 11) — not driven.
//!
//! Ops and their logged results:
//! - `{op:"agentEvent", event}` → `null` — event JSON with `messageIndex` /
//!   `messageIndices` / `toolResultIndices` referencing the pool
//! - `{op:"queueSteer"|"queueFollowUp", text}` → `null`
//! - `{op:"setStreaming", value}` → `null` (any-cast `_isAgentRunActive`)
//! - `{op:"setRetryAttempt", value}` → `null` (any-cast `_retryAttempt`)
//! - `{op:"requestAbort"}` → `null` (any-cast `_agentRunAbortRequested = true`)
//! - `{op:"sendCustomMessage", customType, content, display, details?, triggerTurn?, deliverAs?}` → `null`
//! - `{op:"findPersisted", messageIndex}` → entry id | `null`
//! - `{op:"emitAgentSettled"}` → `null`
//! - `{op:"waitForIdle", tag}` → `null` (resolution reported in `idleWaits`)
//! - `{op:"deferAction", tag}` → `null` (deferred-Log stand-in)
//! - `{op:"unsubscribe"}` → `null`
//!
//! Result:
//! ```json
//! { "results", "sessionEvents", "extensionEvents", "entries",
//!   "queueState", "deferredLog", "idleWaits" }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions, Clock, FixedClock};
use pi_core::session_manager::{Generators, SessionManager};
use pi_core::settings_manager::RetrySettings;
use pi_session::agent_session::events::SessionAgentEvent;
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, NestedCallsStore, SessionSettingsStore,
};

/// Fixed cwd both harnesses construct the session with (unobservable; the
/// resource loader and session manager never touch the filesystem here).
const HARNESS_CWD: &str = "/proj";

// ============================================================================
// Fakes (TS harness swaps the same seams via any-cast)
// ============================================================================

/// TS `settingsManager.getRetrySettings()` over `SettingsManager.inMemory`.
struct FixedSettings {
    retry: RetrySettings,
}

impl SessionSettingsStore for FixedSettings {
    fn retry_settings(&self) -> RetrySettings {
        self.retry.clone()
    }
}

/// Recording stand-in for `_extensionRunner` (TS fake: `emit` logs,
/// `emitMessageEnd` logs and returns the configured replacement).
struct RecordingExtensionHost {
    log: Mutex<Vec<Value>>,
    message_end_replacement: Option<Value>,
}

impl ExtensionHost for RecordingExtensionHost {
    fn has_handlers(&self, _event: &str) -> bool {
        false
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        self.log.lock().expect("extension log").push(event);
        Box::pin(async {})
    }
    fn emit_message_end<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        self.log.lock().expect("extension log").push(event);
        let replacement = self.message_end_replacement.clone();
        Box::pin(async move { replacement })
    }
    fn emit_error(&self, _error: Value) {}
    fn invalidate(&self, _message: &str) {}
}

/// Preloaded stand-in for `_nestedToolCalls` (`takeRecord`/`clear`).
struct PreloadedNestedCallsStore {
    records: Mutex<HashMap<String, Value>>,
}

impl NestedCallsStore for PreloadedNestedCallsStore {
    fn take_record(&self, tool_call_id: &str) -> Option<Value> {
        self.records.lock().expect("nested records").remove(tool_call_id)
    }
    fn clear(&self) {
        self.records.lock().expect("nested records").clear();
    }
}

// ============================================================================
// Validation helpers (byte-exact TS mirror)
// ============================================================================


/// Pool index in range; error text chosen by the caller.
fn pool_index(v: Option<&Value>, len: usize, err: &str) -> Result<usize, String> {
    let raw = v.and_then(Value::as_i64).filter(|i| *i >= 0).ok_or_else(|| err.to_string())?;
    if raw as usize >= len {
        return Err(err.to_string());
    }
    Ok(raw as usize)
}

fn index_list(v: Option<&Value>, len: usize, err: &str) -> Result<Vec<usize>, String> {
    let array = v.and_then(Value::as_array).ok_or_else(|| err.to_string())?;
    array.iter().map(|i| pool_index(Some(i), len, err)).collect()
}

/// Build the `SessionAgentEvent` from the op's `event` JSON (pool indices
/// resolved to arena ids — the arena is seeded with the pool in order, so
/// pool index == MsgId).
fn build_agent_event(op: &Value, len: usize) -> Result<SessionAgentEvent, String> {
    let event = op.get("event").ok_or_else(|| "missing event".to_string())?;
    let obj = event.as_object().ok_or_else(|| "missing event".to_string())?;
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing event type".to_string())?;
    let idx = |key: &str| pool_index(obj.get(key), len, "invalid messageIndex");
    match kind {
        "agent_start" => Ok(SessionAgentEvent::AgentStart),
        "agent_end" => Ok(SessionAgentEvent::AgentEnd {
            messages: index_list(obj.get("messageIndices"), len, "invalid messageIndices")?,
        }),
        "turn_start" => Ok(SessionAgentEvent::TurnStart),
        "turn_end" => Ok(SessionAgentEvent::TurnEnd {
            message: idx("messageIndex")?,
            tool_results: index_list(obj.get("toolResultIndices"), len, "invalid toolResultIndices")?,
        }),
        "message_start" => Ok(SessionAgentEvent::MessageStart { message: idx("messageIndex")? }),
        "message_update" => Ok(SessionAgentEvent::MessageUpdate {
            message: idx("messageIndex")?,
            assistant_message_event: obj
                .get("assistantMessageEvent")
                .cloned()
                .ok_or_else(|| "missing assistantMessageEvent".to_string())?,
        }),
        "message_end" => Ok(SessionAgentEvent::MessageEnd { message: idx("messageIndex")? }),
        "tool_execution_start" => Ok(SessionAgentEvent::ToolExecutionStart {
            tool_call_id: obj
                .get("toolCallId")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing toolCallId".to_string())?
                .to_string(),
            tool_name: obj
                .get("toolName")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing toolName".to_string())?
                .to_string(),
            args: obj.get("args").cloned().ok_or_else(|| "missing args".to_string())?,
        }),
        "tool_execution_update" => Ok(SessionAgentEvent::ToolExecutionUpdate {
            tool_call_id: obj
                .get("toolCallId")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing toolCallId".to_string())?
                .to_string(),
            tool_name: obj
                .get("toolName")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing toolName".to_string())?
                .to_string(),
            args: obj.get("args").cloned().ok_or_else(|| "missing args".to_string())?,
            partial_result: obj
                .get("partialResult")
                .cloned()
                .ok_or_else(|| "missing partialResult".to_string())?,
        }),
        "tool_execution_end" => {
            let duration_ms = match obj.get("durationMs") {
                Some(Value::Null) | None => None,
                Some(v) => Some(
                    v.as_f64().ok_or_else(|| "invalid durationMs".to_string())?,
                ),
            };
            Ok(SessionAgentEvent::ToolExecutionEnd {
                tool_call_id: obj
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing toolCallId".to_string())?
                    .to_string(),
                tool_name: obj
                    .get("toolName")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "missing toolName".to_string())?
                    .to_string(),
                result: obj.get("result").cloned().ok_or_else(|| "missing result".to_string())?,
                is_error: obj
                    .get("isError")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| "missing isError".to_string())?,
                duration_ms,
            })
        }
        other => Err(format!("unknown agent event type: {}", other)),
    }
}

// ============================================================================
// Harness entry
// ============================================================================

pub fn run_agent_session_events_harness(args: &Value) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly.
    let messages = args
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing messages".to_string())?;
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;
    let state_count = args
        .get("stateCount")
        .and_then(Value::as_f64)
        .filter(|v| v.fract() == 0.0)
        .ok_or_else(|| "missing stateCount".to_string())?;
    if state_count < 0.0 || state_count > messages.len() as f64 {
        return Err("invalid stateCount".to_string());
    }
    let state_count = state_count as usize;
    let entries = args
        .get("entries")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .filter(|e| e.first().and_then(|h| h.get("type")).and_then(Value::as_str) == Some("session"))
        .ok_or_else(|| "missing entries".to_string())?;
    if let Some(model) = args.get("model") {
        if !model.is_null() && !model.is_object() {
            return Err("invalid model".to_string());
        }
    }
    let ops = args
        .get("ops")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing ops".to_string())?;

    // Entries: message entries carry `messageIndex` → resolve to the pool
    // message (TS wires the pool OBJECT so `entry.message === message`).
    let mut wired_entries: Vec<Value> = Vec::with_capacity(entries.len());
    let mut entry_message_indices: Vec<(String, usize)> = Vec::new();
    for entry in entries {
        let obj = entry
            .as_object()
            .ok_or_else(|| "entries entries must be objects".to_string())?;
        let mut wired = Map::new();
        for (key, value) in obj {
            if key == "messageIndex" {
                let index = pool_index(Some(value), messages.len(), "invalid messageIndex")?;
                wired.insert("message".to_string(), messages[index].clone());
                if let Some(id) = wired.get("id").and_then(Value::as_str) {
                    entry_message_indices.push((id.to_string(), index));
                }
            } else {
                wired.insert(key.clone(), value.clone());
            }
        }
        wired_entries.push(Value::Object(wired));
    }

    // Fakes.
    let settings_in = args.get("settings").cloned().unwrap_or(json!({}));
    if !settings_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let retry_in = settings_in.get("retry").cloned().unwrap_or(json!({}));
    if !retry_in.is_object() {
        return Err("invalid settings".to_string());
    }
    let retry = RetrySettings {
        enabled: retry_in.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        max_retries: retry_in.get("maxRetries").and_then(Value::as_f64).unwrap_or(3.0),
        base_delay_ms: 2000.0,
        max_agent_delay_ms: 300000.0,
    };
    let extension_config = args.get("extensionConfig").cloned().unwrap_or(json!({}));
    if !extension_config.is_object() {
        return Err("invalid extensionConfig".to_string());
    }
    let message_end_replacement = extension_config.get("messageEndReplacement").cloned();
    let nested_records_in = args.get("nestedRecords").cloned().unwrap_or(json!({}));
    if !nested_records_in.is_object() {
        return Err("invalid nestedRecords".to_string());
    }
    let nested_records: HashMap<String, Value> = nested_records_in
        .as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

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

    // Dummy streamFn: the events slice never runs the loop.
    let stream_fn: Arc<pi_agent::LoopStreamFn> = Arc::new(|_model, _context, _options| {
        Box::pin(async { Err("agent-session-events harness streamFn must never be called".to_string()) })
    });
    let agent = Agent::new(AgentOptions {
        messages: Vec::new(),
        tools: Vec::new(),
        system_prompt: None,
        model: args.get("model").filter(|m| !m.is_null()).cloned().unwrap_or_else(pi_agent::default_model),
        thinking_level: "off".to_string(),
        steering_mode: Default::default(),
        follow_up_mode: Default::default(),
        stream_fn,
        tool_execution: Default::default(),
        clock: Arc::clone(&clock),
    });

    let session_manager = SessionManager::in_memory(HARNESS_CWD, None, wired_entries, &gen)?;
    let nested_store = Arc::new(PreloadedNestedCallsStore {
        records: Mutex::new(nested_records),
    });
    let extension_host = Arc::new(RecordingExtensionHost {
        log: Mutex::new(Vec::new()),
        message_end_replacement,
    });

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(FixedSettings { retry }),
        extension_host: Arc::clone(&extension_host) as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: Some(nested_store as Arc<dyn NestedCallsStore>),
        cwd: HARNESS_CWD.to_string(),
        custom_tools: Vec::new(),
        initial_active_tool_names: None,
        uses_default_tools: false,
        default_tool_modifiers: Vec::new(),
        allowed_tool_names: None,
        excluded_tool_names: None,
        base_tools_override: None,
    });

    // Seed the identity arena with the pool in order (pool index == MsgId),
    // then agent state with the first `stateCount` entries.
    for message in messages {
        session.register_message(message.clone());
    }
    for (entry_id, index) in entry_message_indices {
        session.link_entry_message(&entry_id, index);
    }
    let state_ids: Vec<usize> = (0..state_count).collect();
    session.seed_state_messages(&state_ids);

    // Public listener collector (TS side deep-clones each event at
    // collection time — the Rust emit snapshots values).
    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let collector = {
        let events = Arc::clone(&session_events);
        Box::new(move |event: &Value| {
            events.lock().expect("session events").push(event.clone());
        })
    };
    let listener_id = session.subscribe(collector);

    let result = pi_agent::run_current_thread(async {
        let mut results: Vec<Value> = Vec::with_capacity(ops.len());
        let mut idle_waits: Vec<(String, BoxFuture<'static, ()>, bool)> = Vec::new();
        for op in ops {
            let obj = op
                .as_object()
                .ok_or_else(|| "ops entries must be objects".to_string())?;
            let name = obj
                .get("op")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing op".to_string())?;
            let mut value = Value::Null;
            match name {
                "agentEvent" => {
                    let event = build_agent_event(op, messages.len())?;
                    session.handle_agent_event(&event).await;
                }
                "queueSteer" => {
                    let text = str_field(op, "text")?;
                    session.queue_steer_internal(&text);
                }
                "queueFollowUp" => {
                    let text = str_field(op, "text")?;
                    session.queue_follow_up_internal(&text);
                }
                "setStreaming" => {
                    let flag = bool_field(op, "value")?;
                    session.set_agent_run_active(flag);
                }
                "setRetryAttempt" => {
                    let attempt = num_field(op, "value")?;
                    session.set_retry_attempt(attempt);
                }
                "requestAbort" => {
                    session.request_abort();
                }
                "sendCustomMessage" => {
                    let custom_type = str_field(op, "customType")?;
                    let content = op.get("content").cloned().ok_or_else(|| "missing content".to_string())?;
                    let display = bool_field(op, "display")?;
                    let details = op.get("details").cloned();
                    let trigger_turn = match obj.get("triggerTurn") {
                        Some(Value::Null) | None => None,
                        Some(v) => Some(v.as_bool().ok_or_else(|| "invalid triggerTurn".to_string())?),
                    };
                    let deliver_as = match obj.get("deliverAs") {
                        Some(Value::Null) | None => None,
                        Some(v) => {
                            Some(v.as_str().ok_or_else(|| "invalid deliverAs".to_string())?.to_string())
                        }
                    };
                    session.send_custom_message(
                        &custom_type,
                        content,
                        display,
                        details,
                        trigger_turn,
                        deliver_as.as_deref(),
                    )?;
                }
                "findPersisted" => {
                    let index = pool_index(op.get("messageIndex"), messages.len(), "invalid messageIndex")?;
                    value = session
                        .find_persisted_message_entry_id(index)
                        .map(Value::String)
                        .unwrap_or(Value::Null);
                }
                "emitAgentSettled" => {
                    session.emit_agent_settled().await;
                }
                "waitForIdle" => {
                    let tag = str_field(op, "tag")?;
                    let future = session.wait_for_idle();
                    idle_waits.push((tag, future, false));
                }
                "deferAction" => {
                    let tag = str_field(op, "tag")?;
                    session.push_deferred_log_action(&tag);
                }
                "unsubscribe" => {
                    session.unsubscribe(listener_id);
                }
                other => return Err(format!("unknown agent-session-events op: {}", other)),
            }
            results.push(value);
        }

        // Idle waits: resolution only happens through `_emitAgentSettled`
        // (or registration while idle); poll each future once — a resolved
        // oneshot is ready, a pending one stays pending (TS mirrors this
        // with a `setImmediate` drain before reading `.then` flags).
        use std::task::{Context, Waker};
        let idle_waits: Vec<Value> = idle_waits
            .into_iter()
            .map(|(tag, mut future, _)| {
                let resolved =
                    future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready();
                json!({ "tag": tag, "resolved": resolved })
            })
            .collect();

        let queue_state = json!({
            "steering": session.steering_messages(),
            "followUp": session.follow_up_messages(),
            "isIdle": session.is_idle(),
            "retryAttempt": session.retry_attempt(),
            "pendingCustomMessages": session.pending_custom_messages(),
            "lastAssistantToolResults": session.last_assistant_tool_results(),
            "lastActivityOutcome": session.last_activity_outcome(),
            "turnIndex": session.turn_index(),
        });

        Ok(json!({
            "result": {
                "results": results,
                "sessionEvents": session_events.lock().expect("session events").clone(),
                "extensionEvents": extension_host.log.lock().expect("extension log").clone(),
                "entries": session.session_manager.get_entries(),
                "queueState": queue_state,
                "deferredLog": session.deferred_log(),
                "idleWaits": idle_waits,
            }
        }))
    });

    result
}

fn str_field(op: &Value, key: &str) -> Result<String, String> {
    op.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing {}", key))
}

fn bool_field(op: &Value, key: &str) -> Result<bool, String> {
    op.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing {}", key))
}

fn num_field(op: &Value, key: &str) -> Result<f64, String> {
    op.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("missing {}", key))
}
