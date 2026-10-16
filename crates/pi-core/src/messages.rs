//! Custom message types and transformers for the coding agent.
//! Port of `src/core/messages.ts`.
//!
//! Extends the base agent message set with coding-agent specific message
//! types, and converts them to LLM-compatible messages. Base-role messages
//! (system/user/assistant/toolResult, from `pi-ai`) are carried as parsed
//! JSON and passed through unchanged.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";

pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";

pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

// ============================================================================
// Content types (pi-ai TextContent / ImageContent)
// ============================================================================

/// `TextContent` / `ImageContent` from pi-ai.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextOrImage {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(rename = "textSignature", skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename = "image")]
    Image {
        /// Base64 encoded image data.
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// `string | (TextContent | ImageContent)[]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomContent {
    Text(String),
    Blocks(Vec<TextOrImage>),
}

// ============================================================================
// Message types
// ============================================================================

/// Message type for bash executions via the `!` command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    pub timestamp: Option<i64>,
    /// If true, this message is excluded from LLM context (`!!` prefix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
}

/// Message type for extension-injected messages via `sendMessage()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub custom_type: String,
    pub content: CustomContent,
    pub display: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    /// `string | null` — serialized as null, not omitted.
    pub from_id: Option<String>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: u64,
    pub timestamp: Option<i64>,
}

/// An agent message. Base roles are passed through `convert_to_llm`
/// unchanged (as parsed JSON); the coding-agent custom roles are typed.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentMessage {
    /// system / user / assistant / toolResult.
    Base(Value),
    BashExecution(BashExecutionMessage),
    Custom(CustomMessage),
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
}

fn tagged_value(role: &str, message: &impl Serialize) -> Value {
    let inner = serde_json::to_value(message).expect("message serializes");
    let mut map = serde_json::Map::new();
    map.insert("role".to_string(), Value::String(role.to_string()));
    if let Value::Object(fields) = inner {
        for (key, value) in fields {
            map.insert(key, value);
        }
    }
    Value::Object(map)
}

impl Serialize for AgentMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AgentMessage::Base(value) => value.serialize(serializer),
            AgentMessage::BashExecution(m) => tagged_value("bashExecution", m).serialize(serializer),
            AgentMessage::Custom(m) => tagged_value("custom", m).serialize(serializer),
            AgentMessage::BranchSummary(m) => tagged_value("branchSummary", m).serialize(serializer),
            AgentMessage::CompactionSummary(m) => tagged_value("compactionSummary", m).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AgentMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let role = value.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            "bashExecution" => serde_json::from_value(value)
                .map(AgentMessage::BashExecution)
                .map_err(serde::de::Error::custom),
            "custom" => serde_json::from_value(value).map(AgentMessage::Custom).map_err(serde::de::Error::custom),
            "branchSummary" => serde_json::from_value(value)
                .map(AgentMessage::BranchSummary)
                .map_err(serde::de::Error::custom),
            "compactionSummary" => serde_json::from_value(value)
                .map(AgentMessage::CompactionSummary)
                .map_err(serde::de::Error::custom),
            _ => Ok(AgentMessage::Base(value)),
        }
    }
}

// ============================================================================
// Transformers
// ============================================================================

/// Convert a `BashExecutionMessage` to user message text for LLM context.
pub fn bash_execution_to_text(msg: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", msg.command);
    if !msg.output.is_empty() {
        text.push_str(&format!("```\n{}\n```", msg.output));
    } else {
        text.push_str("(no output)");
    }
    if msg.cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(code) = msg.exit_code {
        if code != 0 {
            text.push_str(&format!("\n\nCommand exited with code {}", code));
        }
    }
    if msg.truncated {
        if let Some(path) = &msg.full_output_path {
            text.push_str(&format!("\n\n[Output truncated. Full output: {}]", path));
        }
    }
    text
}

