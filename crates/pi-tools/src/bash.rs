//! The `bash` tool. Port of `src/core/tools/bash.ts`
//! (`createShellToolDefinition` with the local shell backend).
//!
//! Command execution goes through `shell::LocalShellOperations`
//! (`createLocalShellOperations`) and output handling through
//! `output_accumulator::OutputAccumulator`, matching the original
//! architecture: stdout and stderr are combined in arrival order, streamed
//! through a WHATWG UTF-8 decoder, and truncated from the tail with the same
//! limits and messages as the original.

use std::collections::HashMap;
use std::time::Instant;

use serde::Deserialize;

use pi_core::truncate::{format_size, DEFAULT_MAX_BYTES};

use crate::output_accumulator::{OutputAccumulator, OutputAccumulatorOptions, OutputSnapshot};
use crate::shell::{
    create_local_bash_operations, format_timeout_secs, BashOperations, ShellExecError, ShellExecOptions, StreamKind,
};
use crate::{ToolError, ToolOutput, ToolResult};

/// Output limit of `structuredContent.output` in the original.
const STRUCTURED_OUTPUT_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct BashToolInput {
    pub command: String,
    /// Timeout in seconds (optional, no default timeout)
    pub timeout: Option<f64>,
}

/// `resolveSpawnContext` without a session (harness mode):
/// `getShellEnv` with the PI_* session variables removed.
fn resolve_spawn_env() -> HashMap<String, String> {
    let mut env = crate::shell::get_shell_env();
    for key in ["PI_SESSION_ID", "PI_SESSION_FILE", "PI_PROVIDER", "PI_MODEL", "PI_REASONING_LEVEL"] {
        env.remove(key);
    }
    env
}

fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{}\n\n{}", text, status)
    }
}

/// `finishOutput`: stop accepting output, take the final snapshot, close the
/// spill file.
fn finish_output(output: &mut OutputAccumulator) -> Result<OutputSnapshot, ToolError> {
    output.finish().map_err(ToolError::new)?;
    let snapshot = output.snapshot(true).map_err(ToolError::new)?;
    output.close_temp_file().map_err(ToolError::new)?;
    Ok(snapshot)
}

/// `Math.round((performance.now() - startedAt) / 100) / 10`
fn wall_time_seconds(started_at: Instant) -> f64 {
    let ms = started_at.elapsed().as_secs_f64() * 1000.0;
    (ms / 100.0).round() / 10.0
}

struct FormattedOutput {
    text: String,
    details: Option<serde_json::Value>,
}

/// `formatOutput`: snapshot content plus the truncation notice.
fn format_output(snapshot: &OutputSnapshot, last_line_bytes: u64, empty_text: &str) -> FormattedOutput {
    let truncation = &snapshot.truncation;
    let mut text = if snapshot.content.is_empty() {
        empty_text.to_string()
    } else {
        snapshot.content.clone()
    };
    let mut details: Option<serde_json::Value> = None;
    if truncation.truncated {
        details = Some(serde_json::json!({
            "truncation": truncation,
            "fullOutputPath": snapshot.full_output_path,
        }));
        let start_line = truncation.total_lines - truncation.output_lines + 1;
        let end_line = truncation.total_lines;
        if truncation.last_line_partial {
            let last_line_size = format_size(last_line_bytes);
            text.push_str(&format!(
                "\n\n[Showing last {} of line {} (line is {}). Full output: {}]",
                format_size(truncation.output_bytes),
                end_line,
                last_line_size,
                snapshot.full_output_path.as_deref().unwrap_or("")
            ));
        } else if truncation.truncated_by.as_deref() == Some("lines") {
            text.push_str(&format!(
                "\n\n[Showing lines {}-{} of {}. Full output: {}]",
                start_line,
                end_line,
                truncation.total_lines,
                snapshot.full_output_path.as_deref().unwrap_or("")
            ));
        } else {
            text.push_str(&format!(
                "\n\n[Showing lines {}-{} of {} ({} limit). Full output: {}]",
                start_line,
                end_line,
                truncation.total_lines,
                format_size(DEFAULT_MAX_BYTES),
                snapshot.full_output_path.as_deref().unwrap_or("")
            ));
        }
    }
    FormattedOutput { text, details }
}

/// The original's `structuredContent` construction:
/// `output.readFullOutput(STRUCTURED_OUTPUT_MAX_BYTES)` + exit code + wall
/// time.
fn bash_structured_content(
    output: &OutputAccumulator,
    snapshot: &OutputSnapshot,
    exit_code: i32,
    started_at: Instant,
) -> Result<serde_json::Value, ToolError> {
    // Wall time is measured before reading the full output, like the original.
    let wall_time_seconds = wall_time_seconds(started_at);
    let full = output.read_full_output(STRUCTURED_OUTPUT_MAX_BYTES).map_err(ToolError::new)?;
    let mut obj = serde_json::json!({
        "output": full.content,
        "truncated": full.truncated,
        "exit_code": exit_code,
        "wall_time_seconds": wall_time_seconds,
    });
    if full.truncated {
        if let Some(path) = &snapshot.full_output_path {
            obj.as_object_mut()
                .unwrap()
                .insert("full_output_path".to_string(), serde_json::json!(path));
        }
    }
    Ok(obj)
}

