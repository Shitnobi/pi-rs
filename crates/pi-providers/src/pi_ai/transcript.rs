//! Transcript helpers — port of `pi-ai` `src/utils/transcript.ts`.
//!
//! `normalizeContext` is the only producer of [`TranscriptContext`]; every
//! provider-facing function expects its result. Messages/tools are the
//! placeholder JSON forms (agent-layer types land with the agent-session
//! tranches), so the helpers operate on `serde_json::Value`s with JS
//! semantics: insertion-ordered maps, `Object.entries` iteration, `null`
//! vs absent distinctions.
//!
//! `getCurrentSystemMessage`/`contentText` already live in `pi_core::ai`
//! (differentially verified); this module reuses them and ports the rest.

use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::pi_ai::provider::TranscriptContext;
use pi_core::ai::{content_text_default, get_current_system_message};

// ============================================================================
// normalizeContext
// ============================================================================

/// TS `createInitialSystemMessage` — the leading system message for a
/// prompt and tool set; `None` when both are empty (an empty transcript
/// stays empty). `tools` are the raw JSON tool declarations.
pub fn create_initial_system_message(
	system_prompt: Option<&str>,
	tools: Option<&[Value]>,
) -> Option<Value> {
	let has_system_prompt = system_prompt.is_some_and(|p| !p.is_empty());
	let has_tools = tools.is_some_and(|t| !t.is_empty());
	if !has_system_prompt && !has_tools {
		return None;
	}
	let mut message = Map::new();
	message.insert("role".to_string(), json!("system"));
	message.insert(
		"content".to_string(),
		json!(system_prompt.unwrap_or("")),
	);
	if has_tools {
		message.insert(
			"toolsAdded".to_string(),
			Value::Array(tools.unwrap().to_vec()),
		);
	}
	message.insert("timestamp".to_string(), json!(0));
	Some(Value::Object(message))
}

/// TS `normalizeContext` — fold `Context.systemPrompt` and `Context.tools`
/// into a leading system message. `context` is the JSON `Context` form
/// (`{ systemPrompt?, tools?, messages }`); the result is the
/// `TranscriptContext` placeholder (`{ messages }`).
pub fn normalize_context(context: &Value) -> TranscriptContext {
	let initial_message = create_initial_system_message(
		context.get("systemPrompt").and_then(Value::as_str),
		context.get("tools").and_then(Value::as_array).map(|v| v.as_slice()),
	);
	let mut messages: Vec<Value> = Vec::new();
	if let Some(initial) = initial_message {
		messages.push(initial);
	}
	if let Some(existing) = context.get("messages").and_then(Value::as_array) {
		messages.extend(existing.iter().cloned());
	}
	TranscriptContext(json!({ "messages": messages }))
}

/// The message list of a `TranscriptContext` placeholder.
pub fn transcript_messages(context: &TranscriptContext) -> Vec<Value> {
	context
		.0
		.get("messages")
		.and_then(Value::as_array)
		.cloned()
		.unwrap_or_default()
}

// ============================================================================
// System-message replay helpers
// ============================================================================

fn is_system_message(message: &Value) -> bool {
	message.get("role").and_then(Value::as_str) == Some("system")
}

/// TS `getInitialSystemMessage` — the leading system message, if any.
pub fn get_initial_system_message(messages: &[Value]) -> Option<Value> {
	messages.first().filter(|m| is_system_message(m)).cloned()
}

/// TS `withoutInitialSystemMessage` — drop the leading system message.
pub fn without_initial_system_message(messages: &[Value]) -> Vec<Value> {
	if messages.first().is_some_and(is_system_message) {
		messages[1..].to_vec()
	} else {
		messages.to_vec()
	}
}

/// TS `getCurrentTools` (replay of every system-message tool delta, last
/// definition per name wins). Mirrors the `pi_core::ai` private helper.
pub fn get_current_tools(messages: &[Value]) -> Vec<Value> {
	let mut tools: IndexMap<String, Value> = IndexMap::new();
	for message in messages {
		if !is_system_message(message) {
			continue;
		}
		if let Some(removed) = message.get("toolsRemoved").and_then(Value::as_array) {
			for tool in removed {
				if let Some(name) = tool.get("name").and_then(Value::as_str) {
					tools.shift_remove(name);
				}
			}
		}
		if let Some(added) = message.get("toolsAdded").and_then(Value::as_array) {
			for tool in added {
				if let Some(name) = tool.get("name").and_then(Value::as_str) {
					tools.insert(name.to_string(), tool.clone());
				}
			}
		}
	}
	tools.into_values().collect()
}

