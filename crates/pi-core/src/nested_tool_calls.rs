//! Port of `src/core/nested-tool-calls.ts`.
//!
//! Tool calls that a tool makes while it runs (`ctx.executeTool()`), for
//! example from codemode scripts. The session runs each one through the
//! agent's tool pipeline with its own hooks, emits `tool_execution_*`
//! events with `parentToolCallId`, and records the calls and their usage on
//! the model-issued call's tool result message. Nothing here runs until a
//! tool calls `ctx.executeTool()`.
//!
//! The TS module is async and promise-queued. This port is synchronous:
//! - [`NestedToolCallRunner::execute`] takes `&self` (re-entrant like the
//!   TS class, via interior mutability) so a nested tool can call back into
//!   the same runner;
//! - the promise queue that serializes concurrent calls to sequential tools
//!   is not ported — synchronous execution is inherently serialized — but
//!   the `exclusive` / `holdsQueue` bookkeeping is kept;
//! - `AbortSignal` (the `NestedToolCallOptions.signal` field) is not
//!   ported.
//!
//! `pi-session` carries an earlier partial port of the recorder (Value-based,
//! runner deferred); this is the full module for pi-core.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::Usage;
use crate::messages::TextOrImage;
use crate::usage_totals::combine_usage;

// ============================================================================
// Limits
// ============================================================================

/// Limits of the nested-call record on a tool result: arguments over the
/// per-call or total size are omitted, calls beyond the count are dropped,
/// and the record is marked incomplete when any of that happens.
/// (`NESTED_CALL_LIMITS` in the TS.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NestedCallLimits {
	pub max_calls: usize,
	pub max_argument_bytes_per_call: usize,
	pub max_argument_bytes_total: usize,
	pub max_error_chars: usize,
}

/// `NESTED_CALL_LIMITS`.
pub const NESTED_CALL_LIMITS: NestedCallLimits = NestedCallLimits {
	max_calls: 256,
	max_argument_bytes_per_call: 8 * 1024,
	max_argument_bytes_total: 32 * 1024,
	max_error_chars: 500,
};

// ============================================================================
// pi-ai types
// ============================================================================

/// The `status` of a `NestedToolCallRecord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NestedCallStatus {
	/// The call was still running when the calling tool finished.
	Unfinished,
	Ok,
	Error,
}

/// `NestedToolCallRecord` from pi-ai: a tool call that another tool made
/// while it ran. Serialization order matches the TS object growth: `id`,
/// `name`, `status`, then `arguments`/`argumentsBytes`, `durationMs`,
/// `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCallRecord {
	pub id: String,
	pub name: String,
	pub status: NestedCallStatus,
	/// Omitted when over the size limits; `argumentsBytes` then gives their
	/// size.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub arguments: Option<Value>,
	/// UTF-8 size of the arguments as JSON, set when `arguments` is omitted.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub arguments_bytes: Option<u64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub duration_ms: Option<i64>,
	/// Error text, truncated.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
}

/// `NestedToolCalls` from pi-ai: bounded record of the nested calls a tool
/// made. Results are not recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCalls {
	pub calls: Vec<NestedToolCallRecord>,
	/// False when calls were dropped, arguments omitted, or calls had not
	/// finished.
	pub complete: bool,
}

// ============================================================================
// pi-agent-core types
// ============================================================================

/// `AgentToolCall` from pi-agent-core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
	/// Always `"toolCall"`.
	#[serde(rename = "type")]
	pub kind: String,
	pub id: String,
	pub name: String,
	pub arguments: Value,
}

impl AgentToolCall {
	pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
		Self {
			kind: "toolCall".to_string(),
			id: id.into(),
			name: name.into(),
			arguments,
		}
	}
}

/// `ToolExecutionMode` from pi-agent-core: per-tool execution mode override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolExecutionMode {
	/// This tool must execute one at a time with other tool calls.
	Sequential,
	/// This tool can execute concurrently with other tool calls.
	Parallel,
}

/// `AgentToolResult` from pi-agent-core: final or partial result produced by
/// a tool. The `usage` / `isError` / `terminate` fields are optional, like
/// the TS; `details` is a raw JSON value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentToolResult {
	/// Text or image content returned to the model.
	pub content: Vec<TextOrImage>,
	/// Arbitrary structured details for logs or UI rendering.
	pub details: Value,
	/// Machine-readable result matching the tool's `outputSchema`, for
	/// programmatic callers.
	pub structured_content: Option<Value>,
	/// Usage from the final tool execution itself, if available.
	pub usage: Option<Usage>,
	/// Report a failure without throwing.
	pub is_error: Option<bool>,
	/// Hint that the agent should stop after the current tool batch.
	pub terminate: Option<bool>,
}

impl AgentToolResult {
	/// A text result, as the TS tests build them.
	pub fn text(text: impl Into<String>) -> Self {
		Self {
			content: vec![TextOrImage::Text {
				text: text.into(),
				text_signature: None,
			}],
			..Self::default()
		}
	}
}

