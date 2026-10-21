//! Port of the agent loop from `packages/agent/src/agent-loop.ts` (lines
//! 102–283 and 522–680): `runAgentLoop`, `runAgentLoopContinue`, `runLoop`,
//! `executeToolCalls` (sequential/parallel), and
//! `shouldTerminateToolBatch`, driving the shared `tool_call` pipeline.
//!
//! Everything runs on the current-thread runtime (plan §3 G8): parallel
//! tool batches join futures in order, which matches `Promise.all` over
//! instantly-ready scripted tools — including the TS microtask ordering
//! where a thunk that finds the signal already aborted emits its
//! `tool_execution_end` during the Promise.all map (before the ends of
//! thunks that executed; see the `al-abort-step-mid-run` parity case).

use std::sync::Arc;

use serde_json::{Map, Value};



use crate::conversion::declare_tool_changes;
use crate::host::{AbortHandle, Clock, FixedClock};
use crate::stream_assistant::stream_assistant_response;
use crate::tool_call::{
    create_error_tool_result, create_tool_result_message, finalize_executed_tool_call,
    prepare_tool_call, execute_prepared_tool_call, AfterToolCallHook, BeforeToolCallHook,
    Preparation,
};
use crate::types::{
    AgentContext, AgentEvent, AgentTool, AgentToolCallOutcome, AgentToolResult, BoxFuture,
    ToolExecutionMode,
};

// ============================================================================
// Event sink
// ============================================================================

/// Emit sink for `AgentEvent`s (TS `AgentEventSink`). The loop awaits the
/// sink per event, keeping the TS await points.
pub trait AgentEventSink: Send + Sync {
    fn emit<'a>(&'a self, event: AgentEvent) -> BoxFuture<'a, ()>;
}

/// Sink wrapper for sync closures (the event is delivered at call time,
/// like an async function whose body completes synchronously).
pub struct FnSink<F>(pub F);

impl<F: Fn(AgentEvent) + Send + Sync> AgentEventSink for FnSink<F> {
    fn emit<'a>(&'a self, event: AgentEvent) -> BoxFuture<'a, ()> {
        (self.0)(event);
        Box::pin(async {})
    }
}

// ============================================================================
// Context transforms
// ============================================================================

/// TS `config.convertToLlm` — the message filter applied at the LLM
/// boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConvertToLlm {
    /// Keep system/user/assistant/toolResult roles (`defaultConvertToLlm`).
    #[default]
    Default,
    /// The default filter minus assistant messages.
    DropAssistant,
}

impl ConvertToLlm {
    pub fn apply(&self, messages: &[Value]) -> Vec<Value> {
        messages
            .iter()
            .filter(|m| {
                let role = m.get("role").and_then(Value::as_str);
                match self {
                    ConvertToLlm::Default => matches!(
                        role,
                        Some("system") | Some("user") | Some("assistant") | Some("toolResult")
                    ),
                    ConvertToLlm::DropAssistant => {
                        matches!(role, Some("system") | Some("user") | Some("toolResult"))
                    }
                }
            })
            .cloned()
            .collect()
    }
}

/// TS `config.transformContext` — applied to context messages before
/// `convertToLlm`.
#[derive(Clone)]
pub enum TransformContext {
    Identity,
    DropSystem,
    /// A shared hook (TS `agent.transformContext`, e.g. the session's
    /// forced-prompt projection installed per run).
    Custom(Arc<dyn Fn(&[Value]) -> Vec<Value> + Send + Sync>),
}

impl TransformContext {
    pub fn apply(&self, messages: &[Value]) -> Vec<Value> {
        match self {
            TransformContext::Custom(hook) => hook(messages),
            TransformContext::Identity => messages.to_vec(),
            TransformContext::DropSystem => messages
                .iter()
                .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
                .cloned()
                .collect(),
        }
    }
}

// ============================================================================
// Hooks
// ============================================================================

/// TS `config.getApiKey` — returns the resolved key or `None`
/// (`undefined`); an empty string is falsy and falls through to
/// `config.apiKey`.
pub type GetApiKeyHook =
    dyn Fn(&str) -> BoxFuture<'static, Option<String>> + Send + Sync;

/// TS `config.getSteeringMessages` / `getFollowUpMessages`.
pub type MessagePollHook = dyn Fn() -> BoxFuture<'static, Vec<Value>> + Send + Sync;

/// The `lastCompletedTurn` snapshot passed to `finishTurn` and
/// `prepareNextTurn` (TS `PrepareNextTurnContext`, owned copy).
pub struct CompletedTurn {
    pub message: Value,
    pub tool_results: Vec<Value>,
    pub context: AgentContext,
    pub new_messages: Vec<Value>,
}

