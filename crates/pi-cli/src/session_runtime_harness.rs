//! Parity-harness entry for the `agent-session-runtime` protocol:
//! `pi_session::agent_session::runtime::AgentSessionRuntime`
//! (TS `core/agent-session-runtime.ts`) — `switchSession` / `newSession`
//! / `fork` / `importFromJsonl` / `dispose` over a runtime factory that
//! builds real `AgentSession`s from a scripted provider. TS mirror:
//! parity/ts-harness.js `agent-session-runtime` entry.
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "entries": [...], "model"?: object|null,
//!               "systemPrompt": string },
//!   "files"?: { "<rel>": "<content>" },
//!   "script": [ { "op": "switchSession", "path": "...",
//!                 "cwdOverride"?: "...", "withSession"?: true } |
//!               { "op": "newSession", "parentSession"?: "...",
//!                 "withSession"?: true } |
//!               { "op": "fork", "entryId": "...", "position"?: "...",
//!                 "withSession"?: true } |
//!               { "op": "importFromJsonl", "path": "...",
//!                 "cwdOverride"?: "..." } |
//!               { "op": "dispose" } |
//!               { "op": "prompt", "text": "..." } |
//!               { "op": "setRetryAttempt", "n": number } |
//!               { "op": "getSessionId" } ],
//!   "providerTurns": [ { "events"?: [...], "result": {...} } ],
//!   "extensionHandlers"?: { "session_before_switch": true |
//!                             { "cancel": true }, ... }
//! }
//! ```
//!
//! The initial session is `SessionManager::open` of
//! `<cwd>/pi-parity-asr/sessions/initial.jsonl` (written from
//! `config.entries`); `files` materializes further fixtures under the
//! same fixed base (idempotent — the base is wiped first). Result:
//! `{ results, sessionIds, shutdownOrder, finalEntries,
//! finalSessionFile, promptsSentToAgent }`. `shutdownOrder` is the
//! interleaved trace (`sessionEvent:*` / `emit:*` / `beforeInvalidate` /
//! `invalidate` / `rebind` / `withSession` / `sessionStart:*`).
//!
//! `setRetryAttempt` installs the fake in-flight auto-retry (TS harness:
//! `_retryAbortController` + abort listener running
//! `_finishCancelledRetry()`); `teardownCurrent`'s `abort()` then emits
//! `auto_retry_end` before `session_shutdown`. True concurrent
//! switch-while-running is descoped: Rust blocks concurrent prompt
//! futures (`&mut self`), so the parked retry sleep is unreachable from
//! the harness.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{Clock, FixedClock};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions};
use pi_core::paths::{resolve_path, PathInputOptions};
use pi_core::session_manager::{Generators, SessionManager};
use pi_session::agent_session::runtime::{
    AgentSessionRuntime, BeforeInvalidateFn, CreateAgentSessionRuntimeResult, CreateRuntimeFactory,
    CreateRuntimeOptions, RebindSessionFn,
};
use pi_session::agent_session::services::{
    AgentSessionServices, CreateAgentSessionServicesOptions, ExtensionsRuntimeState,
    LoadedExtension, LoadExtensionsResult, ServicesModelRuntime, ServicesResourceLoader,
};
use pi_session::agent_session::{AgentSession, AgentSessionConfig, ExtensionHost, SessionSettingsStore};

/// Fake settings store (TS `SettingsManager.inMemory` defaults).
struct RuntimeHarnessSettings;

impl SessionSettingsStore for RuntimeHarnessSettings {
    fn retry_settings(&self) -> pi_core::settings_manager::RetrySettings {
        pi_core::settings_manager::RetrySettings {
            enabled: true,
            max_retries: 3.0,
            base_delay_ms: 2000.0,
            max_agent_delay_ms: pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
        }
    }
}

struct RuntimeHost {
    trace: Arc<Mutex<Vec<String>>>,
    handlers: Vec<(String, bool)>,
}