/// TS `getSystemMessageText` (utils/text.ts) — prompt text plus non-null
/// section values, joined with blank lines.
pub fn get_system_message_text(message: &Value) -> String {
	let mut parts = vec![content_text_default(
		message.get("content").unwrap_or(&Value::Null),
	)];
	if let Some(sections) = message.get("sections").and_then(Value::as_object) {
		for value in sections.values() {
			if let Some(text) = value.as_str() {
				parts.push(text.to_string());
			}
		}
	}
	parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// TS `getCurrentSystemPrompt` — current system prompt text after replay.
pub fn get_current_system_prompt(messages: &[Value]) -> String {
	get_current_system_message(messages)
		.map(|m| get_system_message_text(&m))
		.unwrap_or_default()
}

/// TS `collapseSystemMessages` — rebuild for APIs without mid-conversation
/// system messages: the replayed system message leads, later ones dropped.
pub fn collapse_system_messages(context: &TranscriptContext) -> TranscriptContext {
	let messages = transcript_messages(context);
	let head = get_current_system_message(&messages);
	let mut out: Vec<Value> = Vec::new();
	if let Some(head) = head {
		out.push(head);
	}
	out.extend(messages.iter().filter(|m| !is_system_message(m)).cloned());
	TranscriptContext(json!({ "messages": out }))
}

/// TS `resolveTranscript` — keep later system messages when the model
/// accepts them, otherwise collapse.
pub fn resolve_transcript(
	context: &TranscriptContext,
	supports_mid_convo_system_messages: bool,
) -> TranscriptContext {
	if supports_mid_convo_system_messages {
		context.clone()
	} else {
		collapse_system_messages(context)
	}
}

// ============================================================================
// Tool declarations
// ============================================================================

/// TS `toToolDeclaration` — strip executable/display-only fields. The
/// `parameters` clone is a deep JSON clone (`JSON.parse(JSON.stringify())`).
pub fn to_tool_declaration(tool: &Value) -> Value {
	let mut out = Map::new();
	out.insert(
		"name".to_string(),
		tool.get("name").cloned().unwrap_or(Value::Null),
	);
	out.insert(
		"description".to_string(),
		tool.get("description").cloned().unwrap_or(Value::Null),
	);
	out.insert(
		"parameters".to_string(),
		tool.get("parameters").cloned().unwrap_or(Value::Null),
	);
	if let Some(constrained) = tool.get("constrainedSampling") {
		out.insert("constrainedSampling".to_string(), constrained.clone());
	}
	Value::Object(out)
}

/// TS `declarationsEqual` — both sides go through `toToolDeclaration`, so
/// comparing the serialized declarations (same key order, `undefined`
/// dropped) is exact.
pub fn declarations_equal(left: &Value, right: &Value) -> bool {
	let ok = |v: &Value| serde_json::to_string(&to_tool_declaration(v)).ok();
	ok(left) == ok(right)
}

/// TS `ToolStateChanges` — a changed definition is a removal plus an
/// addition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolStateChanges {
	pub tools_added: Vec<Value>,
	pub tools_removed: Vec<Value>,
}

/// TS `getToolStateChanges` — compare two complete tool states.
pub fn get_tool_state_changes(previous: &[Value], current: &[Value]) -> ToolStateChanges {
	let previous_tools: IndexMap<&str, &Value> = previous
		.iter()
		.filter_map(|t| Some((t.get("name")?.as_str()?, t)))
		.collect();
	let current_tools: IndexMap<&str, &Value> = current
		.iter()
		.filter_map(|t| Some((t.get("name")?.as_str()?, t)))
		.collect();
	let tools_added = current
		.iter()
		.filter(|tool| {
			let name = tool.get("name").and_then(Value::as_str);
			match name.and_then(|n| previous_tools.get(n)) {
				None => true,
				Some(previous_tool) => !declarations_equal(previous_tool, tool),
			}
		})
		.map(to_tool_declaration)
		.collect();
	let tools_removed = previous
		.iter()
		.filter(|tool| {
			let name = tool.get("name").and_then(Value::as_str);
			match name.and_then(|n| current_tools.get(n)) {
				None => true,
				Some(current_tool) => !declarations_equal(tool, current_tool),
			}
		})
		.map(|t| json!({ "name": t.get("name").cloned().unwrap_or(Value::Null) }))
		.collect();
	ToolStateChanges { tools_added, tools_removed }
}

/// TS `getDeclaredTools` — every definition referenced by transcript tool
/// state, in first-declaration order.
pub fn get_declared_tools(messages: &[Value]) -> Vec<Value> {
	let mut definitions: IndexMap<String, Value> = IndexMap::new();
	for message in messages {
		if !is_system_message(message) {
			continue;
		}
		if let Some(added) = message.get("toolsAdded").and_then(Value::as_array) {
			for tool in added {
				if let Some(name) = tool.get("name").and_then(Value::as_str) {
					definitions.insert(name.to_string(), tool.clone());
				}
			}
		}
	}
	definitions.into_values().collect()
}

