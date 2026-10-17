//! Port of `packages/tui/src/components/input.ts` — the `Input` component:
//! single-line text input with horizontal scrolling, placeholder rendering,
//! kill-ring (Emacs kill/yank), and undo support.
//!
//! Divergences from TS:
//! - The `onSubmit` / `onEscape` optional callbacks are
//!   [`Option<Box<dyn Fn>>`] fields (assignable after construction).
//! - The `placeholderStyle` option is [`Option<PlaceholderStyle>`]
//!   (`Box<dyn Fn(&str) -> String>`).
//! - Like the rest of the TUI port, the cursor index is a **UTF-16
//!   code-unit index** into `value` (JS `String.length` / `String.slice`
//!   semantics, matching `word-navigation.rs`). UTF-16 slicing is lossy for
//!   indices that split a surrogate pair — unreachable in practice, since
//!   every cursor movement is grapheme-aligned.

use crate::component::{
    render_fake_cursor, Component, ComponentMouseResult, ComponentRef, Focusable, TuiMouseButton,
    TuiMouseEvent, TuiMouseEventResult, TuiMouseEventType, CURSOR_MARKER,
};
use crate::keybindings::get_keybindings;
use crate::keys::decode_kitty_printable;
use crate::kill_ring::KillRing;
use crate::undo_stack::UndoStack;
use crate::unicode::grapheme_segments;
use crate::utils::{
    is_whitespace_char, slice_by_column, truncate_to_width, utf16_len, visible_width,
};
use crate::word_navigation::{find_word_backward, find_word_forward};

/// `placeholderStyle?: (text: string) => string` (input.ts:25).
pub type PlaceholderStyle = Box<dyn Fn(&str) -> String>;

/// `InputState` (input.ts:18-21) — undo snapshot.
#[derive(Debug, Clone)]
struct InputState {
    value: String,
    cursor: usize,
}

/// `InputOptions` (input.ts:23-27).
pub struct InputOptions {
    /// Prompt drawn before the text (default `"> "`).
    pub prompt: Option<String>,
    /// Placeholder drawn when the value is empty (default `""`).
    pub placeholder: Option<String>,
    /// Style applied to placeholder text (default identity).
    pub placeholder_style: Option<PlaceholderStyle>,
}

impl Default for InputOptions {
    fn default() -> Self {
        InputOptions {
            prompt: None,
            placeholder: None,
            placeholder_style: None,
        }
    }
}

impl InputOptions {
    pub fn new() -> Self {
        InputOptions::default()
    }
}

/// `lastAction: "kill" | "yank" | "type-word" | null` (input.ts:55).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

/// JS `String.prototype.slice(start, end)` over UTF-16 code units
/// (indices clamped, exactly like the JS negative-free calls in input.ts).
fn utf16_slice(s: &str, start: usize, end: usize) -> String {
    let len = utf16_len(s);
    let start = start.min(len);
    let end = end.clamp(start, len);
    if start == end {
        return String::new();
    }
    let units: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16_lossy(&units[start..end])
}

/// UTF-16 length of the last grapheme cluster in `s`, or `1` when `s` has
/// no graphemes (input.ts:270-273 `lastGrapheme ? lastGrapheme.segment.length : 1`).
fn last_grapheme_u16_len(s: &str) -> usize {
    grapheme_segments(s)
        .last()
        .map(|g| utf16_len(g))
        .unwrap_or(1)
}

/// UTF-16 length of the first grapheme cluster in `s`, or `1`.
fn first_grapheme_u16_len(s: &str) -> usize {
    grapheme_segments(s)
        .first()
        .map(|g| utf16_len(g))
        .unwrap_or(1)
}

/// Input component — single-line text input with horizontal scrolling
/// (input.ts:29-501).
pub struct Input {
    /// Current value (input.ts:33).
    value: String,
    /// Cursor position in the value, UTF-16 code units (input.ts:35).
    cursor: usize,
    /// Read-only prompt (input.ts:37).
    prompt: String,
    /// Read-only placeholder (input.ts:38).
    placeholder: String,
    /// Read-only placeholder style (input.ts:39).
    placeholder_style: PlaceholderStyle,
    /// First visible column of the last render, for mouse hit mapping
    /// (input.ts:40).
    rendered_start_column: usize,
    /// `onSubmit?: (value: string) => void` (input.ts:39).
    pub on_submit: Option<Box<dyn Fn(&str)>>,
    /// `onEscape?: () => void` (input.ts:40).
    pub on_escape: Option<Box<dyn Fn()>>,
    /// Focusable interface — set by TUI when focus changes (input.ts:43).
    focused: bool,
    /// Bracketed paste mode buffering (input.ts:46-49).
    paste_buffer: String,
    is_in_paste: bool,
    /// Kill ring for Emacs-style kill/yank operations (input.ts:52).
    kill_ring: KillRing,
    /// `lastAction: "kill" | "yank" | "type-word" | null` (input.ts:53).
    last_action: Option<LastAction>,
    /// Undo support (input.ts:54).
    undo_stack: UndoStack<InputState>,
}

