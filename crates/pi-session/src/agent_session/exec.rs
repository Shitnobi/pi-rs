//! Port of `core/exec.ts` (+ the `waitForChildProcess` subset of
//! `utils/child-process.ts`): `execCommand` — spawn without a shell, piped
//! stdout/stderr, SIGTERM-then-SIGKILL(5 s) `killProcess`, abort-signal and
//! `timeout > 0` handling, and the two spawn-argument validation errors.
//!
//! All observable semantics probe-pinned against the compiled dist
//! (node v24, see docs/plans/phase4-tranche6-extension-runner-rest.md):
//! - `{stdout, stderr, code, killed}` literal order in BOTH the normal and
//!   the spawn-error branch; stdout/stderr are UTF-8-**lossy** strings.
//! - `code = exitCode ?? 0`: exit-by-signal (SIGTERM/SIGKILL, ours or the
//!   child's own) → `None` → `code: 0`; `killed` is true only when
//!   `killProcess` ran (abort/timeout).
//! - spawn error (missing binary **or** missing cwd) → the promise resolves
//!   `{stdout: "", stderr: "", code: 1, killed}` — spawn-error stderr stays
//!   empty; `killed` stays false unless the signal was already aborted.
//! - `spawn(command, args)` validation: non-string `command` → reject
//!   `The "file" argument must be of type string. Received type number (5)`;
//!   non-array `args` → reject `The "args" argument must be of type object.
//!   Received type number (5)`. Array **elements are string-coerced, not
//!   validated** (`[5]` → `"5"`, `[null]` → `"null"`).
//! - `options.timeout > 0` (JS `>`: null/""/false/absent → false,
//!   `"120"`/true → true) → `setTimeout(killProcess, timeout)`.
//! - abort: pre-aborted signal → `killProcess` immediately; otherwise an
//!   `addEventListener("abort", killProcess)` (never removed — mirrors the
//!   TS; listeners on an already-aborted signal never fire).
//! - `killProcess` = idempotent first-call guard: `killed = true` +
//!   SIGTERM, then a detached 5 s SIGKILL (both ignored if the process is
//!   already gone).
//! - `waitForChildProcess` resolves when the process exits AND the pipes
//!   reach EOF (partial stdout before a kill is kept). Divergence (doc'd):
//!   the 100 ms post-exit pipe-idle grace for detached descendants is not
//!   ported — the pipes' natural EOF ends the read.
//!
//! `pi.exec` (loader) merges the cwd: `execCommand(command, args,
//! options?.cwd ?? loadingCwd, options)`; `execCommand` itself spawns with
//! the resolved cwd and reads `timeout` off `options`.

use std::io::Read;
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

/// Result of `execCommand` (JSON literal order: stdout, stderr, code,
/// killed).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExecResult {
	pub stdout: String,
	pub stderr: String,
	pub code: i64,
	pub killed: bool,
}

/// The abort-signal seam of `execCommand` options.
pub trait ExecSignal: Send + Sync {
	fn is_aborted(&self) -> bool;
	/// `addEventListener("abort", ..)`; on an already-aborted signal the
	/// listener never fires (the pre-abort check catches it).
	fn add_abort_listener(&self, listener: Box<dyn FnOnce() + Send>);
}

/// A signal that was aborted before the exec started (harness `signal:
/// "aborted"`); `killProcess` runs synchronously at the check.
pub struct PreAbortedSignal;

impl ExecSignal for PreAbortedSignal {
	fn is_aborted(&self) -> bool {
		true
	}
	fn add_abort_listener(&self, _listener: Box<dyn FnOnce() + Send>) {
		// Already-aborted signals never invoke new listeners.
	}
}

/// A live signal (agent-session integration): `abort()` fires the
/// registered listeners once.
pub struct ExecLiveSignal {
	aborted: AtomicBool,
	listeners: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

impl ExecLiveSignal {
	pub fn new() -> Self {
		Self { aborted: AtomicBool::new(false), listeners: Mutex::new(Vec::new()) }
	}

