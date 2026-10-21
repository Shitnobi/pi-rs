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

use serde_json::{json, Value};

use pi_agent::agent_surface::AgentQueues;
use pi_agent::types::QueueMode;

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
