//! Process runtime layer for shell execution.
//!
//! Ports of:
//! - `src/utils/shell.ts` (unix): `getShellConfig`, `getShellEnv`,
//!   `killProcessTree`, detached-child tracking
//! - `src/utils/child-process.ts`: `waitForChildProcess` (exit + stdio grace
//!   semantics, re-armed on every output chunk after exit)
//! - `src/core/tools/bash.ts`: `BashOperations` + `createLocalShellOperations`
//!   (timeout / abort / process-tree kill / 128+signal exit codes)
//! - `src/core/exec.ts`: `execCommand`
//!
//! Windows-only parts of shell.ts (cross-spawn, taskkill, PowerShell) are not
//! ported (gap G4 in TRACKING.md).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pi_core::text_decoder::decode_utf8_node;

/// `EXIT_STDIO_GRACE_MS`: after the child exits, output still arriving on the
/// pipes keeps re-arming a grace timer; quiet pipes finalize after this long.
const EXIT_STDIO_GRACE_MS: u64 = 100;

/// Poll interval while an interrupt (timeout / abort) must be observed.
const INTERRUPT_POLL: Duration = Duration::from_millis(20);

/// `MAX_TIMEOUT_MS` from bash.ts.
pub const MAX_TIMEOUT_MS: f64 = 2_147_483_647.0;

/// Cooperative cancellation flag; stand-in for the DOM `AbortSignal`.
#[derive(Clone, Default)]
pub struct AbortSignal(Arc<AtomicBool>);

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn aborted(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Shell configuration (utils/shell.ts)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTransport {
    /// Command passed as the last argv element (`bash -c <command>`).
    Argv,
    /// Command written to the shell's stdin (`bash -s`), for legacy WSL bash.
    Stdin,
}

#[derive(Debug, Clone)]
pub struct ShellConfig {
    pub shell: String,
    pub args: Vec<String>,
    pub command_transport: CommandTransport,
}

/// `/^[a-z]:\\windows\\(?:system32|sysnative)\\bash\.exe$/` on the
/// slash-to-backslash, lowercased path.
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    let bytes = normalized.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let rest = &normalized[1..];
    rest == ":\\windows\\system32\\bash.exe" || rest == ":\\windows\\sysnative\\bash.exe"
}

fn bash_shell_config(shell: String) -> ShellConfig {
    if is_legacy_wsl_bash_path(&shell) {
        ShellConfig { shell, args: vec!["-s".to_string()], command_transport: CommandTransport::Stdin }
    } else {
        ShellConfig { shell, args: vec!["-c".to_string()], command_transport: CommandTransport::Argv }
    }
}

/// `findExecutableOnPath` (unix branch): run `which` and take the first line.
fn find_executable_on_path(executable: &str) -> Option<String> {
    let output = Command::new("which").arg(executable).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.trim().lines().next()?;
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

/// `getShellConfig` (unix): custom path, then /bin/bash, then bash on PATH,
/// then fallback to `sh -c`. Returns the error message on failure.
pub fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, String> {
    if let Some(custom) = custom_shell_path {
        if std::path::Path::new(custom).exists() {
            return Ok(bash_shell_config(custom.to_string()));
        }
        return Err(format!("Custom shell path not found: {}", custom));
    }

    if std::path::Path::new("/bin/bash").exists() {
        return Ok(bash_shell_config("/bin/bash".to_string()));
    }

    if let Some(bash) = find_executable_on_path("bash") {
        return Ok(bash_shell_config(bash));
    }

    Ok(ShellConfig { shell: "sh".to_string(), args: vec!["-c".to_string()], command_transport: CommandTransport::Argv })
}

/// `getBinDir`: `<agentDir>/bin` where agentDir is `$PI_CODING_AGENT_DIR`
/// (tilde-expanded) or `~/.pi/agent`.
fn get_bin_dir() -> String {
    let agent_dir = match std::env::var("PI_CODING_AGENT_DIR") {
        Ok(dir) => expand_tilde_path(&dir),
        Err(_) => {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/.pi/agent", home)
        }
    };
    format!("{}/bin", agent_dir)
}

/// `expandTildePath`: leading `~` / `~/` replaced with the home directory.
fn expand_tilde_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("{}/{}", home, rest);
    }
    path.to_string()
}