/// TS `hasToolRedefinitions` (deprecated in TS, kept for compatibility).
pub fn has_tool_redefinitions(messages: &[Value]) -> bool {
	let mut declared: IndexMap<String, Value> = IndexMap::new();
	for message in messages {
		if !is_system_message(message) {
			continue;
		}
		if let Some(added) = message.get("toolsAdded").and_then(Value::as_array) {
			for tool in added {
				let Some(name) = tool.get("name").and_then(Value::as_str) else { continue };
				if let Some(previous) = declared.get(name) {
					if !declarations_equal(previous, tool) {
						return true;
					}
				}
				declared.insert(name.to_string(), tool.clone());
			}
		}
	}
	false
}

/// TS `hasNonAdditiveToolChanges` — a removal or same-name redeclaration
/// that an addition-only transport cannot replay.
pub fn has_non_additive_tool_changes(messages: &[Value]) -> bool {
	let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
	for message in messages {
		if !is_system_message(message) {
			continue;
		}
		if message
			.get("toolsRemoved")
			.and_then(Value::as_array)
			.is_some_and(|r| !r.is_empty())
		{
			return true;
		}
		if let Some(added) = message.get("toolsAdded").and_then(Value::as_array) {
			for tool in added {
				if let Some(name) = tool.get("name").and_then(Value::as_str) {
					if !declared.insert(name.to_string()) {
						return true;
					}
				}
			}
		}
	}
	false
}

/// TS `TranscriptTools` — the top-level request tools plus whether later
/// system messages carry their own in-place additions.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptTools {
	pub request_tools: Vec<Value>,
	pub anchors_additions: bool,
}

