//! Parity-harness entry for the `agent-queue` protocol: the queue surface
//! of the agent layer (TS `packages/agent` `class Agent` steer/follow-up
//! queues).
//!
//! Wire protocol (TS mirror contract in parity/ts-harness.js, `agent-queue`
//! entry): bare JSON args on stdin, `{"result": {"results": [...]}}` on
//! stdout; per-case exceptions surface in-band as `{"error": msg}` with
//! byte-identical messages on both sides.
//!
//! args:
//! ```json
//! {
//!   "initialState": { "messages": [...], "tools": [...] },  // optional
//!   "steeringMode": "all" | "one-at-a-time",                // optional
//!   "followUpMode": "all" | "one-at-a-time",                // optional
//!   "ops": [ { "op": ... }, ... ]
//! }
//! ```
//!
//! `initialState` seeds agent state (validated symmetrically with the TS
//! side, which passes the messages to a real `Agent` with a never-called
//! dummy `streamFn`); it is not observable through the queue surface, so
//! the Rust side validates and drops it. Messages carry literal numeric
//! timestamps — no clock is involved, no normalization needed.
//!
//! Ops and their logged results:
//! - `{op:"steer"|"followUp", message}` → `null`
//! - `{op:"setSteeringMode"|"setFollowUpMode", mode}` → `null`
//! - `{op:"getSteeringMode"|"getFollowUpMode"}` → mode string
//! - `{op:"clearSteering"|"clearFollowUp"|"clearAll"}` → `null`
//! - `{op:"hasQueued"}` → `true`/`false`
//! - `{op:"peek"}` → array of messages (verbatim values)

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::agent_surface::AgentQueues;
use pi_agent::types::{AgentTool, AgentToolResult, BoxFuture, QueueMode, ToolExecutionMode};

pub fn run_agent_queue_harness(args: &Value, _cwd: &Path) -> Result<Value, String> {
    // Validation order mirrors the TS entry exactly: ops array, then the
    // top-level modes, then initialState, then per-op.
    let ops = args
        .get("ops")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing ops".to_string())?;

    let steering_mode = parse_mode_field(args.get("steeringMode"), "steeringMode")?
        .unwrap_or(QueueMode::OneAtATime);
    let follow_up_mode = parse_mode_field(args.get("followUpMode"), "followUpMode")?
        .unwrap_or(QueueMode::OneAtATime);

    if let Some(state) = args.get("initialState") {
        if !state.is_object() {
            return Err("initialState must be an object".to_string());
        }
        if let Some(messages) = state.get("messages") {
            if !messages.is_array() {
                return Err("initialState.messages must be an array".to_string());
            }
        }
        if let Some(tools) = state.get("tools") {
            if !tools.is_array() {
                return Err("initialState.tools must be an array".to_string());
            }
        }
    }

    let mut queues = AgentQueues::new(steering_mode, follow_up_mode);
    let mut results: Vec<Value> = Vec::with_capacity(ops.len());
    for op in ops {
        let obj = op
            .as_object()
            .ok_or_else(|| "ops entries must be objects".to_string())?;
        let name = obj
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing op".to_string())?;
        let result = match name {
            "steer" | "followUp" => {
                let message = obj
                    .get("message")
                    .ok_or_else(|| "missing message".to_string())?;
                if name == "steer" {
                    queues.steer(message.clone());
                } else {
                    queues.follow_up(message.clone());
                }
                Value::Null
            }
            "setSteeringMode" | "setFollowUpMode" => {
                let mode = require_mode(obj.get("mode"))?;
                if name == "setSteeringMode" {
                    queues.set_steering_mode(mode);
                } else {
                    queues.set_follow_up_mode(mode);
                }
                Value::Null
            }
            "getSteeringMode" => Value::String(queues.steering_mode().as_str().to_string()),
            "getFollowUpMode" => Value::String(queues.follow_up_mode().as_str().to_string()),
            "clearSteering" => {
                queues.clear_steering_queue();
                Value::Null
            }
            "clearFollowUp" => {
                queues.clear_follow_up_queue();
                Value::Null
            }
            "clearAll" => {
                queues.clear_all_queues();
                Value::Null
            }
            "hasQueued" => Value::Bool(queues.has_queued_messages()),
            "peek" => Value::Array(queues.peek_queued_messages()),
            other => return Err(format!("unknown agent-queue op: {other}")),
        };
        results.push(result);
    }
    Ok(json!({ "result": { "results": results } }))
}

