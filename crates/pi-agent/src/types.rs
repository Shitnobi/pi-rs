//! Port of `packages/agent/src/types.ts` — the type surface of the agent
//! loop layer.
//!
//! Messages are `serde_json::Value` (the established pi-core pattern, see
//! `pi_core::ai`) so unknown fields and future custom message roles
//! round-trip exactly, mirroring the TS `AgentMessage` union that apps
//! extend via declaration merging.
//!
//! Tranche 0 of the phase-3 port (see docs/plans/phase3-agent-session.md):
//! the data model and the `AgentTool` trait boundary (plan §4). The loop
//! itself lands in later tranches.

use std::pin::Pin;

use serde_json::{Map, Value};

use crate::host::AbortHandle;

/// Owned, `Send` boxed future (the crate's `futures::future::BoxFuture`
/// equivalent; no extra dependencies — see the tranche spec).
pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Controls how many queued user messages are injected when the agent loop
/// reaches a queue drain point (TS `QueueMode`).
///
/// - `"all"`: drain and inject every queued message at that point.
/// - `"one-at-a-time"`: drain and inject only the oldest queued message,
///   leaving the rest queued for later drain points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueMode {
    All,
    OneAtATime,
}

impl QueueMode {
    /// The TS string literal for this mode.
    pub fn as_str(&self) -> &'static str {
        match self {
            QueueMode::All => "all",
            QueueMode::OneAtATime => "one-at-a-time",
        }
    }

    /// Parse the TS string literal; `None` for anything else.
    pub fn parse(s: &str) -> Option<QueueMode> {
        match s {
            "all" => Some(QueueMode::All),
            "one-at-a-time" => Some(QueueMode::OneAtATime),
            _ => None,
        }
    }
}

impl Default for QueueMode {
    /// TS default: both agent queues start `"one-at-a-time"`.
    fn default() -> Self {
        QueueMode::OneAtATime
    }
}

impl serde::Serialize for QueueMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for QueueMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        QueueMode::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown QueueMode: {s:?}")))
    }
}

/// Configuration for how tool calls from a single assistant message are
/// executed (TS `ToolExecutionMode`).
///
/// - `"sequential"`: each tool call is prepared, executed, and finalized
///   before the next one starts.
/// - `"parallel"`: tool calls are prepared sequentially, then allowed tools
///   execute concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

impl ToolExecutionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolExecutionMode::Sequential => "sequential",
            ToolExecutionMode::Parallel => "parallel",
        }
    }

    pub fn parse(s: &str) -> Option<ToolExecutionMode> {
        match s {
            "sequential" => Some(ToolExecutionMode::Sequential),
            "parallel" => Some(ToolExecutionMode::Parallel),
            _ => None,
        }
    }
}

impl Default for ToolExecutionMode {
    /// TS default: `"parallel"`.
    fn default() -> Self {
        ToolExecutionMode::Parallel
    }
}

impl serde::Serialize for ToolExecutionMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ToolExecutionMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ToolExecutionMode::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown ToolExecutionMode: {s:?}")))
    }
}

/// Thinking/reasoning level for models that support it (TS `ThinkingLevel`).
/// Kept as a `String` — the concrete set (`"off" | "minimal" | "low" |
/// "medium" | "high" | "xhigh" | "max"`) is validated against model metadata
/// from the provider layer, not here.
pub type ThinkingLevel = String;

/// Recovery policy for a tool whose durable intent exists but whose outcome
/// is unknown (TS `AgentTool.replay`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReplayPolicy {
    /// Never replay the effect (default).
    #[default]
    Never,
    /// Re-running the effect is safe (idempotent).
    Safe,
}

impl ReplayPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReplayPolicy::Never => "never",
            ReplayPolicy::Safe => "safe",
        }
    }
}

// ============================================================================
// AgentEvent
// ============================================================================

