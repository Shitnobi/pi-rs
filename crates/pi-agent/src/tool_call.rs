//! Port of the single tool-call pipeline from
//! `packages/agent/src/agent-loop.ts`: `prepareToolCall`,
//! `executePreparedToolCall`, `finalizeExecutedToolCall`, `runToolCall`,
//! `failToolCallsFromTruncatedMessage`, `createErrorToolResult`, and
//! `createToolResultMessage`.
//!
//! Hooks (`beforeToolCall`/`afterToolCall`) are injected closures returning
//! `Ok(None)` for a JS `undefined` result, `Ok(Some(result))` for a plain
//! object, and `Err(message)` for a thrown error. Clocks are the
//! [`crate::host::Clock`] seams (`performance.now` → `perf_now_ms`,
//! `Date.now` → `date_now_ms`).

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::host::{AbortHandle, Clock};
use crate::types::{
    AgentContext, AgentEvent, AgentTool, AgentToolCallOutcome, AgentToolResult,
    BeforeToolCallResult, BoxFuture,
};
use crate::validation::validate_tool_arguments;

/// `BeforeToolCall` hook: receives `(assistantMessage, toolCall, args,
/// context)`; `Err` = thrown error (message byte-exact).
pub type BeforeToolCallHook = dyn Fn(&Value, &Value, &Value, &AgentContext) -> BoxFuture<'static, Result<Option<BeforeToolCallResult>, String>>
    + Send
    + Sync;

/// `AfterToolCall` hook: receives `(assistantMessage, toolCall, args, result,
/// isError, context)`; `Err` = thrown error.
pub type AfterToolCallHook = dyn Fn(&Value, &Value, &Value, &AgentToolResult, bool, &AgentContext) -> BoxFuture<'static, Result<Option<AfterToolCallInput>, String>>
    + Send
    + Sync;

/// The plain-object `AfterToolCallResult`: each field is `None` when the TS
/// object omitted it (or set it to `null`, which `??` treats the same).
#[derive(Debug, Clone, Default)]
pub struct AfterToolCallInput {
    pub content: Option<Vec<Value>>,
    pub details: Option<Value>,
    pub structured_content: Option<Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Value>,
    pub terminate: Option<bool>,
}

/// `onUpdate` sink for streamed partial results.
pub type ToolUpdateSink = dyn Fn(AgentToolResult) + Send + Sync;

enum Preparation {
    Prepared { tool_call: Value, tool: Arc<dyn AgentTool>, args: Value },
    Immediate { result: AgentToolResult, is_error: bool },
}

struct Executed {
    result: AgentToolResult,
    is_error: bool,
    duration_ms: f64,
}

/// TS `createErrorToolResult`.
pub fn create_error_tool_result(message: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![serde_json::json!({ "type": "text", "text": message })],
        details: serde_json::json!({}),
        ..AgentToolResult::default()
    }
}

/// TS `prepareToolCallArguments` — apply the tool's `prepareArguments` shim.
/// `None` models both an absent shim and the same-reference return (the
/// identity shortcut); `Some` replaces the arguments in place.
fn prepare_tool_call_arguments(tool: &dyn AgentTool, tool_call: &Value) -> Value {
    match tool.prepare_arguments(tool_call.get("arguments").unwrap_or(&Value::Null)) {
        None => tool_call.clone(),
        Some(prepared) => {
            let mut out = tool_call.as_object().cloned().unwrap_or_default();
            out.insert("arguments".to_string(), prepared);
            Value::Object(out)
        }
    }
}

