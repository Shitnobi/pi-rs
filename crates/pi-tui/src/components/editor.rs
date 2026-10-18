//! Port of `packages/tui/src/components/editor.ts` (2473 LOC) — the
//! `Editor` component: multi-line text editing with word wrap, paste
//! markers, prompt history, kill ring, undo, character jump mode, sticky
//! column, and autocomplete.
//!
//! ## Conventions
//!
//! - **UTF-16 code-unit indexing**: `cursor_col` and all text positions are
//!   UTF-16 units (JS `String` semantics) — see `word_navigation.rs`.
//! - The `tui` host is [`crate::component::TuiHost`] (the only `TUI`
//!   members editor.ts consumes: requestRender + terminal.rows).
//! - Pure helpers (`is_paste_marker`, `segment_*_with_markers`,
//!   `word_wrap_line`) live in [`super::editor_word_wrap`].
//!
//! ## File layout (single-owner `impl Editor` blocks)
//!
//! This file is the coordinator-owned foundation: types, the full struct,
//! constructor, trivial accessors, the synchronous autocomplete guard
//! chain, and the pattern matcher. Method bodies live in sibling files:
//! - `editor_text.rs` — text mutation (insert/setText/paste/delete/submit)
//! - `editor_cursor.rs` — cursor movement, visual line map, jump mode
//! - `editor_history.rs` — history navigation, kill/yank, undo
//! - `editor_render.rs` — `render`, `layout_text`, mouse hit-testing
//! - `editor_input.rs` — the `handleInput` dispatch (editor.ts:696-998)
//! - `editor_autocomplete.rs` — the async autocomplete pipeline [T4-D1b]

#![allow(dead_code)] // TODO(T4): remove once the E2–E6/D1b method bodies land

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use crate::component::{
    Component, ComponentMouseResult, Focusable, TuiHost, TuiMouseEvent,
};
use crate::components::autocomplete::AutocompleteProvider;
use crate::components::editor_word_wrap::{segment_graphemes_with_markers, segment_words_with_markers, GraphemeSegment};
use crate::components::select_list::{SelectList, SelectListTheme};
use crate::kill_ring::KillRing;
use crate::unicode::word_break::WordSegment;
use crate::undo_stack::UndoStack;
use crate::utils::{is_cjk_punctuation_char, is_js_whitespace, slice_by_column, visible_width};

/// `ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS` (editor.ts:253).
pub(crate) const ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS: u64 = 20;

/// `DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS` (editor.ts:254).
pub(crate) const DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS: [char; 2] = ['@', '#'];

// =============================================================================
// Types (editor.ts:220-246)
// =============================================================================

/// `EditorState` (editor.ts:219-223). `cursor_col` is a UTF-16 code-unit
/// index into `lines[cursor_line]`.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorState {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

/// `EditorSnapshot` (editor.ts:225-230): editor text state plus the paste
/// registry (undo restores both).
#[derive(Debug, Clone)]
pub(crate) struct EditorSnapshot {
    pub state: EditorState,
    pub pastes: BTreeMap<u32, String>,
    pub paste_counter: u32,
}

/// `LayoutLine` (editor.ts:232-236): one rendered (wrapped) line.
/// `cursor_pos` is a UTF-16 index into `text`.
#[derive(Debug, Clone)]
pub(crate) struct LayoutLine {
    pub text: String,
    pub has_cursor: bool,
    pub cursor_pos: Option<usize>,
}

/// A visual line reference from `buildVisualLineMap` (editor.ts:1836-1864):
/// logical line index plus the UTF-16 column range of the wrapped chunk.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VisualLineRef {
    pub logical_line: usize,
    pub start_col: usize,
    pub length: usize,
}

/// `EditorTheme` (editor.ts:238-241). `select_list` is shared (`Rc`) because
/// `createAutocompleteList` hands the same theme object to the owned
/// `SelectList` (editor.ts:2277-2284) and `SelectListTheme` holds
/// `Box<dyn Fn>` (not `Clone`).
pub struct EditorTheme {
    pub border_color: Box<dyn Fn(&str) -> String>,
    pub select_list: Rc<SelectListTheme>,
}