/// Events emitted by the agent for UI updates (TS `AgentEvent` union).
///
/// `to_value` produces the exact camelCase JSON of the TS union; field order
/// matches the TS declarations (relevant for `preserve_order` maps).
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// `{ type: "agent_start" }`
    AgentStart,
    /// `{ type: "agent_end", messages }`
    AgentEnd { messages: Vec<Value> },
    /// `{ type: "turn_start" }`
    TurnStart,
    /// `{ type: "turn_end", message, toolResults }`
    TurnEnd { message: Value, tool_results: Vec<Value> },
    /// `{ type: "message_start", message }`
    MessageStart { message: Value },
    /// `{ type: "message_update", message, assistantMessageEvent }`
    MessageUpdate { message: Value, assistant_message_event: Value },
    /// `{ type: "message_end", message }`
    MessageEnd { message: Value },
    /// `{ type: "tool_execution_start", toolCallId, toolName, args }`
    ToolExecutionStart { tool_call_id: String, tool_name: String, args: Value },
    /// `{ type: "tool_execution_update", toolCallId, toolName, args, partialResult }`
    ToolExecutionUpdate { tool_call_id: String, tool_name: String, args: Value, partial_result: Value },
    /// `{ type: "tool_execution_end", toolCallId, toolName, result, isError, durationMs? }`
    ///
    /// `durationMs` is omitted (not `null`) when the tool did not run,
    /// matching the TS optional field.
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        is_error: bool,
        duration_ms: Option<f64>,
    },
    /// A session-emitted event forwarded through the agent event queue
    /// (tranche 14: the virtual-model projection's `entry_appended`).
    /// `to_value` returns the wrapped event unchanged; the agent's own
    /// state reduction ignores it.
    SessionEvent(Value),
}

impl AgentEvent {
    /// The exact camelCase JSON of the TS `AgentEvent` union member.
    pub fn to_value(&self) -> Value {
        fn event(type_name: &str) -> Map<String, Value> {
            let mut map = Map::new();
            map.insert("type".to_string(), Value::String(type_name.to_string()));
            map
        }

        let map = match self {
            AgentEvent::AgentStart => event("agent_start"),
            AgentEvent::AgentEnd { messages } => {
                let mut map = event("agent_end");
                map.insert(
                    "messages".to_string(),
                    Value::Array(messages.clone()),
                );
                map
            }
            AgentEvent::TurnStart => event("turn_start"),
            AgentEvent::TurnEnd { message, tool_results } => {
                let mut map = event("turn_end");
                map.insert("message".to_string(), message.clone());
                map.insert(
                    "toolResults".to_string(),
                    Value::Array(tool_results.clone()),
                );
                map
            }
            AgentEvent::MessageStart { message } => {
                let mut map = event("message_start");
                map.insert("message".to_string(), message.clone());
                map
            }
            AgentEvent::MessageUpdate { message, assistant_message_event } => {
                let mut map = event("message_update");
                map.insert("message".to_string(), message.clone());
                map.insert(
                    "assistantMessageEvent".to_string(),
                    assistant_message_event.clone(),
                );
                map
            }
            AgentEvent::MessageEnd { message } => {
                let mut map = event("message_end");
                map.insert("message".to_string(), message.clone());
                map
            }
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                let mut map = event("tool_execution_start");
                map.insert("toolCallId".to_string(), Value::String(tool_call_id.clone()));
                map.insert("toolName".to_string(), Value::String(tool_name.clone()));
                map.insert("args".to_string(), args.clone());
                map
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, args, partial_result } => {
                let mut map = event("tool_execution_update");
                map.insert("toolCallId".to_string(), Value::String(tool_call_id.clone()));
                map.insert("toolName".to_string(), Value::String(tool_name.clone()));
                map.insert("args".to_string(), args.clone());
                map.insert("partialResult".to_string(), partial_result.clone());
                map
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error, duration_ms } => {
                let mut map = event("tool_execution_end");
                map.insert("toolCallId".to_string(), Value::String(tool_call_id.clone()));
                map.insert("toolName".to_string(), Value::String(tool_name.clone()));
                map.insert("result".to_string(), result.clone());
                map.insert("isError".to_string(), Value::Bool(*is_error));
                if let Some(duration_ms) = duration_ms {
                    map.insert(
                        "durationMs".to_string(),
                        pi_core::ai::js_number(*duration_ms),
                    );
                }
                map
            }
            AgentEvent::SessionEvent(event) => return event.clone(),
        };
        Value::Object(map)
    }
}

impl From<AgentEvent> for Value {
    fn from(event: AgentEvent) -> Value {
        event.to_value()
    }
}

impl serde::Serialize for AgentEvent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

// ============================================================================
// AgentContext
// ============================================================================

/// Context snapshot passed into the low-level agent loop (TS `AgentContext`).
/// (No `Debug`: the tool trait objects have no debug representation.)
#[derive(Clone, Default)]
pub struct AgentContext {
    /// Transcript visible to the model (`AgentMessage[]` as values).
    pub messages: Vec<Value>,
    /// Tools available for execution in this run.
    pub tools: Option<Vec<std::sync::Arc<dyn AgentTool>>>,
}

