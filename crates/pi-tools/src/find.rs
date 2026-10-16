//! The `find` tool. Port of `src/core/tools/find.ts`.
//!
//! Like the original, this shells out to `fd` with identical arguments.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use serde::Deserialize;

use pi_core::paths::resolve_to_cwd;
use pi_core::truncate::{format_size, truncate_head, DEFAULT_MAX_BYTES};

use crate::grep::ensure_tool;
use crate::{ToolError, ToolOutput, ToolResult};

const DEFAULT_LIMIT: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct FindToolInput {
    /// Glob pattern to match files, e.g. '*.ts', '**/*.json'
    pub pattern: String,
    /// Directory to search in (default: current directory)
    pub path: Option<String>,
    /// Maximum number of results (default: 1000)
    pub limit: Option<f64>,
}

/// Port of `relativizeFindResultPath`.
pub fn relativize_find_result_path(result_path: &str, search_path: &str) -> String {
    let had_trailing_separator = result_path.ends_with('/');
    let relative_path = if result_path.starts_with('/') {
        node_relative(search_path, result_path)
    } else {
        result_path.to_string()
    };
    let posix_path = relative_path.replace(std::path::MAIN_SEPARATOR, "/");
    if had_trailing_separator && !posix_path.ends_with('/') {
        format!("{}/", posix_path)
    } else {
        posix_path
    }
}

/// Node `path.relative(from, to)` for unix.
fn node_relative(from: &str, to: &str) -> String {
    let resolve = |p: &str| -> Vec<String> {
        let mut stack: Vec<String> = Vec::new();
        for seg in p.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    stack.pop();
                }
                s => stack.push(s.to_string()),
            }
        }
        stack
    };
    let from_segs = resolve(from);
    let to_segs = resolve(to);
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
    parts.join("/")
}

fn path_exists(p: &str) -> bool {
    std::path::Path::new(p).exists()
}

pub fn find_execute(cwd: &str, input: &FindToolInput) -> ToolResult {
    let search_path = resolve_to_cwd(input.path.as_deref().unwrap_or("."), cwd);
    let effective_limit = input.limit.map(|l| l as usize).unwrap_or(DEFAULT_LIMIT);

    let fd_path = match ensure_tool("fd") {
        Some(p) => p,
        None => return Err(ToolError::new("fd is not available and could not be downloaded")),
    };

    let mut args: Vec<String> = vec!["--glob".to_string(), "--color=never".to_string(), "--hidden".to_string()];

    // fd normally ignores .gitignore outside git repos; inside repos use fd's
    // default git-aware behavior.
    let mut inside_git_repo = false;
    let mut current = search_path.clone();
    loop {
        if path_exists(&format!("{}/.git", current)) {
            inside_git_repo = true;
            break;
        }
        let parent = crate::write::parent_dir(&current).to_string();
        if parent == current {
            break;
        }
        current = parent;
    }
    if !inside_git_repo {
        args.push("--no-require-git".to_string());
    }
    args.push("--max-results".to_string());
    args.push(effective_limit.to_string());

    // fd --glob matches against the basename unless --full-path is set.
    let mut effective_pattern = input.pattern.clone();
    if input.pattern.contains('/') {
        args.push("--full-path".to_string());
        if !input.pattern.starts_with('/') && !input.pattern.starts_with("**/") && input.pattern != "**" {
            effective_pattern = format!("**/{}", input.pattern);
        }
    }
    args.push("--".to_string());
    args.push(effective_pattern);
    args.push(search_path.clone());

    let mut child = match Command::new(&fd_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => return Err(ToolError::new(format!("Failed to run fd: {}", err))),
    };

    let stderr_handle = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
    });

    let mut lines: Vec<String> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(l) => lines.push(l),
                Err(_) => break,
            }
        }
    }

    let status = match child.wait() {
        Ok(s) => s,
        Err(err) => return Err(ToolError::new(format!("Failed to run fd: {}", err))),
    };
    let output = lines.join("\n");
    if status.code() != Some(0) {
        let stderr = stderr_handle.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        let error_msg = if stderr.trim().is_empty() {
            format!("fd exited with code {}", status.code().map(|c| c.to_string()).unwrap_or("null".into()))
        } else {
            stderr.trim().to_string()
        };
        if output.is_empty() {
            return Err(ToolError::new(error_msg));
        }
    }

    if output.is_empty() {
        return Ok(ToolOutput::text("No files found matching pattern"));
    }

    let mut relativized: Vec<String> = Vec::new();
    for raw_line in &lines {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        relativized.push(relativize_find_result_path(&line, &search_path));
    }

    let result_limit_reached = relativized.len() >= effective_limit;
    let raw_output = relativized.join("\n");
    let truncation = truncate_head(
        &raw_output,
        pi_core::truncate::TruncationOptions { max_lines: Some(u64::MAX), ..Default::default() },
    );
    let mut result_output = truncation.content.clone();
    let mut details = serde_json::Map::new();
    let mut notices: Vec<String> = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "{} results limit reached. Use limit={} for more, or refine pattern",
            effective_limit,
            effective_limit * 2
        ));
        details.insert("resultLimitReached".to_string(), serde_json::json!(effective_limit));
    }
    if truncation.truncated {
        notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
        details.insert("truncation".to_string(), serde_json::to_value(&truncation).unwrap());
    }
    if !notices.is_empty() {
        result_output.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }

    let details = if details.is_empty() { None } else { Some(serde_json::Value::Object(details)) };
    Ok(ToolOutput {
        content: vec![crate::ContentBlock::Text { text: result_output }],
        details,
        is_error: None,
        structured_content: None,
    })
}