impl Default for EditorTheme {
    fn default() -> Self {
        EditorTheme {
            border_color: Box::new(|t| t.to_string()),
            select_list: Rc::new(SelectListTheme::default()),
        }
    }
}

/// `EditorOptions` (editor.ts:243-246). JS `number` inputs (`?` optional):
/// `None` = TS `undefined`.
#[derive(Debug, Clone, Default)]
pub struct EditorOptions {
    pub padding_x: Option<f64>,
    pub autocomplete_max_visible: Option<f64>,
}

/// `autocompleteState` (editor.ts:344): `"regular" | "force" | null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutocompleteMode {
    Regular,
    Force,
}

/// `lastAction` (editor.ts:358): `"kill" | "yank" | "type-word" | null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorAction {
    Kill,
    Yank,
    TypeWord,
}

/// `jumpMode` (editor.ts:362): `"forward" | "backward" | null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JumpDirection {
    Forward,
    Backward,
}

/// A pending (debounced or immediate) autocomplete request — the Rust model
/// of the TS `setTimeout` callback closure (editor.ts:2297-2301) and the
/// `startAutocompleteRequest` promise task. The async pipeline that fires
/// it lands with [T4-D1b] in `editor_autocomplete.rs`.
#[derive(Debug, Clone)]
pub(crate) struct PendingAutocompleteRequest {
    pub start_token: u64,
    pub force: bool,
    pub explicit_tab: bool,
}

// =============================================================================
// UTF-16 helpers (JS `String` semantics; see word_navigation.rs)
// =============================================================================

/// Byte offset of the char whose UTF-16 range contains `units` (snaps DOWN
/// to the char start). `units` beyond the end returns the byte length.
pub(crate) fn utf16_to_byte(line: &str, units: usize) -> usize {
    let mut acc = 0usize;
    for (byte, ch) in line.char_indices() {
        if acc >= units {
            return byte;
        }
        acc += ch.len_utf16();
    }
    line.len()
}

/// JS `String.prototype.slice(start, end)` over UTF-16 code units
/// (positive indices, clamped; `start >= end` → `""`).
///
/// Divergence note: JS can split a surrogate pair mid-slice producing a
/// lone surrogate; Rust `String` cannot hold one, so indices snap down to
/// char boundaries. No upstream test splits a surrogate (all positions
/// come from grapheme boundaries).
pub(crate) fn slice_utf16(line: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }
    let s = utf16_to_byte(line, start);
    let e = utf16_to_byte(line, end);
    if s >= e {
        return String::new();
    }
    line[s..e].to_string()
}

/// UTF-16 length of the prefix ending at byte offset `byte` (char boundary).
pub(crate) fn utf16_prefix_len(line: &str, byte: usize) -> usize {
    line[..byte].encode_utf16().count()
}

/// JS `String.prototype.indexOf(needle, fromIndex)` over UTF-16 units.
pub(crate) fn js_index_of(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(crate::utils::utf16_len(haystack)));
    }
    let start_byte = utf16_to_byte(haystack, from);
    let mut acc = from;
    let tail = &haystack[start_byte..];
    for (byte, first) in tail.char_indices() {
        if haystack[start_byte + byte..].starts_with(needle) {
            return Some(acc);
        }
        acc += first.len_utf16();
    }
    None
}

