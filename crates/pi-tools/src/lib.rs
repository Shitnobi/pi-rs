//! pi-tools: built-in tools of the pi coding agent.
//!
//! Rust port of `src/core/tools/{read,write,edit,bash,grep,find,ls}.ts`.
//! Each tool mirrors the original's observable behavior: output text, error
//! messages, and details payloads. Parity is enforced by the harness in
//! `parity/` (see PARITY.md).

pub mod bash;
pub mod edit;
pub mod find;
pub mod grep;
pub mod image;
pub mod ls;
pub mod node_fs;
pub mod read;
pub mod write;

use serde::Serialize;

/// A model-facing content block.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Result of a successful tool execution: content blocks + details payload.
#[derive(Debug, Clone, Serialize)]
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(rename = "isError")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// Programmatic-caller payload (bash `structuredContent`).
    #[serde(rename = "structuredContent")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        ToolOutput {
            content: vec![ContentBlock::Text { text: text.into() }],
            details: None,
            is_error: None,
            structured_content: None,
        }
    }

    pub fn text_with_details(text: impl Into<String>, details: serde_json::Value) -> Self {
        ToolOutput {
            content: vec![ContentBlock::Text { text: text.into() }],
            details: Some(details),
            is_error: None,
            structured_content: None,
        }
    }

    pub fn text_str(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Tool execution error; `message` mirrors the original's Error message.
#[derive(Debug, Clone)]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolError { message: message.into() }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

pub type ToolResult = Result<ToolOutput, ToolError>;
