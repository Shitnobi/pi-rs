//! Port of `src/core/compaction/utils.ts`.
//!
//! Shared utilities for compaction and branch summarization: file-operation
//! tracking, message serialization for summarization prompts, and the
//! summarization system prompt.

use serde_json::{json, Map, Value};

use crate::ai::content_text;

// ============================================================================
// File Operation Tracking
// ============================================================================

/// `FileOperations`: insertion-ordered sets (JS `Set` iteration order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

impl FileOperations {
    /// Serialize for the harness / details: `{read, written, edited}` arrays.
    pub fn to_value(&self) -> Value {
        json!({
            "read": self.read,
            "written": self.written,
            "edited": self.edited,
        })
    }

    /// Parse `{read, written, edited}` arrays (harness input). Duplicates are
    /// collapsed like the TS harness's `new Set(...)`.
    pub fn from_value(value: &Value) -> FileOperations {
        let list = |key: &str| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            if let Some(items) = value[key].as_array() {
                for item in items.iter().filter_map(|v| v.as_str()) {
                    FileOperations::add_to(&mut out, item);
                }
            }
            out
        };
        FileOperations {
            read: list("read"),
            written: list("written"),
            edited: list("edited"),
        }
    }

    fn add_to(list: &mut Vec<String>, path: &str) {
        if !list.iter().any(|p| p == path) {
            list.push(path.to_string());
        }
    }

    /// Insert into the `read` set (insertion-ordered).
    pub fn read_push(&mut self, path: &str) {
        FileOperations::add_to(&mut self.read, path);
    }

    /// Insert into the `edited` set (insertion-ordered).
    pub fn edited_push(&mut self, path: &str) {
        FileOperations::add_to(&mut self.edited, path);
    }
}

/// `createFileOps`.
pub fn create_file_ops() -> FileOperations {
    FileOperations::default()
}

/// `extractFileOpsFromMessage`: extract file operations from tool calls in an
/// assistant message, or from the nested calls recorded on a tool result.
pub fn extract_file_ops_from_message(message: &Value, file_ops: &mut FileOperations) {
    let role = message["role"].as_str().unwrap_or("");
    if role == "toolResult" {
        // Calls made from codemode scripts are recorded on the script's result.
        let empty: Vec<Value> = Vec::new();
        let calls = message["nestedCalls"]["calls"].as_array().unwrap_or(&empty);
        for call in calls {
            add_file_op(
                call["name"].as_str().unwrap_or(""),
                call.get("arguments"),
                file_ops,
            );
        }
        return;
    }
    if role != "assistant" {
        return;
    }
    let Some(blocks) = message["content"].as_array() else {
        return;
    };
    for block in blocks {
        if !block.is_object() {
            continue;
        }
        if block["type"] != "toolCall" {
            continue;
        }
        if block.get("arguments").is_none() || block.get("name").is_none() {
            continue;
        }
        add_file_op(block["name"].as_str().unwrap_or(""), block.get("arguments"), file_ops);
    }
}

fn add_file_op(tool_name: &str, args: Option<&Value>, file_ops: &mut FileOperations) {
    let path = args
        .and_then(|a| a.get("path"))
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty());
    let Some(path) = path else { return };
    match tool_name {
        "read" => FileOperations::add_to(&mut file_ops.read, path),
        "write" => FileOperations::add_to(&mut file_ops.written, path),
        "edit" => FileOperations::add_to(&mut file_ops.edited, path),
        _ => {}
    }
}

/// `computeFileLists` result: files only read, and modified files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileLists {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// `computeFileLists`: readFiles (only read, not modified) and modifiedFiles,
/// both sorted.
pub fn compute_file_lists(file_ops: &FileOperations) -> FileLists {
    let mut modified: Vec<String> = file_ops.edited.clone();
    for path in &file_ops.written {
        if !modified.iter().any(|p| p == path) {
            modified.push(path.clone());
        }
    }
    let mut read_only: Vec<String> = file_ops
        .read
        .iter()
        .filter(|f| !modified.iter().any(|p| p == *f))
        .cloned()
        .collect();
    read_only.sort();
    modified.sort();
    FileLists {
        read_files: read_only,
        modified_files: modified,
    }
}

/// `formatFileOperations`: format file operations as XML tags for a summary.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read_files.join("\n")));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

// ============================================================================
// Message Serialization
// ============================================================================

/// Maximum characters for a tool result in serialized summaries.
pub const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// JS `String.prototype.length`: UTF-16 code units.
pub(crate) fn js_str_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// JS `String.prototype.slice(0, n)` truncated at a char boundary (lone
/// surrogate halves cannot be represented in UTF-8).
pub(crate) fn js_slice_prefix(s: &str, max_units: usize) -> &str {
    let mut count = 0usize;
    for (idx, ch) in s.char_indices() {
        if count >= max_units {
            return &s[..idx];
        }
        count += ch.len_utf16();
    }
    s
}

/// `truncateForSummary`: keep the beginning, append a truncation marker.
pub fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let units = js_str_len(text);
    if units <= max_chars {
        return text.to_string();
    }
    let truncated_chars = units - max_chars;
    format!(
        "{}\n\n[... {} more characters truncated]",
        js_slice_prefix(text, max_chars),
        truncated_chars
    )
}

/// `JSON.stringify` for a JSON value (byte-identical for parsed JSON).
pub(crate) fn js_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// `Object.entries(value).map(([k, v]) => k=JSON.stringify(v)).join(", ")`.
fn object_entries_args_str(value: &Value) -> String {
    let pairs: Vec<String> = match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{}={}", k, js_stringify(v)))
            .collect(),
        // Object.entries() on an array yields index keys.
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| format!("{}={}", i, js_stringify(v)))
            .collect(),
        _ => Vec::new(),
    };
    pairs.join(", ")
}