/// JS `String.prototype.lastIndexOf(needle, fromIndex)` over UTF-16 units
/// (`from_index` clamped to the string length; `None` = no fromIndex).
pub(crate) fn js_last_index_of(haystack: &str, needle: &str, from_index: Option<usize>) -> Option<usize> {
    if needle.is_empty() {
        return Some(from_index.unwrap_or(crate::utils::utf16_len(haystack)));
    }
    let limit = from_index.unwrap_or(crate::utils::utf16_len(haystack));
    // Collect all match positions (UTF-16), take the last <= limit.
    let mut best: Option<usize> = None;
    let mut acc = 0usize;
    for (byte, first) in haystack.char_indices() {
        if acc + needle_len_u16(needle) <= limit {
            if haystack[byte..].starts_with(needle) {
                best = Some(acc);
            }
        } else {
            break;
        }
        acc += first.len_utf16();
    }
    best
}

fn needle_len_u16(needle: &str) -> usize {
    crate::utils::utf16_len(needle)
}

// =============================================================================
// Pattern matcher (editor.ts:256-277) + scroll border (298-313)
// =============================================================================

/// `autocompleteSeparatorRegex.test(char)` (utils.ts:62): `\s` or CJK
/// punctuation. Same predicates as `components::autocomplete`.
pub(crate) fn is_autocomplete_separator(c: char) -> bool {
    is_js_whitespace(c) || is_cjk_punctuation_char(c)
}

/// `autocompleteTokenStartSource` wrappers (editor.ts:258):
/// `[([{<\`]*` after the boundary.
fn is_token_wrapper(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '<' | '`')
}

/// Port of `buildTriggerPattern(triggerCharacters)` /
/// `buildDebouncePattern(triggerCharacters)` `.test(text)` (editor.ts:264-277).
///
/// Trigger pattern (end-anchored):
/// `BOUNDARY [([{<\`]* (?:@"[^"]*|[chars]SUFFIX)$`
/// Debounce pattern (end-anchored, `@` branch may be unterminated):
/// `BOUNDARY [([{<\`]* (?:@(?:"[^"]*|SUFFIX)|[chars-without-@]SUFFIX)$`
///
/// where `BOUNDARY = ^|separator`, `SUFFIX = (?:(?!separator).)*`.
/// The regex is unanchored at the start, so a match may begin at any
/// boundary position — this scans every boundary candidate.
pub(crate) fn autocomplete_token_pattern_test(
    text: &str,
    trigger_characters: &[char],
    debounce: bool,
) -> bool {
    let units: Vec<char> = text.chars().collect();
    for i in 0..=units.len() {
        // BOUNDARY: start-of-string or the preceding char is a separator.
        if i > 0 && !is_autocomplete_separator(units[i - 1]) {
            continue;
        }
        // Wrappers: `[([{<\`]*` — try every wrapper-run length.
        let mut j = i;
        loop {
            if token_matches_at(&units, j, trigger_characters, debounce) {
                return true;
            }
            if j < units.len() && is_token_wrapper(units[j]) {
                j += 1;
            } else {
                break;
            }
        }
    }
    false
}

/// Does a trigger/debounce token start at char index `j` and run to the end?
///
/// Mirrors the TS alternation `(?:@"[^"]*|[chars]SUFFIX)$` (trigger,
/// editor.ts:264-270) / `(?:@(?:"[^"]*|SUFFIX)|[chars-without-@]SUFFIX)$`
/// (debounce, editor.ts:271-278), including regex backtracking: when the
/// `@"` branch fails, the char-class alternative is tried at the same
/// position.
fn token_matches_at(units: &[char], j: usize, trigger_characters: &[char], debounce: bool) -> bool {
    if j >= units.len() {
        return false;
    }
    // `@"[^"]*` — the quoted branch, identical in BOTH patterns and
    // unconditional (independent of trigger_characters): no closing quote
    // is required, but the remainder after `@"` must contain no quote
    // (`[^"]*` cannot match one, and `$` then fails).
    if units[j] == '@' && units.get(j + 1) == Some(&'"') && units[j + 2..].iter().all(|&c| c != '"') {
        return true;
    }
    // `SUFFIX` — the remainder after the trigger char is a non-separator run.
    let suffix_ok = units[j + 1..].iter().all(|&c| !is_autocomplete_separator(c));
    // `[chars]SUFFIX` — the debounce char class excludes `@` (it has its
    // own alternatives).
    let in_class = trigger_characters.contains(&units[j]) && !(debounce && units[j] == '@');
    if in_class {
        return suffix_ok;
    }
    // Debounce-only unquoted `@SUFFIX` alternative.
    if debounce && units[j] == '@' {
        return suffix_ok;
    }
    false
}