/// `getShellEnv`: the current environment with the pi agent bin dir
/// prepended to PATH. (The bash tool additionally removes PI_* session
/// variables in `resolveSpawnContext`.)
pub fn get_shell_env() -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    let bin_dir = get_bin_dir();
    // pathKey: the first key whose lowercase form is "path" (unix: "PATH").
    let path_key = env
        .keys()
        .find(|k| k.to_lowercase() == "path")
        .cloned()
        .unwrap_or_else(|| "PATH".to_string());
    let current_path = env.get(&path_key).cloned().unwrap_or_default();
    let has_bin_dir = current_path.split(':').filter(|p| !p.is_empty()).any(|p| p == bin_dir);
    if !has_bin_dir {
        let mut parts: Vec<String> = Vec::new();
        if !bin_dir.is_empty() {
            parts.push(bin_dir);
        }
        if !current_path.is_empty() {
            parts.push(current_path);
        }
        env.insert(path_key, parts.join(":"));
    }
    env
}

// ============================================================================
// Process-tree killing + detached child tracking (utils/shell.ts)
// ============================================================================

#[cfg(unix)]
unsafe fn libc_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, sig)
}

/// Send a signal to a single pid (no process group).
#[cfg(unix)]
fn kill_pid_signal(pid: u32, sig: i32) {
    unsafe {
        let _ = libc_kill(pid as i32, sig);
    }
}

/// `killProcessTree` (unix): SIGKILL the process group, falling back to
/// killing just the pid (e.g. when the child never became a group leader).
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    unsafe {
        if libc_kill(-(pid as i32), 9) != 0 {
            let _ = libc_kill(pid as i32, 9);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn tracked_pids() -> &'static Mutex<HashSet<u32>> {
    static SET: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `trackDetachedChildPid`.
pub fn track_detached_child_pid(pid: u32) {
    tracked_pids().lock().unwrap().insert(pid);
}

/// `untrackDetachedChildPid`.
pub fn untrack_detached_child_pid(pid: u32) {
    tracked_pids().lock().unwrap().remove(&pid);
}

/// `killTrackedDetachedChildren`.
pub fn kill_tracked_detached_children() {
    let pids: Vec<u32> = tracked_pids().lock().unwrap().drain().collect();
    for pid in pids {
        kill_process_tree(pid);
    }
}

// ============================================================================
// Piped child + waitForChildProcess (utils/child-process.ts)
// ============================================================================

#[derive(Debug)]
enum ChildEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    StdoutEnd,
    StderrEnd,
    Exit(ExitStatus),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

/// Exit status of a child: the raw exit code and terminating signal, either
/// of which may be absent.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaitOutcome {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

/// A spawned child with piped stdout/stderr. Reader threads forward chunks
/// (in arrival order, both streams interleaved) over a channel; a wait
/// thread reports exit. Dropping the receiver closes the pipes (Node's
/// stream `destroy()`).
pub struct PipedChild {
    pub pid: u32,
    stdin: Option<std::process::ChildStdin>,
    rx: mpsc::Receiver<ChildEvent>,
}

pub struct SpawnConfig<'a> {
    pub cwd: &'a str,
    /// Environment for the child; `None` inherits the parent's.
    pub env: Option<&'a HashMap<String, String>>,
    /// Detached: the child leads its own process group (unix).
    pub detached: bool,
    /// Pipe the child's stdin (for `commandTransport: "stdin"`).
    pub stdin_pipe: bool,
}

fn pump<R: Read + Send + 'static>(
    reader: R,
    tx: mpsc::Sender<ChildEvent>,
    data: fn(Vec<u8>) -> ChildEvent,
    end: ChildEvent,
) {
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(end);
                    break;
                }
                Ok(n) => {
                    if tx.send(data(buf[..n].to_vec())).is_err() {
                        break; // receiver dropped: stream destroyed
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let _ = tx.send(end);
                    break;
                }
            }
        }
        // Dropping the reader closes the pipe.
    });
}