/// Parse an optional top-level mode field (`None`/`null` → absent).
fn parse_mode_field(value: Option<&Value>, what: &str) -> Result<Option<QueueMode>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => QueueMode::parse(s)
            .ok_or_else(|| format!("{what} must be \"all\" or \"one-at-a-time\""))
            .map(Some),
        Some(_) => Err(format!("{what} must be \"all\" or \"one-at-a-time\"")),
    }
}

/// Parse a required mode field (op-level `mode`): `null`/absent →
/// "missing mode", anything but the two literals → the shared error.
fn require_mode(value: Option<&Value>) -> Result<QueueMode, String> {
    match value {
        None | Some(Value::Null) => Err("missing mode".to_string()),
        Some(Value::String(s)) => QueueMode::parse(s)
            .ok_or_else(|| "mode must be \"all\" or \"one-at-a-time\"".to_string()),
        Some(_) => Err("mode must be \"all\" or \"one-at-a-time\"".to_string()),
    }
}

// ============================================================================
// declare-tool-changes
// ============================================================================

/// `declare-tool-changes` harness: the reduced fixed pipeline of what the
/// provider sees on the first request (no tool calls, single turn).
/// `{contextMessages, contextTools, pendingMessages, nowMs}` →
/// `{llmMessages}` — `declare_tool_changes` + `default_convert_to_llm` over
/// `[...contextMessages, ...pending]`.
pub fn run_declare_tool_changes_harness(args: &Value, _cwd: &Path) -> Result<Value, String> {
    let context_messages = args
        .get("contextMessages")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing contextMessages".to_string())?;
    let context_tools = args
        .get("contextTools")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing contextTools".to_string())?;
    let pending_messages = args
        .get("pendingMessages")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing pendingMessages".to_string())?;
    let now_ms = args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())?;

    let pending = pi_agent::conversion::declare_tool_changes(
        context_messages,
        context_tools,
        pending_messages,
        now_ms,
    );
    let mut all = context_messages.clone();
    all.extend(pending);
    let llm_messages = pi_agent::conversion::default_convert_to_llm(&all);
    Ok(json!({ "result": { "llmMessages": llm_messages } }))
}

// ============================================================================
// run-tool-call
// ============================================================================

use std::sync::atomic::{AtomicUsize, Ordering};

use pi_agent::host::Clock;
use pi_agent::tool_call::{AfterToolCallInput, ToolUpdateSink};
use pi_agent::types::{AgentContext, BeforeToolCallResult};

/// Scripted tool driven by the harness `execute` script (update/return/throw
/// steps) and the shared perf clock. Each step with `perfNow` advances the
/// clock; steps with `update` emit a partial result; the last step returns
/// the result or throws.
struct ScriptedTool {
    spec: Value,
    clock: Arc<SharedPerfClock>,
}

