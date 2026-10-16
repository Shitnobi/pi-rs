//! The `bash` tool. Port of `src/core/tools/bash.ts` +
//! `src/core/tools/output-accumulator.ts` (unix local shell backend).
//!
//! Commands run via `/bin/bash -c` (matching `getShellConfig` on unix),
//! stdout and stderr are combined in arrival order, and output is truncated
//! from the tail with the same limits and messages as the original.

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use pi_core::truncate::{format_size, truncate_tail, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};

use crate::{ToolError, ToolOutput, ToolResult};

/// Output limit of `structuredContent.output` in the original.
const STRUCTURED_OUTPUT_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct BashToolInput {
    pub command: String,
    /// Timeout in seconds (optional, no default timeout)
    pub timeout: Option<f64>,
}

/// Resolve shell configuration like `getShellConfig` on unix:
/// /bin/bash, then bash on PATH, then fallback to sh.
fn get_shell_config() -> (String, Vec<String>) {
    if std::path::Path::new("/bin/bash").exists() {
        return ("/bin/bash".to_string(), vec!["-c".to_string()]);
    }
    if let Some(bash) = crate::grep::ensure_tool("bash") {
        return (bash, vec!["-c".to_string()]);
    }
    ("sh".to_string(), vec!["-c".to_string()])
}

/// Port of `getShellEnv`: pi bin dir prepended to PATH, PI_* session vars
/// removed (no session in harness mode).
fn get_shell_env() -> std::collections::HashMap<String, String> {
    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let bin_dir = std::env::var("HOME")
        .map(|home| format!("{}/.pi/agent/bin", home))
        .unwrap_or_default();
    let current_path = env.get("PATH").cloned().unwrap_or_default();
    let has_bin_dir = current_path.split(':').any(|p| p == bin_dir);
    if !has_bin_dir && !bin_dir.is_empty() {
        let updated = if current_path.is_empty() {
            bin_dir
        } else {
            format!("{}:{}", bin_dir, current_path)
        };
        env.insert("PATH".to_string(), updated);
    }
    for key in ["PI_SESSION_ID", "PI_SESSION_FILE", "PI_PROVIDER", "PI_MODEL", "PI_REASONING_LEVEL"] {
        env.remove(key);
    }
    env
}

/// Port of `resolveTimeoutMs`.
fn resolve_timeout_secs(timeout: Option<f64>) -> Result<Option<Duration>, ToolError> {
    match timeout {
        None => Ok(None),
        Some(t) => {
            if !t.is_finite() || t <= 0.0 {
                return Err(ToolError::new("Invalid timeout: must be a finite number of seconds"));
            }
            if t > 2_147_483.647 {
                return Err(ToolError::new("Invalid timeout: maximum is 2147483.647 seconds"));
            }
            Ok(Some(Duration::from_secs_f64(t)))
        }
    }
}

/// Port of the OutputAccumulator's final snapshot (the incremental rolling
/// window in the original is memory-bounded but yields the same tail:
/// kept content is always within `maxBytes` of the end of the output).
#[allow(dead_code)]
struct AccumulatedOutput {
    /// Combined stdout+stderr in arrival order.
    text: String,
    /// Path of the spilled full-output temp file, if truncation occurred.
    full_output_path: Option<String>,
}

fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{}\n\n{}", text, status)
    }
}