async fn prepare_tool_call(
    context: &AgentContext,
    assistant_message: &Value,
    tool_call: &Value,
    tools: &[Arc<dyn AgentTool>],
    before: Option<&BeforeToolCallHook>,
    signal: Option<&AbortHandle>,
) -> Preparation {
    let Some(tool) = tools.iter().find(|t| t.name() == tool_call.get("name").and_then(Value::as_str).unwrap_or("")) else {
        return Preparation::Immediate {
            result: create_error_tool_result(&format!(
                "Tool {} not found",
                tool_call.get("name").and_then(Value::as_str).unwrap_or("")
            )),
            is_error: true,
        };
    };

    // The TS try/catch around preparation: validation and hook throws
    // become immediate error results.
    let prepared_tool_call = prepare_tool_call_arguments(tool.as_ref(), tool_call);
    let validated_args = match validate_tool_arguments(
        tool.name(),
        tool.parameters(),
        &prepared_tool_call,
    ) {
        Ok(args) => args,
        Err(message) => {
            return Preparation::Immediate {
                result: create_error_tool_result(&message),
                is_error: true,
            }
        }
    };

    if let Some(hook) = before {
        match hook(assistant_message, tool_call, &validated_args, context).await {
            Ok(Some(before_result)) => {
                if signal.is_some_and(|s| s.is_aborted()) {
                    return Preparation::Immediate {
                        result: create_error_tool_result("Operation aborted"),
                        is_error: true,
                    };
                }
                if before_result.block.unwrap_or(false) {
                    let mut result = create_error_tool_result(
                        &before_result.reason.clone().filter(|r| !r.is_empty())
                            .unwrap_or_else(|| "Tool execution was blocked".to_string()),
                    );
                    if before_result.terminate == Some(true) {
                        result.terminate = Some(true);
                    }
                    return Preparation::Immediate { result, is_error: true };
                }
            }
            Ok(None) => {}
            Err(message) => {
                return Preparation::Immediate {
                    result: create_error_tool_result(&message),
                    is_error: true,
                }
            }
        }
    }
    if signal.is_some_and(|s| s.is_aborted()) {
        return Preparation::Immediate {
            result: create_error_tool_result("Operation aborted"),
            is_error: true,
        };
    }
    // TS `prepareToolCall` returns the ORIGINAL toolCall in the prepared arm;
    // the rewritten arguments live on in `args` (validated) for execution.
    Preparation::Prepared { tool_call: tool_call.clone(), tool: tool.clone(), args: validated_args }
}

async fn execute_prepared_tool_call(
    prepared: &Preparation,
    signal: Option<&AbortHandle>,
    on_update: Option<&ToolUpdateSink>,
    clock: &dyn Clock,
) -> Executed {
    let Preparation::Prepared { tool_call, tool, args } = prepared else {
        unreachable!("execute_prepared_tool_call requires a prepared call");
    };
    let started_at = clock.perf_now_ms();
    let update_cell = Arc::new(std::sync::Mutex::new(Vec::<AgentToolResult>::new()));
    let accepting_updates = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let outcome = {
        let updates = Arc::clone(&update_cell);
        let accepting = Arc::clone(&accepting_updates);
        let callback = move |partial: AgentToolResult| {
            if accepting.load(std::sync::atomic::Ordering::SeqCst) {
                updates.lock().expect("update sink").push(partial);
            }
        };
        tool.execute(tool_call.get("id").and_then(Value::as_str).unwrap_or(""), args, signal, Some(&callback)).await
    };

    let duration_ms = (clock.perf_now_ms() - started_at).round();
    accepting_updates.store(false, std::sync::atomic::Ordering::SeqCst);
    // Flush the pending update futures (TS: Promise.all after settle) —
    // the sync sink already recorded them in emission order.
    if let Some(sink) = on_update {
        for partial in update_cell.lock().expect("update sink").drain(..) {
            sink(partial);
        }
    }

    match outcome {
        Ok(result) => Executed {
            is_error: result.is_error == Some(true),
            result,
            duration_ms,
        },
        Err(message) => Executed {
            result: create_error_tool_result(&message),
            is_error: true,
            duration_ms,
        },
    }
}

async fn finalize_executed_tool_call(
    context: &AgentContext,
    assistant_message: &Value,
    prepared: &Preparation,
    executed: Executed,
    after: Option<&AfterToolCallHook>,
    signal: Option<&AbortHandle>,
) -> AgentToolCallOutcome {
    let Preparation::Prepared { tool_call, tool: _, args } = prepared else {
        unreachable!("finalize_executed_tool_call requires a prepared call");
    };
    let mut result = executed.result;
    let mut is_error = executed.is_error;

    if let Some(hook) = after {
        match hook(assistant_message, tool_call, args, &result, is_error, context).await {
            Ok(Some(after_result)) => {
                // Structured content not replaced along with the content may
                // no longer match it.
                let structured_content = after_result
                    .structured_content
                    .clone()
                    .or_else(|| {
                        if after_result.content.is_some() {
                            None
                        } else {
                            result.structured_content.clone()
                        }
                    });
                if let Some(content) = after_result.content {
                    result.content = content;
                }
                if let Some(details) = after_result.details {
                    result.details = details;
                }
                if let Some(usage) = after_result.usage {
                    result.usage = Some(usage);
                }
                if let Some(terminate) = after_result.terminate {
                    result.terminate = Some(terminate);
                }
                result.structured_content = structured_content;
                if let Some(is_error_override) = after_result.is_error {
                    is_error = is_error_override;
                }
            }
            Ok(None) => {}
            Err(message) => {
                result = create_error_tool_result(&message);
                is_error = true;
            }
        }
    }
    let _ = signal;

    AgentToolCallOutcome {
        tool_call: tool_call.clone(),
        result,
        is_error,
        duration_ms: Some(executed.duration_ms),
    }
}

