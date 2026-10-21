//! Parity-harness entry for the `agent-session-bash` protocol: the
//! bash-execution slice of `pi_session::AgentSession` (TS
//! `executeBash`/`recordBashResult`/`abortBash`/`_flushPendingBashMessages`)
//! over a scripted `SessionBashOperations` fake. TS mirror:
//! parity/ts-harness.js `agent-session-bash` entry.
//!
//! Determinism: the fake yields once at exec start and once per
//! `delay`-flagged step (`setImmediate` on TS, [`YieldStep`] here — both
//! single-threaded FIFO round-robin), so concurrent executions interleave
//! identically. `awaitAllBash` polls the pending futures manually in
//! deterministic FIFO round-robin, mirroring the TS macrotask queue
//! (`tokio::spawn` is not usable: the current-thread scheduler's LIFO
//! slot would reorder first polls). Cases must not leave more than one
//! pending execution across `awaitBash` (a floating TS promise keeps
//! progressing on macrotasks; an un-polled Rust future does not).
//!
//! args:
//! ```json
//! {
//!   "nowMs": number,
//!   "config": { "model"?: object|null, "entries": [...],
//!               "systemPrompt": string,
//!               "settings"?: { "shellCommandPrefix"?: string } },
//!   "bashScript": [ { "output"?: string, "chunks"?: [{ "data": string,
//!                   "delay"?: bool, "abortAfter"?: bool }],
//!                   "delay"?: bool, "exitCode"?: number,
//!                   "throws"?: string, "abortAfter"?: bool } ],
//!   "providerTurns"?: [ { "events"?: [...], "result": {...} } ],
//!   "script": [ { "op": "executeBash", "command": string, "id"?: string,
//!                 "excludeFromContext"?: bool, "recordChunks"?: bool } |
//!               { "op": "bashStart", ...same fields } |
//!               { "op": "awaitBash" } | { "op": "awaitAllBash" } |
//!               { "op": "abortBash" } | { "op": "isBashRunning" } |
//!               { "op": "hasPendingBash" } | { "op": "flushPending" } |
//!               { "op": "setStreaming", "value": bool } |
//!               { "op": "recordBashResult", "command": string,
//!                 "result": { "output": string, "exitCode"?: number,
//!                             "cancelled"?: bool, "truncated"?: bool,
//!                             "fullOutputPath"?: string },
//!                 "excludeFromContext"?: bool } |
//!               { "op": "prompt", "text": string } |
//!               { "op": "waitForIdle" } ]
//! }
//! ```
//!
//! Result: `{ results, bashEvents, entries, pendingFlushed, bashCalls,
//! isBashRunning, userChunks }`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use serde_json::{json, Value};

use pi_agent::host::{Clock, FixedClock, Sleeper};
use pi_agent::types::BoxFuture;
use pi_agent::{Agent, AgentOptions};
use pi_core::session_manager::{Generators, SessionManager};
use pi_session::agent_session::bash::{
    BashExecutionOptions, PendingBash, SessionBashOperations,
};
use pi_session::agent_session::{
    AgentSession, AgentSessionConfig, ExtensionHost, ModelAuthStore, SessionSettingsStore,
};
use pi_tools::shell::{AbortSignal, ExecOutcome, ShellExecError};

const HARNESS_CWD: &str = "/proj";
const DEFAULT_DOCS_PATH: &str = "/pi-docs";

/// One normalized `bashScript` entry (flat `output`/`chunks` forms both
/// normalize to `steps`).
struct ScriptedStep {
    data: String,
    delay: bool,
    abort_after: bool,
}

struct ScriptedEntry {
    steps: Vec<ScriptedStep>,
    exit_code: Option<i32>,
    throws: Option<String>,
}

/// Settings fake: `shellCommandPrefix` seed, TS in-memory defaults
/// elsewhere.
struct BashHarnessSettings {
    shell_command_prefix: Option<String>,
}

