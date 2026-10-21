//! `AgentSessionEvent` emission and the identity-keyed agent-event input
//! (agent-session.ts `AgentSessionEvent` union + `_emit`/`_emitQueueUpdate`).
//!
//! Messages are `serde_json::Value`, so the TS union is emitted as its
//! exact camelCase JSON (built with `preserve_order` maps, key order
//! matching the TS declarations).
//!
//! **Identity design (plan §2 trap 1):** TS keys `_entryIdsByMessage`
//! (WeakMap) and `_boundaryDispatchedMessages` (WeakSet) by message
//! *object identity*, and `_findPersistedMessageEntryId` compares
//! `entry.message === message` by reference. Rust messages are values, so
//! this port keys messages by arena index ([`MsgId`]): every message the
//! session observes is registered in an insertion-ordered arena
//! (`Vec<Value>`), and identity comparisons become index comparisons. The
//! parity harness feeds both sides the same logical events by declaring a
//! message pool and referencing messages by index — object wiring the TS
//! harness performs with the very same pool objects. Structurally the
//! algorithms (WeakMap cache, branch scan, projection scan) are unchanged.

use serde_json::{json, Map, Value};

/// Identity handle of a message in the session's arena (TS: the object
/// reference). Indices are assigned in insertion order.
pub type MsgId = usize;

/// The agent events [`crate::agent_session::AgentSession`] consumes —
/// `pi_agent::types::AgentEvent` with message payloads replaced by arena
/// ids so the identity-keyed bookkeeping survives the value port.
///
/// TS `_handleAgentEvent(event: AgentEvent)`.
#[derive(Debug, Clone)]
pub enum SessionAgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<MsgId> },
    TurnStart,
    TurnEnd { message: MsgId, tool_results: Vec<MsgId> },
    MessageStart { message: MsgId },
    MessageUpdate { message: MsgId, assistant_message_event: Value },
    MessageEnd { message: MsgId },
    ToolExecutionStart { tool_call_id: String, tool_name: String, args: Value },
    ToolExecutionUpdate { tool_call_id: String, tool_name: String, args: Value, partial_result: Value },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        is_error: bool,
        duration_ms: Option<f64>,
    },
}

impl SessionAgentEvent {
    /// The exact camelCase JSON of the TS `AgentEvent` union member, with
    /// message payloads read from the arena.
    pub fn to_value(&self, arena: &[Value]) -> Value {
        match self {
            SessionAgentEvent::AgentStart => agent_event_json("agent_start", &[]),
            SessionAgentEvent::AgentEnd { messages } => agent_event_json(
                "agent_end",
                &[("messages", Value::Array(messages.iter().map(|m| arena[*m].clone()).collect()))],
            ),
            SessionAgentEvent::TurnStart => agent_event_json("turn_start", &[]),
            SessionAgentEvent::TurnEnd { message, tool_results } => agent_event_json(
                "turn_end",
                &[
                    ("message", arena[*message].clone()),
                    ("toolResults", Value::Array(tool_results.iter().map(|m| arena[*m].clone()).collect())),
                ],
            ),
            SessionAgentEvent::MessageStart { message } => {
                agent_event_json("message_start", &[("message", arena[*message].clone())])
            }
            SessionAgentEvent::MessageUpdate { message, assistant_message_event } => agent_event_json(
                "message_update",
                &[("message", arena[*message].clone()), ("assistantMessageEvent", assistant_message_event.clone())],
            ),
            SessionAgentEvent::MessageEnd { message } => {
                agent_event_json("message_end", &[("message", arena[*message].clone())])
            }
            SessionAgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => agent_event_json(
                "tool_execution_start",
                &[("toolCallId", json!(tool_call_id)), ("toolName", json!(tool_name)), ("args", args.clone())],
            ),
            SessionAgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, args, partial_result } => {
                agent_event_json(
                    "tool_execution_update",
                    &[
                        ("toolCallId", json!(tool_call_id)),
                        ("toolName", json!(tool_name)),
                        ("args", args.clone()),
                        ("partialResult", partial_result.clone()),
                    ],
                )
            }
            SessionAgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error, duration_ms } => {
                let mut event = agent_event_json(
                    "tool_execution_end",
                    &[
                        ("toolCallId", json!(tool_call_id)),
                        ("toolName", json!(tool_name)),
                        ("result", result.clone()),
                        ("isError", json!(is_error)),
                    ],
                );
                if let Some(duration_ms) = duration_ms {
                    event
                        .as_object_mut()
                        .expect("event is an object")
                        .insert("durationMs".to_string(), pi_core::ai::js_number(*duration_ms));
                }
                event
            }
        }
    }

    /// The `type` discriminant of the TS `AgentEvent` union member.
    pub fn kind(&self) -> &'static str {
        match self {
            SessionAgentEvent::AgentStart => "agent_start",
            SessionAgentEvent::AgentEnd { .. } => "agent_end",
            SessionAgentEvent::TurnStart => "turn_start",
            SessionAgentEvent::TurnEnd { .. } => "turn_end",
            SessionAgentEvent::MessageStart { .. } => "message_start",
            SessionAgentEvent::MessageUpdate { .. } => "message_update",
            SessionAgentEvent::MessageEnd { .. } => "message_end",
            SessionAgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            SessionAgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            SessionAgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
        }
    }
}

/// The exact camelCase JSON of a TS `AgentSessionEvent` union member —
/// the base agent event shape. `agent_end` gains `willRetry` at emission
/// time; nested-tool-call tool events gain `parentToolCallId` (tranche 7+).
pub fn agent_event_json(kind: &str, fields: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    map.insert("type".to_string(), Value::String(kind.to_string()));
    for (key, value) in fields {
        map.insert(key.to_string(), value.clone());
    }
    Value::Object(map)
}

/// `{ type: "agent_settled", aborted }` (TS `agent_settled`).
pub fn agent_settled_event(aborted: bool) -> Value {
    json!({ "type": "agent_settled", "aborted": aborted })
}

/// `{ type: "queue_update", steering, followUp }` (TS `_emitQueueUpdate`).
pub fn queue_update_event(steering: &[String], follow_up: &[String]) -> Value {
    json!({
        "type": "queue_update",
        "steering": steering,
        "followUp": follow_up,
    })
}

/// `{ type: "auto_retry_end", success, attempt, finalError? }`.
pub fn auto_retry_end_event(success: bool, attempt: f64, final_error: Option<&str>) -> Value {
    let mut map = Map::new();
    map.insert("type".to_string(), json!("auto_retry_end"));
    map.insert("success".to_string(), Value::Bool(success));
    map.insert("attempt".to_string(), pi_core::ai::js_number(attempt));
    if let Some(final_error) = final_error {
        map.insert("finalError".to_string(), json!(final_error));
    }
    Value::Object(map)
}
