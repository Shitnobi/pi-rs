//! Port of `src/core/output-guard.ts`.
//!
//! The TS module guards the ambient `process.stdout` / `process.stderr`
//! streams: it can redirect stdout writes to stderr (`takeOverStdout`, so a
//! TUI can own the real stdout), write raw text to the real stdout with
//! backpressure retries, and restore the original write. There is no ambient
//! process in a Rust library, so the streams are INJECTED: an
//! [`OutputGuard`] owns the stdout write slot (the `process.stdout.write`
//! stand-in) plus the stderr writer. Callers route their "stdout" writes
//! through [`OutputGuard::write_stdout`] to observe the takeover.
//!
//! Other divergences from the TS, which is async and process-global:
//! - writes run inline instead of being chained onto a promise tail, so
//!   `waitForRawStdoutBackpressure` is a no-op and `flushRawStdout` simply
//!   performs the empty write the TS uses to flush;
//! - an unrecoverable raw-write failure is returned as `Err` instead of
//!   killing the process via `process.exit(1)`.

use std::cell::RefCell;
use std::rc::Rc;
use std::thread::sleep;
use std::time::Duration;

/// A raw stream write: the `process.stdout.write` / `process.stderr.write`
/// stand-in. The `Err` stands in for the Node write callback's `Error`.
pub type RawWrite = Rc<RefCell<dyn FnMut(&str) -> Result<(), RawWriteError>>>;

/// Build a [`RawWrite`] from a closure.
pub fn raw_write<F>(f: F) -> RawWrite
where
	F: FnMut(&str) -> Result<(), RawWriteError> + 'static,
{
	Rc::new(RefCell::new(f))
}

/// The error a raw write reports via its Node callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawWriteError {
	/// The Node error `code`, e.g. `"ENOBUFS"`.
	pub code: Option<String>,
	pub message: String,
}

impl RawWriteError {
	pub fn new(code: Option<&str>, message: impl Into<String>) -> Self {
		Self {
			code: code.map(str::to_string),
			message: message.into(),
		}
	}

	/// `ENOBUFS` / `EAGAIN` / `EWOULDBLOCK`: the codes the TS retries.
	pub fn is_retryable(&self) -> bool {
		matches!(
			self.code.as_deref(),
			Some("ENOBUFS") | Some("EAGAIN") | Some("EWOULDBLOCK")
		)
	}
}

/// `RAW_STDOUT_RETRY_DELAY_MS`.
pub const RAW_STDOUT_RETRY_DELAY_MS: u64 = 10;

/// `StdoutTakeoverState`: the writes saved when stdout was taken over.
struct StdoutTakeoverState {
	raw_stdout_write: RawWrite,
	raw_stderr_write: RawWrite,
	original_stdout_write: RawWrite,
}

/// The output guard: owns the stdout write slot and the stderr writer, plus
/// the takeover state.
pub struct OutputGuard {
	/// The `process.stdout.write` slot: the writer installed right now.
	stdout_write: RawWrite,
	/// `process.stderr.write`, bound when the guard is built.
	stderr_write: RawWrite,
	state: Option<StdoutTakeoverState>,
}

impl OutputGuard {
	/// Build the guard from the process's current stdout / stderr writers.
	pub fn new(stdout_write: RawWrite, stderr_write: RawWrite) -> Self {
		Self {
			stdout_write,
			stderr_write,
			state: None,
		}
	}

	/// `takeOverStdout`: forward all stdout writes to stderr until
	/// [`OutputGuard::restore_stdout`]. A no-op when already taken over.
	pub fn take_over_stdout(&mut self) {
		if self.state.is_some() {
			return;
		}
		let state = StdoutTakeoverState {
			raw_stdout_write: self.stdout_write.clone(),
			raw_stderr_write: self.stderr_write.clone(),
			original_stdout_write: self.stdout_write.clone(),
		};
		// process.stdout.write = (chunk, ...) => rawStderrWrite(String(chunk), ...)
		let forward_stderr = state.raw_stderr_write.clone();
		self.stdout_write = raw_write(move |chunk| (forward_stderr.borrow_mut())(chunk));
		self.state = Some(state);
	}

	/// `restoreStdout`: put the original stdout write back. A no-op when
	/// stdout is not taken over.
	pub fn restore_stdout(&mut self) {
		let Some(state) = self.state.take() else {
			return;
		};
		self.stdout_write = state.original_stdout_write;
	}

	/// `isStdoutTakenOver`.
	pub fn is_stdout_taken_over(&self) -> bool {
		self.state.is_some()
	}

	/// Call `process.stdout.write`: routes to stderr while taken over.
	pub fn write_stdout(&self, chunk: &str) -> Result<(), RawWriteError> {
		(self.stdout_write.borrow_mut())(chunk)
	}

	/// `writeRawStdout`: write to the real stdout, retrying on backpressure.
	/// Empty text is skipped. Unlike the TS, which queues onto a promise
	/// tail and never reports the failure, errors are returned.
	pub fn write_raw_stdout(&mut self, text: &str) -> Result<(), RawWriteError> {
		if text.is_empty() {
			return Ok(());
		}
		self.write_raw_stdout_chunk(text)
	}

	/// `waitForRawStdoutBackpressure`: wait for the queued writes to settle.
	/// Writes run inline here, so there is nothing to wait for.
	pub fn wait_for_raw_stdout_backpressure(&self) {}

	/// `flushRawStdout`: wait for the queue, then push an empty chunk
	/// through the raw write.
	pub fn flush_raw_stdout(&mut self) -> Result<(), RawWriteError> {
		self.wait_for_raw_stdout_backpressure();
		self.write_raw_stdout_chunk("")
	}

