//! Port of `packages/tui/src/tui-main-screen.ts` (656 LOC) — D-A2.
//!
//! `TuiMainScreen` renders into the terminal's main screen + scrollback with
//! differential line updates. TS inheritance (`TuiMainScreen extends
//! TuiBase`) becomes composition: the screen owns `base: TuiBase` and the
//! TS `protected override` methods become hook methods wired up by
//! [`crate::impl_tui_surface!`].
//!
//! Port notes / divergences (see T5_PLAN §Divergences):
//! - `performance.now()`/`Date.now()` → [`crate::clock::now_ms`]. The debug
//!   log timestamps are derived from the virtual clock via
//!   [`iso_timestamp_from_ms`] (the TS `new Date().toISOString()`).
//! - FS debug logs (`PI_TUI_DEBUG`, `PI_TUI_DEBUG_REDRAW`, crash log on
//!   over-wide lines) are env-gated exactly like TS and use `std::fs`; the
//!   parity harness never enables them.
//! - The over-wide-line crash: TS throws after writing a crash log; this
//!   port panics with the same message.
//! - `Math.random()` in the `PI_TUI_DEBUG` file name becomes a monotonic
//!   counter (debug output only).
//! - `BoundedTerminalWriter` operates on UTF-16 code units exactly like the
//!   TS string indexing (chunk sizes and surrogate-pair preservation are
//!   measured in UTF-16 units), encoding/decoding at the boundaries.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::clock::now_ms;
use crate::component::{
    Component, ComponentMouseResult, ComponentRef, TuiMouseEvent, CURSOR_MARKER,
};
use crate::terminal::Terminal;
use crate::terminal_image::{delete_kitty_image, is_image_line, EnvLookup, KITTY_PREFIX};
use crate::tui::{TuiBase, TuiInputListenerResult, TuiMode, TuiStopOptions};
use crate::utils::{js_trim, utf16_len, visible_width};

use crate::impl_tui_surface;

/// `KITTY_SEQUENCE_PREFIX` (tui-main-screen.ts:8).
const KITTY_SEQUENCE_PREFIX: &str = KITTY_PREFIX;

/// `MAX_RENDER_WRITE_CHARS` (tui-main-screen.ts:9).
const MAX_RENDER_WRITE_CHARS: usize = 1024 * 1024;

// =============================================================================
// BoundedTerminalWriter (tui-main-screen.ts:11-73)
// =============================================================================

/// Streams terminal output in 1 MiB chunks so a full render never forms one
/// string large enough to exceed V8's limit (tui-main-screen.ts:11-15).
///
/// `append()` fills the current chunk and flushes it when full. Oversized
/// input is split at chunk boundaries, preserving surrogate pairs so each
/// write remains valid UTF-16. Callers append synchronized-output begin/end
/// sequences themselves; the final `flush()` writes any remainder, including
/// the end sequence.
///
/// Chunk accounting is in UTF-16 code units to mirror the TS `String.length`
/// semantics; chunks are decoded to Rust `String`s at the boundaries (pairs
/// are always preserved, so decoding is lossless).
pub struct BoundedTerminalWriter {
    buffer: String,
    /// UTF-16 length of `buffer` (TS `this.buffer.length`).
    buffer_units: usize,
    written_chars: usize,
    write: Box<dyn FnMut(&str)>,
}

impl BoundedTerminalWriter {
    /// `constructor(write)` (tui-main-screen.ts:19-24).
    pub fn new(write: impl FnMut(&str) + 'static) -> Self {
        BoundedTerminalWriter {
            buffer: String::new(),
            buffer_units: 0,
            written_chars: 0,
            write: Box::new(write),
        }
    }

    /// `append(value)` (tui-main-screen.ts:28-62): append terminal data,
    /// flushing full chunks as needed. Callers must call `flush()` after the
    /// final append. Oversized values are split without splitting surrogate
    /// pairs.
    pub fn append(&mut self, value: &str) {
        // tui-main-screen.ts:31-62 — indices are UTF-16 code units.
        let units: Vec<u16> = value.encode_utf16().collect();
        let mut offset = 0usize;
        while offset < units.len() {
            let capacity = MAX_RENDER_WRITE_CHARS - self.buffer_units;
            if capacity == 0 {
                self.flush();
                continue;
            }

            let mut end = units.len().min(offset + capacity);
            if end < units.len()
                && (0xd800..=0xdbff).contains(&units[end - 1])
                && (0xdc00..=0xdfff).contains(&units[end])
            {
                end -= 1;
            }
            if end == offset {
                self.flush();
                continue;
            }

            self.buffer
                .push_str(&String::from_utf16_lossy(&units[offset..end]));
            self.buffer_units += end - offset;
            offset = end;
            if self.buffer_units == MAX_RENDER_WRITE_CHARS {
                self.flush();
            }
        }
    }

    /// `flush()` (tui-main-screen.ts:64-69): write the current chunk, if
    /// any, and retain only its character count for debug output.
    pub fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let chunk = std::mem::take(&mut self.buffer);
        let units = self.buffer_units;
        (self.write)(&chunk);
        self.written_chars += units;
        self.buffer_units = 0;
    }

    /// `get length()` (tui-main-screen.ts:71-73).
    pub fn length(&self) -> usize {
        self.written_chars + self.buffer_units
    }
}

// =============================================================================
// Kitty image header parsing (tui-main-screen.ts:76-107)
// =============================================================================

/// `KittyImageHeader` (tui-main-screen.ts:76-79).
#[derive(Debug, Clone, PartialEq)]
pub struct KittyImageHeader {
    pub ids: Vec<i64>,
    pub rows: i64,
}

/// `Number(value)` (JS) for the subset of numeric literals a kitty header
/// can contain: decimal with optional exponent, hex `0x…`, `Infinity` and
/// the empty string (`Number("") === 0`). Anything else is `NaN`.
/// No TS counterpart — explicit matcher (crate rule: no regex crate).
fn js_number(s: &str) -> f64 {
    let trimmed = js_trim(s);
    if trimmed.is_empty() {
        return 0.0; // Number("") === 0
    }
    let (neg, body) = match trimmed.as_bytes()[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if body.is_empty() {
        return f64::NAN; // Number("-") === NaN
    }
    if body == "Infinity" {
        return if neg { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return f64::NAN;
        }
        let mut value = 0f64;
        for b in hex.bytes() {
            value = value * 16.0 + (b as char).to_digit(16).unwrap() as f64;
        }
        return if neg { -value } else { value };
    }
    // Decimal (optionally with exponent). Reject Rust-only accepted forms
    // ("inf", "NaN", …) by whitelisting the JS numeric charset.
    let charset_ok = body
        .bytes()
        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || b == b'+' || b == b'-');
    if !charset_ok {
        return f64::NAN;
    }
    let value = body.parse::<f64>().unwrap_or(f64::NAN);
    if neg { -value } else { value }
}

/// `Number.isInteger(v)` for the finite/NaN cases we need.
fn js_is_integer(v: f64) -> bool {
    v.is_finite() && v == v.trunc()
}

/// `parseKittyImageHeader(line)` (tui-main-screen.ts:81-99).
pub fn parse_kitty_image_header(line: &str) -> Option<KittyImageHeader> {
    let sequence_start = line.find(KITTY_SEQUENCE_PREFIX)?;
    let params_start = sequence_start + KITTY_SEQUENCE_PREFIX.len();
    // `line.indexOf(";", paramsStart)` — the ';' before the base64 payload.
    let params_end = params_start + line[params_start..].find(';')?;

    let mut ids: Vec<i64> = Vec::new();
    let mut rows: i64 = 1;
    for param in line[params_start..params_end].split(',') {
        // JS `param.split("=", 2)` — value is everything after the first '='.
        let Some(eq) = param.find('=') else { continue };
        let key = &param[..eq];
        let value = &param[eq + 1..];
        let number_value = js_number(value);
        if !js_is_integer(number_value) || number_value <= 0.0 || number_value > 0xffff_ffffu32 as f64
        {
            continue;
        }
        if key == "i" {
            ids.push(number_value as i64);
        } else if key == "r" {
            rows = number_value as i64;
        }
    }
    Some(KittyImageHeader { ids, rows })
}