impl AgentTool for ScriptedTool {
    fn name(&self) -> &str {
        self.spec.get("name").and_then(Value::as_str).unwrap_or("")
    }
    fn description(&self) -> &str {
        self.spec.get("description").and_then(Value::as_str).unwrap_or("")
    }
    fn label(&self) -> &str {
        self.spec.get("label").and_then(Value::as_str).unwrap_or("")
    }
    fn parameters(&self) -> &Value {
        self.spec.get("parameters").unwrap_or(&Value::Null)
    }
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.spec
            .get("executionMode")
            .and_then(Value::as_str)
            .and_then(ToolExecutionMode::parse)
    }
    fn prepare_arguments(&self, args: &Value) -> Option<Value> {
        let prepare = self.spec.get("prepareArguments")?;
        match prepare.get("mode").and_then(Value::as_str) {
            Some("identity") => None,
            Some("script") => {
                let pairs = prepare.get("script").and_then(Value::as_array)?;
                for pair in pairs {
                    if pair.get("in") == Some(args) {
                        return pair.get("out").cloned();
                    }
                }
                None
            }
            _ => None,
        }
    }
    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: &'a Value,
        _signal: Option<&'a pi_agent::host::AbortHandle>,
        on_update: Option<&'a pi_agent::types::AgentToolUpdateCallback<'a>>,
    ) -> pi_agent::types::BoxFuture<'a, Result<AgentToolResult, String>> {
        let script = self
            .spec
            .get("execute")
            .and_then(|e| e.get("script"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            let mut steps = script.iter().peekable();
            while let Some(step) = steps.next() {
                if let Some(perf_now) = step.get("perfNow").and_then(Value::as_f64) {
                    clock.set(perf_now);
                    clock.perf_now_ms(); // advance (pop) the scripted sequence
                }
                if let Some(update) = step.get("update") {
                    if let Some(sink) = on_update {
                        sink(parse_tool_result(update));
                    }
                }
                if steps.peek().is_none() {
                    if let Some(message) = step.get("throw").and_then(Value::as_str) {
                        return Err(message.to_string());
                    }
                    if let Some(result) = step.get("return") {
                        return Ok(parse_tool_result(result));
                    }
                }
            }
            Ok(AgentToolResult::default())
        })
    }
}

fn parse_tool_result(value: &Value) -> AgentToolResult {
    AgentToolResult {
        content: value
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        details: value.get("details").cloned().unwrap_or(Value::Null),
        structured_content: value.get("structuredContent").filter(|v| !v.is_null()).cloned(),
        usage: value.get("usage").filter(|v| !v.is_null()).cloned(),
        is_error: value.get("isError").and_then(Value::as_bool),
        terminate: value.get("terminate").and_then(Value::as_bool),
    }
}

/// Shared perf clock: `perf_now_ms` pops the scripted sequence (last value
/// repeats); the tool script can set the current value directly.
struct SharedPerfClock {
    seq: Mutex<Vec<f64>>,
    set_value: Mutex<Option<f64>>,
    calls: AtomicUsize,
}

impl SharedPerfClock {
    fn new(seq: Vec<f64>) -> Self {
        SharedPerfClock {
            seq: Mutex::new(seq),
            set_value: Mutex::new(None),
            calls: AtomicUsize::new(0),
        }
    }
    fn set(&self, value: f64) {
        *self.set_value.lock().expect("perf clock") = Some(value);
    }
}

impl Clock for SharedPerfClock {
    fn date_now_ms(&self) -> f64 {
        0.0
    }
    fn perf_now_ms(&self) -> f64 {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(value) = *self.set_value.lock().expect("perf clock") {
            return value;
        }
        let seq = self.seq.lock().expect("perf clock");
        let n = self.calls.load(Ordering::SeqCst) - 1;
        seq.get(n).or(seq.last()).copied().unwrap_or(0.0)
    }
}

fn parse_before(value: &Value) -> Result<Option<BeforeToolCallResult>, String> {
    if let Some(message) = value.get("throw").and_then(Value::as_str) {
        return Err(message.to_string());
    }
    Ok(Some(BeforeToolCallResult {
        block: value.get("block").and_then(Value::as_bool),
        reason: value.get("reason").and_then(Value::as_str).map(str::to_string),
        terminate: value.get("terminate").and_then(Value::as_bool),
    }))
}

fn parse_after(value: &Value) -> Result<Option<AfterToolCallInput>, String> {
    if let Some(message) = value.get("throw").and_then(Value::as_str) {
        return Err(message.to_string());
    }
    let present = |key: &str| value.get(key).filter(|v| !v.is_null()).cloned();
    Ok(Some(AfterToolCallInput {
        content: present("content").map(|c| c.as_array().cloned().unwrap_or_default()),
        details: present("details"),
        structured_content: present("structuredContent"),
        is_error: value.get("isError").and_then(Value::as_bool),
        usage: present("usage"),
        terminate: value.get("terminate").and_then(Value::as_bool),
    }))
}

