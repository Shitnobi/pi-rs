//! Port of `terminal.ts` — the `Terminal` interface, `ProcessTerminal`, and
//! the Kitty keyboard-protocol negotiation state machine.
//!
//! Rust port of `packages/tui/src/terminal.ts` (1:1 behavior).
//!
//! ## Timing model (divergence from the TS original)
//!
//! The TS original drives everything with real timers: `setTimeout` for the
//! 150 ms negotiation-fragment flush, `setInterval` for the 1000 ms progress
//! keepalive, `Date.now()` polling for `drainInput`, and `StdinBuffer`'s
//! internal input timeouts. This port keeps a *virtual clock* inside
//! [`ProcessTerminal`] instead: deadlines are recorded when the TS original
//! would schedule a timer, and the runtime owner advances the clock with
//! [`ProcessTerminal::tick`], which fires every due deadline. The tests call
//! `tick` directly, mirroring the upstream mocked-timer tests
//! (`mock.timers.tick(...)`).
//!
//! ## I/O injection
//!
//! The upstream tests mock `process.stdout.write` / `process.stdin.on`. This
//! port takes an injectable [`TerminalIo`] layer ([`ProcessIo`] is the real
//! `process.stdin`/`process.stdout` path). The tests use a recording
//! implementation and feed input through [`ProcessTerminal::handle_stdin_data`]
//! — the equivalent of invoking the captured `process.stdin` `data` listener.
//! On the real path the runtime owner reads stdin and calls
//! `handle_stdin_data` / `tick` (this port spawns no threads).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::keys::{env_get, set_kitty_protocol_active};
use crate::program_status::{
    format_program_status, is_program_status_reply, ProgramState, PROGRAM_STATUS_QUERY,
};
pub use crate::program_status::ProgramStatus;
use crate::stdin_buffer::{units_to_string, StdinBuffer, StdinBufferEvent, StdinBufferOptions};
use crate::utils::is_js_whitespace;

const TERMINAL_PROGRESS_KEEPALIVE_MS: u64 = 1000;
const TERMINAL_PROGRESS_ACTIVE_SEQUENCE: &str = "\x1b]9;4;3\x07";
const TERMINAL_PROGRESS_CLEAR_SEQUENCE: &str = "\x1b]9;4;0\x07";
const NATIVE_SHIFT_ENTER_SEQUENCE: &str = "\x1b[13;2u";
/// 1 = disambiguate escape codes, 2 = report event types, 4 = report
/// alternate keys.
pub const DESIRED_KITTY_KEYBOARD_PROTOCOL_FLAGS: u64 = 7;
const KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS: u64 = 150;
const KITTY_KEYBOARD_PROTOCOL_QUERY: &str = "\x1b[>7u\x1b[?u";
const DEVICE_ATTRIBUTES_QUERY: &str = "\x1b[c";

pub const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
pub const DEFAULT_SSH_ESCAPE_TIMEOUT_MS: u64 = 100;
/// Default `drainInput` budget (TS `maxMs`).
pub const DEFAULT_DRAIN_MAX_MS: u64 = 1000;
/// Default `drainInput` idle window (TS `idleMs`).
pub const DEFAULT_DRAIN_IDLE_MS: u64 = 50;

// =============================================================================
// Keyboard protocol negotiation replies
// =============================================================================

/// A parsed terminal reply to the Kitty keyboard protocol negotiation
/// queries (`KeyboardProtocolNegotiationSequence` in the TS original).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardProtocolNegotiationSequence {
    /// `\x1b[?<flags>u` — the currently active Kitty flags.
    KittyFlags { flags: u64 },
    /// `\x1b[?[...digits/semicolons...]c` — a DA1 reply (the query sentinel).
    DeviceAttributes,
}

/// Port of `parseKeyboardProtocolNegotiationSequence`:
/// `/^\x1b\[\?(\d+)u$/` → kitty-flags, `/^\x1b\[\?[\d;]*c$/` → DA.
pub fn parse_keyboard_protocol_negotiation_sequence(
    sequence: &str,
) -> Option<KeyboardProtocolNegotiationSequence> {
    let bytes = sequence.as_bytes();
    if !bytes.starts_with(b"\x1b[?") {
        return None;
    }
    // /^\\x1b\\[\\?(\\d+)u$/
    if let Some(digits) = sequence
        .strip_prefix("\x1b[?")
        .and_then(|rest| rest.strip_suffix('u'))
    {
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            let mut flags: u64 = 0;
            for byte in digits.bytes() {
                flags = flags.saturating_mul(10).saturating_add((byte - b'0') as u64);
            }
            return Some(KeyboardProtocolNegotiationSequence::KittyFlags { flags });
        }
    }
    // /^\\x1b\\[\\?[\\d;]*c$/ (the middle may be empty)
    if bytes.ends_with(b"c") {
        let middle = &bytes[3..bytes.len() - 1];
        if middle.iter().all(|&b| b.is_ascii_digit() || b == b';') {
            return Some(KeyboardProtocolNegotiationSequence::DeviceAttributes);
        }
    }
    None
}

/// `sequence === "\x1b[" || /^\x1b\[\?[\d;]*$/` — the input so far could still
/// become a negotiation reply, so it is buffered briefly.
fn is_keyboard_protocol_negotiation_sequence_prefix(sequence: &str) -> bool {
    if sequence == "\x1b[" {
        return true;
    }
    match sequence.strip_prefix("\x1b[?") {
        Some(rest) => rest.bytes().all(|b| b.is_ascii_digit() || b == b';'),
        None => false,
    }
}

// =============================================================================
// Platform / env helpers
// =============================================================================

/// Whether this is a macOS Apple Terminal session
/// (`process.platform === "darwin" && TERM_PROGRAM === "Apple_Terminal"`).
pub fn is_apple_terminal_session() -> bool {
    is_apple_terminal_session_with(|name| env_get(name))
}

/// Injectable form of [`is_apple_terminal_session`].
pub fn is_apple_terminal_session_with(getenv: impl Fn(&str) -> Option<String>) -> bool {
    cfg!(target_os = "macos") && getenv("TERM_PROGRAM").as_deref() == Some("Apple_Terminal")
}

/// Refresh terminal dimensions on POSIX platforms by sending SIGWINCH to
/// this process. Best-effort: some environments (restricted seccomp or LSM
/// policies) return EACCES for `kill(2)`; in that case the dimensions
/// refresh is skipped rather than crashing.
///
/// Divergence: the signal externs are declared for Linux only; on other
/// platforms this is a no-op.
pub fn refresh_terminal_dimensions() {
    if cfg!(windows) || std::process::id() == 0 {
        return;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = linux_ffi::kill(std::process::id() as i32, linux_ffi::SIGWINCH);
    }
    #[cfg(not(target_os = "linux"))]
    {}
}

/// Rewrite Return as the CSI-u Shift+Enter sequence when native Shift
/// detection is enabled and Shift is pressed (Apple Terminal / Win32 only).
pub fn normalize_native_shift_enter_input(
    data: &str,
    should_detect_native_shift_enter: bool,
    is_shift_pressed: bool,
) -> String {
    if should_detect_native_shift_enter && data == "\r" && is_shift_pressed {
        NATIVE_SHIFT_ENTER_SEQUENCE.to_string()
    } else {
        data.to_string()
    }
}

/// Apple Terminal wrapper around [`normalize_native_shift_enter_input`].
pub fn normalize_apple_terminal_input(
    data: &str,
    is_apple_terminal: bool,
    is_shift_pressed: bool,
) -> String {
    normalize_native_shift_enter_input(data, is_apple_terminal, is_shift_pressed)
}