impl RuntimeHost {
    fn record(&self, event: &Value) -> bool {
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        self.trace.lock().expect("trace").push(format!("emit:{}", event_type));
        self.handlers
            .iter()
            .find(|(k, _)| k == event_type)
            .map(|(_, cancel)| *cancel)
            .unwrap_or(false)
    }
}

impl ExtensionHost for RuntimeHost {
    fn has_handlers(&self, event: &str) -> bool {
        self.handlers.iter().any(|(k, _)| k == event)
    }
    fn emit_session_before_switch<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        let cancel = self.record(&event);
        Box::pin(async move {
            if cancel {
                Some(json!({ "cancel": true }))
            } else {
                None
            }
        })
    }
    fn emit_session_before_fork<'a>(&'a self, event: Value) -> BoxFuture<'a, Option<Value>> {
        let cancel = self.record(&event);
        Box::pin(async move {
            if cancel {
                Some(json!({ "cancel": true }))
            } else {
                None
            }
        })
    }
    fn command_context(&self) -> Value {
        json!({})
    }
    fn emit<'a>(&'a self, event: Value) -> BoxFuture<'a, ()> {
        self.record(&event);
        Box::pin(async {})
    }
    fn emit_message_end<'a>(&'a self, _event: Value) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async { None })
    }
    fn emit_error(&self, _error: Value) {}
    fn invalidate(&self, _message: &str) {
        // TS fakeRunner.invalidate records an "invalidate" trace line on
        // every session dispose.
        self.trace.lock().expect("trace").push("invalidate".to_string());
    }
}

/// No-op `ModelRuntime` fake for the runtime services.
struct NoopModelRuntime;

impl ServicesModelRuntime for NoopModelRuntime {
    fn register_provider(&self, _name: &str, _config: &Value) -> Result<(), String> {
        Ok(())
    }
    fn register_native_provider(&self, _provider: &Value) -> Result<(), String> {
        Ok(())
    }
    fn register_virtual_model(&self, _definition: &Value) -> Result<(), String> {
        Ok(())
    }
    fn refresh<'a>(&'a self, _allow_network: bool) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// No-op `ResourceLoader` fake for the runtime services.
struct NoopResourceLoader;

impl ServicesResourceLoader for NoopResourceLoader {
    fn get_extensions(&self) -> LoadExtensionsResult {
        LoadExtensionsResult {
            extensions: Vec::new(),
            runtime: Arc::new(Mutex::new(ExtensionsRuntimeState::default())),
        }
    }
}

/// Deterministic uuidv7 mirroring TS's `uuidv7` with zero-filled
/// `crypto.getRandomValues` and a pinned `Date` (sequence 0, 1, 2, ...).
fn zero_uuid_v7(now_ms: i64, seq: u64) -> String {
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&(now_ms as u64).to_be_bytes()[2..]);
    bytes[6] = 0x70 | ((seq >> 37) as u8 & 0x0f);
    bytes[7] = (seq >> 29) as u8;
    bytes[8] = 0x80 | ((seq >> 23) as u8 & 0x3f);
    bytes[9] = (seq >> 15) as u8;
    bytes[10] = (seq >> 7) as u8;
    bytes[11] = (((seq & 0x7f) << 1) as u8) | 0;
    let hex = |b: u8| format!("{:02x}", b);
    format!(
        "{}-{}-{}-{}-{}",
        hex(bytes[0]) + &hex(bytes[1]) + &hex(bytes[2]) + &hex(bytes[3]),
        hex(bytes[4]) + &hex(bytes[5]),
        hex(bytes[6]) + &hex(bytes[7]),
        hex(bytes[8]) + &hex(bytes[9]),
        hex(bytes[10]) + &hex(bytes[11]) + &hex(bytes[12]) + &hex(bytes[13]) + &hex(bytes[14])
            + &hex(bytes[15]),
    )
}