pub fn bash_execute(cwd: &str, input: &BashToolInput) -> ToolResult {
    let ops = create_local_bash_operations(None);
    let mut output = OutputAccumulator::new(OutputAccumulatorOptions {
        temp_file_prefix: Some("pi-bash".to_string()),
        ..Default::default()
    });
    let started_at = Instant::now();
    let env = resolve_spawn_env();

    let exec_result = ops.exec(
        &input.command,
        cwd,
        ShellExecOptions {
            on_data: Some(&mut |_kind: StreamKind, data: &[u8]| {
                // Temp-file IO failures here would be an uncaught throw in
                // the original's data handler; they cannot occur in practice.
                let _ = output.append(data);
            }),
            signal: None,
            timeout: input.timeout,
            env: Some(&env),
        },
    );

    match exec_result {
        Err(ShellExecError::Aborted) => {
            let snapshot = finish_output(&mut output)?;
            let display = format_output(&snapshot, output.get_last_line_bytes(), "");
            Err(ToolError::new(append_status(&display.text, "Command aborted")))
        }
        Err(ShellExecError::Timeout(t)) => {
            let snapshot = finish_output(&mut output)?;
            let display = format_output(&snapshot, output.get_last_line_bytes(), "");
            Err(ToolError::new(append_status(
                &display.text,
                &format!("Command timed out after {} seconds", format_timeout_secs(t)),
            )))
        }
        Err(err) => Err(ToolError::new(err.message())),
        Ok(outcome) => {
            let snapshot = finish_output(&mut output)?;
            let last_line_bytes = output.get_last_line_bytes();
            let display = format_output(&snapshot, last_line_bytes, "(no output)");
            let Some(exit_code) = outcome.exit_code else {
                return Err(ToolError::new(append_status(
                    &display.text,
                    "Command terminated without an exit code",
                )));
            };
            let structured = bash_structured_content(&output, &snapshot, exit_code, started_at)?;
            if exit_code != 0 {
                return Ok(ToolOutput {
                    content: vec![crate::ContentBlock::Text {
                        text: append_status(&display.text, &format!("Command exited with code {}", exit_code)),
                    }],
                    details: display.details,
                    is_error: Some(true),
                    structured_content: Some(structured),
                });
            }
            Ok(ToolOutput {
                content: vec![crate::ContentBlock::Text { text: display.text }],
                details: display.details,
                is_error: None,
                structured_content: Some(structured),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &str) -> ToolResult {
        bash_execute(".", &BashToolInput { command: command.to_string(), timeout: None })
    }

    #[test]
    fn simple_echo() {
        let result = run("echo hello").unwrap();
        assert_eq!(result.text_str(), "hello\n");
        assert_eq!(result.is_error, None);
    }

    #[test]
    fn nonzero_exit_is_error_result() {
        let result = run("echo oops; exit 3").unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result.text_str().ends_with("\n\nCommand exited with code 3"));
    }

    #[test]
    fn missing_cwd() {
        let err = bash_execute(
            "/nonexistent/pi-rs-dir",
            &BashToolInput { command: "true".to_string(), timeout: None },
        )
        .unwrap_err();
        assert_eq!(
            err.message,
            "Working directory does not exist: /nonexistent/pi-rs-dir\nCannot execute bash commands."
        );
    }

    #[test]
    fn invalid_timeout() {
        let err = bash_execute(
            ".",
            &BashToolInput { command: "true".to_string(), timeout: Some(0.0) },
        )
        .unwrap_err();
        assert_eq!(err.message, "Invalid timeout: must be a finite number of seconds");
        let err = bash_execute(
            ".",
            &BashToolInput { command: "true".to_string(), timeout: Some(f64::NAN) },
        )
        .unwrap_err();
        assert_eq!(err.message, "Invalid timeout: must be a finite number of seconds");
        let err = bash_execute(
            ".",
            &BashToolInput { command: "true".to_string(), timeout: Some(3e6) },
        )
        .unwrap_err();
        assert_eq!(err.message, "Invalid timeout: maximum is 2147483.647 seconds");
    }

    #[test]
    fn timeout_error_message() {
        let err = bash_execute(
            ".",
            &BashToolInput { command: "sleep 30".to_string(), timeout: Some(0.3) },
        )
        .unwrap_err();
        assert_eq!(err.message, "Command timed out after 0.3 seconds");
    }

    #[test]
    fn truncated_output_spills() {
        let result = run("seq 1 20000").unwrap();
        let text = result.text_str();
        assert!(text.contains("[Showing lines "));
        assert!(text.contains("Full output: "));
        let details = result.details.expect("details");
        assert!(details["truncation"]["truncated"].as_bool().unwrap());
        let path = details["fullOutputPath"].as_str().expect("spill path");
        assert!(path.contains("pi-bash-"));
        // structuredContent holds the (untruncated at 1MB) full output.
        let structured = result.structured_content.expect("structuredContent");
        assert_eq!(structured["truncated"], serde_json::json!(false));
        let output = structured["output"].as_str().unwrap();
        assert!(output.starts_with("1\n"));
        assert!(output.ends_with("20000\n"));
    }

    #[test]
    fn no_output() {
        let result = run("true").unwrap();
        assert_eq!(result.text_str(), "(no output)");
    }

    #[test]
    fn spawn_env_has_no_pi_vars() {
        let env = resolve_spawn_env();
        for key in ["PI_SESSION_ID", "PI_SESSION_FILE", "PI_PROVIDER", "PI_MODEL", "PI_REASONING_LEVEL"] {
            assert!(!env.contains_key(key), "{} leaked into spawn env", key);
        }
    }
}
