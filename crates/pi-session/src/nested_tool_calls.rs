//! Port of `src/core/nested-tool-calls.ts`.
//!
//! Tool calls that a tool makes while it runs (`ctx.executeTool()`), for
//! example from codemode scripts. `NestedCallRecorder` — the state that lands
//! on a tool result message as `nestedCalls` — is ported here.
//! `NestedToolCallRunner` drives the agent tool pipeline and is deferred
//! until the agent session layer exists.

use std::time::Instant;

use serde_json::{json, Map, Value};

use pi_core::usage_totals::combine_usage_values;

/// Limits of the nested-call record on a tool result.
pub const NESTED_CALL_LIMITS_MAX_CALLS: usize = 256;
pub const NESTED_CALL_LIMITS_MAX_ARGUMENT_BYTES_PER_CALL: usize = 8 * 1024;
pub const NESTED_CALL_LIMITS_MAX_ARGUMENT_BYTES_TOTAL: usize = 32 * 1024;
pub const NESTED_CALL_LIMITS_MAX_ERROR_CHARS: usize = 500;

/// `NESTED_CALL_LIMITS` as JSON.
pub fn nested_call_limits() -> Value {
    json!({
        "maxCalls": NESTED_CALL_LIMITS_MAX_CALLS,
        "maxArgumentBytesPerCall": NESTED_CALL_LIMITS_MAX_ARGUMENT_BYTES_PER_CALL,
        "maxArgumentBytesTotal": NESTED_CALL_LIMITS_MAX_ARGUMENT_BYTES_TOTAL,
        "maxErrorChars": NESTED_CALL_LIMITS_MAX_ERROR_CHARS,
    })
}

/// `NestedToolCallRecord`: what one nested call leaves on the tool result
/// message. Key order matches the TS object growth: `id`, `name`, `status`,
/// then `arguments`/`argumentsBytes`, then `durationMs`, then `error`.
#[derive(Debug, Clone, PartialEq)]
pub struct NestedToolCallRecord {
    pub id: String,
    pub name: String,
    /// "unfinished" | "ok" | "error".
    pub status: String,
    pub arguments: Option<Value>,
    pub arguments_bytes: Option<f64>,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
}

impl NestedToolCallRecord {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".into(), json!(self.id));
        map.insert("name".into(), json!(self.name));
        map.insert("status".into(), json!(self.status));
        if let Some(arguments) = &self.arguments {
            map.insert("arguments".into(), arguments.clone());
        }
        if let Some(bytes) = self.arguments_bytes {
            map.insert("argumentsBytes".into(), json!(bytes));
        }
        if let Some(duration) = self.duration_ms {
            map.insert("durationMs".into(), json!(duration));
        }
        if let Some(error) = &self.error {
            map.insert("error".into(), json!(error));
        }
        Value::Object(map)
    }
}

/// `NestedCallRecorder`: collects the nested calls of one model-issued tool
/// call, including calls made by nested tools.
#[derive(Debug, Default)]
pub struct NestedCallRecorder {
    calls: Vec<NestedToolCallRecord>,
    started_at: Vec<Option<Instant>>,
    complete: bool,
    argument_bytes: usize,
    /// Summed usage of every nested result, including dropped calls.
    usage: Option<Value>,
}

impl NestedCallRecorder {
    pub fn new() -> NestedCallRecorder {
        NestedCallRecorder {
            calls: Vec::new(),
            started_at: Vec::new(),
            complete: true,
            argument_bytes: 0,
            usage: None,
        }
    }

    /// Record a call as it starts. Returns the record handle (its index) or
    /// `None` when the call is dropped past `maxCalls`.
    pub fn start(&mut self, tool_call: &Value) -> Option<usize> {
        if self.calls.len() >= NESTED_CALL_LIMITS_MAX_CALLS {
            self.complete = false;
            return None;
        }
        let mut record = NestedToolCallRecord {
            id: tool_call["id"].as_str().unwrap_or_default().to_string(),
            name: tool_call["name"].as_str().unwrap_or_default().to_string(),
            status: "unfinished".to_string(),
            arguments: None,
            arguments_bytes: None,
            duration_ms: None,
            error: None,
        };
        let arguments = tool_call
            .get("arguments")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let json = serde_json::to_string(&arguments).unwrap_or_default();
        let bytes = json.len();
        if bytes > NESTED_CALL_LIMITS_MAX_ARGUMENT_BYTES_PER_CALL
            || self.argument_bytes + bytes > NESTED_CALL_LIMITS_MAX_ARGUMENT_BYTES_TOTAL
        {
            record.arguments_bytes = Some(bytes as f64);
            self.complete = false;
        } else {
            record.arguments = Some(arguments);
            self.argument_bytes += bytes;
        }
        self.calls.push(record);
        self.started_at.push(Some(Instant::now()));
        Some(self.calls.len() - 1)
    }