/// `extractKittyImageIds(line)` (tui-main-screen.ts:101-103).
pub fn extract_kitty_image_ids(line: &str) -> Vec<i64> {
    parse_kitty_image_header(line).map(|header| header.ids).unwrap_or_default()
}

/// `extractKittyImageRows(line)` (tui-main-screen.ts:105-107).
pub fn extract_kitty_image_rows(line: &str) -> i64 {
    parse_kitty_image_header(line).map(|header| header.rows).unwrap_or(1)
}

// =============================================================================
// isTermuxSession (tui-main-screen.ts:109-111)
// =============================================================================

/// `isTermuxSession()` (tui-main-screen.ts:109-111) with an injectable env
/// reader (T5_PLAN divergence: `process.env` → `EnvLookup`).
pub fn is_termux_session_with(env: EnvLookup<'_>) -> bool {
    // `Boolean(process.env.TERMUX_VERSION)`: set and non-empty.
    env("TERMUX_VERSION").is_some_and(|value| !value.is_empty())
}

/// `isTermuxSession()` (tui-main-screen.ts:109-111) against the real
/// process environment.
pub fn is_termux_session() -> bool {
    is_termux_session_with(&|key| std::env::var(key).ok())
}

// =============================================================================
// TuiMainScreenRenderState (tui-main-screen.ts:113-121)
// =============================================================================

/// `TuiMainScreenRenderState` (tui-main-screen.ts:113-121).
#[derive(Debug, Clone, PartialEq)]
pub struct TuiMainScreenRenderState {
    pub previous_lines: Vec<String>,
    pub previous_width: i64,
    pub previous_height: i64,
    pub cursor_row: i64,
    pub hardware_cursor_row: i64,
    pub max_lines_rendered: i64,
    pub previous_viewport_top: i64,
}

// =============================================================================
// TuiMainScreen (tui-main-screen.ts:124-656)
// =============================================================================

/// `TuiMainScreen` (tui-main-screen.ts:124-656) — TUI implementation that
/// renders into the terminal's main screen and scrollback.
pub struct TuiMainScreen {
    /// Composition root (TS: `extends TuiBase`).
    pub base: TuiBase,
    /// `logDirectory` (tui.ts:531) — debug/crash log directory.
    log_directory: Option<String>,
    previous_lines: Vec<String>,
    /// Ordered unique set (TS `Set<number>` iterated in insertion order).
    previous_kitty_image_ids: Vec<i64>,
    previous_width: i64,
    previous_height: i64,
    cursor_row: i64,
    hardware_cursor_row: i64,
    max_lines_rendered: i64,
    previous_viewport_top: i64,
}

impl TuiMainScreen {
    /// `constructor(terminal, showHardwareCursor?, logDirectory?)`
    /// (tui-main-screen.ts:126-133 via tui.ts:543-552).
    pub fn new(
        terminal: Rc<RefCell<dyn Terminal>>,
        show_hardware_cursor: Option<bool>,
        log_directory: Option<String>,
    ) -> Rc<RefCell<TuiMainScreen>> {
        let rc = Rc::new(RefCell::new(TuiMainScreen {
            base: TuiBase::new(terminal, show_hardware_cursor),
            log_directory,
            previous_lines: Vec::new(),
            previous_kitty_image_ids: Vec::new(),
            previous_width: 0,
            previous_height: 0,
            cursor_row: 0,
            hardware_cursor_row: 0,
            max_lines_rendered: 0,
            previous_viewport_top: 0,
        }));
        TuiMainScreen::install_self(&rc);
        rc
    }

    /// `captureRenderState()` (tui-main-screen.ts:135-145).
    pub fn capture_render_state(&self) -> TuiMainScreenRenderState {
        TuiMainScreenRenderState {
            previous_lines: self.previous_lines.clone(),
            previous_width: self.previous_width,
            previous_height: self.previous_height,
            cursor_row: self.cursor_row,
            hardware_cursor_row: self.hardware_cursor_row,
            max_lines_rendered: self.max_lines_rendered,
            previous_viewport_top: self.previous_viewport_top,
        }
    }

    /// `restoreRenderState(state)` (tui-main-screen.ts:147-156).
    pub fn restore_render_state(&mut self, state: &TuiMainScreenRenderState) {
        self.previous_lines = state
            .previous_lines
            .iter()
            .map(|line| if is_image_line(line) { String::new() } else { line.clone() })
            .collect();
        self.previous_kitty_image_ids = Vec::new();
        self.previous_width = state.previous_width;
        self.previous_height = state.previous_height;
        self.cursor_row = state.cursor_row;
        self.hardware_cursor_row = state.hardware_cursor_row;
        self.max_lines_rendered = state.max_lines_rendered;
        self.previous_viewport_top = state.previous_viewport_top;
    }

    /// `resetRenderState()` override (tui-main-screen.ts:158-166).
    fn reset_render_state_hook(&mut self) {
        self.previous_lines = Vec::new();
        self.previous_width = -1;
        self.previous_height = -1;
        self.cursor_row = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.previous_viewport_top = 0;
    }

    /// `beforeTerminalStop(options)` override (tui-main-screen.ts:168-176).
    fn before_terminal_stop(&mut self, options: &TuiStopOptions) {
        if options.preserve_screen || self.previous_lines.is_empty() {
            return;
        }
        self.base.terminal.borrow_mut().write(" ");
        let target_row = self.previous_lines.len() as i64;
        let line_diff = target_row - self.hardware_cursor_row;
        if line_diff > 0 {
            self.base.terminal.borrow_mut().write(&format!("\x1b[{line_diff}B"));
        } else if line_diff < 0 {
            self.base.terminal.borrow_mut().write(&format!("\x1b[{}A", -line_diff));
        }
        self.base.terminal.borrow_mut().write("\r\n");
    }