impl Input {
    /// `constructor(options: InputOptions = {})` (input.ts:56-61).
    pub fn with_options(options: InputOptions) -> Self {
        Input {
            value: String::new(),
            cursor: 0,
            prompt: options.prompt.unwrap_or_else(|| "> ".to_string()),
            placeholder: options.placeholder.unwrap_or_default(),
            placeholder_style: options
                .placeholder_style
                .unwrap_or_else(|| Box::new(|text| text.to_string())),
            rendered_start_column: 0,
            on_submit: None,
            on_escape: None,
            focused: false,
            paste_buffer: String::new(),
            is_in_paste: false,
            kill_ring: KillRing::new(),
            last_action: None,
            undo_stack: UndoStack::new(),
        }
    }

    /// `new Input()` — default options.
    pub fn new() -> Self {
        Input::with_options(InputOptions::default())
    }

    /// `getValue(): string` (input.ts:62-64).
    pub fn get_value(&self) -> &str {
        &self.value
    }

    /// `setValue(value: string): void` (input.ts:66-69) — clamps the cursor
    /// to the new value length.
    pub fn set_value(&mut self, value: impl Into<String>) {
        let value = value.into();
        self.cursor = self.cursor.min(utf16_len(&value));
        self.value = value;
    }

    /// `insertCharacter(char)` (input.ts:254-263) — undo coalescing:
    /// consecutive word chars coalesce into one undo unit.
    fn insert_character(&mut self, chars: &str) {
        // `isWhitespaceChar(char)` — unanchored `/\s/`-style test on the
        // (possibly multi-char) input.
        let is_ws = chars.chars().any(is_whitespace_char);
        if is_ws || self.last_action != Some(LastAction::TypeWord) {
            self.push_undo();
        }
        self.last_action = Some(LastAction::TypeWord);

        let before = utf16_slice(&self.value, 0, self.cursor);
        let after = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.value = format!("{before}{chars}{after}");
        self.cursor += utf16_len(chars);
    }

    /// `handleBackspace()` (input.ts:265-276).
    fn handle_backspace(&mut self) {
        self.last_action = None;
        if self.cursor > 0 {
            self.push_undo();
            let before_cursor = utf16_slice(&self.value, 0, self.cursor);
            let grapheme_length = last_grapheme_u16_len(&before_cursor);
            let start = utf16_slice(&self.value, 0, self.cursor - grapheme_length);
            let end = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
            self.value = format!("{start}{end}");
            self.cursor -= grapheme_length;
        }
    }

    /// `handleForwardDelete()` (input.ts:278-288).
    fn handle_forward_delete(&mut self) {
        self.last_action = None;
        let len = utf16_len(&self.value);
        if self.cursor < len {
            self.push_undo();
            let after_cursor = utf16_slice(&self.value, self.cursor, len);
            let grapheme_length = first_grapheme_u16_len(&after_cursor);
            let start = utf16_slice(&self.value, 0, self.cursor);
            let end = utf16_slice(&self.value, self.cursor + grapheme_length, len);
            self.value = format!("{start}{end}");
        }
    }

