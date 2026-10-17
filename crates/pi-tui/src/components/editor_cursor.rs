//! Cursor movement: column setting, visual-line navigation, sticky column,
//! word movement, page scroll, and character jump mode (editor.ts:1469-1612,
//! 1836-2077, 2138-2166, 2168-2186).
//!
//! **Owner: T4 delegate E3.** Ported 1:1 from
//! `/tmp/pi-repo/packages/tui/src/components/editor.ts` (line refs in each
//! doc comment). All cursor/segment positions are UTF-16 code units.

use super::editor::{
    js_index_of, js_last_index_of, slice_utf16, Editor, JumpDirection, VisualLineRef,
};
use crate::components::editor_word_wrap::{is_paste_marker, word_wrap_line};
use crate::unicode::word_break::WordSegment;
use crate::utils::{utf16_len, visible_width};
use crate::word_navigation::{find_word_backward, find_word_forward, WordNavigationOptions};

impl Editor {
    /// `setCursorCol(col)` (editor.ts:1469-1477). Set cursor column and
    /// clear `preferredVisualCol` — used by all non-vertical cursor
    /// movements to reset sticky column behavior.
    pub(crate) fn set_cursor_col(&mut self, col: usize) {
        self.state.cursor_col = col;
        self.preferred_visual_col = None;
        self.snapped_from_cursor_col = None;
    }

    /// `moveToVisualLine(visualLines, currentVisualLine, targetVisualLine)`
    /// (editor.ts:1479-1606). Resolves the pre-snap column, computes the
    /// target visual column, and moves (recursing across atomic-segment
    /// continuation visual lines).
    pub(crate) fn move_to_visual_line(
        &mut self,
        visual_lines: &[VisualLineRef],
        current_visual_line: usize,
        target_visual_line: usize,
    ) {
        let current_vl = match visual_lines.get(current_visual_line) {
            Some(vl) => vl,
            None => return,
        };
        let target_vl = match visual_lines.get(target_visual_line) {
            Some(vl) => vl,
            None => return,
        };

        // When the cursor was snapped to a segment start, resolve the
        // pre-snap position against the VL it belongs to. This gives the
        // correct visual column even after a resize reshuffles VLs.
        let current_visual_col: i64 = if let Some(snapped_from_cursor_col) = self.snapped_from_cursor_col {
            let vl_index =
                self.find_visual_line_at(visual_lines, current_vl.logical_line, snapped_from_cursor_col);
            snapped_from_cursor_col as i64 - visual_lines[vl_index].start_col as i64
        } else {
            self.state.cursor_col as i64 - current_vl.start_col as i64
        };

        // For non-last segments, clamp to length-1 to stay within the segment
        let is_last_source_segment = current_visual_line == visual_lines.len() - 1
            || visual_lines
                .get(current_visual_line + 1)
                .map(|next| next.logical_line)
                != Some(current_vl.logical_line);
        let source_max_visual_col: i64 = if is_last_source_segment {
            current_vl.length as i64
        } else {
            (current_vl.length as i64 - 1).max(0)
        };

        let is_last_target_segment = target_visual_line == visual_lines.len() - 1
            || visual_lines
                .get(target_visual_line + 1)
                .map(|next| next.logical_line)
                != Some(target_vl.logical_line);
        let target_max_visual_col: i64 = if is_last_target_segment {
            target_vl.length as i64
        } else {
            (target_vl.length as i64 - 1).max(0)
        };

        let move_to_visual_col =
            self.compute_vertical_move_column(current_visual_col, source_max_visual_col, target_max_visual_col);

        // Set cursor position
        self.state.cursor_line = target_vl.logical_line;
        let target_col = target_vl.start_col as i64 + move_to_visual_col;
        let logical_line = self
            .state
            .lines
            .get(target_vl.logical_line)
            .cloned()
            .unwrap_or_default();
        self.state.cursor_col =
            target_col.min(utf16_len(&logical_line) as i64).max(0) as usize;

        // Snap cursor to atomic segment boundary (e.g. paste markers)
        // so the cursor never lands in the middle of a multi-grapheme unit.
        // Single-grapheme segments don't need snapping.
        let segments = self.segment_graphemes(&logical_line);
        for seg in segments {
            if seg.index > self.state.cursor_col {
                break;
            }
            let seg_len = utf16_len(&seg.segment);
            if seg_len <= 1 {
                continue;
            }
            if self.state.cursor_col < seg.index + seg_len {
                let is_continuation = seg.index < target_vl.start_col;
                let is_moving_down = target_visual_line > current_visual_line;

                if is_continuation && is_moving_down {
                    // The segment started on a previous visual line, and we
                    // already visited it on the way down. Skip all remaining
                    // continuation VLs and land on the first VL past it.
                    let seg_end = seg.index + seg_len;
                    let mut next = target_visual_line + 1;
                    while next < visual_lines.len()
                        && visual_lines[next].logical_line == target_vl.logical_line
                        && visual_lines[next].start_col < seg_end
                    {
                        next += 1;
                    }
                    if next < visual_lines.len() {
                        self.move_to_visual_line(visual_lines, current_visual_line, next);
                        return;
                    }
                }

                // Snap to the start of the segment so it gets highlighted.
                // Store the pre-snap position so the next vertical move can
                // resolve it to the correct visual column.
                self.snapped_from_cursor_col = Some(self.state.cursor_col);
                self.state.cursor_col = seg.index;
                return;
            }
        }

        // No snap occurred – we moved out of the atomic segment.
        self.snapped_from_cursor_col = None;
    }

    /// `computeVerticalMoveColumn(currentVisualCol, sourceMaxVisualCol,
    /// targetMaxVisualCol)` (editor.ts:1581-1612) — the sticky-column
    /// decision table (see the TS doc comment for the full table; P =
    /// preferred set, S = cursor in middle of source, T = target shorter
    /// than current, U = target shorter than preferred).
    pub(crate) fn compute_vertical_move_column(
        &mut self,
        current_visual_col: i64,
        source_max_visual_col: i64,
        target_max_visual_col: i64,
    ) -> i64 {
        let has_preferred = self.preferred_visual_col.is_some(); // P
        let cursor_in_middle = current_visual_col < source_max_visual_col; // S
        let target_too_short = target_max_visual_col < current_visual_col; // T

        if !has_preferred || cursor_in_middle {
            if target_too_short {
                // Cases 2 and 7
                self.preferred_visual_col = Some(current_visual_col);
                return target_max_visual_col;
            }

            // Cases 1 and 6
            self.preferred_visual_col = None;
            return current_visual_col;
        }

        let preferred_visual_col = self.preferred_visual_col.unwrap_or(0);
        let target_cant_fit_preferred = target_max_visual_col < preferred_visual_col; // U
        if target_too_short || target_cant_fit_preferred {
            // Cases 4 and 5
            return target_max_visual_col;
        }

        // Case 3
        let result = preferred_visual_col;
        self.preferred_visual_col = None;
        result
    }