/// `AgentToolCallOutcome` from pi-agent-core: final outcome of a tool call
/// after hooks ran.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentToolCallOutcome {
	pub tool_call: AgentToolCall,
	pub result: AgentToolResult,
	pub is_error: bool,
	/// Milliseconds `execute()` took; absent when the tool did not run.
	pub duration_ms: Option<i64>,
}

/// `AgentToolUpdateCallback`: receives partial results of the nested tool,
/// in addition to `tool_execution_update` events.
pub type AgentToolUpdateCallback<'a> = &'a mut dyn FnMut(&AgentToolResult);

/// `AgentTool` from pi-agent-core, as far as nested calls use it. `execute`
/// is synchronous here; the TS `AbortSignal` parameter is not ported, and
/// the schema hooks (`prepareArguments`, `outputSchema`, `replay`) are
/// omitted.
pub trait AgentTool {
	fn name(&self) -> &str;
	/// Human-readable label for UI display.
	fn label(&self) -> &str;
	fn description(&self) -> &str;
	/// JSON Schema of the parameters.
	fn parameters(&self) -> &Value;
	/// Per-tool execution mode override; `None` is the default mode.
	fn execution_mode(&self) -> Option<ToolExecutionMode> {
		None
	}
	/// Execute the tool call. Report failures by returning a result with
	/// `is_error: true`.
	fn execute(
		&self,
		tool_call_id: &str,
		params: &Value,
		on_update: &mut dyn FnMut(&AgentToolResult),
	) -> AgentToolResult;
}

// ============================================================================
// NestedCallSummary
// ============================================================================

/// What the nested calls of one model-issued tool call leave on its tool
/// result message. (`NestedCallSummary` in the TS.)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NestedCallSummary {
	/// Becomes `nestedCalls`. `None` when no nested call was made.
	pub calls: Option<NestedToolCalls>,
	/// Summed `usage` of the nested results, added to the message's `usage`.
	pub usage: Option<Usage>,
}

// ============================================================================
// NestedCallRecorder
// ============================================================================

/// Collects the nested calls of one model-issued tool call, including calls
/// made by nested tools. The snapshot becomes `nestedCalls` on the tool
/// result message.
#[derive(Debug)]
pub struct NestedCallRecorder {
	calls: Vec<NestedToolCallRecord>,
	/// `startedAt`, keyed by record index (the TS keys by object identity).
	started_at: Vec<Option<Instant>>,
	complete: bool,
	argument_bytes: usize,
	/// Summed usage of every nested result, including calls dropped from the
	/// record.
	usage: Option<Usage>,
}

impl NestedCallRecorder {
	pub fn new() -> Self {
		Self {
			calls: Vec::new(),
			started_at: Vec::new(),
			// The TS starts `complete = true`.
			complete: true,
			argument_bytes: 0,
			usage: None,
		}
	}

	/// Record a call as it starts. Returns the record handle (its index), or
	/// `None` when the call is dropped past `maxCalls`.
	pub fn start(&mut self, tool_call: &AgentToolCall) -> Option<usize> {
		if self.calls.len() >= NESTED_CALL_LIMITS.max_calls {
			self.complete = false;
			return None;
		}
		let mut record = NestedToolCallRecord {
			id: tool_call.id.clone(),
			name: tool_call.name.clone(),
			status: NestedCallStatus::Unfinished,
			arguments: None,
			arguments_bytes: None,
			duration_ms: None,
			error: None,
		};
		// JSON.stringify(toolCall.arguments ?? {}) and its UTF-8 byte length.
		let arguments = if tool_call.arguments.is_null() {
			Value::Object(serde_json::Map::new())
		} else {
			tool_call.arguments.clone()
		};
		let json = js_json_stringify(&arguments);
		let bytes = json.len();
		if bytes > NESTED_CALL_LIMITS.max_argument_bytes_per_call
			|| self.argument_bytes + bytes > NESTED_CALL_LIMITS.max_argument_bytes_total
		{
			record.arguments_bytes = Some(bytes as u64);
			self.complete = false;
		} else {
			// JSON.parse(json): re-parse so number formatting normalizes
			// like the TS stringify/parse round trip.
			record.arguments = serde_json::from_str(&json).ok();
			self.argument_bytes += bytes;
		}
		self.calls.push(record);
		self.started_at.push(Some(Instant::now()));
		Some(self.calls.len() - 1)
	}

	/// Mark a record finished. A missing handle (dropped call) is a no-op.
	pub fn finish(&mut self, record: Option<usize>, is_error: bool, error_text: &str) {
		let Some(index) = record else { return };
		let started = self.started_at.get(index).copied().flatten();
		let elapsed_ms = match started {
			// performance.now() - (startedAt.get(record) ?? performance.now())
			Some(instant) => instant.elapsed().as_secs_f64() * 1000.0,
			None => 0.0,
		};
		if let Some(rec) = self.calls.get_mut(index) {
			rec.status = if is_error {
				NestedCallStatus::Error
			} else {
				NestedCallStatus::Ok
			};
			rec.duration_ms = Some(elapsed_ms.round() as i64);
			if is_error && !error_text.is_empty() {
				rec.error = Some(js_slice_prefix(error_text, NESTED_CALL_LIMITS.max_error_chars));
			}
		}
		if let Some(slot) = self.started_at.get_mut(index) {
			*slot = None;
		}
	}

