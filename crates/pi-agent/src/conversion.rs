//! Port of the message-conversion half of `packages/agent/src/agent-loop.ts`
//! (`declareToolChanges`, `withToolChanges`), the `defaultConvertToLlm`
//! filter from `agent.ts`, and `estimateMessagesTokens` from
//! `coding-agent/src/core/agent-session.ts` (the compaction `estimateTokens`
//! arithmetic, already ported in `pi-core`).

use serde_json::{Map, Value};

use pi_providers::pi_ai::transcript::{
    get_current_tools, get_tool_state_changes, to_tool_declaration, ToolStateChanges,
};

/// TS `NO_CHANGES`.
fn no_changes() -> ToolStateChanges {
    ToolStateChanges::default()
}

/// TS `withToolChanges` — copy a system message with its tool fields
/// replaced by `changes`; empty lists omit the field. The remaining keys
/// keep their original order and the tool fields go last (the TS
/// `{...rest, toolsAdded?, toolsRemoved?}` spread order).
pub fn with_tool_changes(message: &Value, changes: &ToolStateChanges) -> Value {
    let mut rest = message.as_object().cloned().unwrap_or_default();
    rest.shift_remove("toolsAdded");
    rest.shift_remove("toolsRemoved");
    if !changes.tools_added.is_empty() {
        rest.insert("toolsAdded".to_string(), Value::Array(changes.tools_added.clone()));
    }
    if !changes.tools_removed.is_empty() {
        rest.insert("toolsRemoved".to_string(), Value::Array(changes.tools_removed.clone()));
    }
    Value::Object(rest)
}

/// TS `declareToolChanges` — the tool-loadout delta as pending system
/// message changes. `context_tools` are the executable tool definitions;
/// `date_now_ms` feeds the inserted system message timestamp (`Date.now`).
pub fn declare_tool_changes(
    context_messages: &[Value],
    context_tools: &[Value],
    pending_messages: &[Value],
    date_now_ms: f64,
) -> Vec<Value> {
    let system_index = pending_messages
        .iter()
        .rposition(|m| m.get("role").and_then(Value::as_str) == Some("system"));

    let baseline: Vec<Value> = match system_index {
        Some(index) => pending_messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if i == index {
                    with_tool_changes(m, &no_changes())
                } else {
                    m.clone()
                }
            })
            .collect(),
        None => pending_messages.to_vec(),
    };

    let mut replay = context_messages.to_vec();
    replay.extend(baseline.iter().cloned());
    let current: Vec<Value> = context_tools.iter().map(to_tool_declaration).collect();
    let changes = get_tool_state_changes(&get_current_tools(&replay), &current);
    let unchanged = changes.tools_added.is_empty() && changes.tools_removed.is_empty();

    if let Some(index) = system_index {
        let pending = &pending_messages[index];
        let pending_added = pending.get("toolsAdded").and_then(Value::as_array).map_or(0, Vec::len);
        let pending_removed = pending.get("toolsRemoved").and_then(Value::as_array).map_or(0, Vec::len);
        if unchanged && pending_added == 0 && pending_removed == 0 {
            return pending_messages.to_vec();
        }
        return baseline
            .iter()
            .enumerate()
            .map(|(i, m)| if i == index { with_tool_changes(pending, &changes) } else { m.clone() })
            .collect();
    }

    if unchanged {
        return pending_messages.to_vec();
    }

    let update = with_tool_changes(
        &Value::Object({
            let mut map = Map::new();
            map.insert("role".to_string(), Value::String("system".to_string()));
            map.insert("content".to_string(), Value::String(String::new()));
            map.insert("timestamp".to_string(), pi_core::ai::js_number(date_now_ms));
            map
        }),
        &changes,
    );
    let insert_index = pending_messages
        .iter()
        .position(|m| m.get("role").and_then(Value::as_str) != Some("system"));
    let index = insert_index.unwrap_or(pending_messages.len());
    let mut out = pending_messages[..index].to_vec();
    out.push(update);
    out.extend(pending_messages[index..].iter().cloned());
    out
}

/// TS `defaultConvertToLlm` — keep the roles providers understand.
pub fn default_convert_to_llm(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter(|m| {
            matches!(
                m.get("role").and_then(Value::as_str),
                Some("system") | Some("user") | Some("assistant") | Some("toolResult")
            )
        })
        .cloned()
        .collect()
}