impl SessionSettingsStore for BashHarnessSettings {
    fn retry_settings(&self) -> pi_core::settings_manager::RetrySettings {
        pi_core::settings_manager::RetrySettings {
            enabled: true,
            max_retries: 3.0,
            base_delay_ms: 2000.0,
            max_agent_delay_ms: pi_session::agent_session::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
        }
    }
    fn shell_command_prefix(&self) -> Option<String> {
        self.shell_command_prefix.clone()
    }
}

struct BashHarnessAuth;

impl ModelAuthStore for BashHarnessAuth {
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

/// Recording fake `_extensionRunner` (no handlers).
struct BashHarnessHost {
    events: Mutex<Vec<Value>>,
    errors: Mutex<Vec<Value>>,
}

impl ExtensionHost for BashHarnessHost {
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

/// The `setImmediate` mirror for the scripted executor: wakes the poller
/// and suspends on first poll, resumes on the next (tokio's
/// `yield_now`/spawn carry scheduler-specific LIFO behavior that would
/// reorder concurrent first polls relative to the TS macrotask FIFO).
struct YieldStep {
    resumed: bool,
}

impl Future for YieldStep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.resumed {
            Poll::Ready(())
        } else {
            self.resumed = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

async fn yield_step() {
    YieldStep { resumed: false }.await;
}

/// The patched-`setTimeout` mirror for the prompt-run backoff (fast-forward).
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

/// Scripted `SessionBashOperations`: one `bashScript` entry per exec call,
/// one yield at start plus one per `delay` step; `abortAfter` steps fire
/// the session's bash abort tap (TS: `session.abortBash()`).
struct ScriptedBashOperations {
    script: Vec<ScriptedEntry>,
    cursor: Mutex<usize>,
    calls: Mutex<Vec<Value>>,
    abort_tap: Arc<dyn Fn() + Send + Sync>,
}

impl SessionBashOperations for ScriptedBashOperations {
    fn exec<'a>(
        &'a self,
        command: &'a str,
        cwd: &'a str,
        on_data: &'a mut (dyn FnMut(&[u8]) + Send),
        _signal: Option<&'a AbortSignal>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ShellExecError>> {
        Box::pin(async move {
            yield_step().await;
            let index = {
                let mut cursor = self.cursor.lock().expect("bash script cursor");
                let value = *cursor;
                *cursor += 1;
                value
            };
            let entry = match self.script.get(index) {
                Some(entry) => entry,
                None => return Err(ShellExecError::Other("bash script exhausted".to_string())),
            };
            self.calls.lock().expect("bash calls").push(json!({
                "command": command,
                "cwd": cwd,
            }));
            for step in &entry.steps {
                if step.delay {
                    yield_step().await;
                }
                on_data(step.data.as_bytes());
                if step.abort_after {
                    (self.abort_tap)();
                }
            }
            if let Some(message) = &entry.throws {
                return Err(ShellExecError::Other(message.clone()));
            }
            Ok(ExecOutcome { exit_code: entry.exit_code })
        })
    }
}

/// Normalize one validated `bashScript` entry.
fn scripted_entry(entry: &Value) -> Result<ScriptedEntry, String> {
    let invalid = || "invalid bashScript".to_string();
    let step_of = |step: &Value| -> Result<ScriptedStep, String> {
        let data = step.get("data").and_then(Value::as_str).unwrap_or_default().to_string();
        let delay = step.get("delay").map(|d| !d.is_null() && d != &Value::Bool(false)).unwrap_or(false);
        let abort_after = step.get("abortAfter").and_then(Value::as_bool).unwrap_or(false);
        Ok(ScriptedStep { data, delay, abort_after })
    };
    let steps = match entry.get("chunks") {
        Some(Value::Array(chunks)) => {
            let mut steps = Vec::new();
            for chunk in chunks {
                steps.push(step_of(chunk)?);
            }
            steps
        }
        _ => vec![ScriptedStep {
            data: entry
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            delay: entry
                .get("delay")
                .map(|d| !d.is_null() && d != &Value::Bool(false))
                .unwrap_or(false),
            abort_after: entry.get("abortAfter").and_then(Value::as_bool).unwrap_or(false),
        }],
    };
    let exit_code = match entry.get("exitCode") {
        None | Some(Value::Null) => None,
        Some(Value::Number(code)) => code.as_i64().map(|c| c as i32),
        _ => return Err(invalid()),
    };
    let throws = entry.get("throws").and_then(Value::as_str).map(str::to_string);
    Ok(ScriptedEntry { steps, exit_code, throws })
}

pub fn run_agent_session_bash_harness(args: &Value) -> Result<Value, String> {
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
    let shell_command_prefix = match settings_in.get("shellCommandPrefix") {
        None | Some(Value::Null) => None,
        Some(Value::String(prefix)) => Some(prefix.clone()),
        _ => return Err("invalid settings".to_string()),
    };
    let bash_script_in = args
        .get("bashScript")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing bashScript".to_string())?
        .clone();
    let invalid_bash = || "invalid bashScript".to_string();
    let mut bash_script: Vec<ScriptedEntry> = Vec::new();
    for entry in &bash_script_in {
        if !entry.is_object() {
            return Err(invalid_bash());
        }
        if let Some(chunks) = entry.get("chunks") {
            let Value::Array(chunks) = chunks else { return Err(invalid_bash()) };
            for chunk in chunks {
                if !chunk.is_object() {
                    return Err(invalid_bash());
                }
                match chunk.get("data").and_then(Value::as_str) {
                    Some(_) => {}
                    None => return Err(invalid_bash()),
                }
                match chunk.get("delay") {
                    None | Some(Value::Bool(_)) | Some(Value::Number(_)) => {}
                    _ => return Err(invalid_bash()),
                }
                match chunk.get("abortAfter") {
                    None | Some(Value::Bool(_)) => {}
                    _ => return Err(invalid_bash()),
                }
            }
        } else if let Some(output) = entry.get("output") {
            if !output.is_string() {
                return Err(invalid_bash());
            }
        }
        match entry.get("delay") {
            None | Some(Value::Bool(_)) | Some(Value::Number(_)) => {}
            _ => return Err(invalid_bash()),
        }
        match entry.get("exitCode") {
            None | Some(Value::Null) | Some(Value::Number(_)) => {}
            _ => return Err(invalid_bash()),
        }
        match entry.get("throws") {
            None | Some(Value::String(_)) => {}
            _ => return Err(invalid_bash()),
        }
        match entry.get("abortAfter") {
            None | Some(Value::Bool(_)) => {}
            _ => return Err(invalid_bash()),
        }
        bash_script.push(scripted_entry(entry)?);
    }
    let provider_turns = match args.get("providerTurns") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(turns)) => turns.clone(),
        _ => return Err("invalid providerTurns".to_string()),
    };
    let script = args
        .get("script")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing script".to_string())?
        .clone();

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