	/// `addUsage`: sum a nested result's usage into the recorder.
	pub fn add_usage(&mut self, usage: Usage) {
		self.usage = Some(match &self.usage {
			Some(current) => combine_usage(current, &usage),
			None => usage,
		});
	}

	/// `totalUsage`.
	pub fn total_usage(&self) -> Option<&Usage> {
		self.usage.as_ref()
	}

	/// `snapshot`: copy of the record so far, or `None` when no nested call
	/// was made.
	pub fn snapshot(&self) -> Option<NestedToolCalls> {
		if self.calls.is_empty() && self.complete {
			return None;
		}
		Some(NestedToolCalls {
			calls: self.calls.clone(),
			complete: self.complete && self.calls.iter().all(|call| call.status != NestedCallStatus::Unfinished),
		})
	}
}

impl Default for NestedCallRecorder {
	fn default() -> Self {
		Self::new()
	}
}

// ============================================================================
// Events and host
// ============================================================================

/// `NestedToolExecutionEvent`: `tool_execution_*` events of nested calls.
#[derive(Debug, Clone, PartialEq)]
pub enum NestedToolExecutionEvent {
	ToolExecutionStart {
		tool_call_id: String,
		tool_name: String,
		args: Value,
		parent_tool_call_id: String,
	},
	ToolExecutionUpdate {
		tool_call_id: String,
		tool_name: String,
		args: Value,
		partial_result: AgentToolResult,
		parent_tool_call_id: String,
	},
	ToolExecutionEnd {
		tool_call_id: String,
		tool_name: String,
		result: AgentToolResult,
		is_error: bool,
		duration_ms: Option<i64>,
		parent_tool_call_id: String,
	},
}

impl NestedToolExecutionEvent {
	/// The event `type` string.
	pub fn kind(&self) -> &'static str {
		match self {
			Self::ToolExecutionStart { .. } => "tool_execution_start",
			Self::ToolExecutionUpdate { .. } => "tool_execution_update",
			Self::ToolExecutionEnd { .. } => "tool_execution_end",
		}
	}

	pub fn tool_call_id(&self) -> &str {
		match self {
			Self::ToolExecutionStart { tool_call_id, .. }
			| Self::ToolExecutionUpdate { tool_call_id, .. }
			| Self::ToolExecutionEnd { tool_call_id, .. } => tool_call_id,
		}
	}

	pub fn parent_tool_call_id(&self) -> &str {
		match self {
			Self::ToolExecutionStart {
				parent_tool_call_id,
				..
			}
			| Self::ToolExecutionUpdate {
				parent_tool_call_id,
				..
			}
			| Self::ToolExecutionEnd {
				parent_tool_call_id,
				..
			} => parent_tool_call_id,
		}
	}
}

/// `NestedToolCallOptions`. The TS `signal` (`AbortSignal`, defaulting to
/// the calling tool's signal) is not ported.
#[derive(Default)]
pub struct NestedToolCallOptions<'a> {
	pub on_update: Option<AgentToolUpdateCallback<'a>>,
}

/// `NestedToolCallHost`: what the runner needs from the session. All
/// methods take `&self` — like the TS host object shared across re-entrant
/// nested calls — so implementors use interior mutability.
pub trait NestedToolCallHost {
	/// Tools nested calls resolve against.
	fn get_tools(&self) -> Vec<Rc<dyn AgentTool>>;
	/// Whether every nested call runs exclusively, as when the agent
	/// executes tool calls sequentially.
	fn is_sequential(&self) -> bool;
	/// Run the call through the tool pipeline, with hooks that report
	/// `parentToolCallId`.
	fn run_tool_call(
		&self,
		tool_call: &AgentToolCall,
		parent_tool_call_id: &str,
		on_update: &mut dyn FnMut(&AgentToolResult),
	) -> AgentToolCallOutcome;
	/// Emit a `tool_execution_*` event.
	fn emit(&self, event: NestedToolExecutionEvent);
}

// ============================================================================
// NestedToolCallRunner
// ============================================================================

/// Calls below one model-issued call share its recorder. (`CallScope` in
/// the TS.)
struct CallScope {
	recorder: Rc<RefCell<NestedCallRecorder>>,
	next_id: u64,
	/// Set inside a call that holds the exclusive queue, so its own nested
	/// calls do not wait on it.
	holds_queue: bool,
}

impl CallScope {
	fn new() -> Self {
		Self {
			recorder: Rc::new(RefCell::new(NestedCallRecorder::new())),
			next_id: 1,
			holds_queue: false,
		}
	}
}

