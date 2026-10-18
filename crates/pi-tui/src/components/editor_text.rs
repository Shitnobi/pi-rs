//! Text mutation: `setText`/`insertTextAtCursor`/`insertCharacter`/
//! `handlePaste`/newline/backspace/word+line deletes/submit, plus the
//! slash-command context predicates (editor.ts:1088-1467, 1625-1834,
//! 2190-2216).
//!
//! **Ported by T4 delegate E2** (stub signatures were binding; bodies ported
//! 1:1 from editor.ts with TS line references).

use super::editor::{
    autocomplete_token_pattern_test, slice_utf16, utf16_to_byte, Editor, EditorAction, EditorState,
};
use crate::keybindings::get_keybindings;
use crate::keys::matches_key;
use crate::unicode::is_cjk_script;
use crate::utils::{is_js_whitespace, is_whitespace_char, js_trim, utf16_len};

// =============================================================================
// JS/paste-marker helpers (private to this file)
// =============================================================================

/// JS `/\w/.test(char)` for a single char (`[A-Za-z0-9_]`).
fn is_js_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JS `String.prototype.trimStart()`.
fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// The char at UTF-16 code-unit index `idx` (JS `line[idx]`).
fn char_at_utf16(line: &str, idx: usize) -> Option<char> {
    let byte = utf16_to_byte(line, idx);
    line[byte..].chars().next()
}

/// One `PASTE_MARKER_REGEX` (editor.ts:32) match: byte span plus the two
/// capture groups.
struct PasteMarkerMatch {
    start: usize,
    end: usize,
    /// Group 1: `\d+` after `[paste #`.
    id: u64,
    /// Group 2: `( (\+\d+ lines|\d+ chars))?` — includes the leading space
    /// and the unit word (or empty for the bare form).
    suffix: String,
}

/// Try to parse a `PASTE_MARKER_REGEX` match starting at the beginning of
/// `s`. Returns `(match byte length, id, suffix)`.
///
/// Shape: `\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]`
fn match_paste_marker_at(s: &str) -> Option<(usize, u64, String)> {
    let rest = s.strip_prefix("[paste #")?;
    let digits_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    let (digits, after) = rest.split_at(digits_len);
    let id: u64 = digits.parse().ok()?;

    // Optional group 2: ` (+\d+ lines|\d+ chars)`
    if let Some(spaced) = after.strip_prefix(' ') {
        if let Some(tail) = spaced.strip_prefix('+') {
            let dl = tail.chars().take_while(|c| c.is_ascii_digit()).count();
            if dl > 0 && tail[dl..].starts_with(" lines]") {
                let suffix = format!(" +{} lines", &tail[..dl]);
                return Some(("[paste #".len() + digits_len + suffix.len() + 1, id, suffix));
            }
            return None;
        }
        let dl = spaced.chars().take_while(|c| c.is_ascii_digit()).count();
        if dl > 0 && spaced[dl..].starts_with(" chars]") {
            let suffix = format!(" {} chars", &spaced[..dl]);
            return Some(("[paste #".len() + digits_len + suffix.len() + 1, id, suffix));
        }
        return None;
    }

    if after.starts_with(']') {
        return Some(("[paste #".len() + digits_len + 1, id, String::new()));
    }
    None
}

/// `text.matchAll(PASTE_MARKER_REGEX)` — non-overlapping matches,
/// left-to-right (editor.ts:32 with the `g` flag).
fn find_paste_marker_matches(text: &str) -> Vec<PasteMarkerMatch> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some(&(pos, ch)) = it.peek() {
        if ch == '[' {
            if let Some((len, id, suffix)) = match_paste_marker_at(&text[pos..]) {
                out.push(PasteMarkerMatch {
                    start: pos,
                    end: pos + len,
                    id,
                    suffix,
                });
                // All matched bytes are ASCII.
                for _ in 0..len {
                    it.next();
                }
                continue;
            }
        }
        it.next();
    }
    out
}

