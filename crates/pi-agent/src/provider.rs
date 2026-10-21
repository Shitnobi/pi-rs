//! The provider seam of the agent loop: the `StreamFn` trait (plan §4)
//! plus `ScriptedProvider`, the deterministic fake used by parity tests.
//!
//! Tranche 0 keeps the event stream and the final assistant message
//! `serde_json::Value`-based; a later tranche refines the stream types.

use std::collections::VecDeque;

use serde_json::Value;

use crate::types::BoxFuture;

/// One stream result: the protocol events plus the final
/// `AssistantMessage`. `AssistantMessageEventStream` equivalent, flattened.
#[derive(Debug, Clone)]
pub struct StreamOutput {
    /// Streamed `AssistantStreamEvent`s in emission order.
    pub events: VecDeque<Value>,
    /// The final `AssistantMessage` (stopReason `error`/`aborted` failures
    /// are encoded here, never thrown — see the TS `StreamFn` contract).
    pub result: Value,
}

/// Stream function used by the agent loop (TS `StreamFn`, plan §4 trait
/// boundary). `Models.streamSimple` satisfies this shape.
///
/// Contract (TS):
/// - Must not fail for request/model/runtime errors; failures are encoded
///   in the returned stream via protocol events and a final result with
///   stopReason `"error"` or `"aborted"` and `errorMessage`.
pub trait StreamFn: Send + Sync {
    fn stream<'a>(
        &'a self,
        model: &'a Value,
        context: &'a Value,
        options: &'a Value,
    ) -> BoxFuture<'a, StreamOutput>;
}

/// One scripted turn of the deterministic fake provider.
#[derive(Debug, Clone)]
pub struct ScriptedTurn {
    /// Streamed events for this turn, in order.
    pub events: Vec<Value>,
    /// Final assistant message for this turn.
    pub result: Value,
}

/// Deterministic fake `StreamFn` for parity tests: replays the scripted
/// turns in order, one per `stream` call. Once the script is exhausted,
/// every further call returns an empty stream with a `null` result (a
/// visible, deterministic sentinel — real providers never produce it).
#[derive(Debug)]
pub struct ScriptedProvider {
    turns: Vec<ScriptedTurn>,
    next: std::sync::Mutex<usize>,
}

impl ScriptedProvider {
    pub fn new(turns: Vec<ScriptedTurn>) -> Self {
        ScriptedProvider { turns, next: std::sync::Mutex::new(0) }
    }

    /// Number of `stream` calls served so far.
    pub fn call_count(&self) -> usize {
        *self.next.lock().expect("ScriptedProvider cursor poisoned")
    }
}

impl StreamFn for ScriptedProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Value,
        _context: &'a Value,
        _options: &'a Value,
    ) -> BoxFuture<'a, StreamOutput> {
        // The cursor is popped synchronously when the stream is requested
        // (the TS fake resolves the stream immediately); the returned future
        // is instantly ready.
        let output = {
            let mut next = self.next.lock().expect("ScriptedProvider cursor poisoned");
            let output = if *next < self.turns.len() {
                let turn = &self.turns[*next];
                StreamOutput {
                    events: turn.events.iter().cloned().collect(),
                    result: turn.result.clone(),
                }
            } else {
                StreamOutput { events: VecDeque::new(), result: Value::Null }
            };
            *next += 1;
            output
        };
        Box::pin(async move { output })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replays_turns_in_order() {
        let provider = ScriptedProvider::new(vec![
            ScriptedTurn { events: vec![json!({ "type": "message_start" })], result: json!({ "n": 1 }) },
            ScriptedTurn { events: vec![], result: json!({ "n": 2 }) },
        ]);
        let model = json!({ "id": "m" });
        let context = json!({ "messages": [] });
        let options = json!({});
        let output = crate::run_current_thread(provider.stream(&model, &context, &options));
        let expected: VecDeque<Value> = vec![json!({ "type": "message_start" })].into();
        assert_eq!(output.events, expected);
        assert_eq!(output.result, json!({ "n": 1 }));

        let output = crate::run_current_thread(provider.stream(&Value::Null, &Value::Null, &Value::Null));
        assert!(output.events.is_empty());
        assert_eq!(output.result, json!({ "n": 2 }));
        assert_eq!(provider.call_count(), 2);
    }

    #[test]
    fn exhausted_script_returns_empty_null_stream() {
        let provider = ScriptedProvider::new(vec![]);
        let output = crate::run_current_thread(provider.stream(&Value::Null, &Value::Null, &Value::Null));
        assert!(output.events.is_empty());
        assert_eq!(output.result, Value::Null);
        let output = crate::run_current_thread(provider.stream(&Value::Null, &Value::Null, &Value::Null));
        assert!(output.events.is_empty());
        assert_eq!(output.result, Value::Null);
        assert_eq!(provider.call_count(), 2);
    }
}