/// TS `config.finishTurn` — returns the decision action string
/// (`"end"` / `"continue"`) or `None` (`undefined` → normal scheduling).
pub type FinishTurnHook =
    dyn Fn(&CompletedTurn) -> BoxFuture<'static, Option<String>> + Send + Sync;

/// TS `config.prepareRequest` — receives `(context, model, thinkingLevel)`.
pub type PrepareRequestHook = dyn
    Fn(&AgentContext, &Value, &str) -> BoxFuture<'static, Option<AgentRequestUpdate>>
    + Send
    + Sync;

/// TS `config.prepareNextTurn`.
pub type PrepareNextTurnHook =
    dyn Fn(&CompletedTurn) -> BoxFuture<'static, Option<AgentLoopTurnUpdate>> + Send + Sync;

/// TS `AgentRequestUpdate`: `undefined` fields keep the current values.
/// (No `Debug`: `AgentContext` has trait-object fields.)
#[derive(Clone, Default)]
pub struct AgentRequestUpdate {
    pub context: Option<AgentContext>,
    pub model: Option<Value>,
    pub thinking_level: Option<String>,
}

/// TS `AgentLoopTurnUpdate`. (No `Debug`: `AgentContext` has trait-object fields.)
#[derive(Clone, Default)]
pub struct AgentLoopTurnUpdate {
    pub context: Option<AgentContext>,
    pub messages: Option<Vec<Value>>,
    pub model: Option<Value>,
    pub thinking_level: Option<String>,
}

/// The stream function seam of the loop (TS `StreamFn`). `Err` models a
/// throw, which propagates out of the loop (test fakes only).
pub type LoopStreamFn = dyn
    Fn(&Value, &Value, &Value) -> BoxFuture<'static, Result<crate::provider::StreamOutput, String>>
    + Send
    + Sync;

// ============================================================================
// LoopConfig
// ============================================================================

/// TS `AgentLoopConfig` (the fields observable through the loop).
#[derive(Clone)]
pub struct LoopConfig {
    pub model: Value,
    pub reasoning: Option<String>,
    pub api_key: Option<String>,
    pub get_api_key: Option<Arc<GetApiKeyHook>>,
    pub convert_to_llm: ConvertToLlm,
    pub transform_context: Option<TransformContext>,
    pub tool_execution: ToolExecutionMode,
    pub get_steering_messages: Option<Arc<MessagePollHook>>,
    pub get_follow_up_messages: Option<Arc<MessagePollHook>>,
    pub finish_turn: Option<Arc<FinishTurnHook>>,
    pub prepare_request: Option<Arc<PrepareRequestHook>>,
    pub prepare_next_turn: Option<Arc<PrepareNextTurnHook>>,
    pub before_tool_call: Option<Arc<BeforeToolCallHook>>,
    pub after_tool_call: Option<Arc<AfterToolCallHook>>,
    pub clock: Arc<dyn Clock>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        LoopConfig {
            model: Value::Null,
            reasoning: None,
            api_key: None,
            get_api_key: None,
            convert_to_llm: ConvertToLlm::default(),
            transform_context: None,
            tool_execution: ToolExecutionMode::default(),
            get_steering_messages: None,
            get_follow_up_messages: None,
            finish_turn: None,
            prepare_request: None,
            prepare_next_turn: None,
            before_tool_call: None,
            after_tool_call: None,
            clock: Arc::new(FixedClock::new(vec![], vec![])),
        }
    }
}

/// TS reasoning update: `undefined` keeps the current value, `"off"` maps
/// to `undefined`, anything else replaces it.
fn apply_thinking_level(current: &mut Option<String>, update: Option<String>) {
    match update {
        None => {}
        Some(level) if level == "off" => *current = None,
        Some(level) => *current = Some(level),
    }
}

/// TS `toToolDeclaration` over executable tools: `{name, description,
/// parameters}` (the trait has no `constrainedSampling`).
pub(crate) fn tool_declarations(tools: &[Arc<dyn AgentTool>]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut map = Map::new();
            map.insert("name".to_string(), Value::String(tool.name().to_string()));
            map.insert("description".to_string(), Value::String(tool.description().to_string()));
            map.insert("parameters".to_string(), tool.parameters().clone());
            Value::Object(map)
        })
        .collect()
}

/// A finalized batch of tool results (TS `ExecutedToolCallBatch`).
pub struct ExecutedToolBatch {
    pub messages: Vec<Value>,
    pub terminate: bool,
}