	/// `getRawStdoutWrite`: the raw stdout writer — bound at takeover when
	/// taken over, the live stdout slot otherwise.
	fn get_raw_stdout_write(&self) -> RawWrite {
		match &self.state {
			Some(state) => state.raw_stdout_write.clone(),
			None => self.stdout_write.clone(),
		}
	}

	/// `writeRawStdoutChunk`: retry the write while it reports `ENOBUFS` /
	/// `EAGAIN` / `EWOULDBLOCK`, waiting `RAW_STDOUT_RETRY_DELAY_MS` between
	/// attempts.
	fn write_raw_stdout_chunk(&self, text: &str) -> Result<(), RawWriteError> {
		loop {
			let write = self.get_raw_stdout_write();
			let outcome = (write.borrow_mut())(text);
			match outcome {
				Ok(()) => return Ok(()),
				Err(error) if error.is_retryable() => {
					sleep(Duration::from_millis(RAW_STDOUT_RETRY_DELAY_MS));
				}
				Err(error) => return Err(error),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Default)]
	struct RecordingStream {
		writes: RefCell<Vec<String>>,
	}

	impl RecordingStream {
		fn writer(self: &Rc<Self>) -> RawWrite {
			let stream = Rc::clone(self);
			raw_write(move |chunk| {
				stream.writes.borrow_mut().push(chunk.to_string());
				Ok(())
			})
		}
	}

	fn guard() -> (Rc<RecordingStream>, Rc<RecordingStream>, OutputGuard) {
		let stdout = Rc::new(RecordingStream::default());
		let stderr = Rc::new(RecordingStream::default());
		let guard = OutputGuard::new(stdout.writer(), stderr.writer());
		(stdout, stderr, guard)
	}

	#[test]
	fn takeover_redirects_stdout_to_stderr() {
		let (stdout, stderr, mut guard) = guard();
		assert!(!guard.is_stdout_taken_over());
		guard.write_stdout("before").unwrap();
		assert_eq!(stdout.writes.borrow().clone(), vec!["before".to_string()]);

		guard.take_over_stdout();
		assert!(guard.is_stdout_taken_over());
		// Idempotent: a second takeover is a no-op.
		guard.take_over_stdout();
		guard.write_stdout("during").unwrap();
		assert_eq!(stdout.writes.borrow().clone(), vec!["before".to_string()]);
		assert_eq!(stderr.writes.borrow().clone(), vec!["during".to_string()]);

		guard.restore_stdout();
		assert!(!guard.is_stdout_taken_over());
		guard.write_stdout("after").unwrap();
		assert_eq!(stdout.writes.borrow().clone(), vec!["before".to_string(), "after".to_string()]);
	}

	#[test]
	fn restore_without_takeover_is_noop() {
		let (_, _, mut guard) = guard();
		guard.restore_stdout();
		assert!(!guard.is_stdout_taken_over());
	}

	#[test]
	fn raw_stdout_bypasses_the_takeover() {
		let (stdout, stderr, mut guard) = guard();
		guard.take_over_stdout();
		guard.write_raw_stdout("raw").unwrap();
		assert_eq!(stdout.writes.borrow().clone(), vec!["raw".to_string()]);
		assert!(stderr.writes.borrow().is_empty());
	}

	#[test]
	fn raw_stdout_skips_empty_text() {
		let (stdout, _, mut guard) = guard();
		guard.write_raw_stdout("").unwrap();
		assert!(stdout.writes.borrow().is_empty());
	}

	#[test]
	fn raw_stdout_retries_backpressure_errors() {
		let stdout = Rc::new(RecordingStream::default());
		let attempts = Rc::new(RefCell::new(0));
		let flaky = {
			let attempts = Rc::clone(&attempts);
			let inner = stdout.writer();
			raw_write(move |chunk| {
				let mut attempts = attempts.borrow_mut();
				*attempts += 1;
				if *attempts < 3 {
					Err(RawWriteError::new(Some("ENOBUFS"), "backpressure"))
				} else {
					(inner.borrow_mut())(chunk)
				}
			})
		};
		let mut guard = OutputGuard::new(flaky, stdout.writer());
		guard.write_raw_stdout("text").unwrap();
		assert_eq!(*attempts.borrow(), 3);
		assert_eq!(stdout.writes.borrow().clone(), vec!["text".to_string()]);
	}

	#[test]
	fn raw_stdout_returns_unrecoverable_errors() {
		let stdout = Rc::new(RecordingStream::default());
		let failing = raw_write(|_chunk| Err(RawWriteError::new(Some("EBADF"), "bad descriptor")));
		let mut guard = OutputGuard::new(failing, stdout.writer());
		let error = guard.write_raw_stdout("text").unwrap_err();
		assert_eq!(error.code.as_deref(), Some("EBADF"));
	}

	#[test]
	fn flush_pushes_an_empty_chunk_through_the_raw_write() {
		let (stdout, _, mut guard) = guard();
		guard.take_over_stdout();
		guard.flush_raw_stdout().unwrap();
		// The empty flush chunk reaches the real stdout even mid-takeover.
		assert_eq!(stdout.writes.borrow().clone(), vec![String::new()]);
	}

	#[test]
	fn retryable_codes_match_the_ts() {
		assert!(RawWriteError::new(Some("ENOBUFS"), "e").is_retryable());
		assert!(RawWriteError::new(Some("EAGAIN"), "e").is_retryable());
		assert!(RawWriteError::new(Some("EWOULDBLOCK"), "e").is_retryable());
		assert!(!RawWriteError::new(Some("EBADF"), "e").is_retryable());
		assert!(!RawWriteError::new(None, "e").is_retryable());
	}
}