/// Native modifier query for the Shift+Enter workaround, delegating to the
/// ported `native-modifiers` module (platform helper lookup; unavailable
/// helpers — including every platform in this port — answer `false`,
/// mirroring the TS fallback).
fn is_native_modifier_pressed(modifier: &str) -> bool {
    match modifier {
        "shift" => crate::native_modifiers::is_native_modifier_pressed(
            crate::native_platform::ModifierKey::Shift,
        ),
        _ => false,
    }
}

/// How long to wait for the rest of an escape sequence before dispatching a
/// lone ESC as the Escape key. Legacy Alt+key input is ESC plus another
/// byte, so high-latency transports need a longer reassembly window.
pub fn resolve_escape_timeout_ms() -> u64 {
    resolve_escape_timeout_ms_with(|name| env_get(name))
}

/// Injectable form of [`resolve_escape_timeout_ms`] (the TS original takes
/// the env object; here it is a lookup closure). `PI_TUI_ESC_TIMEOUT`
/// (finite && > 0) wins, then SSH sessions get 100 ms, otherwise 10 ms.
pub fn resolve_escape_timeout_ms_with(getenv: impl Fn(&str) -> Option<String>) -> u64 {
    if let Some(value) = getenv("PI_TUI_ESC_TIMEOUT") {
        if let Some(configured) = js_number(&value) {
            if configured.is_finite() && configured > 0.0 {
                // The TS returns the fractional value verbatim (e.g. `0.5`);
                // this u64 API rounds up so a sub-millisecond window still
                // dispatches a lone ESC instead of never timing out.
                return configured.ceil() as u64;
            }
        }
    }
    // JS truthiness: an empty-string SSH variable is unset.
    if getenv("SSH_CONNECTION").is_some_and(|v| !v.is_empty())
        || getenv("SSH_TTY").is_some_and(|v| !v.is_empty())
    {
        return DEFAULT_SSH_ESCAPE_TIMEOUT_MS;
    }
    DEFAULT_ESCAPE_TIMEOUT_MS
}

/// JS `Number(string)`: `None` models `NaN`. Handles whitespace trimming,
/// signs, hex (`0x`), and `Infinity`; rejects Rust-only spellings like
/// `"inf"`/`"NaN"` that JS would treat as NaN.
fn js_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_matches(is_js_whitespace);
    if trimmed.is_empty() {
        return Some(0.0);
    }
    let (sign, rest) = if let Some(rest) = trimmed.strip_prefix('-') {
        (-1.0, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (1.0, rest)
    } else {
        (1.0, trimmed)
    };
    if rest == "Infinity" {
        return Some(sign * f64::INFINITY);
    }
    let value = if let Some(hex) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        if hex.is_empty() {
            return None;
        }
        let mut value: f64 = 0.0;
        for c in hex.chars() {
            value = value * 16.0 + c.to_digit(16)? as f64;
        }
        value
    } else {
        if rest.eq_ignore_ascii_case("inf")
            || rest.eq_ignore_ascii_case("infinity")
            || rest.eq_ignore_ascii_case("nan")
        {
            return None;
        }
        rest.parse::<f64>().ok()?
    };
    Some(sign * value)
}

// =============================================================================
// Write log (PI_TUI_WRITE_LOG)
// =============================================================================

/// Resolve `PI_TUI_WRITE_LOG`: an existing directory gets a timestamped
/// `tui-<ts>-<pid>.log` inside it; any other value is used as a file path.
fn resolve_write_log_path() -> Option<String> {
    let env = env_get("PI_TUI_WRITE_LOG").unwrap_or_default();
    if env.is_empty() {
        return None;
    }
    match std::fs::metadata(&env) {
        Ok(metadata) if metadata.is_dir() => Some(
            PathBuf::from(&env)
                .join(format!("tui-{}-{}.log", utc_timestamp_for_log(), std::process::id()))
                .to_string_lossy()
                .into_owned(),
        ),
        // Not an existing directory - use as-is (file path)
        _ => Some(env),
    }
}

/// `YYYY-MM-DD_HH-MM-SS` in UTC. Divergence: the TS original uses local
/// time via `new Date()`; pure `std` has no timezone database.
fn utc_timestamp_for_log() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let seconds_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}_{:02}-{:02}-{:02}",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Days since the Unix epoch → (year, month, day), proleptic Gregorian
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// =============================================================================
// Terminal traits
// =============================================================================

/// The input handler given to [`Terminal::start`] (TS `onInput`).
pub type InputHandler = Rc<RefCell<dyn FnMut(String)>>;
/// The resize handler given to [`Terminal::start`] (TS `onResize`).
pub type ResizeHandler = Rc<RefCell<dyn FnMut()>>;

/// Injectable process I/O for [`ProcessTerminal`]. The upstream tests mock
/// `process.stdout.write` / `process.stdin.on`; this trait is the seam.
///
/// Input is push-based: the runtime owner reads stdin and feeds
/// [`ProcessTerminal::handle_stdin_data`]. The registration markers mirror
/// the TS `process.stdin.on/removeListener("data", ...)` and
/// `process.stdout.on/removeListener("resize", ...)` bookkeeping.
pub trait TerminalIo {
    /// Write to stdout (TS `process.stdout.write`).
    fn write(&mut self, data: &str);
    /// Terminal width in columns, if known (TS `process.stdout.columns`).
    fn columns(&self) -> Option<usize> {
        None
    }
    /// Terminal height in rows, if known (TS `process.stdout.rows`).
    fn rows(&self) -> Option<usize> {
        None
    }
    /// Whether raw mode is available (TS `process.stdin.setRawMode`
    /// presence, i.e. stdin is a TTY).
    fn is_raw_mode_supported(&self) -> bool {
        false
    }
    /// The current raw-mode state (TS `process.stdin.isRaw`).
    fn raw_mode(&self) -> bool {
        false
    }
    /// Enable/disable raw mode (TS `process.stdin.setRawMode`).
    fn set_raw_mode(&mut self, _raw: bool) {}
    /// Pause stdin (TS `process.stdin.pause`); honored by the runtime owner.
    fn pause_stdin(&mut self) {}
    /// Resume stdin (TS `process.stdin.resume`).
    fn resume_stdin(&mut self) {}
    /// Mark the stdin `data` listener as registered/unregistered.
    fn set_stdin_data_registered(&mut self, _registered: bool) {}
    /// Mark the stdout `resize` listener as registered/unregistered.
    fn set_resize_registered(&mut self, _registered: bool) {}
}