    /// `moveToLineStart()` (editor.ts:1614-1617) — Ctrl+A / Home.
    pub(crate) fn move_to_line_start(&mut self) {
        self.last_action = None;
        self.set_cursor_col(0);
    }

    /// `moveToLineEnd()` (editor.ts:1619-1623) — Ctrl+E / End.
    pub(crate) fn move_to_line_end(&mut self) {
        self.last_action = None;
        let current_line = self
            .state
            .lines
            .get(self.state.cursor_line)
            .cloned()
            .unwrap_or_default();
        self.set_cursor_col(utf16_len(&current_line));
    }

    /// `buildVisualLineMap(width)` (editor.ts:1836-1864) — the wrapped
    /// visual-line index built from `wordWrapLine` chunks.
    pub(crate) fn build_visual_line_map(&self, width: usize) -> Vec<VisualLineRef> {
        let mut visual_lines: Vec<VisualLineRef> = Vec::new();

        for i in 0..self.state.lines.len() {
            let line = self.state.lines.get(i).cloned().unwrap_or_default();
            let line_vis_width = visible_width(&line);
            if utf16_len(&line) == 0 {
                // Empty line still takes one visual line
                visual_lines.push(VisualLineRef { logical_line: i, start_col: 0, length: 0 });
            } else if line_vis_width <= width {
                visual_lines.push(VisualLineRef {
                    logical_line: i,
                    start_col: 0,
                    length: utf16_len(&line),
                });
            } else {
                // Line needs wrapping - use word-aware wrapping
                let graphemes = self.segment_graphemes(&line);
                let chunks = word_wrap_line(&line, width, Some(&graphemes));
                for chunk in chunks {
                    visual_lines.push(VisualLineRef {
                        logical_line: i,
                        start_col: chunk.start_index,
                        length: chunk.end_index - chunk.start_index,
                    });
                }
            }
        }

        visual_lines
    }

    /// `findVisualLineAt(visualLines, line, col)` (editor.ts:1866-1886).
    pub(crate) fn find_visual_line_at(
        &self,
        visual_lines: &[VisualLineRef],
        line: usize,
        col: usize,
    ) -> usize {
        for (i, vl) in visual_lines.iter().enumerate() {
            if vl.logical_line != line {
                continue;
            }
            let offset = col as i64 - vl.start_col as i64;
            // Cursor is in this segment if it's within range. For the last
            // segment of a logical line, cursor can be at length (end position)
            let is_last_segment_of_line = i == visual_lines.len() - 1
                || visual_lines.get(i + 1).map(|next| next.logical_line) != Some(vl.logical_line);
            if offset >= 0 && (offset < vl.length as i64 || (is_last_segment_of_line && offset == vl.length as i64))
            {
                return i;
            }
        }
        visual_lines.len() - 1
    }

    /// `findCurrentVisualLine(visualLines)` (editor.ts:1888-1892).
    pub(crate) fn find_current_visual_line(&self, visual_lines: &[VisualLineRef]) -> usize {
        self.find_visual_line_at(visual_lines, self.state.cursor_line, self.state.cursor_col)
    }

    /// `moveCursor(deltaLine, deltaCol)` (editor.ts:1894-1959).
    pub(crate) fn move_cursor(&mut self, delta_line: i64, delta_col: i64) {
        self.last_action = None;
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);

        if delta_line != 0 {
            let target_visual_line = current_visual_line as i64 + delta_line;

            if target_visual_line >= 0 && (target_visual_line as usize) < visual_lines.len() {
                self.move_to_visual_line(&visual_lines, current_visual_line, target_visual_line as usize);
            }
        }