/// `line.replace(PASTE_MARKER_REGEX, replacer)` from `handleBackspace`
/// (editor.ts:1414-1420): decrement every marker id greater than
/// `target_id`, preserving the suffix group.
fn renumber_paste_markers_in(line: &str, target_id: u64) -> String {
    let matches = find_paste_marker_matches(line);
    if matches.is_empty() {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut last = 0usize;
    for m in matches {
        out.push_str(&line[last..m.start]);
        if m.id > target_id {
            out.push_str(&format!("[paste #{}{}]", m.id - 1, m.suffix));
        } else {
            out.push_str(&line[m.start..m.end]);
        }
        last = m.end;
    }
    out.push_str(&line[last..]);
    out
}

/// The CSI-u decode inside `handlePaste` (editor.ts:1276-1286):
/// `text.replace(/\x1b\[(\d+);5u/g, (match, code) => …)` — tmux popups
/// re-encode control bytes inside bracketed paste as `ESC [ <cp> ; 5 u`;
/// decode Ctrl+letter codepoints back to their literal byte.
fn decode_csi_u_in_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            let digits_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start && bytes[j..].starts_with(b";5u") {
                let cp: u64 = text[digits_start..j].parse().unwrap_or(u64::MAX);
                if (97..=122).contains(&cp) {
                    out.push(char::from_u32((cp - 96) as u32).unwrap());
                } else if (65..=90).contains(&cp) {
                    out.push(char::from_u32((cp - 64) as u32).unwrap());
                } else {
                    out.push_str(&text[i..j + 3]);
                }
                i = j + 3;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

impl Editor {
    /// `isAtStartOfMessage()` (editor.ts:2195-2200). `isSlashMenuAllowed()`
    /// (editor.ts:2190-2193) is `cursorLine === 0`, inlined here.
    pub(crate) fn is_at_start_of_message(&self) -> bool {
        if self.state.cursor_line != 0 {
            return false;
        }
        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
        let before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
        let trimmed = js_trim(&before_cursor);
        trimmed.is_empty() || trimmed == "/"
    }

    /// `isInSlashCommandContext(textBeforeCursor)` (editor.ts:2202-2216).
    pub(crate) fn is_in_slash_command_context(&self, text_before_cursor: &str) -> bool {
        self.state.cursor_line == 0 && js_trim_start(text_before_cursor).starts_with('/')
    }

    /// `setText(text)` (editor.ts:1115-1132).
    pub fn set_text(&mut self, text: &str) {
        self.cancel_autocomplete();
        self.last_action = None;
        self.exit_history_browsing();
        let normalized = normalize_text(Some(text));
        // Push undo snapshot if content differs (makes programmatic changes undoable)
        if self.get_text() != normalized {
            self.push_undo_snapshot();
        }
        self.pastes.clear();
        self.paste_counter = 0;
        self.set_text_internal(&normalized, false);
    }

    /// `setTextInternal(text, cursorPlacement = "end")` (editor.ts:494-505).
    /// `cursor_placement_start` = TS `"start"`.
    pub(crate) fn set_text_internal(&mut self, text: &str, cursor_placement_start: bool) {
        let lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
        self.state.lines = if lines.is_empty() { vec![String::new()] } else { lines };
        self.state.cursor_line = if cursor_placement_start {
            0
        } else {
            self.state.lines.len() - 1
        };
        let col = if cursor_placement_start {
            0
        } else {
            utf16_len(self.state.lines.get(self.state.cursor_line).map(String::as_str).unwrap_or(""))
        };
        self.set_cursor_col(col);
        // Reset scroll - render() will adjust to show cursor
        self.scroll_offset = 0;

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `expandPasteMarkers(text)` (editor.ts:1090-1101).
    pub(crate) fn expand_paste_markers(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (&paste_id, paste_content) in &self.pastes {
            // `new RegExp(`\\[paste #${pasteId}( (\\+\\d+ lines|\\d+ chars))?\\]`, "g")`
            // replaced with a function → the paste content is inserted
            // literally (no `$`-pattern handling).
            let matches = find_paste_marker_matches(&result);
            if matches.is_empty() {
                continue;
            }
            let mut next = String::with_capacity(result.len());
            let mut last = 0usize;
            for m in matches {
                next.push_str(&result[last..m.start]);
                if m.id == paste_id as u64 {
                    next.push_str(paste_content);
                } else {
                    next.push_str(&result[m.start..m.end]);
                }
                last = m.end;
            }
            next.push_str(&result[last..]);
            result = next;
        }
        result
    }

    /// `getExpandedText()` (editor.ts:1103-1105).
    pub fn get_expanded_text(&self) -> String {
        self.expand_paste_markers(&self.state.lines.join("\n"))
    }

    /// `insertTextAtCursor(text)` (editor.ts:1134-1146).
    pub fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.cancel_autocomplete();
        self.push_undo_snapshot();
        self.last_action = None;
        self.exit_history_browsing();
        self.insert_text_at_cursor_internal(text);
    }

    /// `insertTextAtCursorInternal(text)` (editor.ts:1157-1199).
    pub(crate) fn insert_text_at_cursor_internal(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        // Normalize line endings and tabs
        let normalized = normalize_text(Some(text));
        let inserted_lines: Vec<&str> = normalized.split('\n').collect();

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
        let before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
        let after_cursor = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));

        if inserted_lines.len() == 1 {
            // Single line - insert at cursor position
            self.state.lines[self.state.cursor_line] =
                format!("{before_cursor}{normalized}{after_cursor}");
            self.set_cursor_col(self.state.cursor_col + utf16_len(&normalized));
        } else {
            // Multi-line insertion
            let mut new_lines: Vec<String> =
                self.state.lines[..self.state.cursor_line].to_vec();
            // The first inserted line merged with text before cursor
            new_lines.push(format!("{before_cursor}{}", inserted_lines[0]));
            // All middle inserted lines
            for mid in &inserted_lines[1..inserted_lines.len() - 1] {
                new_lines.push((*mid).to_string());
            }
            // The last inserted line with text after cursor
            let last_inserted = inserted_lines[inserted_lines.len() - 1];
            new_lines.push(format!("{last_inserted}{after_cursor}"));
            // All lines after current line
            new_lines.extend(self.state.lines[self.state.cursor_line + 1..].iter().cloned());

            self.state.lines = new_lines;
            self.state.cursor_line += inserted_lines.len() - 1;
            self.set_cursor_col(utf16_len(last_inserted));
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `insertCharacter(char, skipUndoCoalescing?)` (editor.ts:1201-1258).
    /// `char` is a full grapheme string (from `decodePrintableKey`).
    #[allow(clippy::if_same_then_else)] // TS if/else-if ladder with identical bodies (editor.ts:1247-1253)
    pub(crate) fn insert_character(&mut self, char_str: &str, skip_undo_coalescing: bool) {
        self.exit_history_browsing();

        // Undo coalescing (fish-style):
        // - Consecutive word chars coalesce into one undo unit
        // - Space captures state before itself (so undo removes space+following word together)
        // - Each space is separately undoable
        // Skip coalescing when called from atomic operations (e.g., handlePaste)
        if !skip_undo_coalescing {
            if char_str.chars().any(is_whitespace_char) || self.last_action != Some(EditorAction::TypeWord) {
                self.push_undo_snapshot();
            }
            self.last_action = Some(EditorAction::TypeWord);
        }

        let line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        let before = slice_utf16(&line, 0, self.state.cursor_col);
        let after = slice_utf16(&line, self.state.cursor_col, utf16_len(&line));

        self.state.lines[self.state.cursor_line] = format!("{before}{char_str}{after}");
        self.set_cursor_col(self.state.cursor_col + utf16_len(char_str));

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }

        // Check if we should trigger or update autocomplete
        if self.autocomplete_state.is_none() {
            // Auto-trigger for "/" at the start of a line (slash commands)
            if char_str == "/" && self.is_at_start_of_message() {
                self.try_trigger_autocomplete(false);
            }
            // Auto-trigger for symbol-based completion like @, #, or provider triggers at token boundaries
            else if self
                .autocomplete_trigger_characters
                .iter()
                .any(|c| char_str == c.to_string())
            {
                let current_line =
                    self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
                let text_before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
                if autocomplete_token_pattern_test(
                    &text_before_cursor,
                    &self.autocomplete_trigger_characters,
                    false,
                ) {
                    self.try_trigger_autocomplete(false);
                }
            }
            // Also auto-trigger when typing letters in a slash command or symbol completion context
            else if char_str.chars().any(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
            }) || char_str.chars().any(|c| is_cjk_script(c as u32))
            {
                let current_line =
                    self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
                let text_before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
                // Check if we're in a slash command (with or without space for arguments)
                if self.is_in_slash_command_context(&text_before_cursor) {
                    self.try_trigger_autocomplete(false);
                }
                // Check if we're in a symbol-based completion context like @, #, or provider triggers
                else if autocomplete_token_pattern_test(
                    &text_before_cursor,
                    &self.autocomplete_trigger_characters,
                    false,
                ) {
                    self.try_trigger_autocomplete(false);
                }
            }
        } else {
            self.update_autocomplete();
        }
    }

    /// `handlePaste(pastedText)` (editor.ts:1260-1326).
    pub(crate) fn handle_paste(&mut self, pasted_text: &str) {
        self.cancel_autocomplete();
        self.exit_history_browsing();
        self.last_action = None;

        self.push_undo_snapshot();

        // Some terminals (e.g. tmux popups with extended-keys-format=csi-u) re-encode
        // control bytes inside bracketed paste as CSI-u Ctrl+<letter> sequences
        // (ESC [ <codepoint> ; 5 u). Decode those back to their literal byte so the
        // per-char filter below preserves newlines instead of stripping ESC and
        // leaking the printable tail (e.g. "[106;5u") into the editor.
        let decoded_text = decode_csi_u_in_paste(pasted_text);

        // Clean the pasted text: normalize line endings, expand tabs
        let clean_text = normalize_text(Some(&decoded_text));

        // Filter out non-printable characters except newlines
        let mut filtered_text = String::with_capacity(clean_text.len());
        for c in clean_text.chars() {
            if c == '\n' || (c as u32) >= 32 {
                filtered_text.push(c);
            }
        }

        // If pasting a file path (starts with /, ~, or .) and the character before
        // the cursor is a word character, prepend a space for better readability
        if matches!(filtered_text.chars().next(), Some('/') | Some('~') | Some('.')) {
            let current_line =
                self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
            let char_before_cursor = if self.state.cursor_col > 0 {
                char_at_utf16(&current_line, self.state.cursor_col - 1)
            } else {
                None
            };
            if char_before_cursor.is_some_and(is_js_word_char) {
                filtered_text = format!(" {filtered_text}");
            }
        }

        // Split into lines to check for large paste
        let pasted_line_count = filtered_text.matches('\n').count() + 1;

        // Check if this is a large paste (> 10 lines or > 1000 characters)
        let total_chars = utf16_len(&filtered_text);
        if pasted_line_count > 10 || total_chars > 1000 {
            // Store the paste and insert a marker
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, filtered_text.clone());

            // Insert marker like "[paste #1 +123 lines]" or "[paste #1 1234 chars]"
            let marker = if pasted_line_count > 10 {
                format!("[paste #{paste_id} +{pasted_line_count} lines]")
            } else {
                format!("[paste #{paste_id} {total_chars} chars]")
            };
            self.insert_text_at_cursor_internal(&marker);
            return;
        }

        // Single-line and multi-line pastes both insert atomically (do not
        // trigger autocomplete during paste).
        self.insert_text_at_cursor_internal(&filtered_text);
    }

    /// `addNewLine()` (editor.ts:1328-1351).
    pub(crate) fn add_new_line(&mut self) {
        self.cancel_autocomplete();
        self.exit_history_browsing();
        self.last_action = None;

        self.push_undo_snapshot();

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        let before = slice_utf16(&current_line, 0, self.state.cursor_col);
        let after = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));

        // Split current line
        self.state.lines[self.state.cursor_line] = before;
        self.state.lines.insert(self.state.cursor_line + 1, after);

        // Move cursor to start of new line
        self.state.cursor_line += 1;
        self.set_cursor_col(0);

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `shouldSubmitOnBackslashEnter(data, kb)` (editor.ts:1353-1362). The
    /// Rust port reads the global keybindings manager instead of taking
    /// `kb` as a parameter.
    pub(crate) fn should_submit_on_backslash_enter(&self, data: &str) -> bool {
        if self.disable_submit {
            return false;
        }
        if !matches_key(data, "enter") {
            return false;
        }
        let submit_keys = get_keybindings().get_keys("tui.input.submit");
        let has_shift_enter =
            submit_keys.iter().any(|k| k == "shift+enter" || k == "shift+return");
        if !has_shift_enter {
            return false;
        }

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
        self.state.cursor_col > 0 && char_at_utf16(&current_line, self.state.cursor_col - 1) == Some('\\')
    }

    /// `submitValue()` (editor.ts:1364-1377).
    pub(crate) fn submit_value(&mut self) {
        self.cancel_autocomplete();
        let result = js_trim(&self.expand_paste_markers(&self.state.lines.join("\n"))).to_string();

        self.state = EditorState { lines: vec![String::new()], cursor_line: 0, cursor_col: 0 };
        self.pastes.clear();
        self.paste_counter = 0;
        self.exit_history_browsing();
        self.scroll_offset = 0;
        self.undo_stack.clear();
        self.last_action = None;

        if let Some(cb) = self.on_change.as_mut() {
            cb("");
        }
        if let Some(cb) = self.on_submit.as_mut() {
            cb(&result);
        }
    }

    /// `handleBackspace()` (editor.ts:1380-1467).
    #[allow(clippy::if_same_then_else)] // TS if/else-if ladder with identical bodies (editor.ts:1455-1461)
    pub(crate) fn handle_backspace(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();

            // Delete grapheme before cursor (handles emojis, combining characters, etc.)
            let line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
            let before_cursor = slice_utf16(&line, 0, self.state.cursor_col);

            // Find the last grapheme in the text before cursor
            let graphemes = self.segment_graphemes(&before_cursor);
            let last_grapheme = graphemes.last();
            let grapheme_length = last_grapheme.map(|g| utf16_len(&g.segment)).unwrap_or(1);
            // `PASTE_MARKER_SINGLE.exec(lastGrapheme.segment)` (editor.ts:1395)
            // — the BOTH-ends-anchored regex. Segments reaching here are
            // either single grapheme clusters (which can never contain a
            // multi-char ASCII marker prefix, so no match either way) or
            // atomic-merged markers (exact marker text, so prefix == full
            // match); the exact-length filter is therefore equivalent on
            // all reachable inputs and keeps this call site faithful to
            // the anchored regex even if segment shapes ever change
            // (redteam R1 open question, resolved).
            let is_pasted_segmented = last_grapheme.and_then(|g| {
                match_paste_marker_at(&g.segment)
                    .filter(|(len, _, _)| *len == g.segment.len())
            });

            if let Some((_, target_id, _)) = is_pasted_segmented {
                // This contains the id part e.g 4 from [paste #4 +123 lines]
                self.pastes.remove(&(target_id as u32));
                self.paste_counter = self.paste_counter.saturating_sub(1);

                // Shift registry entries down in ascending id order, independent
                // of marker order in the text ([paste #3] becomes [paste #2] when
                // [paste #1] is removed).
                let higher_ids: Vec<u32> = self
                    .pastes
                    .keys()
                    .copied()
                    .filter(|&id| (id as u64) > target_id)
                    .collect();
                for id in higher_ids {
                    let content = self.pastes.remove(&id).unwrap();
                    self.pastes.insert(id - 1, content);
                }

                // Renumber markers with ids greater than the removed one.
                self.state.lines = self
                    .state
                    .lines
                    .iter()
                    .map(|l| renumber_paste_markers_in(l, target_id))
                    .collect();
            }

            let line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

            let before = slice_utf16(&line, 0, self.state.cursor_col - grapheme_length);
            let after = slice_utf16(&line, self.state.cursor_col, utf16_len(&line));

            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
            self.set_cursor_col(self.state.cursor_col - grapheme_length);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();

            // Merge with previous line
            let current_line = self.state.lines[self.state.cursor_line].clone();
            let previous_line = self.state.lines[self.state.cursor_line - 1].clone();

            self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
            self.state.lines.remove(self.state.cursor_line);

            self.state.cursor_line -= 1;
            self.set_cursor_col(utf16_len(&previous_line));
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }

        // Update or re-trigger autocomplete after backspace
        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        } else {
            // If autocomplete was cancelled (no matches), re-trigger if we're in a completable context
            let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
            let text_before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
            // Slash command context
            if self.is_in_slash_command_context(&text_before_cursor) {
                self.try_trigger_autocomplete(false);
            }
            // Symbol-based completion context like @, #, or provider triggers
            else if autocomplete_token_pattern_test(
                &text_before_cursor,
                &self.autocomplete_trigger_characters,
                false,
            ) {
                self.try_trigger_autocomplete(false);
            }
        }
    }

    /// `deleteToStartOfLine()` (editor.ts:1625-1658) — Ctrl+U.
    pub(crate) fn delete_to_start_of_line(&mut self) {
        self.exit_history_browsing();

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();

            // Calculate text to be deleted and save to kill ring (backward deletion = prepend)
            let deleted_text = slice_utf16(&current_line, 0, self.state.cursor_col);
            self.kill_ring.push(
                &deleted_text,
                true,
                self.last_action == Some(EditorAction::Kill),
            );
            self.last_action = Some(EditorAction::Kill);

            // Delete from start of line up to cursor
            self.state.lines[self.state.cursor_line] =
                slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));
            self.set_cursor_col(0);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();

            // At start of line - merge with previous line, treating newline as deleted text
            self.kill_ring.push("\n", true, self.last_action == Some(EditorAction::Kill));
            self.last_action = Some(EditorAction::Kill);

            let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
            self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
            self.state.lines.remove(self.state.cursor_line);
            self.state.cursor_line -= 1;
            self.set_cursor_col(utf16_len(&previous_line));
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `deleteToEndOfLine()` (editor.ts:1660-1690) — Ctrl+K.
    pub(crate) fn delete_to_end_of_line(&mut self) {
        self.exit_history_browsing();

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        if self.state.cursor_col < utf16_len(&current_line) {
            self.push_undo_snapshot();

            // Calculate text to be deleted and save to kill ring (forward deletion = append)
            let deleted_text = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));
            self.kill_ring.push(
                &deleted_text,
                false,
                self.last_action == Some(EditorAction::Kill),
            );
            self.last_action = Some(EditorAction::Kill);

            // Delete from cursor to end of line
            self.state.lines[self.state.cursor_line] =
                slice_utf16(&current_line, 0, self.state.cursor_col);
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();

            // At end of line - merge with next line, treating newline as deleted text
            self.kill_ring.push("\n", false, self.last_action == Some(EditorAction::Kill));
            self.last_action = Some(EditorAction::Kill);

            let next_line = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `deleteWordBackwards()` (editor.ts:1692-1735) — Ctrl+W / Alt+Backspace.
    pub(crate) fn delete_word_backwards(&mut self) {
        self.exit_history_browsing();

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        // If at start of line, behave like backspace at column 0 (merge with previous line)
        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.push_undo_snapshot();

                // Treat newline as deleted text (backward deletion = prepend)
                self.kill_ring.push("\n", true, self.last_action == Some(EditorAction::Kill));
                self.last_action = Some(EditorAction::Kill);

                let previous_line = self.state.lines[self.state.cursor_line - 1].clone();
                self.state.lines[self.state.cursor_line - 1] = format!("{previous_line}{current_line}");
                self.state.lines.remove(self.state.cursor_line);
                self.state.cursor_line -= 1;
                self.set_cursor_col(utf16_len(&previous_line));
            }
        } else {
            self.push_undo_snapshot();

            // Save lastAction before cursor movement (moveWordBackwards resets it)
            let was_kill = self.last_action == Some(EditorAction::Kill);

            let old_cursor_col = self.state.cursor_col;
            self.move_word_backwards();
            let delete_from = self.state.cursor_col;
            self.set_cursor_col(old_cursor_col);

            let deleted_text =
                slice_utf16(&current_line, delete_from, self.state.cursor_col);
            self.kill_ring.push(&deleted_text, true, was_kill);
            self.last_action = Some(EditorAction::Kill);

            self.state.lines[self.state.cursor_line] = format!(
                "{}{}",
                slice_utf16(&current_line, 0, delete_from),
                slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line))
            );
            self.set_cursor_col(delete_from);
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `deleteWordForward()` (editor.ts:1737-1777) — Alt+D.
    pub(crate) fn delete_word_forward(&mut self) {
        self.exit_history_browsing();

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        // If at end of line, merge with next line (delete the newline)
        if self.state.cursor_col >= utf16_len(&current_line) {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.push_undo_snapshot();

                // Treat newline as deleted text (forward deletion = append)
                self.kill_ring.push("\n", false, self.last_action == Some(EditorAction::Kill));
                self.last_action = Some(EditorAction::Kill);

                let next_line = self.state.lines[self.state.cursor_line + 1].clone();
                self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
                self.state.lines.remove(self.state.cursor_line + 1);
            }
        } else {
            self.push_undo_snapshot();

            // Save lastAction before cursor movement (moveWordForwards resets it)
            let was_kill = self.last_action == Some(EditorAction::Kill);

            let old_cursor_col = self.state.cursor_col;
            self.move_word_forwards();
            let delete_to = self.state.cursor_col;
            self.set_cursor_col(old_cursor_col);

            let deleted_text = slice_utf16(&current_line, self.state.cursor_col, delete_to);
            self.kill_ring.push(&deleted_text, false, was_kill);
            self.last_action = Some(EditorAction::Kill);

            self.state.lines[self.state.cursor_line] = format!(
                "{}{}",
                slice_utf16(&current_line, 0, self.state.cursor_col),
                slice_utf16(&current_line, delete_to, utf16_len(&current_line))
            );
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }
    }

    /// `handleForwardDelete()` (editor.ts:1779-1834) — Delete key.
    #[allow(clippy::if_same_then_else)] // TS if/else-if ladder with identical bodies (editor.ts:1819-1825)
    pub(crate) fn handle_forward_delete(&mut self) {
        self.exit_history_browsing();
        self.last_action = None;

        let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();

        if self.state.cursor_col < utf16_len(&current_line) {
            self.push_undo_snapshot();

            // Delete grapheme at cursor position (handles emojis, combining characters, etc.)
            let after_cursor = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));

            // Find the first grapheme at cursor
            let graphemes = self.segment_graphemes(&after_cursor);
            let first_grapheme = graphemes.first();
            let grapheme_length = first_grapheme.map(|g| utf16_len(&g.segment)).unwrap_or(1);

            let before = slice_utf16(&current_line, 0, self.state.cursor_col);
            let after = slice_utf16(
                &current_line,
                self.state.cursor_col + grapheme_length,
                utf16_len(&current_line),
            );
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();

            // At end of line - merge with next line
            let next_line = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = format!("{current_line}{next_line}");
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        let text = self.get_text();
        if let Some(cb) = self.on_change.as_mut() {
            cb(&text);
        }

        // Update or re-trigger autocomplete after forward delete
        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        } else {
            let current_line = self.state.lines.get(self.state.cursor_line).cloned().unwrap_or_default();
            let text_before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
            // Slash command context
            if self.is_in_slash_command_context(&text_before_cursor) {
                self.try_trigger_autocomplete(false);
            }
            // Symbol-based completion context like @, #, or provider triggers
            else if autocomplete_token_pattern_test(
                &text_before_cursor,
                &self.autocomplete_trigger_characters,
                false,
            ) {
                self.try_trigger_autocomplete(false);
            }
        }
    }
}