    /// `deleteToLineStart()` (input.ts:290-298).
    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted_text = utf16_slice(&self.value, 0, self.cursor);
        self.kill_ring.push(
            &deleted_text,
            /* prepend */ true,
            self.last_action == Some(LastAction::Kill),
        );
        self.last_action = Some(LastAction::Kill);
        self.value = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.cursor = 0;
    }

    /// `deleteToLineEnd()` (input.ts:300-307).
    fn delete_to_line_end(&mut self) {
        let len = utf16_len(&self.value);
        if self.cursor >= len {
            return;
        }
        self.push_undo();
        let deleted_text = utf16_slice(&self.value, self.cursor, len);
        self.kill_ring.push(
            &deleted_text,
            /* prepend */ false,
            self.last_action == Some(LastAction::Kill),
        );
        self.last_action = Some(LastAction::Kill);
        self.value = utf16_slice(&self.value, 0, self.cursor);
    }

    /// `deleteWordBackwards()` (input.ts:309-330).
    fn delete_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }

        // Save lastAction before cursor movement (moveWordBackwards resets it)
        let was_kill = self.last_action == Some(LastAction::Kill);

        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_backwards();
        let delete_from = self.cursor;
        self.cursor = old_cursor;

        let deleted_text = utf16_slice(&self.value, delete_from, self.cursor);
        self.kill_ring
            .push(&deleted_text, /* prepend */ true, was_kill);
        self.last_action = Some(LastAction::Kill);

        let start = utf16_slice(&self.value, 0, delete_from);
        let end = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.value = format!("{start}{end}");
        self.cursor = delete_from;
    }

    /// `deleteWordForward()` (input.ts:332-350).
    fn delete_word_forward(&mut self) {
        let len = utf16_len(&self.value);
        if self.cursor >= len {
            return;
        }

        // Save lastAction before cursor movement (moveWordForwards resets it)
        let was_kill = self.last_action == Some(LastAction::Kill);

        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_forwards();
        let delete_to = self.cursor;
        self.cursor = old_cursor;

        let deleted_text = utf16_slice(&self.value, self.cursor, delete_to);
        self.kill_ring
            .push(&deleted_text, /* prepend */ false, was_kill);
        self.last_action = Some(LastAction::Kill);

        let start = utf16_slice(&self.value, 0, self.cursor);
        let end = utf16_slice(&self.value, delete_to, len);
        self.value = format!("{start}{end}");
    }

    /// `yank()` (input.ts:352-363).
    fn yank(&mut self) {
        let Some(text) = self.kill_ring.peek().map(str::to_string) else {
            return;
        };

        self.push_undo();

        let before = utf16_slice(&self.value, 0, self.cursor);
        let after = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.value = format!("{before}{text}{after}");
        self.cursor += utf16_len(&text);
        self.last_action = Some(LastAction::Yank);
    }

    /// `yankPop()` (input.ts:365-385).
    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }

        self.push_undo();

        // Delete the previously yanked text (still at end of ring before rotation)
        let prev_text = self.kill_ring.peek().unwrap_or("").to_string();
        let new_cursor = self.cursor.saturating_sub(utf16_len(&prev_text));
        let before = utf16_slice(&self.value, 0, new_cursor);
        let after = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.value = format!("{before}{after}");
        self.cursor = new_cursor;

        // Rotate and insert new entry
        self.kill_ring.rotate();
        let text = self.kill_ring.peek().unwrap_or("").to_string();
        let before = utf16_slice(&self.value, 0, self.cursor);
        let after = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.value = format!("{before}{text}{after}");
        self.cursor += utf16_len(&text);
        self.last_action = Some(LastAction::Yank);
    }

    /// `pushUndo()` (input.ts:387-389).
    fn push_undo(&mut self) {
        self.undo_stack.push(&InputState {
            value: self.value.clone(),
            cursor: self.cursor,
        });
    }

    /// `undo()` (input.ts:391-398).
    fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.value = snapshot.value;
        self.cursor = snapshot.cursor;
        self.last_action = None;
    }

    /// `moveWordBackwards()` (input.ts:400-404).
    fn move_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = None;
        self.cursor = find_word_backward(&self.value, self.cursor, None);
    }

    /// `moveWordForwards()` (input.ts:406-410).
    fn move_word_forwards(&mut self) {
        if self.cursor >= utf16_len(&self.value) {
            return;
        }
        self.last_action = None;
        self.cursor = find_word_forward(&self.value, self.cursor, None);
    }

    /// `handlePaste(pastedText)` (input.ts:412-424).
    fn handle_paste(&mut self, pasted_text: &str) {
        self.last_action = None;
        self.push_undo();

        // Clean the pasted text - remove newlines and carriage returns,
        // expand tabs to 4 spaces.
        let clean_text = pasted_text
            .replace("\r\n", "")
            .replace('\r', "")
            .replace('\n', "")
            .replace('\t', "    ");

        // Insert at cursor position
        let before = utf16_slice(&self.value, 0, self.cursor);
        let after = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
        self.value = format!("{before}{clean_text}{after}");
        self.cursor += utf16_len(&clean_text);
    }
}

impl Default for Input {
    fn default() -> Self {
        Input::new()
    }
}