/// `createScrollBorder` (editor.ts:279-295): centered `↑/↓ N more` label
/// inside a `─` border, falling back to a left-aligned indicator and then
/// an ellipsis-truncated form on narrow widths.
pub(crate) fn create_scroll_border(direction: char, hidden_line_count: usize, width: usize) -> String {
    let available_width = width;
    let label = format!(" {direction} {hidden_line_count} more ");
    let label_width = visible_width(&label);
    if label_width + 2 <= available_width {
        let left_width = (available_width - label_width) / 2;
        let mut out = "─".repeat(left_width);
        out.push_str(&label);
        out.push_str(&"─".repeat(available_width - left_width - label_width));
        return out;
    }

    let indicator = format!("─── {direction} {hidden_line_count} more ");
    let remaining = available_width as i64 - visible_width(&indicator) as i64;
    if remaining >= 0 {
        let mut out = indicator;
        out.push_str(&"─".repeat(remaining as usize));
        return out;
    }

    let ellipsis: String = "...".chars().take(available_width).collect();
    let indicator_width = available_width - visible_width(&ellipsis);
    let mut out = slice_by_column(&indicator, 0, indicator_width, true);
    out.push_str(&ellipsis);
    out
}

// =============================================================================
// Editor struct (editor.ts:297-383)
// =============================================================================

/// TS `(text: string) => void` — the public `onSubmit` / `onChange`
/// callback properties (editor.ts:390-391).
pub type EditorCallback = Box<dyn FnMut(&str)>;

pub struct Editor {
    /// `EditorState` (editor.ts:299-303). `cursor_col` is UTF-16 units.
    pub(crate) state: EditorState,

    /// `Focusable.focused` (editor.ts:307) — set by the TUI (T5).
    pub(crate) focused: bool,

    pub(crate) tui: Rc<RefCell<dyn TuiHost>>,

    /// The select-list half of `EditorTheme` (only consumed by
    /// `createAutocompleteList`, editor.ts:2280).
    pub(crate) select_list_theme: Rc<SelectListTheme>,

    /// TS `borderColor` is a public mutable field initialized from
    /// `theme.borderColor` (editor.ts:323).
    pub border_color: Box<dyn Fn(&str) -> String>,

    pub(crate) padding_x: usize,

    // Last render geometry for cursor navigation and mouse hit-testing
    // (editor.ts:327-330).
    pub(crate) last_width: usize,
    pub(crate) rendered_visible_line_count: usize,
    pub(crate) rendered_autocomplete_height: usize,

    /// Vertical scrolling support (editor.ts:333).
    pub(crate) scroll_offset: usize,

    // Autocomplete support (editor.ts:336-349).
    pub(crate) autocomplete_provider: Option<Rc<dyn AutocompleteProvider>>,
    pub(crate) autocomplete_trigger_characters: Vec<char>,
    pub(crate) autocomplete_list: Option<SelectList>,
    pub(crate) autocomplete_state: Option<AutocompleteMode>,
    pub(crate) autocomplete_prefix: String,
    pub(crate) autocomplete_max_visible: usize,
    /// TS `autocompleteRequestTask` promise chain — modeled as an explicit
    /// pending-request queue owned by [T4-D1b].
    pub(crate) autocomplete_request_pending: Vec<PendingAutocompleteRequest>,
    /// TS `autocompleteDebounceTimer` (editor.ts:347).
    pub(crate) autocomplete_debounce_timer: Option<PendingAutocompleteRequest>,
    pub(crate) autocomplete_start_token: u64,
    pub(crate) autocomplete_request_id: u64,

