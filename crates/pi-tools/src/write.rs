//! The `write` tool. Port of `src/core/tools/write.ts`.

use serde::Deserialize;

use pi_core::paths::resolve_to_cwd;

use crate::node_fs;
use crate::{ToolError, ToolOutput, ToolResult};

#[derive(Debug, Deserialize)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

pub fn write_execute(cwd: &str, input: &WriteToolInput) -> ToolResult {
    let absolute_path = resolve_to_cwd(&input.path, cwd);
    let dir = parent_dir(&absolute_path);

    // Create parent directories if needed (mkdir recursive).
    if !dir.is_empty() {
        if let Err(message) = node_fs::node_mkdir_recursive(dir) {
            return Err(ToolError::new(message));
        }
    }

    // Write the file contents.
    if let Err(message) = node_fs::node_write_file(&absolute_path, &input.content) {
        return Err(ToolError::new(message));
    }

    Ok(ToolOutput::text(format!("Successfully wrote to {}", input.path)))
}

/// Node `path.dirname` for unix paths.
pub fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        None => ".",
        Some(0) => "/",
        Some(idx) => &path[..idx],
    }
}