impl Component for Input {
    /// `handleInput(data)` (input.ts:71-233).
    fn handle_input(&mut self, data: &str) {
        // Handle bracketed paste mode
        // Start of paste: \x1b[200~
        // End of paste: \x1b[201~

        // Check if we're starting a bracketed paste
        // (TS String.replace with a string replaces only the first occurrence)
        let mut data = data.to_string();
        if data.contains("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            data = data.replacen("\x1b[200~", "", 1);
        }

        // If we're in a paste, buffer the data
        if self.is_in_paste {
            // Check if this chunk contains the end marker
            self.paste_buffer.push_str(&data);

            if let Some(end_index) = self.paste_buffer.find("\x1b[201~") {
                // Extract the pasted content
                let paste_content = self.paste_buffer[..end_index].to_string();

                // Process the complete paste
                self.handle_paste(&paste_content);

                // Reset paste state
                self.is_in_paste = false;

                // Handle any remaining input after the paste marker
                // (6 = length of \x1b[201~)
                let remaining = self.paste_buffer[end_index + 6..].to_string();
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input(&remaining);
                }
            }
            return;
        }

        let kb = get_keybindings();

        // Escape/Cancel
        if kb.matches(&data, "tui.select.cancel") {
            if let Some(cb) = self.on_escape.as_ref() {
                cb();
            }
            return;
        }

        // Undo
        if kb.matches(&data, "tui.editor.undo") {
            self.undo();
            return;
        }

        // Submit
        if kb.matches(&data, "tui.input.submit") || data == "\n" {
            if let Some(cb) = self.on_submit.as_ref() {
                cb(&self.value);
            }
            return;
        }

        // Deletion
        if kb.matches(&data, "tui.editor.deleteCharBackward") {
            self.handle_backspace();
            return;
        }

        if kb.matches(&data, "tui.editor.deleteCharForward") {
            self.handle_forward_delete();
            return;
        }

        if kb.matches(&data, "tui.editor.deleteWordBackward") {
            self.delete_word_backwards();
            return;
        }

        if kb.matches(&data, "tui.editor.deleteWordForward") {
            self.delete_word_forward();
            return;
        }

        if kb.matches(&data, "tui.editor.deleteToLineStart") {
            self.delete_to_line_start();
            return;
        }

        if kb.matches(&data, "tui.editor.deleteToLineEnd") {
            self.delete_to_line_end();
            return;
        }

        // Kill ring actions
        if kb.matches(&data, "tui.editor.yank") {
            self.yank();
            return;
        }
        if kb.matches(&data, "tui.editor.yankPop") {
            self.yank_pop();
            return;
        }

        // Cursor movement
        if kb.matches(&data, "tui.editor.cursorLeft") {
            self.last_action = None;
            if self.cursor > 0 {
                let before_cursor = utf16_slice(&self.value, 0, self.cursor);
                self.cursor -= last_grapheme_u16_len(&before_cursor);
            }
            return;
        }

        if kb.matches(&data, "tui.editor.cursorRight") {
            self.last_action = None;
            if self.cursor < utf16_len(&self.value) {
                let after_cursor = utf16_slice(&self.value, self.cursor, utf16_len(&self.value));
                self.cursor += first_grapheme_u16_len(&after_cursor);
            }
            return;
        }

        if kb.matches(&data, "tui.editor.cursorLineStart") {
            self.last_action = None;
            self.cursor = 0;
            return;
        }

        if kb.matches(&data, "tui.editor.cursorLineEnd") {
            self.last_action = None;
            self.cursor = utf16_len(&self.value);
            return;
        }

        if kb.matches(&data, "tui.editor.cursorWordLeft") {
            self.move_word_backwards();
            return;
        }

        if kb.matches(&data, "tui.editor.cursorWordRight") {
            self.move_word_forwards();
            return;
        }

        // Kitty CSI-u printable character (e.g. \x1b[97u for 'a').
        // Terminals with Kitty protocol flag 1 (disambiguate) send CSI-u for
        // all keys, including plain printable characters. Decode before the
        // control-char check since CSI-u sequences contain \x1b which would
        // be rejected.
        if let Some(kitty_printable) = decode_kitty_printable(&data) {
            self.insert_character(&kitty_printable);
            return;
        }

        // Regular character input - accept printable characters including
        // Unicode, but reject control characters (C0: 0x00-0x1F, DEL: 0x7F,
        // C1: 0x80-0x9F)
        let has_control_chars = data.chars().any(|ch| {
            let code = ch as u32;
            code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
        });
        if !has_control_chars {
            self.insert_character(&data);
        }
    }

    /// Input handles keyboard input (TS `handleInput` presence).
    fn handles_input(&self) -> bool {
        true
    }

    /// `handleMouse(event)` (input.ts:236-252).
    fn handle_mouse(
        &mut self,
        _self_ref: &ComponentRef,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        if event.event_type != TuiMouseEventType::Press
            || event.button != TuiMouseButton::Left
            || event.y != 0
        {
            return None;
        }
        let visible_column = (event.x - 2).max(0) as usize;
        let target_column = self.rendered_start_column + visible_column;
        let mut current_column = 0usize;
        self.cursor = utf16_len(&self.value);
        let mut index = 0usize;
        for grapheme in grapheme_segments(&self.value) {
            let next_column = current_column + visible_width(grapheme);
            if target_column < next_column {
                self.cursor = index;
                break;
            }
            current_column = next_column;
            index += utf16_len(grapheme);
        }
        self.last_action = None;
        Some(ComponentMouseResult::Result(TuiMouseEventResult {
            handled: Some(true),
            focus: Some(true),
            ..Default::default()
        }))
    }

    /// `invalidate()` (input.ts:426-428) — no cached state to invalidate.
    fn invalidate(&mut self) {}