    // Paste tracking for large pastes (editor.ts:352-354).
    /// TS `Map<number, string>`; the only ordering-sensitive consumer
    /// (editor.ts:1382-1389) sorts by ascending id, which `BTreeMap` gives.
    pub(crate) pastes: BTreeMap<u32, String>,
    pub(crate) paste_counter: u32,

    // Bracketed paste mode buffering (editor.ts:357-359).
    pub(crate) paste_buffer: String,
    pub(crate) is_in_paste: bool,

    // Prompt history for up/down navigation (editor.ts:362-365).
    pub(crate) history: Vec<String>,
    /// -1 = not browsing, 0 = most recent, 1 = older, … (TS uses unshift).
    pub(crate) history_index: i64,
    pub(crate) history_draft: Option<EditorState>,

    // Kill ring for Emacs-style kill/yank (editor.ts:368-370).
    pub(crate) kill_ring: KillRing,
    pub(crate) last_action: Option<EditorAction>,

    // Character jump mode (editor.ts:373).
    pub(crate) jump_mode: Option<JumpDirection>,

    // Preferred visual column for vertical movement (editor.ts:376).
    pub(crate) preferred_visual_col: Option<i64>,

    /// Pre-snap `cursorCol` when the cursor was snapped to the start of an
    /// atomic segment (editor.ts:378-384).
    pub(crate) snapped_from_cursor_col: Option<usize>,

    // Undo support (editor.ts:387).
    pub(crate) undo_stack: UndoStack<EditorSnapshot>,

    /// TS public optional callbacks / fields (editor.ts:390-392).
    pub on_submit: Option<EditorCallback>,
    pub on_change: Option<EditorCallback>,
    pub disable_submit: bool,
}

impl Editor {
    /// `constructor(tui, theme, options = {})` (editor.ts:374-383).
    pub fn new(tui: Rc<RefCell<dyn TuiHost>>, theme: EditorTheme) -> Self {
        Self::with_options(tui, theme, EditorOptions::default())
    }

    pub fn with_options(
        tui: Rc<RefCell<dyn TuiHost>>,
        theme: EditorTheme,
        options: EditorOptions,
    ) -> Self {
        let EditorTheme { border_color, select_list } = theme;
        // `Number.isFinite(paddingX) ? Math.max(0, Math.floor(paddingX)) : 0`
        let padding_x = match options.padding_x {
            Some(v) if v.is_finite() => v.max(0.0).floor() as usize,
            _ => 0,
        };
        // `Number.isFinite(maxVisible) ? Math.max(3, Math.min(20, Math.floor(maxVisible))) : 5`
        let autocomplete_max_visible = match options.autocomplete_max_visible {
            Some(v) if v.is_finite() => (v.floor().clamp(3.0, 20.0)) as usize,
            _ => 5,
        };
        Editor {
            state: EditorState {
                lines: vec![String::new()],
                cursor_line: 0,
                cursor_col: 0,
            },
            focused: false,
            tui,
            select_list_theme: select_list,
            border_color,
            padding_x,
            last_width: 80,
            rendered_visible_line_count: 1,
            rendered_autocomplete_height: 0,
            scroll_offset: 0,
            autocomplete_provider: None,
            autocomplete_trigger_characters: DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS.to_vec(),
            autocomplete_list: None,
            autocomplete_state: None,
            autocomplete_prefix: String::new(),
            autocomplete_max_visible,
            autocomplete_request_pending: Vec::new(),
            autocomplete_debounce_timer: None,
            autocomplete_start_token: 0,
            autocomplete_request_id: 0,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            paste_buffer: String::new(),
            is_in_paste: false,
            history: Vec::new(),
            history_index: -1,
            history_draft: None,
            kill_ring: KillRing::new(),
            last_action: None,
            jump_mode: None,
            preferred_visual_col: None,
            snapped_from_cursor_col: None,
            undo_stack: UndoStack::new(),
            on_submit: None,
            on_change: None,
            disable_submit: false,
        }
    }