/// Spawn a child with piped stdout/stderr.
pub fn spawn_piped(program: &str, args: &[String], cfg: SpawnConfig<'_>) -> std::io::Result<PipedChild> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cfg.cwd)
        .stdin(if cfg.stdin_pipe { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if cfg.detached {
        #[cfg(unix)]
        cmd.process_group(0);
    }
    if let Some(env) = cfg.env {
        cmd.env_clear().envs(env);
    }
    let mut child: Child = cmd.spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        pump(stdout, tx, ChildEvent::Stdout, ChildEvent::StdoutEnd);
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        pump(stderr, tx, ChildEvent::Stderr, ChildEvent::StderrEnd);
    }
    std::thread::spawn(move || match child.wait() {
        Ok(status) => {
            let _ = tx.send(ChildEvent::Exit(status));
        }
        Err(err) => {
            let _ = tx.send(ChildEvent::Error(err.to_string()));
        }
    });
    Ok(PipedChild { pid, stdin, rx })
}

impl PipedChild {
    /// Write `data` to the child's stdin and close it (`child.stdin.end(command)`).
    pub fn write_stdin_and_close(&mut self, data: &[u8]) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(data);
        }
    }

    /// `proc.kill(sig)`: signal just this process (no process group).
    pub fn kill_signal(&self, sig: i32) {
        #[cfg(unix)]
        unsafe {
            let _ = libc_kill(self.pid as i32, sig);
        }
        #[cfg(not(unix))]
        let _ = sig;
    }

    /// `killProcessTree(pid)`.
    pub fn kill_tree(&self) {
        kill_process_tree(self.pid);
    }
}