        if delta_col != 0 {
            let current_line = self
                .state
                .lines
                .get(self.state.cursor_line)
                .cloned()
                .unwrap_or_default();

            if delta_col > 0 {
                // Moving right - move by one grapheme (handles emojis, combining characters, etc.)
                if self.state.cursor_col < utf16_len(&current_line) {
                    let after_cursor = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));
                    let graphemes = self.segment_graphemes(&after_cursor);
                    let first_grapheme_len = graphemes
                        .first()
                        .map(|g| utf16_len(&g.segment))
                        .unwrap_or(1);
                    self.set_cursor_col(self.state.cursor_col + first_grapheme_len);
                } else if self.state.cursor_line < self.state.lines.len() - 1 {
                    // Wrap to start of next logical line
                    self.state.cursor_line += 1;
                    self.set_cursor_col(0);
                } else {
                    // At end of last line - can't move, but set preferredVisualCol for up/down navigation
                    if let Some(current_vl) = visual_lines.get(current_visual_line) {
                        self.preferred_visual_col =
                            Some(self.state.cursor_col as i64 - current_vl.start_col as i64);
                    }
                }
            } else {
                // Moving left - move by one grapheme (handles emojis, combining characters, etc.)
                if self.state.cursor_col > 0 {
                    let before_cursor = slice_utf16(&current_line, 0, self.state.cursor_col);
                    let graphemes = self.segment_graphemes(&before_cursor);
                    let last_grapheme_len = graphemes
                        .last()
                        .map(|g| utf16_len(&g.segment))
                        .unwrap_or(1);
                    self.set_cursor_col(self.state.cursor_col - last_grapheme_len);
                } else if self.state.cursor_line > 0 {
                    // Wrap to end of previous logical line
                    self.state.cursor_line -= 1;
                    let prev_line = self
                        .state
                        .lines
                        .get(self.state.cursor_line)
                        .cloned()
                        .unwrap_or_default();
                    self.set_cursor_col(utf16_len(&prev_line));
                }
            }
        }

        // Keep an open autocomplete picker in sync with the new cursor
        // position (editor.ts:1950-1958).
        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
        }
    }

    /// `pageScroll(direction)` (editor.ts:1961-1971) — PageUp/PageDown.
    /// Moves cursor by the page size while keeping it in bounds.
    pub(crate) fn page_scroll(&mut self, direction: i64) {
        self.last_action = None;
        let terminal_rows = self.tui.borrow().terminal_rows();
        let page_size = ((terminal_rows as f64 * 0.3).floor() as i64).max(5);

        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);
        let target_visual_line = ((current_visual_line as i64 + direction * page_size)
            .max(0)
            .min(visual_lines.len() as i64 - 1)) as usize;

        self.move_to_visual_line(&visual_lines, current_visual_line, target_visual_line);
    }

    /// `moveWordBackwards()` (editor.ts:1973-1996) — Ctrl+Left / Alt+B.
    /// Uses `find_word_backward` with the marker-aware segmenter
    /// (editor.ts:1989).
    pub(crate) fn move_word_backwards(&mut self) {
        self.last_action = None;
        let current_line = self
            .state
            .lines
            .get(self.state.cursor_line)
            .cloned()
            .unwrap_or_default();

        // If at start of line, move to end of previous line
        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let prev_line = self
                    .state
                    .lines
                    .get(self.state.cursor_line)
                    .cloned()
                    .unwrap_or_default();
                self.set_cursor_col(utf16_len(&prev_line));
            }
            return;
        }

        let segment = |text: &str| -> Vec<WordSegment> { self.segment_words(text) };
        let col = find_word_backward(
            &current_line,
            self.state.cursor_col,
            Some(&WordNavigationOptions {
                segment: Some(&segment),
                is_atomic_segment: Some(&is_paste_marker),
            }),
        );
        self.set_cursor_col(col);
    }

    /// `moveWordForwards()` (editor.ts:2168-2186) — Ctrl+Right / Alt+F.
    pub(crate) fn move_word_forwards(&mut self) {
        self.last_action = None;
        let current_line = self
            .state
            .lines
            .get(self.state.cursor_line)
            .cloned()
            .unwrap_or_default();

        // If at end of line, move to start of next line
        if self.state.cursor_col >= utf16_len(&current_line) {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.state.cursor_line += 1;
                self.set_cursor_col(0);
            }
            return;
        }

        let segment = |text: &str| -> Vec<WordSegment> { self.segment_words(text) };
        let col = find_word_forward(
            &current_line,
            self.state.cursor_col,
            Some(&WordNavigationOptions {
                segment: Some(&segment),
                is_atomic_segment: Some(&is_paste_marker),
            }),
        );
        self.set_cursor_col(col);
    }

    /// `jumpToChar(char, direction)` (editor.ts:2138-2166) — Ctrl+] /
    /// Ctrl+Alt+] character jump. Multi-line search. Case-sensitive. Skips
    /// the current cursor position.
    pub(crate) fn jump_to_char(&mut self, char_str: &str, direction: JumpDirection) {
        self.last_action = None;
        let is_forward = matches!(direction, JumpDirection::Forward);

        let end: i64 = if is_forward {
            self.state.lines.len() as i64
        } else {
            -1
        };
        let step: i64 = if is_forward { 1 } else { -1 };

        let mut line_idx = self.state.cursor_line as i64;
        while line_idx != end {
            let line_idx_u = line_idx as usize;
            let line = self.state.lines.get(line_idx_u).cloned().unwrap_or_default();
            let is_current_line = line_idx_u == self.state.cursor_line;

            // Current line: start after/before cursor; other lines: search full line
            let search_from: Option<i64> = if is_current_line {
                Some(if is_forward {
                    self.state.cursor_col as i64 + 1
                } else {
                    self.state.cursor_col as i64 - 1
                })
            } else {
                None
            };

            // `line.indexOf(char, searchFrom)` / `line.lastIndexOf(char, searchFrom)`.
            // JS `lastIndexOf` clamps a negative `position` to 0.
            let idx = if is_forward {
                js_index_of(&line, char_str, search_from.unwrap_or(0).max(0) as usize)
            } else {
                js_last_index_of(
                    &line,
                    char_str,
                    search_from.map(|v| v.max(0) as usize),
                )
            };

            if let Some(idx) = idx {
                self.state.cursor_line = line_idx_u;
                self.set_cursor_col(idx);
                return;
            }
            line_idx += step;
        }
        // No match found - cursor stays in place
    }
}

