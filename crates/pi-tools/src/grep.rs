//! The `grep` tool. Port of `src/core/tools/grep.ts`.
//!
//! Like the original, this shells out to `ripgrep (rg)` with identical
//! arguments, so search behavior (gitignore handling, regex dialect) matches
//! the original exactly as long as the same rg binary is used.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use serde::Deserialize;

use pi_core::paths::resolve_to_cwd;
use pi_core::truncate::{
    format_size, truncate_head, truncate_line, DEFAULT_MAX_BYTES, GREP_MAX_LINE_LENGTH,
};

use crate::{ToolError, ToolOutput, ToolResult};

const DEFAULT_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct GrepToolInput {
    pub pattern: String,
    /// Directory or file to search (default: current directory)
    pub path: Option<String>,
    /// Filter files by glob pattern
    pub glob: Option<String>,
    /// Case-insensitive search (default: false)
    #[serde(rename = "ignoreCase")]
    pub ignore_case: Option<bool>,
    /// Treat pattern as literal string instead of regex (default: false)
    pub literal: Option<bool>,
    /// Number of context lines before and after each match (default: 0)
    pub context: Option<f64>,
    /// Maximum number of matches to return (default: 100)
    pub limit: Option<f64>,
}

/// Locate the rg binary the way pi's tools-manager does (PATH first; the
/// pi-managed bin dir `~/.pi/agent/bin` second). No auto-download in Rust.
pub fn ensure_tool(tool: &str) -> Option<String> {
    // PATH lookup
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = format!("{}/{}", dir, tool);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    // pi bin dir
    if let Ok(home) = std::env::var("HOME") {
        let candidate = format!("{}/.pi/agent/bin/{}", home, tool);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn grep_execute(cwd: &str, input: &GrepToolInput) -> ToolResult {
    let rg_path = match ensure_tool("rg") {
        Some(p) => p,
        None => {
            return Err(ToolError::new(
                "ripgrep (rg) is not available and could not be downloaded",
            ))
        }
    };

    let search_path = resolve_to_cwd(input.path.as_deref().unwrap_or("."), cwd);

    let is_directory = match std::fs::metadata(&search_path) {
        Ok(md) => md.is_dir(),
        Err(_) => return Err(ToolError::new(format!("Path not found: {}", search_path))),
    };

    let context_value = input
        .context
        .filter(|c| *c > 0.0)
        .map(|c| c as usize)
        .unwrap_or(0);
    let effective_limit = std::cmp::max(1, input.limit.map(|l| l as usize).unwrap_or(DEFAULT_LIMIT));

    let format_path = |file_path: &str| -> String {
        if is_directory {
            if let Some(relative) = path_relative(&search_path, file_path) {
                if !relative.is_empty() && !relative.starts_with("..") {
                    return relative.replace('\\', "/");
                }
            }
        }
        basename(file_path)
    };

    let mut args: Vec<String> = vec![
        "--json".to_string(),
        "--line-number".to_string(),
        "--color=never".to_string(),
        "--hidden".to_string(),
    ];
    if input.ignore_case == Some(true) {
        args.push("--ignore-case".to_string());
    }
    if input.literal == Some(true) {
        args.push("--fixed-strings".to_string());
    }
    if let Some(glob) = &input.glob {
        args.push("--glob".to_string());
        args.push(glob.clone());
    }
    args.push("--".to_string());
    args.push(input.pattern.clone());
    args.push(search_path.clone());

    let mut child = match Command::new(&rg_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => return Err(ToolError::new(format!("Failed to run ripgrep: {}", err))),
    };

    // Drain stderr on a thread to avoid pipe-buffer deadlocks.
    let stderr_handle = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
    });

    let mut matches: Vec<(String, i64, Option<String>)> = Vec::new();
    let mut match_count = 0usize;
    let mut killed_due_to_limit = false;
    let mut match_limit_reached = false;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() || match_count >= effective_limit {
                // Drain remaining output without recording more matches.
                if match_count >= effective_limit {
                    continue;
                }
                continue;
            }
            let event: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if event.get("type").and_then(|t| t.as_str()) == Some("match") {
                match_count += 1;
                let data = &event["data"];
                let file_path = data["path"]["text"].as_str().map(|s| s.to_string());
                let line_number = data["line_number"].as_i64();
                let line_text = data["lines"]["text"].as_str().map(|s| s.to_string());
                if let (Some(fp), Some(ln)) = (file_path, line_number) {
                    matches.push((fp, ln, line_text));
                }
                if match_count >= effective_limit {
                    match_limit_reached = true;
                    killed_due_to_limit = true;
                    // Mirror the original: kill the child once the limit is hit.
                    let _ = child.kill();
                }
            }
        }
    }

    let output = match child.wait() {
        Ok(status) => status,
        Err(err) => return Err(ToolError::new(format!("Failed to run ripgrep: {}", err))),
    };

    let code = output.code();
    if !killed_due_to_limit && code != Some(0) && code != Some(1) {
        let stderr = stderr_handle
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default()
            .trim()
            .to_string();
        let error_msg = if stderr.is_empty() {
            format!("ripgrep exited with code {}", code.map(|c| c.to_string()).unwrap_or("null".into()))
        } else {
            stderr
        };
        return Err(ToolError::new(error_msg));
    }

    if match_count == 0 {
        return Ok(ToolOutput::text("No matches found"));
    }

    // Format matches after streaming finishes.
    let mut output_lines: Vec<String> = Vec::new();
    let mut lines_truncated = false;
    let mut file_cache: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    let get_file_lines = |file_path: &str,
                          cache: &mut std::collections::HashMap<String, Vec<String>>|
     -> Vec<String> {
        cache
            .entry(file_path.to_string())
            .or_insert_with(|| {
                match std::fs::read_to_string(file_path) {
                    Ok(content) => content
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .split('\n')
                        .map(|s| s.to_string())
                        .collect(),
                    Err(_) => Vec::new(),
                }
            })
            .clone()
    };

    for (file_path, line_number, line_text) in &matches {
        if context_value == 0 && line_text.is_some() {
            let relative_path = format_path(file_path);
            let sanitized = line_text.as_ref().unwrap();
            let sanitized = sanitized
                .replace("\r\n", "\n")
                .replace('\r', "");
            // JS: .replace(/\n$/, "") — strips exactly one trailing newline.
            let sanitized = sanitized.strip_suffix('\n').unwrap_or(&sanitized).to_string();
            let (truncated_text, was_truncated) = truncate_line(&sanitized, GREP_MAX_LINE_LENGTH);
            if was_truncated {
                lines_truncated = true;
            }
            output_lines.push(format!("{}:{}: {}", relative_path, line_number, truncated_text));
        } else {
            // Context block
            let relative_path = format_path(file_path);
            let lines = get_file_lines(file_path, &mut file_cache);
            if lines.is_empty() {
                output_lines.push(format!("{}:{}: (unable to read file)", relative_path, line_number));
                continue;
            }
            let start = if context_value > 0 {
                std::cmp::max(1, line_number - context_value as i64)
            } else {
                *line_number
            };
            let end = if context_value > 0 {
                std::cmp::min(lines.len() as i64, line_number + context_value as i64)
            } else {
                *line_number
            };
            let mut current = start;
            while current <= end {
                let line_text = lines.get((current - 1) as usize).cloned().unwrap_or_default();
                let sanitized = line_text.replace('\r', "");
                let is_match_line = current == *line_number;
                let (truncated_text, was_truncated) = truncate_line(&sanitized, GREP_MAX_LINE_LENGTH);
                if was_truncated {
                    lines_truncated = true;
                }
                if is_match_line {
                    output_lines.push(format!("{}:{}: {}", relative_path, current, truncated_text));
                } else {
                    output_lines.push(format!("{}-{}- {}", relative_path, current, truncated_text));
                }
                current += 1;
            }
        }
    }

    let raw_output = output_lines.join("\n");
    // Apply byte truncation (no line limit; match limit already capped rows).
    let truncation = truncate_head(
        &raw_output,
        pi_core::truncate::TruncationOptions { max_lines: Some(u64::MAX), ..Default::default() },
    );
    let mut out = truncation.content.clone();
    let mut details = serde_json::Map::new();
    let mut notices: Vec<String> = Vec::new();
    if match_limit_reached {
        notices.push(format!(
            "{} matches limit reached. Use limit={} for more, or refine pattern",
            effective_limit,
            effective_limit * 2
        ));
        details.insert("matchLimitReached".to_string(), serde_json::json!(effective_limit));
    }
    if truncation.truncated {
        notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
        details.insert("truncation".to_string(), serde_json::to_value(&truncation).unwrap());
    }
    if lines_truncated {
        notices.push(format!(
            "Some lines truncated to {} chars. Use read tool to see full lines",
            GREP_MAX_LINE_LENGTH
        ));
        details.insert("linesTruncated".to_string(), serde_json::json!(true));
    }
    if !notices.is_empty() {
        out.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }

    let details = if details.is_empty() { None } else { Some(serde_json::Value::Object(details)) };
    Ok(ToolOutput { content: vec![crate::ContentBlock::Text { text: out }], details, is_error: None, structured_content: None })
}

fn basename(path: &str) -> String {
    match path.rfind('/') {
        Some(idx) => path[idx + 1..].to_string(),
        None => path.to_string(),
    }
}

/// Node `path.relative(from, to)` (unix, absolute inputs).
fn path_relative(from: &str, to: &str) -> Option<String> {
    let norm = |p: &str| -> Vec<String> {
        p.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(|s| s.to_string())
            .collect()
    };
    let from_segs = norm(from);
    let to_segs = norm(to);
    let mut common = 0;
    while common < from_segs.len() && common < to_segs.len() && from_segs[common] == to_segs[common] {
        common += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_segs.len() {
        parts.push("..".to_string());
    }
    for seg in &to_segs[common..] {
        parts.push(seg.to_string());
    }
    Some(parts.join("/"))
}