    /// `render(width)` (input.ts:430-501).
    fn render(&mut self, width: usize) -> Vec<String> {
        // Calculate visible window
        let available_width = width as isize - visible_width(&self.prompt) as isize;

        if available_width <= 0 {
            return vec![truncate_to_width(&self.prompt, width, "", false)];
        }
        let available_width = available_width as usize;

        if self.value.is_empty() && !self.placeholder.is_empty() {
            let placeholder = truncate_to_width(&self.placeholder, available_width, "", false);
            let graphemes = grapheme_segments(&placeholder);
            let at_cursor = graphemes.first().copied().unwrap_or(" ");
            let after_cursor =
                utf16_slice(&placeholder, utf16_len(at_cursor), utf16_len(&placeholder));
            let marker = if self.focused { CURSOR_MARKER } else { "" };
            let cursor_char = render_fake_cursor(&(self.placeholder_style)(at_cursor));
            let text_with_cursor = format!(
                "{marker}{cursor_char}{}",
                (self.placeholder_style)(&after_cursor)
            );
            let padding =
                " ".repeat(available_width.saturating_sub(visible_width(&text_with_cursor)));
            return vec![format!("{}{text_with_cursor}{padding}", self.prompt)];
        }

        let mut visible_text;
        let mut cursor_display = self.cursor;
        self.rendered_start_column = 0;
        let total_width = visible_width(&self.value);
        let value_len = utf16_len(&self.value);

        if total_width < available_width {
            // Everything fits (leave room for cursor at end)
            visible_text = self.value.clone();
        } else {
            // Need horizontal scrolling
            // Reserve one column for cursor if it's at the end
            let scroll_width = if self.cursor == value_len {
                available_width - 1
            } else {
                available_width
            };
            let cursor_col = visible_width(&utf16_slice(&self.value, 0, self.cursor));

            if scroll_width > 0 {
                let half_width = scroll_width / 2;
                let start_col = if cursor_col < half_width {
                    // Cursor near start
                    0
                } else if cursor_col > total_width.saturating_sub(half_width) {
                    // Cursor near end
                    total_width.saturating_sub(scroll_width)
                } else {
                    // Cursor in middle
                    cursor_col.saturating_sub(half_width)
                };

                self.rendered_start_column = start_col;
                visible_text = slice_by_column(&self.value, start_col, scroll_width, true);
                let before_cursor = slice_by_column(
                    &self.value,
                    start_col,
                    cursor_col.saturating_sub(start_col),
                    true,
                );
                cursor_display = utf16_len(&before_cursor);
            } else {
                visible_text = String::new();
                cursor_display = 0;
            }
        }

        // Build line with fake cursor
        // Insert cursor character at cursor position
        let visible_len = utf16_len(&visible_text);
        let before_cursor = utf16_slice(&visible_text, 0, cursor_display);
        let rest = utf16_slice(&visible_text, cursor_display, visible_len);
        let at_cursor = grapheme_segments(&rest).first().copied().unwrap_or(" ");
        let after_cursor = utf16_slice(
            &visible_text,
            cursor_display + utf16_len(at_cursor),
            visible_len,
        );

        // Hardware cursor marker (zero-width, emitted before fake cursor for
        // IME positioning)
        let marker = if self.focused { CURSOR_MARKER } else { "" };

        // TUI draws the fake cursor in reverse video, or omits it when the
        // hardware cursor is shown
        let cursor_char = render_fake_cursor(at_cursor);
        let text_with_cursor = format!("{before_cursor}{marker}{cursor_char}{after_cursor}");

        // Calculate visual width
        let visual_length = visible_width(&text_with_cursor);
        let padding = " ".repeat(available_width.saturating_sub(visual_length));
        let line = format!("{}{text_with_cursor}{padding}", self.prompt);

        vec![line]
    }
}

impl Focusable for Input {
    /// `focused` getter (input.ts:43).
    fn focused(&self) -> bool {
        self.focused
    }

    /// `focused` setter (input.ts:43) — set by TUI when focus changes.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

// =============================================================================
// Tests — port of packages/tui/test/input.test.ts (664 lines, 36 it() blocks)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::global_test_lock;
    use crate::utils::strip_terminal_sequences;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Type each character of `text` as an individual `handleInput` call,
    /// as the upstream tests do.
    fn type_text(input: &mut Input, text: &str) {
        for ch in text.chars() {
            let s = ch.to_string();
            input.handle_input(&s);
        }
    }

    fn move_right(input: &mut Input, count: usize) {
        for _ in 0..count {
            input.handle_input("\x1b[C");
        }
    }

    // -------------------------------------------------------------------------
    // Top level
    // -------------------------------------------------------------------------

    #[test]
    fn submits_value_including_backslash_on_enter() {
        let _guard = global_test_lock();
        let mut input = Input::new();
        let submitted = Rc::new(RefCell::new(None::<String>));
        let sink = submitted.clone();
        input.on_submit = Some(Box::new(move |value| {
            *sink.borrow_mut() = Some(value.to_string());
        }));

        // Type hello, then backslash, then Enter
        type_text(&mut input, "hello");
        input.handle_input("\\");
        input.handle_input("\r");

        // Input is single-line, no backslash+Enter workaround
        assert_eq!(*submitted.borrow(), Some("hello\\".to_string()));
    }

    #[test]
    fn inserts_backslash_as_regular_character() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.handle_input("\\");
        input.handle_input("x");