// =============================================================================
// Tests — ports of editor.test.ts:575-705 (word navigation), 3079-3275
// (jump mode), 3311-3813 (sticky column).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::super::editor::{
        slice_utf16, Editor, EditorAction, EditorSnapshot, EditorTheme, JumpDirection,
    };
    use crate::test_support::{create_test_tui, create_test_tui_sized};
    use crate::utils::{is_whitespace_char, utf16_len};

    fn make_editor() -> Editor {
        Editor::new(create_test_tui(), EditorTheme::default())
    }

    /// `editor.setText(text)` (editor.ts:1115-1130 → setTextInternal
    /// 494-505). History is always empty in these tests, so
    /// `exitHistoryBrowsing()` is a no-op; `setCursorCol` clears the sticky
    /// column exactly like the TS path.
    fn set_text(editor: &mut Editor, text: &str) {
        let lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
        let lines = if lines.is_empty() { vec![String::new()] } else { lines };
        editor.state.lines = lines;
        editor.state.cursor_line = editor.state.lines.len() - 1;
        let col = utf16_len(editor.state.lines.last().unwrap());
        editor.set_cursor_col(col);
        editor.scroll_offset = 0;
    }

    /// `editor.render(width)` (editor.ts:521-531) — only the `lastWidth`
    /// side effect matters for cursor navigation; the padding/layout math
    /// is ported exactly.
    fn render(editor: &mut Editor, width: usize) {
        let max_padding = (((width as i64) - 1) / 2).max(0) as usize;
        let padding_x = editor.padding_x.min(max_padding);
        let content_width = ((width as i64) - (padding_x as i64) * 2).max(1) as usize;
        let layout_width = if padding_x > 0 {
            content_width
        } else {
            content_width.saturating_sub(1).max(1)
        };
        editor.last_width = layout_width;
    }

    /// `decodePrintableKey(data) ?? (charCodeAt(0) >= 32 ? data : undefined)`
    /// (editor.ts:976) for the single-character inputs used in these tests.
    fn decode_printable(data: &str) -> Option<&str> {
        let mut chars = data.chars();
        let first = chars.next()?;
        if chars.next().is_none() && (first as u32) >= 32 {
            Some(data)
        } else {
            None
        }
    }

    /// Movement-focused port of `handleInput` (editor.ts:696-998) covering
    /// exactly the key sequences used by the tests below (no history, no
    /// paste, no autocomplete). The real dispatch lives in editor_input.rs
    /// [T4-E6]; the branches here mirror it 1:1.
    fn feed(editor: &mut Editor, data: &str) {
        // Character jump mode (editor.ts:702-717)
        if editor.jump_mode.is_some() {
            // Cancel if the hotkey is pressed again
            if data == "\x1d" || data == "\x1b\x1d" {
                editor.jump_mode = None;
                return;
            }
            if let Some(printable) = decode_printable(data) {
                // Printable character - perform the jump
                let direction = editor.jump_mode.take().unwrap();
                editor.jump_to_char(printable, direction);
                return;
            }
            // Control character - cancel and fall through to normal handling
            editor.jump_mode = None;
        }

        match data {
            // Cursor movement actions (editor.ts:855-862)
            "\x01" => editor.move_to_line_start(),        // cursorLineStart
            "\x05" => editor.move_to_line_end(),          // cursorLineEnd
            "\x1b[1;5D" => editor.move_word_backwards(),  // cursorWordLeft
            "\x1b[1;5C" => editor.move_word_forwards(),   // cursorWordRight
            // Arrow key navigation (editor.ts:921-952) — history is empty
            // in these tests, so navigateHistory() is a no-op.
            "\x1b[A" => {
                if editor.is_on_first_visual_line() {
                    if editor.is_editor_empty() || editor.history_index > -1 || editor.state.cursor_col == 0 {
                        // navigateHistory(-1) — no-op (empty history)
                    } else {
                        // Already at top - jump to start of line
                        editor.move_to_line_start();
                    }
                } else {
                    editor.move_cursor(-1, 0);
                }
            }
            "\x1b[B" => {
                if editor.is_on_last_visual_line() {
                    if editor.history_index > -1 {
                        // navigateHistory(1) — no-op (empty history)
                    } else {
                        // Already at bottom - jump to end of line
                        editor.move_to_line_end();
                    }
                } else {
                    editor.move_cursor(1, 0);
                }
            }
            "\x1b[C" => editor.move_cursor(0, 1), // cursorRight
            "\x1b[D" => editor.move_cursor(0, -1), // cursorLeft
            // deleteCharBackward (editor.ts:817-819)
            "\x7f" => backspace(editor),
            // undo (editor.ts:737-740)
            "\x1b[45;5u" => undo(editor),
            // Character jump mode triggers (editor.ts:960-968)
            "\x1d" => editor.jump_mode = Some(JumpDirection::Forward),
            "\x1b\x1d" => editor.jump_mode = Some(JumpDirection::Backward),
            // Escape alone does nothing in the editor
            "\x1b" => {}
            _ => {
                if let Some(printable) = decode_printable(data) {
                    insert_character(editor, printable);
                }
            }
        }
    }

    /// `insertCharacter(char)` (editor.ts:1201-1230) — undo coalescing plus
    /// the insert. Autocomplete triggering is omitted (no provider in these
    /// tests).
    fn insert_character(editor: &mut Editor, ch: &str) {
        // Undo coalescing (editor.ts:1208-1216)
        let ch_first = ch.chars().next().unwrap_or(' ');
        if is_whitespace_char(ch_first) || editor.last_action != Some(EditorAction::TypeWord) {
            push_undo_snapshot(editor);
        }
        editor.last_action = Some(EditorAction::TypeWord);

        let line = editor.state.lines[editor.state.cursor_line].clone();
        let before = slice_utf16(&line, 0, editor.state.cursor_col);
        let after = slice_utf16(&line, editor.state.cursor_col, utf16_len(&line));
        editor.state.lines[editor.state.cursor_line] = format!("{before}{ch}{after}");
        editor.set_cursor_col(editor.state.cursor_col + utf16_len(ch));
    }

    /// `handleBackspace()` (editor.ts:1380-1405) — the plain grapheme
    /// deletion path (no paste markers in these tests; the line-merge
    /// branch at col 0 is not exercised).
    fn backspace(editor: &mut Editor) {
        editor.last_action = None;
        if editor.state.cursor_col > 0 {
            push_undo_snapshot(editor);
            let line = editor.state.lines[editor.state.cursor_line].clone();
            let before_cursor = slice_utf16(&line, 0, editor.state.cursor_col);
            let graphemes = editor.segment_graphemes(&before_cursor);
            let grapheme_length = graphemes.last().map(|g| utf16_len(&g.segment)).unwrap_or(1);
            let before = slice_utf16(&line, 0, editor.state.cursor_col - grapheme_length);
            let after = slice_utf16(&line, editor.state.cursor_col, utf16_len(&line));
            editor.state.lines[editor.state.cursor_line] = format!("{before}{after}");
            editor.set_cursor_col(editor.state.cursor_col - grapheme_length);
        }
    }

    /// `pushUndoSnapshot()` (editor.ts:2078-2080).
    fn push_undo_snapshot(editor: &mut Editor) {
        editor.undo_stack.push(&EditorSnapshot {
            state: editor.state.clone(),
            pastes: editor.pastes.clone(),
            paste_counter: editor.paste_counter,
        });
    }

    /// `undo()` (editor.ts:2120-2136) — history browsing is never active in
    /// these tests.
    fn undo(editor: &mut Editor) {
        if let Some(snapshot) = editor.undo_stack.pop() {
            editor.state = snapshot.state;
            editor.pastes = snapshot.pastes;
            editor.paste_counter = snapshot.paste_counter;
            editor.last_action = None;
            editor.preferred_visual_col = None;
        }
    }

    // -------------------------------------------------------------------------
    // Word navigation (editor.test.ts:575-705)
    // -------------------------------------------------------------------------

    #[test]
    fn navigates_words_correctly_with_ctrl_left_right() {
        let mut editor = make_editor();

        set_text(&mut editor, "foo bar... baz");
        // Cursor at end

        // Move left over baz
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 11)); // after '...'

        // Move left over punctuation
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 7)); // after 'bar'

        // Move left over bar
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 4)); // after 'foo '

        // Move right over bar
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 7)); // at end of 'bar'

        // Move right over punctuation run
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 10)); // after '...'

        // Move right skips space and lands after baz
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 14)); // end of line

        // Test forward from start with leading whitespace
        set_text(&mut editor, "   foo bar");
        feed(&mut editor, "\x01"); // Ctrl+A to go to start
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 6)); // after 'foo'

        // ASCII punctuation inside Intl word-like segments preserves old boundaries
        set_text(&mut editor, "foo.bar baz");
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left over baz
        assert_eq!(editor.get_cursor(), (0, 8));
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left over bar
        assert_eq!(editor.get_cursor(), (0, 4));
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left over .
        assert_eq!(editor.get_cursor(), (0, 3));

        feed(&mut editor, "\x01"); // Ctrl+A
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right over foo
        assert_eq!(editor.get_cursor(), (0, 3));
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right over .
        assert_eq!(editor.get_cursor(), (0, 4));
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right over bar
        assert_eq!(editor.get_cursor(), (0, 7));
    }

    #[test]
    fn stops_at_fullwidth_chinese_punctuation_issue_4972() {
        let mut editor = make_editor();

        // 你好，世界 = 你好(0-2) ，(2-3) 世界(3-5)
        set_text(&mut editor, "你好，世界");
        // Cursor at end (col 5)

        // Move left over 世界
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 3)); // after ，

        // Move left over ，
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 2)); // after 你好

        // Move left over 你好
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 0)); // start

        // Move right over 你好
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 2)); // after 你好

        // Move right over ，
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 3)); // after ，

        // Move right over 世界
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 5)); // end
    }

    #[test]
    fn handles_mixed_cjk_and_ascii_word_movement() {
        let mut editor = make_editor();

        // "hello你好，world世界" = hello(0-5) 你好(5-7) ，(7-8) world(8-13) 世界(13-15)
        set_text(&mut editor, "hello你好，world世界");
        // Cursor at end (col 15)

        // Move left over 世界
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 13)); // after 'world'

        // Move left over world
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 8)); // after ，

        // Move left over ，
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 7)); // after 你好

        // Move left over 你好
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 5)); // after 'hello'

        // Move left over hello
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 0)); // start

        // Forward from start
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 5)); // after 'hello'

        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 7)); // after 你好

        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 8)); // after ，

        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 13)); // after 'world'

        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 15)); // end
    }

    // -------------------------------------------------------------------------
    // Jump mode (editor.test.ts:3079-3275)
    // -------------------------------------------------------------------------

    #[test]
    fn jumps_forward_to_first_occurrence_of_character_on_same_line() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        assert_eq!(editor.get_cursor(), (0, 0));

        feed(&mut editor, "\x1d"); // Ctrl+] (legacy sequence for ctrl+])
        feed(&mut editor, "o"); // Jump to first 'o'

        assert_eq!(editor.get_cursor(), (0, 4)); // 'o' in "hello"
    }

    #[test]
    fn jumps_forward_to_next_occurrence_after_cursor() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        // Move cursor to the 'o' in "hello" (col 4)
        for _ in 0..4 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (0, 4));

        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "o"); // Jump to next 'o' (in "world")

        assert_eq!(editor.get_cursor(), (0, 7)); // 'o' in "world"
    }

    #[test]
    fn jumps_forward_across_multiple_lines() {
        let mut editor = make_editor();

        set_text(&mut editor, "abc\ndef\nghi");
        // Cursor is at end (line 2, col 3). Move to line 0 via up arrows, then Ctrl+A
        feed(&mut editor, "\x1b[A"); // Up
        feed(&mut editor, "\x1b[A"); // Up - now on line 0
        feed(&mut editor, "\x01"); // Ctrl+A - go to start of line
        assert_eq!(editor.get_cursor(), (0, 0));

        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "g"); // Jump to 'g' on line 3

        assert_eq!(editor.get_cursor(), (2, 0));
    }

    #[test]
    fn jumps_backward_to_first_occurrence_before_cursor_on_same_line() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        // Cursor at end (col 11)
        assert_eq!(editor.get_cursor(), (0, 11));

        feed(&mut editor, "\x1b\x1d"); // Ctrl+Alt+] (ESC followed by Ctrl+])
        feed(&mut editor, "o"); // Jump to last 'o' before cursor

        assert_eq!(editor.get_cursor(), (0, 7)); // 'o' in "world"
    }

    #[test]
    fn jumps_backward_across_multiple_lines() {
        let mut editor = make_editor();

        set_text(&mut editor, "abc\ndef\nghi");
        // Cursor at end of line 3
        assert_eq!(editor.get_cursor(), (2, 3));

        feed(&mut editor, "\x1b\x1d"); // Ctrl+Alt+]
        feed(&mut editor, "a"); // Jump to 'a' on line 1

        assert_eq!(editor.get_cursor(), (0, 0));
    }

    #[test]
    fn does_nothing_when_character_is_not_found_forward() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        assert_eq!(editor.get_cursor(), (0, 0));

        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "z"); // 'z' doesn't exist

        assert_eq!(editor.get_cursor(), (0, 0)); // Cursor unchanged
    }

    #[test]
    fn does_nothing_when_character_is_not_found_backward() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        // Cursor at end
        assert_eq!(editor.get_cursor(), (0, 11));

        feed(&mut editor, "\x1b\x1d"); // Ctrl+Alt+]
        feed(&mut editor, "z"); // 'z' doesn't exist

        assert_eq!(editor.get_cursor(), (0, 11)); // Cursor unchanged
    }

    #[test]
    fn jump_mode_is_case_sensitive() {
        let mut editor = make_editor();

        set_text(&mut editor, "Hello World");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        assert_eq!(editor.get_cursor(), (0, 0));

        // Search for lowercase 'h' - should not find it (only 'H' exists)
        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "h");

        assert_eq!(editor.get_cursor(), (0, 0)); // Cursor unchanged

        // Search for uppercase 'W' - should find it
        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "W");

        assert_eq!(editor.get_cursor(), (0, 6)); // 'W' in "World"
    }

    #[test]
    fn cancels_jump_mode_when_ctrl_right_bracket_is_pressed_again() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        assert_eq!(editor.get_cursor(), (0, 0));

        feed(&mut editor, "\x1d"); // Ctrl+] - enter jump mode
        feed(&mut editor, "\x1d"); // Ctrl+] again - cancel

        // Type 'o' normally - should insert, not jump
        feed(&mut editor, "o");
        assert_eq!(editor.get_text(), "ohello world");
    }

    #[test]
    fn cancels_jump_mode_on_escape_and_processes_the_escape() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        assert_eq!(editor.get_cursor(), (0, 0));

        feed(&mut editor, "\x1d"); // Ctrl+] - enter jump mode
        feed(&mut editor, "\x1b"); // Escape - cancel jump mode

        // Cursor should be unchanged (Escape itself doesn't move cursor in editor)
        assert_eq!(editor.get_cursor(), (0, 0));

        // Type 'o' normally - should insert, not jump
        feed(&mut editor, "o");
        assert_eq!(editor.get_text(), "ohello world");
    }

    #[test]
    fn cancels_backward_jump_mode_when_ctrl_alt_right_bracket_is_pressed_again() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        // Cursor at end
        assert_eq!(editor.get_cursor(), (0, 11));

        feed(&mut editor, "\x1b\x1d"); // Ctrl+Alt+] - enter backward jump mode
        feed(&mut editor, "\x1b\x1d"); // Ctrl+Alt+] again - cancel

        // Type 'o' normally - should insert, not jump
        feed(&mut editor, "o");
        assert_eq!(editor.get_text(), "hello worldo");
    }

    #[test]
    fn jump_mode_searches_for_special_characters() {
        let mut editor = make_editor();

        set_text(&mut editor, "foo(bar) = baz;");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start
        assert_eq!(editor.get_cursor(), (0, 0));

        // Jump to '('
        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "(");

        assert_eq!(editor.get_cursor(), (0, 3));

        // Jump to '='
        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "=");

        assert_eq!(editor.get_cursor(), (0, 9));
    }

    #[test]
    fn jump_mode_handles_empty_text_gracefully() {
        let mut editor = make_editor();

        set_text(&mut editor, "");
        assert_eq!(editor.get_cursor(), (0, 0));

        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "x");

        assert_eq!(editor.get_cursor(), (0, 0)); // Cursor unchanged
    }

    #[test]
    fn jump_mode_resets_last_action_when_jumping() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world");
        feed(&mut editor, "\x01"); // Ctrl+A - go to start

        // Type to set lastAction to "type-word"
        feed(&mut editor, "x");
        assert_eq!(editor.get_text(), "xhello world");

        // Jump forward
        feed(&mut editor, "\x1d"); // Ctrl+]
        feed(&mut editor, "o");

        // Type more - should start a new undo unit (lastAction was reset)
        feed(&mut editor, "Y");
        assert_eq!(editor.get_text(), "xhellYo world");

        // Undo should only undo "Y", not "x" as well
        feed(&mut editor, "\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "xhello world");
    }

    // -------------------------------------------------------------------------
    // Sticky column (editor.test.ts:3311-3813)
    // -------------------------------------------------------------------------

    /// Helper: position cursor at a specific line and column
    /// (editor.test.ts:3313-3320).
    fn position_cursor(editor: &mut Editor, line: usize, col: usize) {
        // Go to line 0 first
        for _ in 0..20 {
            feed(editor, "\x1b[A");
        }
        // Go to target line
        for _ in 0..line {
            feed(editor, "\x1b[B");
        }
        // Go to target col
        feed(editor, "\x01"); // Ctrl+A
        for _ in 0..col {
            feed(editor, "\x1b[C");
        }
    }

    #[test]
    fn preserves_target_column_when_moving_up_through_a_shorter_line() {
        let mut editor = make_editor();

        // Line 0: "2222222222x222" (x at col 10)
        // Line 1: "" (empty)
        // Line 2: "1111111111_111111111111" (_ at col 10)
        set_text(&mut editor, "2222222222x222\n\n1111111111_111111111111");

        // Position cursor on _ (line 2, col 10)
        assert_eq!(editor.get_cursor(), (2, 23)); // At end
        feed(&mut editor, "\x01"); // Ctrl+A - go to start of line
        for _ in 0..10 {
            feed(&mut editor, "\x1b[C"); // Move right to col 10
        }
        assert_eq!(editor.get_cursor(), (2, 10));

        // Press Up - should move to empty line (col clamped to 0)
        feed(&mut editor, "\x1b[A"); // Up arrow
        assert_eq!(editor.get_cursor(), (1, 0));

        // Press Up again - should move to line 0 at col 10 (on 'x')
        feed(&mut editor, "\x1b[A"); // Up arrow
        assert_eq!(editor.get_cursor(), (0, 10));
    }

    #[test]
    fn preserves_target_column_when_moving_down_through_a_shorter_line() {
        let mut editor = make_editor();

        set_text(&mut editor, "1111111111_111\n\n2222222222x222222222222");

        // Position cursor on _ (line 0, col 10)
        feed(&mut editor, "\x1b[A"); // Up to line 1
        feed(&mut editor, "\x1b[A"); // Up to line 0
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..10 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (0, 10));

        // Press Down - should move to empty line (col clamped to 0)
        feed(&mut editor, "\x1b[B"); // Down arrow
        assert_eq!(editor.get_cursor(), (1, 0));

        // Press Down again - should move to line 2 at col 10 (on 'x')
        feed(&mut editor, "\x1b[B"); // Down arrow
        assert_eq!(editor.get_cursor(), (2, 10));
    }

    #[test]
    fn resets_sticky_column_on_horizontal_movement_left_arrow() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Start at line 2, col 5
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..5 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (2, 5));

        // Move up through empty line
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 5 (sticky)
        assert_eq!(editor.get_cursor(), (0, 5));

        // Move left - resets sticky column
        feed(&mut editor, "\x1b[D"); // Left
        assert_eq!(editor.get_cursor(), (0, 4));

        // Move down twice
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 4 (new sticky from col 4)
        assert_eq!(editor.get_cursor(), (2, 4));
    }

    #[test]
    fn resets_sticky_column_on_horizontal_movement_right_arrow() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Start at line 0, col 5
        feed(&mut editor, "\x1b[A"); // Up to line 1
        feed(&mut editor, "\x1b[A"); // Up to line 0
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..5 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (0, 5));

        // Move down through empty line
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 5 (sticky)
        assert_eq!(editor.get_cursor(), (2, 5));

        // Move right - resets sticky column
        feed(&mut editor, "\x1b[C"); // Right
        assert_eq!(editor.get_cursor(), (2, 6));

        // Move up twice
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 6 (new sticky from col 6)
        assert_eq!(editor.get_cursor(), (0, 6));
    }

    #[test]
    fn resets_sticky_column_on_typing() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Start at line 2, col 8
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..8 {
            feed(&mut editor, "\x1b[C");
        }

        // Move up through empty line
        feed(&mut editor, "\x1b[A"); // Up
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 8
        assert_eq!(editor.get_cursor(), (0, 8));

        // Type a character - resets sticky column
        feed(&mut editor, "X");
        assert_eq!(editor.get_cursor(), (0, 9));

        // Move down twice
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 9 (new sticky from col 9)
        assert_eq!(editor.get_cursor(), (2, 9));
    }

    #[test]
    fn resets_sticky_column_on_backspace() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Start at line 2, col 8
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..8 {
            feed(&mut editor, "\x1b[C");
        }

        // Move up through empty line
        feed(&mut editor, "\x1b[A"); // Up
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 8
        assert_eq!(editor.get_cursor(), (0, 8));

        // Backspace - resets sticky column
        feed(&mut editor, "\x7f"); // Backspace
        assert_eq!(editor.get_cursor(), (0, 7));

        // Move down twice
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 7 (new sticky from col 7)
        assert_eq!(editor.get_cursor(), (2, 7));
    }

    #[test]
    fn resets_sticky_column_on_ctrl_a_move_to_line_start() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Start at line 2, col 8
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..8 {
            feed(&mut editor, "\x1b[C");
        }

        // Move up - establishes sticky col 8
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0

        // Ctrl+A - resets sticky column to 0
        feed(&mut editor, "\x01"); // Ctrl+A
        assert_eq!(editor.get_cursor(), (1, 0));

        // Move up
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 0 (new sticky from col 0)
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    #[test]
    fn resets_sticky_column_on_ctrl_e_move_to_line_end() {
        let mut editor = make_editor();

        set_text(&mut editor, "12345\n\n1234567890");

        // Start at line 2, col 3
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..3 {
            feed(&mut editor, "\x1b[C");
        }

        // Move up through empty line - establishes sticky col 3
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 3
        assert_eq!(editor.get_cursor(), (0, 3));

        // Ctrl+E - resets sticky column to end
        feed(&mut editor, "\x05"); // Ctrl+E
        assert_eq!(editor.get_cursor(), (0, 5));

        // Move down twice
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 5 (new sticky from col 5)
        assert_eq!(editor.get_cursor(), (2, 5));
    }

    #[test]
    fn resets_sticky_column_on_word_movement_ctrl_left() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world\n\nhello world");

        // Start at end of line 2 (col 11)
        assert_eq!(editor.get_cursor(), (2, 11));

        // Move up through empty line - establishes sticky col 11
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 11
        assert_eq!(editor.get_cursor(), (0, 11));

        // Ctrl+Left - word movement resets sticky column
        feed(&mut editor, "\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 6)); // Before "world"

        // Move down twice
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 6 (new sticky from col 6)
        assert_eq!(editor.get_cursor(), (2, 6));
    }

    #[test]
    fn resets_sticky_column_on_word_movement_ctrl_right() {
        let mut editor = make_editor();

        set_text(&mut editor, "hello world\n\nhello world");

        // Start at line 0, col 0
        feed(&mut editor, "\x1b[A"); // Up
        feed(&mut editor, "\x1b[A"); // Up
        feed(&mut editor, "\x01"); // Ctrl+A
        assert_eq!(editor.get_cursor(), (0, 0));

        // Move down through empty line - establishes sticky col 0
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 0
        assert_eq!(editor.get_cursor(), (2, 0));

        // Ctrl+Right - word movement resets sticky column
        feed(&mut editor, "\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (2, 5)); // After "hello"

        // Move up twice
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 5 (new sticky from col 5)
        assert_eq!(editor.get_cursor(), (0, 5));
    }

    #[test]
    fn resets_sticky_column_on_undo() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Go to line 0, col 8
        feed(&mut editor, "\x1b[A"); // Up to line 1
        feed(&mut editor, "\x1b[A"); // Up to line 0
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..8 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (0, 8));

        // Move down through empty line - establishes sticky col 8
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 8 (sticky)
        assert_eq!(editor.get_cursor(), (2, 8));

        // Type something to create undo state - this clears sticky and sets col to 9
        feed(&mut editor, "X");
        assert_eq!(editor.get_text(), "1234567890\n\n12345678X90");
        assert_eq!(editor.get_cursor(), (2, 9));

        // Move up - establishes new sticky col 9
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 9
        assert_eq!(editor.get_cursor(), (0, 9));

        // Undo - resets sticky column and restores cursor to line 2, col 8
        feed(&mut editor, "\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "1234567890\n\n1234567890");
        assert_eq!(editor.get_cursor(), (2, 8));

        // Move up - should capture new sticky from restored col 8, not old col 9
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 8 (new sticky from restored position)
        assert_eq!(editor.get_cursor(), (0, 8));
    }

    #[test]
    fn handles_multiple_consecutive_up_down_movements() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\nab\ncd\nef\n1234567890");

        // Start at line 4, col 7
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..7 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (4, 7));

        // Move up multiple times through short lines
        feed(&mut editor, "\x1b[A"); // Up - line 3, col 2 (clamped)
        feed(&mut editor, "\x1b[A"); // Up - line 2, col 2 (clamped)
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 2 (clamped)
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 7 (restored)
        assert_eq!(editor.get_cursor(), (0, 7));

        // Move down multiple times - sticky should still be 7
        feed(&mut editor, "\x1b[B"); // Down - line 1, col 2
        feed(&mut editor, "\x1b[B"); // Down - line 2, col 2
        feed(&mut editor, "\x1b[B"); // Down - line 3, col 2
        feed(&mut editor, "\x1b[B"); // Down - line 4, col 7 (restored)
        assert_eq!(editor.get_cursor(), (4, 7));
    }

    #[test]
    fn moves_correctly_through_wrapped_visual_lines_without_getting_stuck() {
        let tui = create_test_tui_sized(15, 24); // Narrow terminal
        let mut editor = Editor::new(tui, EditorTheme::default());

        // Line 0: short
        // Line 1: 30 chars = wraps to 3 visual lines at width 10 (after padding)
        set_text(&mut editor, "short\n123456789012345678901234567890");
        render(&mut editor, 15); // This gives 14 layout width

        // Position at end of line 1 (col 30)
        assert_eq!(editor.get_cursor(), (1, 30));

        // Move up repeatedly - should traverse all visual lines of the wrapped text
        // and eventually reach line 0
        feed(&mut editor, "\x1b[A"); // Up - to previous visual line within line 1
        assert_eq!(editor.get_cursor().0, 1);

        feed(&mut editor, "\x1b[A"); // Up - another visual line
        assert_eq!(editor.get_cursor().0, 1);

        feed(&mut editor, "\x1b[A"); // Up - should reach line 0
        assert_eq!(editor.get_cursor().0, 0);
    }

    #[test]
    fn handles_set_text_resetting_sticky_column() {
        let mut editor = make_editor();

        set_text(&mut editor, "1234567890\n\n1234567890");

        // Establish sticky column
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..8 {
            feed(&mut editor, "\x1b[C");
        }
        feed(&mut editor, "\x1b[A"); // Up

        // setText should reset sticky column
        set_text(&mut editor, "abcdefghij\n\nabcdefghij");
        assert_eq!(editor.get_cursor(), (2, 10)); // At end

        // Move up - should capture new sticky from current position (10)
        feed(&mut editor, "\x1b[A"); // Up - line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 10
        assert_eq!(editor.get_cursor(), (0, 10));
    }

    #[test]
    fn sets_preferred_visual_col_when_pressing_right_at_end_of_prompt_last_line() {
        let mut editor = make_editor();

        // Line 0: 20 chars with 'x' at col 10
        // Line 1: empty
        // Line 2: 10 chars ending with '_'
        set_text(&mut editor, "111111111x1111111111\n\n333333333_");

        // Go to line 0, press Ctrl+E (end of line) - col 20
        feed(&mut editor, "\x1b[A"); // Up to line 1
        feed(&mut editor, "\x1b[A"); // Up to line 0
        feed(&mut editor, "\x05"); // Ctrl+E - move to end of line
        assert_eq!(editor.get_cursor(), (0, 20));

        // Move down to line 2 - cursor clamped to col 10 (end of line)
        feed(&mut editor, "\x1b[B"); // Down to line 1, col 0
        feed(&mut editor, "\x1b[B"); // Down to line 2, col 10 (clamped)
        assert_eq!(editor.get_cursor(), (2, 10));

        // Press Right at end of prompt - nothing visible happens, but sets preferredVisualCol to 10
        feed(&mut editor, "\x1b[C"); // Right - can't move, but sets preferredVisualCol
        assert_eq!(editor.get_cursor(), (2, 10)); // Still at same position

        // Move up twice to line 0 - should use preferredVisualCol (10) to land on 'x'
        feed(&mut editor, "\x1b[A"); // Up to line 1, col 0
        feed(&mut editor, "\x1b[A"); // Up to line 0, col 10 (on 'x')
        assert_eq!(editor.get_cursor(), (0, 10));
    }

    #[test]
    fn handles_editor_resizes_when_preferred_visual_col_is_on_the_same_line() {
        // Create editor with wider terminal
        let tui = create_test_tui_sized(80, 24);
        let mut editor = Editor::new(tui, EditorTheme::default());

        set_text(&mut editor, "12345678901234567890\n\n12345678901234567890");

        // Start at line 2, col 15
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..15 {
            feed(&mut editor, "\x1b[C");
        }

        // Move up through empty line - establishes sticky col 15
        feed(&mut editor, "\x1b[A"); // Up
        feed(&mut editor, "\x1b[A"); // Up - line 0, col 15
        assert_eq!(editor.get_cursor(), (0, 15));

        // Render with narrower width to simulate resize
        render(&mut editor, 12); // Width 12

        // Move down - sticky should be clamped to new width
        feed(&mut editor, "\x1b[B"); // Down - line 1
        feed(&mut editor, "\x1b[B"); // Down - line 2, col should be clamped
        assert_eq!(editor.get_cursor().1, 4);
    }

    #[test]
    fn handles_editor_resizes_when_preferred_visual_col_is_on_a_different_line() {
        let tui = create_test_tui_sized(80, 24);
        let mut editor = Editor::new(tui, EditorTheme::default());

        // Create a line that wraps into multiple visual lines at width 10
        // "12345678901234567890" = 20 chars, wraps to 2 visual lines at width 10
        set_text(&mut editor, "short\n12345678901234567890");

        // Go to line 1, col 15
        feed(&mut editor, "\x01"); // Ctrl+A
        for _ in 0..15 {
            feed(&mut editor, "\x1b[C");
        }
        assert_eq!(editor.get_cursor(), (1, 15));

        // Move up to establish sticky col 15
        feed(&mut editor, "\x1b[A"); // Up to line 0
        // Line 0 has only 5 chars, so cursor at col 5
        assert_eq!(editor.get_cursor(), (0, 5));

        // Narrow the editor
        render(&mut editor, 10);

        // Move down - preferredVisualCol was 15, but width is 10
        // Should land on line 1, clamped to width (visual col 9, which is logical col 9)
        feed(&mut editor, "\x1b[B"); // Down to line 1
        assert_eq!(editor.get_cursor(), (1, 8));

        // Move up
        feed(&mut editor, "\x1b[A"); // Up - should go to line 0
        assert_eq!(editor.get_cursor(), (0, 5)); // Line 0 only has 5 chars

        // Restore the original width
        render(&mut editor, 80);

        // Move down - preferredVisualCol was kept at 15
        feed(&mut editor, "\x1b[B"); // Down to line 1
        assert_eq!(editor.get_cursor(), (1, 15));
    }

    #[test]
    fn rewrapped_lines_target_fits_current_visual_column() {
        let tui = create_test_tui_sized(80, 24);
        let mut editor = Editor::new(tui, EditorTheme::default());
        set_text(&mut editor, "abcdefghijklmnopqr\n123456789012345678");

        position_cursor(&mut editor, 0, 18);
        assert_eq!(editor.get_cursor(), (0, 18));

        // Narrow to width 10 (layoutWidth = 9).
        // Line 0 last segment has visual col max 9, line 1 first segment max 8
        render(&mut editor, 10);

        // Move down: cursor clamps to 8
        feed(&mut editor, "\x1b[B");
        assert_eq!(editor.get_cursor(), (1, 8));

        // Widen back. Move up, the current visual col wins
        render(&mut editor, 80);
        feed(&mut editor, "\x1b[A");
        assert_eq!(editor.get_cursor(), (0, 8));

        // Preferred was cleared by the rewrapped branch
        feed(&mut editor, "\x1b[B");
        assert_eq!(editor.get_cursor(), (1, 8));
    }

    #[test]
    fn rewrapped_lines_target_shorter_than_current_visual_column() {
        let tui = create_test_tui_sized(80, 24);
        let mut editor = Editor::new(tui, EditorTheme::default());
        set_text(&mut editor, "abcdefghijklmnopqr\n123456789012345678\nab");

        position_cursor(&mut editor, 0, 18);
        assert_eq!(editor.get_cursor(), (0, 18));

        // Narrow to width 10 (layoutWidth = 9). Moving down clamps to col 8
        render(&mut editor, 10);
        feed(&mut editor, "\x1b[B");
        assert_eq!(editor.get_cursor(), (1, 8));

        // Widen the editor
        render(&mut editor, 80);

        // Move down to short line "ab".
        // preferredVisualCol is replaced with current visual col (8), cursor clamps to 2
        feed(&mut editor, "\x1b[B");
        assert_eq!(editor.get_cursor(), (2, 2));

        // Moving up restores to preferred col 8
        feed(&mut editor, "\x1b[A");
        assert_eq!(editor.get_cursor(), (1, 8));
    }
}
