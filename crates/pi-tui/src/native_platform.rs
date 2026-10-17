//! Port of `native-platform.ts` — platform-gated native clipboard helpers.
//!
//! The TS module loads N-API `.node` prebuilds for macOS/Windows/Linux. The
//! Rust port follows the gap decisions in `docs/TUI_PORT_PLAN.md`:
//!
//! * **T3** — no N-API `.node` binary is shipped. Module loading is ported
//!   as pure logic with an injectable loader (the production default is "no
//!   modules"), and the Linux native clipboard behavior is the coding
//!   agent's external-command fallback: wl-paste/wl-copy (Wayland) and
//!   xclip/xsel (X11) — see `packages/coding-agent/src/utils/clipboard.ts`
//!   and `clipboard-image.ts` for the command/flag details. The command
//!   runner ports the safety rails of `clipboard-command.ts`: writers get
//!   no stdout pipe (clipboard writers daemonize and would retain one),
//!   stdin write errors are ignored, stalled commands are killed at a
//!   deadline, and stdout is capped.
//! * **T4** — win32/darwin native helpers are out of scope and cleanly
//!   unavailable (`None`), so `enableVirtualTerminalInput` is not ported.
//!
//! Upstream semantics preserved as pure logic: platform gating
//! (`getNativePlatformHelper` only for darwin/win32; `getNativeClipboard`
//! on linux only when `DISPLAY` is set, with the `-x11` module suffix
//! concept), arch gating (x64/arm64 only), and module-load caching per
//! native path *including cached failures* — a disconnected display can
//! recover because availability is re-checked, module loading is not.
//!
//! Environment access, module loading, and command execution are injectable
//! so the gating/caching/fallback-selection behavior is testable without a
//! display.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::native_module_path::{get_native_module_candidates, NativeModuleCandidateOptions};

/// Modifier keys queryable through a native platform helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierKey {
    Shift,
    Command,
    Control,
    Option,
}

/// Clipboard text read: `None` means unavailable, `Some(None)` means the
/// clipboard holds no text. (TS Promise rejections for transfer failures
/// collapse into `None` in this sync, std-only port.)
pub type ClipboardText = Option<Option<String>>;
/// Image bytes read, using the same convention as [`ClipboardText`].
pub type ClipboardImage = Option<Option<Vec<u8>>>;
/// POSIX paths of file URLs: `None` means unsupported, `Some(None)` means
/// no files, `Some(Some(paths))` means file paths.
pub type ClipboardFilePaths = Option<Option<Vec<String>>>;

/// Port of the `NativeClipboard` interface.
pub trait NativeClipboard: Send + Sync {
    /// `None` means unavailable, `Some(None)` means no text.
    fn get_text(&self) -> ClipboardText;
    /// `None` means unavailable, `Some(None)` means no image.
    fn get_image(&self) -> ClipboardImage;
    /// POSIX paths of file URLs on the clipboard. `None` means unsupported.
    fn get_file_paths(&self) -> ClipboardFilePaths {
        None
    }
    /// Linux uses command-line tools to retain clipboard ownership instead.
    fn set_text(&self, _text: &str) -> Option<()> {
        None
    }
}

/// Port of the `NativePlatformHelper` type: a clipboard plus the optional
/// native extras. `enableVirtualTerminalInput` is win32-only and out of
/// scope (T4); `is_modifier_pressed` models the optional TS method with
/// `None` = absent.
pub trait NativePlatformHelper: NativeClipboard {
    fn is_modifier_pressed(&self, _key: ModifierKey) -> Option<bool> {
        None
    }
}

/// Only x64/arm64 prebuilds exist upstream.
pub fn is_supported_arch(arch: &str) -> bool {
    arch == "x64" || arch == "arm64"
}

/// Injectable process state behind the gating logic: TS `process.platform`,
/// `process.arch`, `process.env`, and module loading.
pub struct ProcessContext<'a> {
    pub platform: &'a str,
    pub arch: &'a str,
    pub get_env: &'a dyn Fn(&str) -> Option<String>,
    pub load_module: &'a dyn Fn(&str) -> Option<Arc<dyn NativePlatformHelper>>,
}