	pub fn abort(&self) {
		if self.aborted.swap(true, Ordering::SeqCst) {
			return;
		}
		for listener in self.listeners.lock().unwrap().drain(..) {
			listener();
		}
	}
}

impl Default for ExecLiveSignal {
	fn default() -> Self {
		Self::new()
	}
}

impl ExecSignal for ExecLiveSignal {
	fn is_aborted(&self) -> bool {
		self.aborted.load(Ordering::SeqCst)
	}
	fn add_abort_listener(&self, listener: Box<dyn FnOnce() + Send>) {
		if self.is_aborted() {
			return;
		}
		self.listeners.lock().unwrap().push(listener);
	}
}

/// Shared `killProcess` state (idempotent first-call guard).
#[derive(Clone)]
struct KillState {
	killed: Arc<AtomicBool>,
	pid: i32,
}

impl KillState {
	/// `killProcess()`: first call sets `killed = true` and sends SIGTERM;
	/// 5 s later a detached thread sends SIGKILL (both ignored if the
	/// process is already gone).
	fn kill_process(&self) {
		if self.killed.swap(true, Ordering::SeqCst) {
			return;
		}
		// `try { proc.kill("SIGTERM") } catch {}` — errors ignored.
		unsafe {
			libc::kill(self.pid, libc::SIGTERM);
		}
		let state = self.clone();
		std::thread::spawn(move || {
			std::thread::sleep(Duration::from_secs(5));
			// try { proc.kill("SIGKILL") } catch {}
			unsafe {
				libc::kill(state.pid, libc::SIGKILL);
			}
		});
	}
}

/// JS `String(v)` coercion for spawn `args` elements (probe-pinned: the
/// elements are coerced, not validated).
fn js_string(v: &Value) -> String {
	match v {
		Value::Null => "null".into(),
		Value::Bool(b) => b.to_string(),
		Value::Number(n) => n.to_string(),
		Value::String(s) => s.clone(),
		// String([1,2]) = "1,2" (recursive join).
		Value::Array(a) => a.iter().map(js_string).collect::<Vec<_>>().join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// Node's `Received type …` fragment of the spawn validation errors.
fn received_type(v: &Value) -> String {
	match v {
		Value::Null => "null (null)".into(),
		Value::Bool(b) => format!("boolean ({})", b),
		Value::Number(n) => format!("number ({})", n),
		Value::String(s) => format!("string ('{}')", s),
		Value::Array(_) => "array ([])".into(),
		Value::Object(_) => "object ({})".into(),
	}
}

/// JS `ToNumber(v)` (for the `options?.timeout > 0` check):
/// null/undefined/""/false → 0; "120"/true → 120/1; non-numeric strings /
/// objects → NaN.
fn js_to_number(v: &Value) -> f64 {
	match v {
		Value::Null => 0.0,
		Value::Bool(b) => *b as u64 as f64,
		Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
		Value::String(s) => {
			let t = s.trim();
			if t.is_empty() {
				0.0
			} else {
				t.parse::<f64>().unwrap_or(f64::NAN)
			}
		}
		Value::Array(_) | Value::Object(_) => f64::NAN,
	}
}

fn spawn_reader<T: Read + Send + 'static>(pipe: Option<T>) -> std::thread::JoinHandle<String> {
	std::thread::spawn(move || {
		let mut buf = Vec::new();
		if let Some(mut pipe) = pipe {
			let _ = pipe.read_to_end(&mut buf);
		}
		String::from_utf8_lossy(&buf).into_owned()
	})
}

/// `execCommand(command, args, cwd, options)` — see the module docs for
/// the probe-pinned semantics. `command`/`args` are the raw JSON values
/// (`pi.exec` passes them through); `cwd` is the resolved working
/// directory (the `options?.cwd ?? loadingCwd` merge happens in
/// `pi.exec`). `options` is the raw options object (the `timeout` read);
/// `signal` is the abort-signal seam (TS `options.signal`).
///
/// Errors: only the two spawn-argument validation failures reject (exact
/// node messages); spawn errors (missing binary/cwd) resolve
/// `{code: 1, killed}`.
pub async fn exec_command(
	command: &Value,
	args: &Value,
	cwd: Option<&str>,
	options: &Value,
	signal: Option<Arc<dyn ExecSignal>>,
) -> Result<ExecResult, String> {
	// spawn(command, args, …) argument validation (synchronous throw →
	// rejection).
	let command_str = match command {
		Value::String(s) => s.clone(),
		other => {
			return Err(format!(
				"The \"file\" argument must be of type string. Received type {}",
				received_type(other)
			))
		}
	};
	let args_vec = match args {
		Value::Array(a) => a.iter().map(js_string).collect::<Vec<_>>(),
		other => {
			return Err(format!(
				"The \"args\" argument must be of type object. Received type {}",
				received_type(other)
			))
		}
	};

	let mut cmd = Command::new(&command_str);
	cmd.args(&args_vec).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
	if let Some(dir) = cwd {
		cmd.current_dir(dir);
	}
	let mut child = match cmd.spawn() {
		Ok(child) => child,
		// Spawn error (missing binary / missing cwd): the 'error' event
		// fires asynchronously — after the signal check. `killProcess` on
		// the never-spawned child is swallowed, but `killed` is already
		// true for a pre-aborted signal; `waitForChildProcess` rejects →
		// the catch resolves `{code: 1, killed}`.
		Err(_) => {
			let killed = signal.as_ref().map(|s| s.is_aborted()).unwrap_or(false);
			return Ok(ExecResult {
				stdout: String::new(),
				stderr: String::new(),
				code: 1,
				killed,
			});
		}
	};

	// stdout/stderr accumulate as UTF8-**lossy** strings (JS
	// `data.toString()`); the reads end at the pipes' natural EOF
	// (piped() → always Some).
	let stdout_thread = spawn_reader(child.stdout.take() as Option<ChildStdout>);
	let stderr_thread = spawn_reader(child.stderr.take() as Option<ChildStderr>);

	let kill_state = KillState { killed: Arc::new(AtomicBool::new(false)), pid: child.id() as i32 };

	// if (options?.signal?.aborted) killProcess(); else
	//   options?.signal?.addEventListener("abort", killProcess);
	match &signal {
		Some(signal) if signal.is_aborted() => kill_state.kill_process(),
		Some(signal) => {
			let state = kill_state.clone();
			signal.add_abort_listener(Box::new(move || state.kill_process()));
		}
		None => {}
	}

	// if (options?.timeout > 0) setTimeout(killProcess, timeout);
	// JS `options?.timeout > 0` (NaN > 0 = false).
	if let Some(timeout) = options.get("timeout").map(js_to_number) {
		if !timeout.is_nan() && timeout > 0.0 {
			let state = kill_state.clone();
			std::thread::spawn(move || {
				std::thread::sleep(Duration::from_millis(timeout.max(0.0) as u64));
				state.kill_process();
			});
		}
	}

	// waitForChildProcess(proc) → { code: exitCode ?? 0, killed }.
	let status = child.wait();
	let killed = kill_state.killed.load(Ordering::SeqCst);
	// A wait error mirrors a rejected `waitForChildProcess` →
	// `{code: 1, killed}`.
	let code = match status {
		Ok(status) => status.code().map(i64::from).unwrap_or(0),
		Err(_) => 1,
	};
	let stdout = stdout_thread.join().unwrap_or_default();
	let stderr = stderr_thread.join().unwrap_or_default();
	Ok(ExecResult { stdout, stderr, code, killed })
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn exec_result_serializes_in_literal_order() {
		// JSON literal order (probe-pinned): stdout, stderr, code, killed.
		let r = ExecResult { stdout: "a".into(), stderr: "".into(), code: 1, killed: false };
		assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"stdout":"a","stderr":"","code":1,"killed":false}"#);
	}

	async fn run(command: &Value, args: &Value, cwd: Option<&str>, options: &Value) -> Result<ExecResult, String> {
		exec_command(command, args, cwd, options, None).await
	}

	#[tokio::test]
	async fn printf_stdout() {
		let r = run(&json!("/usr/bin/printf"), &json!(["hello"]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "hello".into(), stderr: "".into(), code: 0, killed: false });
	}

	#[tokio::test]
	async fn stderr_and_exit_code() {
		let r = run(&json!("/bin/sh"), &json!(["-c", "echo err 1>&2; exit 7"]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "err\n".into(), code: 7, killed: false });
	}

	#[tokio::test]
	async fn timeout_kills() {
		// SIGTERM kill: exitCode null → code 0, killed true (probe-pinned).
		let r = run(&json!("/bin/sleep"), &json!(["2"]), Some("/tmp"), &json!({ "timeout": 120 })).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 0, killed: true });
	}

	#[tokio::test]
	async fn enoent_resolves_code_1() {
		let r = run(&json!("/no/such/binary"), &json!([]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 1, killed: false });
	}

	#[tokio::test]
	async fn bad_cwd_resolves_code_1() {
		let r = run(&json!("/bin/ls"), &json!([]), Some("/no/such/dir"), &json!({})).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 1, killed: false });
	}

	#[tokio::test]
	async fn preaborted_signal_kills() {
		let r = exec_command(
			&json!("/bin/sleep"),
			&json!(["1"]),
			Some("/tmp"),
			&json!({}),
			Some(Arc::new(PreAbortedSignal)),
		)
		.await
		.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 0, killed: true });
	}

	#[tokio::test]
	async fn mid_abort_signal_kills() {
		let signal = Arc::new(ExecLiveSignal::new());
		let s2 = Arc::clone(&signal);
		std::thread::spawn(move || {
			std::thread::sleep(Duration::from_millis(100));
			s2.abort();
		});
		let r = exec_command(&json!("/bin/sleep"), &json!(["1"]), Some("/tmp"), &json!({}), Some(signal))
			.await
			.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 0, killed: true });
	}

	#[tokio::test]
	async fn partial_stdout_before_timeout() {
		let r = run(&json!("/bin/sh"), &json!(["-c", "echo out; sleep 2"]), Some("/tmp"), &json!({ "timeout": 150 })).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "out\n".into(), stderr: "".into(), code: 0, killed: true });
	}

	#[tokio::test]
	async fn child_signal_exit_is_code_0() {
		// kill -9 $$: exitCode null (our killProcess never ran) → code 0,
		// killed false (probe-pinned).
		let r = run(&json!("/bin/sh"), &json!(["-c", "kill -9 $$"]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 0, killed: false });
	}

	#[tokio::test]
	async fn non_string_command_rejects() {
		let e = run(&json!(5), &json!([]), Some("/tmp"), &json!({})).await.unwrap_err();
		assert_eq!(e, "The \"file\" argument must be of type string. Received type number (5)");
	}

	#[tokio::test]
	async fn non_array_args_rejects() {
		let e = run(&json!("/bin/ls"), &json!(5), Some("/tmp"), &json!({})).await.unwrap_err();
		assert_eq!(e, "The \"args\" argument must be of type object. Received type number (5)");
	}

	#[tokio::test]
	async fn args_elements_string_coerced() {
		// [5] → "5", [null] → "null" (probe-pinned: coerced, not validated).
		let r = run(&json!("/bin/ls"), &json!([5]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r.code, 2);
		assert_eq!(r.stderr, "/bin/ls: cannot access '5': No such file or directory\n");
		let r = run(&json!("/bin/ls"), &json!([null]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r.code, 2);
		assert_eq!(r.stderr, "/bin/ls: cannot access 'null': No such file or directory\n");
	}

	#[tokio::test]
	async fn no_shell() {
		// The command string is the binary path (no shell parsing).
		let r = run(&json!("/bin/sh -c 'echo hi'"), &json!([]), Some("/tmp"), &json!({})).await.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 1, killed: false });
	}

	#[tokio::test]
	async fn timeout_zero_and_negative_are_no_timeout() {
		let r = run(&json!("/bin/sleep"), &json!(["0.1"]), Some("/tmp"), &json!({ "timeout": 0 })).await.unwrap();
		assert_eq!(r.killed, false);
		let r = run(&json!("/bin/sleep"), &json!(["0.1"]), Some("/tmp"), &json!({ "timeout": -5 })).await.unwrap();
		assert_eq!(r.killed, false);
	}

	#[tokio::test]
	async fn timeout_to_number_coercion() {
		// "120" > 0 → ToNumber 120 → kills; null > 0 → 0 → no.
		let r = run(&json!("/bin/sleep"), &json!(["2"]), Some("/tmp"), &json!({ "timeout": "120" })).await.unwrap();
		assert_eq!(r.killed, true);
		let r = run(&json!("/bin/sleep"), &json!(["0.1"]), Some("/tmp"), &json!({ "timeout": null })).await.unwrap();
		assert_eq!(r.killed, false);
	}

	#[tokio::test]
	async fn enoent_with_preaborted_signal_is_killed() {
		// Missing binary + pre-aborted signal: killProcess ran (killed =
		// true) before the async 'error' event rejected → {code: 1,
		// killed: true}.
		let r = exec_command(
			&json!("/no/such/binary"),
			&json!([]),
			Some("/tmp"),
			&json!({}),
			Some(Arc::new(PreAbortedSignal)),
		)
		.await
		.unwrap();
		assert_eq!(r, ExecResult { stdout: "".into(), stderr: "".into(), code: 1, killed: true });
	}

	#[test]
	fn js_string_coercions() {
		assert_eq!(js_string(&json!(5)), "5");
		assert_eq!(js_string(&json!(null)), "null");
		assert_eq!(js_string(&json!(true)), "true");
		assert_eq!(js_string(&json!({ "a": 1 })), "[object Object]");
		assert_eq!(js_string(&json!([1, 2])), "1,2");
		assert_eq!(js_string(&json!([[1], [2, 3]])), "1,2,3");
	}

	#[test]
	fn js_to_number_coercions() {
		assert_eq!(js_to_number(&json!(null)), 0.0);
		assert_eq!(js_to_number(&json!(false)), 0.0);
		assert_eq!(js_to_number(&json!(true)), 1.0);
		assert_eq!(js_to_number(&json!(120)), 120.0);
		assert_eq!(js_to_number(&json!("120")), 120.0);
		assert_eq!(js_to_number(&json!("")), 0.0);
		assert!(js_to_number(&json!("x")).is_nan());
		assert!(js_to_number(&json!({})).is_nan());
	}
}