        assert_eq!(input.get_value(), "\\x");
    }

    // -------------------------------------------------------------------------
    // describe("render")
    // -------------------------------------------------------------------------

    #[test]
    fn render_supports_a_custom_prompt_and_styled_placeholder() {
        let _guard = global_test_lock();
        let mut input = Input::with_options(InputOptions {
            prompt: Some(String::new()),
            placeholder: Some("Find transcript".to_string()),
            placeholder_style: Some(Box::new(|text: &str| format!("\x1b[2m{text}\x1b[22m"))),
        });
        input.set_focused(true);

        let empty = input.render(20).remove(0);
        assert!(empty.contains("\x1b[2m"));
        assert_eq!(
            strip_terminal_sequences(&empty).trim_end(),
            "Find transcript"
        );

        input.handle_input("n");
        let populated = input.render(20).remove(0);
        assert_eq!(strip_terminal_sequences(&populated).trim_end(), "n");
    }

    #[test]
    fn render_does_not_overflow_with_wide_cjk_and_fullwidth_text() {
        let _guard = global_test_lock();
        let width = 93;
        let cases = [
            "가나다라마바사아자차카타파하 한글 텍스트가 터미널 너비를 초과하면 크래시가 발생합니다 이것은 재현용 테스트입니다",
            "これはテスト文章です。日本語のテキストが正しく表示されるかどうかを確認するためのサンプルテキストです。あいうえお",
            "这是一段测试文本，用于验证中文字符在终端中的显示宽度是否被正确计算，如果不正确就会导致用户界面崩溃的问题",
            "ＡＢＣＤＥＦＧＨＩＪＫＬＭＮＯＰＱＲＳＴＵＶＷＸＹＺ０１２３４５６７８９ａｂｃｄｅｆｇｈｉｊｋｌｍ",
        ];
        // (label, move): start = no move, middle = 10× right, end = Ctrl+E
        let cursor_positions: [(&str, i64); 3] = [("start", 0), ("middle", 10), ("end", -1)];

        for text in cases {
            for (label, mv) in cursor_positions {
                let mut input = Input::new();
                input.set_value(text);
                input.set_focused(true);
                if mv == -1 {
                    input.handle_input("\x05");
                } else {
                    move_right(&mut input, mv as usize);
                }

                let line = input.render(width).remove(0);
                assert!(
                    visible_width(&line) <= width,
                    "rendered line overflowed for {text} at {label}"
                );
            }
        }
    }

    #[test]
    fn render_keeps_the_cursor_visible_when_horizontally_scrolling_wide_text() {
        let _guard = global_test_lock();
        let mut input = Input::new();
        let width = 20;
        let text = "가나다라마바사아자차카타파하";
        input.set_value(text);
        input.set_focused(true);
        input.handle_input("\x01");
        move_right(&mut input, 5);

        let line = input.render(width).remove(0);
        assert!(visible_width(&line) <= width);
    }

    // -------------------------------------------------------------------------
    // describe("Kill ring")
    // -------------------------------------------------------------------------

    #[test]
    fn ctrl_w_saves_deleted_text_to_kill_ring_and_ctrl_y_yanks_it() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("foo bar baz");
        // Move cursor to end
        input.handle_input("\x05"); // Ctrl+E

        input.handle_input("\x17"); // Ctrl+W - deletes "baz"
        assert_eq!(input.get_value(), "foo bar ");

        // Move to beginning and yank
        input.handle_input("\x01"); // Ctrl+A
        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "bazfoo bar ");
    }

    #[test]
    fn ctrl_w_preserves_ascii_punctuation_boundaries() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("foo.bar");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "bar"
        assert_eq!(input.get_value(), "foo.");

        input.set_value("foo:bar");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "bar"
        assert_eq!(input.get_value(), "foo:");
    }

    #[test]
    fn ctrl_w_handles_unicode_word_boundaries() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        // "你好世界。你好，世界" segments as: 你好|世界|。|你好|，|世界
        input.set_value("你好世界。你好，世界");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "世界"
        assert_eq!(input.get_value(), "你好世界。你好，");
        input.handle_input("\x17"); // Ctrl+W - deletes "，"
        assert_eq!(input.get_value(), "你好世界。你好");
        input.handle_input("\x17"); // Ctrl+W - deletes "你好"
        assert_eq!(input.get_value(), "你好世界。");
        input.handle_input("\x17"); // Ctrl+W - deletes "。"
        assert_eq!(input.get_value(), "你好世界");
        input.handle_input("\x17"); // Ctrl+W - deletes "世界"
        assert_eq!(input.get_value(), "你好");
        input.handle_input("\x17"); // Ctrl+W - deletes "你好"
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn ctrl_u_saves_deleted_text_to_kill_ring() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("hello world");
        // Move cursor to after "hello "
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 6);

        input.handle_input("\x15"); // Ctrl+U - deletes "hello "
        assert_eq!(input.get_value(), "world");

        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn ctrl_k_saves_deleted_text_to_kill_ring() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("hello world");
        input.handle_input("\x01"); // Ctrl+A
        input.handle_input("\x0b"); // Ctrl+K - deletes "hello world"

        assert_eq!(input.get_value(), "");

        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn ctrl_y_does_nothing_when_kill_ring_is_empty() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("test");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "test");
    }

    #[test]
    fn alt_y_cycles_through_kill_ring_after_ctrl_y() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        // Create kill ring with multiple entries
        input.set_value("first");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "first"
        input.set_value("second");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "second"
        input.set_value("third");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "third"

        assert_eq!(input.get_value(), "");

        input.handle_input("\x19"); // Ctrl+Y - yanks "third"
        assert_eq!(input.get_value(), "third");

        input.handle_input("\x1by"); // Alt+Y - cycles to "second"
        assert_eq!(input.get_value(), "second");

        input.handle_input("\x1by"); // Alt+Y - cycles to "first"
        assert_eq!(input.get_value(), "first");

        input.handle_input("\x1by"); // Alt+Y - cycles back to "third"
        assert_eq!(input.get_value(), "third");
    }

    #[test]
    fn alt_y_does_nothing_if_not_preceded_by_yank() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("test");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "test"
        input.set_value("other");
        input.handle_input("\x05"); // Ctrl+E

        // Type something to break the yank chain
        input.handle_input("x");
        assert_eq!(input.get_value(), "otherx");

        input.handle_input("\x1by"); // Alt+Y - should do nothing
        assert_eq!(input.get_value(), "otherx");
    }

    #[test]
    fn alt_y_does_nothing_if_kill_ring_has_one_entry() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("only");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "only"

        input.handle_input("\x19"); // Ctrl+Y - yanks "only"
        assert_eq!(input.get_value(), "only");

        input.handle_input("\x1by"); // Alt+Y - should do nothing
        assert_eq!(input.get_value(), "only");
    }

    #[test]
    fn consecutive_ctrl_w_accumulates_into_one_kill_ring_entry() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("one two three");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "three"
        input.handle_input("\x17"); // Ctrl+W - deletes "two "
        input.handle_input("\x17"); // Ctrl+W - deletes "one "

        assert_eq!(input.get_value(), "");

        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "one two three");
    }

    #[test]
    fn non_delete_actions_break_kill_accumulation() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("foo bar baz");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "baz"
        assert_eq!(input.get_value(), "foo bar ");

        input.handle_input("x"); // Typing breaks accumulation
        assert_eq!(input.get_value(), "foo bar x");

        input.handle_input("\x17"); // Ctrl+W - deletes "x" (separate entry)
        assert_eq!(input.get_value(), "foo bar ");

        input.handle_input("\x19"); // Ctrl+Y - most recent is "x"
        assert_eq!(input.get_value(), "foo bar x");

        input.handle_input("\x1by"); // Alt+Y - cycle to "baz"
        assert_eq!(input.get_value(), "foo bar baz");
    }

    #[test]
    fn non_yank_actions_break_alt_y_chain() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("first");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W
        input.set_value("second");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W
        input.set_value("");

        input.handle_input("\x19"); // Ctrl+Y - yanks "second"
        assert_eq!(input.get_value(), "second");

        input.handle_input("x"); // Breaks yank chain
        assert_eq!(input.get_value(), "secondx");

        input.handle_input("\x1by"); // Alt+Y - should do nothing
        assert_eq!(input.get_value(), "secondx");
    }

    #[test]
    fn kill_ring_rotation_persists_after_cycling() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("first");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // deletes "first"
        input.set_value("second");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // deletes "second"
        input.set_value("third");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // deletes "third"
        input.set_value("");

        input.handle_input("\x19"); // Ctrl+Y - yanks "third"
        input.handle_input("\x1by"); // Alt+Y - cycles to "second"
        assert_eq!(input.get_value(), "second");

        // Break chain and start fresh
        input.handle_input("x");
        input.set_value("");

        // New yank should get "second" (now at end after rotation)
        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "second");
    }

    #[test]
    fn backward_deletions_prepend_forward_deletions_append_during_accumulation() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("prefix|suffix");
        // Position cursor at "|"
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 6);

        input.handle_input("\x0b"); // Ctrl+K - deletes "|suffix" (forward)
        assert_eq!(input.get_value(), "prefix");

        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "prefix|suffix");
    }

    #[test]
    fn alt_d_deletes_word_forward_and_saves_to_kill_ring() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("hello world test");
        input.handle_input("\x01"); // Ctrl+A

        input.handle_input("\x1bd"); // Alt+D - deletes "hello"
        assert_eq!(input.get_value(), " world test");

        input.handle_input("\x1bd"); // Alt+D - deletes " world"
        assert_eq!(input.get_value(), " test");

        // Yank should get accumulated text
        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "hello world test");
    }

    #[test]
    fn alt_d_preserves_ascii_punctuation_boundaries() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("foo.bar baz");
        input.handle_input("\x01"); // Ctrl+A
        input.handle_input("\x1bd"); // Alt+D - deletes "foo"
        assert_eq!(input.get_value(), ".bar baz");
        input.handle_input("\x1bd"); // Alt+D - deletes "."
        assert_eq!(input.get_value(), "bar baz");
        input.handle_input("\x1bd"); // Alt+D - deletes "bar"
        assert_eq!(input.get_value(), " baz");
    }

    #[test]
    fn alt_d_handles_unicode_word_boundaries() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        // "你好世界。你好，世界" segments as: 你好|世界|。|你好|，|世界
        input.set_value("你好世界。你好，世界");
        input.handle_input("\x01"); // Ctrl+A
        input.handle_input("\x1bd"); // Alt+D - deletes "你好"
        assert_eq!(input.get_value(), "世界。你好，世界");
        input.handle_input("\x1bd"); // Alt+D - deletes "世界"
        assert_eq!(input.get_value(), "。你好，世界");
        input.handle_input("\x1bd"); // Alt+D - deletes "。"
        assert_eq!(input.get_value(), "你好，世界");
        input.handle_input("\x1bd"); // Alt+D - deletes "你好"
        assert_eq!(input.get_value(), "，世界");
        input.handle_input("\x1bd"); // Alt+D - deletes "，"
        assert_eq!(input.get_value(), "世界");
        input.handle_input("\x1bd"); // Alt+D - deletes "世界"
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn handles_yank_in_middle_of_text() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("word");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "word"
        input.set_value("hello world");
        // Move to middle (after "hello ")
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 6);

        input.handle_input("\x19"); // Ctrl+Y
        assert_eq!(input.get_value(), "hello wordworld");
    }

    #[test]
    fn handles_yank_pop_in_middle_of_text() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        // Create two kill ring entries
        input.set_value("FIRST");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "FIRST"
        input.set_value("SECOND");
        input.handle_input("\x05"); // Ctrl+E
        input.handle_input("\x17"); // Ctrl+W - deletes "SECOND"

        // Set up "hello world" and position cursor after "hello "
        input.set_value("hello world");
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 6);

        input.handle_input("\x19"); // Ctrl+Y - yanks "SECOND"
        assert_eq!(input.get_value(), "hello SECONDworld");

        input.handle_input("\x1by"); // Alt+Y - replaces with "FIRST"
        assert_eq!(input.get_value(), "hello FIRSTworld");
    }

    // -------------------------------------------------------------------------
    // describe("Undo")
    // -------------------------------------------------------------------------

    #[test]
    fn does_nothing_when_undo_stack_is_empty() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn coalesces_consecutive_word_characters_into_one_undo_unit() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello world");
        assert_eq!(input.get_value(), "hello world");

        // Undo removes " world"
        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello");

        // Undo removes "hello"
        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn undoes_spaces_one_at_a_time() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello  ");
        assert_eq!(input.get_value(), "hello  ");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes second " "
        assert_eq!(input.get_value(), "hello ");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes first " "
        assert_eq!(input.get_value(), "hello");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes "hello"
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn undoes_backspace() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello");
        input.handle_input("\x7f"); // Backspace
        assert_eq!(input.get_value(), "hell");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello");
    }

    #[test]
    fn undoes_forward_delete() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello");
        input.handle_input("\x01"); // Ctrl+A - go to start
        input.handle_input("\x1b[C"); // Right arrow
        input.handle_input("\x1b[3~"); // Delete key
        assert_eq!(input.get_value(), "hllo");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello");
    }

    #[test]
    fn undoes_ctrl_w_delete_word_backward() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello world");
        assert_eq!(input.get_value(), "hello world");

        input.handle_input("\x17"); // Ctrl+W
        assert_eq!(input.get_value(), "hello ");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn undoes_ctrl_k_delete_to_line_end() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello world");
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 6);

        input.handle_input("\x0b"); // Ctrl+K
        assert_eq!(input.get_value(), "hello ");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn undoes_ctrl_u_delete_to_line_start() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello world");
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 6);

        input.handle_input("\x15"); // Ctrl+U
        assert_eq!(input.get_value(), "world");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn undoes_yank() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "hello ");
        input.handle_input("\x17"); // Ctrl+W - delete "hello "
        input.handle_input("\x19"); // Ctrl+Y - yank
        assert_eq!(input.get_value(), "hello ");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn undoes_paste_atomically() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("hello world");
        input.handle_input("\x01"); // Ctrl+A
        move_right(&mut input, 5);

        // Simulate bracketed paste
        input.handle_input("\x1b[200~beep boop\x1b[201~");
        assert_eq!(input.get_value(), "hellobeep boop world");

        // Single undo should restore entire pre-paste state
        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn undoes_alt_d_delete_word_forward() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        input.set_value("hello world");
        input.handle_input("\x01"); // Ctrl+A

        input.handle_input("\x1bd"); // Alt+D - deletes "hello"
        assert_eq!(input.get_value(), " world");

        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn cursor_movement_starts_new_undo_unit() {
        let _guard = global_test_lock();
        let mut input = Input::new();

        type_text(&mut input, "abc");
        input.handle_input("\x01"); // Ctrl+A - movement breaks coalescing
        input.handle_input("\x05"); // Ctrl+E
        type_text(&mut input, "de");
        assert_eq!(input.get_value(), "abcde");

        // Undo removes "de" (typed after movement)
        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "abc");

        // Undo removes "abc"
        input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(input.get_value(), "");
    }
}