// ============================================================================
// runAgentLoop / runAgentLoopContinue
// ============================================================================

/// TS `runAgentLoop` — start an agent loop with new prompt messages.
/// `Err` models a rejection (e.g. a throwing stream function).
pub async fn run_agent_loop(
    prompts: &[Value],
    context: AgentContext,
    config: &LoopConfig,
    emit: &dyn AgentEventSink,
    signal: Option<&AbortHandle>,
    stream_fn: &LoopStreamFn,
) -> Result<Vec<Value>, String> {
    let tools = context.tools.clone().unwrap_or_default();
    let initial_messages = declare_tool_changes(
        &context.messages,
        &tool_declarations(&tools),
        prompts,
        config.clock.date_now_ms(),
    );
    let mut new_messages: Vec<Value> = initial_messages.clone();
    let mut messages = context.messages.clone();
    messages.extend(initial_messages.iter().cloned());
    let mut current_context = AgentContext { messages, tools: context.tools };

    emit.emit(AgentEvent::AgentStart).await;
    emit.emit(AgentEvent::TurnStart).await;
    for message in &initial_messages {
        emit.emit(AgentEvent::MessageStart { message: message.clone() }).await;
        emit.emit(AgentEvent::MessageEnd { message: message.clone() }).await;
    }

    run_loop(&mut current_context, &mut new_messages, config, signal, emit, stream_fn).await?;
    Ok(new_messages)
}

/// TS `runAgentLoopContinue` — continue from the current context without
/// adding a new message. Guard errors are byte-exact.
pub async fn run_agent_loop_continue(
    context: AgentContext,
    config: &LoopConfig,
    emit: &dyn AgentEventSink,
    signal: Option<&AbortHandle>,
    stream_fn: &LoopStreamFn,
) -> Result<Vec<Value>, String> {
    if context.messages.is_empty() {
        return Err("Cannot continue: no messages in context".to_string());
    }
    if context
        .messages
        .last()
        .and_then(|m| m.get("role").and_then(Value::as_str))
        == Some("assistant")
    {
        return Err("Cannot continue from message role: assistant".to_string());
    }

    let mut new_messages: Vec<Value> = Vec::new();
    let mut current_context = context;

    emit.emit(AgentEvent::AgentStart).await;
    emit.emit(AgentEvent::TurnStart).await;

    run_loop(&mut current_context, &mut new_messages, config, signal, emit, stream_fn).await?;
    Ok(new_messages)
}

/// TS in-loop continuation (a `finishTurn` hook returning `continue`
/// iterates the loop): emits `turn_start` but no `agent_start`, and has no
/// entry guards.
pub async fn run_agent_loop_resume(
    context: AgentContext,
    config: &LoopConfig,
    emit: &dyn AgentEventSink,
    signal: Option<&AbortHandle>,
    stream_fn: &LoopStreamFn,
) -> Result<Vec<Value>, String> {
    let mut new_messages: Vec<Value> = Vec::new();
    let mut current_context = context;

    emit.emit(AgentEvent::TurnStart).await;

    run_loop(&mut current_context, &mut new_messages, config, signal, emit, stream_fn).await?;
    Ok(new_messages)
}

// ============================================================================
// runLoop
// ============================================================================