// ============================================================================
// Tool results
// ============================================================================

/// Final or partial result produced by a tool (TS `AgentToolResult`).
///
/// Serialized with TS field names; optional fields are omitted (not `null`)
/// when absent, matching the TS optionals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentToolResult {
    /// Text or image content returned to the model.
    pub content: Vec<Value>,
    /// Arbitrary structured details for logs or UI rendering.
    pub details: Value,
    /// Machine-readable result matching the tool's `outputSchema`, for
    /// programmatic callers. Not sent to the model.
    pub structured_content: Option<Value>,
    /// Usage from the final tool execution itself, if available.
    pub usage: Option<Value>,
    /// Report a failure without throwing.
    pub is_error: Option<bool>,
    /// Hint that the agent should stop after the current tool batch.
    pub terminate: Option<bool>,
}

impl serde::Serialize for AgentToolResult {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = Map::new();
        map.insert("content".to_string(), Value::Array(self.content.clone()));
        map.insert("details".to_string(), self.details.clone());
        if let Some(structured_content) = &self.structured_content {
            map.insert("structuredContent".to_string(), structured_content.clone());
        }
        if let Some(usage) = &self.usage {
            map.insert("usage".to_string(), usage.clone());
        }
        if let Some(is_error) = self.is_error {
            map.insert("isError".to_string(), Value::Bool(is_error));
        }
        if let Some(terminate) = self.terminate {
            map.insert("terminate".to_string(), Value::Bool(terminate));
        }
        Value::Object(map).serialize(serializer)
    }
}

/// Final outcome of a tool call after hooks ran (TS `AgentToolCallOutcome`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentToolCallOutcome {
    /// The raw tool call block from `assistantMessage.content`.
    pub tool_call: Value,
    /// The executed tool result after `afterToolCall` overrides.
    pub result: AgentToolResult,
    /// Whether the outcome is currently treated as an error.
    pub is_error: bool,
    /// Milliseconds `execute()` took, measured with a monotonic clock;
    /// absent when the tool did not run.
    pub duration_ms: Option<f64>,
}

impl serde::Serialize for AgentToolCallOutcome {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = Map::new();
        map.insert("toolCall".to_string(), self.tool_call.clone());
        map.insert("result".to_string(), serde_json::to_value(&self.result).expect("AgentToolResult serializes"));
        map.insert("isError".to_string(), Value::Bool(self.is_error));
        if let Some(duration_ms) = self.duration_ms {
            map.insert("durationMs".to_string(), pi_core::ai::js_number(duration_ms));
        }
        Value::Object(map).serialize(serializer)
    }
}

/// Result returned from `beforeToolCall` (TS `BeforeToolCallResult`).
/// Returning `block: true` prevents the tool from executing; the loop emits
/// an error tool result instead.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeToolCallResult {
    pub block: Option<bool>,
    pub reason: Option<String>,
    /// Hint that the agent should stop after the current tool batch when
    /// this call is blocked.
    pub terminate: Option<bool>,
}

/// Partial override returned from `afterToolCall` (TS `AfterToolCallResult`).
/// Merge semantics are field-by-field; omitted fields keep the original
/// executed tool result values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<Value>>,
    pub details: Option<Value>,
    pub structured_content: Option<Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Value>,
    pub terminate: Option<bool>,
}

// ============================================================================
// AgentTool
// ============================================================================

/// Callback used by tools to stream partial execution updates (TS
/// `AgentToolUpdateCallback`). Scoped to the current `execute` invocation;
/// calls made after the tool future settles are ignored by the caller.
pub type AgentToolUpdateCallback<'a> = dyn Fn(AgentToolResult) + Send + Sync + 'a;