/// Minimal terminal interface for TUI (port of the TS `Terminal` interface,
/// plus the helpers this port exposes for parity with the class).
pub trait Terminal {
    /// **Clock contract (port note):** the TS drives its timers via the Node
    /// event loop; this port replaces them with deadline state that the
    /// runtime owner must pump (`ProcessTerminal::on_tick`-style deadline
    /// firing — see `StdinBuffer::fire_timeout` for the pattern). Neglecting
    /// to pump the clock silently defers negotiation-buffer flushes and
    /// progress keepalives; it never corrupts state. `drain_input` stepping
    /// is likewise explicit-time-driven; `drain_input_with_defaults` is a
    /// `ProcessTerminal` convenience (not on the trait) because it needs the
    /// concrete default parameters.
    /// Start the terminal with input and resize handlers.
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler);
    /// Stop the terminal and restore state.
    fn stop(&mut self);
    /// Begin draining stdin before exiting, to prevent Kitty key release
    /// events from leaking to the parent shell over slow SSH connections.
    ///
    /// Divergence: the TS original is an async method with a real-time
    /// polling loop (`maxMs` default 1000, `idleMs` default 50). This port
    /// begins an injectable, time-stepped state machine; the runtime owner
    /// advances the clock with [`ProcessTerminal::tick`] (or
    /// [`ProcessTerminal::poll_drain_input`]) until
    /// [`ProcessTerminal::drain_input_active`] is false.
    fn drain_input(&mut self, max_ms: u64, idle_ms: u64);
    /// Write output to the terminal.
    fn write(&mut self, data: &str);
    /// Terminal dimensions (columns).
    fn columns(&self) -> usize;
    /// Terminal dimensions (rows).
    fn rows(&self) -> usize;
    /// Whether the Kitty keyboard protocol is active.
    fn kitty_protocol_active(&self) -> bool;
    /// Move cursor up (negative) or down (positive) by N lines.
    fn move_by(&mut self, lines: i32);
    /// Hide the cursor.
    fn hide_cursor(&mut self);
    /// Show the cursor.
    fn show_cursor(&mut self);
    /// Clear current line.
    fn clear_line(&mut self);
    /// Clear from cursor to end of screen.
    fn clear_from_cursor(&mut self);
    /// Clear entire screen and move cursor to (0,0).
    fn clear_screen(&mut self);
    /// Set terminal window title.
    fn set_title(&mut self, title: &str);
    /// Progress indicator (OSC 9;4).
    fn set_progress(&mut self, active: bool);
    /// Report what the program is doing (OSC 7501). Sent only to terminals
    /// that support it; the latest status is re-sent when support is
    /// confirmed or the terminal restarts.
    fn set_program_status(&mut self, status: ProgramStatus);
    /// Forward a (possibly reassembled) input sequence to the input handler.
    fn forward_input_sequence(&self, _sequence: &str) {}
    /// Enable ENABLE_VIRTUAL_TERMINAL_INPUT on Windows consoles.
    fn enable_windows_vt_input(&mut self) {}
    /// Whether raw mode is available on the underlying I/O.
    fn is_raw_mode_supported(&self) -> bool {
        false
    }
}

// =============================================================================
// ProcessTerminal
// =============================================================================

/// State for the time-stepped `drainInput` port (the TS async polling loop).
struct DrainState {
    end_time: u64,
    last_data_time: u64,
    idle_ms: u64,
    previous_handler: Option<InputHandler>,
}

/// Result of [`ProcessTerminal::read_keyboard_protocol_negotiation_sequence`]
/// (the TS `{ parsed, sequence } | "pending" | undefined`).
#[derive(Debug)]
enum NegotiationRead {
    Pending,
    Matched {
        parsed: KeyboardProtocolNegotiationSequence,
        sequence: String,
    },
    NoMatch,
}

/// Real terminal over process stdin/stdout (TS `ProcessTerminal`).
///
/// Construction is the TS `new ProcessTerminal()` (real I/O) via
/// [`ProcessTerminal::new`], or with an injected [`TerminalIo`] layer via
/// [`ProcessTerminal::with_io`] (used by the tests, mirroring the upstream
/// mocked-stream tests).
pub struct ProcessTerminal {
    io: Box<dyn TerminalIo>,
    /// Virtual clock (ms); advanced by [`ProcessTerminal::tick`]. The TS
    /// original uses `Date.now()` / real timers.
    now_ms: u64,
    was_raw: bool,
    input_handler: Option<InputHandler>,
    resize_handler: Option<ResizeHandler>,
    kitty_protocol_active: bool,
    modify_other_keys_active: bool,
    keyboard_protocol_pushed: bool,
    /// DA1 replies owed to keyboard protocol queries. Later DA1 replies
    /// answer other queries and are forwarded.
    pending_keyboard_protocol_device_attributes: u32,
    keyboard_protocol_negotiation_buffer: String,
    /// Deadline of the TS `keyboardProtocolBufferFlushTimer` (150 ms).
    negotiation_flush_deadline: Option<u64>,
    stdin_buffer: Option<StdinBuffer>,
    /// Deadline of the StdinBuffer's internal `setTimeout`.
    stdin_buffer_timeout_deadline: Option<u64>,
    /// Next keepalive fire time of the TS `progressInterval` (1000 ms).
    progress_keepalive_next: Option<u64>,
    /// Latest program status, kept across `stop()` so `start()` can report
    /// it again.
    program_status: Option<ProgramStatus>,
    /// Whether the terminal confirmed OSC 7501 support since the last
    /// `start()`, or `PI_PROGRAM_STATUS=1`.
    program_status_supported: bool,
    /// The support query was sent and its DA1 sentinel has not arrived yet.
    program_status_query_pending: bool,
    write_log_path: Option<String>,
    drain: Option<DrainState>,
}

impl ProcessTerminal {
    /// Real process stdin/stdout terminal (TS `new ProcessTerminal()`).
    pub fn new() -> Self {
        Self::with_io(Box::new(ProcessIo::new()))
    }

    /// Injectable-I/O form used by tests and alternative runtimes.
    pub fn with_io(io: Box<dyn TerminalIo>) -> Self {
        ProcessTerminal {
            io,
            now_ms: 0,
            was_raw: false,
            input_handler: None,
            resize_handler: None,
            kitty_protocol_active: false,
            modify_other_keys_active: false,
            keyboard_protocol_pushed: false,
            pending_keyboard_protocol_device_attributes: 0,
            keyboard_protocol_negotiation_buffer: String::new(),
            negotiation_flush_deadline: None,
            stdin_buffer: None,
            stdin_buffer_timeout_deadline: None,
            progress_keepalive_next: None,
            program_status: None,
            program_status_supported: false,
            program_status_query_pending: false,
            write_log_path: resolve_write_log_path(),
            drain: None,
        }
    }

    /// Whether the modifyOtherKeys fallback is currently enabled
    /// (TS `modifyOtherKeysActive` getter).
    pub fn modify_other_keys_active(&self) -> bool {
        self.modify_other_keys_active
    }

    /// Feed stdin data through the StdinBuffer (the TS
    /// `process.stdin.on("data", this.stdinDataHandler)` path). The runtime
    /// owner calls this with stdin bytes; the tests call it directly,
    /// mirroring the upstream harness invoking the captured listener.
    pub fn handle_stdin_data(&mut self, data: &str) {
        // The drainInput loop's `onData` listener updates `lastDataTime`.
        if let Some(drain) = &mut self.drain {
            drain.last_data_time = self.now_ms;
        }
        if let Some(buffer) = self.stdin_buffer.as_mut() {
            buffer.process(data);
        }
        self.pump_stdin_events();
    }

    /// Invoke the registered resize handler. The TS original relies on
    /// `process.stdout`'s `resize` event (SIGWINCH); the runtime owner calls
    /// this when a window-size change is observed.
    pub fn fire_resize(&self) {
        if let Some(handler) = &self.resize_handler {
            handler.borrow_mut()();
        }
    }

    /// Advance the virtual clock by `delta_ms` milliseconds, firing every
    /// due deadline: the StdinBuffer input timeout, the 150 ms
    /// negotiation-fragment flush, the 1000 ms progress keepalive, and
    /// `drainInput` completion. The tests call this directly, mirroring the
    /// upstream `mock.timers.tick(...)` tests.
    pub fn tick(&mut self, delta_ms: u64) {
        self.now_ms += delta_ms;
        // StdinBuffer timeout (the TS original's internal `setTimeout`).
        while self
            .stdin_buffer_timeout_deadline
            .is_some_and(|deadline| deadline <= self.now_ms)
        {
            self.stdin_buffer_timeout_deadline = None;
            if let Some(buffer) = self.stdin_buffer.as_mut() {
                buffer.fire_timeout();
            }
            self.pump_stdin_events();
        }
        // Negotiation-fragment flush timer.
        if self
            .negotiation_flush_deadline
            .is_some_and(|deadline| deadline <= self.now_ms)
        {
            self.negotiation_flush_deadline = None;
            self.flush_keyboard_protocol_negotiation_buffer_as_input();
        }
        // Progress keepalive interval (fires once per elapsed period).
        while self
            .progress_keepalive_next
            .is_some_and(|deadline| deadline <= self.now_ms)
        {
            let next = self.progress_keepalive_next.unwrap_or(0) + TERMINAL_PROGRESS_KEEPALIVE_MS;
            self.io.write(TERMINAL_PROGRESS_ACTIVE_SEQUENCE);
            self.progress_keepalive_next = Some(next);
        }
        // drainInput polling-loop completion.
        if self.drain.is_some() {
            self.poll_drain_input();
        }
    }