    let provider_calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let provider_cursor = Arc::new(Mutex::new(0usize));
    let stream_fn: Arc<pi_agent::LoopStreamFn> = {
        let turns = provider_turns;
        let cursor = Arc::clone(&provider_cursor);
        let sink = Arc::clone(&provider_calls);
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
                sink.lock().expect("provider calls").push(json!({
                    "model": model,
                    "messages": context.get("messages").cloned().unwrap_or(Value::Array(Vec::new())),
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

    let host = Arc::new(BashHarnessHost {
        events: Mutex::new(Vec::new()),
        errors: Mutex::new(Vec::new()),
    });
    let session_events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let user_chunks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut session = AgentSession::new(AgentSessionConfig {
        agent,
        session_manager,
        settings: Arc::new(BashHarnessSettings { shell_command_prefix }) as Arc<dyn SessionSettingsStore>,
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
        model_auth: Some(Arc::new(BashHarnessAuth)),
        skills: Vec::new(),
        prompt_templates: Vec::new(),
        system_prompt: Some(system_prompt),
        model_lookup: None,
        scoped_models: Vec::new(),
        model_routes: None,
        sleeper: Some(Arc::new(ImmediateSleeper)),
    });
    {
        let sink = Arc::clone(&session_events);
        session.subscribe(Box::new(move |event: &Value| {
            sink.lock().expect("session events").push(event.clone());
        }));
    }
    let operations = Arc::new(ScriptedBashOperations {
        script: bash_script,
        cursor: Mutex::new(0),
        calls: Mutex::new(Vec::new()),
        abort_tap: session.bash_abort_tap(),
    });

    pi_agent::run_current_thread(async move {
        let mut pending: Vec<PendingBash> = Vec::new();
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        for op in &script {
            let outcome = run_op(&mut session, op, &operations, &user_chunks, &mut pending).await;
            results.push(match outcome {
                Ok(value) => value,
                Err(message) => json!({ "thrown": message }),
            });
        }

        let entries = session.session_manager.get_entries();
        Ok(json!({
            "result": {
                "results": results,
                "bashEvents": session_events.lock().expect("session events").clone(),
                "entries": entries,
                "pendingFlushed": session.has_pending_bash_messages(),
                "bashCalls": operations.calls.lock().expect("bash calls").clone(),
                "isBashRunning": session.is_bash_running(),
                "userChunks": user_chunks.lock().expect("user chunks").clone(),
            }
        }))
    })
}

fn bash_options(op: &Value) -> Result<BashExecutionOptions, String> {
    let id = match op.get("id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id.clone()),
        _ => return Err("invalid id".to_string()),
    };
    let exclude_from_context = match op.get("excludeFromContext") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        _ => return Err("invalid excludeFromContext".to_string()),
    };
    Ok(BashExecutionOptions { exclude_from_context, id, operations: None })
}

async fn run_op(
    session: &mut AgentSession,
    op: &Value,
    operations: &Arc<ScriptedBashOperations>,
    user_chunks: &Arc<Mutex<Vec<String>>>,
    pending: &mut Vec<PendingBash>,
) -> Result<Value, String> {
    let obj = op.as_object().ok_or_else(|| "ops entries must be objects".to_string())?;
    let name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing op".to_string())?;
    let command_of = || -> Result<String, String> {
        obj.get("command")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "missing command".to_string())
    };
    let record_chunks = obj.get("recordChunks").and_then(Value::as_bool).unwrap_or(false);
    let user_callback = if record_chunks {
        let sink = Arc::clone(user_chunks);
        Some(Arc::new(move |chunk: &str| {
            sink.lock().expect("user chunks").push(chunk.to_string());
        }) as Arc<dyn Fn(&str) + Send + Sync>)
    } else {
        None
    };
    match name {
        "executeBash" | "bashStart" => {
            let command = command_of()?;
            let mut options = bash_options(op)?;
            options.operations = Some(Arc::clone(operations) as Arc<dyn SessionBashOperations>);
            if name == "executeBash" {
                let result = session.execute_bash(&command, user_callback, options).await?;
                Ok(serde_json::to_value(&result).unwrap_or(Value::Null))
            } else {
                pending.push(session.start_bash(&command, user_callback, options));
                Ok(Value::Null)
            }
        }
        "awaitBash" => {
            if pending.is_empty() {
                return Err("no pending bash".to_string());
            }
            let pending_bash = pending.remove(0);
            let result = pending_bash.future.await;
            let outcome = session.complete_bash(
                &pending_bash.command,
                pending_bash.exclude_from_context,
                &pending_bash.signal,
                result,
            );
            Ok(match outcome {
                Ok(result) => serde_json::to_value(&result).unwrap_or(Value::Null),
                Err(message) => return Err(message),
            })
        }
        "awaitAllBash" => {
            let count = pending.len();
            if count == 0 {
                return Err("no pending bash".to_string());
            }
            // Promise.allSettled semantics over the TS macrotask queue: poll
            // every pending future once per round, in input order (each
            // future advances at most one YieldStep per poll, so round-robin
            // mirrors the setImmediate FIFO). A future completing mid-round
            // runs its `complete_bash` continuation immediately — the TS
            // microtask runs before the next macrotask.
            let mut slots: Vec<Option<PendingBash>> = pending.drain(..).map(Some).collect();
            let mut settled: Vec<Value> = vec![Value::Null; count];
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            let mut remaining = count;
            while remaining > 0 {
                for index in 0..count {
                    let poll = match slots[index].as_mut() {
                        Some(pending_bash) => pending_bash.future.as_mut().poll(&mut cx),
                        None => continue,
                    };
                    if let Poll::Ready(result) = poll {
                        let pending_bash = slots[index].take().expect("polled pending bash");
                        remaining -= 1;
                        let outcome = session.complete_bash(
                            &pending_bash.command,
                            pending_bash.exclude_from_context,
                            &pending_bash.signal,
                            result,
                        );
                        settled[index] = match outcome {
                            Ok(result) => json!({
                                "status": "fulfilled",
                                "value": serde_json::to_value(&result).unwrap_or(Value::Null),
                            }),
                            Err(reason) => json!({ "status": "rejected", "reason": reason }),
                        };
                    }
                }
            }
            Ok(Value::Array(settled))
        }
        "abortBash" => {
            session.abort_bash();
            Ok(Value::Null)
        }
        "isBashRunning" => Ok(json!(session.is_bash_running())),
        "hasPendingBash" => Ok(json!(session.has_pending_bash_messages())),
        "flushPending" => {
            session.flush_pending_bash_messages();
            Ok(Value::Null)
        }
        "setStreaming" => {
            let value = obj.get("value").and_then(Value::as_bool).ok_or_else(|| "missing value".to_string())?;
            session.set_agent_run_active(value);
            Ok(Value::Null)
        }
        "recordBashResult" => {
            let command = command_of()?;
            let result_in = obj.get("result").ok_or_else(|| "missing result".to_string())?;
            if !result_in.is_object() {
                return Err("invalid result".to_string());
            }
            let output = match result_in.get("output") {
                Some(Value::String(output)) => output.clone(),
                _ => return Err("invalid result".to_string()),
            };
            let exit_code = match result_in.get("exitCode") {
                None | Some(Value::Null) => None,
                Some(Value::Number(code)) => code.as_i64().map(|c| c as i32),
                _ => return Err("invalid result".to_string()),
            };
            let cancelled = match result_in.get("cancelled") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(value)) => *value,
                _ => return Err("invalid result".to_string()),
            };
            let truncated = match result_in.get("truncated") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(value)) => *value,
                _ => return Err("invalid result".to_string()),
            };
            let full_output_path = match result_in.get("fullOutputPath") {
                None | Some(Value::Null) => None,
                Some(Value::String(path)) => Some(path.clone()),
                _ => return Err("invalid result".to_string()),
            };
            let exclude_from_context = match obj.get("excludeFromContext") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(value)) => Some(*value),
                _ => return Err("invalid excludeFromContext".to_string()),
            };
            session.record_bash_result(
                &command,
                &pi_tools::bash_executor::BashResult {
                    output,
                    exit_code,
                    cancelled,
                    truncated,
                    full_output_path,
                },
                exclude_from_context,
            );
            Ok(Value::Null)
        }
        "prompt" => {
            let text = obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing text".to_string())?;
            session.prompt(text, Default::default()).await?;
            Ok(Value::Null)
        }
        "waitForIdle" => {
            session.wait_for_idle().await;
            Ok(Value::Null)
        }
        other => Err(format!("unknown agent-session-bash op: {}", other)),
    }
}