pub fn run_agent_session_runtime_harness(args: &Value, process_cwd: &str) -> Result<Value, String> {
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
    let mut handlers: Vec<(String, bool)> = Vec::new();
    if let Some(ext) = args.get("extensionHandlers") {
        let obj = ext.as_object().ok_or_else(|| "invalid extensionHandlers".to_string())?;
        for (key, value) in obj {
            let cancel = match value {
                Value::Bool(true) => false,
                Value::Object(map) => {
                    if map.len() == 1 && map.get("cancel") == Some(&Value::Bool(true)) {
                        true
                    } else {
                        return Err("invalid extensionHandlers".to_string());
                    }
                }
                _ => return Err("invalid extensionHandlers".to_string()),
            };
            handlers.push((key.clone(), cancel));
        }
    }
    let files = args.get("files").cloned().unwrap_or(json!({}));
    if !files.is_object() {
        return Err("invalid files".to_string());
    }
    // TS validates every file value before touching the filesystem; a bad
    // value must leave no fixtures behind (fs snapshot stays empty).
    for value in files.as_object().expect("files object").values() {
        if !value.is_string() {
            return Err("invalid files".to_string());
        }
    }

    // Fixed fixture base (both sides use the harness cwd, so paths agree).
    let base = format!("{}/pi-parity-asr", process_cwd.trim_end_matches('/'));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(format!("{}/sessions", base)).map_err(|e| e.to_string())?;
    let initial_path = format!("{}/sessions/initial.jsonl", base);
    let initial_content = entries
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&initial_path, initial_content).map_err(|e| e.to_string())?;
    if let Some(obj) = files.as_object() {
        for (rel, content) in obj {
            let content = content.as_str().ok_or_else(|| "invalid files".to_string())?;
            let target = format!("{}/{}", base, rel.trim_start_matches('/'));
            if let Some(parent) = std::path::Path::new(&target).parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&target, content).map_err(|e| e.to_string())?;
        }
    }

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
    let v7_counter = Arc::new(Mutex::new(0u64));
    let uuid_v7 = {
        let counter = Arc::clone(&v7_counter);
        Arc::new(move || {
            let mut n = counter.lock().expect("v7 counter");
            let value = *n;
            *n += 1;
            zero_uuid_v7(now_ms_i, value)
        })
    };
    let now_gen = Arc::new(move || now_ms_i);
    let gen = Generators::with_gens(uuid_v7, uuid_v4, now_gen);

    let trace: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let session_ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let prompts_sent: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let host: Arc<RuntimeHost> = Arc::new(RuntimeHost {
        trace: Arc::clone(&trace),
        handlers,
    });

    let model = config.get("model").filter(|m| !m.is_null()).cloned().unwrap_or(Value::Null);
    let turns_for_factory = provider_turns;
    let factory_model = model.clone();
    let factory_prompt = system_prompt.clone();
    let factory_clock = Arc::clone(&clock);
    let factory_trace = Arc::clone(&trace);
    let factory_ids = Arc::clone(&session_ids);
    let factory_prompts = Arc::clone(&prompts_sent);
    let factory_host = Arc::clone(&host);
    let create_runtime: CreateRuntimeFactory = {
        Arc::new(move |options: CreateRuntimeOptions| {
            let model = factory_model.clone();
            let system_prompt = factory_prompt.clone();
            let clock = Arc::clone(&factory_clock);
            let trace = Arc::clone(&factory_trace);
            let session_ids = Arc::clone(&factory_ids);
            let prompts_sent = Arc::clone(&factory_prompts);
            let host = Arc::clone(&factory_host);
            let turns = turns_for_factory.clone();
            Box::pin(async move {
                if let Some(event) = &options.session_start_event {
                    let reason = event.get("reason").and_then(Value::as_str).unwrap_or("");
                    trace.lock().expect("trace").push(format!("sessionStart:{}", reason));
                }
                let session = build_session(BuildSessionInputs {
                    session_manager: options.session_manager,
                    model,
                    system_prompt,
                    clock,
                    trace,
                    session_ids,
                    prompts_sent,
                    host,
                    turns,
                })
                .await?;
                let services = AgentSessionServices {
                    cwd: options.cwd.clone(),
                    agent_dir: options.agent_dir.clone(),
                    model_runtime: Arc::new(NoopModelRuntime) as Arc<dyn ServicesModelRuntime>,
                    settings_manager: Arc::new(RuntimeHarnessSettings) as Arc<dyn SessionSettingsStore>,
                    resource_loader: Arc::new(NoopResourceLoader) as Arc<dyn ServicesResourceLoader>,
                    diagnostics: Vec::new(),
                };
                Ok(CreateAgentSessionRuntimeResult {
                    session,
                    services,
                    diagnostics: Vec::new(),
                    model_fallback_message: None,
                })
            })
        })
    };

    pi_agent::run_current_thread(async move {
        let session_manager = SessionManager::open(&initial_path, None, None, &gen)?;
        let mut runtime = pi_session::agent_session::runtime::create_agent_session_runtime(
            create_runtime,
            CreateRuntimeOptions {
                cwd: base.clone(),
                agent_dir: base.clone(),
                session_manager,
                session_start_event: None,
                project_trust_context: None,
            },
        )
        .await?;

        // Mirror the TS harness's setRebindSession / setBeforeSessionInvalidate
        // trace callbacks.
        {
            let rb_trace = Arc::clone(&trace);
            runtime.set_rebind_session(Some(Arc::new(move || {
                let trace = Arc::clone(&rb_trace);
                Box::pin(async move {
                    trace.lock().expect("trace").push("rebind".to_string());
                }) as BoxFuture<'static, ()>
            }) as RebindSessionFn));
            let trace = Arc::clone(&trace);
            runtime.set_before_session_invalidate(Some(Arc::new(move || {
                trace.lock().expect("trace").push("beforeInvalidate".to_string());
            }) as BeforeInvalidateFn));
        }

        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(&mut runtime, op, &gen, &trace, process_cwd).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }

        let final_entries = runtime.session().session_manager.get_entries();
        let final_session_file = runtime
            .session()
            .session_manager
            .get_session_file()
            .map(|f| json!(f));
        Ok(json!({
            "result": {
                "results": results,
                "sessionIds": session_ids.lock().expect("session ids").clone(),
                "shutdownOrder": trace.lock().expect("trace").clone(),
                "finalEntries": final_entries,
                "finalSessionFile": final_session_file,
                "promptsSentToAgent": prompts_sent.lock().expect("prompts sent").clone(),
            }
        }))
    })
}