    /// Begin draining stdin with the TS defaults (1000 ms / 50 ms).
    pub fn drain_input_with_defaults(&mut self) {
        self.drain_input(DEFAULT_DRAIN_MAX_MS, DEFAULT_DRAIN_IDLE_MS);
    }

    /// Whether a `drainInput` state machine is still running.
    pub fn drain_input_active(&self) -> bool {
        self.drain.is_some()
    }

    /// One iteration of the drainInput loop body: returns `true` when the
    /// drain has finished (restoring the previous input handler). The
    /// runtime owner calls this (or [`ProcessTerminal::tick`]) after
    /// advancing time or feeding input.
    pub fn poll_drain_input(&mut self) -> bool {
        let finished = match &self.drain {
            None => true,
            Some(drain) => {
                self.now_ms >= drain.end_time
                    || self.now_ms.saturating_sub(drain.last_data_time) >= drain.idle_ms
            }
        };
        if finished && self.drain.is_some() {
            let drain = self.drain.take().expect("drain checked above");
            self.input_handler = drain.previous_handler;
        }
        finished
    }

    /// Set up StdinBuffer to split batched input into individual sequences.
    /// This ensures components receive single events, making
    /// matchesKey/isKeyRelease work correctly.
    ///
    /// Also watches for the Kitty protocol response and enables it when
    /// detected. This is done here (after stdinBuffer parsing) rather than
    /// on raw stdin to handle the case where the response arrives split
    /// across multiple events.
    fn setup_stdin_buffer(&mut self) {
        self.stdin_buffer = Some(StdinBuffer::new(StdinBufferOptions {
            escape_timeout_ms: resolve_escape_timeout_ms(),
            ..StdinBufferOptions::default()
        }));
    }

    /// Query terminal for Kitty keyboard protocol support and enable it if
    /// available. Private in the TS original (the upstream tests reach it
    /// through a cast; the in-crate test module can call it directly).
    ///
    /// Kitty's progressive enhancement detection requires requesting the
    /// desired flags before querying them. The trailing DA query is a
    /// sentinel supported by terminals that do not know Kitty keyboard
    /// protocol; receiving DA before a Kitty response enables
    /// modifyOtherKeys fallback without a startup timeout.
    ///
    /// The OSC 7501 program status query shares the DA sentinel: a terminal
    /// that supports it replies before DA. `PI_PROGRAM_STATUS=1` or `0`
    /// skips the query.
    fn query_and_enable_kitty_protocol(&mut self) {
        self.setup_stdin_buffer();
        self.io.set_stdin_data_registered(true);
        self.keyboard_protocol_pushed = true;
        self.pending_keyboard_protocol_device_attributes += 1;
        self.clear_keyboard_protocol_negotiation_buffer();
        let program_status_override = env_get("PI_PROGRAM_STATUS");
        self.program_status_supported = program_status_override.as_deref() == Some("1");
        self.program_status_query_pending = program_status_override.as_deref() != Some("1")
            && program_status_override.as_deref() != Some("0");
        let program_status_query = if self.program_status_query_pending {
            PROGRAM_STATUS_QUERY
        } else {
            ""
        };
        self.io.write(&format!(
            "{KITTY_KEYBOARD_PROTOCOL_QUERY}{program_status_query}{DEVICE_ATTRIBUTES_QUERY}"
        ));
        self.write_program_status();
    }

    /// The TS `stdinBuffer.on("data", ...)` handler: split events are
    /// dispatched here, one sequence at a time.
    fn dispatch_sequence(&mut self, sequence: &str) {
        if is_program_status_reply(sequence) {
            if self.program_status_query_pending {
                self.program_status_query_pending = false;
                self.program_status_supported = true;
                self.write_program_status();
            }
            return;
        }
        match self.read_keyboard_protocol_negotiation_sequence(sequence) {
            NegotiationRead::Pending => {
                // Wait briefly for the rest of a split Kitty response.
                self.schedule_keyboard_protocol_negotiation_buffer_flush();
            }
            NegotiationRead::Matched { parsed, sequence } => {
                if self.handle_keyboard_protocol_negotiation_sequence(parsed) {
                    return;
                }
                self.forward_input_sequence(&sequence);
            }
            NegotiationRead::NoMatch => {
                self.forward_input_sequence(sequence);
            }
        }
    }

    /// Drain StdinBuffer events (in emission order) and track its pending
    /// timeout as a virtual deadline.
    fn pump_stdin_events(&mut self) {
        let now = self.now_ms;
        let (events, pending_ms) = match self.stdin_buffer.as_mut() {
            Some(buffer) => (
                buffer.take_events(),
                buffer
                    .pending_timeout()
                    .map(|kind| buffer.pending_timeout_ms(kind)),
            ),
            None => (Vec::new(), None),
        };
        self.stdin_buffer_timeout_deadline = pending_ms.map(|ms| now + ms);
        for event in events {
            match event {
                StdinBufferEvent::Data(units) => {
                    self.dispatch_sequence(&units_to_string(&units));
                }
                StdinBufferEvent::Paste(content) => {
                    // Re-wrap paste content with bracketed paste markers for
                    // existing editor handling.
                    if let Some(handler) = &self.input_handler {
                        handler.borrow_mut()(format!(
                            "\x1b[200~{}\x1b[201~",
                            units_to_string(&content)
                        ));
                    }
                }
            }
        }
    }

    /// Handle a parsed negotiation reply. Returns whether the sequence was
    /// consumed (not forwarded as input).
    fn handle_keyboard_protocol_negotiation_sequence(
        &mut self,
        negotiation_sequence: KeyboardProtocolNegotiationSequence,
    ) -> bool {
        self.clear_keyboard_protocol_negotiation_buffer();
        if let KeyboardProtocolNegotiationSequence::DeviceAttributes = negotiation_sequence {
            if self.pending_keyboard_protocol_device_attributes == 0 {
                return false;
            }
            self.pending_keyboard_protocol_device_attributes -= 1;
            // The last owed DA answers the latest query, which got no
            // program status reply first. Earlier DA replies belong to
            // queries from before a restart.
            if self.pending_keyboard_protocol_device_attributes == 0 {
                self.program_status_query_pending = false;
            }
        }
        if let KeyboardProtocolNegotiationSequence::KittyFlags { flags } = negotiation_sequence {
            if flags != 0 {
                self.disable_modify_other_keys();
                if !self.kitty_protocol_active {
                    self.kitty_protocol_active = true;
                    set_kitty_protocol_active(true);
                }
            } else {
                self.enable_modify_other_keys();
            }
            return true;
        }

        if !self.kitty_protocol_active {
            self.enable_modify_other_keys();
        }
        true
    }