/// TS `runLoop` — the main loop logic shared by both entries.
async fn run_loop(
    context: &mut AgentContext,
    new_messages: &mut Vec<Value>,
    initial_config: &LoopConfig,
    signal: Option<&AbortHandle>,
    emit: &dyn AgentEventSink,
    stream_fn: &LoopStreamFn,
) -> Result<(), String> {
    // TS mutates the config object (model/reasoning) via the prepare hooks.
    let mut config = initial_config.clone();
    let mut last_completed_turn: Option<CompletedTurn> = None;
    let mut explicit_continuation = false;
    // Check for steering messages at start (user may have typed while waiting)
    let mut pending_messages: Vec<Value> = match &config.get_steering_messages {
        Some(hook) => hook().await,
        None => Vec::new(),
    };

    // Outer loop: continues when queued follow-up messages arrive after the
    // agent would stop.
    loop {
        let mut has_more_tool_calls = true;

        // Inner loop: process tool calls and steering messages.
        while has_more_tool_calls || !pending_messages.is_empty() {
            let mut prepared_messages: Vec<Value> = Vec::new();
            if let Some(turn) = &last_completed_turn {
                if let Some(hook) = &config.prepare_next_turn {
                    if let Some(update) = hook(turn).await {
                        if let Some(next_context) = update.context {
                            *context = next_context;
                        }
                        prepared_messages = update.messages.unwrap_or_default();
                        if let Some(model) = update.model {
                            config.model = model;
                        }
                        apply_thinking_level(&mut config.reasoning, update.thinking_level);
                    }
                }
                // Preparation can be long-running; pick up steering queued
                // while it ran. Only poll again if the earlier poll returned
                // nothing (one-at-a-time would otherwise double-deliver).
                if pending_messages.is_empty() {
                    pending_messages = match &config.get_steering_messages {
                        Some(hook) => hook().await,
                        None => Vec::new(),
                    };
                }
                emit.emit(AgentEvent::TurnStart).await;
            }

            // Process prepared and queued messages before the next response.
            let tools = context.tools.clone().unwrap_or_default();
            let mut combined = prepared_messages;
            combined.append(&mut pending_messages);
            let declared = declare_tool_changes(
                &context.messages,
                &tool_declarations(&tools),
                &combined,
                config.clock.date_now_ms(),
            );
            for message in declared {
                emit.emit(AgentEvent::MessageStart { message: message.clone() }).await;
                emit.emit(AgentEvent::MessageEnd { message: message.clone() }).await;
                context.messages.push(message.clone());
                new_messages.push(message);
            }

            if let Some(hook) = &config.prepare_request {
                let thinking_level =
                    config.reasoning.clone().unwrap_or_else(|| "off".to_string());
                if let Some(update) = hook(context, &config.model.clone(), &thinking_level).await {
                    if let Some(next_context) = update.context {
                        *context = next_context;
                    }
                    if let Some(model) = update.model {
                        config.model = model;
                    }
                    apply_thinking_level(&mut config.reasoning, update.thinking_level);
                }
            }

            // Stream assistant response
            let message =
                stream_assistant_response(context, &config, signal, emit, stream_fn).await?;
            new_messages.push(message.clone());

            let stop_reason = message.get("stopReason").and_then(Value::as_str).unwrap_or("");
            if stop_reason == "error" || stop_reason == "aborted" {
                let turn = CompletedTurn {
                    message: message.clone(),
                    tool_results: Vec::new(),
                    context: context.clone(),
                    new_messages: new_messages.clone(),
                };
                if let Some(hook) = &config.finish_turn {
                    hook(&turn).await;
                }
                emit.emit(AgentEvent::TurnEnd { message, tool_results: Vec::new() }).await;
                emit.emit(AgentEvent::AgentEnd { messages: new_messages.clone() }).await;
                return Ok(());
            }

            // Check for tool calls
            let tool_calls: Vec<Value> = message
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter(|c| c.get("type").and_then(Value::as_str) == Some("toolCall"))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();

            let mut tool_results: Vec<Value> = Vec::new();
            has_more_tool_calls = false;
            if !tool_calls.is_empty() {
                // A "length" stop means the output was cut off by the token
                // limit; fail every tool call instead of executing borked
                // arguments.
                let executed_batch = if stop_reason == "length" {
                    fail_tool_calls_from_truncated_message(
                        &tool_calls,
                        emit,
                        config.clock.date_now_ms(),
                    )
                    .await
                } else {
                    execute_tool_calls(context, &message, &tool_calls, &config, signal, emit).await
                };
                tool_results = executed_batch.messages;
                has_more_tool_calls = !executed_batch.terminate;

                for result in tool_results.clone() {
                    context.messages.push(result.clone());
                    new_messages.push(result);
                }
            }

            let turn = CompletedTurn {
                message: message.clone(),
                tool_results: tool_results.clone(),
                context: context.clone(),
                new_messages: new_messages.clone(),
            };
            let decision = match &config.finish_turn {
                Some(hook) => hook(&turn).await,
                None => None,
            };
            last_completed_turn = Some(turn);
            emit.emit(AgentEvent::TurnEnd { message, tool_results: tool_results.clone() }).await;

            if decision.as_deref() == Some("end") {
                emit.emit(AgentEvent::AgentEnd { messages: new_messages.clone() }).await;
                return Ok(());
            }

            explicit_continuation = decision.as_deref() == Some("continue");
            pending_messages = match &config.get_steering_messages {
                Some(hook) => hook().await,
                None => Vec::new(),
            };
            if has_more_tool_calls || !pending_messages.is_empty() {
                explicit_continuation = false;
            }
        }

        // Agent would stop here. Check for follow-up messages.
        let follow_up_messages = match &config.get_follow_up_messages {
            Some(hook) => hook().await,
            None => Vec::new(),
        };
        if !follow_up_messages.is_empty() {
            explicit_continuation = false;
            pending_messages = follow_up_messages;
            continue;
        }

        // Fulfill the continuation decision with one context-only turn.
        if explicit_continuation {
            explicit_continuation = false;
            continue;
        }

        break;
    }

    emit.emit(AgentEvent::AgentEnd { messages: new_messages.clone() }).await;
    Ok(())
}