struct BuildSessionInputs {
    session_manager: SessionManager,
    model: Value,
    system_prompt: String,
    clock: Arc<dyn Clock>,
    trace: Arc<Mutex<Vec<String>>>,
    session_ids: Arc<Mutex<Vec<String>>>,
    prompts_sent: Arc<Mutex<Vec<Value>>>,
    host: Arc<RuntimeHost>,
    turns: Vec<Value>,
}

async fn build_session(inputs: BuildSessionInputs) -> Result<AgentSession, String> {
    let BuildSessionInputs {
        session_manager,
        model,
        system_prompt,
        clock,
        trace,
        session_ids,
        prompts_sent,
        host,
        turns,
    } = inputs;
    let session_cwd = session_manager.get_cwd().to_string();
    session_ids
        .lock()
        .expect("session ids")
        .push(session_manager.get_session_id().to_string());
    let projected = session_manager
        .build_session_projection()
        .messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect::<Vec<Value>>();

    let provider_cursor = Arc::new(Mutex::new(0usize));
    let stream_fn: Arc<pi_agent::LoopStreamFn> = {
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
                sink.lock().expect("prompts sent").push(json!({
                    "messages": context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())),
                }));
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
        model,
        thinking_level: "off".to_string(),
        steering_mode: Default::default(),
        follow_up_mode: Default::default(),
        stream_fn,
        tool_execution: Default::default(),
        clock: Arc::clone(&clock),
    });

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(RuntimeHarnessSettings) as Arc<dyn SessionSettingsStore>,
        extension_host: host as Arc<dyn ExtensionHost>,
        clock: Arc::clone(&clock),
        nested_tool_calls: None,
        cwd: session_cwd,
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
        sleeper: None,
    });
    {
        let trace = Arc::clone(&trace);
        session.subscribe(Box::new(move |event: &Value| {
            let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
            trace
                .lock()
                .expect("trace")
                .push(format!("sessionEvent:{}", event_type));
        }));
    }
    Ok(session)
}