pub fn bash_execute(cwd: &str, input: &BashToolInput) -> ToolResult {
    let timeout = resolve_timeout_secs(input.timeout)?;
    if let Some(err) = check_cwd(cwd) {
        return Err(err);
    }

    let (shell, shell_args) = get_shell_config();
    let mut command = Command::new(&shell);
    // The original spawns with `detached: true` on unix so the child leads
    // its own process group and killProcessTree can kill(-pid, SIGKILL).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .args(&shell_args)
        .arg(&input.command)
        .current_dir(cwd)
        .env_clear()
        .envs(get_shell_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(err) => return Err(ToolError::new(format!("Failed to run bash: {}", err))),
    };

    let combined: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    let stdout_handle = {
        let combined = Arc::clone(&combined);
        child.stdout.take().map(|mut s| {
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => combined.lock().unwrap().extend_from_slice(&buf[..n]),
                    }
                }
            })
        })
    };
    let stderr_handle = {
        let combined = Arc::clone(&combined);
        child.stderr.take().map(|mut s| {
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => combined.lock().unwrap().extend_from_slice(&buf[..n]),
                    }
                }
            })
        })
    };

    let started_at = Instant::now();
    let timed_out = match timeout {
        Some(duration) => {
            // Wait with timeout; kill the process tree (child process group)
            // on expiry, matching the original's killProcessTree behavior.
            let deadline = started_at + duration;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break false,
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            let _ = kill_process_tree(child.id());
                            // Give the child a moment to die and pipes to drain.
                            let _ = child.wait();
                            break true;
                        } else {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                    Err(_) => break false,
                }
            }
        }
        None => {
            let _ = child.wait();
            false
        }
    };

    if let Some(h) = stdout_handle {
        let _ = h.join();
    }
    if let Some(h) = stderr_handle {
        let _ = h.join();
    }

    let bytes = Arc::try_unwrap(combined)
        .map(|m| m.into_inner().unwrap())
        .unwrap_or_else(|m| m.lock().unwrap().clone());
    let text = String::from_utf8_lossy(&bytes).into_owned();

    if timed_out {
        let timeout_secs = input.timeout.unwrap();
        // Match the original's error shape: partial output + status line.
        let snapshot = snapshot_from_full_output(&text, Some("pi-bash"));
        let display = format_output(&snapshot, "");
        return Err(ToolError::new(append_status(
            &display.text,
            &format!("Command timed out after {} seconds", format_timeout_secs(timeout_secs)),
        )));
    }

    let exit_code: Option<i32> = child.wait().ok().and_then(|s| s.code());
    // A signal-killed shell has no exit code; use 128 + signal convention.
    let exit_code = match exit_code {
        Some(c) => Some(c),
        None => child
            .wait()
            .ok()
            .and_then(|s| s.signal())
            .map(|sig| 128 + sig),
    };

    let snapshot = snapshot_from_full_output(&text, Some("pi-bash"));
    let display = format_output(&snapshot, "(no output)");

    let exit_code = match exit_code {
        None => {
            return Err(ToolError::new(append_status(&display.text, "Command terminated without an exit code")));
        }
        Some(code) => code,
    };

    if exit_code != 0 {
        return Ok(ToolOutput {
            content: vec![crate::ContentBlock::Text {
                text: append_status(&display.text, &format!("Command exited with code {}", exit_code)),
            }],
            details: display.details,
            is_error: Some(true),
            structured_content: Some(bash_structured_content(&text, &snapshot, exit_code, started_at)),
        });
    }

    Ok(ToolOutput {
        content: vec![crate::ContentBlock::Text { text: display.text }],
        details: display.details,
        is_error: None,
        structured_content: Some(bash_structured_content(&text, &snapshot, exit_code, started_at)),
    })
}

/// Port of the original's `structuredContent` construction:
/// `output.readFullOutput(STRUCTURED_OUTPUT_MAX_BYTES)` + exit code + wall time.
fn bash_structured_content(
    full_text: &str,
    snapshot: &Snapshot,
    exit_code: i32,
    started_at: Instant,
) -> serde_json::Value {
    let (output, truncated) = read_full_output(full_text, STRUCTURED_OUTPUT_MAX_BYTES);
    let mut obj = serde_json::json!({
        "output": output,
        "truncated": truncated,
        "exit_code": exit_code,
        "wall_time_seconds": wall_time_seconds(started_at),
    });
    if truncated {
        if let Some(path) = &snapshot.full_output_path {
            obj.as_object_mut()
                .unwrap()
                .insert("full_output_path".to_string(), serde_json::json!(path));
        }
    }
    obj
}

/// `Math.round((performance.now() - startedAt) / 100) / 10`
fn wall_time_seconds(started_at: Instant) -> f64 {
    let ms = started_at.elapsed().as_secs_f64() * 1000.0;
    (ms / 100.0).round() / 10.0
}

/// Port of `OutputAccumulator.readFullOutput(maxBytes)`.
fn read_full_output(full_text: &str, max_bytes: usize) -> (String, bool) {
    let bytes = full_text.as_bytes();
    if bytes.len() <= max_bytes {
        // TextDecoder strips a leading BOM.
        let s = full_text.strip_prefix('\u{feff}').unwrap_or(full_text);
        return (s.to_string(), false);
    }
    let head_bytes = max_bytes / 2;
    let tail_bytes = max_bytes - head_bytes;
    let head = &bytes[..head_bytes];
    let tail = &bytes[bytes.len() - tail_bytes..];
    // Streaming decode holds back an incomplete trailing sequence.
    let head_text = decode_stream_head(head);
    // The tail skips leading continuation bytes.
    let mut tail_start = 0;
    while tail_start < tail.len() && (tail[tail_start] & 0xc0) == 0x80 {
        tail_start += 1;
    }
    let tail_text = String::from_utf8_lossy(&tail[tail_start..]).into_owned();
    let omitted = bytes.len() - head_bytes - tail_bytes;
    (format!("{}\n\n[... {} bytes omitted ...]\n\n{}", head_text, omitted, tail_text), true)
}