// ============================================================================
// Tool-call execution
// ============================================================================

/// TS `shouldTerminateToolBatch` — terminate only when every finalized
/// result has `terminate === true` and there is at least one.
fn should_terminate_tool_batch(finalized_calls: &[AgentToolCallOutcome]) -> bool {
    !finalized_calls.is_empty()
        && finalized_calls
            .iter()
            .all(|finalized| finalized.result.terminate == Some(true))
}

fn tool_call_str(tool_call: &Value, key: &str) -> String {
    tool_call
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

async fn emit_tool_execution_end(
    finalized: &AgentToolCallOutcome,
    emit: &dyn AgentEventSink,
) {
    emit.emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: tool_call_str(&finalized.tool_call, "id"),
        tool_name: tool_call_str(&finalized.tool_call, "name"),
        result: serde_json::to_value(&finalized.result).expect("AgentToolResult serializes"),
        is_error: finalized.is_error,
        duration_ms: finalized.duration_ms,
    })
    .await;
}

async fn emit_tool_result_message(
    tool_result_message: &Value,
    emit: &dyn AgentEventSink,
) {
    emit.emit(AgentEvent::MessageStart { message: tool_result_message.clone() }).await;
    emit.emit(AgentEvent::MessageEnd { message: tool_result_message.clone() }).await;
}

/// TS `failToolCallsFromTruncatedMessage`.
async fn fail_tool_calls_from_truncated_message(
    tool_calls: &[Value],
    emit: &dyn AgentEventSink,
    date_now_ms: f64,
) -> ExecutedToolBatch {
    let mut messages: Vec<Value> = Vec::new();
    for tool_call in tool_calls {
        emit.emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call_str(tool_call, "id"),
            tool_name: tool_call_str(tool_call, "name"),
            args: tool_call.get("arguments").cloned().unwrap_or(Value::Null),
        })
        .await;
        let name = tool_call_str(tool_call, "name");
        let finalized = AgentToolCallOutcome {
            tool_call: tool_call.clone(),
            result: create_error_tool_result(&format!(
                "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                name
            )),
            is_error: true,
            duration_ms: None,
        };
        emit_tool_execution_end(&finalized, emit).await;
        let tool_result_message = create_tool_result_message(&finalized, date_now_ms);
        emit_tool_result_message(&tool_result_message, emit).await;
        messages.push(tool_result_message);
    }
    ExecutedToolBatch { messages, terminate: false }
}

/// TS `executeToolCalls` — dispatch on config mode / per-tool
/// `executionMode`.
async fn execute_tool_calls(
    current_context: &AgentContext,
    assistant_message: &Value,
    tool_calls: &[Value],
    config: &LoopConfig,
    signal: Option<&AbortHandle>,
    emit: &dyn AgentEventSink,
) -> ExecutedToolBatch {
    let tools = current_context.tools.clone().unwrap_or_default();
    let has_sequential_tool_call = tool_calls.iter().any(|tc| {
        tools
            .iter()
            .find(|t| t.name() == tool_call_str(tc, "name").as_str())
            .and_then(|tool| tool.execution_mode())
            == Some(ToolExecutionMode::Sequential)
    });
    if config.tool_execution == ToolExecutionMode::Sequential || has_sequential_tool_call {
        execute_tool_calls_sequential(
            current_context,
            assistant_message,
            tool_calls,
            config,
            signal,
            emit,
        )
        .await
    } else {
        execute_tool_calls_parallel(
            current_context,
            assistant_message,
            tool_calls,
            config,
            signal,
            emit,
        )
        .await
    }
}