async fn run_op(
    runtime: &mut AgentSessionRuntime,
    op: &Value,
    gen: &Generators,
    trace: &Arc<Mutex<Vec<String>>>,
    ambient: &str,
) -> Result<Value, String> {
    // TS resolves op paths against process.cwd() (the harness chdir'd to
    // the ambient cwd); resolve here so the Rust ambient ("/") never
    // leaks into case paths.
    let resolve = |input: &str| resolve_path(input, ambient, &PathInputOptions::default());
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    let with_session = |trace: &Arc<Mutex<Vec<String>>>| -> pi_session::agent_session::runtime::WithSessionFn {
        let trace = Arc::clone(trace);
        Arc::new(move |_ctx| {
            let trace = Arc::clone(&trace);
            Box::pin(async move {
                trace.lock().expect("trace").push("withSession".to_string());
            })
        })
    };
    match name {
        "switchSession" => {
            let path = obj
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing path".to_string())?;
            let path = resolve(path);
            let cwd_override = obj.get("cwdOverride").and_then(Value::as_str).map(&resolve);
            let with = obj
                .get("withSession")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .then(|| with_session(trace));
            let cancelled = runtime
                .switch_session(&path, cwd_override.as_deref(), with, None, gen)
                .await?;
            Ok(json!({ "cancelled": cancelled }))
        }
        "newSession" => {
            let parent = obj.get("parentSession").and_then(Value::as_str);
            let with = obj
                .get("withSession")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .then(|| with_session(trace));
            let cancelled = runtime.new_session(parent, None, with, gen).await?;
            Ok(json!({ "cancelled": cancelled }))
        }
        "fork" => {
            let entry_id = obj
                .get("entryId")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing entryId".to_string())?;
            let position = obj.get("position").and_then(Value::as_str);
            let with = obj
                .get("withSession")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .then(|| with_session(trace));
            let (cancelled, selected_text) = runtime.fork(entry_id, position, with, gen).await?;
            let mut result = serde_json::Map::new();
            result.insert("cancelled".to_string(), json!(cancelled));
            if let Some(text) = selected_text {
                result.insert("selectedText".to_string(), json!(text));
            }
            Ok(Value::Object(result))
        }
        "importFromJsonl" => {
            let path = obj
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing path".to_string())?;
            let path = resolve(path);
            let cwd_override = obj.get("cwdOverride").and_then(Value::as_str).map(&resolve);
            let cancelled = runtime.import_from_jsonl(&path, cwd_override.as_deref(), gen).await?;
            Ok(json!({ "cancelled": cancelled }))
        }
        "dispose" => {
            runtime.dispose().await;
            Ok(Value::Null)
        }
        "prompt" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing text".to_string())?;
            runtime.session_mut().prompt(text, Default::default()).await?;
            Ok(Value::Null)
        }
        "setRetryAttempt" => {
            let n = obj.get("n").and_then(Value::as_f64).ok_or_else(|| "missing n".to_string())?;
            runtime.session_mut().install_pending_retry(n);
            Ok(Value::Null)
        }
        "getSessionId" => Ok(json!(runtime.session().session_manager.get_session_id())),
        other => Err(format!("unknown agent-session-runtime op: {}", other)),
    }
}