/// `textOf`: the text blocks of a result, joined with newlines.
fn text_of(result: &AgentToolResult) -> String {
	result
		.content
		.iter()
		.filter_map(|block| match block {
			TextOrImage::Text { text, .. } => Some(text.as_str()),
			TextOrImage::Image { .. } => None,
		})
		.collect::<Vec<_>>()
		.join("\n")
}

/// Runs nested tool calls on behalf of a host session. `execute` never
/// fails for tool errors: they come back as `is_error: true`.
pub struct NestedToolCallRunner<H: NestedToolCallHost> {
	host: H,
	/// Scopes by the id of the calling tool call.
	scopes: RefCell<HashMap<String, CallScope>>,
}

impl<H: NestedToolCallHost> NestedToolCallRunner<H> {
	pub fn new(host: H) -> Self {
		Self {
			host,
			scopes: RefCell::new(HashMap::new()),
		}
	}

	/// The host this runner drives.
	pub fn host(&self) -> &H {
		&self.host
	}

	/// Run `name` on behalf of the call `callerId`. The nested call gets the
	/// id `<callerId>/<n>`.
	pub fn execute(
		&self,
		caller_id: &str,
		name: &str,
		args: &Value,
		options: NestedToolCallOptions<'_>,
	) -> AgentToolCallOutcome {
		let (recorder, parent_holds_queue, next_id) = {
			let mut scopes = self.scopes.borrow_mut();
			let scope = scopes
				.entry(caller_id.to_string())
				.or_insert_with(CallScope::new);
			let next_id = scope.next_id;
			scope.next_id += 1;
			(scope.recorder.clone(), scope.holds_queue, next_id)
		};
		let tool_call = AgentToolCall {
			kind: "toolCall".to_string(),
			id: format!("{}/{}", caller_id, next_id),
			name: name.to_string(),
			// (args ?? {}) as AgentToolCall["arguments"]
			arguments: if args.is_null() {
				Value::Object(serde_json::Map::new())
			} else {
				args.clone()
			},
		};
		let record = recorder.borrow_mut().start(&tool_call);
		self.host.emit(NestedToolExecutionEvent::ToolExecutionStart {
			tool_call_id: tool_call.id.clone(),
			tool_name: name.to_string(),
			args: tool_call.arguments.clone(),
			parent_tool_call_id: caller_id.to_string(),
		});

		// this.host.getTools().find((tool) => tool.name === name)?.executionMode === "sequential"
		let sequential_tool = self
			.host
			.get_tools()
			.iter()
			.find(|tool| tool.name() == name)
			.and_then(|tool| tool.execution_mode())
			== Some(ToolExecutionMode::Sequential);
		let exclusive = !parent_holds_queue && (self.host.is_sequential() || sequential_tool);
		{
			let mut scopes = self.scopes.borrow_mut();
			scopes.insert(
				tool_call.id.clone(),
				CallScope {
					recorder: recorder.clone(),
					next_id: 1,
					holds_queue: parent_holds_queue || exclusive,
				},
			);
		}
		let mut user_on_update = options.on_update;
		let outcome = {
			let mut on_update = |partial: &AgentToolResult| {
				if let Some(callback) = user_on_update.as_mut() {
					callback(partial);
				}
				self.host.emit(NestedToolExecutionEvent::ToolExecutionUpdate {
					tool_call_id: tool_call.id.clone(),
					tool_name: name.to_string(),
					args: tool_call.arguments.clone(),
					partial_result: partial.clone(),
					parent_tool_call_id: caller_id.to_string(),
				});
			};
			self.host.run_tool_call(&tool_call, caller_id, &mut on_update)
		};
		// The TS finally block: drop the nested scope and release the queue.
		self.scopes.borrow_mut().remove(&tool_call.id);

		let error_text = text_of(&outcome.result);
		recorder.borrow_mut().finish(record, outcome.is_error, &error_text);
		// Nested results are not persisted, so their usage is only counted
		// through the recorder.
		if let Some(usage) = &outcome.result.usage {
			recorder.borrow_mut().add_usage(usage.clone());
		}
		self.host.emit(NestedToolExecutionEvent::ToolExecutionEnd {
			tool_call_id: tool_call.id.clone(),
			tool_name: name.to_string(),
			result: outcome.result.clone(),
			is_error: outcome.is_error,
			// ...(outcome.durationMs === undefined ? {} : { durationMs })
			duration_ms: outcome.duration_ms,
			parent_tool_call_id: caller_id.to_string(),
		});
		outcome
	}

	/// Remove and return the record of the nested calls a model-issued call
	/// made.
	pub fn take_record(&self, tool_call_id: &str) -> Option<NestedCallSummary> {
		let scope = self.scopes.borrow_mut().remove(tool_call_id)?;
		let recorder = scope.recorder.borrow_mut();
		Some(NestedCallSummary {
			calls: recorder.snapshot(),
			usage: recorder.total_usage().cloned(),
		})
	}

	/// `clear`: drop every scope.
	pub fn clear(&self) {
		self.scopes.borrow_mut().clear();
	}
}