/// `std::env::consts::OS` → the Node `process.platform` spelling (macOS is
/// "macos" in Rust and "darwin" in Node, Windows is "windows"/"win32").
fn platform_name(os: &str) -> &str {
    match os {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn real_platform() -> &'static str {
    platform_name(std::env::consts::OS)
}

fn real_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn real_get_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// TS truthiness on `process.env.X` (`if (env.DISPLAY)`): only a non-empty
/// value counts as set — `DISPLAY=""` is a headless session.
fn env_set(value: Option<String>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// T3/T4: the Rust port ships no N-API modules, so nothing ever loads.
fn no_native_module(_module_path: &str) -> Option<Arc<dyn NativePlatformHelper>> {
    None
}

impl ProcessContext<'static> {
    /// The real process. Per gap decisions T3/T4 the default module loader
    /// is "no modules".
    pub fn real() -> Self {
        Self {
            platform: real_platform(),
            arch: real_arch(),
            get_env: &real_get_env,
            load_module: &no_native_module,
        }
    }
}

fn as_clipboard(helper: Arc<dyn NativePlatformHelper>) -> Arc<dyn NativeClipboard> {
    helper
}

/// Module-load cache plus the Linux fallback clipboard factory.
///
/// Caches module loading, not display availability: a disconnected display
/// can recover (availability is re-checked on every
/// [`NativePlatformRegistry::get_native_clipboard`] call) while a failed or
/// successful module load is never retried for the same native path.
#[derive(Default)]
pub struct NativePlatformRegistry {
    helpers: Mutex<HashMap<String, Option<Arc<dyn NativePlatformHelper>>>>,
    linux_fallback: Option<Box<dyn Fn(&ProcessContext) -> Arc<dyn NativeClipboard> + Send + Sync>>,
}

impl NativePlatformRegistry {
    /// Production registry: module loads are attempted and cached
    /// (including failures), and Linux falls back to the external-command
    /// clipboard (T3). The fallback is built from the caller's
    /// [`ProcessContext`], so injected environments flow into it.
    pub fn new() -> Self {
        Self::with_linux_fallback_factory(real_linux_fallback)
    }

    /// Registry with an explicit Linux fallback factory, so tests can route
    /// the fallback through an injected environment and command runner.
    pub fn with_linux_fallback_factory(
        factory: impl Fn(&ProcessContext) -> Arc<dyn NativeClipboard> + Send + Sync + 'static,
    ) -> Self {
        Self {
            linux_fallback: Some(Box::new(factory)),
            ..Default::default()
        }
    }

    /// Test registry with the Linux command fallback disabled, exposing the
    /// pure upstream gating semantics (`undefined` when no module loads).
    pub fn without_linux_fallback() -> Self {
        Self::default()
    }

    /// Port of `loadNativePlatformHelper`: arch gating, native path
    /// computation (`native/<platform>/prebuilds/<platform>-<arch>/
    /// <platform>-platform<suffix>.node`), candidate iteration, and caching
    /// of both successes and failures per native path.
    pub fn load_platform_helper(
        &self,
        ctx: &ProcessContext,
        platform: &str,
        suffix: &str,
    ) -> Option<Arc<dyn NativePlatformHelper>> {
        if !is_supported_arch(ctx.arch) {
            return None;
        }
        let native_path = format!(
            "native/{platform}/prebuilds/{platform}-{arch}/{platform}-platform{suffix}.node",
            platform = platform,
            arch = ctx.arch,
            suffix = suffix,
        );
        if let Some(cached) = self.helpers.lock().unwrap().get(&native_path).cloned() {
            return cached;
        }
        let mut loaded = None;
        for module_path in
            get_native_module_candidates(&native_path, NativeModuleCandidateOptions::default())
        {
            // Try the next possible packaging location on failure.
            if let Some(helper) = (ctx.load_module)(&module_path) {
                loaded = Some(helper);
                break;
            }
        }
        self.helpers.lock().unwrap().insert(native_path, loaded.clone());
        loaded
    }

    /// Port of `getNativePlatformHelper`: only darwin/win32 have a helper.
    pub fn get_platform_helper(&self, ctx: &ProcessContext) -> Option<Arc<dyn NativePlatformHelper>> {
        if ctx.platform != "darwin" && ctx.platform != "win32" {
            return None;
        }
        self.load_platform_helper(ctx, ctx.platform, "")
    }

    /// Port of `getNativeClipboard`: load a clipboard helper without opening
    /// the display until a read is requested.
    ///
    /// On non-Linux platforms this is the platform helper (per T4, always
    /// `None` in production). On Linux the X11 helper loads only when
    /// `DISPLAY` is set (`-x11` suffix), as in native-platform.ts; when no
    /// module is available the external-command fallback provides the
    /// native behavior (T3). Unlike the module attempt, the fallback also
    /// serves Wayland-only sessions (`WAYLAND_DISPLAY` set, `DISPLAY`
    /// unset) — upstream's coding agent runs wl-paste/wl-copy whenever
    /// `WAYLAND_DISPLAY` is set, independent of `getNativeClipboard`
    /// (clipboard.ts:64,109).
    pub fn get_native_clipboard(&self, ctx: &ProcessContext) -> Option<Arc<dyn NativeClipboard>> {
        if ctx.platform != "linux" {
            return self.get_platform_helper(ctx).map(as_clipboard);
        }
        let display = env_set((ctx.get_env)("DISPLAY"));
        let wayland = env_set((ctx.get_env)("WAYLAND_DISPLAY"));
        if !display && !wayland {
            return None;
        }
        if display {
            if let Some(helper) = self.load_platform_helper(ctx, "linux", "-x11") {
                return Some(as_clipboard(helper));
            }
        }
        self.linux_fallback.as_ref().map(|make| make(ctx))
    }
}

/// Snapshot the clipboard-gating environment through a [`ProcessContext`].
/// The context cannot outlive the `get_native_clipboard` call, so the
/// fallback clipboard answers its environment queries from this snapshot —
/// like the coding agent, which consults `process.env` once per clipboard
/// operation while building its command list.
fn snapshot_env(ctx: &ProcessContext, names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .filter_map(|name| (ctx.get_env)(name).map(|value| ((*name).to_string(), value)))
        .collect()
}

/// The production Linux fallback: the coding agent's command clipboard,
/// driven by the caller's environment snapshot and real child processes.
fn real_linux_fallback(ctx: &ProcessContext) -> Arc<dyn NativeClipboard> {
    let vars = snapshot_env(ctx, &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"]);
    let clipboard: Arc<dyn NativeClipboard> = Arc::new(LinuxCommandClipboard::new(
        move |name| {
            vars.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        },
        run_clipboard_command,
    ));
    clipboard
}

fn global_registry() -> &'static NativePlatformRegistry {
    static REGISTRY: OnceLock<NativePlatformRegistry> = OnceLock::new();
    REGISTRY.get_or_init(NativePlatformRegistry::new)
}

/// Port of `getNativePlatformHelper` for the real process.
pub fn get_native_platform_helper() -> Option<Arc<dyn NativePlatformHelper>> {
    global_registry().get_platform_helper(&ProcessContext::real())
}

/// Port of `getNativeClipboard` for the real process.
pub fn get_native_clipboard() -> Option<Arc<dyn NativeClipboard>> {
    global_registry().get_native_clipboard(&ProcessContext::real())
}

/// Image mime types the coding agent prefers, in priority order.
const SUPPORTED_IMAGE_MIME_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// Port of `baseMimeType` from `clipboard-image.ts`.
fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

/// Port of `selectPreferredImageMimeType` from `clipboard-image.ts`.
fn select_preferred_image_mime_type(mime_types: &[String]) -> Option<String> {
    let normalized: Vec<(&str, String)> = mime_types
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(|t| (t, base_mime_type(t)))
        .collect();
    for preferred in SUPPORTED_IMAGE_MIME_TYPES {
        if let Some((raw, _)) = normalized.iter().find(|(_, base)| base == preferred) {
            return Some((*raw).to_string());
        }
    }
    normalized
        .iter()
        .find(|(_, base)| base.starts_with("image/"))
        .map(|(raw, _)| (*raw).to_string())
}

fn parse_type_list(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn data_to_image(data: Vec<u8>) -> ClipboardImage {
    if data.is_empty() {
        Some(None)
    } else {
        Some(Some(data))
    }
}

fn bytes_to_text(bytes: Vec<u8>) -> ClipboardText {
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if text.is_empty() {
        Some(None)
    } else {
        Some(Some(text))
    }
}

/// Timeout for the synchronous command fallback. Mirrors the native C
/// module's `CLIPBOARD_TIMEOUT_MS` (linux-platform-x11.c:10); the TS
/// command runner defaults to 3 s with callers passing 5 s.
const CLIPBOARD_TIMEOUT_MS: u64 = 2000;
/// Cap on collected stdout, matching `maxBufferBytes` in
/// `clipboard-command.ts`.
const CLIPBOARD_MAX_STDOUT_BYTES: usize = 50 * 1024 * 1024;
/// Poll cadence of the synchronous wait loop.
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The stdio wiring requested for a spawned clipboard command, mirroring
/// `clipboard-command.ts`: writers (input provided) get `Stdio::null()`
/// stdout because clipboard writers daemonize — the forked selection owner
/// would retain a stdout pipe forever, so a writer's success is judged by
/// its exit status alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CommandStdio {
    stdin_piped: bool,
    stdout_piped: bool,
}

/// Result of a non-blocking stdout poll.
enum StdoutPoll {
    /// Data was appended (or none was ready yet); more may follow.
    Data,
    /// The stdout pipe closed (EOF or read error).
    Eof,
    /// The collected output exceeded the cap.
    OverCap,
}

/// Injectable seam over one running clipboard command: stdin writes,
/// incremental stdout reads, exit polling, and termination. The production
/// implementation wraps `std::process`; tests simulate hung, fast, and
/// oversized commands without real subprocesses.
trait CommandProcess {
    /// Write the payload and close stdin. Write errors — EPIPE when the
    /// writer exits before consuming all input — are deliberately ignored
    /// (upstream swallows stdin `error` events, clipboard-command.ts:41).
    fn write_stdin(&mut self, input: &str);
    /// Append available stdout to `buffer`, reporting [`StdoutPoll::OverCap`]
    /// once `buffer` grows past `cap`.
    fn poll_stdout(&mut self, buffer: &mut Vec<u8>, cap: usize) -> StdoutPoll;
    /// Non-blocking exit poll: `Some(success)` once the process has exited
    /// (and been reaped).
    fn try_wait(&mut self) -> Option<bool>;
    /// Terminate immediately (SIGKILL semantics) and reap.
    fn kill(&mut self);
    /// Blocking reap after an exit; `false` when the wait itself fails.
    fn wait(&mut self) -> bool;
}

/// Injectable spawn seam behind [`run_command_via`].
trait CommandSpawner {
    fn spawn(
        &self,
        command: &str,
        args: &[&str],
        stdio: CommandStdio,
    ) -> Option<Box<dyn CommandProcess>>;
}

/// Port of `runClipboardCommand`: run a clipboard command synchronously
/// with the upstream safety rails — no stdout pipe for writers, stdin write
/// errors ignored, a kill-on-deadline timeout, and a stdout cap. `None`
/// means the command is missing, fails, times out, or overflows the cap.
fn run_command_via(
    spawner: &dyn CommandSpawner,
    command: &str,
    args: &[&str],
    input: Option<&str>,
    timeout: Duration,
    max_stdout: usize,
) -> Option<Vec<u8>> {
    let stdio = CommandStdio {
        stdin_piped: input.is_some(),
        stdout_piped: input.is_none(),
    };
    let mut child = spawner.spawn(command, args, stdio)?;
    if let Some(text) = input {
        // A writer may exit before consuming all input; the exit status is
        // the only verdict.
        child.write_stdin(text);
    }
    let deadline = Instant::now() + timeout;
    let mut stdout = Vec::new();
    // Writers have no pipe to wait for; readers are done at EOF.
    let mut stdout_eof = !stdio.stdout_piped;
    let mut exited = None;
    loop {
        if !stdout_eof {
            match child.poll_stdout(&mut stdout, max_stdout) {
                StdoutPoll::OverCap => {
                    child.kill();
                    return None;
                }
                StdoutPoll::Eof => stdout_eof = true,
                StdoutPoll::Data => {}
            }
        }
        if exited.is_none() {
            exited = child.try_wait();
        }
        if let Some(_exited) = exited {
            if stdout_eof {
                // `try_wait` already reported the exit; `wait` both reaps
                // the child portably and gives the authoritative status.
                let success = child.wait();
                return if success { Some(stdout) } else { None };
            }
        }
        if Instant::now() >= deadline {
            // A stalled backend (dead compositor, wedged X server) must not
            // freeze the synchronous caller: SIGKILL, reap, fail.
            child.kill();
            return None;
        }
        std::thread::sleep(COMMAND_POLL_INTERVAL);
    }
}

/// The production spawn: real child processes through `std::process`.
struct RealCommandSpawner;

impl CommandSpawner for RealCommandSpawner {
    fn spawn(
        &self,
        command: &str,
        args: &[&str],
        stdio: CommandStdio,
    ) -> Option<Box<dyn CommandProcess>> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::sync::mpsc;
        let mut child = Command::new(command)
            .args(args)
            .stdin(if stdio.stdin_piped {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            // Clipboard writers daemonize; do not give them output pipes
            // to retain (clipboard-command.ts:10-12).
            .stdout(if stdio.stdout_piped {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take();
        let (chunks_tx, chunks_rx) = mpsc::channel();
        if let Some(mut stdout) = child.stdout.take() {
            // Drain stdout on a helper thread: a blocking read on the
            // caller's thread could not observe the deadline, and the cap
            // must be checked while the child is alive. Killing the child
            // closes the pipe and ends the thread.
            std::thread::spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) | Err(_) => break, // EOF or a broken pipe ends the read.
                        Ok(n) => {
                            if chunks_tx.send(chunk[..n].to_vec()).is_err() {
                                break; // The driver is gone.
                            }
                        }
                    }
                }
            });
        }
        Some(Box::new(RealCommandProcess {
            child,
            stdin,
            chunks_rx,
        }))
    }
}