    /// Returns the parsed negotiation reply with its full (possibly
    /// reassembled) sequence.
    fn read_keyboard_protocol_negotiation_sequence(&mut self, sequence: &str) -> NegotiationRead {
        if !self.keyboard_protocol_negotiation_buffer.is_empty() {
            let buffered_sequence =
                format!("{}{}", self.keyboard_protocol_negotiation_buffer, sequence);
            if let Some(parsed) = parse_keyboard_protocol_negotiation_sequence(&buffered_sequence) {
                self.clear_keyboard_protocol_negotiation_buffer();
                return NegotiationRead::Matched {
                    parsed,
                    sequence: buffered_sequence,
                };
            }
            if is_keyboard_protocol_negotiation_sequence_prefix(&buffered_sequence) {
                self.set_keyboard_protocol_negotiation_buffer(&buffered_sequence);
                return NegotiationRead::Pending;
            }
            self.flush_keyboard_protocol_negotiation_buffer_as_input();
        }

        if let Some(parsed) = parse_keyboard_protocol_negotiation_sequence(sequence) {
            return NegotiationRead::Matched {
                parsed,
                sequence: sequence.to_string(),
            };
        }
        if is_keyboard_protocol_negotiation_sequence_prefix(sequence) {
            self.set_keyboard_protocol_negotiation_buffer(sequence);
            return NegotiationRead::Pending;
        }
        NegotiationRead::NoMatch
    }

    fn set_keyboard_protocol_negotiation_buffer(&mut self, sequence: &str) {
        self.clear_keyboard_protocol_negotiation_buffer_flush_timer();
        self.keyboard_protocol_negotiation_buffer = sequence.to_string();
    }

    fn clear_keyboard_protocol_negotiation_buffer(&mut self) {
        self.clear_keyboard_protocol_negotiation_buffer_flush_timer();
        self.keyboard_protocol_negotiation_buffer.clear();
    }

    fn flush_keyboard_protocol_negotiation_buffer_as_input(&mut self) {
        if self.keyboard_protocol_negotiation_buffer.is_empty() {
            return;
        }
        let sequence = std::mem::take(&mut self.keyboard_protocol_negotiation_buffer);
        self.clear_keyboard_protocol_negotiation_buffer_flush_timer();
        self.forward_input_sequence(&sequence);
    }

    /// The TS `setTimeout(flush, 150)`; recorded as a virtual deadline that
    /// [`ProcessTerminal::tick`] fires.
    fn schedule_keyboard_protocol_negotiation_buffer_flush(&mut self) {
        if self.keyboard_protocol_negotiation_buffer.is_empty() {
            return;
        }
        if self.negotiation_flush_deadline.is_some() {
            return;
        }
        self.negotiation_flush_deadline =
            Some(self.now_ms + KEYBOARD_PROTOCOL_RESPONSE_FRAGMENT_TIMEOUT_MS);
    }

    fn clear_keyboard_protocol_negotiation_buffer_flush_timer(&mut self) {
        self.negotiation_flush_deadline = None;
    }

    fn enable_modify_other_keys(&mut self) {
        if self.kitty_protocol_active || self.modify_other_keys_active {
            return;
        }
        self.io.write("\x1b[>4;2m");
        self.modify_other_keys_active = true;
    }

    fn disable_modify_other_keys(&mut self) {
        if !self.modify_other_keys_active {
            return;
        }
        self.io.write("\x1b[>4;0m");
        self.modify_other_keys_active = false;
    }

    fn write_program_status(&mut self) {
        if self.program_status_supported {
            if let Some(status) = &self.program_status {
                let sequence = format_program_status(status);
                self.io.write(&sequence);
            }
        }
    }