/// `run-tool-call` harness: run the scripted pipeline and report the outcome
/// plus every recorded partial result. Timestamps: none (runToolCall adds no
/// messages).
pub fn run_run_tool_call_harness(args: &Value, _cwd: &Path) -> Result<Value, String> {
    let tool_call = args.get("toolCall").ok_or_else(|| "missing toolCall".to_string())?.clone();
    let tool_specs = args
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing tools".to_string())?
        .clone();
    let assistant_message = args
        .get("assistantMessage")
        .ok_or_else(|| "missing assistantMessage".to_string())?
        .clone();
    let context_messages = args
        .get("contextMessages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let perf_seq = args
        .get("perfSeq")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();

    let clock = Arc::new(SharedPerfClock::new(perf_seq));
    let tools: Vec<Arc<dyn AgentTool>> = tool_specs
        .iter()
        .map(|spec| Arc::new(ScriptedTool { spec: spec.clone(), clock: Arc::clone(&clock) }) as Arc<dyn AgentTool>)
        .collect();

    let context = AgentContext { messages: context_messages, tools: None };

    let before_spec = args.get("beforeToolCall").cloned();
    let after_spec = args.get("afterToolCall").cloned();

    let updates: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    let outcome = pi_agent::run_current_thread(async {
        let before = before_spec.map(|spec| {
            move |assistant: &Value, call: &Value, args_value: &Value, ctx: &AgentContext| {
                let _ = (assistant, call, args_value, ctx);
                let result = parse_before(&spec);
                Box::pin(async move { result }) as pi_agent::types::BoxFuture<'static, Result<Option<BeforeToolCallResult>, String>>
            }
        });
        let after = after_spec.map(|spec| {
            move |assistant: &Value, call: &Value, args_value: &Value, _result: &AgentToolResult, _is_error: bool, ctx: &AgentContext| {
                let _ = (assistant, call, args_value, ctx);
                let result = parse_after(&spec);
                Box::pin(async move { result }) as pi_agent::types::BoxFuture<'static, Result<Option<AfterToolCallInput>, String>>
            }
        });
        let sink_updates = Arc::clone(&updates);
        let sink = move |partial: AgentToolResult| {
            sink_updates
                .lock()
                .expect("update sink")
                .push(serde_json::to_value(&partial).expect("AgentToolResult serializes"));
        };
        pi_agent::tool_call::run_tool_call(
            &tool_call,
            &assistant_message,
            &context,
            &tools,
            before.as_ref().map(|f| f as &pi_agent::tool_call::BeforeToolCallHook),
            after.as_ref().map(|f| f as &pi_agent::tool_call::AfterToolCallHook),
            None,
            Some(&*Box::leak(Box::new(sink)) as &ToolUpdateSink),
            clock.as_ref(),
        )
        .await
    });

    let updates = updates.lock().expect("update sink").clone();
    Ok(json!({
        "result": {
            "outcome": serde_json::to_value(&outcome).expect("outcome serializes"),
            "updates": updates,
        }
    }))
}

// ============================================================================
// agent (class Agent parity)
// ============================================================================

/// Shared environment for the agent-entry scripted tools: the perf clock,
/// the snapshot log, and the agent cell (the tools are constructed before
/// the `Agent`, so they reach it through this shared cell — the tool
/// snapshot/steer/followUp/abort steps observe and drive the live agent).
struct AgentToolEnv {
    perf: Arc<Mutex<f64>>,
    snapshots: Arc<Mutex<Vec<Value>>>,
    agent: Arc<Mutex<Option<pi_agent::Agent>>>,
}

/// Scripted tool: the agent-loop ScriptedTool shape plus the
/// steer/followUp/abort/snapshot step kinds. Step processing order:
/// perfNow, steer/followUp/abort/snapshot, update, then (last step)
/// throw/return.
struct AgentScriptedTool {
    spec: Value,
    env: Arc<AgentToolEnv>,
}

impl AgentTool for AgentScriptedTool {
    fn name(&self) -> &str {
        self.spec.get("name").and_then(Value::as_str).unwrap_or("")
    }
    fn description(&self) -> &str {
        self.spec.get("description").and_then(Value::as_str).unwrap_or("")
    }
    fn label(&self) -> &str {
        match self.spec.get("label").and_then(Value::as_str) {
            Some(label) => label,
            None => self.name(),
        }
    }
    fn parameters(&self) -> &Value {
        self.spec.get("parameters").unwrap_or(&Value::Null)
    }
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.spec
            .get("executionMode")
            .and_then(Value::as_str)
            .and_then(ToolExecutionMode::parse)
    }
    fn prepare_arguments(&self, args: &Value) -> Option<Value> {
        let prepare = self.spec.get("prepareArguments")?;
        match prepare.get("mode").and_then(Value::as_str) {
            Some("script") => {
                let pairs = prepare.get("script").and_then(Value::as_array)?;
                for pair in pairs {
                    if pair.get("in") == Some(args) {
                        return pair.get("out").cloned();
                    }
                }
                None
            }
            _ => None,
        }
    }
    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        _args: &'a Value,
        _signal: Option<&'a pi_agent::host::AbortHandle>,
        on_update: Option<&'a pi_agent::types::AgentToolUpdateCallback<'a>>,
    ) -> BoxFuture<'a, Result<AgentToolResult, String>> {
        let script = self
            .spec
            .get("execute")
            .and_then(|e| e.get("script"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let env = Arc::clone(&self.env);
        Box::pin(async move {
            let mut steps = script.iter().peekable();
            while let Some(step) = steps.next() {
                if let Some(perf_now) = step.get("perfNow").and_then(Value::as_f64) {
                    *env.perf.lock().expect("perf clock") = perf_now;
                }
                if let Some(steer) = step.get("steer") {
                    if let Some(agent) = env.agent.lock().expect("agent cell").as_ref() {
                        agent.steer(steer.clone());
                    }
                }
                if let Some(follow_up) = step.get("followUp") {
                    if let Some(agent) = env.agent.lock().expect("agent cell").as_ref() {
                        agent.follow_up(follow_up.clone());
                    }
                }
                if step.get("abort") == Some(&Value::Bool(true)) {
                    if let Some(agent) = env.agent.lock().expect("agent cell").as_ref() {
                        agent.abort();
                    }
                }
                if let Some(tag) = step.get("snapshot") {
                    let state = env
                        .agent
                        .lock()
                        .expect("agent cell")
                        .as_ref()
                        .map(|agent| agent.state_snapshot())
                        .unwrap_or(Value::Null);
                    env.snapshots
                        .lock()
                        .expect("snapshot log")
                        .push(json!({ "tag": tag, "state": state }));
                }
                if let Some(update) = step.get("update") {
                    if let Some(sink) = on_update {
                        sink(crate::agent_loop_harness::parse_tool_result(update));
                    }
                }
                if steps.peek().is_none() {
                    if let Some(message) = step.get("throw").and_then(Value::as_str) {
                        return Err(message.to_string());
                    }
                    if let Some(result) = step.get("return") {
                        return Ok(crate::agent_loop_harness::parse_tool_result(result));
                    }
                }
            }
            Ok(AgentToolResult::default())
        })
    }
}