/// Transform AgentMessages (including custom types) to LLM-compatible
/// messages. Used by the agent loop's `transformToLlm`, compaction
/// summarization, and extensions.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::BashExecution(msg) => {
                // Skip messages excluded from context (!! prefix).
                if msg.exclude_from_context == Some(true) {
                    return None;
                }
                Some(json!({
                    "role": "user",
                    "content": [{ "type": "text", "text": bash_execution_to_text(msg) }],
                    "timestamp": msg.timestamp,
                }))
            }
            AgentMessage::Custom(msg) => {
                let content = match &msg.content {
                    CustomContent::Text(s) => json!([{ "type": "text", "text": s }]),
                    CustomContent::Blocks(blocks) => serde_json::to_value(blocks).expect("content serializes"),
                };
                Some(json!({
                    "role": "user",
                    "content": content,
                    "timestamp": msg.timestamp,
                }))
            }
            AgentMessage::BranchSummary(msg) => Some(json!({
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": format!("{}{}{}", BRANCH_SUMMARY_PREFIX, msg.summary, BRANCH_SUMMARY_SUFFIX),
                }],
                "timestamp": msg.timestamp,
            })),
            AgentMessage::CompactionSummary(msg) => Some(json!({
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": format!("{}{}{}", COMPACTION_SUMMARY_PREFIX, msg.summary, COMPACTION_SUMMARY_SUFFIX),
                }],
                "timestamp": msg.timestamp,
            })),
            AgentMessage::Base(value) => Some(value.clone()),
        })
        .collect()
}

// ============================================================================
// Factories
// ============================================================================

/// `new Date(timestamp).getTime()` for the ISO-8601 strings used in session
/// files: `YYYY-MM-DDTHH:MM:SS(.sss)?(Z|±HH:MM)`. Returns Unix milliseconds.
pub fn parse_date_millis(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(range)?;
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut millis = num(20..23).unwrap_or(0);

    // Offset.
    let tail = &s[19..];
    let offset_secs: i64;
    if let Some(after_dot) = tail.strip_prefix('.') {
        // Fraction of 1+ digits; Node truncates to milliseconds
        // (.5 → 500, .1234 → 123).
        let frac_digits: usize = after_dot.bytes().take_while(|b| b.is_ascii_digit()).count();
        if frac_digits == 0 {
            return None;
        }
        let mut padded: String = after_dot[..frac_digits].to_string();
        while padded.len() < 3 {
            padded.push('0');
        }
        millis = padded[..3].parse().ok()?;
        offset_secs = parse_offset(&after_dot[frac_digits..])?;
    } else {
        offset_secs = parse_offset(tail)?;
    }

    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_secs;
    Some(secs * 1000 + millis)
}

fn parse_offset(tail: &str) -> Option<i64> {
    if tail == "Z" {
        return Some(0);
    }
    let bytes = tail.as_bytes();
    if bytes.len() == 6 && (bytes[0] == b'+' || bytes[0] == b'-') && bytes[3] == b':' {
        let sign = if bytes[0] == b'+' { 1 } else { -1 };
        let h: i64 = tail[1..3].parse().ok()?;
        let m: i64 = tail[4..6].parse().ok()?;
        return Some(sign * (h * 3600 + m * 60));
    }
    None
}

/// `createBranchSummaryMessage`.
pub fn create_branch_summary_message(
    summary: &str,
    from_id: Option<&str>,
    timestamp: &str,
) -> BranchSummaryMessage {
    BranchSummaryMessage {
        summary: summary.to_string(),
        from_id: from_id.map(str::to_string),
        timestamp: parse_date_millis(timestamp),
    }
}

/// `createCompactionSummaryMessage`.
pub fn create_compaction_summary_message(
    summary: &str,
    tokens_before: u64,
    timestamp: &str,
) -> CompactionSummaryMessage {
    CompactionSummaryMessage {
        summary: summary.to_string(),
        tokens_before,
        timestamp: parse_date_millis(timestamp),
    }
}