/// `normalizeText(text)` (editor.ts:1148-1155) — module-level pure fn
/// (CRLF/CR → LF, control-char filtering). `None` = `undefined` (never
/// passed by the current callers; treated as `""`).
pub(crate) fn normalize_text(text: Option<&str>) -> String {
    let text = text.unwrap_or("");
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' => out.push_str("    "),
            _ => out.push(c),
        }
    }
    out
}

// =============================================================================
// Ported upstream tests (packages/tui/test/editor.test.ts)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::components::editor::EditorTheme;
    use crate::test_support::{create_test_tui, create_test_tui_sized};
    use crate::utils::visible_width;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn test_editor() -> Editor {
        Editor::new(create_test_tui(), EditorTheme::default())
    }

    fn test_editor_sized(cols: usize, rows: usize) -> Editor {
        Editor::new(create_test_tui_sized(cols, rows), EditorTheme::default())
    }

    /// `text.match(/\[paste #\d+ \+\d+ lines\]/)` — the first lines-shaped
    /// paste marker in `text`, if any.
    fn first_lines_marker(text: &str) -> Option<String> {
        find_paste_marker_matches(text)
            .into_iter()
            .map(|m| text[m.start..m.end].to_string())
            .find(|m| m.contains('+') && m.ends_with(" lines]"))
    }

    /// `[...text.matchAll(/\[paste #\d+ \+\d+ lines\]/g)]`.
    fn all_lines_markers(text: &str) -> Vec<String> {
        find_paste_marker_matches(text)
            .into_iter()
            .map(|m| text[m.start..m.end].to_string())
            .filter(|m| m.contains('+') && m.ends_with(" lines]"))
            .collect()
    }

    /// `pasteWithMarker` helper (editor.test.ts:3804-3809): simulate a large
    /// paste that creates a marker.
    fn paste_with_marker(editor: &mut Editor) -> String {
        let big_content = "line\n".repeat(20);
        let big_content = big_content.trim_end_matches('\n');
        editor.handle_input(&format!("\u{1b}[200~{big_content}\u{1b}[201~"));
        // The editor replaces large pastes with a marker like "[paste #1 +20 lines]"
        editor.get_text()
    }

    /// `bigPaste(tag)` helper (editor.test.ts:3812-3814): 12-line paste
    /// content with a distinguishing tag.
    fn big_paste(tag: &str) -> String {
        (0..12).map(|i| format!("{tag}{i}")).collect::<Vec<_>>().join("\n")
    }

    // -------------------------------------------------------------------------
    // "public state accessors" (editor.test.ts:291-316)
    // -------------------------------------------------------------------------

    /// "returns cursor position" (editor.test.ts:292-305).
    #[test]
    fn returns_cursor_position() {
        let mut editor = test_editor();

        assert_eq!(editor.get_cursor(), (0, 0));

        editor.handle_input("a");
        editor.handle_input("b");
        editor.handle_input("c");

        assert_eq!(editor.get_cursor(), (0, 3));

        editor.handle_input("\u{1b}[D"); // Left
        assert_eq!(editor.get_cursor(), (0, 2));
    }

    /// "returns lines as a defensive copy" (editor.test.ts:307-316).
    #[test]
    fn returns_lines_as_a_defensive_copy() {
        let mut editor = test_editor();
        editor.set_text("a\nb");

        let mut lines = editor.get_lines();
        assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);

        lines[0] = "mutated".to_string();
        assert_eq!(editor.get_lines(), vec!["a".to_string(), "b".to_string()]);
    }

    // -------------------------------------------------------------------------
    // "Backslash+Enter newline workaround" (editor.test.ts:319-376)
    // -------------------------------------------------------------------------

    /// "inserts backslash immediately (no buffering)" (editor.test.ts:320-327).
    #[test]
    fn inserts_backslash_immediately_no_buffering() {
        let mut editor = test_editor();

        editor.handle_input("\\");

        // Backslash should be visible immediately, not buffered
        assert_eq!(editor.get_text(), "\\");
    }

    /// "converts standalone backslash to newline on Enter" (editor.test.ts:329-337).
    #[test]
    fn converts_standalone_backslash_to_newline_on_enter() {
        let mut editor = test_editor();

        editor.handle_input("\\");
        editor.handle_input("\r");

        assert_eq!(editor.get_text(), "\n");
    }

    /// "inserts backslash normally when followed by other characters"
    /// (editor.test.ts:338-345).
    #[test]
    fn inserts_backslash_normally_when_followed_by_other_characters() {
        let mut editor = test_editor();

        editor.handle_input("\\");
        editor.handle_input("x");

        assert_eq!(editor.get_text(), "\\x");
    }

    /// "does not trigger newline when backslash is not immediately before
    /// cursor" (editor.test.ts:347-361).
    #[test]
    fn does_not_trigger_newline_when_backslash_not_immediately_before_cursor() {
        let mut editor = test_editor();
        let submitted = Rc::new(RefCell::new(false));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |_t| {
            *sink.borrow_mut() = true;
        }));

        editor.handle_input("\\");
        editor.handle_input("x");
        editor.handle_input("\r");

        // Should submit, not insert newline (backslash not at cursor)
        assert!(*submitted.borrow());
    }

    /// "only removes one backslash when multiple are present"
    /// (editor.test.ts:363-375).
    #[test]
    fn only_removes_one_backslash_when_multiple_are_present() {
        let mut editor = test_editor();

        editor.handle_input("\\");
        editor.handle_input("\\");
        editor.handle_input("\\");
        assert_eq!(editor.get_text(), "\\\\\\");

        editor.handle_input("\r");
        // Only the last backslash is removed, newline inserted
        assert_eq!(editor.get_text(), "\\\\\n");
    }

    // -------------------------------------------------------------------------
    // "Kitty CSI-u handling" (editor.test.ts:377-401)
    // -------------------------------------------------------------------------

    /// "ignores printable CSI-u sequences with unsupported modifiers"
    /// (editor.test.ts:378-384).
    #[test]
    fn ignores_printable_csi_u_sequences_with_unsupported_modifiers() {
        let mut editor = test_editor();

        editor.handle_input("\u{1b}[99;9u");

        assert_eq!(editor.get_text(), "");
    }

    /// "inserts shifted CSI-u letters as text" (editor.test.ts:386-392).
    #[test]
    fn inserts_shifted_csi_u_letters_as_text() {
        let mut editor = test_editor();

        editor.handle_input("\u{1b}[69;2u");

        assert_eq!(editor.get_text(), "E");
    }

    /// "inserts shifted xterm modifyOtherKeys letters as text"
    /// (editor.test.ts:394-401).
    #[test]
    fn inserts_shifted_xterm_modify_other_keys_letters_as_text() {
        let mut editor = test_editor();

        editor.handle_input("\u{1b}[27;2;69~");

        assert_eq!(editor.get_text(), "E");
    }

    // -------------------------------------------------------------------------
    // "Unicode text editing behavior" (editor.test.ts:403-512)
    // -------------------------------------------------------------------------

    /// "inserts mixed ASCII, umlauts, and emojis as literal text"
    /// (editor.test.ts:404-421).
    #[test]
    fn inserts_mixed_ascii_umlauts_and_emojis_as_literal_text() {
        let mut editor = test_editor();

        for ch in ["H", "e", "l", "l", "o", " ", "ä", "ö", "ü", " ", "😀"] {
            editor.handle_input(ch);
        }

        assert_eq!(editor.get_text(), "Hello äöü 😀");
    }

    /// "deletes single-code-unit unicode characters (umlauts) with
    /// Backspace" (editor.test.ts:423-435).
    #[test]
    fn deletes_single_code_unit_unicode_characters_with_backspace() {
        let mut editor = test_editor();

        editor.handle_input("ä");
        editor.handle_input("ö");
        editor.handle_input("ü");

        // Delete the last character (ü)
        editor.handle_input("\u{7f}"); // Backspace

        assert_eq!(editor.get_text(), "äö");
    }

    /// "deletes multi-code-unit emojis with single Backspace"
    /// (editor.test.ts:437-448).
    #[test]
    fn deletes_multi_code_unit_emojis_with_single_backspace() {
        let mut editor = test_editor();

        editor.handle_input("😀");
        editor.handle_input("👍");

        // Delete the last emoji (👍) - single backspace deletes whole grapheme cluster
        editor.handle_input("\u{7f}"); // Backspace

        assert_eq!(editor.get_text(), "😀");
    }

    /// "inserts characters at the correct position after cursor movement
    /// over umlauts" (editor.test.ts:450-466).
    #[test]
    fn inserts_characters_at_correct_position_after_cursor_movement_over_umlauts() {
        let mut editor = test_editor();

        editor.handle_input("ä");
        editor.handle_input("ö");
        editor.handle_input("ü");

        // Move cursor left twice
        editor.handle_input("\u{1b}[D"); // Left arrow
        editor.handle_input("\u{1b}[D"); // Left arrow

        // Insert 'x' in the middle
        editor.handle_input("x");

        assert_eq!(editor.get_text(), "äxöü");
    }

    /// "moves cursor across multi-code-unit emojis with single arrow key"
    /// (editor.test.ts:468-486).
    #[test]
    fn moves_cursor_across_multi_code_unit_emojis_with_single_arrow_key() {
        let mut editor = test_editor();

        editor.handle_input("😀");
        editor.handle_input("👍");
        editor.handle_input("🎉");

        // Move cursor left over last emoji (🎉) - single arrow moves over whole grapheme
        editor.handle_input("\u{1b}[D"); // Left arrow

        // Move cursor left over second emoji (👍)
        editor.handle_input("\u{1b}[D");

        // Insert 'x' between first and second emoji
        editor.handle_input("x");

        assert_eq!(editor.get_text(), "😀x👍🎉");
    }

    /// "preserves umlauts across line breaks" (editor.test.ts:488-501).
    #[test]
    fn preserves_umlauts_across_line_breaks() {
        let mut editor = test_editor();

        editor.handle_input("ä");
        editor.handle_input("ö");
        editor.handle_input("ü");
        editor.handle_input("\n"); // new line
        editor.handle_input("Ä");
        editor.handle_input("Ö");
        editor.handle_input("Ü");

        assert_eq!(editor.get_text(), "äöü\nÄÖÜ");
    }

    /// "replaces the entire document with unicode text via setText (paste
    /// simulation)" (editor.test.ts:503-511).
    #[test]
    fn replaces_entire_document_with_unicode_text_via_set_text() {
        let mut editor = test_editor();

        // Simulate bracketed paste / programmatic replacement
        editor.set_text("Hällö Wörld! 😀 äöüÄÖÜß");

        assert_eq!(editor.get_text(), "Hällö Wörld! 😀 äöüÄÖÜß");
    }

    /// "moves cursor to document start on Ctrl+A and inserts at the
    /// beginning" (editor.test.ts:513-523).
    #[test]
    fn moves_cursor_to_document_start_on_ctrl_a_and_inserts_at_beginning() {
        let mut editor = test_editor();

        editor.handle_input("a");
        editor.handle_input("b");
        editor.handle_input("\u{01}"); // Ctrl+A (move to start)
        editor.handle_input("x"); // Insert at start

        assert_eq!(editor.get_text(), "xab");
    }

    /// "deletes words correctly with Ctrl+W and Alt+Backspace"
    /// (editor.test.ts:525-553).
    #[test]
    fn deletes_words_correctly_with_ctrl_w_and_alt_backspace() {
        let mut editor = test_editor();

        // Basic word deletion
        editor.set_text("foo bar baz");
        editor.handle_input("\u{17}"); // Ctrl+W
        assert_eq!(editor.get_text(), "foo bar ");

        // Trailing whitespace
        editor.set_text("foo bar   ");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "foo ");

        // Punctuation run
        editor.set_text("foo bar...");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "foo bar");

        // ASCII punctuation inside Intl word-like segments preserves old boundaries
        editor.set_text("foo.bar");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "foo.");

        editor.set_text("foo:bar");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "foo:");

        // Delete across multiple lines
        editor.set_text("line one\nline two");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "line one\nline ");

        // Delete empty line (merge)
        editor.set_text("line one\n");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "line one");

        // Grapheme safety (emoji as a word)
        editor.set_text("foo 😀😀 bar");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "foo 😀😀 ");
        editor.handle_input("\u{17}");
        assert_eq!(editor.get_text(), "foo ");

        // Alt+Backspace
        editor.set_text("foo bar");
        editor.handle_input("\u{1b}\u{7f}"); // Alt+Backspace (legacy)
        assert_eq!(editor.get_text(), "foo ");
    }

    // -------------------------------------------------------------------------
    // insertTextAtCursor (editor.test.ts:1888-1925)
    // -------------------------------------------------------------------------

    /// "insertTextAtCursor handles multiline text" (editor.test.ts:1888-1907).
    #[test]
    fn insert_text_at_cursor_handles_multiline_text() {
        let mut editor = test_editor();

        editor.set_text("hello world");
        editor.handle_input("\u{01}"); // Ctrl+A - go to start
        for _ in 0..5 {
            editor.handle_input("\u{1b}[C"); // Move right 5 (after "hello", before space)
        }

        // Insert multiline text
        editor.insert_text_at_cursor("line1\nline2\nline3");
        assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");

        // Cursor should be at end of inserted text (after "line3", before " world")
        let cursor = editor.get_cursor();
        assert_eq!(cursor, (2, 5)); // "line3".length

        // Single undo should restore entire pre-insert state
        editor.handle_input("\u{1b}[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");
    }

    /// "insertTextAtCursor normalizes CRLF and CR line endings"
    /// (editor.test.ts:1909-1924).
    #[test]
    fn insert_text_at_cursor_normalizes_crlf_and_cr_line_endings() {
        let mut editor = test_editor();

        editor.set_text("");

        // Insert text with CRLF
        editor.insert_text_at_cursor("a\r\nb\r\nc");
        assert_eq!(editor.get_text(), "a\nb\nc");

        editor.handle_input("\u{1b}[45;5u"); // Undo
        assert_eq!(editor.get_text(), "");

        // Insert text with CR only
        editor.insert_text_at_cursor("x\ry\rz");
        assert_eq!(editor.get_text(), "x\ny\nz");
    }

    // -------------------------------------------------------------------------
    // "Paste marker atomic behavior" (editor.test.ts:3801-4380)
    // (except "wordWrapLine re-checks overflow" at 4124 — ported by D2)
    // -------------------------------------------------------------------------

    /// "creates a paste marker for large pastes" (editor.test.ts:3815-3819).
    #[test]
    fn creates_a_paste_marker_for_large_pastes() {
        let mut editor = test_editor();
        let text = paste_with_marker(&mut editor);
        assert!(first_lines_marker(&text).is_some());
    }

    /// "treats paste marker as single unit for right arrow"
    /// (editor.test.ts:3821-3844).
    #[test]
    fn treats_paste_marker_as_single_unit_for_right_arrow() {
        let mut editor = test_editor();
        editor.handle_input("A");
        paste_with_marker(&mut editor);
        editor.handle_input("B");
        // Text: "A[paste #1 +20 lines]B", cursor at end

        // Go to start
        editor.handle_input("\u{01}"); // Ctrl+A
        assert_eq!(editor.get_cursor(), (0, 0));

        // Right arrow: should move past "A"
        editor.handle_input("\u{1b}[C");
        assert_eq!(editor.get_cursor(), (0, 1));

        // Right arrow: should skip the entire marker
        editor.handle_input("\u{1b}[C");
        let marker = first_lines_marker(&editor.get_text()).unwrap();
        assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

        // Right arrow: should move past "B"
        editor.handle_input("\u{1b}[C");
        assert_eq!(editor.get_cursor(), (0, 1 + marker.len() + 1));
    }

    /// "treats paste marker as single unit for left arrow"
    /// (editor.test.ts:3846-3866).
    #[test]
    fn treats_paste_marker_as_single_unit_for_left_arrow() {
        let mut editor = test_editor();
        editor.handle_input("A");
        paste_with_marker(&mut editor);
        editor.handle_input("B");
        // Cursor at end

        // Left arrow: past "B"
        editor.handle_input("\u{1b}[D");
        let text = editor.get_text();
        let marker = first_lines_marker(&text).unwrap();
        assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

        // Left arrow: skip the entire marker
        editor.handle_input("\u{1b}[D");
        assert_eq!(editor.get_cursor(), (0, 1));

        // Left arrow: past "A"
        editor.handle_input("\u{1b}[D");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    /// "treats paste marker as single unit for backspace"
    /// (editor.test.ts:3868-3888).
    #[test]
    fn treats_paste_marker_as_single_unit_for_backspace() {
        let mut editor = test_editor();
        editor.handle_input("A");
        paste_with_marker(&mut editor);
        editor.handle_input("B");

        let text = editor.get_text();
        let marker = first_lines_marker(&text).unwrap();

        // Position cursor right after the marker (before "B")
        editor.handle_input("\u{01}"); // Ctrl+A
        // Move past "A" and the marker
        editor.handle_input("\u{1b}[C"); // past "A"
        editor.handle_input("\u{1b}[C"); // past marker
        assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

        // Backspace: should delete the entire marker at once
        editor.handle_input("\u{7f}");
        assert_eq!(editor.get_text(), "AB");
        assert_eq!(editor.get_cursor(), (0, 1));
    }

    /// "treats paste marker as single unit for forward delete"
    /// (editor.test.ts:3890-3904).
    #[test]
    fn treats_paste_marker_as_single_unit_for_forward_delete() {
        let mut editor = test_editor();
        editor.handle_input("A");
        paste_with_marker(&mut editor);
        editor.handle_input("B");

        // Position cursor on "A" (col 0) then move right once to be just before marker
        editor.handle_input("\u{01}"); // Ctrl+A
        editor.handle_input("\u{1b}[C"); // past "A", now at col 1 (start of marker)

        // Forward delete: should delete the entire marker at once
        editor.handle_input("\u{1b}[3~"); // Delete key
        assert_eq!(editor.get_text(), "AB");
        assert_eq!(editor.get_cursor(), (0, 1));
    }

    /// "treats paste marker as single unit for word movement"
    /// (editor.test.ts:3906-3928).
    #[test]
    fn treats_paste_marker_as_single_unit_for_word_movement() {
        let mut editor = test_editor();
        editor.handle_input("X");
        editor.handle_input(" ");
        paste_with_marker(&mut editor);
        editor.handle_input(" ");
        editor.handle_input("Y");
        // Text: "X [paste #1 +20 lines] Y"

        let text = editor.get_text();
        let marker = first_lines_marker(&text).unwrap();

        // Go to start
        editor.handle_input("\u{01}"); // Ctrl+A

        // Ctrl+Right: skip "X"
        editor.handle_input("\u{1b}[1;5C");
        assert_eq!(editor.get_cursor(), (0, 1));

        // Ctrl+Right: skip whitespace + marker (marker treated as single non-ws, non-punct unit)
        editor.handle_input("\u{1b}[1;5C");
        assert_eq!(editor.get_cursor(), (0, 2 + marker.len()));
    }

    /// "undo restores marker after backspace deletion" (editor.test.ts:3930-3950).
    #[test]
    fn undo_restores_marker_after_backspace_deletion() {
        let mut editor = test_editor();
        editor.handle_input("A");
        paste_with_marker(&mut editor);
        editor.handle_input("B");

        let text_before = editor.get_text();

        // Position after marker
        editor.handle_input("\u{01}");
        editor.handle_input("\u{1b}[C"); // past A
        editor.handle_input("\u{1b}[C"); // past marker

        // Delete marker
        editor.handle_input("\u{7f}");
        assert_eq!(editor.get_text(), "AB");

        // Undo
        editor.handle_input("\u{1b}[45;5u");
        assert_eq!(editor.get_text(), text_before);
    }

    /// "undo after paste marker deletion restores the paste registry"
    /// (editor.test.ts:3952-3965).
    #[test]
    fn undo_after_paste_marker_deletion_restores_paste_registry() {
        let mut editor = test_editor();
        let submitted = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |t| {
            *sink.borrow_mut() = t.to_string();
        }));

        let paste = big_paste("alpha");
        editor.handle_input(&format!("\u{1b}[200~{paste}\u{1b}[201~"));
        editor.handle_input("\u{7f}"); // delete the marker
        editor.handle_input("\u{1b}[45;5u"); // undo: restores marker text and registry
        editor.handle_input("\r");
        assert_eq!(*submitted.borrow(), paste);
    }

    /// "undo after deleting the first of two paste markers restores both
    /// registry entries" (editor.test.ts:3967-3984).
    #[test]
    fn undo_after_deleting_first_of_two_markers_restores_both_registry_entries() {
        let mut editor = test_editor();
        let submitted = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |t| {
            *sink.borrow_mut() = t.to_string();
        }));

        let paste_a = big_paste("alpha");
        let paste_b = big_paste("beta");
        editor.handle_input(&format!("\u{1b}[200~{paste_a}\u{1b}[201~")); // #1 = A
        editor.handle_input(&format!("\u{1b}[200~{paste_b}\u{1b}[201~")); // #2 = B, cursor at end
        editor.handle_input("\u{01}"); // Ctrl+A
        editor.handle_input("\u{1b}[C"); // right over marker #1
        editor.handle_input("\u{7f}"); // delete marker #1, renumbers #2 -> #1
        editor.handle_input("\u{1b}[45;5u"); // undo
        editor.handle_input("\r");
        assert_eq!(*submitted.borrow(), format!("{paste_a}{paste_b}"));
    }

    /// "renumbers the paste registry in ascending id order when markers are
    /// out of order in text" (editor.test.ts:3986-4005).
    #[test]
    fn renumbers_paste_registry_in_ascending_id_order_when_markers_out_of_order() {
        let mut editor = test_editor();
        let submitted = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |t| {
            *sink.borrow_mut() = t.to_string();
        }));

        let paste_a = big_paste("alpha");
        let paste_b = big_paste("beta");
        let paste_c = big_paste("gamma");
        editor.handle_input(&format!("\u{1b}[200~{paste_a}\u{1b}[201~")); // #1 = A
        editor.handle_input("\u{01}"); // Ctrl+A
        editor.handle_input(&format!("\u{1b}[200~{paste_b}\u{1b}[201~")); // #2 = B, text: [#2][#1]
        editor.handle_input("\u{01}"); // Ctrl+A
        editor.handle_input(&format!("\u{1b}[200~{paste_c}\u{1b}[201~")); // #3 = C, text: [#3][#2][#1]
        editor.handle_input("\u{05}"); // Ctrl+E
        editor.handle_input("\u{7f}"); // delete marker #1, renumber #3 -> #2 and #2 -> #1
        editor.handle_input("\r");
        assert_eq!(*submitted.borrow(), format!("{paste_c}{paste_b}"));
    }

    /// "undo after setText restores paste markers and registry"
    /// (editor.test.ts:4007-4020).
    #[test]
    fn undo_after_set_text_restores_paste_markers_and_registry() {
        let mut editor = test_editor();
        let submitted = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |t| {
            *sink.borrow_mut() = t.to_string();
        }));

        let paste = big_paste("alpha");
        editor.handle_input(&format!("\u{1b}[200~{paste}\u{1b}[201~"));
        editor.set_text("replacement");
        editor.handle_input("\u{1b}[45;5u"); // undo
        editor.handle_input("\r");
        assert_eq!(*submitted.borrow(), paste);
    }

    /// "handles multiple paste markers in same line" (editor.test.ts:4022-4049).
    #[test]
    fn handles_multiple_paste_markers_in_same_line() {
        let mut editor = test_editor();
        paste_with_marker(&mut editor);
        editor.handle_input(" ");
        paste_with_marker(&mut editor);

        let text = editor.get_text();
        let markers = all_lines_markers(&text);
        assert_eq!(markers.len(), 2);

        // Go to start
        editor.handle_input("\u{01}");

        // Right arrow: should skip first marker atomically
        editor.handle_input("\u{1b}[C");
        assert_eq!(editor.get_cursor(), (0, markers[0].len()));

        // Right arrow: past space
        editor.handle_input("\u{1b}[C");
        assert_eq!(editor.get_cursor(), (0, markers[0].len() + 1));

        // Right arrow: should skip second marker atomically
        editor.handle_input("\u{1b}[C");
        assert_eq!(editor.get_cursor(), (0, markers[0].len() + 1 + markers[1].len()));
    }

    /// "does not treat manually typed marker-like text as atomic (no valid
    /// paste ID)" (editor.test.ts:4051-4064).
    #[test]
    fn does_not_treat_manually_typed_marker_like_text_as_atomic() {
        let mut editor = test_editor();
        // Type text that matches the pattern but was typed manually (no paste entry)
        let fake_marker = "[paste #99 +5 lines]";
        for ch in fake_marker.chars() {
            editor.handle_input(&ch.to_string());
        }

        assert_eq!(editor.get_text(), fake_marker);

        // No paste with ID 99 exists, so the marker is NOT treated atomically.
        // Right arrow should move one grapheme at a time.
        editor.handle_input("\u{01}"); // Ctrl+A
        editor.handle_input("\u{1b}[C"); // Right
        assert_eq!(editor.get_cursor(), (0, 1)); // Just past "["
    }

    /// "does not crash when paste marker is wider than terminal width"
    /// (editor.test.ts:4066-4087).
    #[test]
    fn does_not_crash_when_paste_marker_is_wider_than_terminal_width() {
        // Reproduce: terminal width 8, paste marker "[paste #1 +47 lines]" (21 chars)
        let mut editor = test_editor();
        let big_content = "line\n".repeat(47);
        let big_content = big_content.trim_end_matches('\n');
        editor.handle_input(&format!("\u{1b}[200~{big_content}\u{1b}[201~"));

        let text = editor.get_text();
        let marker = first_lines_marker(&text).expect("paste marker should be created");
        assert!(visible_width(&marker) > 8, "marker should be wider than render width");

        // Render at very narrow width - should not throw
        let lines = editor.render(8);
        // Every rendered line must fit within the width (marker is split)
        for line in &lines {
            assert!(
                visible_width(line) <= 8,
                "line exceeds width 8: visible={} text={line:?}",
                visible_width(line)
            );
        }
    }

    /// "does not crash when text + paste marker exceeds terminal width with
    /// cursor on marker" (editor.test.ts:4089-4122).
    #[test]
    fn does_not_crash_when_text_plus_marker_exceeds_width_with_cursor_on_marker() {
        // Reproduce: terminal width 54, text "b".repeat(35) + "[paste #1 +27 lines]" + "bbbb"
        // Cursor lands on the paste marker after word-wrap, causing the rendered line
        // to be 55 visible chars (1 over the width).
        let mut editor = test_editor();

        // Type 35 'b' characters
        for _ in 0..35 {
            editor.handle_input("b");
        }

        // Paste 27 lines
        let big_content = "line\n".repeat(27);
        let big_content = big_content.trim_end_matches('\n');
        editor.handle_input(&format!("\u{1b}[200~{big_content}\u{1b}[201~"));

        // Type a few more characters
        for _ in 0..4 {
            editor.handle_input("b");
        }

        // Move cursor left to land on the paste marker
        editor.handle_input("\u{1b}[D"); // past last 'b'
        editor.handle_input("\u{1b}[D"); // past last 'b'
        editor.handle_input("\u{1b}[D"); // past last 'b'
        editor.handle_input("\u{1b}[D"); // past last 'b'
        editor.handle_input("\u{1b}[D"); // now on the paste marker

        // Render at width 54 - should not throw
        let render_width = 54;
        let lines = editor.render(render_width);
        for line in &lines {
            assert!(
                visible_width(line) <= render_width,
                "line exceeds width {render_width}: visible={} text={line:?}",
                visible_width(line)
            );
        }
    }

    /// "expands large pasted content literally in getExpandedText"
    /// (editor.test.ts:4153-4173).
    #[test]
    fn expands_large_pasted_content_literally_in_get_expanded_text() {
        let mut editor = test_editor();
        let pasted_text = [
            "line 1",
            "line 2",
            "line 3",
            "line 4",
            "line 5",
            "line 6",
            "line 7",
            "line 8",
            "line 9",
            "line 10",
            "tokens $1 $2 $& $$ $` $' end",
        ]
        .join("\n");

        editor.handle_input(&format!("\u{1b}[200~{pasted_text}\u{1b}[201~"));

        assert!(first_lines_marker(&editor.get_text()).is_some());
        assert_eq!(editor.get_expanded_text(), pasted_text);
    }

    /// "snaps to the paste marker start when navigating down into it"
    /// (editor.test.ts:4175-4208).
    #[test]
    fn snaps_to_paste_marker_start_when_navigating_down_into_it() {
        let mut editor = test_editor();

        // Line 0: long enough text to establish a sticky column
        editor.set_text("12345678901234567890\n\nhello ");

        // Create a large paste to get a marker
        let big_content = "x".repeat(2000);
        editor.handle_input(&format!("\u{1b}[200~{big_content}\u{1b}[201~"));
        editor.render(80);

        // Line 0: "12345678901234567890"
        // Line 1: "" (empty)
        // Line 2: "hello [paste #1 2000 chars]"
        //         marker starts at col 6

        // Navigate to line 0, col 10
        editor.handle_input("\u{1b}[A"); // Up to line 1
        editor.handle_input("\u{1b}[A"); // Up to line 0
        editor.handle_input("\u{01}"); // Ctrl+A (start of line)
        for _ in 0..10 {
            editor.handle_input("\u{1b}[C"); // Right 10
        }
        assert_eq!(editor.get_cursor(), (0, 10));

        // Down to empty line
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (1, 0));

        // Down to paste marker line - sticky col 10 falls inside marker (starts at col 6).
        // Cursor should snap to start of marker (col 6), not end (col 6 + marker.length).
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (2, 6));
    }

    /// "preserves sticky column when navigating through paste marker line"
    /// (editor.test.ts:4210-4250).
    #[test]
    fn preserves_sticky_column_when_navigating_through_paste_marker_line() {
        let mut editor = test_editor_sized(30, 24);

        // Build:
        // Line 0: "1234567890123456" (16 chars)
        // Line 1: "" (empty)
        // Line 2: "[paste #1 2000 chars]" (22 chars, paste marker)
        // Line 3: "" (empty)
        // Line 4: "abcdefghijklmnop" (16 chars)
        for ch in "1234567890123456".chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.handle_input("\n");
        editor.handle_input("\n");
        editor.handle_input(&format!("\u{1b}[200~{}{}\u{1b}[201~", "x".repeat(2000), ""));
        editor.handle_input("\n");
        editor.handle_input("\n");
        for ch in "abcdefghijklmnop".chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.render(30);

        // Navigate to line 0, col 10
        for _ in 0..4 {
            editor.handle_input("\u{1b}[A"); // Up to line 0
        }
        editor.handle_input("\u{01}"); // Ctrl+A
        for _ in 0..10 {
            editor.handle_input("\u{1b}[C");
        }
        assert_eq!(editor.get_cursor(), (0, 10));

        // Down to empty line - sticky col 10 established
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (1, 0));

        // Down to paste marker - cursor snapped to col 0 (start of marker)
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (2, 0));

        // Down to empty line
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (3, 0));

        // Down to last line - should restore sticky col 10
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (4, 10));
    }

    /// "does not get stuck moving down from a multi-visual-line paste
    /// marker" (editor.test.ts:4252-4311).
    #[test]
    fn does_not_get_stuck_moving_down_from_multi_visual_line_paste_marker() {
        let mut editor = test_editor_sized(20, 24);

        // Build:
        // Logical line 0: "abcdefgh" + marker(21 chars) + "ijklmnopqr"
        // Logical line 1: "123456789012345678"
        //
        // Marker "[paste #1 +100 lines]" (21 chars) is wider than the
        // terminal (20). Word-wrap splits at the space before "lines",
        // producing:
        //   VL1: abcdefgh              (startCol 0,  len 8)
        //   VL2: [paste #1 +100        (startCol 8,  len 15) <- marker head
        //   VL3: lines]ijklmnopqr      (startCol 23, len 16) <- marker tail + content
        //   VL4: 123456789012345678    (line 1)
        //
        // On VL3 the marker tail "lines]" occupies visual cols 0-5.
        // Content ("i") starts at visual col 6 = logical col 29.
        for ch in "abcdefgh".chars() {
            editor.handle_input(&ch.to_string());
        }
        let big_content = "line\n".repeat(100);
        let big_content = big_content.trim_end_matches('\n');
        editor.handle_input(&format!("\u{1b}[200~{big_content}\u{1b}[201~"));
        for ch in "ijklmnopqr".chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.handle_input("\n");
        for ch in "123456789012345678".chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.render(20);

        let text = editor.get_text();
        let marker = first_lines_marker(&text).expect("paste marker should be created");
        let marker_len = marker.len(); // 21
        assert!(marker_len > 20, "marker should be wider than terminal");
        let marker_start = 8;
        let marker_end = marker_start + marker_len; // 29

        // Navigate to line 0, col 6 (on "g"). Preferred col 6 is past the
        // marker tail on VL3, so the cursor should land on content ("i" at
        // col 29) without snapping back.
        editor.handle_input("\u{1b}[A"); // Up to line 0
        editor.handle_input("\u{01}"); // Ctrl+A (start of line)
        for _ in 0..6 {
            editor.handle_input("\u{1b}[C"); // Right to col 6
        }
        assert_eq!(editor.get_cursor(), (0, 6));

        // Down: cursor lands on paste marker start
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (0, marker_start));

        // Down again: preferred col 6 lands at VL3 col 29 ("i"), which is
        // past the marker. Cursor stays on line 0.
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor().0, 0);
        assert_eq!(editor.get_cursor().1, marker_end); // col 29 = "i"

        // Up: back to paste marker
        editor.handle_input("\u{1b}[A");
        assert_eq!(editor.get_cursor(), (0, marker_start));

        // Up again: back to col 6 ("g")
        editor.handle_input("\u{1b}[A");
        assert_eq!(editor.get_cursor(), (0, 6));
    }

    /// "skips marker continuation VLs when preferred col falls in marker
    /// tail" (editor.test.ts:4313-4352).
    #[test]
    fn skips_marker_continuation_vls_when_preferred_col_falls_in_marker_tail() {
        let mut editor = test_editor_sized(20, 24);

        // Same layout. Start at col 3 ("d"). Preferred col 3 maps to VL3
        // visual col 3 which is inside the "lines]" marker tail.
        // moveToVisualLine detects the continuation VL and skips to VL4
        // (line 1).
        for ch in "abcdefgh".chars() {
            editor.handle_input(&ch.to_string());
        }
        let big_content = "line\n".repeat(100);
        let big_content = big_content.trim_end_matches('\n');
        editor.handle_input(&format!("\u{1b}[200~{big_content}\u{1b}[201~"));
        for ch in "ijklmnopqr".chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.handle_input("\n");
        for ch in "123456789012345678".chars() {
            editor.handle_input(&ch.to_string());
        }
        editor.render(20);

        // Navigate to line 0, col 3 (on "d")
        editor.handle_input("\u{1b}[A"); // Up to line 0
        editor.handle_input("\u{01}"); // Ctrl+A
        for _ in 0..3 {
            editor.handle_input("\u{1b}[C");
        }
        assert_eq!(editor.get_cursor(), (0, 3));

        // Down: marker
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor().1, 8);

        // Down: skips VL3 (col 3 in marker tail) and lands on line 1
        editor.handle_input("\u{1b}[B");
        assert_eq!(editor.get_cursor(), (1, 3));

        // Round-trip back
        editor.handle_input("\u{1b}[A");
        assert_eq!(editor.get_cursor().1, 8); // marker
        editor.handle_input("\u{1b}[A");
        assert_eq!(editor.get_cursor(), (0, 3));
    }

    /// "submits large pasted content literally" (editor.test.ts:4354-4378).
    #[test]
    fn submits_large_pasted_content_literally() {
        let mut editor = test_editor();
        let pasted_text = [
            "line 1",
            "line 2",
            "line 3",
            "line 4",
            "line 5",
            "line 6",
            "line 7",
            "line 8",
            "line 9",
            "line 10",
            "tokens $1 $2 $& $$ $` $' end",
        ]
        .join("\n");
        let submitted = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |text| {
            *sink.borrow_mut() = text.to_string();
        }));

        editor.handle_input(&format!("\u{1b}[200~{pasted_text}\u{1b}[201~"));
        editor.handle_input("\r");

        assert_eq!(*submitted.borrow(), pasted_text);
    }
}