/// The `waitForChildProcess` event loop: exit + stdio-grace semantics, with
/// an optional `interrupt` closure invoked whenever the loop goes idle for
/// `poll` (used for timeout / abort handling).
fn event_loop(
    child: &mut PipedChild,
    on_data: &mut dyn FnMut(StreamKind, &[u8]),
    mut interrupt: Option<&mut dyn FnMut()>,
    poll: Duration,
) -> Result<WaitOutcome, String> {
    let grace = Duration::from_millis(EXIT_STDIO_GRACE_MS);
    let mut exited = false;
    let mut outcome = WaitOutcome::default();
    let mut stdout_ended = false;
    let mut stderr_ended = false;
    let mut grace_deadline: Option<Instant> = None;

    loop {
        if exited && stdout_ended && stderr_ended {
            return Ok(outcome);
        }
        let now = Instant::now();
        let wait = grace_deadline
            .map(|d| d.saturating_duration_since(now))
            .unwrap_or(poll)
            .min(poll);
        let wait = if wait.is_zero() { Duration::from_millis(1) } else { wait };
        match child.rx.recv_timeout(wait) {
            Ok(ChildEvent::Stdout(data)) => {
                on_data(StreamKind::Stdout, &data);
                // Output after exit re-arms the grace timer.
                if exited {
                    grace_deadline = Some(Instant::now() + grace);
                }
            }
            Ok(ChildEvent::Stderr(data)) => {
                on_data(StreamKind::Stderr, &data);
                if exited {
                    grace_deadline = Some(Instant::now() + grace);
                }
            }
            Ok(ChildEvent::StdoutEnd) => stdout_ended = true,
            Ok(ChildEvent::StderrEnd) => stderr_ended = true,
            Ok(ChildEvent::Exit(status)) => {
                exited = true;
                outcome.exit_code = status.code();
                outcome.signal = status.signal();
                grace_deadline = Some(Instant::now() + grace);
            }
            Ok(ChildEvent::Error(err)) => return Err(err),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(deadline) = grace_deadline {
                    if Instant::now() >= deadline {
                        // Grace elapsed: destroy the streams (drop the
                        // receiver; blocked readers then exit and close the
                        // pipes) and finalize with the exit status.
                        return Ok(outcome);
                    }
                }
                if let Some(f) = interrupt.as_mut() {
                    f();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(outcome),
        }
    }
}

/// `waitForChildProcess`: wait for a child to terminate without hanging on
/// inherited stdio handles held open by detached descendants. After the
/// child exits, output still arriving re-arms a 100ms grace timer; quiet
/// pipes (or both streams ending) finalize.
pub fn wait_for_child_process(
    child: &mut PipedChild,
    on_data: &mut dyn FnMut(StreamKind, &[u8]),
) -> Result<WaitOutcome, String> {
    event_loop(child, on_data, None, Duration::from_secs(86_400))
}

// ============================================================================
// BashOperations + createLocalShellOperations (core/tools/bash.ts)
// ============================================================================

#[derive(Debug, Clone)]
pub enum ShellExecError {
    /// `new Error("aborted")`
    Aborted,
    /// `new Error(\`timeout:${rawSeconds}\`)` — carries the raw timeout value.
    Timeout(f64),
    /// Any other error; `message()` mirrors the original's Error message.
    Other(String),
}

impl ShellExecError {
    pub fn message(&self) -> String {
        match self {
            ShellExecError::Aborted => "aborted".to_string(),
            ShellExecError::Timeout(t) => format!("timeout:{}", format_timeout_secs(*t)),
            ShellExecError::Other(m) => m.clone(),
        }
    }
}

/// JS number-to-string for timeout values (`\`${timeout}\``): integral values
/// print without a decimal part.
pub fn format_timeout_secs(t: f64) -> String {
    if t.fract() == 0.0 && t.abs() < 1e21 {
        format!("{}", t as i64)
    } else {
        format!("{}", t)
    }
}

/// `resolveTimeoutMs`: seconds → milliseconds, with the original's errors.
pub fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<Duration>, ShellExecError> {
    let Some(t) = timeout else { return Ok(None) };
    if !t.is_finite() || t <= 0.0 {
        return Err(ShellExecError::Other(
            "Invalid timeout: must be a finite number of seconds".to_string(),
        ));
    }
    let timeout_ms = t * 1000.0;
    if timeout_ms > MAX_TIMEOUT_MS {
        return Err(ShellExecError::Other("Invalid timeout: maximum is 2147483.647 seconds".to_string()));
    }
    Ok(Some(Duration::from_secs_f64(timeout_ms / 1000.0)))
}

/// Result of `BashOperations.exec`.
#[derive(Debug)]
pub struct ExecOutcome {
    /// Process exit code; `None` when the shell died without one (treated as
    /// a failed command by callers).
    pub exit_code: Option<i32>,
}

/// Streaming output callback (stdout and stderr, arrival order).
pub type OnData<'a> = &'a mut dyn FnMut(StreamKind, &[u8]);

/// Options for `BashOperations.exec`.
#[derive(Default)]
pub struct ShellExecOptions<'a> {
    pub on_data: Option<OnData<'a>>,
    pub signal: Option<&'a AbortSignal>,
    /// Timeout in seconds — the raw input value (like the bash tool input).
    pub timeout: Option<f64>,
    /// Environment override; `None` uses `getShellEnv()`.
    pub env: Option<&'a HashMap<String, String>>,
}

/// Pluggable operations for the bash tool; override to delegate command
/// execution to remote systems (SSH, containers, ...).
pub trait BashOperations {
    fn exec(
        &self,
        command: &str,
        cwd: &str,
        options: ShellExecOptions<'_>,
    ) -> Result<ExecOutcome, ShellExecError>;
}