    /// `collectKittyImageIds(lines)` (tui-main-screen.ts:178-186) — ordered
    /// unique IDs (TS `Set` insertion order).
    fn collect_kitty_image_ids(&self, lines: &[String]) -> Vec<i64> {
        let mut ids: Vec<i64> = Vec::new();
        for line in lines {
            for id in extract_kitty_image_ids(line) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// `deleteKittyImages(ids)` (tui-main-screen.ts:188-194).
    fn delete_kitty_images(&self, ids: &[i64]) -> String {
        let mut buffer = String::new();
        for id in ids {
            buffer.push_str(&delete_kitty_image(*id));
        }
        buffer
    }

    /// `getKittyImageReservedRows(lines, index, maxIndex = lines.length - 1)`
    /// (tui-main-screen.ts:196-208).
    fn get_kitty_image_reserved_rows(&self, lines: &[String], index: usize, max_index: usize) -> i64 {
        let rows = extract_kitty_image_rows(lines.get(index).map(|s| s.as_str()).unwrap_or(""));
        if rows <= 1 {
            return 1;
        }

        let max_rows = rows
            .min(max_index as i64 - index as i64 + 1)
            .min(lines.len() as i64 - index as i64);
        let mut reserved_rows: i64 = 1;
        while reserved_rows < max_rows {
            let line = lines
                .get(index + reserved_rows as usize)
                .map(|s| s.as_str())
                .unwrap_or("");
            if is_image_line(line) || visible_width(line) > 0 {
                break;
            }
            reserved_rows += 1;
        }
        reserved_rows
    }

    /// `expandChangedRangeForKittyImages(firstChanged, lastChanged, newLines)`
    /// (tui-main-screen.ts:210-231).
    fn expand_changed_range_for_kitty_images(
        &self,
        first_changed: i64,
        last_changed: i64,
        new_lines: &[String],
    ) -> (i64, i64) {
        let mut expanded_first_changed = first_changed;
        let mut expanded_last_changed = last_changed;
        fn expand_for_lines(
            screen: &TuiMainScreen,
            lines: &[String],
            first_changed: i64,
            last_changed: i64,
            expanded_first_changed: &mut i64,
            expanded_last_changed: &mut i64,
        ) {
            for i in 0..lines.len() {
                if extract_kitty_image_ids(&lines[i]).is_empty() {
                    continue;
                }
                let block_end =
                    i as i64 + screen.get_kitty_image_reserved_rows(lines, i, lines.len() - 1) - 1;
                if i as i64 >= first_changed
                    || (i as i64 <= last_changed && block_end >= first_changed)
                {
                    *expanded_first_changed = (*expanded_first_changed).min(i as i64);
                    *expanded_last_changed = (*expanded_last_changed).max(block_end);
                }
            }
        }
        expand_for_lines(
            self,
            &self.previous_lines,
            first_changed,
            last_changed,
            &mut expanded_first_changed,
            &mut expanded_last_changed,
        );
        expand_for_lines(
            self,
            new_lines,
            first_changed,
            last_changed,
            &mut expanded_first_changed,
            &mut expanded_last_changed,
        );
        (expanded_first_changed, expanded_last_changed)
    }

    /// `deleteChangedKittyImages(firstChanged, lastChanged)`
    /// (tui-main-screen.ts:233-245).
    fn delete_changed_kitty_images(&self, first_changed: i64, last_changed: i64) -> String {
        if first_changed < 0 || last_changed < first_changed {
            return String::new();
        }

        let mut ids: Vec<i64> = Vec::new();
        let max_line = last_changed.min(self.previous_lines.len() as i64 - 1);
        let mut i = first_changed;
        while i <= max_line {
            for id in extract_kitty_image_ids(&self.previous_lines[i as usize]) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            i += 1;
        }

        self.delete_kitty_images(&ids)
    }

    /// `doRender()` (tui-main-screen.ts:247-622).
    fn do_render(&mut self) {
        if self.base.stopped {
            return;
        }
        let width = self.base.terminal.borrow().columns();
        let height = self.base.terminal.borrow().rows();
        let width_changed = self.previous_width != 0 && self.previous_width != width as i64;
        let height_changed = self.previous_height != 0 && self.previous_height != height as i64;
        let previous_buffer_length = if self.previous_height > 0 {
            self.previous_viewport_top + self.previous_height
        } else {
            height as i64
        };
        let mut prev_viewport_top = if height_changed {
            (previous_buffer_length - height as i64).max(0)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row;

        // Render all components to get new lines.
        // Resolve fake cursors before compositing so overlays and line
        // slicing see plain SGR codes.
        let rendered = self.render_children(width);
        let mut new_lines = self.base.resolve_fake_cursors(rendered);

        // Composite overlays into the rendered lines (before differential
        // compare).
        if !self.base.overlay_stack.is_empty() {
            new_lines = self.base.composite_overlays(new_lines, width, height);
        }

        // Extract cursor position before applying line resets (marker must
        // be found first).
        let cursor_pos = TuiBase::extract_cursor_position(&mut new_lines, height);

        new_lines = self.base.apply_line_resets(new_lines);

        // `redrawLogDirectory` (tui-main-screen.ts:323-324).
        let redraw_log_directory = if env_flag("PI_TUI_DEBUG_REDRAW") {
            self.log_directory.clone()
        } else {
            None
        };

        // First render - just output everything without clearing (assumes
        // clean screen).
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            log_redraw(
                redraw_log_directory.as_deref(),
                "first render",
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(&new_lines, false, cursor_pos, width, height);
            return;
        }

        // Width changes always need a full re-render because wrapping
        // changes.
        if width_changed {
            log_redraw(
                redraw_log_directory.as_deref(),
                &format!("terminal width changed ({} -> {})", self.previous_width, width),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(&new_lines, true, cursor_pos, width, height);
            return;
        }

        // Height changes normally need a full re-render to keep the visible
        // viewport aligned, but Termux changes height when the software
        // keyboard shows or hides. In that environment, a full redraw causes
        // the entire history to replay on every toggle.
        if height_changed && !is_termux_session() {
            log_redraw(
                redraw_log_directory.as_deref(),
                &format!("terminal height changed ({} -> {})", self.previous_height, height),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(&new_lines, true, cursor_pos, width, height);
            return;
        }

        // Content shrunk below the working area and no overlays - re-render
        // to clear empty rows (overlays need the padding, so only do this
        // when no overlays are active). Configurable via setClearOnShrink().
        let has_overlay_entries = !self.base.overlay_stack.is_empty();
        if self.base.get_clear_on_shrink()
            && (new_lines.len() as i64) < self.max_lines_rendered
            && !has_overlay_entries
        {
            log_redraw(
                redraw_log_directory.as_deref(),
                &format!("clearOnShrink (maxLinesRendered={})", self.max_lines_rendered),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(&new_lines, true, cursor_pos, width, height);
            return;
        }

        // Find first and last changed lines.
        #[allow(clippy::needless_range_loop)] // tui-main-screen.ts:353-368 index scan
        let mut first_changed: i64 = -1;
        let mut last_changed: i64 = -1;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for i in 0..max_lines {
            let old_line = if i < self.previous_lines.len() {
                self.previous_lines[i].as_str()
            } else {
                ""
            };
            let new_line = if i < new_lines.len() { new_lines[i].as_str() } else { "" };

            if old_line != new_line {
                if first_changed == -1 {
                    first_changed = i as i64;
                }
                last_changed = i as i64;
            }
        }
        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed == -1 {
                first_changed = self.previous_lines.len() as i64;
            }
            last_changed = new_lines.len() as i64 - 1;
        }
        if first_changed != -1 {
            let (expanded_first, expanded_last) =
                self.expand_changed_range_for_kitty_images(first_changed, last_changed, &new_lines);
            first_changed = expanded_first;
            last_changed = expanded_last;
        }
        let append_start = appended_lines
            && first_changed == self.previous_lines.len() as i64
            && first_changed > 0;

        // No changes - but still need to update hardware cursor position if
        // it moved.
        if first_changed == -1 {
            self.position_hardware_cursor(cursor_pos, new_lines.len());
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height as i64;
            return;
        }

        // All changes are in deleted lines (nothing to render, just clear).
        if first_changed >= new_lines.len() as i64 {
            if self.previous_lines.len() > new_lines.len() {
                let terminal = self.base.terminal.clone();
                let mut output =
                    BoundedTerminalWriter::new(move |data| terminal.borrow_mut().write(data));
                output.append("\x1b[?2026h");
                output.append(&self.delete_changed_kitty_images(first_changed, last_changed));
                // Move to end of new content (clamp to 0 for empty content).
                let target_row = (new_lines.len() as i64 - 1).max(0);
                if target_row < prev_viewport_top {
                    log_redraw(
                        redraw_log_directory.as_deref(),
                        &format!(
                            "deleted lines moved viewport up ({target_row} < {prev_viewport_top})"
                        ),
                        self.previous_lines.len(),
                        new_lines.len(),
                        height,
                    );
                    self.full_render(&new_lines, true, cursor_pos, width, height);
                    return;
                }
                let line_diff =
                    compute_line_diff(hardware_cursor_row, prev_viewport_top, viewport_top, target_row);
                if line_diff > 0 {
                    output.append(&format!("\x1b[{line_diff}B"));
                } else if line_diff < 0 {
                    output.append(&format!("\x1b[{}A", -line_diff));
                }
                output.append("\r");
                // Clear extra lines without scrolling.
                let extra_lines = self.previous_lines.len() as i64 - new_lines.len() as i64;
                if extra_lines > height as i64 {
                    log_redraw(
                        redraw_log_directory.as_deref(),
                        &format!("extraLines > height ({extra_lines} > {height})"),
                        self.previous_lines.len(),
                        new_lines.len(),
                        height,
                    );
                    self.full_render(&new_lines, true, cursor_pos, width, height);
                    return;
                }
                let clear_start_offset = if new_lines.is_empty() { 0 } else { 1 };
                if extra_lines > 0 && clear_start_offset > 0 {
                    output.append(&format!("\x1b[{clear_start_offset}B"));
                }
                for i in 0..extra_lines {
                    output.append("\r\x1b[2K");
                    if i < extra_lines - 1 {
                        output.append("\x1b[1B");
                    }
                }
                let move_back = (extra_lines - 1 + clear_start_offset).max(0);
                if move_back > 0 {
                    output.append(&format!("\x1b[{move_back}A"));
                }
                output.append("\x1b[?2026l");
                output.flush();
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(cursor_pos, new_lines.len());
            self.previous_kitty_image_ids = self.collect_kitty_image_ids(&new_lines);
            self.previous_lines = new_lines;
            self.previous_width = width as i64;
            self.previous_height = height as i64;
            self.previous_viewport_top = prev_viewport_top;
            return;
        }

        // Differential rendering can only touch what was actually visible.
        // If the first changed line is above the previous viewport, we need
        // a full redraw.
        if first_changed < prev_viewport_top {
            log_redraw(
                redraw_log_directory.as_deref(),
                &format!("firstChanged < viewportTop ({first_changed} < {prev_viewport_top})"),
                self.previous_lines.len(),
                new_lines.len(),
                height,
            );
            self.full_render(&new_lines, true, cursor_pos, width, height);
            return;
        }

        // Render from first changed line to end.
        // Keep updates wrapped in synchronized output while writing bounded
        // chunks.
        let terminal = self.base.terminal.clone();
        let mut output = BoundedTerminalWriter::new(move |data| terminal.borrow_mut().write(data));
        output.append("\x1b[?2026h"); // Begin synchronized output
        output.append(&self.delete_changed_kitty_images(first_changed, last_changed));
        let prev_viewport_bottom = prev_viewport_top + height as i64 - 1;
        let move_target_row = if append_start { first_changed - 1 } else { first_changed };
        if move_target_row > prev_viewport_bottom {
            let current_screen_row =
                (hardware_cursor_row - prev_viewport_top).clamp(0, height as i64 - 1);
            let move_to_bottom = height as i64 - 1 - current_screen_row;
            if move_to_bottom > 0 {
                output.append(&format!("\x1b[{move_to_bottom}B"));
            }
            let scroll = move_target_row - prev_viewport_bottom;
            output.append(&"\r\n".repeat(scroll as usize));
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row;
        }

        // Move cursor to first changed line (use hardwareCursorRow for
        // actual position).
        let line_diff =
            compute_line_diff(hardware_cursor_row, prev_viewport_top, viewport_top, move_target_row);
        if line_diff > 0 {
            output.append(&format!("\x1b[{line_diff}B")); // Move down
        } else if line_diff < 0 {
            output.append(&format!("\x1b[{}A", -line_diff)); // Move up
        }

        output.append(if append_start { "\r\n" } else { "\r" }); // Move to column 0

        // Only render changed lines (firstChanged to lastChanged), not all
        // lines to end. This reduces flicker when only a single line changes
        // (e.g., spinner animation).
        let render_end = last_changed.min(new_lines.len() as i64 - 1);
        let mut i = first_changed;
        while i <= render_end {
            if i > first_changed {
                output.append("\r\n");
            }
            let line = &new_lines[i as usize];
            let is_image = is_image_line(line);
            let image_reserved_rows = if is_image {
                self.get_kitty_image_reserved_rows(&new_lines, i as usize, render_end as usize)
            } else {
                1
            };
            if image_reserved_rows > 1 {
                let image_start_screen_row = i - viewport_top;
                if image_start_screen_row < 0
                    || image_start_screen_row + image_reserved_rows > height as i64
                {
                    log_redraw(
                        redraw_log_directory.as_deref(),
                        &format!(
                            "kitty image pre-clear would scroll ({} + {} > {})",
                            image_start_screen_row, image_reserved_rows, height
                        ),
                        self.previous_lines.len(),
                        new_lines.len(),
                        height,
                    );
                    self.full_render(&new_lines, true, cursor_pos, width, height);
                    return;
                }

                output.append("\x1b[2K");
                for _row in 1..image_reserved_rows {
                    output.append("\r\n\x1b[2K");
                }
                output.append(&format!("\x1b[{}A", image_reserved_rows - 1));
                output.append(line);
                output.append(&format!("\x1b[{}B", image_reserved_rows - 1));
                // TS `i += imageReservedRows - 1; continue;` + loop `i++`.
                i += image_reserved_rows - 1;
            } else {
                output.append("\x1b[2K"); // Clear current line
                if !is_image && visible_width(line) > width {
                    // Log all lines to crash file for debugging.
                    let crash_log_dir = self.log_directory.clone().unwrap_or_else(|| {
                        std::env::temp_dir().to_string_lossy().into_owned()
                    });
                    let crash_log_path = PathBuf::from(crash_log_dir).join("pi-tui-crash.log");
                    let mut crash_data = format!(
                        "Crash at {}\nTerminal width: {}\nLine {} visible width: {}\n\n=== All rendered lines ===\n",
                        iso_timestamp_from_ms(now_ms()),
                        width,
                        i,
                        visible_width(line)
                    );
                    for (idx, rendered) in new_lines.iter().enumerate() {
                        crash_data
                            .push_str(&format!("[{}] (w={}) {}\n", idx, visible_width(rendered), rendered));
                    }
                    crash_data.push('\n');
                    let _ = fs_create_dir_all(crash_log_path.parent());
                    let _ = std::fs::write(&crash_log_path, crash_data);

                    // Clean up terminal state before throwing.
                    self.stop(&TuiStopOptions::default());

                    let error_msg = format!(
                        "Rendered line {} exceeds terminal width ({} > {}).\n\nThis is likely caused by a custom TUI component not truncating its output.\nUse visibleWidth() to measure and truncateToWidth() to truncate lines.\n\nDebug log written to: {}",
                        i,
                        visible_width(line),
                        width,
                        crash_log_path.display()
                    );
                    panic!("{error_msg}");
                }
                output.append(line);
            }
            i += 1;
        }

        // Track where cursor ended up after rendering.
        let mut final_cursor_row = render_end;

        // If we had more lines before, clear them and move cursor back.
        if self.previous_lines.len() > new_lines.len() {
            // Move to end of new content first if we stopped before it.
            if render_end < new_lines.len() as i64 - 1 {
                let move_down = new_lines.len() as i64 - 1 - render_end;
                output.append(&format!("\x1b[{move_down}B"));
                final_cursor_row = new_lines.len() as i64 - 1;
            }
            let extra_lines = self.previous_lines.len() as i64 - new_lines.len() as i64;
            for _i in new_lines.len()..self.previous_lines.len() {
                output.append("\r\n\x1b[2K");
            }
            // Move cursor back to end of new content.
            output.append(&format!("\x1b[{extra_lines}A"));
        }

        output.append("\x1b[?2026l"); // End synchronized output

        if env_flag("PI_TUI_DEBUG") {
            write_debug_render_dump(
                first_changed,
                viewport_top,
                self.cursor_row,
                height,
                line_diff,
                hardware_cursor_row,
                render_end,
                final_cursor_row,
                cursor_pos,
                &new_lines,
                &self.previous_lines,
                output.length(),
            );
        }

        output.flush();

        // Track cursor position for next render.
        // cursorRow tracks end of content (for viewport calculation);
        // hardwareCursorRow tracks actual terminal cursor position (for
        // movement).
        self.cursor_row = (new_lines.len() as i64 - 1).max(0);
        self.hardware_cursor_row = final_cursor_row;
        // Track terminal's working area (grows but doesn't shrink unless
        // cleared).
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len() as i64);
        self.previous_viewport_top = (prev_viewport_top).max(final_cursor_row - height as i64 + 1);

        // Position hardware cursor for IME.
        self.position_hardware_cursor(cursor_pos, new_lines.len());

        self.previous_kitty_image_ids = self.collect_kitty_image_ids(&new_lines);
        self.previous_lines = new_lines;
        self.previous_width = width as i64;
        self.previous_height = height as i64;
    }

    /// The `fullRender(clear)` closure (tui-main-screen.ts:278-321) —
    /// hoisted to a method so the early-return bail-outs inside the
    /// differential paths can invoke it. Mirrors the TS closure exactly
    /// (it captures `newLines` and `cursorPos`).
    #[allow(clippy::too_many_arguments)] // TS closure captures (tui-main-screen.ts:278)
    fn full_render(
        &mut self,
        new_lines: &[String],
        clear: bool,
        cursor_pos: Option<(usize, usize)>,
        width: usize,
        height: usize,
    ) {
        self.base.full_redraw_count += 1;
        let terminal = self.base.terminal.clone();
        let mut output = BoundedTerminalWriter::new(move |data| terminal.borrow_mut().write(data));
        output.append("\x1b[?2026h"); // Begin synchronized output
        if clear {
            output.append(&self.delete_kitty_images(&self.previous_kitty_image_ids));
            output.append("\x1b[2J\x1b[H\x1b[3J"); // Clear screen, home, then clear scrollback
        }
        let mut i: i64 = 0;
        while i < new_lines.len() as i64 {
            if i > 0 {
                output.append("\r\n");
            }
            let line = &new_lines[i as usize];
            let is_image = is_image_line(line);
            let image_reserved_rows = if is_image {
                self.get_kitty_image_reserved_rows(new_lines, i as usize, new_lines.len() - 1)
            } else {
                1
            };
            if image_reserved_rows > 1 && image_reserved_rows <= height as i64 {
                for _row in 1..image_reserved_rows {
                    output.append("\r\n");
                }
                output.append(&format!("\x1b[{}A", image_reserved_rows - 1));
                output.append(line);
                output.append(&format!("\x1b[{}B", image_reserved_rows - 1));
                // TS `i += imageReservedRows - 1; continue;` + loop `i++`.
                i += image_reserved_rows - 1;
                i += 1;
                continue;
            }
            output.append(line);
            i += 1;
        }
        output.append("\x1b[?2026l"); // End synchronized output
        output.flush();
        self.cursor_row = (new_lines.len() as i64 - 1).max(0);
        self.hardware_cursor_row = self.cursor_row;
        // Reset max lines when clearing, otherwise track growth.
        if clear {
            self.max_lines_rendered = new_lines.len() as i64;
        } else {
            self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len() as i64);
        }
        let buffer_length = (height as i64).max(new_lines.len() as i64);
        self.previous_viewport_top = (buffer_length - height as i64).max(0);
        self.position_hardware_cursor(cursor_pos, new_lines.len());
        self.previous_kitty_image_ids = self.collect_kitty_image_ids(new_lines);
        self.previous_lines = new_lines.to_vec();
        self.previous_width = width as i64;
        self.previous_height = height as i64;
    }

    /// `positionHardwareCursor(cursorPos, totalLines)`
    /// (tui-main-screen.ts:624-656): position the hardware cursor for the
    /// IME candidate window.
    fn position_hardware_cursor(&mut self, cursor_pos: Option<(usize, usize)>, total_lines: usize) {
        let Some((row, col)) = cursor_pos else {
            self.base.terminal.borrow_mut().hide_cursor();
            return;
        };
        if total_lines <= 0 {
            self.base.terminal.borrow_mut().hide_cursor();
            return;
        }

        // Clamp cursor position to valid range.
        let target_row = (row as i64).clamp(0, total_lines as i64 - 1);
        let target_col = col as i64;

        // Move cursor from current position to target.
        let row_delta = target_row - self.hardware_cursor_row;
        let mut buffer = String::new();
        if row_delta > 0 {
            buffer.push_str(&format!("\x1b[{row_delta}B")); // Move down
        } else if row_delta < 0 {
            buffer.push_str(&format!("\x1b[{}A", -row_delta)); // Move up
        }
        // Move to absolute column (1-indexed).
        buffer.push_str(&format!("\x1b[{}G", target_col + 1));

        if !buffer.is_empty() {
            self.base.terminal.borrow_mut().write(&buffer);
        }

        self.hardware_cursor_row = target_row;
        if self.base.get_show_hardware_cursor() {
            self.base.terminal.borrow_mut().show_cursor();
        } else {
            self.base.terminal.borrow_mut().hide_cursor();
        }
    }

    // ----- TuiSurface hooks (TS `protected override` / inherited) -----

    /// `readonly mode = "regular"` (tui-main-screen.ts:125).
    fn mode(&self) -> TuiMode {
        TuiMode::Regular
    }

    /// `beforeTerminalStart()` (tui.ts:587) — no main-screen override.
    fn before_terminal_start(&mut self) {}

    /// `afterTerminalStart()` (tui.ts:589) — no main-screen override.
    fn after_terminal_start(&mut self) {}

    /// `afterTerminalStop()` (tui.ts:593) — no main-screen override.
    fn after_terminal_stop(&mut self, _options: &TuiStopOptions) {}

    /// `render(width)` — inherited from `Container` (tui.ts:363+).
    fn render_children(&mut self, width: usize) -> Vec<String> {
        Component::render(&mut *self.base.container.borrow_mut(), width)
    }

    /// `getMountedRoots()` resync (tui.ts:717-719): the main screen's
    /// mounted roots are exactly the container children.
    fn refresh_mounted_roots(&mut self) {
        self.base.mounted_roots = self.base.container.borrow().children.clone();
    }

    /// `handleMouse` — inherited from `Container` (tui.ts:390-418).
    fn handle_mouse_base(
        &mut self,
        self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        Component::handle_mouse(&mut *self.base.container.borrow_mut(), self_ref, event)
    }

    /// Main screens never install the implicit viewport input listener
    /// (that is alt-screen only — tui.ts constructor).
    fn handle_viewport_input(&mut self, _data: &str) -> Option<TuiInputListenerResult> {
        None
    }
}

impl_tui_surface!(TuiMainScreen);

/// `computeLineDiff` closure (tui-main-screen.ts:259-264).
fn compute_line_diff(
    hardware_cursor_row: i64,
    prev_viewport_top: i64,
    viewport_top: i64,
    target_row: i64,
) -> i64 {
    let current_screen_row = hardware_cursor_row - prev_viewport_top;
    let target_screen_row = target_row - viewport_top;
    target_screen_row - current_screen_row
}

// =============================================================================
// Debug-log helpers (env-gated; T5_PLAN divergence notes in module docs)
// =============================================================================

/// `process.env.<name> === "1"` (tui-main-screen.ts:323, 581).
fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().as_deref() == Some("1")
}

/// `fs.mkdirSync(path.dirname(logPath), { recursive: true })`.
fn fs_create_dir_all(dir: Option<&Path>) -> std::io::Result<()> {
    match dir {
        Some(dir) => std::fs::create_dir_all(dir),
        None => Ok(()),
    }
}

/// `logRedraw(reason)` closure (tui-main-screen.ts:323-333).
fn log_redraw(
    redraw_log_directory: Option<&str>,
    reason: &str,
    prev_len: usize,
    new_len: usize,
    height: usize,
) {
    let Some(directory) = redraw_log_directory else { return };
    let log_path = PathBuf::from(directory).join("pi-tui-debug.log");
    let msg = format!(
        "[{}] fullRender: {} (prev={}, new={}, height={})\n",
        iso_timestamp_from_ms(now_ms()),
        reason,
        prev_len,
        new_len,
        height
    );
    let _ = fs_create_dir_all(log_path.parent());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, msg.as_bytes()));
}

thread_local! {
    /// `Math.random()` stand-in for the PI_TUI_DEBUG file name suffix.
    static DEBUG_DUMP_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The `PI_TUI_DEBUG` render dump (tui-main-screen.ts:581-598).
/// `JSON.stringify(...)` becomes Rust `Debug` formatting (debug output only).
#[allow(clippy::too_many_arguments)] // mirrors the TS dump fields
fn write_debug_render_dump(
    first_changed: i64,
    viewport_top: i64,
    cursor_row: i64,
    height: usize,
    line_diff: i64,
    hardware_cursor_row: i64,
    render_end: i64,
    final_cursor_row: i64,
    cursor_pos: Option<(usize, usize)>,
    new_lines: &[String],
    previous_lines: &[String],
    written_chars: usize,
) {
    let debug_dir = "/tmp/tui";
    let _ = std::fs::create_dir_all(debug_dir);
    let suffix = DEBUG_DUMP_COUNTER.with(|counter| {
        let value = counter.get();
        counter.set(value + 1);
        value
    });
    let debug_path = PathBuf::from(debug_dir)
        .join(format!("render-{}-{}.log", now_ms() as u64, suffix));
    let cursor_pos_str = match cursor_pos {
        Some((row, col)) => format!("{{\"row\":{row},\"col\":{col}}}"),
        None => "null".to_string(),
    };
    let debug_data = format!(
        "firstChanged: {}\nviewportTop: {}\ncursorRow: {}\nheight: {}\nlineDiff: {}\nhardwareCursorRow: {}\nrenderEnd: {}\nfinalCursorRow: {}\ncursorPos: {}\nnewLines.length: {}\npreviousLines.length: {}\n\n=== newLines ===\n{:#?}\n\n=== previousLines ===\n{:#?}\n\n=== buffer ===\n[{} chars written in bounded chunks]\n",
        first_changed,
        viewport_top,
        cursor_row,
        height,
        line_diff,
        hardware_cursor_row,
        render_end,
        final_cursor_row,
        cursor_pos_str,
        new_lines.len(),
        previous_lines.len(),
        new_lines,
        previous_lines,
        written_chars
    );
    let _ = std::fs::write(debug_path, debug_data);
}

/// `new Date().toISOString()` from the virtual clock's epoch milliseconds
/// (the TS uses wall-clock `Date`; the port's clock has no calendar origin,
/// so timestamps are derived from `now_ms()` — debug output only).
fn iso_timestamp_from_ms(ms: f64) -> String {
    let ms_total = ms.floor() as i64;
    let millis = ms_total.rem_euclid(1000);
    let secs_total = ms_total.div_euclid(1000);
    let seconds = secs_total.rem_euclid(60);
    let mins_total = secs_total.div_euclid(60);
    let minutes = mins_total.rem_euclid(60);
    let hours_total = mins_total.div_euclid(60);
    let hours = hours_total.rem_euclid(24);
    let days = hours_total.div_euclid(24);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

/// Days-since-Unix-epoch → (year, month, day). Howard Hinnant's
/// `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

// =============================================================================
// Tests — upstream unit-testable behaviors + integration-style render tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::terminal::{InputHandler, ProgramStatus, ResizeHandler};
    use crate::test_support::global_test_lock;
    use crate::tui::TuiSurface;
    use std::cell::Cell;

    /// Recording terminal double (pattern from tui_overlay.rs tests), with
    /// settable size for resize tests.
    struct RecordingTerminal {
        cols: Cell<usize>,
        rows: Cell<usize>,
        writes: Vec<String>,
        hide_cursor_calls: u32,
        show_cursor_calls: u32,
    }

    impl RecordingTerminal {
        fn new(cols: usize, rows: usize) -> Self {
            RecordingTerminal {
                cols: Cell::new(cols),
                rows: Cell::new(rows),
                writes: Vec::new(),
                hide_cursor_calls: 0,
                show_cursor_calls: 0,
            }
        }
    }

    impl Terminal for RecordingTerminal {
        fn start(&mut self, _on_input: InputHandler, _on_resize: ResizeHandler) {}
        fn stop(&mut self) {}
        fn drain_input(&mut self, _max_ms: u64, _idle_ms: u64) {}
        fn write(&mut self, data: &str) {
            self.writes.push(data.to_string());
        }
        fn columns(&self) -> usize {
            self.cols.get()
        }
        fn rows(&self) -> usize {
            self.rows.get()
        }
        fn kitty_protocol_active(&self) -> bool {
            false
        }
        fn move_by(&mut self, _lines: i32) {}
        fn hide_cursor(&mut self) {
            self.hide_cursor_calls += 1;
        }
        fn show_cursor(&mut self) {
            self.show_cursor_calls += 1;
        }
        fn clear_line(&mut self) {}
        fn clear_from_cursor(&mut self) {}
        fn clear_screen(&mut self) {}
        fn set_title(&mut self, _title: &str) {}
        fn set_progress(&mut self, _active: bool) {}
        fn set_program_status(&mut self, _status: ProgramStatus) {}
    }

    /// Component rendering a fixed set of lines.
    struct LinesComponent {
        lines: Vec<String>,
    }

    impl Component for LinesComponent {
        fn render(&mut self, _width: usize) -> Vec<String> {
            self.lines.clone()
        }
    }

    fn lines_component(lines: &[&str]) -> Rc<RefCell<LinesComponent>> {
        Rc::new(RefCell::new(LinesComponent {
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }))
    }

    fn set_lines(component: &Rc<RefCell<LinesComponent>>, lines: &[&str]) {
        component.borrow_mut().lines = lines.iter().map(|s| s.to_string()).collect();
    }

    fn recording_screen(
        cols: usize,
        rows: usize,
        show_hardware_cursor: Option<bool>,
    ) -> (Rc<RefCell<TuiMainScreen>>, Rc<RefCell<RecordingTerminal>>) {
        let terminal = Rc::new(RefCell::new(RecordingTerminal::new(cols, rows)));
        let screen = TuiMainScreen::new(terminal.clone(), show_hardware_cursor, None);
        (screen, terminal)
    }

    /// `applyLineResets` tail every non-image line carries after a render.
    fn rl(line: &str) -> String {
        format!("{line}\x1b[0m\x1b]8;;\x07")
    }

    /// A kitty image line with header `i=<id>,r=<rows>` and a payload
    /// separator (the header parser needs the `;` before base64 data).
    fn kitty_image_line(id: i64, rows: i64) -> String {
        format!("\x1b_Gi={id},r={rows},q=2;AAAA\x1b\\")
    }

    // ----- BoundedTerminalWriter (tui-main-screen.ts:11-73) -----

    #[test]
    fn bounded_writer_small_appends_and_length() {
        let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = writes.clone();
        let mut writer = BoundedTerminalWriter::new(move |data| {
            sink.borrow_mut().push(data.to_string());
        });
        writer.append("hello");
        writer.append(" ");
        writer.append("world");
        // Nothing written before flush; length counts buffered units.
        assert!(writes.borrow().is_empty());
        assert_eq!(writer.length(), 11);
        writer.flush();
        assert_eq!(*writes.borrow(), vec!["hello world".to_string()]);
        // Flush of an empty buffer is a no-op.
        writer.flush();
        assert_eq!(writes.borrow().len(), 1);
        assert_eq!(writer.length(), 11);
    }

    #[test]
    fn bounded_writer_chunks_oversized_input() {
        let half = MAX_RENDER_WRITE_CHARS / 2;
        let chunk: String = "a".repeat(half);
        let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = writes.clone();
        let mut writer = BoundedTerminalWriter::new(move |data| {
            sink.borrow_mut().push(data.to_string());
        });
        writer.append(&chunk);
        writer.append(&chunk); // buffer now full → auto flush
        writer.append(&chunk); // remainder stays buffered
        assert_eq!(writes.borrow().len(), 1);
        assert_eq!(utf16_len(&writes.borrow()[0]), MAX_RENDER_WRITE_CHARS);
        assert_eq!(writer.length(), half * 3);
        writer.flush();
        assert_eq!(writes.borrow().len(), 2);
        assert_eq!(utf16_len(&writes.borrow()[1]), half);
        assert_eq!(writer.length(), half * 3);
    }

    #[test]
    fn bounded_writer_preserves_surrogate_pairs_at_boundaries() {
        // "a" + astral chars: UTF-16 units are 1 + 2·k with high surrogates
        // at odd indices, so a MAX-unit boundary always lands mid-pair.
        let count = MAX_RENDER_WRITE_CHARS / 2;
        let value = format!("a{}", "😀".repeat(count));
        assert_eq!(utf16_len(&value), MAX_RENDER_WRITE_CHARS + 1);
        let writes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = writes.clone();
        let mut writer = BoundedTerminalWriter::new(move |data| {
            sink.borrow_mut().push(data.to_string());
        });
        writer.append(&value);
        writer.flush();
        let writes = writes.borrow();
        // First chunk is shortened by one unit to keep the pair whole.
        assert_eq!(utf16_len(&writes[0]), MAX_RENDER_WRITE_CHARS - 1);
        assert!(writes[0].ends_with('😀'));
        // The remainder is exactly the final complete pair.
        assert_eq!(writes[1], "😀");
        assert_eq!(writer.length(), MAX_RENDER_WRITE_CHARS + 1);
    }

    // ----- parseKittyImageHeader / extract helpers (tui-main-screen.ts:76-107) -----

    #[test]
    fn parse_kitty_image_header_cases() {
        // tui-main-screen.ts:81-99
        let header = parse_kitty_image_header("\x1b_Gi=5,r=3,q=2;BASE\x1b\\").unwrap();
        assert_eq!(header.ids, vec![5]);
        assert_eq!(header.rows, 3);

        // Multiple ids accumulate; unknown keys ignored.
        let header = parse_kitty_image_header("\x1b_Gi=1,i=2,f=100;X").unwrap();
        assert_eq!(header.ids, vec![1, 2]);
        assert_eq!(header.rows, 1);

        // Sequence not at line start (multi-row images carry a cursor-up
        // prefix).
        let header = parse_kitty_image_header(&format!("\x1b[2A{}", kitty_image_line(9, 4))).unwrap();
        assert_eq!(header.ids, vec![9]);
        assert_eq!(header.rows, 4);

        // Non-integer / out-of-range / non-positive values are skipped.
        let header = parse_kitty_image_header("\x1b_Gi=abc,i=3.5,i=0,i=4294967296,r=x,i=7;X").unwrap();
        assert_eq!(header.ids, vec![7]);
        assert_eq!(header.rows, 1); // r=x is not a number → default

        // JS Number semantics: exponents and hex parse to integers.
        let header = parse_kitty_image_header("\x1b_Gi=1e2,r=0x3;X").unwrap();
        assert_eq!(header.ids, vec![100]);
        assert_eq!(header.rows, 3);

        // Missing prefix / missing ';' before payload → undefined.
        assert!(parse_kitty_image_header("plain text").is_none());
        assert!(parse_kitty_image_header("\x1b_Gi=5,q=2\x1b\\").is_none());

        // Params without '=' are skipped entirely.
        let header = parse_kitty_image_header("\x1b_Gm,i=5;X").unwrap();
        assert_eq!(header.ids, vec![5]);
    }

    #[test]
    fn extract_kitty_defaults() {
        // tui-main-screen.ts:101-107
        assert!(extract_kitty_image_ids("hello").is_empty());
        assert_eq!(extract_kitty_image_rows("hello"), 1);
        assert_eq!(extract_kitty_image_rows(&kitty_image_line(1, 6)), 6);
        assert_eq!(extract_kitty_image_ids(&kitty_image_line(42, 1)), vec![42]);
    }

    #[test]
    fn js_number_semantics() {
        // No TS counterpart (explicit matcher for `Number()`).
        assert_eq!(js_number(""), 0.0);
        assert_eq!(js_number("  12  "), 12.0);
        assert_eq!(js_number("-5"), -5.0);
        assert_eq!(js_number("+7"), 7.0);
        assert_eq!(js_number("1e2"), 100.0);
        assert_eq!(js_number("0x10"), 16.0);
        assert_eq!(js_number("-0x10"), -16.0);
        assert!(js_number("abc").is_nan());
        assert!(js_number("12x").is_nan());
        assert!(js_number("-").is_nan());
        assert!(js_number("Infinity").is_infinite());
    }

    // ----- isTermuxSession (tui-main-screen.ts:109-111) -----

    #[test]
    fn is_termux_session_injected_env() {
        let _lock = global_test_lock();
        assert!(is_termux_session_with(&|key| (key == "TERMUX_VERSION")
            .then(|| "1.38.0".to_string())));
        // Boolean("") === false — an empty value is not a Termux session.
        assert!(!is_termux_session_with(&|_| Some(String::new())));
        assert!(!is_termux_session_with(&|_| None));
    }

    // ----- Kitty-image row reservation / changed-range expansion -----

    #[test]
    fn kitty_image_reserved_rows() {
        // tui-main-screen.ts:196-208
        let _lock = global_test_lock();
        let (screen, _terminal) = recording_screen(80, 24, None);
        let screen = screen.borrow();

        // Image with r=3 followed by two blank rows reserves 3 rows.
        let lines = vec![kitty_image_line(5, 3), String::new(), String::new(), "x".into()];
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 0, 3), 3);

        // A non-blank row stops the reservation early.
        let lines = vec![kitty_image_line(5, 3), "text".into(), String::new(), "x".into()];
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 0, 3), 1);

        // Another image row stops the reservation early.
        let lines = vec![
            kitty_image_line(5, 3),
            kitty_image_line(6, 1),
            String::new(),
            "x".into(),
        ];
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 0, 3), 1);

        // maxIndex clamps the reservation.
        let lines = vec![kitty_image_line(5, 3), String::new(), String::new(), String::new()];
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 0, 1), 2);

        // rows <= 1 → 1; non-image line → 1.
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 3, 3), 1);
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 0, 3).max(0), 3);

        // Missing index (TS `lines[index] ?? ""`) → rows default 1.
        assert_eq!(screen.get_kitty_image_reserved_rows(&lines, 10, 3), 1);
    }

    #[test]
    fn expand_changed_range_for_kitty_images() {
        // tui-main-screen.ts:210-231
        let _lock = global_test_lock();
        let (screen, _terminal) = recording_screen(80, 24, None);
        let mut screen = screen.borrow_mut();

        // A change inside an image block expands to the whole block.
        let image_block =
            vec![kitty_image_line(5, 3), String::new(), String::new(), "tail".into()];
        screen.previous_lines = image_block.clone();
        let new_lines = vec![
            kitty_image_line(5, 3),
            String::new(),
            String::new(),
            "changed".into(),
        ];
        let (first, last) = screen.expand_changed_range_for_kitty_images(2, 3, &new_lines);
        assert_eq!((first, last), (0, 3));

        // A change fully outside any image block is not expanded.
        let (first, last) = screen.expand_changed_range_for_kitty_images(3, 3, &image_block);
        // Row 3 is past blockEnd=2 and i=0 fails both `i >= firstChanged`
        // and `blockEnd >= firstChanged`, so the block is not expanded.
        assert_eq!((first, last), (3, 3));
    }

    #[test]
    fn delete_changed_kitty_images_order_and_bounds() {
        // tui-main-screen.ts:233-245
        let _lock = global_test_lock();
        let (screen, _terminal) = recording_screen(80, 24, None);
        let mut screen = screen.borrow_mut();
        screen.previous_lines = vec![
            kitty_image_line(5, 1),
            "plain".into(),
            kitty_image_line(9, 1),
            kitty_image_line(5, 1), // duplicate id collected once
        ];
        // Range covering lines 0-3 deletes ids in first-appearance order.
        assert_eq!(
            screen.delete_changed_kitty_images(0, 3),
            delete_kitty_image(5) + &delete_kitty_image(9)
        );
        // Partial range only collects ids inside it.
        assert_eq!(screen.delete_changed_kitty_images(2, 2), delete_kitty_image(9));
        // Invalid ranges return "".
        assert_eq!(screen.delete_changed_kitty_images(-1, 3), "");
        assert_eq!(screen.delete_changed_kitty_images(3, 2), "");
        // lastChanged beyond previousLines clamps.
        assert_eq!(
            screen.delete_changed_kitty_images(0, 99),
            delete_kitty_image(5) + &delete_kitty_image(9)
        );
    }

    // ----- render state capture/restore + reset (tui-main-screen.ts:135-166) -----

    #[test]
    fn render_state_capture_restore() {
        // tui-main-screen.ts:135-156
        let _lock = global_test_lock();
        let (screen, _terminal) = recording_screen(80, 24, None);
        let mut screen = screen.borrow_mut();
        screen.previous_lines = vec!["plain".into(), kitty_image_line(5, 1)];
        screen.previous_kitty_image_ids = vec![7];
        screen.previous_width = 80;
        screen.previous_height = 24;
        screen.cursor_row = 3;
        screen.hardware_cursor_row = 2;
        screen.max_lines_rendered = 9;
        screen.previous_viewport_top = 4;

        let state = screen.capture_render_state();
        assert_eq!(state.previous_lines, vec!["plain".to_string(), kitty_image_line(5, 1)]);
        assert_eq!(state.previous_width, 80);

        screen.previous_lines = vec!["other".into()];
        screen.previous_kitty_image_ids = vec![1];
        screen.cursor_row = 0;

        screen.restore_render_state(&state);
        // Image lines are blanked on restore; kitty ids are cleared.
        assert_eq!(screen.previous_lines, vec!["plain".to_string(), String::new()]);
        assert!(screen.previous_kitty_image_ids.is_empty());
        assert_eq!(screen.previous_width, 80);
        assert_eq!(screen.previous_height, 24);
        assert_eq!(screen.cursor_row, 3);
        assert_eq!(screen.hardware_cursor_row, 2);
        assert_eq!(screen.max_lines_rendered, 9);
        assert_eq!(screen.previous_viewport_top, 4);
    }

    #[test]
    fn reset_render_state_hook_fields() {
        // tui-main-screen.ts:158-166
        let _lock = global_test_lock();
        let (screen, _terminal) = recording_screen(80, 24, None);
        let mut screen = screen.borrow_mut();
        screen.previous_lines = vec!["x".into()];
        screen.previous_width = 80;
        screen.previous_height = 24;
        screen.cursor_row = 5;
        screen.hardware_cursor_row = 5;
        screen.max_lines_rendered = 5;
        screen.previous_viewport_top = 5;
        screen.reset_render_state_hook();
        assert!(screen.previous_lines.is_empty());
        assert_eq!(screen.previous_width, -1);
        assert_eq!(screen.previous_height, -1);
        assert_eq!(screen.cursor_row, 0);
        assert_eq!(screen.hardware_cursor_row, 0);
        assert_eq!(screen.max_lines_rendered, 0);
        assert_eq!(screen.previous_viewport_top, 0);
    }

    // ----- integration: render paths against a recording terminal -----

    #[test]
    fn first_render_outputs_everything_without_clearing() {
        // tui-main-screen.ts:344-348
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b", "c"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 1);
        assert_eq!(
            terminal.writes[0],
            format!(
                "\x1b[?2026h{}\r\n{}\r\n{}\x1b[?2026l",
                rl("a"),
                rl("b"),
                rl("c")
            )
        );
        // No cursor position → hardware cursor hidden.
        assert_eq!(terminal.hide_cursor_calls, 1);
        assert_eq!(terminal.show_cursor_calls, 0);
        // The first render still counts as a full render.
        assert_eq!(screen.borrow().full_redraws(), 1);
    }

    #[test]
    fn differential_render_updates_only_changed_line() {
        // tui-main-screen.ts:455-540
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b", "c"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        set_lines(&component, &["a", "X", "c"]);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 2);
        // Move up one row (cursor sat on row 2), CR, clear line, write.
        assert_eq!(
            terminal.writes[1],
            format!("\x1b[?2026h\x1b[1A\r\x1b[2K{}\x1b[?2026l", rl("X"))
        );
        assert_eq!(screen.borrow().full_redraws(), 1);
    }

    #[test]
    fn differential_render_appends_lines() {
        // tui-main-screen.ts:369-379 (appendedLines / appendStart)
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        set_lines(&component, &["a", "b", "c", "d"]);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 2);
        // appendStart: newline (scroll) instead of CR, then the new lines.
        assert_eq!(
            terminal.writes[1],
            format!(
                "\x1b[?2026h\r\n\x1b[2K{}\r\n\x1b[2K{}\x1b[?2026l",
                rl("c"),
                rl("d")
            )
        );
    }

    #[test]
    fn differential_render_clears_deleted_lines() {
        // tui-main-screen.ts:398-453 (all changes in deleted lines)
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b", "c", "d"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        set_lines(&component, &["a", "b"]);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 2);
        // Move up 2 to row 1, CR, down 1, clear 2 rows separated by 1B,
        // move back up 2.
        assert_eq!(
            terminal.writes[1],
            "\x1b[?2026h\x1b[2A\r\x1b[1B\r\x1b[2K\x1b[1B\r\x1b[2K\x1b[2A\x1b[?2026l"
        );
        // State tracks the shrunken content.
        let screen = screen.borrow();
        assert_eq!(screen.cursor_row, 1);
        assert_eq!(screen.hardware_cursor_row, 1);
        assert_eq!(screen.previous_lines.len(), 2);
    }

    #[test]
    fn width_change_triggers_clearing_full_redraw() {
        // tui-main-screen.ts:350-355
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        terminal.borrow().cols.set(60);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 2);
        assert!(terminal.writes[1].starts_with("\x1b[?2026h\x1b[2J\x1b[H\x1b[3J"));
        assert_eq!(screen.borrow().full_redraws(), 2);
    }

    #[test]
    fn height_change_triggers_clearing_full_redraw() {
        // tui-main-screen.ts:357-366 (non-Termux environment)
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        terminal.borrow().rows.set(30);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 2);
        assert!(terminal.writes[1].starts_with("\x1b[?2026h\x1b[2J\x1b[H\x1b[3J"));
        assert_eq!(screen.borrow().full_redraws(), 2);
    }

    #[test]
    fn clear_on_shrink_triggers_full_redraw() {
        // tui-main-screen.ts:368-376
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b", "c"]);
        screen.borrow_mut().add_child(component.clone());
        TuiSurface::set_clear_on_shrink(&mut *screen.borrow_mut(), true);
        screen.borrow_mut().render_now(false);

        set_lines(&component, &["a", "b"]);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 2);
        assert!(terminal.writes[1].starts_with("\x1b[?2026h\x1b[2J\x1b[H\x1b[3J"));
        assert_eq!(screen.borrow().full_redraws(), 2);
    }

    #[test]
    fn cursor_marker_positions_hardware_cursor() {
        // tui-main-screen.ts:265-273 (marker stripped before the diff),
        // 624-656 (positionHardwareCursor).
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, Some(true));
        let component = lines_component(&["ab", "z"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);

        {
            let terminal = terminal.borrow();
            // First render: full frame. No marker → cursorPos is null →
            // hideCursor, no cursor move write.
            assert_eq!(terminal.writes.len(), 1);
            assert_eq!(
                terminal.writes[0],
                format!("\x1b[?2026h{}\r\n{}\x1b[?2026l", rl("ab"), rl("z"))
            );
            assert_eq!(terminal.show_cursor_calls, 0);
            assert_eq!(terminal.hide_cursor_calls, 1);
        }

        // Marker appears on row 0: extractCursorPosition strips it before
        // the differential compare, so the stripped lines are unchanged and
        // the render only positions the cursor — up one row from the full
        // render's final row (hardwareCursorRow=1) and to column 3.
        let marked = format!("ab{CURSOR_MARKER}");
        set_lines(&component, &[&marked, "z"]);
        screen.borrow_mut().render_now(false);
        {
            let terminal = terminal.borrow();
            assert_eq!(terminal.writes.len(), 2);
            assert_eq!(terminal.writes[1], "\x1b[1A\x1b[3G");
            assert_eq!(terminal.show_cursor_calls, 1);
        }

        // Change the last line: differential row-1 rewrite (cursor moves
        // down one row to the changed row first), then the cursor moves
        // back up to the marker row at column 3.
        set_lines(&component, &[&marked, "y"]);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 4);
        assert_eq!(
            terminal.writes[2],
            format!("\x1b[?2026h\x1b[1B\r\x1b[2K{}\x1b[?2026l", rl("y"))
        );
        assert_eq!(terminal.writes[3], "\x1b[1A\x1b[3G");
        assert_eq!(terminal.show_cursor_calls, 2);
        assert_eq!(screen.borrow().hardware_cursor_row_field(), 0);
    }

    #[test]
    fn no_change_render_only_updates_cursor_state() {
        // tui-main-screen.ts:392-397 (firstChanged === -1)
        let _lock = global_test_lock();
        let (screen, terminal) = recording_screen(80, 24, None);
        let component = lines_component(&["a", "b"]);
        screen.borrow_mut().add_child(component.clone());
        screen.borrow_mut().render_now(false);
        screen.borrow_mut().render_now(false);

        let terminal = terminal.borrow();
        assert_eq!(terminal.writes.len(), 1);
        assert_eq!(terminal.hide_cursor_calls, 2);
        assert_eq!(screen.borrow().full_redraws(), 1);
    }
}

/// Test-only accessor for `hardware_cursor_row` (used by integration tests
/// in this module). Not part of the TS surface.
#[cfg(test)]
impl TuiMainScreen {
    fn hardware_cursor_row_field(&self) -> i64 {
        self.hardware_cursor_row
    }
}