struct RealCommandProcess {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    chunks_rx: std::sync::mpsc::Receiver<Vec<u8>>,
}

impl CommandProcess for RealCommandProcess {
    fn write_stdin(&mut self, input: &str) {
        use std::io::Write;
        if let Some(mut stdin) = self.stdin.take() {
            // EPIPE is expected when the writer exits early; upstream
            // ignores stdin errors, so the exit status is the only verdict.
            let _ = stdin.write_all(input.as_bytes());
            // Dropping the handle closes the pipe so readers see EOF.
        }
    }

    fn poll_stdout(&mut self, buffer: &mut Vec<u8>, cap: usize) -> StdoutPoll {
        use std::sync::mpsc::RecvTimeoutError;
        match self.chunks_rx.recv_timeout(COMMAND_POLL_INTERVAL) {
            Ok(chunk) => {
                buffer.extend_from_slice(&chunk);
                if buffer.len() > cap {
                    StdoutPoll::OverCap
                } else {
                    StdoutPoll::Data
                }
            }
            Err(RecvTimeoutError::Timeout) => StdoutPoll::Data,
            Err(RecvTimeoutError::Disconnected) => StdoutPoll::Eof,
        }
    }

    fn try_wait(&mut self) -> Option<bool> {
        self.child.try_wait().ok().flatten().map(|status| status.success())
    }

    fn kill(&mut self) {
        // Always reap so a killed child never lingers as a zombie.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn wait(&mut self) -> bool {
        self.child
            .wait()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

/// Real command execution: `None` when the command is missing, fails,
/// times out, or overflows the stdout cap (TS `runClipboardCommand`
/// returning `undefined`).
fn run_clipboard_command(command: &str, args: &[&str], input: Option<&str>) -> Option<Vec<u8>> {
    run_command_via(
        &RealCommandSpawner,
        command,
        args,
        input,
        Duration::from_millis(CLIPBOARD_TIMEOUT_MS),
        CLIPBOARD_MAX_STDOUT_BYTES,
    )
}

/// Linux native clipboard behavior per gap decision T3: the coding agent's
/// external-command fallbacks. Wayland uses wl-paste/wl-copy, X11 uses
/// xclip then xsel; each tool is tried in turn until one succeeds.
///
/// Like upstream, the environment is consulted lazily at read/write time so
/// a disconnected display can recover, and `setText` exists because the
/// Linux tools retain clipboard ownership after the call returns.
pub struct LinuxCommandClipboard {
    get_env: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
    run_command: Box<dyn Fn(&str, &[&str], Option<&str>) -> Option<Vec<u8>> + Send + Sync>,
}

impl LinuxCommandClipboard {
    /// Build a clipboard from injected environment access and command
    /// execution.
    pub fn new(
        get_env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
        run_command: impl Fn(&str, &[&str], Option<&str>) -> Option<Vec<u8>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            get_env: Box::new(get_env),
            run_command: Box::new(run_command),
        }
    }

    /// The production clipboard: real environment and child processes.
    pub fn real() -> Self {
        Self::new(real_get_env, run_clipboard_command)
    }

    fn env(&self, name: &str) -> Option<String> {
        (self.get_env)(name)
    }

    fn run(&self, command: &str, args: &[&str], input: Option<&str>) -> Option<Vec<u8>> {
        (self.run_command)(command, args, input)
    }

    /// Port of `isWaylandSession` from `clipboard-image.ts` (the image path
    /// also honors `XDG_SESSION_TYPE`; the text paths use only
    /// `WAYLAND_DISPLAY`, as in `clipboard.ts`).
    fn is_wayland_session(&self) -> bool {
        env_set(self.env("WAYLAND_DISPLAY"))
            || self.env("XDG_SESSION_TYPE").as_deref() == Some("wayland")
    }

    /// Wayland image read (`readClipboardImageViaWlPaste`): `None` (a
    /// failed backend) falls through to X11, while `Some(None)` (an empty
    /// or text-only clipboard) must not expose stale X11 contents.
    fn image_via_wl_paste(&self) -> ClipboardImage {
        let list = self.run("wl-paste", &["--list-types"], None)?;
        let types = parse_type_list(&list);
        let Some(selected) = select_preferred_image_mime_type(&types) else {
            return Some(None);
        };
        let data = match self.run("wl-paste", &["--type", &selected, "--no-newline"], None) {
            Some(data) => data,
            None => return None,
        };
        data_to_image(data)
    }

    /// X11 image read (`readClipboardImageViaXclip`).
    fn image_via_xclip(&self) -> ClipboardImage {
        let targets = self.run("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"], None)?;
        let types = parse_type_list(&targets);
        let Some(preferred) = select_preferred_image_mime_type(&types) else {
            return Some(None);
        };
        let data = self.run("xclip", &["-selection", "clipboard", "-t", &preferred, "-o"], None)?;
        data_to_image(data)
    }
}