    fn clear_progress_interval(&mut self) -> bool {
        if self.progress_keepalive_next.is_none() {
            return false;
        }
        self.progress_keepalive_next = None;
        true
    }
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal for ProcessTerminal {
    fn start(&mut self, on_input: InputHandler, on_resize: ResizeHandler) {
        self.input_handler = Some(on_input);
        self.resize_handler = Some(on_resize);

        // Save previous state and enable raw mode
        self.was_raw = self.io.raw_mode();
        self.io.set_raw_mode(true);
        self.io.resume_stdin();

        // Enable bracketed paste mode - terminal will wrap pastes in
        // \x1b[200~ ... \x1b[201~
        self.io.write("\x1b[?2004h");

        // Set up resize handler immediately
        self.io.set_resize_registered(true);

        // Refresh terminal dimensions - they may be stale after
        // suspend/resume (SIGWINCH is lost while process is stopped).
        // Unix only, best-effort.
        refresh_terminal_dimensions();

        // On Windows, enable ENABLE_VIRTUAL_TERMINAL_INPUT so the console
        // sends VT escape sequences (e.g. \x1b[Z for Shift+Tab) instead of
        // raw console events that lose modifier information. Must run AFTER
        // setRawMode(true) since that resets console mode flags.
        self.enable_windows_vt_input();

        // Query Kitty keyboard protocol and fall back to modifyOtherKeys
        // when DA confirms no Kitty response.
        self.query_and_enable_kitty_protocol();
    }

    fn stop(&mut self) {
        if self.clear_progress_interval() {
            self.io.write(TERMINAL_PROGRESS_CLEAR_SEQUENCE);
        }
        // Remove the status while stopped, for example after exit or while
        // suspended. start() reports it again.
        if self.program_status_supported && self.program_status.is_some() {
            self.io.write(&format_program_status(&ProgramStatus {
                state: Some(ProgramState::Clear),
                ..Default::default()
            }));
        }
        self.program_status_supported = false;
        self.program_status_query_pending = false;

        // Disable bracketed paste mode
        self.io.write("\x1b[?2004l");

        let should_disable_kitty_protocol = self.keyboard_protocol_pushed || self.kitty_protocol_active;
        self.clear_keyboard_protocol_negotiation_buffer();

        // Disable Kitty keyboard protocol if not already done by drainInput()
        if should_disable_kitty_protocol {
            self.io.write("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            self.kitty_protocol_active = false;
            set_kitty_protocol_active(false);
        }
        self.disable_modify_other_keys();

        // Clean up StdinBuffer
        if let Some(mut buffer) = self.stdin_buffer.take() {
            buffer.destroy();
        }

        // Remove event handlers
        self.io.set_stdin_data_registered(false);
        self.input_handler = None;
        self.io.set_resize_registered(false);
        self.resize_handler = None;

        // Pause stdin to prevent any buffered input (e.g., Ctrl+D) from
        // being re-interpreted after raw mode is disabled. This fixes a race
        // condition where Ctrl+D could close the parent shell over SSH.
        self.io.pause_stdin();

        // Restore raw mode state
        self.io.set_raw_mode(self.was_raw);
    }

    fn drain_input(&mut self, max_ms: u64, idle_ms: u64) {
        let should_disable_kitty_protocol = self.keyboard_protocol_pushed || self.kitty_protocol_active;
        self.clear_keyboard_protocol_negotiation_buffer();
        if should_disable_kitty_protocol {
            // Disable Kitty keyboard protocol first so any late key
            // releases do not generate new Kitty escape sequences.
            self.io.write("\x1b[<u");
            self.keyboard_protocol_pushed = false;
            self.kitty_protocol_active = false;
            set_kitty_protocol_active(false);
        }
        self.disable_modify_other_keys();

        let previous_handler = self.input_handler.take();
        self.drain = Some(DrainState {
            end_time: self.now_ms + max_ms,
            last_data_time: self.now_ms,
            idle_ms,
            previous_handler,
        });
    }

    fn write(&mut self, data: &str) {
        self.io.write(data);
        if let Some(path) = &self.write_log_path {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = file.write_all(data.as_bytes());
            }
        }
    }

    fn columns(&self) -> usize {
        self.io
            .columns()
            .filter(|&columns| columns > 0)
            .unwrap_or_else(|| env_dimension("COLUMNS", 80))
    }

    fn rows(&self) -> usize {
        self.io
            .rows()
            .filter(|&rows| rows > 0)
            .unwrap_or_else(|| env_dimension("LINES", 24))
    }

    fn kitty_protocol_active(&self) -> bool {
        self.kitty_protocol_active
    }

    fn move_by(&mut self, lines: i32) {
        if lines > 0 {
            // Move down
            self.io.write(&format!("\x1b[{lines}B"));
        } else if lines < 0 {
            // Move up
            self.io.write(&format!("\x1b[{}A", -lines));
        }
        // lines === 0: no movement
    }

    fn hide_cursor(&mut self) {
        self.io.write("\x1b[?25l");
    }

    fn show_cursor(&mut self) {
        self.io.write("\x1b[?25h");
    }

    fn clear_line(&mut self) {
        self.io.write("\x1b[K");
    }

    fn clear_from_cursor(&mut self) {
        self.io.write("\x1b[J");
    }

    fn clear_screen(&mut self) {
        // Clear screen and move to home (1,1)
        self.io.write("\x1b[2J\x1b[H");
    }

    fn set_title(&mut self, title: &str) {
        // OSC 0;title BEL - set terminal window title
        self.io.write(&format!("\x1b]0;{title}\x07"));
    }

    fn set_program_status(&mut self, status: ProgramStatus) {
        self.program_status = if status.state == Some(ProgramState::Clear) {
            None
        } else {
            Some(status.clone())
        };
        if self.program_status_supported {
            let sequence = format_program_status(&status);
            self.io.write(&sequence);
        }
    }

    fn set_progress(&mut self, active: bool) {
        if active {
            // OSC 9;4;3 - indeterminate progress
            self.io.write(TERMINAL_PROGRESS_ACTIVE_SEQUENCE);
            if self.progress_keepalive_next.is_none() {
                self.progress_keepalive_next = Some(self.now_ms + TERMINAL_PROGRESS_KEEPALIVE_MS);
            }
        } else {
            self.clear_progress_interval();
            // OSC 9;4;0 - clear progress
            self.io.write(TERMINAL_PROGRESS_CLEAR_SEQUENCE);
        }
    }

    fn forward_input_sequence(&self, sequence: &str) {
        let Some(handler) = &self.input_handler else {
            return;
        };
        let should_detect_native_shift_enter = sequence == "\r"
            && (is_apple_terminal_session() || cfg!(target_os = "windows"));
        let input = normalize_native_shift_enter_input(
            sequence,
            should_detect_native_shift_enter,
            should_detect_native_shift_enter && is_native_modifier_pressed("shift"),
        );
        handler.borrow_mut()(input);
    }

    fn enable_windows_vt_input(&mut self) {
        if !cfg!(target_os = "windows") {
            return;
        }
        // The TS original asks the native platform helper
        // (`native-platform.ts`) to add ENABLE_VIRTUAL_TERMINAL_INPUT to the
        // console handle. No native helper exists in this pure-std port
        // (the TS "helper not available" path): Shift+Tab is not
        // distinguishable from Tab on Windows consoles.
    }

    fn is_raw_mode_supported(&self) -> bool {
        self.io.is_raw_mode_supported()
    }
}

/// `process.stdout.columns || Number(process.env.COLUMNS) || fallback` —
/// zero and NaN fall through to the default.
fn env_dimension(name: &str, fallback: usize) -> usize {
    if let Some(value) = env_get(name) {
        if let Some(number) = js_number(&value) {
            if number.is_finite() && number > 0.0 {
                // Fractional dimensions are meaningless; the TS would pass
                // e.g. `0.5` through (truthy), this usize API falls back to
                // the default instead of collapsing to zero.
                let dimension = number.floor() as usize;
                if dimension > 0 {
                    return dimension;
                }
            }
        }
    }
    fallback
}

// =============================================================================
// Real process I/O
// =============================================================================

/// Hand-declared libc externs for Linux (pure std, no libc crate). The
/// `termios` layout matches glibc/musl on Linux (`NCCS = 32`, trailing
/// `c_ispeed`/`c_ospeed`).
#[cfg(target_os = "linux")]
mod linux_ffi {
    use std::os::raw::{c_int, c_uchar, c_uint, c_ulong};

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct Termios {
        pub c_iflag: c_uint,
        pub c_oflag: c_uint,
        pub c_cflag: c_uint,
        pub c_lflag: c_uint,
        pub c_line: c_uchar,
        pub c_cc: [u8; 32],
        pub c_ispeed: c_uint,
        pub c_ospeed: c_uint,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Winsize {
        pub ws_row: u16,
        pub ws_col: u16,
        pub ws_xpixel: u16,
        pub ws_ypixel: u16,
    }

    extern "C" {
        pub fn tcgetattr(fd: c_int, termios_p: *mut Termios) -> c_int;
        pub fn tcsetattr(fd: c_int, optional_actions: c_int, termios_p: *const Termios) -> c_int;
        pub fn cfmakeraw(termios_p: *mut Termios);
        pub fn isatty(fd: c_int) -> c_int;
        pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        pub fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    pub const TCSANOW: c_int = 0;
    pub const TIOCGWINSZ: c_ulong = 0x5413;
    pub const SIGWINCH: c_int = 28;
}

/// Real `process.stdin`/`process.stdout` I/O (the TS default path).
///
/// Divergence: stdin reading is runtime-owned — this layer only writes
/// stdout, manages raw mode via hand-declared termios externs (Linux), and
/// reports the window size via `TIOCGWINSZ`. The `pause`/registration
/// markers are informational for the runtime owner.
pub struct ProcessIo {
    raw: bool,
    #[cfg(target_os = "linux")]
    saved_termios: Option<linux_ffi::Termios>,
}

impl ProcessIo {
    pub fn new() -> Self {
        ProcessIo {
            raw: false,
            #[cfg(target_os = "linux")]
            saved_termios: None,
        }
    }
}

impl Default for ProcessIo {
    fn default() -> Self {
        Self::new()
    }
}

/// `ioctl(TIOCGWINSZ)` on stdout (fd 1).
#[cfg(target_os = "linux")]
fn terminal_size() -> Option<(u16, u16)> {
    let mut winsize = linux_ffi::Winsize::default();
    let result = unsafe { linux_ffi::ioctl(1, linux_ffi::TIOCGWINSZ, &mut winsize) };
    if result == 0 && winsize.ws_col > 0 && winsize.ws_row > 0 {
        Some((winsize.ws_col, winsize.ws_row))
    } else {
        None
    }
}

impl TerminalIo for ProcessIo {
    fn write(&mut self, data: &str) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(data.as_bytes());
        let _ = lock.flush();
    }

    fn columns(&self) -> Option<usize> {
        #[cfg(target_os = "linux")]
        {
            terminal_size().map(|(columns, _)| columns as usize)
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    fn rows(&self) -> Option<usize> {
        #[cfg(target_os = "linux")]
        {
            terminal_size().map(|(_, rows)| rows as usize)
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    fn is_raw_mode_supported(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            unsafe { linux_ffi::isatty(0) == 1 }
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    fn raw_mode(&self) -> bool {
        self.raw
    }

    fn set_raw_mode(&mut self, raw: bool) {
        if !self.is_raw_mode_supported() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            if raw {
                if !self.raw {
                    unsafe {
                        let mut termios: linux_ffi::Termios = std::mem::zeroed();
                        if linux_ffi::tcgetattr(0, &mut termios) == 0 {
                            self.saved_termios = Some(termios);
                            linux_ffi::cfmakeraw(&mut termios);
                            linux_ffi::tcsetattr(0, linux_ffi::TCSANOW, &termios);
                            self.raw = true;
                        }
                    }
                }
            } else if let Some(saved) = self.saved_termios {
                unsafe {
                    linux_ffi::tcsetattr(0, linux_ffi::TCSANOW, &saved);
                }
                self.raw = false;
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = raw;
        }
    }
}

// =============================================================================
// Tests — 1:1 port of `test/terminal.test.ts` (27 tests)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program_status::{ProgramState, ProgramStatus};
    use crate::test_support::global_test_lock;

    /// Recording [`TerminalIo`] — the port's equivalent of the upstream
    /// tests mocking `process.stdout.write` / `process.stdin.on`.
    struct TestIo {
        writes: Rc<RefCell<Vec<String>>>,
    }

    impl TestIo {
        fn new(writes: Rc<RefCell<Vec<String>>>) -> Self {
            TestIo { writes }
        }
    }

    impl TerminalIo for TestIo {
        fn write(&mut self, data: &str) {
            self.writes.borrow_mut().push(data.to_string());
        }
    }

    // -------------------------------------------------------------------------
    // resolveEscapeTimeoutMs
    // -------------------------------------------------------------------------

    mod resolve_escape_timeout_ms {
        use super::*;

        fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
            move |name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        }

        #[test]
        fn uses_pi_tui_esc_timeout_when_configured() {
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("PI_TUI_ESC_TIMEOUT", "80")])),
                80
            );
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[
                    ("PI_TUI_ESC_TIMEOUT", "80"),
                    ("SSH_TTY", "/dev/pts/1"),
                ])),
                80
            );
        }

        #[test]
        fn ignores_invalid_pi_tui_esc_timeout_values() {
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("PI_TUI_ESC_TIMEOUT", "abc")])),
                10
            );
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("PI_TUI_ESC_TIMEOUT", "0")])),
                10
            );
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("PI_TUI_ESC_TIMEOUT", "-5")])),
                10
            );
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("PI_TUI_ESC_TIMEOUT", "")])),
                10
            );
        }

        #[test]
        fn defaults_to_100ms_over_ssh() {
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("SSH_CONNECTION", "10.0.0.1 22")])),
                100
            );
            assert_eq!(
                resolve_escape_timeout_ms_with(env(&[("SSH_TTY", "/dev/pts/1")])),
                100
            );
        }

        #[test]
        fn defaults_to_10ms_otherwise() {
            assert_eq!(resolve_escape_timeout_ms_with(env(&[])), 10);
        }
    }

    // -------------------------------------------------------------------------
    // normalizeNativeShiftEnterInput
    // -------------------------------------------------------------------------

    mod normalize_native_shift_enter_input {
        use super::*;

        #[test]
        fn rewrites_return_to_csi_u_shift_enter_when_native_shift_detection_is_enabled_and_shift_is_pressed()
         {
            assert_eq!(
                normalize_native_shift_enter_input("\r", true, true),
                "\x1b[13;2u"
            );
        }

        #[test]
        fn leaves_return_unchanged_when_native_shift_detection_is_disabled() {
            assert_eq!(normalize_native_shift_enter_input("\r", false, true), "\r");
        }

        #[test]
        fn leaves_return_unchanged_when_shift_is_not_pressed() {
            assert_eq!(normalize_native_shift_enter_input("\r", true, false), "\r");
        }

        #[test]
        fn leaves_non_return_input_unchanged() {
            assert_eq!(
                normalize_native_shift_enter_input("\x1b[13;2u", true, true),
                "\x1b[13;2u"
            );
            assert_eq!(normalize_native_shift_enter_input("a", true, true), "a");
        }
    }

    // -------------------------------------------------------------------------
    // normalizeAppleTerminalInput
    // -------------------------------------------------------------------------

    mod normalize_apple_terminal_input {
        use super::*;

        #[test]
        fn rewrites_apple_terminal_return_to_csi_u_shift_enter_when_shift_is_pressed() {
            assert_eq!(
                normalize_apple_terminal_input("\r", true, true),
                "\x1b[13;2u"
            );
        }

        #[test]
        fn leaves_apple_terminal_return_unchanged_when_shift_is_not_pressed() {
            assert_eq!(normalize_apple_terminal_input("\r", true, false), "\r");
        }

        #[test]
        fn leaves_non_apple_terminal_return_unchanged_when_shift_is_pressed() {
            assert_eq!(normalize_apple_terminal_input("\r", false, true), "\r");
        }

        #[test]
        fn leaves_non_return_input_unchanged() {
            assert_eq!(
                normalize_apple_terminal_input("\x1b[13;2u", true, true),
                "\x1b[13;2u"
            );
            assert_eq!(normalize_apple_terminal_input("a", true, true), "a");
        }
    }

    // -------------------------------------------------------------------------
    // ProcessTerminal Kitty keyboard protocol negotiation
    // -------------------------------------------------------------------------

    mod process_terminal_negotiation {
        use super::*;

        /// The upstream test harness: mocks stdout writes, captures the
        /// stdin data listener, assigns `terminal.inputHandler` through a
        /// cast, and calls the private `queryAndEnableKittyProtocol`. The
        /// test module can reach those private members directly.
        struct NegotiationHarness {
            terminal: ProcessTerminal,
            writes: Rc<RefCell<Vec<String>>>,
            input: Rc<RefCell<Option<String>>>,
            cleaned: bool,
        }

        fn setup_negotiation() -> NegotiationHarness {
            let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
            let input: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            let mut terminal = ProcessTerminal::with_io(Box::new(TestIo::new(writes.clone())));
            let input_sink = input.clone();
            terminal.input_handler = Some(Rc::new(RefCell::new(move |data: String| {
                *input_sink.borrow_mut() = Some(data);
            })));
            terminal.query_and_enable_kitty_protocol();
            NegotiationHarness {
                terminal,
                writes,
                input,
                cleaned: false,
            }
        }

        impl NegotiationHarness {
            /// The upstream `send(data)` invokes the captured
            /// `process.stdin` `data` listener (the terminal's own
            /// `stdinDataHandler`).
            fn send(&mut self, data: &str) {
                self.terminal.handle_stdin_data(data);
            }

            fn get_input(&self) -> Option<String> {
                self.input.borrow().clone()
            }

            fn writes(&self) -> Vec<String> {
                self.writes.borrow().clone()
            }

            fn writes_contain(&self, needle: &str) -> bool {
                self.writes().iter().any(|write| write == needle)
            }

            fn count_writes(&self, needle: &str) -> usize {
                self.writes().iter().filter(|write| *write == needle).count()
            }

            fn last_write(&self) -> Option<String> {
                self.writes().last().cloned()
            }

            fn any_write_starts_with(&self, prefix: &str) -> bool {
                self.writes().iter().any(|write| write.starts_with(prefix))
            }

            fn cleanup(&mut self) {
                if self.cleaned {
                    return;
                }
                self.cleaned = true;
                self.terminal.stop();
                crate::keys::set_kitty_protocol_active(false);
            }
        }

        impl Drop for NegotiationHarness {
            fn drop(&mut self) {
                self.cleanup();
            }
        }

        #[test]
        fn queries_kitty_mode_before_enabling_modify_other_keys_fallback() {
            let _guard = global_test_lock();
            let harness = setup_negotiation();
            assert_eq!(harness.writes()[0], "\x1b[>7u\x1b[?u\x1b]7501;?\x1b\\\x1b[c");
            assert!(!harness.writes_contain("\x1b[>4;2m"));
            assert!(!harness.terminal.kitty_protocol_active());
        }

        #[test]
        fn activates_kitty_mode_for_non_zero_negotiated_flags() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("\x1b[?7u");

            assert_eq!(harness.get_input(), None);
            assert!(harness.terminal.kitty_protocol_active());
            assert!(!harness.writes_contain("\x1b[>4;2m"));
            assert!(!harness.writes_contain("\x1b[>4;0m"));

            harness.cleanup();
            assert_eq!(harness.count_writes("\x1b[<u"), 1);
            assert!(!harness.writes_contain("\x1b[>4;0m"));
        }

        #[test]
        fn falls_back_to_modify_other_keys_for_zero_kitty_flags() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("\x1b[?0u");

            assert_eq!(harness.get_input(), None);
            assert!(!harness.terminal.kitty_protocol_active());
            assert_eq!(harness.count_writes("\x1b[>4;2m"), 1);

            harness.cleanup();
            assert_eq!(harness.count_writes("\x1b[>4;0m"), 1);
        }

        #[test]
        fn falls_back_to_modify_other_keys_for_device_attributes_without_kitty_flags() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("\x1b[?62;4;52c");

            assert_eq!(harness.get_input(), None);
            assert!(!harness.terminal.kitty_protocol_active());
            assert_eq!(harness.count_writes("\x1b[>4;2m"), 1);
        }

        #[test]
        fn forwards_device_attributes_replies_that_answer_other_queries() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("\x1b[?7u");
            harness.send("\x1b[?62;4;52c");
            assert_eq!(harness.get_input(), None);

            // The TUI's color query uses DA1 as its own sentinel.
            harness.send("\x1b[?62;4;52c");
            assert_eq!(harness.get_input().as_deref(), Some("\x1b[?62;4;52c"));
        }

        #[test]
        fn forwards_normal_input_while_waiting_for_kitty_response() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("a");

            assert_eq!(harness.get_input().as_deref(), Some("a"));
            assert!(!harness.terminal.kitty_protocol_active());
        }

        #[test]
        fn tracks_split_kitty_confirmation() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("\x1b[?7");
            harness.terminal.tick(10);

            assert_eq!(harness.get_input(), None);

            harness.send("u");

            assert!(harness.terminal.kitty_protocol_active());
            assert!(!harness.writes_contain("\x1b[>4;2m"));
        }

        // ---------------------------------------------------------------------
        // program status (OSC 7501)
        // ---------------------------------------------------------------------

        mod program_status {
            use super::*;

            const WORKING: &str = "\x1b]7501;state=working:app=pi\x1b\\";
            const CLEAR: &str = "\x1b]7501;state=clear\x1b\\";

            fn status_working() -> ProgramStatus {
                ProgramStatus {
                    state: Some(ProgramState::Working),
                    app: Some("pi".to_string()),
                    ..Default::default()
                }
            }

            /// The upstream `withOverride` (sets / deletes
            /// `PI_PROGRAM_STATUS` around the test body).
            fn with_override(value: Option<&str>, f: impl FnOnce()) {
                crate::keys::with_env_vars(&[("PI_PROGRAM_STATUS", value)], f);
            }

            #[test]
            fn reports_the_latest_status_once_the_terminal_answers_the_query_before_da() {
                let _guard = global_test_lock();
                with_override(None, || {
                    let mut harness = setup_negotiation();
                    harness.terminal.set_program_status(status_working());
                    assert!(!harness.writes_contain(WORKING));

                    harness.send("\x1b]7501;?\x1b\\");
                    assert_eq!(harness.get_input(), None);
                    assert_eq!(harness.count_writes(WORKING), 1);
                    harness.send("\x1b[?62;4;52c");

                    harness.terminal.set_program_status(ProgramStatus {
                        state: Some(ProgramState::Done),
                        ..Default::default()
                    });
                    assert_eq!(
                        harness.last_write().as_deref(),
                        Some("\x1b]7501;state=done\x1b\\")
                    );
                });
            }

            #[test]
            fn reports_nothing_when_da_arrives_first_and_swallows_late_replies() {
                let _guard = global_test_lock();
                with_override(None, || {
                    let mut harness = setup_negotiation();
                    harness.send("\x1b[?62;4;52c");
                    harness.send("\x1b]7501;?\x07");
                    harness.terminal.set_program_status(status_working());

                    assert_eq!(harness.get_input(), None);
                    assert!(!harness.any_write_starts_with("\x1b]7501;state="));
                });
            }

            #[test]
            fn skips_the_query_when_pi_program_status_is_set() {
                let _guard = global_test_lock();
                with_override(Some("1"), || {
                    let mut harness = setup_negotiation();
                    assert_eq!(harness.writes()[0], "\x1b[>7u\x1b[?u\x1b[c");
                    harness.terminal.set_program_status(status_working());
                    assert_eq!(harness.last_write().as_deref(), Some(WORKING));
                });
                with_override(Some("0"), || {
                    let mut harness = setup_negotiation();
                    assert_eq!(harness.writes()[0], "\x1b[>7u\x1b[?u\x1b[c");
                    harness.terminal.set_program_status(status_working());
                    assert!(!harness.writes_contain(WORKING));
                });
            }

            #[test]
            fn does_not_let_a_da_reply_from_before_a_restart_end_the_new_query() {
                let _guard = global_test_lock();
                with_override(None, || {
                    let mut harness = setup_negotiation();
                    harness.terminal.set_program_status(status_working());
                    harness.terminal.stop();
                    harness.terminal.query_and_enable_kitty_protocol();

                    // The first start's replies arrive late: its DA, then
                    // the second start's reply and DA.
                    harness.send("\x1b[?62;4;52c");
                    harness.send("\x1b]7501;?\x1b\\");
                    harness.send("\x1b[?62;4;52c");
                    assert_eq!(harness.last_write().as_deref(), Some(WORKING));
                    assert_eq!(harness.get_input(), None);
                });
            }

            #[test]
            fn clears_the_status_on_stop_and_reports_it_again_after_restart() {
                let _guard = global_test_lock();
                with_override(None, || {
                    let mut harness = setup_negotiation();
                    harness.terminal.set_program_status(status_working());
                    harness.send("\x1b]7501;?\x1b\\");
                    harness.terminal.stop();
                    assert!(harness.writes_contain(CLEAR));

                    // Stopped: nothing is written until the restarted
                    // terminal confirms support again.
                    let writes_before_restart = harness.writes().len();
                    harness.terminal.set_program_status(status_working());
                    assert_eq!(harness.writes().len(), writes_before_restart);

                    harness.terminal.query_and_enable_kitty_protocol();
                    harness.send("\x1b]7501;?\x1b\\");
                    assert_eq!(harness.last_write().as_deref(), Some(WORKING));
                });
            }
        }

        #[test]
        fn replays_buffered_csi_prefix_input_when_it_is_not_a_kitty_response() {
            let _guard = global_test_lock();
            let mut harness = setup_negotiation();
            harness.send("\x1b[");
            // StdinBuffer sequence timeout, not the lone-ESC timeout.
            harness.terminal.tick(50);

            assert_eq!(harness.get_input(), None);

            // Negotiation-fragment flush timeout (150 ms after the sequence
            // timeout flushed the fragment).
            harness.terminal.tick(150);

            assert_eq!(harness.get_input().as_deref(), Some("\x1b["));
        }
    }

    // -------------------------------------------------------------------------
    // ProcessTerminal progress
    // -------------------------------------------------------------------------

    mod process_terminal_progress {
        use super::*;

        #[test]
        fn writes_a_valid_osc_9_4_clear_sequence() {
            let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
            let mut terminal = ProcessTerminal::with_io(Box::new(TestIo::new(writes.clone())));

            terminal.set_progress(false);
            assert_eq!(*writes.borrow(), vec!["\x1b]9;4;0\x07".to_string()]);
        }
    }

    // -------------------------------------------------------------------------
    // ProcessTerminal dimensions
    // -------------------------------------------------------------------------

    mod process_terminal_dimensions {
        use super::*;

        #[test]
        fn falls_back_to_columns_and_lines_before_default_dimensions() {
            let _guard = global_test_lock();
            crate::keys::with_env_vars(
                &[("COLUMNS", Some("123")), ("LINES", Some("45"))],
                || {
                    let terminal =
                        ProcessTerminal::with_io(Box::new(TestIo::new(Rc::new(RefCell::new(
                            Vec::new(),
                        )))));

                    assert_eq!(terminal.columns(), 123);
                    assert_eq!(terminal.rows(), 45);
                },
            );
        }
    }
}