// ============================================================================
// JS helpers (private)
// ============================================================================

/// JS `JSON.stringify` for a parsed JSON value: compact, insertion key
/// order, V8 number formatting (integral floats print without a decimal
/// point; exponents outside `[1e-6, 1e21)`).
pub fn js_json_stringify(value: &Value) -> String {
	match value {
		Value::Null => "null".to_string(),
		Value::Bool(b) => b.to_string(),
		Value::Number(number) => js_number_to_string(number),
		Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()),
		Value::Array(items) => {
			let parts: Vec<String> = items.iter().map(js_json_stringify).collect();
			format!("[{}]", parts.join(","))
		}
		Value::Object(map) => {
			let parts: Vec<String> = map
				.iter()
				.map(|(key, value)| {
					format!(
						"{}:{}",
						serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()),
						js_json_stringify(value)
					)
				})
				.collect();
			format!("{{{}}}", parts.join(","))
		}
	}
}

fn js_number_to_string(number: &serde_json::Number) -> String {
	if let Some(i) = number.as_i64() {
		return i.to_string();
	}
	if let Some(u) = number.as_u64() {
		return u.to_string();
	}
	format_js_number(number.as_f64().unwrap_or(0.0))
}

/// V8 `Number.prototype.toString()` for the finite values JSON can hold.
/// `Infinity` / `NaN` stringify as `null` in JSON.
fn format_js_number(f: f64) -> String {
	if !f.is_finite() {
		return "null".to_string();
	}
	if f == 0.0 {
		// Includes -0.
		return "0".to_string();
	}
	let abs = f.abs();
	if abs >= 1e21 || abs < 1e-6 {
		// V8 switches to exponential; Rust's `{:e}` omits the `+`.
		let s = format!("{:e}", f);
		if let Some(e) = s.find('e') {
			let (mantissa, exponent) = s.split_at(e);
			let exponent = &exponent[1..];
			if let Some(digits) = exponent.strip_prefix('-') {
				format!("{mantissa}e-{digits}")
			} else {
				format!("{mantissa}e+{exponent}")
			}
		} else {
			s
		}
	} else if f.fract() == 0.0 {
		// Integral below 1e21: V8 prints every digit.
		(f as i128).to_string()
	} else {
		// Both Rust and V8 print the shortest round-trip decimal here.
		format!("{f}")
	}
}

/// JS `String.prototype.slice(0, max)` for `max` UTF-16 code units. A
/// surrogate pair straddling the boundary is dropped: JS would keep the
/// lone lead surrogate, which is not representable in a Rust string.
fn js_slice_prefix(s: &str, max_units: usize) -> String {
	let mut out = String::new();
	let mut units = 0;
	for c in s.chars() {
		let len = c.len_utf16();
		if units + len > max_units {
			break;
		}
		units += len;
		out.push(c);
	}
	out
}