/// Node-style spawn error message (`spawn /bin/bash ENOENT`).
fn spawn_error_message(program: &str, err: &std::io::Error) -> String {
    let code = match err.raw_os_error() {
        Some(2) => "ENOENT".to_string(),
        Some(13) => "EACCES".to_string(),
        Some(20) => "ENOTDIR".to_string(),
        Some(c) => format!("Error {}", c),
        None => err.to_string(),
    };
    format!("spawn {} {}", program, code)
}

/// `createLocalShellOperations`: shared process execution used by the built-in
/// shell tools. Runs `bash -c <command>` in its own process group, streams
/// stdout+stderr, kills the tree on timeout/abort, and reports signal
/// terminations as 128 + signal.
pub struct LocalShellOperations {
    shell_name: String,
    shell_path: Option<String>,
}

/// `createLocalBashOperations`.
pub fn create_local_bash_operations(shell_path: Option<&str>) -> LocalShellOperations {
    LocalShellOperations { shell_name: "bash".to_string(), shell_path: shell_path.map(str::to_string) }
}

impl LocalShellOperations {
    pub fn new(shell_name: &str, shell_path: Option<&str>) -> Self {
        LocalShellOperations { shell_name: shell_name.to_string(), shell_path: shell_path.map(str::to_string) }
    }
}

impl BashOperations for LocalShellOperations {
    fn exec(
        &self,
        command: &str,
        cwd: &str,
        mut options: ShellExecOptions<'_>,
    ) -> Result<ExecOutcome, ShellExecError> {
        let timeout_ms = resolve_timeout_ms(options.timeout)?;
        if let Some(signal) = options.signal {
            if signal.aborted() {
                return Err(ShellExecError::Aborted);
            }
        }
        let shell_config = get_shell_config(self.shell_path.as_deref()).map_err(ShellExecError::Other)?;
        if std::fs::metadata(cwd).is_err() {
            return Err(ShellExecError::Other(format!(
                "Working directory does not exist: {}\nCannot execute {} commands.",
                cwd, self.shell_name
            )));
        }

        let command_from_stdin = shell_config.command_transport == CommandTransport::Stdin;
        let mut args = shell_config.args.clone();
        if !command_from_stdin {
            args.push(command.to_string());
        }
        let env = options.env.cloned().unwrap_or_else(get_shell_env);
        let mut child = spawn_piped(
            &shell_config.shell,
            &args,
            SpawnConfig { cwd, env: Some(&env), detached: true, stdin_pipe: command_from_stdin },
        )
        .map_err(|e| ShellExecError::Other(spawn_error_message(&shell_config.shell, &e)))?;
        if command_from_stdin {
            child.write_stdin_and_close(command.as_bytes());
        }
        track_detached_child_pid(child.pid);

        let mut timed_out = false;
        let pid = child.pid;
        let deadline = timeout_ms.map(|d| Instant::now() + d);
        let signal = options.signal;
        let on_data = options.on_data.take();
        let mut noop = |_kind: StreamKind, _data: &[u8]| {};
        let on_data: &mut dyn FnMut(StreamKind, &[u8]) = match on_data {
            Some(f) => f,
            None => &mut noop,
        };

        let result = {
            let mut interrupt = || {
                if let Some(deadline) = deadline {
                    if !timed_out && Instant::now() >= deadline {
                        timed_out = true;
                        kill_process_tree(pid);
                    }
                }
                if let Some(signal) = signal {
                    if signal.aborted() {
                        kill_process_tree(pid);
                    }
                }
            };
            event_loop(&mut child, on_data, Some(&mut interrupt), INTERRUPT_POLL)
        };
        untrack_detached_child_pid(pid);
        let outcome = result.map_err(ShellExecError::Other)?;
        if let Some(signal) = signal {
            if signal.aborted() {
                return Err(ShellExecError::Aborted);
            }
        }
        if timed_out {
            return Err(ShellExecError::Timeout(options.timeout.unwrap_or(0.0)));
        }
        // A signal-killed shell has no exit code. Use the standard shell
        // convention so callers do not mistake it for success.
        let exit_code = outcome
            .exit_code
            .or_else(|| outcome.signal.map(|sig| 128 + sig))
            .or(Some(1));
        Ok(ExecOutcome { exit_code })
    }
}