/// Execute one prepared tool call, emitting buffered
/// `tool_execution_update` events (TS pushes them during `execute`;
/// ordering relative to the end event is preserved).
async fn run_prepared_tool_call(
    current_context: &AgentContext,
    assistant_message: &Value,
    tool_call: &Value,
    preparation: &Preparation,
    config: &LoopConfig,
    signal: Option<&AbortHandle>,
    emit: &dyn AgentEventSink,
) -> AgentToolCallOutcome {
    let update_buffer: Arc<std::sync::Mutex<Vec<AgentToolResult>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let buffer = Arc::clone(&update_buffer);
    let update_sink = move |partial: AgentToolResult| {
        buffer.lock().expect("update buffer").push(partial);
    };
    let executed =
        execute_prepared_tool_call(preparation, signal, Some(&update_sink), config.clock.as_ref())
            .await;
    let buffered_updates: Vec<_> = update_buffer.lock().expect("update buffer").drain(..).collect();
    for partial in buffered_updates {
        emit.emit(AgentEvent::ToolExecutionUpdate {
            tool_call_id: tool_call_str(tool_call, "id"),
            tool_name: tool_call_str(tool_call, "name"),
            args: tool_call.get("arguments").cloned().unwrap_or(Value::Null),
            partial_result: serde_json::to_value(&partial).expect("AgentToolResult serializes"),
        })
        .await;
    }
    finalize_executed_tool_call(
        current_context,
        assistant_message,
        preparation,
        executed,
        config.after_tool_call.as_deref(),
        signal,
    )
    .await
}

/// TS `executeToolCallsSequential`.
async fn execute_tool_calls_sequential(
    current_context: &AgentContext,
    assistant_message: &Value,
    tool_calls: &[Value],
    config: &LoopConfig,
    signal: Option<&AbortHandle>,
    emit: &dyn AgentEventSink,
) -> ExecutedToolBatch {
    let tools = current_context.tools.clone().unwrap_or_default();
    let mut finalized_calls: Vec<AgentToolCallOutcome> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    for tool_call in tool_calls {
        emit.emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call_str(tool_call, "id"),
            tool_name: tool_call_str(tool_call, "name"),
            args: tool_call.get("arguments").cloned().unwrap_or(Value::Null),
        })
        .await;

        let preparation = prepare_tool_call(
            current_context,
            assistant_message,
            tool_call,
            &tools,
            config.before_tool_call.as_deref(),
            signal,
        )
        .await;
        let finalized = match preparation {
            Preparation::Immediate { result, is_error } => AgentToolCallOutcome {
                tool_call: tool_call.clone(),
                result,
                is_error,
                duration_ms: None,
            },
            prepared @ Preparation::Prepared { .. } => {
                run_prepared_tool_call(
                    current_context,
                    assistant_message,
                    tool_call,
                    &prepared,
                    config,
                    signal,
                    emit,
                )
                .await
            }
        };

        emit_tool_execution_end(&finalized, emit).await;
        let tool_result_message = create_tool_result_message(&finalized, config.clock.date_now_ms());
        emit_tool_result_message(&tool_result_message, emit).await;
        finalized_calls.push(finalized);
        messages.push(tool_result_message);

        if signal.is_some_and(|s| s.is_aborted()) {
            break;
        }
    }

    ExecutedToolBatch {
        messages,
        terminate: should_terminate_tool_batch(&finalized_calls),
    }
}