// ============================================================================
// Tests (mirroring test/nested-tool-calls.test.ts)
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	use crate::ai::UsageCost;

	fn usage(input: f64, cost: f64) -> Usage {
		Usage {
			input,
			output: 0.0,
			cache_read: 0.0,
			cache_write: 0.0,
			cache_write_1h: None,
			reasoning: None,
			total_tokens: input,
			cost: UsageCost {
				input: cost,
				output: 0.0,
				cache_read: 0.0,
				cache_write: 0.0,
				total: cost,
			},
		}
	}

	fn call(id: &str, args: Value) -> AgentToolCall {
		AgentToolCall::new(id, "t", args)
	}

	// ------------------------------------------------------------------
	// Test tools and host, mirroring createRunner in the TS test.
	// ------------------------------------------------------------------

	type ExecuteFn = Box<dyn Fn(&str, &Value, &mut dyn FnMut(&AgentToolResult)) -> AgentToolResult>;

	struct TestTool {
		name: String,
		parameters: Value,
		execution_mode: Option<ToolExecutionMode>,
		execute: ExecuteFn,
	}

	impl TestTool {
		fn new(name: &str, execute: ExecuteFn) -> Rc<Self> {
			Rc::new(Self {
				name: name.to_string(),
				parameters: json!({ "type": "object", "properties": {} }),
				execution_mode: None,
				execute,
			})
		}

		fn with_mode(name: &str, execution_mode: ToolExecutionMode, execute: ExecuteFn) -> Rc<Self> {
			Rc::new(Self {
				name: name.to_string(),
				parameters: json!({ "type": "object", "properties": {} }),
				execution_mode: Some(execution_mode),
				execute,
			})
		}
	}

	impl AgentTool for TestTool {
		fn name(&self) -> &str {
			&self.name
		}
		fn label(&self) -> &str {
			&self.name
		}
		fn description(&self) -> &str {
			&self.name
		}
		fn parameters(&self) -> &Value {
			// `Type.Object({})`.
			&self.parameters
		}
		fn execution_mode(&self) -> Option<ToolExecutionMode> {
			self.execution_mode
		}
		fn execute(
			&self,
			tool_call_id: &str,
			params: &Value,
			on_update: &mut dyn FnMut(&AgentToolResult),
		) -> AgentToolResult {
			(self.execute)(tool_call_id, params, on_update)
		}
	}

	struct TestHost {
		tools: RefCell<Vec<Rc<dyn AgentTool>>>,
		sequential: bool,
		events: RefCell<Vec<NestedToolExecutionEvent>>,
	}

	impl TestHost {
		fn new(sequential: bool) -> Self {
			Self {
				tools: RefCell::new(Vec::new()),
				sequential,
				events: RefCell::new(Vec::new()),
			}
		}

		fn add_tool(&self, tool: Rc<dyn AgentTool>) {
			self.tools.borrow_mut().push(tool);
		}

		fn event_triples(&self) -> Vec<(&'static str, String, String)> {
			self.events
				.borrow()
				.iter()
				.map(|event| {
					(
						event.kind(),
						event.tool_call_id().to_string(),
						event.parent_tool_call_id().to_string(),
					)
				})
				.collect()
		}
	}

	impl NestedToolCallHost for TestHost {
		fn get_tools(&self) -> Vec<Rc<dyn AgentTool>> {
			self.tools.borrow().clone()
		}

		fn is_sequential(&self) -> bool {
			self.sequential
		}

		fn run_tool_call(
			&self,
			tool_call: &AgentToolCall,
			_parent_id: &str,
			on_update: &mut dyn FnMut(&AgentToolResult),
		) -> AgentToolCallOutcome {
			// Clone the Rc before executing: a nested tool may re-enter.
			let tool = self
				.tools
				.borrow()
				.iter()
				.find(|candidate| candidate.name() == tool_call.name)
				.cloned();
			let Some(tool) = tool else {
				return AgentToolCallOutcome {
					tool_call: tool_call.clone(),
					result: AgentToolResult::text(format!("Tool {} not found", tool_call.name)),
					is_error: true,
					duration_ms: None,
				};
			};
			let result = tool.execute(&tool_call.id, &tool_call.arguments, &mut |partial| {
				on_update(partial)
			});
			AgentToolCallOutcome {
				tool_call: tool_call.clone(),
				is_error: result.is_error == Some(true),
				result,
				duration_ms: None,
			}
		}

		fn emit(&self, event: NestedToolExecutionEvent) {
			self.events.borrow_mut().push(event);
		}
	}

	fn create_runner(sequential: bool) -> Rc<NestedToolCallRunner<TestHost>> {
		Rc::new(NestedToolCallRunner::new(TestHost::new(sequential)))
	}

	// ------------------------------------------------------------------
	// NestedToolCallRunner
	// ------------------------------------------------------------------

	#[test]
	fn assigns_ids_emits_events_and_records() {
		let runner = create_runner(false);
		runner.host().add_tool(TestTool::new("echo", Box::new(
			|_id, _params, on_update| {
				on_update(&AgentToolResult::text("partial"));
				AgentToolResult::text("ok")
			},
		)));
		let mut updates: Vec<AgentToolResult> = Vec::new();

		let first = runner.execute(
			"call",
			"echo",
			&json!({"a": 1}),
			NestedToolCallOptions {
				on_update: Some(&mut |partial: &AgentToolResult| updates.push(partial.clone())),
			},
		);
		let missing = runner.execute("call", "missing", &json!({}), NestedToolCallOptions::default());

		assert_eq!(first.tool_call.id, "call/1");
		assert_eq!(missing.tool_call.id, "call/2");
		assert!(missing.is_error);
		assert_eq!(updates.len(), 1);
		assert_eq!(updates[0].content, AgentToolResult::text("partial").content);
		assert_eq!(
			runner.host().event_triples(),
			vec![
				("tool_execution_start", "call/1".to_string(), "call".to_string()),
				("tool_execution_update", "call/1".to_string(), "call".to_string()),
				("tool_execution_end", "call/1".to_string(), "call".to_string()),
				("tool_execution_start", "call/2".to_string(), "call".to_string()),
				("tool_execution_end", "call/2".to_string(), "call".to_string()),
			]
		);
		let summary = runner.take_record("call").unwrap();
		let calls = summary.calls.unwrap();
		assert!(calls.complete);
		assert_eq!(calls.calls.len(), 2);
		assert_eq!(calls.calls[0].id, "call/1");
		assert_eq!(calls.calls[0].name, "echo");
		assert_eq!(calls.calls[0].arguments, Some(json!({"a": 1})));
		assert_eq!(calls.calls[0].status, NestedCallStatus::Ok);
		assert!(calls.calls[0].duration_ms.is_some());
		assert_eq!(calls.calls[1].id, "call/2");
		assert_eq!(calls.calls[1].status, NestedCallStatus::Error);
		assert_eq!(calls.calls[1].arguments, Some(json!({})));
		assert_eq!(calls.calls[1].error.as_deref(), Some("Tool missing not found"));
		// The record is taken once.
		assert!(runner.take_record("call").is_none());
		assert!(runner.take_record("other").is_none());
	}

	#[test]
	fn records_calls_of_nested_tools_on_the_model_issued_call() {
		let runner = create_runner(false);
		runner.host().add_tool(TestTool::new("leaf", Box::new(|_, _, _| {
			AgentToolResult::default()
		})));
		let middle_runner = Rc::clone(&runner);
		runner.host().add_tool(TestTool::new(
			"middle",
			Box::new(move |tool_call_id, _params, _on_update| {
				middle_runner.execute(tool_call_id, "leaf", &json!({}), NestedToolCallOptions::default());
				AgentToolResult::default()
			}),
		));

		runner.execute("call", "middle", &json!({}), NestedToolCallOptions::default());

		let summary = runner.take_record("call").unwrap();
		let calls = summary.calls.unwrap();
		let ids: Vec<&str> = calls.calls.iter().map(|call| call.id.as_str()).collect();
		assert_eq!(ids, vec!["call/1", "call/1/1"]);
	}

	#[test]
	fn sums_the_usage_of_nested_results_at_every_depth() {
		let runner = create_runner(false);
		runner.host().add_tool(TestTool::new("leaf", Box::new(|_, _, _| {
			AgentToolResult {
				usage: Some(usage(10.0, 0.01)),
				..Default::default()
			}
		})));
		runner
			.host()
			.add_tool(TestTool::new("plain", Box::new(|_, _, _| AgentToolResult::default())));
		let middle_runner = Rc::clone(&runner);
		runner.host().add_tool(TestTool::new(
			"middle",
			Box::new(move |tool_call_id, _params, _on_update| {
				middle_runner.execute(tool_call_id, "leaf", &json!({}), NestedToolCallOptions::default());
				// Its own usage only: the leaf's usage is counted once, by the
				// recorder.
				AgentToolResult {
					usage: Some(usage(5.0, 0.005)),
					..Default::default()
				}
			}),
		));

		runner.execute("call", "middle", &json!({}), NestedToolCallOptions::default());
		runner.execute("call", "leaf", &json!({}), NestedToolCallOptions::default());
		runner.execute("call", "plain", &json!({}), NestedToolCallOptions::default());
		runner.execute("free", "plain", &json!({}), NestedToolCallOptions::default());

		let summary = runner.take_record("call").unwrap();
		let total = summary.usage.unwrap();
		assert_eq!(total.input, 25.0);
		assert!((total.cost.total - 0.025).abs() < 1e-10);
		let free = runner.take_record("free").unwrap();
		assert!(free.calls.unwrap().complete);
		assert_eq!(free.usage, None);
	}

	#[test]
	fn sequential_tools_execute_in_order() {
		// The TS races concurrent calls with Promise.all and asserts the
		// sequential tool never overlaps; execution here is synchronous, so
		// the calls trivially serialize. This mirrors the observable part:
		// sequential-mode tools run through the exclusive path and their
		// events interleave in call order.
		let order: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
		let runner = create_runner(false);
		let order_tool = Rc::clone(&order);
		runner.host().add_tool(TestTool::with_mode(
			"sequential",
			ToolExecutionMode::Sequential,
			Box::new(move |id, _params, _on_update| {
				order_tool.borrow_mut().push(format!("start {id}"));
				order_tool.borrow_mut().push(format!("end {id}"));
				AgentToolResult::default()
			}),
		));

		for n in 1..=3 {
			runner.execute("call", "sequential", &json!({ "n": n }), NestedToolCallOptions::default());
		}

		assert_eq!(
			*order.borrow(),
			vec![
				"start call/1".to_string(),
				"end call/1".to_string(),
				"start call/2".to_string(),
				"end call/2".to_string(),
				"start call/3".to_string(),
				"end call/3".to_string(),
			]
		);
		assert!(runner.take_record("call").unwrap().calls.unwrap().complete);
	}

	// ------------------------------------------------------------------
	// NestedCallRecorder
	// ------------------------------------------------------------------

	#[test]
	fn recorder_omits_oversized_arguments_and_drops_calls_beyond_the_limit() {
		let mut recorder = NestedCallRecorder::new();
		assert_eq!(recorder.snapshot(), None);
		let small = recorder.start(&call("a", json!({"x": 1})));
		recorder.finish(small, false, "");
		let snapshot = recorder.snapshot().unwrap();
		assert!(snapshot.complete);
		assert_eq!(snapshot.calls.len(), 1);
		assert_eq!(snapshot.calls[0].id, "a");
		assert_eq!(snapshot.calls[0].arguments, Some(json!({"x": 1})));
		assert_eq!(snapshot.calls[0].status, NestedCallStatus::Ok);
		assert!(snapshot.calls[0].duration_ms.is_some());

		let big = recorder.start(&call("b", json!({ "text": "x".repeat(NESTED_CALL_LIMITS.max_argument_bytes_per_call) })));
		recorder.finish(big, true, &"e".repeat(1000));
		let snapshot = recorder.snapshot().unwrap();
		assert!(!snapshot.complete);
		let second = &snapshot.calls[1];
		assert_eq!(second.id, "b");
		assert_eq!(second.status, NestedCallStatus::Error);
		assert_eq!(second.arguments, None);
		assert!(
			second.arguments_bytes.unwrap() as usize > NESTED_CALL_LIMITS.max_argument_bytes_per_call
		);
		assert_eq!(second.error.as_deref().map(str::len), Some(NESTED_CALL_LIMITS.max_error_chars));

		for i in 0..NESTED_CALL_LIMITS.max_calls {
			let handle = recorder.start(&call(&format!("c{i}"), json!({})));
			recorder.finish(handle, false, "");
		}
		assert_eq!(recorder.snapshot().unwrap().calls.len(), NESTED_CALL_LIMITS.max_calls);
	}

	#[test]
	fn recorder_caps_the_total_argument_size_and_marks_unfinished_calls_incomplete() {
		let mut recorder = NestedCallRecorder::new();
		let chunk = json!({ "text": "x".repeat(7000) });
		let records: Vec<Option<usize>> = (0..6)
			.map(|i| recorder.start(&call(&format!("c{i}"), chunk.clone())))
			.collect();
		let snapshot = recorder.snapshot().unwrap();
		// 32 KiB fits four 7000-byte argument objects.
		assert_eq!(
			snapshot
				.calls
				.iter()
				.filter(|entry| entry.arguments.is_some())
				.count(),
			4
		);
		assert!(snapshot.calls.iter().all(|entry| entry.status == NestedCallStatus::Unfinished));
		assert!(!snapshot.complete);
		assert_eq!(records.len(), 6);
		assert!(records.iter().all(Option::is_some));
	}

	#[test]
	fn recorder_normalizes_numbers_like_the_json_round_trip() {
		// JSON.stringify(JSON.parse(...)) turns 1.0 into 1.
		let mut recorder = NestedCallRecorder::new();
		recorder.start(&call("a", json!({"a": 1.0, "b": 2})));
		let snapshot = recorder.snapshot().unwrap();
		assert_eq!(snapshot.calls[0].arguments, Some(json!({"a": 1, "b": 2})));
	}

	#[test]
	fn recorder_truncates_errors_at_utf16_boundaries() {
		let mut recorder = NestedCallRecorder::new();
		let handle = recorder.start(&call("a", json!({})));
		// 250 surrogate pairs = 500 UTF-16 units.
		let error_text: String = "𝄞".repeat(300);
		recorder.finish(handle, true, &error_text);
		let snapshot = recorder.snapshot().unwrap();
		let error = snapshot.calls[0].error.as_deref().unwrap();
		assert_eq!(error.encode_utf16().count(), NESTED_CALL_LIMITS.max_error_chars);
		assert_eq!(error.chars().count(), 250);
	}

	#[test]
	fn recorder_serializes_with_ts_key_order() {
		let mut recorder = NestedCallRecorder::new();
		let handle = recorder.start(&call("a", json!({"x": 1})));
		recorder.finish(handle, true, "boom");
		let snapshot = recorder.snapshot().unwrap();
		let json = serde_json::to_value(&snapshot).unwrap();
		assert_eq!(
			json,
			json!({
				"calls": [{
					"id": "a",
					"name": "t",
					"status": "error",
					"arguments": {"x": 1},
					"durationMs": snapshot.calls[0].duration_ms,
					"error": "boom"
				}],
				"complete": true
			})
		);
		let text = json.to_string();
		assert!(text.starts_with(r#"{"calls":[{"id":"a","name":"t","status":"error","arguments":"#,));
	}

	#[test]
	fn runner_clear_drops_all_scopes() {
		let runner = create_runner(false);
		runner.execute("call", "missing", &json!({}), NestedToolCallOptions::default());
		assert!(runner.take_record("call").is_some());
		runner.execute("call", "missing", &json!({}), NestedToolCallOptions::default());
		runner.clear();
		assert!(runner.take_record("call").is_none());
	}

	#[test]
	fn js_json_stringify_matches_v8() {
		assert_eq!(js_json_stringify(&json!({})), "{}");
		assert_eq!(js_json_stringify(&json!({"a": 1})), r#"{"a":1}"#);
		assert_eq!(js_json_stringify(&json!([1, "x"])), r#"[1,"x"]"#);
		// Integral floats print without a decimal point.
		assert_eq!(js_json_stringify(&json!({"a": 1.0})), r#"{"a":1}"#);
		assert_eq!(js_json_stringify(&json!(0.25)), "0.25");
		assert_eq!(js_json_stringify(&Value::Bool(true)), "true");
		assert_eq!(js_json_stringify(&Value::Null), "null");
		// Exponential range.
		assert_eq!(format_js_number(1e21), "1e+21");
		assert_eq!(format_js_number(1e-7), "1e-7");
		assert_eq!(format_js_number(1.5e-7), "1.5e-7");
		assert_eq!(format_js_number(1e20), "100000000000000000000");
	}
}