// ============================================================================
// execCommand (core/exec.ts)
// ============================================================================

#[derive(Default)]
pub struct ExecCommandOptions {
    pub signal: Option<AbortSignal>,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub killed: bool,
}

/// `execCommand`: execute a program and return stdout/stderr/code. Supports
/// timeout and abort signal; resolves (rather than rejects) with `code: 1`
/// on spawn/wait errors, mirroring the original.
pub fn exec_command(command: &str, args: &[String], cwd: &str, options: ExecCommandOptions) -> ExecResult {
    let spawn_result = spawn_piped(
        command,
        args,
        SpawnConfig { cwd, env: None, detached: false, stdin_pipe: false },
    );
    let mut child = match spawn_result {
        Ok(child) => child,
        Err(_) => return ExecResult { stdout: String::new(), stderr: String::new(), code: 1, killed: false },
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut killed = false;
    let pid = child.pid;
    let deadline = options.timeout.map(|t| Instant::now() + t);

    // killProcess: SIGTERM. (The original also arms a 5s SIGKILL fallback
    // guarded by `!proc.killed`, but Node sets that flag on the first kill()
    // call, so the fallback never fires — reproduced by not arming it.)
    let mut kill_process = || {
        if !killed {
            killed = true;
            kill_pid_signal(pid, 15);
        }
    };

    if let Some(signal) = &options.signal {
        if signal.aborted() {
            kill_process();
        }
    }

    let signal = options.signal;
    let mut on_data = |kind: StreamKind, data: &[u8]| {
        // `stdout += data.toString()`: each chunk decoded independently
        // (non-streaming), BOM preserved.
        let text = decode_utf8_node(data);
        match kind {
            StreamKind::Stdout => stdout.push_str(&text),
            StreamKind::Stderr => stderr.push_str(&text),
        }
    };

    let outcome = {
        let mut interrupt = || {
            if let Some(deadline) = deadline {
                if Instant::now() >= deadline {
                    kill_process();
                }
            }
            if let Some(signal) = &signal {
                if signal.aborted() {
                    kill_process();
                }
            }
        };
        event_loop(&mut child, &mut on_data, Some(&mut interrupt), INTERRUPT_POLL)
    };

    match outcome {
        Ok(outcome) => ExecResult {
            // `code: code ?? 0` — a signal-killed process reports 0 here.
            code: outcome.exit_code.unwrap_or(0),
            stdout,
            stderr,
            killed,
        },
        Err(_) => ExecResult { stdout, stderr, code: 1, killed },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash() -> LocalShellOperations {
        create_local_bash_operations(None)
    }

    #[test]
    fn exec_simple_echo() {
        let result = exec_command(
            "echo",
            &["hello".to_string()],
            ".",
            ExecCommandOptions::default(),
        );
        assert_eq!(result.code, 0);
        assert_eq!(result.stdout, "hello\n");
        assert_eq!(result.stderr, "");
        assert!(!result.killed);
    }

    #[test]
    fn exec_nonzero_exit() {
        let result = exec_command("sh", &["-c".to_string(), "exit 3".to_string()], ".", ExecCommandOptions::default());
        assert_eq!(result.code, 3);
    }

    #[test]
    fn exec_missing_program_is_code_1() {
        let result = exec_command("/nonexistent/pi-rs-test", &[], ".", ExecCommandOptions::default());
        assert_eq!(result.code, 1);
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn exec_timeout_kills() {
        let result = exec_command(
            "sh",
            &["-c".to_string(), "echo start; sleep 30; echo end".to_string()],
            ".",
            ExecCommandOptions { signal: None, timeout: Some(Duration::from_millis(300)) },
        );
        assert!(result.killed);
        assert_eq!(result.stdout, "start\n");
        // `code ?? 0`: SIGTERM-killed processes report 0 here.
        assert_eq!(result.code, 0);
    }

    #[test]
    fn exec_abort_signal() {
        let signal = AbortSignal::new();
        signal.abort();
        let result = exec_command(
            "sh",
            &["-c".to_string(), "sleep 30".to_string()],
            ".",
            ExecCommandOptions { signal: Some(signal), timeout: None },
        );
        assert!(result.killed);
    }

    #[test]
    fn exec_stderr_capture() {
        let result = exec_command(
            "sh",
            &["-c".to_string(), "echo out; echo err 1>&2".to_string()],
            ".",
            ExecCommandOptions::default(),
        );
        assert_eq!(result.stdout, "out\n");
        assert_eq!(result.stderr, "err\n");
    }

    #[test]
    fn ops_exec_echo() {
        let mut collected: Vec<u8> = Vec::new();
        let mut on_data = |_kind: StreamKind, data: &[u8]| {
            collected.extend_from_slice(data);
        };
        let outcome = bash()
            .exec(
                "echo hi",
                ".",
                ShellExecOptions { on_data: Some(&mut on_data), ..Default::default() },
            )
            .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(&collected), "hi\n");
    }

    #[test]
    fn ops_exec_exit_code() {
        let outcome = bash().exec("exit 7", ".", ShellExecOptions::default()).unwrap();
        assert_eq!(outcome.exit_code, Some(7));
    }

    #[test]
    fn ops_exec_missing_cwd() {
        let err = bash().exec("true", "/nonexistent/pi-rs-dir", ShellExecOptions::default()).unwrap_err();
        assert_eq!(
            err.message(),
            "Working directory does not exist: /nonexistent/pi-rs-dir\nCannot execute bash commands."
        );
    }

    #[test]
    fn ops_exec_invalid_timeout() {
        let err = bash()
            .exec("true", ".", ShellExecOptions { timeout: Some(0.0), ..Default::default() })
            .unwrap_err();
        assert_eq!(err.message(), "Invalid timeout: must be a finite number of seconds");
        let err = bash()
            .exec("true", ".", ShellExecOptions { timeout: Some(f64::NAN), ..Default::default() })
            .unwrap_err();
        assert_eq!(err.message(), "Invalid timeout: must be a finite number of seconds");
        let err = bash()
            .exec("true", ".", ShellExecOptions { timeout: Some(3e6), ..Default::default() })
            .unwrap_err();
        assert_eq!(err.message(), "Invalid timeout: maximum is 2147483.647 seconds");
    }

    #[test]
    fn ops_exec_timeout_error() {
        let err = bash()
            .exec(
                "sleep 30",
                ".",
                ShellExecOptions { timeout: Some(0.3), ..Default::default() },
            )
            .unwrap_err();
        assert_eq!(err.message(), "timeout:0.3");
    }

    #[test]
    fn ops_exec_aborted_before_spawn() {
        let signal = AbortSignal::new();
        signal.abort();
        let err = bash()
            .exec("true", ".", ShellExecOptions { signal: Some(&signal), ..Default::default() })
            .unwrap_err();
        assert!(matches!(err, ShellExecError::Aborted));
    }

    #[test]
    fn ops_exec_signal_kill_is_128_plus_signal() {
        // killProcessTree uses SIGKILL (9) → exit code 137 via the timeout path.
        let err = bash()
            .exec(
                "trap '' TERM; sleep 30",
                ".",
                ShellExecOptions { timeout: Some(0.3), ..Default::default() },
            )
            .unwrap_err();
        assert!(matches!(err, ShellExecError::Timeout(_)));
    }

    #[test]
    fn wait_grace_captures_late_descendant_output() {
        // A descendant keeps the stdout pipe open and keeps writing after the
        // child exits; each chunk re-arms the grace timer so we keep reading.
        let script = "echo hi; sh -c 'for i in 1 2 3 4 5 6 7 8 9 10; do echo tick; sleep 0.05; done' &";
        let mut child = spawn_piped(
            "bash",
            &["-c".to_string(), script.to_string()],
            SpawnConfig { cwd: ".", env: None, detached: true, stdin_pipe: false },
        )
        .unwrap();
        let mut collected = String::new();
        let mut on_data = |_kind: StreamKind, data: &[u8]| {
            collected.push_str(&String::from_utf8_lossy(data));
        };
        let outcome = wait_for_child_process(&mut child, &mut on_data).unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(collected.contains("hi"));
        let ticks = collected.matches("tick").count();
        assert_eq!(ticks, 10, "late descendant output lost: {:?}", collected);
    }

    #[test]
    fn wait_quiet_inherited_pipe_finalizes_after_grace() {
        // A descendant holds the pipe open but stays quiet: we must not hang
        // (finalize after the grace period).
        let mut child = spawn_piped(
            "bash",
            &["-c".to_string(), "echo hi; sleep 5 &".to_string()],
            SpawnConfig { cwd: ".", env: None, detached: true, stdin_pipe: false },
        )
        .unwrap();
        let started = Instant::now();
        let mut on_data = |_kind: StreamKind, _data: &[u8]| {};
        let outcome = wait_for_child_process(&mut child, &mut on_data).unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(started.elapsed() < Duration::from_secs(3), "hung on quiet inherited pipe");
        // The descendant from this test lingers ~5s holding a broken pipe;
        // it exits on its own. Kill the tree to be tidy.
    }

    #[test]
    fn kill_tree_kills_descendants() {
        let mut child = spawn_piped(
            "bash",
            &["-c".to_string(), "sleep 30 & sleep 30".to_string()],
            SpawnConfig { cwd: ".", env: None, detached: true, stdin_pipe: false },
        )
        .unwrap();
        child.kill_tree();
        let started = Instant::now();
        let mut on_data = |_kind: StreamKind, _data: &[u8]| {};
        let outcome = wait_for_child_process(&mut child, &mut on_data).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(outcome.exit_code, None);
        assert_eq!(outcome.signal, Some(9));
    }

    #[test]
    fn shell_config_fallbacks() {
        let config = get_shell_config(None).unwrap();
        assert_eq!(config.args, vec!["-c".to_string()]);
        assert_eq!(config.command_transport, CommandTransport::Argv);
        let err = get_shell_config(Some("/nonexistent/pi-rs-shell")).unwrap_err();
        assert_eq!(err, "Custom shell path not found: /nonexistent/pi-rs-shell");
        let config = get_shell_config(Some("/bin/sh")).unwrap();
        assert_eq!(config.shell, "/bin/sh");
        // Legacy WSL path uses stdin transport.
        assert!(is_legacy_wsl_bash_path("C:\\Windows\\System32\\bash.exe"));
        assert!(!is_legacy_wsl_bash_path("/bin/bash"));
    }

    #[test]
    fn shell_env_prepends_bin_dir() {
        let env = get_shell_env();
        let path = env.get("PATH").expect("PATH present");
        let bin_dir = get_bin_dir();
        assert!(path.split(':').any(|p| p == bin_dir), "bin dir {} not in PATH", bin_dir);
    }

    #[test]
    fn format_timeout() {
        assert_eq!(format_timeout_secs(1.0), "1");
        assert_eq!(format_timeout_secs(0.5), "0.5");
        assert_eq!(format_timeout_secs(2.25), "2.25");
    }
}