/// Tool definition used by the agent runtime (TS `AgentTool`, plan §4
/// trait boundary). Messages/arguments/results are `serde_json::Value`.
pub trait AgentTool: Send + Sync {
    /// Tool name (TS `Tool.name`).
    fn name(&self) -> &str;
    /// Tool description sent to the model.
    fn description(&self) -> &str;
    /// Human-readable label for UI display.
    fn label(&self) -> &str;
    /// JSON Schema of the tool's parameters.
    fn parameters(&self) -> &Value;
    /// JSON Schema of `structuredContent` in successful results.
    fn output_schema(&self) -> Option<&Value> {
        None
    }
    /// Per-tool execution mode override; `None` applies the default mode.
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }
    /// Recovery policy for an effect whose durable intent exists but whose
    /// outcome is unknown.
    fn replay(&self) -> ReplayPolicy {
        ReplayPolicy::Never
    }
    /// Optional compatibility shim for raw tool-call arguments before schema
    /// validation. `None` models both an absent shim and the TS
    /// same-reference return (the identity shortcut); `Some` replaces the
    /// arguments (the rebuilt `{...toolCall, arguments}` object).
    fn prepare_arguments(&self, _args: &Value) -> Option<Value> {
        None
    }
    /// Execute the tool call. Failures are reported via a result with
    /// `is_error: true`; `Err` models a thrown error (the pipeline converts
    /// it to an error result with the error message).
    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        args: &'a Value,
        signal: Option<&'a AbortHandle>,
        on_update: Option<&'a AgentToolUpdateCallback<'a>>,
    ) -> BoxFuture<'a, Result<AgentToolResult, String>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queue_mode_round_trips() {
        for mode in [QueueMode::All, QueueMode::OneAtATime] {
            let value = serde_json::to_value(mode).unwrap();
            assert_eq!(value, json!(mode.as_str()));
            let back: QueueMode = serde_json::from_value(value).unwrap();
            assert_eq!(back, mode);
        }
        assert_eq!(QueueMode::default(), QueueMode::OneAtATime);
        assert!(serde_json::from_value::<QueueMode>(json!("sometimes")).is_err());
    }

    #[test]
    fn tool_execution_mode_round_trips() {
        assert_eq!(
            serde_json::to_value(ToolExecutionMode::Sequential).unwrap(),
            json!("sequential")
        );
        assert_eq!(ToolExecutionMode::default(), ToolExecutionMode::Parallel);
    }

    #[test]
    fn agent_event_json_shapes() {
        let message = json!({ "role": "user", "content": [], "timestamp": 1 });
        assert_eq!(
            AgentEvent::AgentStart.to_value(),
            json!({ "type": "agent_start" })
        );
        assert_eq!(
            AgentEvent::AgentEnd { messages: vec![message.clone()] }.to_value(),
            json!({ "type": "agent_end", "messages": [message] })
        );
        assert_eq!(
            AgentEvent::TurnStart.to_value(),
            json!({ "type": "turn_start" })
        );
    }

    #[test]
    fn agent_event_tool_execution_end_omits_absent_duration() {
        let event = AgentEvent::ToolExecutionEnd {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            result: json!({ "ok": true }),
            is_error: false,
            duration_ms: None,
        };
        assert_eq!(
            event.to_value(),
            json!({
                "type": "tool_execution_end",
                "toolCallId": "call-1",
                "toolName": "read",
                "result": { "ok": true },
                "isError": false,
            })
        );
        let event = AgentEvent::ToolExecutionEnd {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            result: Value::Null,
            is_error: true,
            duration_ms: Some(12.0),
        };
        assert_eq!(
            event.to_value(),
            json!({
                "type": "tool_execution_end",
                "toolCallId": "call-1",
                "toolName": "read",
                "result": null,
                "isError": true,
                "durationMs": 12,
            })
        );
    }

    #[test]
    fn agent_tool_result_omits_optional_fields() {
        let result = AgentToolResult::default();
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({ "content": [], "details": null })
        );
        let result = AgentToolResult {
            content: vec![json!({ "type": "text", "text": "hi" })],
            details: json!({ "n": 1 }),
            structured_content: Some(json!({ "n": 1 })),
            usage: None,
            is_error: Some(true),
            terminate: Some(true),
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "content": [{ "type": "text", "text": "hi" }],
                "details": { "n": 1 },
                "structuredContent": { "n": 1 },
                "isError": true,
                "terminate": true,
            })
        );
    }

    #[test]
    fn agent_tool_call_outcome_json() {
        let outcome = AgentToolCallOutcome {
            tool_call: json!({ "type": "toolCall", "id": "c1", "name": "read" }),
            result: AgentToolResult::default(),
            is_error: false,
            duration_ms: Some(3.5),
        };
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            json!({
                "toolCall": { "type": "toolCall", "id": "c1", "name": "read" },
                "result": { "content": [], "details": null },
                "isError": false,
                "durationMs": 3.5,
            })
        );
    }
}