/// TS `estimateMessagesTokens` — sum of the compaction `estimateTokens`
/// heuristic over every message.
pub fn estimate_messages_tokens(messages: &[Value]) -> i64 {
    let mut tokens = 0.0f64;
    for message in messages {
        tokens += pi_core::compaction::estimate_tokens(message);
    }
    tokens as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sys(content: &str) -> Value {
        json!({ "role": "system", "content": content, "timestamp": 1 })
    }

    #[test]
    fn unchanged_returns_pending_as_is() {
        let context = vec![json!({
            "role": "system", "content": "", "timestamp": 0,
            "toolsAdded": [ { "name": "read", "description": "d", "parameters": {} } ]
        })];
        let tools = vec![json!({ "name": "read", "description": "d", "parameters": {}, "label": "Read" })];
        let pending = vec![json!({ "role": "user", "content": "hi", "timestamp": 2 })];
        let out = declare_tool_changes(&context, &tools, &pending, 1000.0);
        assert_eq!(out, pending);
    }

    #[test]
    fn new_system_message_inserted_before_first_non_system() {
        let context: Vec<Value> = vec![];
        let tools = vec![json!({ "name": "read", "description": "d", "parameters": {} })];
        let pending = vec![json!({ "role": "user", "content": "hi", "timestamp": 2 })];
        let out = declare_tool_changes(&context, &tools, &pending, 1234.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["timestamp"], json!(1234));
        assert_eq!(out[0]["toolsAdded"].as_array().unwrap().len(), 1);
        // Field order: role, content, timestamp, toolsAdded (tool fields last).
        let keys: Vec<&String> = out[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["role", "content", "timestamp", "toolsAdded"]);
        assert_eq!(out[1], pending[0]);
    }

    #[test]
    fn pending_system_fields_replaced_by_delta() {
        let context = vec![json!({
            "role": "system", "content": "", "timestamp": 0,
            "toolsAdded": [ { "name": "old", "description": "d", "parameters": {} } ]
        })];
        let tools = vec![json!({ "name": "new", "description": "d", "parameters": {} })];
        let pending = vec![json!({
            "role": "system", "content": "", "timestamp": 5,
            "toolsAdded": [ { "name": "stale", "description": "d", "parameters": {} } ]
        })];
        let out = declare_tool_changes(&context, &tools, &pending, 100.0);
        assert_eq!(out.len(), 1);
        let added: Vec<&str> = out[0]["toolsAdded"].as_array().unwrap()
            .iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(added, ["new"]);
        let removed: Vec<&str> = out[0]["toolsRemoved"].as_array().unwrap()
            .iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(removed, ["old"]);
        // toolsRemoved goes after toolsAdded; other fields keep their slots.
        let keys: Vec<&String> = out[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["role", "content", "timestamp", "toolsAdded", "toolsRemoved"]);
    }

    #[test]
    fn pending_system_already_clean_kept_as_is() {
        let context = vec![json!({
            "role": "system", "content": "", "timestamp": 0,
            "toolsAdded": [ { "name": "read", "description": "d", "parameters": {} } ]
        })];
        let tools = vec![json!({ "name": "read", "description": "d", "parameters": {} })];
        let pending = vec![json!({ "role": "system", "content": "", "timestamp": 5 })];
        let out = declare_tool_changes(&context, &tools, &pending, 100.0);
        assert_eq!(out, pending);
    }

    #[test]
    fn tools_removed_only() {
        let context = vec![json!({
            "role": "system", "content": "", "timestamp": 0,
            "toolsAdded": [ { "name": "gone", "description": "d", "parameters": {} } ]
        })];
        let out = declare_tool_changes(&context, &[], &[], 7.0);
        assert_eq!(out.len(), 1);
        assert!(out[0].get("toolsAdded").is_none());
        assert_eq!(out[0]["toolsRemoved"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn convert_to_llm_filters_roles() {
        let messages = vec![
            json!({ "role": "system", "content": "" }),
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": [] }),
            json!({ "role": "toolResult", "content": [] }),
            json!({ "role": "custom", "content": [] }),
            json!({ "role": "bashExecution", "command": "ls", "output": "" }),
        ];
        assert_eq!(default_convert_to_llm(&messages).len(), 4);
    }

    #[test]
    fn estimate_tokens_sum() {
        let messages = vec![
            json!({ "role": "user", "content": "12345678" }), // 8 chars -> 2
            json!({ "role": "user", "content": "ab" }),       // 2 chars -> 1
        ];
        assert_eq!(estimate_messages_tokens(&messages), 3);
    }
}