/// `new TextDecoder().decode(head, { stream: true })`: valid UTF-8 decodes,
/// invalid sequences become U+FFFD, an incomplete trailing multi-byte
/// sequence is held back, and a leading BOM is stripped.
fn decode_stream_head(head: &[u8]) -> String {
    let mut end = head.len();
    for skip in 1..=3.min(head.len()) {
        let b = head[head.len() - skip];
        let width = if (b & 0xe0) == 0xc0 {
            2
        } else if (b & 0xf0) == 0xe0 {
            3
        } else if (b & 0xf8) == 0xf0 {
            4
        } else {
            0
        };
        if width > 0 && skip < width {
            end = head.len() - skip;
            break;
        }
    }
    let mut s = String::from_utf8_lossy(&head[..end]).into_owned();
    if s.starts_with('\u{feff}') {
        s.remove(0);
    }
    s
}

fn check_cwd(cwd: &str) -> Option<ToolError> {
    if !std::path::Path::new(cwd).exists() {
        Some(ToolError::new(format!(
            "Working directory does not exist: {}\nCannot execute bash commands.",
            cwd
        )))
    } else {
        None
    }
}

fn format_timeout_secs(t: f64) -> String {
    // The original prints the raw input value (err.message.split(":")[1]).
    if t.fract() == 0.0 {
        format!("{}", t as i64)
    } else {
        format!("{}", t)
    }
}

fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    // Negative pid kills the whole process group (the original spawns the
    // shell detached on unix and kills the tree).
    unsafe {
        killpg(pid)
    }
}

#[cfg(unix)]
unsafe fn killpg(pid: u32) -> std::io::Result<()> {
    let ret = libc_killpg(pid);
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
unsafe fn libc_killpg(pid: u32) -> i32 {
    // Direct syscall without a libc crate dependency.
    #[repr(C)]
    #[allow(dead_code)]
    struct SigAction;
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(-(pid as i32), 9)
}

#[cfg(not(unix))]
unsafe fn killpg(_pid: u32) -> std::io::Result<()> {
    Ok(())
}

/// Snapshot over the full output: truncate from the tail with the standard
/// limits, spilling the full output to a temp file when truncated.
struct Snapshot {
    content: String,
    truncation: pi_core::truncate::TruncationResult,
    full_output_path: Option<String>,
    last_line_bytes: u64,
}

struct FormattedOutput {
    text: String,
    details: Option<serde_json::Value>,
}

fn snapshot_from_full_output(text: &str, temp_prefix: Option<&str>) -> Snapshot {
    let truncation = truncate_tail(
        text,
        pi_core::truncate::TruncationOptions { max_lines: Some(DEFAULT_MAX_LINES), max_bytes: Some(DEFAULT_MAX_BYTES) },
    );

    let truncated = truncation.total_lines > DEFAULT_MAX_LINES || truncation.total_bytes > DEFAULT_MAX_BYTES;
    let truncated_by = if truncated {
        truncation
            .truncated_by
            .clone()
            .or(if truncation.total_bytes > DEFAULT_MAX_BYTES { Some("bytes".to_string()) } else { Some("lines".to_string()) })
    } else {
        None
    };
    let mut truncation = truncation;
    truncation.truncated = truncated;
    truncation.truncated_by = truncated_by;

    let full_output_path = if truncation.truncated {
        temp_prefix.and_then(|prefix| spill_to_temp_file(text, prefix))
    } else {
        None
    };

    // Bytes of the last (possibly open) line of the full output.
    let last_line_bytes = match text.rfind('\n') {
        Some(idx) => (text.len() - idx - 1) as u64,
        None => text.len() as u64,
    };

    Snapshot {
        content: truncation.content.clone(),
        truncation,
        full_output_path,
        last_line_bytes,
    }
}

fn spill_to_temp_file(text: &str, prefix: &str) -> Option<String> {
    // Original: <tmpdir>/<prefix>-<8 random bytes as hex>.log, mode 0600,
    // created with "wx" (never follows a link placed at the path).
    let dir = std::env::temp_dir();
    let name = format!("{}-{}.log", prefix, random_hex_16());
    let path = dir.join(name);
    if path.exists() {
        return None; // 'wx' semantics: fail rather than clobber
    }
    std::fs::write(&path, text).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Some(path.to_string_lossy().into_owned())
}

/// 8 random bytes as 16 hex chars, like `randomBytes(8).toString("hex")`.
fn random_hex_16() -> String {
    let mut bytes = [0u8; 8];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
        .is_ok();
    if !ok {
        // Fallback: mix the clock and pid.
        let nanos = nanos_now() as u64;
        let pid = std::process::id() as u64;
        let mut state = nanos ^ (pid << 32) ^ 0x9e3779b97f4a7c15;
        for b in bytes.iter_mut() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *b = (state >> 33) as u8;
        }
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn nanos_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn format_output(snapshot: &Snapshot, empty_text: &str) -> FormattedOutput {
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
            let last_line_size = format_size(snapshot.last_line_bytes);
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