/// A settle:false op: either a spawned run (polled concurrently on the
/// current-thread runtime, mirroring the un-awaited JS promise) or an
/// immediate start error (the JS rejected promise); the error, if any,
/// becomes a thrown entry once awaited at script end.
enum PendingRun {
    Spawned(tokio::task::JoinHandle<Option<String>>),
    Failed(String),
}

/// The `agent` harness entry (shared wire protocol in
/// /home/shinobi/agent-t5-protocol.md): the real `Agent` (crates/pi-agent
/// agent.rs, port of packages/agent agent.ts) over scripted provider
/// turns, scripted tools (snapshot/steer/followUp/abort steps), one
/// event-recording listener, and a script of harness ops.
pub fn run_agent_harness(args: &Value, _cwd: &Path) -> Result<Value, String> {
    use pi_agent::host::Clock;
    use pi_agent::r#loop::LoopStreamFn;
    use pi_agent::types::{AgentEvent, QueueMode};

    // Validation order per the shared protocol spec (byte-exact errors):
    // script, providerTurns, nowMs, initialState, then tool shape.
    let script: Vec<Value> = args
        .get("script")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing script".to_string())?
        .clone();
    let provider_turns: Vec<Value> = args
        .get("providerTurns")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing providerTurns".to_string())?
        .clone();
    let now_ms = args
        .get("nowMs")
        .and_then(Value::as_f64)
        .ok_or_else(|| "missing nowMs".to_string())?;
    let initial_state: Value = match args.get("initialState") {
        None | Some(Value::Null) => json!({}),
        Some(state) if state.is_object() => state.clone(),
        _ => return Err("initialState must be an object".to_string()),
    };
    if let Some(messages) = initial_state.get("messages") {
        if !messages.is_array() {
            return Err("initialState.messages must be an array".to_string());
        }
    }
    if let Some(tools) = initial_state.get("tools") {
        if !tools.is_array() {
            return Err("initialState.tools must be an array".to_string());
        }
    }
    if let Some(tools) = args.get("tools") {
        if !tools.is_array() {
            return Err("tools must be an array".to_string());
        }
    }
    let tool_specs: Vec<Value> = match args.get("tools").and_then(Value::as_array) {
        Some(specs) => specs.clone(),
        None => initial_state
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    // Tool/step shape errors (same list as the agent-loop entry).
    for tool in &tool_specs {
        if !tool.is_object() {
            return Err("tools entries must be objects".to_string());
        }
        if tool.get("name").and_then(Value::as_str).is_none() {
            return Err("missing tool name".to_string());
        }
    }
    let perf_seq: Vec<f64> = args
        .get("perfSeq")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();

    // Pinned clocks: Date.now → nowMs, performance.now → a single settable
    // value starting at perfSeq[0] (or 0); execute steps set it via perfNow.
    let perf = Arc::new(Mutex::new(perf_seq.first().copied().unwrap_or(0.0)));
    let clock: Arc<dyn Clock> =
        Arc::new(crate::agent_loop_harness::RunClock { now_ms, perf: Arc::clone(&perf) });
    let snapshots: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let agent_cell: Arc<Mutex<Option<pi_agent::Agent>>> = Arc::new(Mutex::new(None));
    let env = Arc::new(AgentToolEnv {
        perf: Arc::clone(&perf),
        snapshots: Arc::clone(&snapshots),
        agent: Arc::clone(&agent_cell),
    });
    let make_tool = {
        let env = Arc::clone(&env);
        move |spec: &Value| -> Arc<dyn AgentTool> {
            Arc::new(AgentScriptedTool { spec: spec.clone(), env: Arc::clone(&env) })
        }
    };
    let tools: Vec<Arc<dyn AgentTool>> = tool_specs.iter().map(make_tool).collect();

    // Fake provider popping providerTurns[i] per stream call; exhaustion
    // throws (the in-band error path reaching handleRunFailure).
    let provider_cursor = Arc::new(Mutex::new(0usize));
    let stream_fn: Arc<LoopStreamFn> = {
        let turns = provider_turns;
        let cursor = Arc::clone(&provider_cursor);
        Arc::new(move |_model: &Value, _context: &Value, _options: &Value| {
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
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

    // TS createMutableAgentState + constructor defaults.
    let parse_mode = |key: &str| -> QueueMode {
        initial_state
            .get(key)
            .and_then(Value::as_str)
            .and_then(QueueMode::parse)
            .unwrap_or_default()
    };
    let agent = pi_agent::Agent::new(pi_agent::AgentOptions {
        messages: initial_state
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        tools,
        system_prompt: initial_state
            .get("systemPrompt")
            .and_then(Value::as_str)
            .map(str::to_string),
        model: initial_state
            .get("model")
            .cloned()
            .unwrap_or_else(pi_agent::default_model),
        thinking_level: initial_state
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .unwrap_or("off")
            .to_string(),
        steering_mode: parse_mode("steeringMode"),
        follow_up_mode: parse_mode("followUpMode"),
        stream_fn,
        tool_execution: ToolExecutionMode::default(),
        clock: Arc::clone(&clock),
    });
    *agent_cell.lock().expect("agent cell") = Some(agent.clone());

    // One listener recording every event.
    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let sink = Arc::clone(&events);
        agent.subscribe(Arc::new(
            move |event: AgentEvent, _signal: pi_agent::host::AbortHandle| {
                sink.lock().expect("event recorder").push(event.to_value());
                Box::pin(async {}) as BoxFuture<'static, ()>
            },
        ));
    }

    let (results, state_messages) = pi_agent::run_current_thread(async {
        let mut results: Vec<Value> = Vec::with_capacity(script.len());
        let mut pending: Vec<PendingRun> = Vec::new();
        for op in &script {
            let obj = match op.as_object() {
                Some(obj) => obj,
                None => {
                    results.push(json!({ "thrown": "ops entries must be objects" }));
                    continue;
                }
            };
            let name = match obj.get("op").and_then(Value::as_str) {
                Some(name) => name,
                None => {
                    results.push(json!({ "thrown": "missing op" }));
                    continue;
                }
            };
            let settle_false = obj.get("settle") == Some(&Value::Bool(false));
            match name {
                "prompt" | "continue" => {
                    let started = if name == "prompt" {
                        // normalizePromptInput: messages array | input string
                        // (user message with timestamp nowMs) | single message.
                        let input = obj.get("messages").or_else(|| obj.get("input"));
                        match input {
                            Some(Value::Array(messages)) => agent.prompt(messages.clone()),
                            Some(Value::String(text)) => {
                                agent.prompt(vec![agent.normalize_prompt_input(text)])
                            }
                            Some(single) => agent.prompt(vec![single.clone()]),
                            None => agent.prompt(Vec::new()),
                        }
                    } else {
                        agent.continue_run()
                    };
                    match started {
                        Ok(run) => {
                            if settle_false {
                                pending.push(PendingRun::Spawned(tokio::spawn(async move {
                                    run.await.err()
                                })));
                                results.push(Value::Null);
                            } else {
                                match run.await {
                                    Ok(()) => results.push(Value::Null),
                                    Err(message) => results.push(json!({ "thrown": message })),
                                }
                            }
                        }
                        Err(message) => {
                            if settle_false {
                                pending.push(PendingRun::Failed(message));
                                results.push(Value::Null);
                            } else {
                                results.push(json!({ "thrown": message }));
                            }
                        }
                    }
                }
                "steer" | "followUp" => {
                    let outcome = match obj.get("message") {
                        Some(message) => {
                            if name == "steer" {
                                agent.steer(message.clone());
                            } else {
                                agent.follow_up(message.clone());
                            }
                            Ok(())
                        }
                        None => Err("missing message".to_string()),
                    };
                    results.push(match outcome {
                        Ok(()) => Value::Null,
                        Err(message) => json!({ "thrown": message }),
                    });
                }
                "abort" => {
                    agent.abort();
                    results.push(Value::Null);
                }
                "waitForIdle" => {
                    agent.wait_for_idle().await;
                    results.push(Value::Null);
                }
                "reset" => match agent.reset() {
                    Ok(()) => results.push(Value::Null),
                    Err(message) => results.push(json!({ "thrown": message })),
                },
                "getState" => results.push(agent.state_snapshot()),
                other => results.push(json!({ "thrown": format!("unknown op: {other}") })),
            }
        }
        // Await the pending settle:false ops; rejected ones surface as
        // thrown entries (in pending order).
        for run in pending {
            let thrown = match run {
                PendingRun::Spawned(handle) => handle.await.unwrap_or_default(), // the wrapper never panics
                PendingRun::Failed(message) => Some(message),
            };
            if let Some(message) = thrown {
                results.push(json!({ "thrown": message }));
            }
        }
        (results, agent.messages())
    });

    let events = events.lock().expect("event recorder").clone();
    let snapshots = snapshots.lock().expect("snapshot log").clone();
    Ok(json!({
        "result": {
            "events": events,
            "stateMessages": state_messages,
            "results": results,
            "snapshots": snapshots,
        }
    }))
}
