//! Bash command execution with streaming support and cancellation.
//! Port of `src/core/bash-executor.ts`.
//!
//! Used by `AgentSession.executeBash()` (the `!` command) and other modes
//! that need bash execution with rolling-buffer output, ANSI stripping, and
//! cancellation semantics. Distinct from the bash *tool*, which uses
//! `OutputAccumulator`.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use pi_core::ansi::{split_incomplete_ansi_suffix, strip_ansi};
use pi_core::js_string::js_length;
use pi_core::text::sanitize_binary_output;
use pi_core::text_decoder::StreamingDecoder;
use pi_core::truncate::{truncate_tail, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES};

use crate::output_files::create_output_file_stream;
use crate::shell::{AbortSignal, BashOperations, ShellExecError, ShellExecOptions, StreamKind};

/// Rolling-buffer limit: `DEFAULT_MAX_BYTES * 2` (in UTF-16 code units,
/// matching JS `.length`).
const MAX_OUTPUT_BYTES: u64 = DEFAULT_MAX_BYTES * 2;

#[derive(Default)]
pub struct BashExecutorOptions<'a> {
    /// Callback for streaming output chunks (already sanitized).
    pub on_chunk: Option<&'a mut dyn FnMut(&str)>,
    pub signal: Option<&'a AbortSignal>,
}

/// Port of `BashResult`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    /// Combined stdout + stderr output (sanitized, possibly truncated).
    pub output: String,
    /// Process exit code (`None` if killed/cancelled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Whether the command was cancelled via signal.
    pub cancelled: bool,
    /// Whether the output was truncated.
    pub truncated: bool,
    /// Path to temp file containing full output (if output exceeded the
    /// truncation threshold).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

struct ExecutorState {
    output_chunks: VecDeque<String>,
    /// Total UTF-16 length of `output_chunks` (JS `.length` semantics).
    output_bytes: u64,
    temp_file_path: Option<PathBuf>,
    temp_file: Option<File>,
    total_bytes: u64,
    decoder: StreamingDecoder,
    /// Unfinished escape sequence at the end of the previous chunk,
    /// completed by the next chunk.
    pending_ansi: String,
}

impl ExecutorState {
    fn new() -> Self {
        ExecutorState {
            output_chunks: VecDeque::new(),
            output_bytes: 0,
            temp_file_path: None,
            temp_file: None,
            total_bytes: 0,
            decoder: StreamingDecoder::new(),
            pending_ansi: String::new(),
        }
    }

    /// `ensureTempFile`: open the spill file and flush existing chunks.
    fn ensure_temp_file(&mut self) -> Result<(), String> {
        if self.temp_file_path.is_some() {
            return Ok(());
        }
        let (path, mut file) = create_output_file_stream("pi-bash", ".log").map_err(|e| e.to_string())?;
        for chunk in &self.output_chunks {
            file.write_all(chunk.as_bytes()).map_err(|e| e.to_string())?;
        }
        self.temp_file_path = Some(path);
        self.temp_file = Some(file);
        Ok(())
    }

    fn on_data(
        &mut self,
        on_chunk: &mut Option<&mut dyn FnMut(&str)>,
        data: &[u8],
    ) -> Result<(), String> {
        self.total_bytes += data.len() as u64;
        let mut combined = std::mem::take(&mut self.pending_ansi);
        combined.push_str(&self.decoder.decode(data));
        let (complete, pending) = split_incomplete_ansi_suffix(&combined);
        self.pending_ansi = pending;
        self.append_text(on_chunk, &complete)
    }

    /// `appendText`: sanitize (strip ANSI, remove binary garbage, drop \r),
    /// spill to the temp file past the threshold, keep the rolling buffer,
    /// and stream to the callback.
    fn append_text(
        &mut self,
        on_chunk: &mut Option<&mut dyn FnMut(&str)>,
        raw_text: &str,
    ) -> Result<(), String> {
        let text = sanitize_binary_output(&strip_ansi(raw_text)).replace('\r', "");
        if text.is_empty() {
            return Ok(());
        }

        // Start writing to temp file if output exceeds the threshold.
        if self.total_bytes > DEFAULT_MAX_BYTES {
            self.ensure_temp_file()?;
        }

        if let Some(file) = &mut self.temp_file {
            file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }

        // Keep rolling buffer.
        self.output_bytes += js_length(&text) as u64;
        self.output_chunks.push_back(text.clone());
        while self.output_bytes > MAX_OUTPUT_BYTES && self.output_chunks.len() > 1 {
            if let Some(removed) = self.output_chunks.pop_front() {
                self.output_bytes -= js_length(&removed) as u64;
            }
        }

        // Stream to callback.
        if let Some(cb) = on_chunk.as_mut() {
            cb(&text);
        }
        Ok(())
    }
}