    // -------------------------------------------------------------------------
    // Segment helpers (editor.ts:385-392)
    // -------------------------------------------------------------------------

    /// `validPasteIds()` (editor.ts:385-388).
    pub(crate) fn valid_paste_ids(&self) -> HashSet<u32> {
        self.pastes.keys().copied().collect()
    }

    /// `segment(text, "grapheme")` (editor.ts:390-392).
    pub(crate) fn segment_graphemes(&self, text: &str) -> Vec<GraphemeSegment> {
        segment_graphemes_with_markers(text, &self.valid_paste_ids())
    }

    /// `segment(text, "word")` (editor.ts:390-392).
    pub(crate) fn segment_words(&self, text: &str) -> Vec<WordSegment> {
        segment_words_with_markers(text, &self.valid_paste_ids())
    }

    // -------------------------------------------------------------------------
    // Trivial accessors (editor.ts:394-416, 1086-1113)
    // -------------------------------------------------------------------------

    /// `getPaddingX()` (editor.ts:394-396).
    pub fn get_padding_x(&self) -> usize {
        self.padding_x
    }

    /// `setPaddingX(padding)` (editor.ts:398-404).
    pub fn set_padding_x(&mut self, padding: f64) {
        let new_padding = if padding.is_finite() { padding.max(0.0).floor() as usize } else { 0 };
        if self.padding_x != new_padding {
            self.padding_x = new_padding;
            self.tui.borrow_mut().request_render();
        }
    }

    /// `getAutocompleteMaxVisible()` (editor.ts:406-408).
    pub fn get_autocomplete_max_visible(&self) -> usize {
        self.autocomplete_max_visible
    }

    /// `setAutocompleteMaxVisible(maxVisible)` (editor.ts:410-416).
    pub fn set_autocomplete_max_visible(&mut self, max_visible: f64) {
        let new_max_visible = if max_visible.is_finite() {
            (max_visible.floor().clamp(3.0, 20.0)) as usize
        } else {
            5
        };
        if self.autocomplete_max_visible != new_max_visible {
            self.autocomplete_max_visible = new_max_visible;
            self.tui.borrow_mut().request_render();
        }
    }

    /// `getText()` (editor.ts:1086-1088).
    pub fn get_text(&self) -> String {
        self.state.lines.join("\n")
    }

    /// `getLines()` (editor.ts:1107-1109) — defensive copy.
    pub fn get_lines(&self) -> Vec<String> {
        self.state.lines.clone()
    }

    /// `getCursor()` (editor.ts:1111-1113). `col` is UTF-16 units.
    pub fn get_cursor(&self) -> (usize, usize) {
        (self.state.cursor_line, self.state.cursor_col)
    }

    /// `isShowingAutocomplete()` (editor.ts:2465-2467).
    pub fn is_showing_autocomplete(&self) -> bool {
        self.autocomplete_state.is_some()
    }

    // -------------------------------------------------------------------------
    // Pure state predicates (editor.ts:440-454)
    // -------------------------------------------------------------------------

    /// `isEditorEmpty()` (editor.ts:440-442).
    pub(crate) fn is_editor_empty(&self) -> bool {
        self.state.lines.len() == 1 && self.state.lines[0].is_empty()
    }

