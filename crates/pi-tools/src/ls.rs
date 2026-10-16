//! The `ls` tool. Port of `src/core/tools/ls.ts`.

use serde::Deserialize;

use pi_core::paths::resolve_to_cwd;
use pi_core::truncate::{format_size, truncate_head, DEFAULT_MAX_BYTES};

use crate::node_fs;
use crate::{ToolError, ToolOutput, ToolResult};

const DEFAULT_LIMIT: usize = 500;

#[derive(Debug, Deserialize)]
pub struct LsToolInput {
    /// Directory to list (default: current directory)
    pub path: Option<String>,
    /// Maximum number of entries to return (default: 500)
    pub limit: Option<f64>,
}

pub fn ls_execute(cwd: &str, input: &LsToolInput) -> ToolResult {
    let dir_path = resolve_to_cwd(input.path.as_deref().unwrap_or("."), cwd);
    let effective_limit = input.limit.map(|l| l.max(0.0) as usize).unwrap_or(DEFAULT_LIMIT);

    // Check if path exists.
    if !std::path::Path::new(&dir_path).exists() {
        return Err(ToolError::new(format!("Path not found: {}", dir_path)));
    }

    // Check if path is a directory.
    match node_fs::node_stat_is_dir(&dir_path) {
        Ok(true) => {}
        Ok(false) => return Err(ToolError::new(format!("Not a directory: {}", dir_path))),
        Err(message) => return Err(ToolError::new(message)),
    }

    // Read directory entries.
    let entries = match node_fs::node_readdir(&dir_path) {
        Ok(e) => e,
        Err(message) => return Err(ToolError::new(format!("Cannot read directory: {}", message))),
    };

    // Sort alphabetically, case-insensitive.
    let mut sorted = entries;
    sorted.sort_by_key(|a| a.to_lowercase());

    // Format entries with directory indicators.
    let mut results: Vec<String> = Vec::new();
    let mut entry_limit_reached = false;
    for entry in &sorted {
        if results.len() >= effective_limit {
            entry_limit_reached = true;
            break;
        }

        let full_path = join_path(&dir_path, entry);
        let is_dir = match node_fs::node_stat_is_dir(&full_path) {
            Ok(d) => d,
            Err(_) => {
                // Skip entries we cannot stat.
                continue;
            }
        };
        results.push(if is_dir { format!("{}/", entry) } else { entry.clone() });
    }

    if results.is_empty() {
        return Ok(ToolOutput::text("(empty directory)"));
    }

    let raw_output = results.join("\n");
    // Apply byte truncation. No separate line limit (entry count is capped).
    let truncation = truncate_head(
        &raw_output,
        pi_core::truncate::TruncationOptions {
            max_lines: Some(u64::MAX),
            ..Default::default()
        },
    );
    let mut output = truncation.content.clone();
    let mut details = serde_json::Map::new();
    let mut notices: Vec<String> = Vec::new();
    if entry_limit_reached {
        notices.push(format!(
            "{} entries limit reached. Use limit={} for more",
            effective_limit,
            effective_limit * 2
        ));
        details.insert("entryLimitReached".to_string(), serde_json::json!(effective_limit));
    }
    if truncation.truncated {
        notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
        details.insert("truncation".to_string(), serde_json::to_value(&truncation).unwrap());
    }
    if !notices.is_empty() {
        output.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }

    let details = if details.is_empty() { None } else { Some(serde_json::Value::Object(details)) };
    Ok(ToolOutput { content: vec![crate::ContentBlock::Text { text: output }], details, is_error: None, structured_content: None })
}

/// Node `path.join(dir, name)` for unix.
fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{}{}", dir, name)
    } else {
        format!("{}/{}", dir, name)
    }
}