/// TS `runToolCall` — one model-issued tool call through argument
/// preparation, schema validation, `beforeToolCall`, execution, and
/// `afterToolCall`. Emits no events, adds no messages, never fails: unknown
/// tools, validation errors, blocked calls, and thrown errors come back as
/// `is_error: true`.
#[allow(clippy::too_many_arguments)]
pub async fn run_tool_call(
    tool_call: &Value,
    assistant_message: &Value,
    context: &AgentContext,
    tools: &[Arc<dyn AgentTool>],
    before: Option<&BeforeToolCallHook>,
    after: Option<&AfterToolCallHook>,
    signal: Option<&AbortHandle>,
    on_update: Option<&ToolUpdateSink>,
    clock: &dyn Clock,
) -> AgentToolCallOutcome {
    let preparation =
        prepare_tool_call(context, assistant_message, tool_call, tools, before, signal).await;
    match preparation {
        Preparation::Immediate { result, is_error } => {
            AgentToolCallOutcome { tool_call: tool_call.clone(), result, is_error, duration_ms: None }
        }
        prepared @ Preparation::Prepared { .. } => {
            let executed =
                execute_prepared_tool_call(&prepared, signal, on_update, clock).await;
            finalize_executed_tool_call(context, assistant_message, &prepared, executed, after, signal).await
        }
    }
}

/// TS `createToolResultMessage` — the toolResult message for a finalized
/// outcome (`content ?? []` normalization; timestamp from the clock).
pub fn create_tool_result_message(finalized: &AgentToolCallOutcome, date_now_ms: f64) -> Value {
    let mut map = Map::new();
    map.insert("role".to_string(), Value::String("toolResult".to_string()));
    map.insert(
        "toolCallId".to_string(),
        finalized.tool_call.get("id").cloned().unwrap_or(Value::Null),
    );
    map.insert(
        "toolName".to_string(),
        finalized.tool_call.get("name").cloned().unwrap_or(Value::Null),
    );
    map.insert("content".to_string(), Value::Array(finalized.result.content.clone()));
    if !finalized.result.details.is_null() {
        map.insert("details".to_string(), finalized.result.details.clone());
    }
    if let Some(usage) = &finalized.result.usage {
        map.insert("usage".to_string(), usage.clone());
    }
    map.insert("isError".to_string(), Value::Bool(finalized.is_error));
    if let Some(duration_ms) = finalized.duration_ms {
        map.insert("durationMs".to_string(), pi_core::ai::js_number(duration_ms));
    }
    map.insert("timestamp".to_string(), pi_core::ai::js_number(date_now_ms));
    Value::Object(map)
}

/// A finalized batch of tool results (`ExecutedToolCallBatch`).
pub struct ExecutedToolBatch {
    pub messages: Vec<Value>,
    pub terminate: bool,
}

/// TS `failToolCallsFromTruncatedMessage` — fail every tool call of an
/// output-limit-truncated assistant message. Emits the same event sequence
/// through the (sync) sink; `terminate` is always false.
pub fn fail_tool_calls_from_truncated_message(
    tool_calls: &[Value],
    emit: &dyn Fn(AgentEvent),
    date_now_ms: f64,
) -> ExecutedToolBatch {
    let mut messages: Vec<Value> = Vec::new();
    for tool_call in tool_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            tool_name: tool_call.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            args: tool_call.get("arguments").cloned().unwrap_or(Value::Null),
        });
        let finalized = AgentToolCallOutcome {
            tool_call: tool_call.clone(),
            result: create_error_tool_result(&format!(
                "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                tool_call.get("name").and_then(Value::as_str).unwrap_or("")
            )),
            is_error: true,
            duration_ms: None,
        };
        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: tool_call.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            tool_name: tool_call.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            result: serde_json::to_value(&finalized.result).expect("AgentToolResult serializes"),
            is_error: true,
            duration_ms: None,
        });
        let tool_result_message = create_tool_result_message(&finalized, date_now_ms);
        emit(AgentEvent::MessageStart { message: tool_result_message.clone() });
        emit(AgentEvent::MessageEnd { message: tool_result_message.clone() });
        messages.push(tool_result_message);
    }
    ExecutedToolBatch { messages, terminate: false }
}
