//! Parity-harness entries for tranches 3+4 (shared wire protocol in
//! /home/shinobi/agent-t34-protocol.md): `stream-assistant` and
//! `agent-loop` — `runAgentLoop` driving `streamAssistantResponse` (single
//! scripted turn) and the full loop (multi-turn fake provider, scripted
//! tools with steer/followUp/abort steps, hook scripts, pinned clocks).
//!
//! Tool/step shape errors (the complete list, mirrored by the TS side):
//! - "tools entries must be objects": a tools entry that is null, an
//!   array, or not an object
//! - "missing tool name": a tools entry whose `name` is not a string

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use pi_agent::host::{AbortHandle, Clock};
use pi_agent::provider::StreamOutput;
use pi_agent::r#loop::{
    AgentEventSink, AgentLoopTurnUpdate, AgentRequestUpdate, CompletedTurn, ConvertToLlm,
    LoopConfig, TransformContext,
};
use pi_agent::types::{
    AgentContext, AgentEvent, AgentTool, AgentToolResult, BeforeToolCallResult, BoxFuture,
    ToolExecutionMode,
};
use pi_agent::tool_call::AfterToolCallInput;

// ============================================================================
// Shared pieces
// ============================================================================

/// Default model (protocol spec, exact JSON / key order).
fn default_model() -> Value {
    json!({
        "id": "unknown",
        "name": "unknown",
        "api": "unknown",
        "provider": "unknown",
        "baseUrl": "",
        "reasoning": false,
        "input": [],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 0,
        "maxTokens": 0,
    })
}

/// Shared-field validation (both entries; order: contextMessages,
/// transformContext, convertToLlm).
fn validate_shared_fields(args: &Value) -> Result<(), String> {
    if let Some(messages) = args.get("contextMessages") {
        if !messages.is_array() {
            return Err("contextMessages must be an array".to_string());
        }
    }
    if let Some(mode) = args.get("transformContext") {
        if !matches!(mode.as_str(), Some("identity") | Some("dropSystem")) {
            return Err("transformContext must be \"identity\" or \"dropSystem\"".to_string());
        }
    }
    if let Some(mode) = args.get("convertToLlm") {
        if !matches!(mode.as_str(), Some("default") | Some("dropAssistant")) {
            return Err("convertToLlm must be \"default\" or \"dropAssistant\"".to_string());
        }
    }
    Ok(())
}

fn parse_now_ms(args: &Value) -> Result<f64, String> {
    args.get("nowMs").and_then(Value::as_f64).ok_or_else(|| "missing nowMs".to_string())
}

/// Pinned clocks: `Date.now` → nowMs, `performance.now` → a single
/// settable value starting at `perfSeq[0]` (or 0); execute steps set it
/// via `perfNow`.
struct RunClock {
    now_ms: f64,
    perf: Arc<Mutex<f64>>,
}

impl Clock for RunClock {
    fn date_now_ms(&self) -> f64 {
        self.now_ms
    }
    fn perf_now_ms(&self) -> f64 {
        *self.perf.lock().expect("perf clock")
    }
}

/// Shared harness state the scripted tools and hooks operate on.
struct ToolEnv {
    perf: Arc<Mutex<f64>>,
    steer_buffer: Arc<Mutex<Vec<Value>>>,
    follow_up_buffer: Arc<Mutex<Vec<Value>>>,
    abort_handle: AbortHandle,
}