impl NativeClipboard for LinuxCommandClipboard {
    /// Port of the Linux command list in `readClipboardText`.
    fn get_text(&self) -> ClipboardText {
        if env_set(self.env("WAYLAND_DISPLAY")) {
            if let Some(bytes) = self.run("wl-paste", &["--no-newline", "--type", "text"], None) {
                return bytes_to_text(bytes);
            }
        }
        if env_set(self.env("DISPLAY")) {
            if let Some(bytes) = self.run("xclip", &["-selection", "clipboard", "-out"], None) {
                return bytes_to_text(bytes);
            }
            if let Some(bytes) = self.run("xsel", &["--clipboard", "--output"], None) {
                return bytes_to_text(bytes);
            }
        }
        None
    }

    fn get_image(&self) -> ClipboardImage {
        if self.is_wayland_session() {
            let wayland = self.image_via_wl_paste();
            if wayland.is_some() {
                return wayland;
            }
        }
        if env_set(self.env("DISPLAY")) {
            return self.image_via_xclip();
        }
        None
    }

    /// POSIX paths of copied file URLs are a native (macOS Finder) feature;
    /// the command fallback does not support them.
    fn get_file_paths(&self) -> ClipboardFilePaths {
        None
    }

    /// Port of the Linux command list in `copyToClipboard`.
    fn set_text(&self, text: &str) -> Option<()> {
        if env_set(self.env("WAYLAND_DISPLAY")) {
            if self.run("wl-copy", &[], Some(text)).is_some() {
                return Some(());
            }
        }
        if env_set(self.env("DISPLAY")) {
            if self.run("xclip", &["-selection", "clipboard"], Some(text)).is_some() {
                return Some(());
            }
            if self.run("xsel", &["--clipboard", "--input"], Some(text)).is_some() {
                return Some(());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Mock of the X11 native module from the upstream Linux test.
    #[derive(Default)]
    struct MockHelper {
        available: AtomicBool,
        text_calls: AtomicUsize,
    }

    impl NativeClipboard for MockHelper {
        fn get_text(&self) -> ClipboardText {
            self.text_calls.fetch_add(1, Ordering::SeqCst);
            if self.available.load(Ordering::SeqCst) {
                Some(Some("X11".to_string()))
            } else {
                None // Undefined means unavailable.
            }
        }

        fn get_image(&self) -> ClipboardImage {
            Some(None)
        }
    }

    impl NativePlatformHelper for MockHelper {}

    fn test_env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        move |name| {
            vars.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn counting_loader(
        helper: Arc<MockHelper>,
        calls: &Arc<AtomicUsize>,
        enabled: &Arc<AtomicBool>,
    ) -> impl Fn(&str) -> Option<Arc<dyn NativePlatformHelper>> {
        let helper: Arc<dyn NativePlatformHelper> = helper;
        let calls = Arc::clone(calls);
        let enabled = Arc::clone(enabled);
        move |_module_path| {
            calls.fetch_add(1, Ordering::SeqCst);
            if enabled.load(Ordering::SeqCst) {
                Some(Arc::clone(&helper))
            } else {
                None
            }
        }
    }

    fn never_loads(_module_path: &str) -> Option<Arc<dyn NativePlatformHelper>> {
        None
    }

    #[test]
    fn arch_gating_rejects_unsupported_architectures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let enabled = Arc::new(AtomicBool::new(true));
        let loader = counting_loader(Arc::new(MockHelper::default()), &calls, &enabled);
        let env = test_env(&[]);
        for arch in ["ia32", "arm", "riscv64", "x86", "wasm32"] {
            let ctx = ProcessContext {
                platform: "darwin",
                arch,
                get_env: &env,
                load_module: &loader,
            };
            assert!(
                NativePlatformRegistry::without_linux_fallback()
                    .get_platform_helper(&ctx)
                    .is_none(),
                "arch {arch}"
            );
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no module may load for unsupported architectures"
        );
        for arch in ["x64", "arm64"] {
            let ctx = ProcessContext {
                platform: "darwin",
                arch,
                get_env: &env,
                load_module: &loader,
            };
            assert!(
                NativePlatformRegistry::without_linux_fallback()
                    .get_platform_helper(&ctx)
                    .is_some(),
                "arch {arch}"
            );
        }
    }

    #[test]
    fn platform_helper_is_gated_to_darwin_and_win32() {
        let calls = Arc::new(AtomicUsize::new(0));
        let enabled = Arc::new(AtomicBool::new(true));
        let loader = counting_loader(Arc::new(MockHelper::default()), &calls, &enabled);
        let env = test_env(&[]);
        let registry = NativePlatformRegistry::without_linux_fallback();
        for platform in ["linux", "freebsd", "android", "sunos"] {
            let ctx = ProcessContext {
                platform,
                arch: "x64",
                get_env: &env,
                load_module: &loader,
            };
            assert!(
                registry.get_platform_helper(&ctx).is_none(),
                "platform {platform}"
            );
            assert!(
                registry.get_native_clipboard(&ctx).is_none(),
                "clipboard for platform {platform}"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        for platform in ["darwin", "win32"] {
            let ctx = ProcessContext {
                platform,
                arch: "arm64",
                get_env: &env,
                load_module: &loader,
            };
            assert!(
                registry.get_platform_helper(&ctx).is_some(),
                "platform {platform}"
            );
        }
    }

    /// Upstream: "uses the native platform helper directly as the clipboard
    /// API" (darwin/win32 + supported arch).
    #[test]
    fn clipboard_on_darwin_is_the_cached_platform_helper() {
        let calls = Arc::new(AtomicUsize::new(0));
        let enabled = Arc::new(AtomicBool::new(true));
        let loader = counting_loader(Arc::new(MockHelper::default()), &calls, &enabled);
        let env = test_env(&[]);
        let ctx = ProcessContext {
            platform: "darwin",
            arch: "x64",
            get_env: &env,
            load_module: &loader,
        };
        let registry = NativePlatformRegistry::without_linux_fallback();
        let clipboard = registry.get_native_clipboard(&ctx).expect("helper clipboard");
        let helper = registry.get_platform_helper(&ctx).expect("platform helper");
        let helper_as_clipboard: Arc<dyn NativeClipboard> = helper;
        assert!(Arc::ptr_eq(&clipboard, &helper_as_clipboard));
        assert!(Arc::ptr_eq(
            &clipboard,
            &registry.get_native_clipboard(&ctx).unwrap()
        ));
    }

    /// Upstream: "Linux loads X11 lazily and rechecks DISPLAY".
    #[test]
    fn linux_clipboard_requires_display_and_rechecks_it() {
        let helper = Arc::new(MockHelper::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let enabled = Arc::new(AtomicBool::new(true));
        let loader = counting_loader(Arc::clone(&helper), &calls, &enabled);
        let registry = NativePlatformRegistry::without_linux_fallback();

        // Wayland-only session: the native module is X11-only (Wayland
        // paste uses wl-paste).
        let wayland = test_env(&[("WAYLAND_DISPLAY", "wayland-0")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &wayland,
            load_module: &loader,
        };
        assert!(registry.get_native_clipboard(&ctx).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no module load without DISPLAY");

        let display = test_env(&[("DISPLAY", ":0")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &display,
            load_module: &loader,
        };
        let clipboard = registry.get_native_clipboard(&ctx).expect("X11 clipboard");
        assert_eq!(helper.text_calls.load(Ordering::SeqCst), 0, "loads X11 lazily");
        assert_eq!(clipboard.get_text(), None, "unavailable until the display answers");
        helper.available.store(true, Ordering::SeqCst);
        assert_eq!(clipboard.get_text(), Some(Some("X11".to_string())));
        assert_eq!(clipboard.get_image(), Some(None));

        // Display availability is rechecked; module loading is not.
        let disconnected = test_env(&[]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &disconnected,
            load_module: &loader,
        };
        assert!(registry.get_native_clipboard(&ctx).is_none());
        let recovered = test_env(&[("DISPLAY", ":1")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &recovered,
            load_module: &loader,
        };
        let again = registry.get_native_clipboard(&ctx).expect("recovered clipboard");
        assert!(Arc::ptr_eq(&clipboard, &again));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the module loads exactly once");
    }

    /// Cache module loading, not display availability: failures are cached
    /// so the loader is never retried for the same native path.
    #[test]
    fn module_load_failures_are_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let enabled = Arc::new(AtomicBool::new(false));
        let loader = counting_loader(Arc::new(MockHelper::default()), &calls, &enabled);
        let env = test_env(&[]);
        let ctx = ProcessContext {
            platform: "darwin",
            arch: "arm64",
            get_env: &env,
            load_module: &loader,
        };
        let registry = NativePlatformRegistry::without_linux_fallback();

        assert!(registry.get_platform_helper(&ctx).is_none());
        let after_first = calls.load(Ordering::SeqCst);
        assert!(after_first > 0, "every candidate is tried");
        // The failure is cached: no retry, even once the loader recovers.
        assert!(registry.get_platform_helper(&ctx).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), after_first);
        enabled.store(true, Ordering::SeqCst);
        assert!(registry.get_platform_helper(&ctx).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), after_first);

        // A fresh registry loads the now-available module once and reuses it.
        let fresh = NativePlatformRegistry::without_linux_fallback();
        let first = fresh.get_platform_helper(&ctx).expect("loaded module");
        let second = fresh.get_platform_helper(&ctx).expect("cached module");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(calls.load(Ordering::SeqCst), after_first + 1, "the first candidate wins");
    }

    /// T3: with no N-API module available, Linux uses the external-command
    /// fallback — but only while `DISPLAY` is set.
    #[test]
    fn default_registry_uses_linux_command_fallback() {
        let display = test_env(&[("DISPLAY", ":0")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &display,
            load_module: &never_loads,
        };
        let registry = NativePlatformRegistry::new();
        assert!(registry.get_native_clipboard(&ctx).is_some());
        let headless = test_env(&[]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &headless,
            load_module: &never_loads,
        };
        assert!(
            registry.get_native_clipboard(&ctx).is_none(),
            "DISPLAY gates the fallback"
        );
    }

    /// Real-process entry point: T3/T4 mean no native modules exist, so the
    /// helper is unavailable (on darwin/win32 the loader is empty; on Linux
    /// `getNativePlatformHelper` is undefined by the platform gate).
    #[test]
    fn real_process_has_no_platform_helper() {
        let _guard = crate::test_support::global_test_lock();
        assert!(get_native_platform_helper().is_none());
    }

    /// Records `(command, args, stdin)` invocations and answers from a
    /// `"command arg..."` keyed response table (`None` = failure).
    fn fake_runner(
        responses: &[(&str, Option<&[u8]>)],
    ) -> (
        Arc<Mutex<Vec<(String, Vec<String>, Option<String>)>>>,
        impl Fn(&str, &[&str], Option<&str>) -> Option<Vec<u8>>,
    ) {
        let invocations: Arc<Mutex<Vec<(String, Vec<String>, Option<String>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let responses: Vec<(String, Option<Vec<u8>>)> = responses
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.map(<[u8]>::to_vec)))
            .collect();
        let recorder = Arc::clone(&invocations);
        let run = move |command: &str, args: &[&str], input: Option<&str>| {
            recorder.lock().unwrap().push((
                command.to_string(),
                args.iter().map(|arg| (*arg).to_string()).collect(),
                input.map(str::to_string),
            ));
            let key = std::iter::once(command)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            responses
                .iter()
                .find(|(k, _)| *k == key)
                .and_then(|(_, value)| value.clone())
        };
        (invocations, run)
    }

    fn command_clipboard(
        env: &[(&str, &str)],
        responses: &[(&str, Option<&[u8]>)],
    ) -> (
        LinuxCommandClipboard,
        Arc<Mutex<Vec<(String, Vec<String>, Option<String>)>>>,
    ) {
        let (invocations, run) = fake_runner(responses);
        let env = test_env(env);
        (LinuxCommandClipboard::new(env, run), invocations)
    }

    fn assert_invocations(
        invocations: &Arc<Mutex<Vec<(String, Vec<String>, Option<String>)>>>,
        expected: &[(&str, &str, Option<&str>)],
    ) {
        let recorded = invocations.lock().unwrap().clone();
        let expected: Vec<(String, Vec<String>, Option<String>)> = expected
            .iter()
            .map(|(command, args, input)| {
                (
                    (*command).to_string(),
                    args.split_whitespace().map(str::to_string).collect(),
                    input.map(str::to_string),
                )
            })
            .collect();
        assert_eq!(recorded, expected);
    }

    #[test]
    fn command_clipboard_reads_text_preferring_wayland() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
            &[("wl-paste --no-newline --type text", Some(b"hello".as_slice()))],
        );
        assert_eq!(clipboard.get_text(), Some(Some("hello".to_string())));
        assert_invocations(
            &invocations,
            &[("wl-paste", "--no-newline --type text", None)],
        );
    }

    #[test]
    fn command_clipboard_falls_back_from_wayland_to_x11_text() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
            &[("xclip -selection clipboard -out", Some(b"via xclip".as_slice()))],
        );
        assert_eq!(clipboard.get_text(), Some(Some("via xclip".to_string())));
        assert_invocations(
            &invocations,
            &[
                ("wl-paste", "--no-newline --type text", None),
                ("xclip", "-selection clipboard -out", None),
            ],
        );
    }

    #[test]
    fn command_clipboard_falls_back_to_xsel_text() {
        let (clipboard, invocations) = command_clipboard(
            &[("DISPLAY", ":0")],
            &[("xsel --clipboard --output", Some(b"via xsel".as_slice()))],
        );
        assert_eq!(clipboard.get_text(), Some(Some("via xsel".to_string())));
        assert_invocations(
            &invocations,
            &[
                ("xclip", "-selection clipboard -out", None),
                ("xsel", "--clipboard --output", None),
            ],
        );
    }

    #[test]
    fn command_clipboard_empty_text_is_no_text() {
        let (clipboard, _) = command_clipboard(
            &[("DISPLAY", ":0")],
            &[("xclip -selection clipboard -out", Some(b"".as_slice()))],
        );
        assert_eq!(clipboard.get_text(), Some(None));
    }

    #[test]
    fn command_clipboard_without_a_display_is_unavailable() {
        let (clipboard, invocations) = command_clipboard(&[], &[]);
        assert_eq!(clipboard.get_text(), None);
        assert_eq!(clipboard.get_image(), None);
        assert_eq!(clipboard.set_text("text"), None);
        assert_eq!(
            clipboard.get_file_paths(),
            None,
            "POSIX file paths are unsupported"
        );
        assert!(invocations.lock().unwrap().is_empty());
    }

    #[test]
    fn command_clipboard_writes_text_preferring_wayland() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0")],
            &[("wl-copy", Some(b"".as_slice()))],
        );
        assert_eq!(clipboard.set_text("clipboard text"), Some(()));
        assert_invocations(&invocations, &[("wl-copy", "", Some("clipboard text"))]);
    }

    #[test]
    fn command_clipboard_writes_text_through_x11_fallbacks() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
            &[("xsel --clipboard --input", Some(b"".as_slice()))],
        );
        assert_eq!(clipboard.set_text("x11 text"), Some(()));
        assert_invocations(
            &invocations,
            &[
                ("wl-copy", "", Some("x11 text")),
                ("xclip", "-selection clipboard", Some("x11 text")),
                ("xsel", "--clipboard --input", Some("x11 text")),
            ],
        );
    }

    #[test]
    fn command_clipboard_writes_text_on_x11_only_sessions() {
        let (clipboard, invocations) = command_clipboard(
            &[("DISPLAY", ":0")],
            &[("xclip -selection clipboard", Some(b"".as_slice()))],
        );
        assert_eq!(clipboard.set_text("x11 only"), Some(()));
        assert_invocations(
            &invocations,
            &[("xclip", "-selection clipboard", Some("x11 only"))],
        );
    }

    #[test]
    fn command_clipboard_reads_wayland_image_by_preferred_type() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0")],
            &[
                ("wl-paste --list-types", Some(b"text/plain\nimage/png\n".as_slice())),
                ("wl-paste --type image/png --no-newline", Some(&[1, 2, 3])),
            ],
        );
        assert_eq!(clipboard.get_image(), Some(Some(vec![1, 2, 3])));
        assert_invocations(
            &invocations,
            &[
                ("wl-paste", "--list-types", None),
                ("wl-paste", "--type image/png --no-newline", None),
            ],
        );
    }

    #[test]
    fn command_clipboard_prefers_png_over_other_image_types() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0")],
            &[
                (
                    "wl-paste --list-types",
                    Some(b"image/webp\nimage/png\nimage/jpeg\n".as_slice()),
                ),
                ("wl-paste --type image/png --no-newline", Some(&[7])),
            ],
        );
        assert_eq!(clipboard.get_image(), Some(Some(vec![7])));
        assert_invocations(
            &invocations,
            &[
                ("wl-paste", "--list-types", None),
                ("wl-paste", "--type image/png --no-newline", None),
            ],
        );
    }

    /// An empty Wayland clipboard must not fall through to stale X11
    /// contents.
    #[test]
    fn command_clipboard_reports_empty_wayland_clipboards_as_no_image() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
            &[("wl-paste --list-types", Some(b"\n".as_slice()))],
        );
        assert_eq!(clipboard.get_image(), Some(None));
        assert_invocations(&invocations, &[("wl-paste", "--list-types", None)]);
    }

    #[test]
    fn command_clipboard_reports_text_only_wayland_clipboards_as_no_image() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
            &[("wl-paste --list-types", Some(b"text/plain".as_slice()))],
        );
        assert_eq!(clipboard.get_image(), Some(None));
        assert_invocations(&invocations, &[("wl-paste", "--list-types", None)]);
    }

    /// A failed Wayland backend (unlike an empty one) falls through to X11.
    #[test]
    fn command_clipboard_falls_back_from_wayland_to_xclip_images() {
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
            &[
                (
                    "xclip -selection clipboard -t TARGETS -o",
                    Some(b"image/jpeg\n".as_slice()),
                ),
                ("xclip -selection clipboard -t image/jpeg -o", Some(&[9, 9])),
            ],
        );
        assert_eq!(clipboard.get_image(), Some(Some(vec![9, 9])));
        assert_invocations(
            &invocations,
            &[
                ("wl-paste", "--list-types", None),
                ("xclip", "-selection clipboard -t TARGETS -o", None),
                ("xclip", "-selection clipboard -t image/jpeg -o", None),
            ],
        );
    }

    #[test]
    fn command_clipboard_reports_x11_text_only_clipboards_as_no_image() {
        let (clipboard, invocations) = command_clipboard(
            &[("DISPLAY", ":0")],
            &[(
                "xclip -selection clipboard -t TARGETS -o",
                Some(b"text/plain\n".as_slice()),
            )],
        );
        assert_eq!(clipboard.get_image(), Some(None));
        assert_invocations(
            &invocations,
            &[("xclip", "-selection clipboard -t TARGETS -o", None)],
        );
    }

    #[test]
    fn command_clipboard_accepts_any_image_type_on_x11() {
        let (clipboard, invocations) = command_clipboard(
            &[("DISPLAY", ":0")],
            &[
                (
                    "xclip -selection clipboard -t TARGETS -o",
                    Some(b"image/tiff\n".as_slice()),
                ),
                ("xclip -selection clipboard -t image/tiff -o", Some(&[5])),
            ],
        );
        assert_eq!(clipboard.get_image(), Some(Some(vec![5])));
        assert_invocations(
            &invocations,
            &[
                ("xclip", "-selection clipboard -t TARGETS -o", None),
                ("xclip", "-selection clipboard -t image/tiff -o", None),
            ],
        );
    }

    #[test]
    fn command_clipboard_reports_empty_x11_images_as_no_image() {
        let (clipboard, invocations) = command_clipboard(
            &[("DISPLAY", ":0")],
            &[
                (
                    "xclip -selection clipboard -t TARGETS -o",
                    Some(b"image/png\n".as_slice()),
                ),
                ("xclip -selection clipboard -t image/png -o", Some(b"".as_slice())),
            ],
        );
        assert_eq!(clipboard.get_image(), Some(None));
        assert_invocations(
            &invocations,
            &[
                ("xclip", "-selection clipboard -t TARGETS -o", None),
                ("xclip", "-selection clipboard -t image/png -o", None),
            ],
        );
    }

    /// The image path also honors `XDG_SESSION_TYPE=wayland`
    /// (`isWaylandSession` in `clipboard-image.ts`).
    #[test]
    fn command_clipboard_uses_xdg_session_type_for_images() {
        let (clipboard, invocations) = command_clipboard(
            &[("XDG_SESSION_TYPE", "wayland"), ("DISPLAY", ":0")],
            &[
                ("wl-paste --list-types", Some(b"image/png\n".as_slice())),
                ("wl-paste --type image/png --no-newline", Some(&[3])),
            ],
        );
        assert_eq!(clipboard.get_image(), Some(Some(vec![3])));
        assert_invocations(
            &invocations,
            &[
                ("wl-paste", "--list-types", None),
                ("wl-paste", "--type image/png --no-newline", None),
            ],
        );
    }

    #[test]
    fn preferred_image_mime_type_selection() {
        assert_eq!(
            select_preferred_image_mime_type(&["image/png".to_string()]),
            Some("image/png".to_string())
        );
        // Priority order: png > jpeg > webp > gif.
        assert_eq!(
            select_preferred_image_mime_type(&[
                "image/gif".to_string(),
                "image/webp".to_string(),
                "image/jpeg".to_string(),
            ]),
            Some("image/jpeg".to_string())
        );
        // The raw (unnormalized) type is requested back.
        assert_eq!(
            select_preferred_image_mime_type(&["image/png; charset=utf-8".to_string()]),
            Some("image/png; charset=utf-8".to_string())
        );
        // Any image type, then no image at all.
        assert_eq!(
            select_preferred_image_mime_type(&["text/plain".to_string(), "image/x-ms-bmp".to_string()]),
            Some("image/x-ms-bmp".to_string())
        );
        assert_eq!(
            select_preferred_image_mime_type(&["text/plain".to_string(), "UTF8_STRING".to_string()]),
            None
        );
        assert_eq!(select_preferred_image_mime_type(&[]), None);
    }

    // ---- `runClipboardCommand` safety rails (command runner seam) ----

    /// Shared state of one fake child process, inspectable after the run.
    #[derive(Default)]
    struct FakeProcessState {
        /// Queued stdout items: `Some(chunk)` is data, `None` marks EOF.
        chunks: VecDeque<Option<Vec<u8>>>,
        /// Simulates a daemonizing fork that never releases the pipe.
        hang_stdout: bool,
        /// Exit status once "exited"; `None` keeps running until killed.
        exit: Option<bool>,
        killed: bool,
        stdin: Option<String>,
    }

    struct FakeProcess {
        state: Arc<Mutex<FakeProcessState>>,
    }

    impl CommandProcess for FakeProcess {
        fn write_stdin(&mut self, input: &str) {
            self.state.lock().unwrap().stdin = Some(input.to_string());
        }

        fn poll_stdout(&mut self, buffer: &mut Vec<u8>, cap: usize) -> StdoutPoll {
            let mut state = self.state.lock().unwrap();
            if state.hang_stdout {
                return StdoutPoll::Data;
            }
            match state.chunks.pop_front() {
                Some(Some(chunk)) => {
                    buffer.extend_from_slice(&chunk);
                    if buffer.len() > cap {
                        StdoutPoll::OverCap
                    } else {
                        StdoutPoll::Data
                    }
                }
                Some(None) => StdoutPoll::Eof,
                None => StdoutPoll::Data, // waiting for more output
            }
        }

        fn try_wait(&mut self) -> Option<bool> {
            let state = self.state.lock().unwrap();
            if state.killed {
                Some(false)
            } else {
                state.exit
            }
        }

        fn kill(&mut self) {
            self.state.lock().unwrap().killed = true;
        }

        fn wait(&mut self) -> bool {
            let state = self.state.lock().unwrap();
            !state.killed && state.exit.unwrap_or(false)
        }
    }

    /// Spawns [`FakeProcess`]es and records the stdio wiring each spawn
    /// requested.
    struct FakeSpawner {
        state: Arc<Mutex<FakeProcessState>>,
        spawns: Arc<Mutex<Vec<(String, Vec<String>, CommandStdio)>>>,
    }

    impl FakeSpawner {
        fn new(state: FakeProcessState) -> (Self, Arc<Mutex<FakeProcessState>>) {
            let state = Arc::new(Mutex::new(state));
            let spawner = Self {
                spawns: Arc::new(Mutex::new(Vec::new())),
                state: Arc::clone(&state),
            };
            (spawner, state)
        }
    }

    impl CommandSpawner for FakeSpawner {
        fn spawn(
            &self,
            command: &str,
            args: &[&str],
            stdio: CommandStdio,
        ) -> Option<Box<dyn CommandProcess>> {
            self.spawns.lock().unwrap().push((
                command.to_string(),
                args.iter().map(|arg| (*arg).to_string()).collect(),
                stdio,
            ));
            Some(Box::new(FakeProcess {
                state: Arc::clone(&self.state),
            }))
        }
    }

    /// [BUG-HIGH regression guard] xclip/xsel/wl-copy fork a background
    /// selection owner that would retain a stdout pipe forever: writer
    /// invocations must get `Stdio::null()` stdout and be judged by exit
    /// status, so even a "hanging" stdout returns promptly.
    #[test]
    fn writer_commands_get_no_stdout_pipe_and_return_promptly() {
        let (spawner, state) = FakeSpawner::new(FakeProcessState {
            hang_stdout: true, // the daemonized fork never closes the pipe
            exit: Some(true),  // the spawning parent exits 0
            ..FakeProcessState::default()
        });
        let started = Instant::now();
        let output = run_command_via(
            &spawner,
            "xclip",
            &["-selection", "clipboard"],
            Some("copied"),
            Duration::from_millis(CLIPBOARD_TIMEOUT_MS),
            CLIPBOARD_MAX_STDOUT_BYTES,
        );
        assert_eq!(output, Some(Vec::new()));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a daemonizing writer must not stall the caller"
        );
        let spawns = spawner.spawns.lock().unwrap();
        assert_eq!(spawns.len(), 1);
        assert!(spawns[0].2.stdin_piped);
        assert!(!spawns[0].2.stdout_piped, "writers get Stdio::null() stdout");
        assert_eq!(state.lock().unwrap().stdin.as_deref(), Some("copied"));
    }

    /// [BUG-MED] A writer may exit before consuming all input (EPIPE on
    /// stdin); upstream ignores stdin errors, so the command still succeeds
    /// by exit status.
    #[test]
    fn writer_stdin_errors_do_not_fail_the_command() {
        let (spawner, state) = FakeSpawner::new(FakeProcessState {
            exit: Some(true), // exits immediately, never reads stdin
            ..FakeProcessState::default()
        });
        let output = run_command_via(
            &spawner,
            "wl-copy",
            &[],
            Some("text the writer never reads"),
            Duration::from_millis(CLIPBOARD_TIMEOUT_MS),
            CLIPBOARD_MAX_STDOUT_BYTES,
        );
        assert_eq!(output, Some(Vec::new()));
        assert_eq!(
            state.lock().unwrap().stdin.as_deref(),
            Some("text the writer never reads"),
            "the input is still handed to stdin"
        );
    }

    /// [MISSING] A stalled backend (dead compositor, wedged X server) is
    /// SIGKILLed at the deadline instead of freezing the caller forever.
    #[test]
    fn stalled_commands_are_killed_at_the_deadline() {
        let (spawner, state) = FakeSpawner::new(FakeProcessState {
            hang_stdout: true, // never EOFs
            exit: None,        // never exits
            ..FakeProcessState::default()
        });
        let started = Instant::now();
        let output = run_command_via(
            &spawner,
            "wl-paste",
            &["--no-newline", "--type", "text"],
            None,
            Duration::from_millis(60),
            CLIPBOARD_MAX_STDOUT_BYTES,
        );
        assert_eq!(output, None);
        assert!(state.lock().unwrap().killed, "the stalled child is killed");
        assert!(
            started.elapsed() < Duration::from_millis(2000),
            "the deadline bounds the wait"
        );
    }

    /// [MISSING] Stdout past the cap (upstream `maxBufferBytes`, 50 MB)
    /// aborts the command.
    #[test]
    fn oversized_stdout_aborts_the_command() {
        assert_eq!(CLIPBOARD_MAX_STDOUT_BYTES, 50 * 1024 * 1024);
        let (spawner, state) = FakeSpawner::new(FakeProcessState {
            chunks: VecDeque::from(vec![Some(vec![0u8; 4096]), Some(vec![0u8; 4096])]),
            exit: None,
            ..FakeProcessState::default()
        });
        let output = run_command_via(
            &spawner,
            "wl-paste",
            &["--type", "image/png", "--no-newline"],
            None,
            Duration::from_secs(5),
            4096,
        );
        assert_eq!(output, None);
        assert!(state.lock().unwrap().killed, "the overflowing child is killed");
    }

    /// The fast path: reader stdout is collected in order up to EOF, and
    /// the exit status decides success.
    #[test]
    fn reader_output_is_collected_until_eof() {
        let (spawner, _state) = FakeSpawner::new(FakeProcessState {
            chunks: VecDeque::from(vec![
                Some(b"clip".to_vec()),
                Some(b"board".to_vec()),
                None, // EOF
            ]),
            exit: Some(true),
            ..FakeProcessState::default()
        });
        let output = run_command_via(
            &spawner,
            "wl-paste",
            &["--no-newline", "--type", "text"],
            None,
            Duration::from_millis(CLIPBOARD_TIMEOUT_MS),
            CLIPBOARD_MAX_STDOUT_BYTES,
        );
        assert_eq!(output, Some(b"clipboard".to_vec()));
    }

    /// [BUG-MED] Wayland-only sessions (`WAYLAND_DISPLAY` set, `DISPLAY`
    /// unset) still get the command fallback; the X11 native-module attempt
    /// stays DISPLAY-gated as in native-platform.ts.
    #[test]
    fn linux_fallback_serves_wayland_only_sessions() {
        let calls = Arc::new(AtomicUsize::new(0));
        let enabled = Arc::new(AtomicBool::new(true));
        let loader = counting_loader(Arc::new(MockHelper::default()), &calls, &enabled);
        let wayland = test_env(&[("WAYLAND_DISPLAY", "wayland-0")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &wayland,
            load_module: &loader,
        };
        let registry = NativePlatformRegistry::new();
        let clipboard = registry
            .get_native_clipboard(&ctx)
            .expect("the command fallback serves Wayland-only sessions");
        // Only a command-free use: get_text would run the real wl-paste.
        assert_eq!(clipboard.get_file_paths(), None);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the X11 module is not attempted without DISPLAY"
        );
    }

    /// [MISSING-LOW] The registry fallback is built from the caller's
    /// `ProcessContext`, so injected environments flow into the fallback
    /// clipboard instead of the real one.
    #[test]
    fn registry_fallback_honors_the_injected_process_context() {
        let invocations: Arc<Mutex<Vec<(String, Vec<String>, Option<String>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&invocations);
        let registry = NativePlatformRegistry::with_linux_fallback_factory(move |ctx| {
            let vars = snapshot_env(ctx, &["DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"]);
            let recorded = Arc::clone(&recorded);
            let clipboard: Arc<dyn NativeClipboard> = Arc::new(LinuxCommandClipboard::new(
                move |name| {
                    vars.iter()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.clone())
                },
                move |command: &str, args: &[&str], input: Option<&str>| {
                    recorded.lock().unwrap().push((
                        command.to_string(),
                        args.iter().map(|arg| (*arg).to_string()).collect(),
                        input.map(str::to_string),
                    ));
                    if command == "wl-paste" {
                        Some(b"from the injected ctx".to_vec())
                    } else {
                        None
                    }
                },
            ));
            clipboard
        });
        let wayland = test_env(&[("WAYLAND_DISPLAY", "wayland-0")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &wayland,
            load_module: &never_loads,
        };
        let clipboard = registry.get_native_clipboard(&ctx).expect("fallback clipboard");
        assert_eq!(
            clipboard.get_text(),
            Some(Some("from the injected ctx".to_string()))
        );
        assert_eq!(
            invocations.lock().unwrap().len(),
            1,
            "the injected WAYLAND_DISPLAY routed the read to wl-paste"
        );
    }

    /// [BUG-LOW] TS truthiness: `DISPLAY=""`/`WAYLAND_DISPLAY=""` count as
    /// unset everywhere they gate.
    #[test]
    fn empty_display_variables_count_as_unset() {
        // The registry gate: no clipboard at all on empty variables.
        let empty = test_env(&[("DISPLAY", ""), ("WAYLAND_DISPLAY", "")]);
        let ctx = ProcessContext {
            platform: "linux",
            arch: "x64",
            get_env: &empty,
            load_module: &never_loads,
        };
        assert!(NativePlatformRegistry::new().get_native_clipboard(&ctx).is_none());

        // The command clipboard: no tool may run on empty variables.
        let (clipboard, invocations) =
            command_clipboard(&[("DISPLAY", ""), ("WAYLAND_DISPLAY", "")], &[]);
        assert_eq!(clipboard.get_text(), None);
        assert_eq!(clipboard.set_text("text"), None);
        assert_eq!(clipboard.get_image(), None);
        assert!(invocations.lock().unwrap().is_empty());

        // An empty WAYLAND_DISPLAY must not trigger wl-paste while X11 lives.
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", ""), ("DISPLAY", ":0")],
            &[("xclip -selection clipboard -out", Some(b"x11 only".as_slice()))],
        );
        assert_eq!(clipboard.get_text(), Some(Some("x11 only".to_string())));
        assert_invocations(&invocations, &[("xclip", "-selection clipboard -out", None)]);

        // The image path's `isWaylandSession` also ignores an empty variable.
        let (clipboard, invocations) = command_clipboard(
            &[("WAYLAND_DISPLAY", ""), ("XDG_SESSION_TYPE", ""), ("DISPLAY", ":0")],
            &[(
                "xclip -selection clipboard -t TARGETS -o",
                Some(b"text/plain".as_slice()),
            )],
        );
        assert_eq!(clipboard.get_image(), Some(None));
        assert_invocations(
            &invocations,
            &[("xclip", "-selection clipboard -t TARGETS -o", None)],
        );
    }

    /// [BUG-LOW, latent] `std::env::consts::OS` spells macOS "macos" where
    /// Node's `process.platform` says "darwin".
    #[test]
    fn macos_maps_to_the_darwin_platform() {
        assert_eq!(platform_name("macos"), "darwin");
        assert_eq!(platform_name("windows"), "win32");
        assert_eq!(platform_name("linux"), "linux");
    }

    /// Real-subprocess smoke test of the production runner (POSIX `sh`).
    /// Ignored by default: the suite must not execute external programs on
    /// the happy path.
    #[test]
    #[ignore = "spawns real subprocesses; run explicitly with cargo test -- --ignored"]
    fn real_command_runner_smoke_test() {
        // Reader: stdout is collected up to EOF.
        assert_eq!(
            run_clipboard_command("sh", &["-c", "printf hello"], None),
            Some(b"hello".to_vec())
        );
        // Writer: no stdout pipe, success is the exit status, and a stdin
        // EPIPE (the writer exits without reading) does not fail it.
        assert_eq!(
            run_clipboard_command("sh", &["-c", "exit 0"], Some("never read")),
            Some(Vec::new())
        );
        // A missing command is a clean failure.
        assert_eq!(
            run_clipboard_command("pi-tui-missing-command-0f3a", &[], None),
            None
        );
    }
}