    /// Mark a record finished. A missing handle (dropped call) is a no-op.
    pub fn finish(&mut self, record: Option<usize>, is_error: bool, error_text: &str) {
        let Some(index) = record else { return };
        let started = self.started_at.get(index).and_then(|s| *s);
        let Some(rec) = self.calls.get_mut(index) else { return };
        rec.status = if is_error { "error" } else { "ok" }.to_string();
        // JS: performance.now() - (startedAt.get(record) ?? performance.now())
        // — a missing start time yields 0.
        rec.duration_ms = Some(match started {
            Some(instant) => (instant.elapsed().as_secs_f64() * 1000.0).round(),
            None => 0.0,
        });
        if let Some(slot) = self.started_at.get_mut(index) {
            *slot = None;
        }
        if is_error && !error_text.is_empty() {
            rec.error = Some(crate::compaction_utils::js_slice_prefix(
                error_text,
                NESTED_CALL_LIMITS_MAX_ERROR_CHARS,
            ).to_string());
        }
    }

    /// `addUsage`: sum a nested result's usage into the recorder.
    pub fn add_usage(&mut self, usage: &Value) {
        self.usage = Some(match &self.usage {
            Some(current) => combine_usage_values(current, usage),
            None => usage.clone(),
        });
    }

    /// `totalUsage`.
    pub fn total_usage(&self) -> Option<Value> {
        self.usage.clone()
    }

    /// `snapshot`: copy of the record so far, or `None` when no nested call
    /// was made.
    pub fn snapshot(&self) -> Option<Value> {
        if self.calls.is_empty() && self.complete {
            return None;
        }
        let calls: Vec<Value> = self.calls.iter().map(|call| call.to_value()).collect();
        let complete =
            self.complete && self.calls.iter().all(|call| call.status != "unfinished");
        let mut map = Map::new();
        map.insert("calls".into(), Value::Array(calls));
        map.insert("complete".into(), json!(complete));
        Some(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recorder_records_and_finishes() {
        let mut recorder = NestedCallRecorder::new();
        let handle = recorder.start(&json!({"id": "t1", "name": "read", "arguments": {"path": "/a"}}));
        assert_eq!(handle, Some(0));
        recorder.finish(handle, false, "");
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap["complete"], json!(true));
        assert_eq!(snap["calls"][0]["status"], json!("ok"));
        assert_eq!(snap["calls"][0]["arguments"], json!({"path": "/a"}));
        // durationMs is set (wall clock).
        assert!(snap["calls"][0]["durationMs"].is_number());
    }

    #[test]
    fn recorder_drops_past_max_calls() {
        let mut recorder = NestedCallRecorder::new();
        for i in 0..NESTED_CALL_LIMITS_MAX_CALLS {
            let handle = recorder.start(&json!({"id": i, "name": "read", "arguments": {}}));
            assert!(handle.is_some());
        }
        let dropped = recorder.start(&json!({"id": "extra", "name": "read", "arguments": {}}));
        assert_eq!(dropped, None);
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap["complete"], json!(false));
        assert_eq!(snap["calls"].as_array().unwrap().len(), NESTED_CALL_LIMITS_MAX_CALLS);
    }

    #[test]
    fn recorder_argument_byte_limits() {
        let mut recorder = NestedCallRecorder::new();
        // Over the per-call limit: arguments dropped, byte count recorded.
        let big = json!({"path": "x".repeat(9000)});
        let handle = recorder.start(&json!({"id": "t1", "name": "write", "arguments": big}));
        assert_eq!(handle, Some(0));
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap["complete"], json!(false));
        assert!(snap["calls"][0]["arguments"].is_null());
        assert!(snap["calls"][0]["argumentsBytes"].as_f64().unwrap() > 8192.0);
    }

    #[test]
    fn recorder_null_arguments_become_empty_object() {
        let mut recorder = NestedCallRecorder::new();
        recorder.start(&json!({"id": "t1", "name": "read"}));
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap["calls"][0]["arguments"], json!({}));
    }

    #[test]
    fn recorder_error_truncated_and_usage_summed() {
        let mut recorder = NestedCallRecorder::new();
        let a = recorder.start(&json!({"id": "t1", "name": "read", "arguments": {}})).unwrap();
        recorder.finish(Some(a), true, &"boom ".repeat(120));
        recorder.add_usage(&json!({"input": 10, "output": 5}));
        recorder.add_usage(&json!({"input": 1, "output": 2}));
        let total = recorder.total_usage().unwrap();
        assert_eq!(total["input"], json!(11));
        assert_eq!(total["output"], json!(7));
        let snap = recorder.snapshot().unwrap();
        let error = snap["calls"][0]["error"].as_str().unwrap();
        assert_eq!(js_len(error), 500);
        assert_eq!(snap["calls"][0]["status"], json!("error"));
    }

    fn js_len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    #[test]
    fn recorder_empty_snapshot_is_none() {
        let recorder = NestedCallRecorder::new();
        assert!(recorder.snapshot().is_none());
        assert!(recorder.total_usage().is_none());
    }

    #[test]
    fn recorder_finish_missing_handle_is_noop() {
        let mut recorder = NestedCallRecorder::new();
        recorder.finish(None, true, "x");
        assert!(recorder.snapshot().is_none());
    }
}