/// TS `resolveTranscriptTools`.
pub fn resolve_transcript_tools(messages: &[Value], supports_tool_additions: bool) -> TranscriptTools {
	let anchors_additions =
		supports_tool_additions && !has_non_additive_tool_changes(messages);
	let request_tools = if anchors_additions {
		get_initial_system_message(messages)
			.and_then(|m| m.get("toolsAdded").and_then(Value::as_array).cloned())
			.unwrap_or_default()
	} else {
		get_current_tools(messages)
	};
	TranscriptTools { request_tools, anchors_additions }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn normalize_context_folds_prompt_and_tools() {
		// Both empty → messages pass through untouched.
		let ctx = json!({"messages": [{"role": "user", "content": "hi"}]});
		let t = normalize_context(&ctx);
		assert_eq!(
			t.0,
			json!({"messages": [{"role": "user", "content": "hi"}]})
		);
		// Prompt only.
		let t = normalize_context(&json!({
			"systemPrompt": "be brief", "messages": [{"role": "user", "content": "hi"}]
		}));
		assert_eq!(
			t.0,
			json!({"messages": [
				{"role": "system", "content": "be brief", "timestamp": 0},
				{"role": "user", "content": "hi"}
			]})
		);
		// Tools only (empty prompt string, no toolsAdded when tools empty).
		let t = normalize_context(&json!({
			"systemPrompt": "", "tools": [], "messages": []
		}));
		assert_eq!(t.0, json!({"messages": []}));
		let t = normalize_context(&json!({
			"tools": [{"name": "t", "description": "d", "parameters": {}}],
			"messages": []
		}));
		assert_eq!(
			t.0,
			json!({"messages": [
				{"role": "system", "content": "", "toolsAdded": [{"name": "t", "description": "d", "parameters": {}}], "timestamp": 0}
			]})
		);
	}

	#[test]
	fn initial_system_message_helpers() {
		let messages = vec![
			json!({"role": "system", "content": "s", "timestamp": 0}),
			json!({"role": "user", "content": "u"}),
		];
		assert_eq!(
			get_initial_system_message(&messages).unwrap()["content"],
			json!("s")
		);
		assert_eq!(without_initial_system_message(&messages).len(), 1);
		assert_eq!(get_initial_system_message(&messages[1..]), None);
		assert_eq!(without_initial_system_message(&messages[1..]).len(), 1);
	}

	#[test]
	fn current_tools_replay() {
		let messages = vec![
			json!({"role": "system", "content": "", "timestamp": 0,
				"toolsAdded": [{"name": "a", "description": "1", "parameters": {}}, {"name": "b", "description": "2", "parameters": {}}]}),
			json!({"role": "user", "content": "u"}),
			json!({"role": "system", "content": "", "timestamp": 1,
				"toolsRemoved": [{"name": "a"}],
				"toolsAdded": [{"name": "b", "description": "2b", "parameters": {}}, {"name": "c", "description": "3", "parameters": {}}]}),
		];
		let tools = get_current_tools(&messages);
		let names: Vec<&str> = tools
			.iter()
			.map(|t| t["name"].as_str().unwrap())
			.collect();
		assert_eq!(names, vec!["b", "c"]);
		assert_eq!(tools[0]["description"], json!("2b"));
	}

	#[test]
	fn collapse_and_resolve() {
		let context = TranscriptContext(json!({"messages": [
			{"role": "system", "content": "one", "timestamp": 5},
			{"role": "user", "content": "u"},
			{"role": "system", "content": "two", "timestamp": 6}
		]}));
		let collapsed = collapse_system_messages(&context);
		assert_eq!(
			collapsed.0,
			json!({"messages": [
				{"role": "system", "content": "one\n\ntwo", "timestamp": 5},
				{"role": "user", "content": "u"}
			]})
		);
		// Keeping mid-convo messages returns the context unchanged.
		let kept = resolve_transcript(&context, true);
		assert_eq!(kept.0, context.0);
		let collapsed = resolve_transcript(&context, false);
		assert_eq!(collapsed.0["messages"].as_array().unwrap().len(), 2);
	}

	#[test]
	fn system_prompt_text_renders_sections() {
		let messages = vec![json!({
			"role": "system", "content": "base", "timestamp": 0,
			"sections": {"skills": "use tools", "gone": null}
		})];
		assert_eq!(get_current_system_prompt(&messages), "base\n\nuse tools");
		assert_eq!(get_current_system_prompt(&[json!({"role": "user", "content": "u"})]), "");
	}

	#[test]
	fn tool_declarations_and_changes() {
		let tool = json!({"name": "t", "description": "d", "parameters": {"a": 1},
			"constrainedSampling": {"x": 1}, "executable": "stripped"});
		let declaration = to_tool_declaration(&tool);
		assert_eq!(
			declaration,
			json!({"name": "t", "description": "d", "parameters": {"a": 1},
				"constrainedSampling": {"x": 1}})
		);
		// No constrainedSampling → key omitted.
		let plain = json!({"name": "t", "description": "d", "parameters": {}});
		assert!(!to_tool_declaration(&plain).get("constrainedSampling").is_some());

		let same = json!({"name": "t", "description": "d", "parameters": {"a": 1},
			"constrainedSampling": {"x": 1}, "executable": "other"});
		assert!(declarations_equal(&tool, &same));
		let changed = json!({"name": "t", "description": "d2", "parameters": {"a": 1}});
		assert!(!declarations_equal(&tool, &changed));

		let changes = get_tool_state_changes(std::slice::from_ref(&tool), &[changed.clone(), json!({"name": "n", "description": "x", "parameters": {}})]);
		assert_eq!(changes.tools_added.len(), 2);
		assert_eq!(changes.tools_removed, vec![json!({"name": "t"})]);
		// Unchanged definitions produce no changes.
		let changes = get_tool_state_changes(std::slice::from_ref(&tool), std::slice::from_ref(&same));
		assert_eq!(changes.tools_added.len(), 0);
		assert_eq!(changes.tools_removed.len(), 0);
	}

	#[test]
	fn declared_and_non_additive() {
		let messages = vec![
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "a", "description": "1", "parameters": {}}]}),
			json!({"role": "user", "content": "u"}),
		];
		assert_eq!(get_declared_tools(&messages).len(), 1);
		assert!(!has_non_additive_tool_changes(&messages));
		assert!(!has_tool_redefinitions(&messages));

		let redeclared = vec![
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "a", "description": "1", "parameters": {}}]}),
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "a", "description": "2", "parameters": {}}]}),
		];
		assert!(has_non_additive_tool_changes(&redeclared));
		assert!(has_tool_redefinitions(&redeclared));

		let removed = vec![
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "a", "description": "1", "parameters": {}}]}),
			json!({"role": "system", "content": "", "toolsRemoved": [{"name": "a"}]}),
		];
		assert!(has_non_additive_tool_changes(&removed));
		assert!(!has_tool_redefinitions(&removed));
	}

	#[test]
	fn resolve_transcript_tools_split() {
		let additive = vec![
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "a", "description": "1", "parameters": {}}, {"name": "b", "description": "2", "parameters": {}}]}),
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "c", "description": "3", "parameters": {}}]}),
		];
		let anchored = resolve_transcript_tools(&additive, true);
		assert!(anchored.anchors_additions);
		assert_eq!(anchored.request_tools.len(), 2); // initial only
		let flat = resolve_transcript_tools(&additive, false);
		assert!(!flat.anchors_additions);
		assert_eq!(flat.request_tools.len(), 3); // current set
		// Non-additive history forces the flat list even with support.
		let non_additive = vec![
			json!({"role": "system", "content": "", "toolsAdded": [{"name": "a", "description": "1", "parameters": {}}]}),
			json!({"role": "system", "content": "", "toolsRemoved": [{"name": "a"}]}),
		];
		let forced = resolve_transcript_tools(&non_additive, true);
		assert!(!forced.anchors_additions);
		assert!(forced.request_tools.is_empty());
	}
}