    /// `isOnFirstVisualLine()` (editor.ts:444-449).
    pub(crate) fn is_on_first_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) == 0
    }

    /// `isOnLastVisualLine()` (editor.ts:450-454).
    pub(crate) fn is_on_last_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) + 1 == visual_lines.len()
    }

    // -------------------------------------------------------------------------
    // Borders (editor.ts:511-519)
    // -------------------------------------------------------------------------

    /// `renderTopBorder(width, hiddenLineCount)` (editor.ts:511-514).
    pub(crate) fn render_top_border(&self, width: usize, hidden_line_count: usize) -> String {
        let border = if hidden_line_count > 0 {
            create_scroll_border('↑', hidden_line_count, width)
        } else {
            "─".repeat(width)
        };
        (self.border_color)(&border)
    }

    /// `renderBottomBorder(width, hiddenLineCount)` (editor.ts:516-519).
    pub(crate) fn render_bottom_border(&self, width: usize, hidden_line_count: usize) -> String {
        let border = if hidden_line_count > 0 {
            create_scroll_border('↓', hidden_line_count, width)
        } else {
            "─".repeat(width)
        };
        (self.border_color)(&border)
    }

    // -------------------------------------------------------------------------
    // Synchronous autocomplete surface (editor.ts:418-427, 2262-2341,
    // 2354-2365, 2444-2473) — the guard chain every text mutation runs
    // through. The async pipeline (start/run/apply) is [T4-D1b].
    // -------------------------------------------------------------------------

    /// `setAutocompleteProvider(provider)` (editor.ts:418-423).
    pub fn set_autocomplete_provider(&mut self, provider: Rc<dyn AutocompleteProvider>) {
        self.cancel_autocomplete();
        let trigger_characters = provider.trigger_characters();
        self.autocomplete_provider = Some(provider);
        self.set_autocomplete_trigger_characters(trigger_characters);
    }

    /// `setAutocompleteTriggerCharacters(triggerCharacters)`
    /// (editor.ts:2341-2352): defaults + filtered custom characters.
    pub(crate) fn set_autocomplete_trigger_characters(&mut self, trigger_characters: Vec<char>) {
        let mut next: Vec<char> = DEFAULT_AUTOCOMPLETE_TRIGGER_CHARACTERS.to_vec();
        for character in trigger_characters {
            // TS skips `character.length !== 1` (astral chars are 2 UTF-16
            // units), `"/"`, and whitespace characters.
            if character.len_utf16() != 1 || character == '/' || is_js_whitespace(character) {
                continue;
            }
            if next.contains(&character) {
                continue;
            }
            next.push(character);
        }
        self.autocomplete_trigger_characters = next;
    }

    /// `tryTriggerAutocomplete(explicitTab = false)` (editor.ts:2262-2264).
    pub(crate) fn try_trigger_autocomplete(&mut self, explicit_tab: bool) {
        self.request_autocomplete(false, explicit_tab);
    }

    /// `handleTabCompletion()` (editor.ts:2266-2277).
    pub(crate) fn handle_tab_completion(&mut self) {
        if self.autocomplete_provider.is_none() {
            return;
        }
        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
        let before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
        // TS: `!beforeCursor.trimStart().includes(" ")` — JS trimStart
        // semantics (leading whitespace only).
        let trimmed_start: String =
            before_cursor.trim_start_matches(is_js_whitespace).to_string();
        if self.is_in_slash_command_context(&before_cursor) && !trimmed_start.contains(' ') {
            self.handle_slash_command_completion();
        } else {
            self.force_file_autocomplete(true);
        }
    }

    /// `handleSlashCommandCompletion()` (editor.ts:2279-2281).
    pub(crate) fn handle_slash_command_completion(&mut self) {
        self.request_autocomplete(false, true);
    }

    /// `forceFileAutocomplete(explicitTab = false)` (editor.ts:2283-2285).
    pub(crate) fn force_file_autocomplete(&mut self, explicit_tab: bool) {
        self.request_autocomplete(true, explicit_tab);
    }

    /// `requestAutocomplete({force, explicitTab})` (editor.ts:2287-2322) —
    /// the synchronous scheduling half. The `setTimeout` callback /
    /// `startAutocompleteRequest` promise handoff is modeled by the pending
    /// request records consumed by [T4-D1b].
    pub(crate) fn request_autocomplete(&mut self, force: bool, explicit_tab: bool) {
        let Some(provider) = self.autocomplete_provider.clone() else {
            return;
        };

        if force {
            let should_trigger = provider.should_trigger_file_completion(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
            );
            if !should_trigger {
                return;
            }
        }

        self.cancel_autocomplete_request();
        self.autocomplete_start_token += 1;
        let start_token = self.autocomplete_start_token;

        let debounce_ms = self.get_autocomplete_debounce_ms(force, explicit_tab);
        if debounce_ms > 0 {
            self.autocomplete_debounce_timer = Some(PendingAutocompleteRequest {
                start_token,
                force,
                explicit_tab,
            });
            return;
        }

        self.autocomplete_request_pending.push(PendingAutocompleteRequest {
            start_token,
            force,
            explicit_tab,
        });
    }

    /// `getAutocompleteDebounceMs(options)` (editor.ts:2354-2365).
    pub(crate) fn get_autocomplete_debounce_ms(&self, force: bool, explicit_tab: bool) -> u64 {
        if explicit_tab || force {
            return 0;
        }
        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
        let text_before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
        let chars: Vec<char> = self
            .autocomplete_trigger_characters
            .iter()
            .copied()
            .filter(|&c| c != '@')
            .collect();
        if autocomplete_token_pattern_test(&text_before_cursor, &chars, true) {
            ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS
        } else {
            0
        }
    }

    /// `cancelAutocompleteRequest()` (editor.ts:2444-2452): bump the
    /// token, clear the armed debounce timer and the queued requests,
    /// then ABORT the in-flight continuation (`this.autocompleteAbort?
    /// .abort()` + `= undefined`). Aborting at cancel time (not lazily at
    /// the next request start) is observable: the provider's captured
    /// `AbortSignal` reads aborted immediately, and a later flush can
    /// never resurrect a cancelled request (redteam R3 finding #1/#2).
    pub(crate) fn cancel_autocomplete_request(&mut self) {
        self.autocomplete_start_token += 1;
        self.autocomplete_debounce_timer = None;
        self.autocomplete_request_pending.clear();
        self.abort_in_flight_autocomplete();
    }

    /// `clearAutocompleteUi()` (editor.ts:2454-2458).
    pub(crate) fn clear_autocomplete_ui(&mut self) {
        self.autocomplete_state = None;
        self.autocomplete_list = None;
        self.autocomplete_prefix = String::new();
    }

    /// `cancelAutocomplete()` (editor.ts:2460-2462).
    pub(crate) fn cancel_autocomplete(&mut self) {
        self.cancel_autocomplete_request();
        self.clear_autocomplete_ui();
    }

    /// `updateAutocomplete()` (editor.ts:2469-2473).
    pub(crate) fn update_autocomplete(&mut self) {
        if self.autocomplete_state.is_none() || self.autocomplete_provider.is_none() {
            return;
        }
        let force = self.autocomplete_state == Some(AutocompleteMode::Force);
        self.request_autocomplete(force, false);
    }
}

// =============================================================================
// Component / Focusable (tui.ts) — thin delegations to sibling files
// =============================================================================

impl Component for Editor {
    /// `render(width)` (editor.ts:521-633) — see `editor_render.rs`.
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_editor(width)
    }

    /// TS `"focused" in component` — this component implements `Focusable`
    /// (editor.ts:305).
    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    /// `handleInput(data)` (editor.ts:696-998) — see `editor_input.rs`.
    fn handle_input(&mut self, data: &str) {
        self.handle_input_data(data);
    }

    /// `Editor` defines `handleInput` (editor.ts:696).
    fn handles_input(&self) -> bool {
        true
    }

    /// `handleMouse(event)` (editor.ts:633-694) — see `editor_render.rs`.
    fn handle_mouse(
        &mut self,
        _self_ref: &Rc<RefCell<dyn Component>>,
        event: &TuiMouseEvent,
    ) -> Option<ComponentMouseResult> {
        self.handle_mouse_impl(event)
    }

    /// `invalidate()` (editor.ts:507-509): no cached state to invalidate.
    fn invalidate(&mut self) {}
}

impl Focusable for Editor {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}