/// Update-script context entry `{messages?, tools?}` → `AgentContext` (a
/// free fn so 'static hook closures can call it with a cloned env Arc).
fn map_update_context(value: &Value, env: &Arc<ToolEnv>) -> AgentContext {
    AgentContext {
        messages: value
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        tools: Some(
            value
                .get("tools")
                .and_then(Value::as_array)
                .map(|specs| {
                    specs
                        .iter()
                        .map(|spec| {
                            Arc::new(LoopScriptedTool { spec: spec.clone(), env: Arc::clone(env) })
                                as Arc<dyn AgentTool>
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
    }
}

/// Scripted tool: the run-tool-call ScriptedTool shape plus the
/// steer/followUp/abort step kinds. Step processing order: perfNow, then
/// steer/followUp/abort, then update, then (last step) throw/return.
struct LoopScriptedTool {
    spec: Value,
    env: Arc<ToolEnv>,
}

impl AgentTool for LoopScriptedTool {
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
                    env.steer_buffer.lock().expect("steer buffer").push(steer.clone());
                }
                if let Some(follow_up) = step.get("followUp") {
                    env.follow_up_buffer.lock().expect("follow-up buffer").push(follow_up.clone());
                }
                if step.get("abort") == Some(&Value::Bool(true)) {
                    env.abort_handle.abort();
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
        content: value.get("content").and_then(Value::as_array).cloned().unwrap_or_default(),
        details: value.get("details").cloned().unwrap_or(Value::Null),
        structured_content: value.get("structuredContent").filter(|v| !v.is_null()).cloned(),
        usage: value.get("usage").filter(|v| !v.is_null()).cloned(),
        is_error: value.get("isError").and_then(Value::as_bool),
        terminate: value.get("terminate").and_then(Value::as_bool),
    }
}

/// The run-tool-call before/after hook shims (parityMakeBefore/After).
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

/// Emit recorder: pushes every AgentEvent JSON into the events list.
struct RecorderSink(Arc<Mutex<Vec<Value>>>);

impl AgentEventSink for RecorderSink {
    fn emit<'a>(&'a self, event: AgentEvent) -> BoxFuture<'a, ()> {
        self.0.lock().expect("event recorder").push(event.to_value());
        Box::pin(async {})
    }
}

/// A counter-based script popper shared by the hook closures.
fn pop_index(counter: &Arc<Mutex<usize>>) -> usize {
    let mut index = counter.lock().expect("hook cursor");
    let value = *index;
    *index += 1;
    value
}

/// Build the getApiKey hook (absent field → None).
fn make_get_api_key(args: &Value) -> Option<Arc<pi_agent::r#loop::GetApiKeyHook>> {
    let script = args.get("getApiKeyScript").and_then(Value::as_array).cloned()?;
    let cursor = Arc::new(Mutex::new(0usize));
    Some(Arc::new(move |_provider: &str| {
        let value = script
            .get(pop_index(&cursor))
            .and_then(Value::as_str)
            .map(str::to_string);
        Box::pin(async move { value })
    }))
}

/// Build the base config shared by both entries.
fn base_config(args: &Value, _now_ms: f64, clock: Arc<dyn Clock>) -> LoopConfig {
    let get_api_key = make_get_api_key(args);
    LoopConfig {
        model: default_model(),
        reasoning: args.get("reasoning").and_then(Value::as_str).map(str::to_string),
        api_key: args.get("apiKey").and_then(Value::as_str).map(str::to_string),
        get_api_key,
        convert_to_llm: if args.get("convertToLlm").and_then(Value::as_str) == Some("dropAssistant")
        {
            ConvertToLlm::DropAssistant
        } else {
            ConvertToLlm::Default
        },
        transform_context: if args.get("transformContext").and_then(Value::as_str)
            == Some("dropSystem")
        {
            Some(TransformContext::DropSystem)
        } else {
            None
        },
        clock,
        ..LoopConfig::default()
    }
}

// ============================================================================
// stream-assistant
// ============================================================================

pub fn run_stream_assistant_harness(args: &Value, _cwd: &Path) -> Result<Value, String> {
    // Validation order per the shared protocol spec (byte-exact errors).
    let script = args
        .get("script")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing script".to_string())?
        .clone();
    let result = args.get("result").ok_or_else(|| "missing result".to_string())?.clone();
    let now_ms = parse_now_ms(args)?;
    if let Some(prompts) = args.get("prompts") {
        if !prompts.is_array() {
            return Err("prompts must be an array".to_string());
        }
    }
    validate_shared_fields(args)?;

    let prompts: Vec<Value> =
        args.get("prompts").and_then(Value::as_array).cloned().unwrap_or_default();
    let context_messages: Vec<Value> = args
        .get("contextMessages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let clock = Arc::new(RunClock { now_ms, perf: Arc::new(Mutex::new(0.0)) });
    let config = base_config(args, now_ms, clock);

    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let stream_calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    let new_messages = pi_agent::run_current_thread(async {
        let sink = RecorderSink(Arc::clone(&events));
        // The fake streamFn records {apiKey, model, messages} per call; the
        // final message comes from `result`, never the event payloads.
        let calls = Arc::clone(&stream_calls);
        let stream_fn = move |model: &Value, context: &Value, options: &Value| {
            let model = model.clone();
            let messages =
                context.get("messages").cloned().unwrap_or(Value::Null);
            let api_key = options
                .get("apiKey")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(Value::Null);
            let calls = Arc::clone(&calls);
            let events = script.clone();
            let result = result.clone();
            Box::pin(async move {
                calls.lock().expect("stream call log").push(json!({
                    "apiKey": api_key,
                    "model": model,
                    "messages": messages,
                }));
                Ok(StreamOutput { events: events.into(), result })
            }) as BoxFuture<'static, Result<StreamOutput, String>>
        };
        pi_agent::r#loop::run_agent_loop(
            &prompts,
            AgentContext { messages: context_messages, tools: Some(Vec::new()) },
            &config,
            &sink,
            None,
            &stream_fn,
        )
        .await
    })?;

    let events = events.lock().expect("event recorder").clone();
    let stream_calls = stream_calls.lock().expect("stream call log").clone();
    Ok(json!({
        "result": {
            "events": events,
            "newMessages": new_messages,
            "streamCalls": stream_calls,
        }
    }))
}

// ============================================================================
// agent-loop
// ============================================================================

pub fn run_agent_loop_harness(args: &Value, _cwd: &Path) -> Result<Value, String> {
    // Validation order per the shared protocol spec (byte-exact errors).
    let prompts: Vec<Value> = args
        .get("prompts")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing prompts".to_string())?
        .clone();
    let provider_turns: Vec<Value> = args
        .get("providerTurns")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing providerTurns".to_string())?
        .clone();
    if let Some(tools) = args.get("tools") {
        if !tools.is_array() {
            return Err("tools must be an array".to_string());
        }
    }
    if let Some(mode) = args.get("toolExecution") {
        if !matches!(mode.as_str(), Some("parallel") | Some("sequential")) {
            return Err("toolExecution must be \"parallel\" or \"sequential\"".to_string());
        }
    }
    let now_ms = parse_now_ms(args)?;
    validate_shared_fields(args)?;
    // Tool/step shape errors (the complete list; see the module docs).
    let tool_specs: Vec<Value> =
        args.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
    for tool in &tool_specs {
        if !tool.is_object() {
            return Err("tools entries must be objects".to_string());
        }
        if tool.get("name").and_then(Value::as_str).is_none() {
            return Err("missing tool name".to_string());
        }
    }

    let context_messages: Vec<Value> = args
        .get("contextMessages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let perf_seq: Vec<f64> = args
        .get("perfSeq")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();

    let perf = Arc::new(Mutex::new(perf_seq.first().copied().unwrap_or(0.0)));
    let steer_buffer: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let follow_up_buffer: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let abort_handle = AbortHandle::new();
    let env = Arc::new(ToolEnv {
        perf: Arc::clone(&perf),
        steer_buffer: Arc::clone(&steer_buffer),
        follow_up_buffer: Arc::clone(&follow_up_buffer),
        abort_handle: abort_handle.clone(),
    });

    let clock: Arc<dyn Clock> = Arc::new(RunClock { now_ms, perf: Arc::clone(&perf) });
    let mut config = base_config(args, now_ms, Arc::clone(&clock));
    config.tool_execution = args
        .get("toolExecution")
        .and_then(Value::as_str)
        .and_then(ToolExecutionMode::parse)
        .unwrap_or(ToolExecutionMode::Parallel);

    let make_tool = {
        let env = Arc::clone(&env);
        move |spec: &Value| -> Arc<dyn AgentTool> {
            Arc::new(LoopScriptedTool { spec: spec.clone(), env: Arc::clone(&env) })
        }
    };
    let tools: Vec<Arc<dyn AgentTool>> = tool_specs.iter().map(make_tool).collect();

    // Update-script context entries: {messages?, tools?} → AgentContext.
    // A free fn (not a closure) so both hook scripts can use it without
    // borrowing locals for 'static.

    // Absent field → hook undefined (the loop treats that as no poll);
    // present → each poll pops script[i] ?? [] after draining the harness
    // buffer.
    if let Some(script) = args.get("steeringScript").cloned() {
        let cursor = Arc::new(Mutex::new(0usize));
        let buffer = Arc::clone(&steer_buffer);
        config.get_steering_messages = Some(Arc::new(move || {
            let drained: Vec<Value> = buffer.lock().expect("steer buffer").drain(..).collect();
            let scripted = script
                .get(pop_index(&cursor))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let out = [drained, scripted].concat();
            Box::pin(async move { out })
        }));
    }
    if let Some(script) = args.get("followUpScript").cloned() {
        let cursor = Arc::new(Mutex::new(0usize));
        let buffer = Arc::clone(&follow_up_buffer);
        config.get_follow_up_messages = Some(Arc::new(move || {
            let drained: Vec<Value> = buffer.lock().expect("follow-up buffer").drain(..).collect();
            let scripted = script
                .get(pop_index(&cursor))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let out = [drained, scripted].concat();
            Box::pin(async move { out })
        }));
    }
    // Each call pops decisions[i] ?? null; null → undefined (normal
    // scheduling).
    if let Some(decisions) = args.get("finishTurnDecisions").cloned() {
        let cursor = Arc::new(Mutex::new(0usize));
        config.finish_turn = Some(Arc::new(move |_turn: &CompletedTurn| {
            let decision = decisions
                .get(pop_index(&cursor))
                .and_then(Value::as_str)
                .map(str::to_string);
            Box::pin(async move { decision })
        }));
    }
    if let Some(script) = args.get("prepareRequestScript").cloned() {
        let cursor = Arc::new(Mutex::new(0usize));
        let env = Arc::clone(&env);
        config.prepare_request = Some(Arc::new(
            move |_context: &AgentContext, _model: &Value, _thinking_level: &str| {
                let update = script
                    .get(pop_index(&cursor))
                    .filter(|entry| !entry.is_null())
                    .map(|entry| AgentRequestUpdate {
                        context: entry.get("context").map(|value| map_update_context(value, &env)),
                        model: entry.get("model").cloned(),
                        thinking_level: entry
                            .get("thinkingLevel")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    });
                Box::pin(async move { update })
            },
        ));
    }
    if let Some(script) = args.get("prepareNextTurnScript").cloned() {
        let cursor = Arc::new(Mutex::new(0usize));
        let env = Arc::clone(&env);
        config.prepare_next_turn = Some(Arc::new(move |_turn: &CompletedTurn| {
            let update = script
                .get(pop_index(&cursor))
                .filter(|entry| !entry.is_null())
                .map(|entry| AgentLoopTurnUpdate {
                    context: entry.get("context").map(|value| map_update_context(value, &env)),
                    messages: entry
                        .get("messages")
                        .and_then(Value::as_array)
                        .cloned(),
                    model: entry.get("model").cloned(),
                    thinking_level: entry
                        .get("thinkingLevel")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            Box::pin(async move { update })
        }));
    }
    if let Some(spec) = args.get("beforeToolCall").cloned() {
        config.before_tool_call = Some(Arc::new(
            move |_assistant: &Value, _call: &Value, _args: &Value, _ctx: &AgentContext| {
                let parsed = parse_before(&spec);
                Box::pin(async move { parsed })
            },
        ));
    }
    if let Some(spec) = args.get("afterToolCall").cloned() {
        config.after_tool_call = Some(Arc::new(
            move |_assistant: &Value, _call: &Value, _args: &Value, _result: &AgentToolResult,
                 _is_error: bool, _ctx: &AgentContext| {
                let parsed = parse_after(&spec);
                Box::pin(async move { parsed })
            },
        ));
    }

    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let stream_calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

    let outcome = pi_agent::run_current_thread(async {
        let sink = RecorderSink(Arc::clone(&events));
        // The fake provider pops providerTurns[i] per stream call; when the
        // script is exhausted it throws (the in-band error path).
        let turns = provider_turns;
        let cursor = Arc::new(Mutex::new(0usize));
        let calls = Arc::clone(&stream_calls);
        let stream_fn = move |model: &Value, context: &Value, options: &Value| {
            let model = model.clone();
            let messages = context.get("messages").cloned().unwrap_or(Value::Null);
            let api_key = options
                .get("apiKey")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(Value::Null);
            let calls = Arc::clone(&calls);
            let turns = turns.clone();
            let cursor = Arc::clone(&cursor);
            Box::pin(async move {
                let turn = match turns.get(pop_index(&cursor)) {
                    Some(turn) => turn.clone(),
                    None => return Err("provider script exhausted".to_string()),
                };
                calls.lock().expect("stream call log").push(json!({
                    "apiKey": api_key,
                    "model": model,
                    "messages": messages,
                }));
                Ok(StreamOutput {
                    events: turn
                        .get("events")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into(),
                    result: turn.get("result").cloned().unwrap_or(Value::Null),
                })
            }) as BoxFuture<'static, Result<StreamOutput, String>>
        };
        pi_agent::r#loop::run_agent_loop(
            &prompts,
            AgentContext { messages: context_messages, tools: Some(tools) },
            &config,
            &sink,
            Some(&abort_handle),
            &stream_fn,
        )
        .await
    });

    let events = events.lock().expect("event recorder").clone();
    let stream_calls = stream_calls.lock().expect("stream call log").clone();
    match outcome {
        Ok(new_messages) => Ok(json!({
            "result": {
                "events": events,
                "newMessages": new_messages,
                "streamCalls": stream_calls,
            }
        })),
        Err(message) => Ok(json!({ "error": message })),
    }
}