/// TS `executeToolCallsParallel`. The two-phase structure reproduces the
/// TS observable ordering of `Promise.all` with instantly-ready tools:
/// phase 2 (the map calling the thunks) runs each thunk's synchronous
/// prefix — the abort check and the tool `execute` — emitting the
/// `tool_execution_end` of thunks that found the signal aborted; phase 3
/// emits the ends of the executed thunks, in order.
async fn execute_tool_calls_parallel(
    current_context: &AgentContext,
    assistant_message: &Value,
    tool_calls: &[Value],
    config: &LoopConfig,
    signal: Option<&AbortHandle>,
    emit: &dyn AgentEventSink,
) -> ExecutedToolBatch {
    let tools = current_context.tools.clone().unwrap_or_default();
    enum Entry {
        Finalized(AgentToolCallOutcome),
        Thunk { tool_call: Value, preparation: Preparation },
    }
    let mut entries: Vec<Entry> = Vec::new();

    // Preparation loop.
    for tool_call in tool_calls {
        emit.emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call_str(tool_call, "id"),
            tool_name: tool_call_str(tool_call, "name"),
            args: tool_call.get("arguments").cloned().unwrap_or(Value::Null),
        })
        .await;

        let preparation = prepare_tool_call(
            current_context,
            assistant_message,
            tool_call,
            &tools,
            config.before_tool_call.as_deref(),
            signal,
        )
        .await;
        match preparation {
            Preparation::Immediate { result, is_error } => {
                let finalized = AgentToolCallOutcome {
                    tool_call: tool_call.clone(),
                    result,
                    is_error,
                    duration_ms: None,
                };
                emit_tool_execution_end(&finalized, emit).await;
                entries.push(Entry::Finalized(finalized));
            }
            prepared => entries.push(Entry::Thunk { tool_call: tool_call.clone(), preparation: prepared }),
        }
        if signal.is_some_and(|s| s.is_aborted()) {
            break;
        }
    }

    // Phase 2: run the thunks' sync prefixes (abort check + execute +
    // finalize). Aborted thunks emit their end here; executed thunks
    // defer the end to phase 3.
    enum Slot {
        Done(AgentToolCallOutcome),
        PendingEnd(AgentToolCallOutcome),
    }
    let mut slots: Vec<Slot> = Vec::new();
    for entry in entries {
        match entry {
            Entry::Finalized(finalized) => slots.push(Slot::Done(finalized)),
            Entry::Thunk { tool_call, preparation } => {
                if signal.is_some_and(|s| s.is_aborted()) {
                    let finalized = AgentToolCallOutcome {
                        tool_call: tool_call.clone(),
                        result: create_error_tool_result("Operation aborted"),
                        is_error: true,
                        duration_ms: None,
                    };
                    emit_tool_execution_end(&finalized, emit).await;
                    slots.push(Slot::Done(finalized));
                } else {
                    let finalized = run_prepared_tool_call(
                        current_context,
                        assistant_message,
                        &tool_call,
                        &preparation,
                        config,
                        signal,
                        emit,
                    )
                    .await;
                    slots.push(Slot::PendingEnd(finalized));
                }
            }
        }
    }

    // Phase 3: emit the ends of the executed thunks, in order.
    for slot in &slots {
        if let Slot::PendingEnd(finalized) = slot {
            emit_tool_execution_end(finalized, emit).await;
        }
    }

    let finalized_calls: Vec<AgentToolCallOutcome> = slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Done(finalized) | Slot::PendingEnd(finalized) => finalized,
        })
        .collect();
    let mut messages: Vec<Value> = Vec::new();
    for finalized in &finalized_calls {
        let tool_result_message = create_tool_result_message(finalized, config.clock.date_now_ms());
        emit_tool_result_message(&tool_result_message, emit).await;
        messages.push(tool_result_message);
    }

    ExecutedToolBatch {
        messages,
        terminate: should_terminate_tool_batch(&finalized_calls),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::StreamOutput;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct Recorder(Arc<Mutex<Vec<Value>>>);
    impl AgentEventSink for Recorder {
        fn emit<'a>(&'a self, event: AgentEvent) -> BoxFuture<'a, ()> {
            self.0.lock().unwrap().push(event.to_value());
            Box::pin(async {})
        }
    }

    fn assistant(content: Value, stop_reason: &str) -> Value {
        json!({ "role": "assistant", "content": content, "stopReason": stop_reason, "timestamp": 1 })
    }

    fn turn(result: Value) -> StreamOutput {
        StreamOutput { events: vec![json!({ "type": "done", "reason": "stop" })].into(), result }
    }

    fn user(text: &str) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": text }], "timestamp": 1 })
    }

    fn stream_fn(turns: Vec<Value>) -> Arc<LoopStreamFn> {
        let turns = Arc::new(Mutex::new(turns));
        Arc::new(move |_m: &Value, _c: &Value, _o: &Value| {
            let turns = Arc::clone(&turns);
            Box::pin(async move {
                let mut queue = turns.lock().unwrap();
                if queue.is_empty() {
                    return Err("provider script exhausted".to_string());
                }
                Ok(turn(queue.remove(0)))
            })
        })
    }

    fn run(prompts: Vec<Value>, turns: Vec<Value>, config: &LoopConfig) -> (Vec<Value>, Vec<Value>) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let sink = Recorder(Arc::clone(&recorded));
        let messages = crate::run_current_thread(run_agent_loop(
            &prompts,
            AgentContext { messages: vec![], tools: None },
            config,
            &sink,
            None,
            &*stream_fn(turns),
        ))
        .unwrap();
        let events = recorded.lock().unwrap().clone();
        (events, messages)
    }

    #[test]
    fn hard_exit_on_error_stop_reason() {
        let (events, _messages) = run(
            vec![user("hi")],
            vec![assistant(json!([]), "error")],
            &LoopConfig::default(),
        );
        let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["agent_start", "turn_start", "message_start", "message_end", "message_start", "message_end", "turn_end", "agent_end"]);
        assert_eq!(events[6]["toolResults"], json!([]));
    }

    #[test]
    fn truncated_tool_calls_fail_without_executing() {
        let tool_call = json!({ "type": "toolCall", "id": "c1", "name": "read", "arguments": { "path": "x" } });
        let (events, messages) = run(
            vec![user("x")],
            vec![assistant(json!([tool_call]), "length"), assistant(json!([]), "stop")],
            &LoopConfig::default(),
        );
        let end = events.iter().find(|e| e["type"] == "tool_execution_end").unwrap();
        assert_eq!(end["result"]["content"][0]["text"].as_str().unwrap(),
            "Tool call \"read\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.");
        assert!(end.get("durationMs").is_none());
        // newMessages: [user, assistant(length), toolResult] — the assistant
        // message is pushed before its tool results (TS ordering).
        assert_eq!(messages[1]["role"], json!("assistant"));
        assert_eq!(messages[2]["role"], json!("toolResult"));
        assert_eq!(messages[2]["isError"], json!(true));
    }

    #[test]
    fn continue_guards() {
        let sink = Recorder(Arc::new(Mutex::new(Vec::new())));
        let config = LoopConfig::default();
        let err = crate::run_current_thread(run_agent_loop_continue(
            AgentContext::default(),
            &config,
            &sink,
            None,
            &*stream_fn(vec![]),
        ))
        .unwrap_err();
        assert_eq!(err, "Cannot continue: no messages in context");
        let err = crate::run_current_thread(run_agent_loop_continue(
            AgentContext { messages: vec![assistant(json!([]), "stop")], tools: None },
            &config,
            &sink,
            None,
            &*stream_fn(vec![]),
        ))
        .unwrap_err();
        assert_eq!(err, "Cannot continue from message role: assistant");
    }

    #[test]
    fn explicit_continuation_runs_a_context_only_turn() {
        let decisions = Arc::new(Mutex::new(vec!["continue".to_string()]));
        let decision_source = Arc::clone(&decisions);
        let mut config = LoopConfig::default();
        config.finish_turn = Some(Arc::new(move |_turn: &CompletedTurn| {
            let decision = decision_source.lock().unwrap().pop();
            Box::pin(async move { decision })
        }));
        let (events, _messages) = run(
            vec![user("hi")],
            vec![assistant(json!([]), "stop"), assistant(json!([]), "stop")],
            &config,
        );
        let turn_starts = events.iter().filter(|e| e["type"] == "turn_start").count();
        assert_eq!(turn_starts, 2, "one context-only extra turn");
    }

    #[test]
    fn steering_repolled_only_when_earlier_poll_was_empty() {
        // Turn 1 carries a tool call so the inner loop iterates again; the
        // post-turn steering poll (poll 2) returns nothing, so the re-poll
        // after prepareNextTurn runs (poll 3) and its message is processed
        // before the second request.
        let polls = Arc::new(Mutex::new(vec![
            vec![user("first")],   // loop start
            vec![],                // after turn 1 (steering poll)
            vec![user("second")],  // re-poll after prepareNextTurn (poll 2 was empty)
        ]));
        let source = Arc::clone(&polls);
        let mut config = LoopConfig::default();
        config.get_steering_messages = Some(Arc::new(move || {
            let mut src = source.lock().unwrap();
            let poll = if src.is_empty() { Vec::new() } else { src.remove(0) };
            Box::pin(async move { poll })
        }));
        let tool_call = json!({ "type": "toolCall", "id": "c1", "name": "read", "arguments": {} });
        let (events, messages) = run(
            vec![user("hi")],
            vec![assistant(json!([tool_call]), "stop"), assistant(json!([]), "stop")],
            &config,
        );
        // [user hi, first, assistant1, toolResult (tool not found), second, assistant2]
        assert_eq!(messages.len(), 6);
        let user_texts: Vec<&str> = messages
            .iter()
            .filter(|m| m["role"] == "user")
            .filter_map(|m| m["content"][0]["text"].as_str())
            .collect();
        assert_eq!(user_texts, ["hi", "first", "second"]);
        assert_eq!(events.iter().filter(|e| e["type"] == "turn_start").count(), 2);
    }

    #[test]
    fn should_terminate_requires_all_and_at_least_one() {
        fn outcome(terminate: Option<bool>) -> AgentToolCallOutcome {
            AgentToolCallOutcome {
                tool_call: json!({ "id": "c" }),
                result: AgentToolResult { terminate, ..Default::default() },
                is_error: false,
                duration_ms: None,
            }
        }
        assert!(!should_terminate_tool_batch(&[]));
        assert!(!should_terminate_tool_batch(&[outcome(None)]));
        assert!(!should_terminate_tool_batch(&[outcome(Some(true)), outcome(None)]));
        assert!(should_terminate_tool_batch(&[outcome(Some(true)), outcome(Some(true))]));
    }
}