/// `serializeConversation`: serialize LLM messages to text for summarization.
/// Tool results are truncated to `TOOL_RESULT_MAX_CHARS`; thinking is omitted.
pub fn serialize_conversation(messages: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for msg in messages {
        match msg["role"].as_str() {
            Some("user") => {
                let content = content_text(&msg["content"], "");
                if !content.is_empty() {
                    parts.push(format!("[User]: {}", content));
                }
            }
            Some("assistant") => {
                let mut tool_calls: Vec<String> = Vec::new();
                if let Some(blocks) = msg["content"].as_array() {
                    for block in blocks {
                        if block["type"] != "toolCall" {
                            continue;
                        }
                        let name = block["name"].as_str().unwrap_or("");
                        let args_str = object_entries_args_str(&block["arguments"]);
                        tool_calls.push(format!("{}({})", name, args_str));
                    }
                    if blocks.iter().any(|b| b["type"] == "text") {
                        parts.push(format!("[Assistant]: {}", content_text(&msg["content"], "\n")));
                    }
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Some("toolResult") => {
                let content = content_text(&msg["content"], "");
                if !content.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            _ => {}
        }
    }

    parts.join("\n\n")
}

// ============================================================================
// Summarization System Prompt
// ============================================================================

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

// ============================================================================
// Harness helpers
// ============================================================================

/// Serialize a `FileLists` pair the way `computeFileLists` returns it.
pub fn file_lists_to_value(lists: &FileLists) -> Value {
    let mut map = Map::new();
    map.insert("readFiles".into(), json!(lists.read_files));
    map.insert("modifiedFiles".into(), json!(lists.modified_files));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_ops_insertion_order_and_dedupe() {
        let mut ops = create_file_ops();
        extract_file_ops_from_message(
            &json!({
                "role": "assistant",
                "content": [
                    {"type": "toolCall", "name": "read", "arguments": {"path": "/a"}},
                    {"type": "toolCall", "name": "read", "arguments": {"path": "/a"}},
                    {"type": "toolCall", "name": "write", "arguments": {"path": "/b"}},
                    {"type": "toolCall", "name": "edit", "arguments": {"path": "/c"}},
                    {"type": "toolCall", "name": "other", "arguments": {"path": "/z"}},
                    {"type": "toolCall", "name": "read", "arguments": {"count": 1}},
                    {"type": "toolCall", "name": "read", "arguments": {"path": 5}},
                ]
            }),
            &mut ops,
        );
        assert_eq!(ops.read, vec!["/a".to_string()]);
        assert_eq!(ops.written, vec!["/b".to_string()]);
        assert_eq!(ops.edited, vec!["/c".to_string()]);
    }

    #[test]
    fn file_ops_from_tool_result_nested_calls() {
        let mut ops = create_file_ops();
        extract_file_ops_from_message(
            &json!({
                "role": "toolResult",
                "content": [],
                "nestedCalls": {"calls": [{"name": "read", "arguments": {"path": "/n"}}]}
            }),
            &mut ops,
        );
        assert_eq!(ops.read, vec!["/n".to_string()]);
    }

    #[test]
    fn compute_file_lists_split_and_sort() {
        let ops = FileOperations::from_value(&json!({
            "read": ["/b", "/a"], "written": ["/b"], "edited": ["/c"]
        }));
        let lists = compute_file_lists(&ops);
        assert_eq!(lists.read_files, vec!["/a".to_string()]);
        assert_eq!(lists.modified_files, vec!["/b".to_string(), "/c".to_string()]);
    }

    #[test]
    fn from_value_dedupes_like_a_set() {
        let ops = FileOperations::from_value(&json!({"read": ["/c", "/c"]}));
        assert_eq!(ops.read, vec!["/c".to_string()]);
    }

    #[test]
    fn format_file_operations_sections() {
        assert_eq!(format_file_operations(&[], &[]), "");
        assert_eq!(
            format_file_operations(&["/a".into(), "/b".into()], &[]),
            "\n\n<read-files>\n/a\n/b\n</read-files>"
        );
        assert_eq!(
            format_file_operations(&[], &["/m".into()]),
            "\n\n<modified-files>\n/m\n</modified-files>"
        );
    }

    #[test]
    fn truncate_for_summary_marker() {
        assert_eq!(truncate_for_summary("abc", 5), "abc");
        let out = truncate_for_summary(&"x".repeat(2505), 2000);
        assert!(out.ends_with("\n\n[... 505 more characters truncated]"));
        assert_eq!(js_str_len(out.trim_end_matches("\n\n[... 505 more characters truncated]")), 2000);
    }

    #[test]
    fn serialize_conversation_shapes() {
        let out = serialize_conversation(&[
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hidden"},
                {"type": "text", "text": "hello"},
                {"type": "toolCall", "name": "read", "arguments": {"path": "/x"}},
            ]}),
            json!({"role": "toolResult", "content": "ok"}),
        ]);
        assert_eq!(
            out,
            "[User]: hi\n\n[Assistant]: hello\n\n[Assistant tool calls]: read(path=\"/x\")\n\n[Tool result]: ok"
        );
    }

    #[test]
    fn serialize_conversation_truncates_tool_results() {
        let out = serialize_conversation(&[json!({"role": "toolResult", "content": "y".repeat(2010)})]);
        assert!(out.contains("[... 10 more characters truncated]"));
    }

    #[test]
    fn serialize_conversation_skips_empty_parts() {
        // No text blocks anywhere: empty output.
        assert_eq!(
            serialize_conversation(&[json!({"role": "assistant", "content": [{"type": "thinking", "thinking": "x"}]})]),
            ""
        );
    }
}