/// `createCustomMessage`.
pub fn create_custom_message(
    custom_type: &str,
    content: CustomContent,
    display: bool,
    details: Option<Value>,
    timestamp: &str,
) -> CustomMessage {
    CustomMessage {
        custom_type: custom_type.to_string(),
        content,
        display,
        details,
        timestamp: parse_date_millis(timestamp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_execution_to_text_basic() {
        let msg = BashExecutionMessage {
            command: "ls -la".to_string(),
            output: "total 0".to_string(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: Some(1000),
            exclude_from_context: None,
        };
        assert_eq!(bash_execution_to_text(&msg), "Ran `ls -la`\n```\ntotal 0\n```");
    }

    #[test]
    fn bash_execution_to_text_no_output() {
        let msg = BashExecutionMessage {
            command: "true".to_string(),
            output: String::new(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: Some(1000),
            exclude_from_context: None,
        };
        assert_eq!(bash_execution_to_text(&msg), "Ran `true`\n(no output)");
    }

    #[test]
    fn bash_execution_to_text_statuses() {
        let base = |output: &str, exit_code: Option<i32>, cancelled: bool| BashExecutionMessage {
            command: "cmd".to_string(),
            output: output.to_string(),
            exit_code,
            cancelled,
            truncated: false,
            full_output_path: None,
            timestamp: Some(0),
            exclude_from_context: None,
        };
        assert_eq!(
            bash_execution_to_text(&base("", Some(0), true)),
            "Ran `cmd`\n(no output)\n\n(command cancelled)"
        );
        assert_eq!(
            bash_execution_to_text(&base("", Some(3), false)),
            "Ran `cmd`\n(no output)\n\nCommand exited with code 3"
        );
        // cancelled wins over a non-zero exit code.
        assert_eq!(
            bash_execution_to_text(&base("", Some(3), true)),
            "Ran `cmd`\n(no output)\n\n(command cancelled)"
        );
        // null exit code prints nothing.
        assert_eq!(bash_execution_to_text(&base("", None, false)), "Ran `cmd`\n(no output)");
        let truncated = BashExecutionMessage {
            truncated: true,
            full_output_path: Some("/tmp/pi-bash-abc.log".to_string()),
            ..base("x", Some(0), false)
        };
        assert_eq!(
            bash_execution_to_text(&truncated),
            "Ran `cmd`\n```\nx\n```\n\n[Output truncated. Full output: /tmp/pi-bash-abc.log]"
        );
    }

    #[test]
    fn convert_to_llm_maps_and_filters() {
        let bash = AgentMessage::BashExecution(BashExecutionMessage {
            command: "echo hi".to_string(),
            output: "hi\n".to_string(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: Some(42),
            exclude_from_context: None,
        });
        let excluded = AgentMessage::BashExecution(BashExecutionMessage {
            exclude_from_context: Some(true),
            ..match &bash {
                AgentMessage::BashExecution(m) => m.clone(),
                _ => unreachable!(),
            }
        });
        let custom = AgentMessage::Custom(CustomMessage {
            custom_type: "myExtension".to_string(),
            content: CustomContent::Text("hello".to_string()),
            display: true,
            details: None,
            timestamp: Some(43),
        });
        let custom_blocks = AgentMessage::Custom(CustomMessage {
            custom_type: "img".to_string(),
            content: CustomContent::Blocks(vec![TextOrImage::Image {
                data: "abcd".to_string(),
                mime_type: "image/png".to_string(),
            }]),
            display: false,
            details: None,
            timestamp: Some(44),
        });
        let branch = AgentMessage::BranchSummary(BranchSummaryMessage {
            summary: "did things".to_string(),
            from_id: Some("id-1".to_string()),
            timestamp: Some(45),
        });
        let compaction = AgentMessage::CompactionSummary(CompactionSummaryMessage {
            summary: "compacted".to_string(),
            tokens_before: 1234,
            timestamp: Some(46),
        });
        let base = AgentMessage::Base(json!({"role": "user", "content": "plain", "timestamp": 47}));

        let out = convert_to_llm(&[bash, excluded, custom, custom_blocks, branch, compaction, base]);
        assert_eq!(out.len(), 6); // excluded dropped
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"][0]["text"], "Ran `echo hi`\n```\nhi\n\n```");
        assert_eq!(out[0]["timestamp"], 42);
        assert_eq!(out[1]["content"][0]["text"], "hello");
        assert_eq!(out[2]["content"][0]["type"], "image");
        assert_eq!(out[2]["content"][0]["mimeType"], "image/png");
        assert_eq!(out[3]["content"][0]["text"], format!("{}did things{}", BRANCH_SUMMARY_PREFIX, BRANCH_SUMMARY_SUFFIX));
        assert_eq!(
            out[4]["content"][0]["text"],
            format!("{}compacted{}", COMPACTION_SUMMARY_PREFIX, COMPACTION_SUMMARY_SUFFIX)
        );
        assert_eq!(out[5], json!({"role": "user", "content": "plain", "timestamp": 47}));
    }

    #[test]
    fn message_roundtrip() {
        let msgs = vec![
            AgentMessage::BashExecution(BashExecutionMessage {
                command: "c".to_string(),
                output: "o".to_string(),
                exit_code: None,
                cancelled: false,
                truncated: false,
                full_output_path: None,
                timestamp: Some(1),
                exclude_from_context: None,
            }),
            AgentMessage::Base(json!({"role": "assistant", "content": [], "usage": {}})),
        ];
        let serialized = serde_json::to_string(&msgs).unwrap();
        let back: Vec<AgentMessage> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(back, msgs);
        // Typed role round-trips through the tagged form.
        let value: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(value[0]["role"], "bashExecution");
        assert_eq!(value[1]["role"], "assistant");
    }

    #[test]
    fn date_parsing() {
        // Expected values verified against Node's Date parser.
        assert_eq!(parse_date_millis("2026-01-02T03:04:05.123Z"), Some(1_767_323_045_123));
        assert_eq!(parse_date_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_date_millis("2026-01-02T03:04:05Z"), Some(1_767_323_045_000));
        // Offsets.
        assert_eq!(parse_date_millis("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_date_millis("1969-12-31T23:00:00-01:00"), Some(0));
        assert_eq!(parse_date_millis("not a date"), None);
        assert_eq!(parse_date_millis("2026-13-02T03:04:05Z"), None);
        // Cross-check against a known value: 2024-06-15T12:30:45.5Z.
        assert_eq!(parse_date_millis("2024-06-15T12:30:45.5Z"), Some(1_718_454_645_500));
    }

    #[test]
    fn factories() {
        let m = create_branch_summary_message("s", Some("id"), "2026-01-02T03:04:05Z");
        assert_eq!(m.from_id, Some("id".to_string()));
        assert_eq!(m.timestamp, Some(1_767_323_045_000));
        // null fromId passes through (JSON null), invalid date -> None
        // (JSON.stringify(new Date("bogus").getTime()) is null too).
        let m = create_branch_summary_message("s", None, "bogus");
        assert_eq!(m.from_id, None);
        assert_eq!(m.timestamp, None);
        let m = create_compaction_summary_message("s", 99, "2026-01-02T03:04:05Z");
        assert_eq!(m.tokens_before, 99);
        assert_eq!(m.timestamp, Some(1_767_323_045_000));
        let m = create_custom_message(
            "t",
            CustomContent::Text("x".to_string()),
            true,
            None,
            "2026-01-02T03:04:05Z",
        );
        assert!(m.display);
        // Pre-epoch dates are negative milliseconds, like getTime().
        let m = create_compaction_summary_message("s", 1, "1969-12-31T23:59:59Z");
        assert_eq!(m.timestamp, Some(-1000));
    }
}