/// Port of `executeBashWithOperations`. An aborted command still returns the
/// output produced so far; other exec errors propagate after ending the
/// spill stream.
pub fn execute_bash_with_operations(
    command: &str,
    cwd: &str,
    operations: &dyn BashOperations,
    options: Option<BashExecutorOptions<'_>>,
) -> Result<BashResult, ShellExecError> {
    let mut options = options;
    let mut on_chunk = options.as_mut().and_then(|o| o.on_chunk.take());
    let signal = options.as_ref().and_then(|o| o.signal);
    let mut state = ExecutorState::new();

    let mut exit_code: Option<i32> = None;
    let exec_result = {
        let mut on_data = |_kind: StreamKind, data: &[u8]| {
            // IO errors on the spill file surface as Other below.
            let _ = state.on_data(&mut on_chunk, data);
        };
        let opts = ShellExecOptions { on_data: Some(&mut on_data), signal, timeout: None, env: None };
        operations.exec(command, cwd, opts)
    };
    match exec_result {
        Ok(outcome) => exit_code = outcome.exit_code,
        Err(err) => {
            // An aborted command still returns the output it produced so far.
            if !signal.map(|s| s.aborted()).unwrap_or(false) {
                state.temp_file = None; // tempFileStream?.end()
                return Err(err);
            }
        }
    }

    // flushOutput: pendingAnsi + decoder.decode()
    let mut rest = std::mem::take(&mut state.pending_ansi);
    rest.push_str(&state.decoder.finish());
    state.append_text(&mut on_chunk, &rest).map_err(ShellExecError::Other)?;

    let full_output: String = state.output_chunks.iter().flat_map(|c| c.chars()).collect();
    let truncation: TruncationResult = truncate_tail(&full_output, TruncationOptions::default());
    if truncation.truncated {
        state.ensure_temp_file().map_err(ShellExecError::Other)?;
    }
    state.temp_file = None; // tempFileStream?.end()
    let cancelled = signal.map(|s| s.aborted()).unwrap_or(false);

    Ok(BashResult {
        output: if truncation.truncated { truncation.content.clone() } else { full_output },
        exit_code: if cancelled { None } else { exit_code },
        cancelled,
        truncated: truncation.truncated,
        full_output_path: state.temp_file_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::create_local_bash_operations;

    fn run(command: &str) -> Result<BashResult, ShellExecError> {
        let ops = create_local_bash_operations(None);
        execute_bash_with_operations(command, ".", &ops, None)
    }

    #[test]
    fn simple_echo() {
        let result = run("echo hello").unwrap();
        assert_eq!(result.output, "hello\n");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.cancelled);
        assert!(!result.truncated);
        assert_eq!(result.full_output_path, None);
    }

    #[test]
    fn no_output() {
        let result = run("true").unwrap();
        assert_eq!(result.output, "");
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn nonzero_exit() {
        let result = run("echo oops; exit 3").unwrap();
        assert_eq!(result.output, "oops\n");
        assert_eq!(result.exit_code, Some(3));
    }

    #[test]
    fn ansi_stripped() {
        let result = run("printf 'a\\033[31mred\\033[0mb\\n'").unwrap();
        assert_eq!(result.output, "aredb\n");
    }

    #[test]
    fn ansi_split_across_chunks() {
        // The escape sequence is split across two pipe writes; the pending
        // suffix must be held back and completed by the next chunk.
        let result = run("printf 'a\\033[3'; sleep 0.3; printf '1mred\\033[0m\\n'").unwrap();
        assert_eq!(result.output, "ared\n");
    }

    #[test]
    fn carriage_returns_removed() {
        let result = run("printf 'a\\rb\\r\\n'").unwrap();
        assert_eq!(result.output, "ab\n");
    }

    #[test]
    fn binary_garbage_removed() {
        let result = run("printf 'a\\001\\002b\\003\\n'").unwrap();
        assert_eq!(result.output, "ab\n");
    }

    #[test]
    fn utf8_split_across_chunks() {
        let result = run("printf 'a\\303'; sleep 0.3; printf '\\251b\\n'").unwrap();
        assert_eq!(result.output, "aéb\n");
    }

    #[test]
    fn on_chunk_streams_sanitized_text() {
        let ops = create_local_bash_operations(None);
        let mut chunks: Vec<String> = Vec::new();
        {
            let mut cb = |text: &str| chunks.push(text.to_string());
            let options = BashExecutorOptions { on_chunk: Some(&mut cb), signal: None };
            let result = execute_bash_with_operations("printf 'one\\n'; sleep 0.2; printf 'two\\n'", ".", &ops, Some(options)).unwrap();
            assert_eq!(result.output, "one\ntwo\n");
        }
        assert!(chunks.iter().any(|c| c == "one\n"), "chunks: {:?}", chunks);
        assert!(chunks.iter().any(|c| c == "two\n"), "chunks: {:?}", chunks);
    }

    #[test]
    fn big_output_truncates_and_spills() {
        // ~109KB of output: past the 50KB threshold → truncated + spill file.
        let result = run("seq 1 20000").unwrap();
        assert!(result.truncated);
        let path = result.full_output_path.expect("spill path");
        assert!(path.contains("pi-bash-"), "path: {}", path);
        assert!(path.ends_with(".log"));
        // Truncated output keeps the tail (truncateTail drops the final
        // newline of the last line, matching the original).
        assert!(result.output.ends_with("20000"));
        assert!(!result.output.starts_with("1\n"));
        assert!(result.output.len() < 110_000);
        let spilled = std::fs::read_to_string(&path).unwrap();
        assert!(spilled.starts_with("1\n"));
        assert!(spilled.ends_with("20000\n"));
    }

    #[test]
    fn aborted_returns_partial_output() {
        let ops = create_local_bash_operations(None);
        let signal = AbortSignal::new();
        // Abort while the command is running.
        let signal2 = signal.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            signal2.abort();
        });
        let options = BashExecutorOptions { on_chunk: None, signal: Some(&signal) };
        let result =
            execute_bash_with_operations("echo before; sleep 5; echo after", ".", &ops, Some(options)).unwrap();
        assert!(result.cancelled);
        assert_eq!(result.exit_code, None);
        assert_eq!(result.output, "before\n");
    }

    use std::time::Duration;
}
