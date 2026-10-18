//! History navigation, kill ring / yank, and undo (editor.ts:428-438,
//! 456-505, 1998-2136).
//!
//! **Owner: T4 delegate E4.** Ported from the TS ground truth
//! `/tmp/pi-repo/packages/tui/src/components/editor.ts`; TS line references
//! in each doc comment. Tests at the end of this file are ports of the
//! history / kill-ring / undo blocks of `test/editor.test.ts` plus the
//! whole of `test/editor-history-keybindings.test.ts`.

use super::editor::{slice_utf16, Editor, EditorAction, EditorSnapshot};
use crate::utils::utf16_len;

impl Editor {
    /// `addToHistory(text)` (editor.ts:428-438): trim, skip empty and
    /// consecutive duplicates, unshift, cap at 100.
    pub fn add_to_history(&mut self, text: &str) {
        // const trimmed = text.trim();
        // if (!trimmed) return;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // Don't add consecutive duplicates
        // if (this.history.length > 0 && this.history[0] === trimmed) return;
        if !self.history.is_empty() && self.history[0] == trimmed {
            return;
        }
        // this.history.unshift(trimmed);
        self.history.insert(0, trimmed.to_string());
        // Limit history size
        // if (this.history.length > 100) { this.history.pop(); }
        if self.history.len() > 100 {
            self.history.pop();
        }
    }

    /// `navigateHistory(direction)` (editor.ts:456-486) — Up = -1, Down = 1.
    pub(crate) fn navigate_history(&mut self, direction: i64) {
        // this.lastAction = null;
        self.last_action = None;
        // if (this.history.length === 0) return;
        if self.history.is_empty() {
            return;
        }

        // const newIndex = this.historyIndex - direction; // Up(-1) increases index, Down(1) decreases
        let new_index = self.history_index - direction;
        // if (newIndex < -1 || newIndex >= this.history.length) return;
        if new_index < -1 || new_index >= self.history.len() as i64 {
            return;
        }

        // Capture state when first entering history browsing mode
        // if (this.historyIndex === -1 && newIndex >= 0) {
        //   this.pushUndoSnapshot();
        //   this.historyDraft = structuredClone(this.state);
        // }
        if self.history_index == -1 && new_index >= 0 {
            self.push_undo_snapshot();
            self.history_draft = Some(self.state.clone());
        }

        // this.historyIndex = newIndex;
        self.history_index = new_index;

        if self.history_index == -1 {
            // const draft = this.historyDraft;
            // this.historyDraft = null;
            let draft = self.history_draft.take();
            if let Some(draft) = draft {
                // this.state = draft;
                self.state = draft;
                // this.preferredVisualCol = null;
                self.preferred_visual_col = None;
                // this.snappedFromCursorCol = null;
                self.snapped_from_cursor_col = None;
                // this.scrollOffset = 0;
                self.scroll_offset = 0;
                // if (this.onChange) this.onChange(this.getText());
                let text = self.get_text();
                if let Some(on_change) = self.on_change.as_mut() {
                    on_change(&text);
                }
            } else {
                // this.setTextInternal("");
                self.set_text_internal("", false);
            }
        } else {
            // this.setTextInternal(this.history[this.historyIndex] || "",
            //     direction === -1 ? "start" : "end");
            let text = self
                .history
                .get(self.history_index as usize)
                .cloned()
                .unwrap_or_default();
            self.set_text_internal(&text, direction == -1);
        }
    }

    /// `exitHistoryBrowsing()` (editor.ts:488-492).
    pub(crate) fn exit_history_browsing(&mut self) {
        // this.historyIndex = -1;
        self.history_index = -1;
        // this.historyDraft = null;
        self.history_draft = None;
    }

    /// `yank()` (editor.ts:1998-2011) — Ctrl+Y.
    pub(crate) fn yank(&mut self) {
        // if (this.killRing.length === 0) return;
        if self.kill_ring.is_empty() {
            return;
        }

        // this.pushUndoSnapshot();
        self.push_undo_snapshot();

        // const text = this.killRing.peek()!;
        // this.insertYankedText(text);
        let text = self.kill_ring.peek().unwrap_or("").to_string();
        self.insert_yanked_text(&text);

        // this.lastAction = "yank";
        self.last_action = Some(EditorAction::Yank);
    }

    /// `yankPop()` (editor.ts:2013-2033) — Alt+Y.
    pub(crate) fn yank_pop(&mut self) {
        // Only works if we just yanked and have more than one entry
        // if (this.lastAction !== "yank" || this.killRing.length <= 1) return;
        if self.last_action != Some(EditorAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }

        // this.pushUndoSnapshot();
        self.push_undo_snapshot();

        // Delete the previously yanked text (still at end of ring before rotation)
        // this.deleteYankedText();
        self.delete_yanked_text();

        // Rotate the ring: move end to front
        // this.killRing.rotate();
        self.kill_ring.rotate();

        // Insert the new most recent entry (now at end after rotation)
        // const text = this.killRing.peek()!;
        // this.insertYankedText(text);
        let text = self.kill_ring.peek().unwrap_or("").to_string();
        self.insert_yanked_text(&text);

        // this.lastAction = "yank";
        self.last_action = Some(EditorAction::Yank);
    }

    /// `insertYankedText(text)` (editor.ts:2035-2076).
    pub(crate) fn insert_yanked_text(&mut self, text: &str) {
        // this.exitHistoryBrowsing();
        self.exit_history_browsing();
        // const lines = text.split("\n");
        let lines: Vec<&str> = text.split('\n').collect();

        if lines.len() == 1 {
            // Single line - insert at cursor
            // const currentLine = this.state.lines[this.state.cursorLine] || "";
            let current_line = self
                .state
                .lines
                .get(self.state.cursor_line)
                .cloned()
                .unwrap_or_default();
            // const before = currentLine.slice(0, this.state.cursorCol);
            let before = slice_utf16(&current_line, 0, self.state.cursor_col);
            // const after = currentLine.slice(this.state.cursorCol);
            let after = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));
            // this.state.lines[this.state.cursorLine] = before + text + after;
            self.state.lines[self.state.cursor_line] = format!("{before}{text}{after}");
            // this.setCursorCol(this.state.cursorCol + text.length);
            let col = self.state.cursor_col + utf16_len(text);
            self.set_cursor_col(col);
        } else {
            // Multi-line insert
            // const currentLine = this.state.lines[this.state.cursorLine] || "";
            let current_line = self
                .state
                .lines
                .get(self.state.cursor_line)
                .cloned()
                .unwrap_or_default();
            // const before = currentLine.slice(0, this.state.cursorCol);
            let before = slice_utf16(&current_line, 0, self.state.cursor_col);
            // const after = currentLine.slice(this.state.cursorCol);
            let after = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));

            // First line merges with text before cursor
            // this.state.lines[this.state.cursorLine] = before + (lines[0] || "");
            self.state.lines[self.state.cursor_line] = format!("{before}{}", lines[0]);

            // Insert middle lines
            // for (let i = 1; i < lines.length - 1; i++) {
            //   this.state.lines.splice(this.state.cursorLine + i, 0, lines[i] || "");
            // }
            #[allow(clippy::needless_range_loop)] // index drives the splice position (cursor_line + i)
            for i in 1..lines.len() - 1 {
                self.state
                    .lines
                    .insert(self.state.cursor_line + i, lines[i].to_string());
            }

            // Last line merges with text after cursor
            // const lastLineIndex = this.state.cursorLine + lines.length - 1;
            let last_line_index = self.state.cursor_line + lines.len() - 1;
            // this.state.lines.splice(lastLineIndex, 0, (lines[lines.length - 1] || "") + after);
            self.state
                .lines
                .insert(last_line_index, format!("{}{after}", lines[lines.len() - 1]));

            // Update cursor position
            // this.state.cursorLine = lastLineIndex;
            self.state.cursor_line = last_line_index;
            // this.setCursorCol((lines[lines.length - 1] || "").length);
            let col = utf16_len(lines[lines.len() - 1]);
            self.set_cursor_col(col);
        }

        // if (this.onChange) { this.onChange(this.getText()); }
        let new_text = self.get_text();
        if let Some(on_change) = self.on_change.as_mut() {
            on_change(&new_text);
        }
    }

    /// `deleteYankedText()` (editor.ts:2078-2114).
    pub(crate) fn delete_yanked_text(&mut self) {
        // const yankedText = this.killRing.peek();
        // if (!yankedText) return;
        let yanked_text = match self.kill_ring.peek() {
            Some(text) => text.to_string(),
            None => return,
        };

        // const yankLines = yankedText.split("\n");
        let yank_lines: Vec<&str> = yanked_text.split('\n').collect();

        if yank_lines.len() == 1 {
            // Single line - delete backward from cursor
            // const currentLine = this.state.lines[this.state.cursorLine] || "";
            let current_line = self
                .state
                .lines
                .get(self.state.cursor_line)
                .cloned()
                .unwrap_or_default();
            // const deleteLen = yankedText.length;
            let delete_len = utf16_len(&yanked_text);
            // const before = currentLine.slice(0, this.state.cursorCol - deleteLen);
            // (JS slice clamps negative indices; saturating_sub mirrors that
            // for the tested paths where the cursor sits at the yank end.)
            let before = slice_utf16(
                &current_line,
                0,
                self.state.cursor_col.saturating_sub(delete_len),
            );
            // const after = currentLine.slice(this.state.cursorCol);
            let after = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));
            // this.state.lines[this.state.cursorLine] = before + after;
            self.state.lines[self.state.cursor_line] = format!("{before}{after}");
            // this.setCursorCol(this.state.cursorCol - deleteLen);
            let col = self.state.cursor_col.saturating_sub(delete_len);
            self.set_cursor_col(col);
        } else {
            // Multi-line delete - cursor is at end of last yanked line
            // const startLine = this.state.cursorLine - (yankLines.length - 1);
            let start_line = self
                .state
                .cursor_line
                .saturating_sub(yank_lines.len() - 1);
            let start_line_text = self
                .state
                .lines
                .get(start_line)
                .cloned()
                .unwrap_or_default();
            // const startCol = (this.state.lines[startLine] || "").length - (yankLines[0] || "").length;
            let start_col = utf16_len(&start_line_text).saturating_sub(utf16_len(yank_lines[0]));

            // Get text after cursor on current line
            // const afterCursor = (this.state.lines[this.state.cursorLine] || "").slice(this.state.cursorCol);
            let current_line = self
                .state
                .lines
                .get(self.state.cursor_line)
                .cloned()
                .unwrap_or_default();
            let after_cursor = slice_utf16(&current_line, self.state.cursor_col, utf16_len(&current_line));

            // Get text before yank start position
            // const beforeYank = (this.state.lines[startLine] || "").slice(0, startCol);
            let before_yank = slice_utf16(&start_line_text, 0, start_col);

            // Remove all lines from startLine to cursorLine and replace with merged line
            // this.state.lines.splice(startLine, yankLines.length, beforeYank + afterCursor);
            let n = yank_lines.len();
            let merged = format!("{before_yank}{after_cursor}");
            self.state.lines.drain(start_line..start_line + n);
            self.state.lines.insert(start_line, merged);

            // Update cursor
            // this.state.cursorLine = startLine;
            self.state.cursor_line = start_line;
            // this.setCursorCol(startCol);
            self.set_cursor_col(start_col);
        }

        // if (this.onChange) { this.onChange(this.getText()); }
        let new_text = self.get_text();
        if let Some(on_change) = self.on_change.as_mut() {
            on_change(&new_text);
        }
    }

    /// `pushUndoSnapshot()` (editor.ts:2116-2118).
    pub(crate) fn push_undo_snapshot(&mut self) {
        // this.undoStack.push({ state: this.state, pastes: this.pastes,
        //                        pasteCounter: this.pasteCounter });
        let snapshot = EditorSnapshot {
            state: self.state.clone(),
            pastes: self.pastes.clone(),
            paste_counter: self.paste_counter,
        };
        self.undo_stack.push(&snapshot);
    }

    /// `undo()` (editor.ts:2120-2136) — Ctrl+_ / Ctrl+Shift+-.
    pub(crate) fn undo(&mut self) {
        // this.exitHistoryBrowsing();
        self.exit_history_browsing();
        // const snapshot = this.undoStack.pop();
        // if (!snapshot) return;
        if let Some(snapshot) = self.undo_stack.pop() {
            // Object.assign(this.state, snapshot.state);
            self.state = snapshot.state;
            // this.pastes = snapshot.pastes;
            self.pastes = snapshot.pastes;
            // this.pasteCounter = snapshot.pasteCounter;
            self.paste_counter = snapshot.paste_counter;
            // this.lastAction = null;
            self.last_action = None;
            // this.preferredVisualCol = null;
            self.preferred_visual_col = None;
            // if (this.onChange) { this.onChange(this.getText()); }
            let text = self.get_text();
            if let Some(on_change) = self.on_change.as_mut() {
                on_change(&text);
            }
        }
    }
}

// =============================================================================
// Tests — ports of the history / kill-ring / undo blocks of
// packages/tui/test/editor.test.ts (lines 47-316, 1201-1596, 1598-2096)
// plus all of packages/tui/test/editor-history-keybindings.test.ts.
//
// NOTE: most of these drive the editor through `handleInput`, which
// dispatches through [T4-E6] `handle_input_data` into [T4-E2]/[T4-E3]
// method bodies; until those delegates land, the tests below panic on
// their `todo!()` stubs (marked [T4-E6] in docs/t4-reports). The E4-owned
// method bodies themselves are exercised directly by the sanity tests at
// the bottom.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::editor::{EditorState, EditorTheme};
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::component::Component;    use crate::keybindings::{
        set_keybindings, KeybindingsConfig, KeybindingsManager, tui_keybinding_definitions,
    };
    use crate::test_support::{create_test_tui, global_test_lock};

    /// `new Editor(createTestTUI(), defaultEditorTheme)`
    /// (editor.test.ts:16-18, 41).
    fn create_editor() -> Editor {
        Editor::new(create_test_tui(), EditorTheme::default())
    }

    // describe("Editor component") > describe("Prompt history navigation")

    // it("does nothing on Up arrow when history is empty") [T4-E6]
    #[test]
    fn does_nothing_on_up_arrow_when_history_is_empty() {
        let mut editor = create_editor();

        editor.handle_input("\x1b[A"); // Up arrow

        assert_eq!(editor.get_text(), "");
    }

    // it("shows most recent history entry on Up arrow when editor is empty") [T4-E6]
    #[test]
    fn shows_most_recent_history_entry_on_up_arrow_when_editor_is_empty() {
        let mut editor = create_editor();

        editor.add_to_history("first prompt");
        editor.add_to_history("second prompt");

        editor.handle_input("\x1b[A"); // Up arrow

        assert_eq!(editor.get_text(), "second prompt");
    }

    // it("cycles through history entries on repeated Up arrow") [T4-E6]
    #[test]
    fn cycles_through_history_entries_on_repeated_up_arrow() {
        let mut editor = create_editor();

        editor.add_to_history("first");
        editor.add_to_history("second");
        editor.add_to_history("third");

        editor.handle_input("\x1b[A"); // Up - shows "third"
        assert_eq!(editor.get_text(), "third");

        editor.handle_input("\x1b[A"); // Up - shows "second"
        assert_eq!(editor.get_text(), "second");

        editor.handle_input("\x1b[A"); // Up - shows "first"
        assert_eq!(editor.get_text(), "first");

        editor.handle_input("\x1b[A"); // Up - stays at "first" (oldest)
        assert_eq!(editor.get_text(), "first");
    }

    // it("jumps to start before entering history from a non-empty draft") [T4-E6]
    #[test]
    fn jumps_to_start_before_entering_history_from_a_non_empty_draft() {
        let mut editor = create_editor();

        editor.add_to_history("prompt");
        editor.set_text("draft");
        editor.handle_input("\x1b[D");
        editor.handle_input("\x1b[D");

        editor.handle_input("\x1b[A"); // Up - jumps to start before history browsing
        assert_eq!(editor.get_text(), "draft");
        assert_eq!(editor.get_cursor(), (0, 0));

        editor.handle_input("\x1b[A"); // Up at start - shows "prompt"
        assert_eq!(editor.get_text(), "prompt");

        editor.handle_input("\x1b[B"); // Down - restores draft
        assert_eq!(editor.get_text(), "draft");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    // it("navigates forward through history with Down arrow") [T4-E6]
    #[test]
    fn navigates_forward_through_history_with_down_arrow() {
        let mut editor = create_editor();

        editor.add_to_history("first");
        editor.add_to_history("second");
        editor.add_to_history("third");
        editor.set_text("draft");

        // Go to oldest
        editor.handle_input("\x1b[A"); // start of draft
        editor.handle_input("\x1b[A"); // third
        editor.handle_input("\x1b[A"); // second
        editor.handle_input("\x1b[A"); // first

        // Navigate back
        editor.handle_input("\x1b[B"); // second
        assert_eq!(editor.get_text(), "second");

        editor.handle_input("\x1b[B"); // third
        assert_eq!(editor.get_text(), "third");

        editor.handle_input("\x1b[B"); // draft
        assert_eq!(editor.get_text(), "draft");
    }

    // it("exits history mode when typing a character") [T4-E6]
    #[test]
    fn exits_history_mode_when_typing_a_character() {
        let mut editor = create_editor();

        editor.add_to_history("old prompt");

        editor.handle_input("\x1b[A"); // Up - shows "old prompt"
        editor.handle_input("x"); // Type a character - exits history mode

        assert_eq!(editor.get_text(), "xold prompt");
    }

    // it("exits history mode on setText") [T4-E6]
    #[test]
    fn exits_history_mode_on_set_text() {
        let mut editor = create_editor();

        editor.add_to_history("first");
        editor.add_to_history("second");

        editor.handle_input("\x1b[A"); // Up - shows "second"
        editor.set_text(""); // External clear

        // Up should start fresh from most recent
        editor.handle_input("\x1b[A");
        assert_eq!(editor.get_text(), "second");
    }

    // it("does not add empty strings to history") [T4-E6]
    #[test]
    fn does_not_add_empty_strings_to_history() {
        let mut editor = create_editor();

        editor.add_to_history("");
        editor.add_to_history("   ");
        editor.add_to_history("valid");

        editor.handle_input("\x1b[A");
        assert_eq!(editor.get_text(), "valid");

        // Should not have more entries
        editor.handle_input("\x1b[A");
        assert_eq!(editor.get_text(), "valid");
    }

    // it("does not add consecutive duplicates to history") [T4-E6]
    #[test]
    fn does_not_add_consecutive_duplicates_to_history() {
        let mut editor = create_editor();

        editor.add_to_history("same");
        editor.add_to_history("same");
        editor.add_to_history("same");

        editor.handle_input("\x1b[A"); // "same"
        assert_eq!(editor.get_text(), "same");

        editor.handle_input("\x1b[A"); // stays at "same" (only one entry)
        assert_eq!(editor.get_text(), "same");
    }

    // it("allows non-consecutive duplicates in history") [T4-E6]
    #[test]
    fn allows_non_consecutive_duplicates_in_history() {
        let mut editor = create_editor();

        editor.add_to_history("first");
        editor.add_to_history("second");
        editor.add_to_history("first"); // Not consecutive, should be added

        editor.handle_input("\x1b[A"); // "first"
        assert_eq!(editor.get_text(), "first");

        editor.handle_input("\x1b[A"); // "second"
        assert_eq!(editor.get_text(), "second");

        editor.handle_input("\x1b[A"); // "first" (older one)
        assert_eq!(editor.get_text(), "first");
    }

    // it("uses cursor movement instead of history when editor has content") [T4-E6]
    #[test]
    fn uses_cursor_movement_instead_of_history_when_editor_has_content() {
        let mut editor = create_editor();

        editor.add_to_history("history item");
        editor.set_text("line1\nline2");

        // Cursor is at end of line2, Up should move to line1
        editor.handle_input("\x1b[A"); // Up - cursor movement

        // Insert character to verify cursor position
        editor.handle_input("X");

        // X should be inserted in line1, not replace with history
        assert_eq!(editor.get_text(), "line1X\nline2");
    }

    // it("limits history to 100 entries") [T4-E6]
    #[test]
    fn limits_history_to_100_entries() {
        let mut editor = create_editor();

        // Add 105 entries
        for i in 0..105 {
            editor.add_to_history(&format!("prompt {i}"));
        }

        // Navigate to oldest
        for _ in 0..100 {
            editor.handle_input("\x1b[A");
        }

        // Should be at entry 5 (oldest kept), not entry 0
        assert_eq!(editor.get_text(), "prompt 5");

        // One more Up should not change anything
        editor.handle_input("\x1b[A");
        assert_eq!(editor.get_text(), "prompt 5");
    }

    // it("places cursor at start after browsing history upward") [T4-E6]
    #[test]
    fn places_cursor_at_start_after_browsing_history_upward() {
        let mut editor = create_editor();

        editor.add_to_history("older entry");
        editor.add_to_history("line1\nline2\nline3");

        editor.handle_input("\x1b[A"); // Up - shows multi-line entry at start
        assert_eq!(editor.get_text(), "line1\nline2\nline3");
        assert_eq!(editor.get_cursor(), (0, 0));

        editor.handle_input("\x1b[A"); // Up again - immediately navigates to older entry
        assert_eq!(editor.get_text(), "older entry");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    // it("places cursor at end after browsing history downward") [T4-E6]
    #[test]
    fn places_cursor_at_end_after_browsing_history_downward() {
        let mut editor = create_editor();

        editor.add_to_history("older entry");
        editor.add_to_history("line1\nline2\nline3");
        editor.add_to_history("newer entry");

        editor.handle_input("\x1b[A"); // newer entry
        editor.handle_input("\x1b[A"); // multi-line entry
        editor.handle_input("\x1b[A"); // older entry

        editor.handle_input("\x1b[B"); // Down - shows multi-line entry at end
        assert_eq!(editor.get_text(), "line1\nline2\nline3");
        assert_eq!(editor.get_cursor(), (2, 5));

        editor.handle_input("\x1b[B"); // Down again - immediately navigates to newer entry
        assert_eq!(editor.get_text(), "newer entry");
    }

    // it("allows opposite-direction cursor movement within multi-line history entry") [T4-E6]
    #[test]
    fn allows_opposite_direction_cursor_movement_within_multi_line_history_entry() {
        let mut editor = create_editor();

        editor.add_to_history("line1\nline2\nline3");

        editor.handle_input("\x1b[A"); // Up - shows entry at start
        assert_eq!(editor.get_cursor(), (0, 0));

        editor.handle_input("\x1b[B"); // Down - cursor moves to line2
        assert_eq!(editor.get_text(), "line1\nline2\nline3");
        assert_eq!(editor.get_cursor(), (1, 0));

        editor.handle_input("\x1b[A"); // Up - cursor moves back to line1
        assert_eq!(editor.get_text(), "line1\nline2\nline3");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    // describe("Kill ring / yank")

    // it("Ctrl+W saves deleted text to kill ring and Ctrl+Y yanks it") [T4-E6]
    #[test]
    fn ctrl_w_saves_deleted_text_to_kill_ring_and_ctrl_y_yanks_it() {
        let mut editor = create_editor();

        editor.set_text("foo bar baz");
        editor.handle_input("\x17"); // Ctrl+W - deletes "baz"
        assert_eq!(editor.get_text(), "foo bar ");

        // Move to beginning and yank
        editor.handle_input("\x01"); // Ctrl+A
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "bazfoo bar ");
    }

    // it("Ctrl+U saves deleted text to kill ring") [T4-E6]
    #[test]
    fn ctrl_u_saves_deleted_text_to_kill_ring() {
        let mut editor = create_editor();

        editor.set_text("hello world");
        // Move cursor to middle
        editor.handle_input("\x01"); // Ctrl+A (start)
        editor.handle_input("\x1b[C"); // Right 5 times
        editor.handle_input("\x1b[C");
        editor.handle_input("\x1b[C");
        editor.handle_input("\x1b[C");
        editor.handle_input("\x1b[C");
        editor.handle_input("\x1b[C"); // After "hello "

        editor.handle_input("\x15"); // Ctrl+U - deletes "hello "
        assert_eq!(editor.get_text(), "world");

        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "hello world");
    }

    // it("Ctrl+K saves deleted text to kill ring") [T4-E6]
    #[test]
    fn ctrl_k_saves_deleted_text_to_kill_ring() {
        let mut editor = create_editor();

        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A (start)
        editor.handle_input("\x0b"); // Ctrl+K - deletes "hello world"

        assert_eq!(editor.get_text(), "");

        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "hello world");
    }

    // it("Ctrl+Y does nothing when kill ring is empty") [T4-E6]
    #[test]
    fn ctrl_y_does_nothing_when_kill_ring_is_empty() {
        let mut editor = create_editor();

        editor.set_text("test");
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "test");
    }

    // it("Alt+Y cycles through kill ring after Ctrl+Y") [T4-E6]
    #[test]
    fn alt_y_cycles_through_kill_ring_after_ctrl_y() {
        let mut editor = create_editor();

        // Create kill ring with multiple entries
        editor.set_text("first");
        editor.handle_input("\x17"); // Ctrl+W - deletes "first"
        editor.set_text("second");
        editor.handle_input("\x17"); // Ctrl+W - deletes "second"
        editor.set_text("third");
        editor.handle_input("\x17"); // Ctrl+W - deletes "third"

        // Kill ring now has: [first, second, third]
        assert_eq!(editor.get_text(), "");

        editor.handle_input("\x19"); // Ctrl+Y - yanks "third" (most recent)
        assert_eq!(editor.get_text(), "third");

        editor.handle_input("\x1by"); // Alt+Y - cycles to "second"
        assert_eq!(editor.get_text(), "second");

        editor.handle_input("\x1by"); // Alt+Y - cycles to "first"
        assert_eq!(editor.get_text(), "first");

        editor.handle_input("\x1by"); // Alt+Y - cycles back to "third"
        assert_eq!(editor.get_text(), "third");
    }

    // it("Alt+Y does nothing if not preceded by yank") [T4-E6]
    #[test]
    fn alt_y_does_nothing_if_not_preceded_by_yank() {
        let mut editor = create_editor();

        editor.set_text("test");
        editor.handle_input("\x17"); // Ctrl+W - deletes "test"
        editor.set_text("other");

        // Type something to break the yank chain
        editor.handle_input("x");
        assert_eq!(editor.get_text(), "otherx");

        // Alt+Y should do nothing
        editor.handle_input("\x1by"); // Alt+Y
        assert_eq!(editor.get_text(), "otherx");
    }

    // it("Alt+Y does nothing if kill ring has ≤1 entry") [T4-E6]
    #[test]
    fn alt_y_does_nothing_if_kill_ring_has_le_1_entry() {
        let mut editor = create_editor();

        editor.set_text("only");
        editor.handle_input("\x17"); // Ctrl+W - deletes "only"

        editor.handle_input("\x19"); // Ctrl+Y - yanks "only"
        assert_eq!(editor.get_text(), "only");

        editor.handle_input("\x1by"); // Alt+Y - should do nothing (only 1 entry)
        assert_eq!(editor.get_text(), "only");
    }

    // it("consecutive Ctrl+W accumulates into one kill ring entry") [T4-E6]
    #[test]
    fn consecutive_ctrl_w_accumulates_into_one_kill_ring_entry() {
        let mut editor = create_editor();

        editor.set_text("one two three");
        editor.handle_input("\x17"); // Ctrl+W - deletes "three"
        editor.handle_input("\x17"); // Ctrl+W - deletes "two " (prepended)
        editor.handle_input("\x17"); // Ctrl+W - deletes "one " (prepended)

        assert_eq!(editor.get_text(), "");

        // Should be one combined entry
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "one two three");
    }

    // it("Ctrl+U accumulates multiline deletes including newlines") [T4-E6]
    #[test]
    fn ctrl_u_accumulates_multiline_deletes_including_newlines() {
        let mut editor = create_editor();

        // Start with multiline text, cursor at end
        editor.set_text("line1\nline2\nline3");
        // Cursor is at end of line3 (line 2, col 5)

        // Delete "line3"
        editor.handle_input("\x15"); // Ctrl+U
        assert_eq!(editor.get_text(), "line1\nline2\n");

        // Delete newline (at start of empty line 2, merges with line1)
        editor.handle_input("\x15"); // Ctrl+U
        assert_eq!(editor.get_text(), "line1\nline2");

        // Delete "line2"
        editor.handle_input("\x15"); // Ctrl+U
        assert_eq!(editor.get_text(), "line1\n");

        // Delete newline
        editor.handle_input("\x15"); // Ctrl+U
        assert_eq!(editor.get_text(), "line1");

        // Delete "line1"
        editor.handle_input("\x15"); // Ctrl+U
        assert_eq!(editor.get_text(), "");

        // All deletions accumulated into one entry: "line1\nline2\nline3"
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "line1\nline2\nline3");
    }

    // it("backward deletions prepend, forward deletions append during accumulation") [T4-E6]
    #[test]
    fn backward_deletions_prepend_forward_deletions_append_during_accumulation() {
        let mut editor = create_editor();

        editor.set_text("prefix|suffix");
        // Position cursor at |
        editor.handle_input("\x01"); // Ctrl+A
        for _ in 0..6 {
            editor.handle_input("\x1b[C"); // Move right 6 times
        }

        editor.handle_input("\x0b"); // Ctrl+K - deletes "suffix" (forward)
        editor.handle_input("\x0b"); // Ctrl+K - deletes "|" (forward, appended)
        assert_eq!(editor.get_text(), "prefix");

        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "prefix|suffix");
    }

    // it("non-delete actions break kill accumulation") [T4-E6]
    #[test]
    fn non_delete_actions_break_kill_accumulation() {
        let mut editor = create_editor();

        // Delete "baz", then type "x" to break accumulation, then delete "x"
        editor.set_text("foo bar baz");
        editor.handle_input("\x17"); // Ctrl+W - deletes "baz"
        assert_eq!(editor.get_text(), "foo bar ");

        editor.handle_input("x"); // Typing breaks accumulation
        assert_eq!(editor.get_text(), "foo bar x");

        editor.handle_input("\x17"); // Ctrl+W - deletes "x" (separate entry, not accumulated)
        assert_eq!(editor.get_text(), "foo bar ");

        // Yank most recent - should be "x", not "xbaz"
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "foo bar x");

        // Cycle to previous - should be "baz" (separate entry)
        editor.handle_input("\x1by"); // Alt+Y
        assert_eq!(editor.get_text(), "foo bar baz");
    }

    // it("non-yank actions break Alt+Y chain") [T4-E6]
    #[test]
    fn non_yank_actions_break_alt_y_chain() {
        let mut editor = create_editor();

        editor.set_text("first");
        editor.handle_input("\x17"); // Ctrl+W
        editor.set_text("second");
        editor.handle_input("\x17"); // Ctrl+W
        editor.set_text("");

        editor.handle_input("\x19"); // Ctrl+Y - yanks "second"
        assert_eq!(editor.get_text(), "second");

        editor.handle_input("x"); // Type breaks yank chain
        assert_eq!(editor.get_text(), "secondx");

        editor.handle_input("\x1by"); // Alt+Y - should do nothing
        assert_eq!(editor.get_text(), "secondx");
    }

    // it("kill ring rotation persists after cycling") [T4-E6]
    #[test]
    fn kill_ring_rotation_persists_after_cycling() {
        let mut editor = create_editor();

        editor.set_text("first");
        editor.handle_input("\x17"); // deletes "first"
        editor.set_text("second");
        editor.handle_input("\x17"); // deletes "second"
        editor.set_text("third");
        editor.handle_input("\x17"); // deletes "third"
        editor.set_text("");

        // Ring: [first, second, third]

        editor.handle_input("\x19"); // Ctrl+Y - yanks "third"
        editor.handle_input("\x1by"); // Alt+Y - cycles to "second", ring rotates

        // Now ring is: [third, first, second]
        assert_eq!(editor.get_text(), "second");

        // Do something else
        editor.handle_input("x");
        editor.set_text("");

        // New yank should get "second" (now at end after rotation)
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "second");
    }

    // it("consecutive deletions across lines coalesce into one entry") [T4-E6]
    #[test]
    fn consecutive_deletions_across_lines_coalesce_into_one_entry() {
        let mut editor = create_editor();

        // "1\n2\n3" with cursor at end, delete everything with Ctrl+W
        editor.set_text("1\n2\n3");
        editor.handle_input("\x17"); // Ctrl+W - deletes "3"
        assert_eq!(editor.get_text(), "1\n2\n");

        editor.handle_input("\x17"); // Ctrl+W - deletes newline (merge with prev line)
        assert_eq!(editor.get_text(), "1\n2");

        editor.handle_input("\x17"); // Ctrl+W - deletes "2"
        assert_eq!(editor.get_text(), "1\n");

        editor.handle_input("\x17"); // Ctrl+W - deletes newline
        assert_eq!(editor.get_text(), "1");

        editor.handle_input("\x17"); // Ctrl+W - deletes "1"
        assert_eq!(editor.get_text(), "");

        // All deletions should have accumulated into one entry
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "1\n2\n3");
    }

    // it("Ctrl+K at line end deletes newline and coalesces") [T4-E6]
    #[test]
    fn ctrl_k_at_line_end_deletes_newline_and_coalesces() {
        let mut editor = create_editor();

        // "ab" on line 1, "cd" on line 2, cursor at end of line 1
        editor.set_text("");
        editor.handle_input("a");
        editor.handle_input("b");
        editor.handle_input("\n");
        editor.handle_input("c");
        editor.handle_input("d");
        // Move to end of first line
        editor.handle_input("\x1b[A"); // Up arrow
        editor.handle_input("\x05"); // Ctrl+E - end of line

        // Now at end of "ab", Ctrl+K should delete newline (merge with "cd")
        editor.handle_input("\x0b"); // Ctrl+K - deletes newline
        assert_eq!(editor.get_text(), "abcd");

        // Continue deleting
        editor.handle_input("\x0b"); // Ctrl+K - deletes "cd"
        assert_eq!(editor.get_text(), "ab");

        // Both deletions should accumulate
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "ab\ncd");
    }

    // it("handles yank in middle of text") [T4-E6]
    #[test]
    fn handles_yank_in_middle_of_text() {
        let mut editor = create_editor();

        editor.set_text("word");
        editor.handle_input("\x17"); // Ctrl+W - deletes "word"
        editor.set_text("hello world");

        // Move to middle (after "hello ")
        editor.handle_input("\x01"); // Ctrl+A
        for _ in 0..6 {
            editor.handle_input("\x1b[C");
        }

        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "hello wordworld");
    }

    // it("handles yank-pop in middle of text") [T4-E6]
    #[test]
    fn handles_yank_pop_in_middle_of_text() {
        let mut editor = create_editor();

        // Create two kill ring entries
        editor.set_text("FIRST");
        editor.handle_input("\x17"); // Ctrl+W - deletes "FIRST"
        editor.set_text("SECOND");
        editor.handle_input("\x17"); // Ctrl+W - deletes "SECOND"

        // Ring: ["FIRST", "SECOND"]

        // Set up "hello world" and position cursor after "hello "
        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A - go to start of line
        for _ in 0..6 {
            editor.handle_input("\x1b[C"); // Move right 6
        }

        // Yank "SECOND" in the middle
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "hello SECONDworld");

        // Yank-pop replaces "SECOND" with "FIRST"
        editor.handle_input("\x1by"); // Alt+Y
        assert_eq!(editor.get_text(), "hello FIRSTworld");
    }

    // it("multiline yank and yank-pop in middle of text") [T4-E6]
    #[test]
    fn multiline_yank_and_yank_pop_in_middle_of_text() {
        let mut editor = create_editor();

        // Create single-line entry
        editor.set_text("SINGLE");
        editor.handle_input("\x17"); // Ctrl+W - deletes "SINGLE"

        // Create multiline entry via consecutive Ctrl+U
        editor.set_text("A\nB");
        editor.handle_input("\x15"); // Ctrl+U - deletes "B"
        editor.handle_input("\x15"); // Ctrl+U - deletes newline
        editor.handle_input("\x15"); // Ctrl+U - deletes "A"
        // Ring: ["SINGLE", "A\nB"]

        // Insert in middle of "hello world"
        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A
        for _ in 0..6 {
            editor.handle_input("\x1b[C");
        }

        // Yank multiline "A\nB"
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "hello A\nBworld");

        // Yank-pop replaces with "SINGLE"
        editor.handle_input("\x1by"); // Alt+Y
        assert_eq!(editor.get_text(), "hello SINGLEworld");
    }

    // it("Alt+D deletes word forward and saves to kill ring") [T4-E6]
    #[test]
    fn alt_d_deletes_word_forward_and_saves_to_kill_ring() {
        let mut editor = create_editor();

        editor.set_text("hello world test");
        editor.handle_input("\x01"); // Ctrl+A - go to start

        editor.handle_input("\x1bd"); // Alt+D - deletes "hello"
        assert_eq!(editor.get_text(), " world test");

        editor.handle_input("\x1bd"); // Alt+D - deletes " world" (skips whitespace, then word)
        assert_eq!(editor.get_text(), " test");

        // Yank should get accumulated text
        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "hello world test");
    }

    // it("Alt+D at end of line deletes newline") [T4-E6]
    #[test]
    fn alt_d_at_end_of_line_deletes_newline() {
        let mut editor = create_editor();

        editor.set_text("line1\nline2");
        // Move to start of document, then to end of first line
        editor.handle_input("\x1b[A"); // Up arrow - go to first line
        editor.handle_input("\x05"); // Ctrl+E - end of line

        editor.handle_input("\x1bd"); // Alt+D - deletes newline (merges lines)
        assert_eq!(editor.get_text(), "line1line2");

        editor.handle_input("\x19"); // Ctrl+Y
        assert_eq!(editor.get_text(), "line1\nline2");
    }

    // describe("Undo")

    // it("does nothing when undo stack is empty") [T4-E6]
    #[test]
    fn does_nothing_when_undo_stack_is_empty() {
        let mut editor = create_editor();

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");
    }

    // it("coalesces consecutive word characters into one undo unit") [T4-E6]
    #[test]
    fn coalesces_consecutive_word_characters_into_one_undo_unit() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        assert_eq!(editor.get_text(), "hello world");

        // Undo removes " world" (space captured state before it, so we restore to "hello")
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello");

        // Undo removes "hello"
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");
    }

    // it("undoes spaces one at a time") [T4-E6]
    #[test]
    fn undoes_spaces_one_at_a_time() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input(" ");
        assert_eq!(editor.get_text(), "hello  ");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes second " "
        assert_eq!(editor.get_text(), "hello ");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes first " "
        assert_eq!(editor.get_text(), "hello");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes "hello"
        assert_eq!(editor.get_text(), "");
    }

    // it("undoes newlines and signals next word to capture state") [T4-E6]
    #[test]
    fn undoes_newlines_and_signals_next_word_to_capture_state() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input("\n");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        assert_eq!(editor.get_text(), "hello\nworld");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello\n");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");
    }

    // it("undoes backspace") [T4-E6]
    #[test]
    fn undoes_backspace() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input("\x7f"); // Backspace
        assert_eq!(editor.get_text(), "hell");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello");
    }

    // it("undoes forward delete") [T4-E6]
    #[test]
    fn undoes_forward_delete() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        editor.handle_input("\x1b[C"); // Right arrow
        editor.handle_input("\x1b[3~"); // Delete key
        assert_eq!(editor.get_text(), "hllo");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello");
    }

    // it("undoes Ctrl+W (delete word backward)") [T4-E6]
    #[test]
    fn undoes_ctrl_w_delete_word_backward() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        assert_eq!(editor.get_text(), "hello world");

        editor.handle_input("\x17"); // Ctrl+W
        assert_eq!(editor.get_text(), "hello ");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");
    }

    // it("undoes Ctrl+K (delete to line end)") [T4-E6]
    #[test]
    fn undoes_ctrl_k_delete_to_line_end() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        for _ in 0..6 {
            editor.handle_input("\x1b[C"); // Move right 6 times
        }

        editor.handle_input("\x0b"); // Ctrl+K
        assert_eq!(editor.get_text(), "hello ");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");

        editor.handle_input("|");
        assert_eq!(editor.get_text(), "hello |world");
    }

    // it("undoes Ctrl+U (delete to line start)") [T4-E6]
    #[test]
    fn undoes_ctrl_u_delete_to_line_start() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        for _ in 0..6 {
            editor.handle_input("\x1b[C"); // Move right 6 times
        }

        editor.handle_input("\x15"); // Ctrl+U
        assert_eq!(editor.get_text(), "world");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");
    }

    // it("undoes yank") [T4-E6]
    #[test]
    fn undoes_yank() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("\x17"); // Ctrl+W - delete "hello "
        editor.handle_input("\x19"); // Ctrl+Y - yank
        assert_eq!(editor.get_text(), "hello ");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");
    }

    // it("undoes single-line paste atomically") [T4-E6]
    #[test]
    fn undoes_single_line_paste_atomically() {
        let mut editor = create_editor();

        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        for _ in 0..5 {
            editor.handle_input("\x1b[C"); // Move right 5 (after "hello", before space)
        }

        // Simulate bracketed paste of "beep boop"
        editor.handle_input("\x1b[200~beep boop\x1b[201~");
        assert_eq!(editor.get_text(), "hellobeep boop world");

        // Single undo should restore entire pre-paste state
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");

        editor.handle_input("|");
        assert_eq!(editor.get_text(), "hello| world");
    }

    // it("decodes CSI-u Ctrl+letter sequences inside bracketed paste (tmux popup)") [T4-E6]
    #[test]
    fn decodes_csi_u_ctrl_letter_sequences_inside_bracketed_paste() {
        let mut editor = create_editor();

        // tmux popups with extended-keys-format=csi-u re-encode \n in pastes as
        // \x1b[106;5u (Ctrl+J). Without decoding, the per-char filter strips ESC
        // and leaks "[106;5u" between lines. See issue #3599.
        editor.handle_input("\x1b[200~line1\x1b[106;5uline2\x1b[106;5uline3\x1b[201~");
        assert_eq!(editor.get_text(), "line1\nline2\nline3");
    }

    // it("undoes multi-line paste atomically") [T4-E6]
    #[test]
    fn undoes_multi_line_paste_atomically() {
        let mut editor = create_editor();

        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        for _ in 0..5 {
            editor.handle_input("\x1b[C"); // Move right 5 (after "hello", before space)
        }

        // Simulate bracketed paste of multi-line text
        editor.handle_input("\x1b[200~line1\nline2\nline3\x1b[201~");
        assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");

        // Single undo should restore entire pre-paste state
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");

        editor.handle_input("|");
        assert_eq!(editor.get_text(), "hello| world");
    }

    // it("undoes insertTextAtCursor atomically") [T4-E6]
    #[test]
    fn undoes_insert_text_at_cursor_atomically() {
        let mut editor = create_editor();

        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        for _ in 0..5 {
            editor.handle_input("\x1b[C"); // Move right 5 (after "hello", before space)
        }

        // Programmatic insertion (e.g., clipboard image path)
        editor.insert_text_at_cursor("/tmp/image.png");
        assert_eq!(editor.get_text(), "hello/tmp/image.png world");

        // Single undo should restore entire pre-insert state
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");

        editor.handle_input("|");
        assert_eq!(editor.get_text(), "hello| world");
    }

    // it("insertTextAtCursor handles multiline text") [T4-E6]
    #[test]
    fn insert_text_at_cursor_handles_multiline_text() {
        let mut editor = create_editor();

        editor.set_text("hello world");
        editor.handle_input("\x01"); // Ctrl+A - go to start
        for _ in 0..5 {
            editor.handle_input("\x1b[C"); // Move right 5 (after "hello", before space)
        }

        // Insert multiline text
        editor.insert_text_at_cursor("line1\nline2\nline3");
        assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");

        // Cursor should be at end of inserted text (after "line3", before " world")
        let cursor = editor.get_cursor();
        assert_eq!(cursor.0, 2);
        assert_eq!(cursor.1, 5); // "line3".length

        // Single undo should restore entire pre-insert state
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");
    }

    // it("insertTextAtCursor normalizes CRLF and CR line endings") [T4-E6]
    #[test]
    fn insert_text_at_cursor_normalizes_crlf_and_cr_line_endings() {
        let mut editor = create_editor();

        editor.set_text("");

        // Insert text with CRLF
        editor.insert_text_at_cursor("a\r\nb\r\nc");
        assert_eq!(editor.get_text(), "a\nb\nc");

        editor.handle_input("\x1b[45;5u"); // Undo
        assert_eq!(editor.get_text(), "");

        // Insert text with CR only
        editor.insert_text_at_cursor("x\ry\rz");
        assert_eq!(editor.get_text(), "x\ny\nz");
    }

    // it("undoes setText to empty string") [T4-E6]
    #[test]
    fn undoes_set_text_to_empty_string() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        assert_eq!(editor.get_text(), "hello world");

        editor.set_text("");
        assert_eq!(editor.get_text(), "");

        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");
    }

    // it("clears undo stack on submit") [T4-E6]
    #[test]
    fn clears_undo_stack_on_submit() {
        let submitted = Rc::new(RefCell::new(String::new()));
        let mut editor = create_editor();
        editor.on_submit = Some(Box::new({
            let submitted = submitted.clone();
            move |text: &str| *submitted.borrow_mut() = text.to_string()
        }));

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input("\r"); // Enter - submit

        assert_eq!(submitted.borrow().as_str(), "hello");
        assert_eq!(editor.get_text(), "");

        // Undo should do nothing - stack was cleared
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");
    }

    // it("exits history browsing mode on undo") [T4-E6]
    #[test]
    fn exits_history_browsing_mode_on_undo() {
        let mut editor = create_editor();

        // Add "hello" to history
        editor.add_to_history("hello");
        assert_eq!(editor.get_text(), "");

        // Type "world"
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        assert_eq!(editor.get_text(), "world");

        // Ctrl+W - delete word
        editor.handle_input("\x17"); // Ctrl+W
        assert_eq!(editor.get_text(), "");

        // Press Up - enter history browsing, shows "hello"
        editor.handle_input("\x1b[A"); // Up arrow
        assert_eq!(editor.get_text(), "hello");

        // Undo should restore to "" (state before entering history browsing)
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");

        // Undo again should restore to "world" (state before Ctrl+W)
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "world");
    }

    // it("undo restores to pre-history state even after multiple history navigations") [T4-E6]
    #[test]
    fn undo_restores_to_pre_history_state_even_after_multiple_history_navigations() {
        let mut editor = create_editor();

        // Add history entries
        editor.add_to_history("first");
        editor.add_to_history("second");
        editor.add_to_history("third");

        // Type something
        editor.handle_input("c");
        editor.handle_input("u");
        editor.handle_input("r");
        editor.handle_input("r");
        editor.handle_input("e");
        editor.handle_input("n");
        editor.handle_input("t");
        assert_eq!(editor.get_text(), "current");

        // Clear editor
        editor.handle_input("\x17"); // Ctrl+W
        assert_eq!(editor.get_text(), "");

        // Navigate through history multiple times
        editor.handle_input("\x1b[A"); // Up - "third"
        assert_eq!(editor.get_text(), "third");
        editor.handle_input("\x1b[A"); // Up - "second"
        assert_eq!(editor.get_text(), "second");
        editor.handle_input("\x1b[A"); // Up - "first"
        assert_eq!(editor.get_text(), "first");

        // Undo should go back to "" (state before we started browsing), not intermediate states
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "");

        // Another undo goes back to "current"
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "current");
    }

    // it("cursor movement starts new undo unit") [T4-E6]
    #[test]
    fn cursor_movement_starts_new_undo_unit() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input(" ");
        editor.handle_input("w");
        editor.handle_input("o");
        editor.handle_input("r");
        editor.handle_input("l");
        editor.handle_input("d");
        assert_eq!(editor.get_text(), "hello world");

        // Move cursor left 5 (to after "hello ")
        for _ in 0..5 {
            editor.handle_input("\x1b[D");
        }

        // Type "lol" in the middle
        editor.handle_input("l");
        editor.handle_input("o");
        editor.handle_input("l");
        assert_eq!(editor.get_text(), "hello lolworld");

        // Undo should restore to "hello world" (before inserting "lol")
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello world");

        editor.handle_input("|");
        assert_eq!(editor.get_text(), "hello |world");
    }

    // it("no-op delete operations do not push undo snapshots") [T4-E6]
    #[test]
    fn no_op_delete_operations_do_not_push_undo_snapshots() {
        let mut editor = create_editor();

        editor.handle_input("h");
        editor.handle_input("e");
        editor.handle_input("l");
        editor.handle_input("l");
        editor.handle_input("o");
        assert_eq!(editor.get_text(), "hello");

        // Delete word on empty - multiple times (should be no-ops)
        editor.handle_input("\x17"); // Ctrl+W - deletes "hello"
        assert_eq!(editor.get_text(), "");
        editor.handle_input("\x17"); // Ctrl+W - no-op (nothing to delete)
        editor.handle_input("\x17"); // Ctrl+W - no-op

        // Single undo should restore "hello"
        editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
        assert_eq!(editor.get_text(), "hello");
    }

    // =========================================================================
    // editor-history-keybindings.test.ts (whole file, 1 test)
    // =========================================================================

    /// Restores the default global keybindings on drop — the port of the
    /// TS `afterEach(() => setKeybindings(new KeybindingsManager(TUI_KEYBINDINGS)))`
    /// (editor-history-keybindings.test.ts:10-12), robust against assertion
    /// panics mid-test.
    struct RestoreDefaultKeybindings;

    impl Drop for RestoreDefaultKeybindings {
        fn drop(&mut self) {
            set_keybindings(KeybindingsManager::new(
                tui_keybinding_definitions(),
                KeybindingsConfig::default(),
            ));
        }
    }

    // describe("Editor prompt history keybindings")
    // it("browses history directly without first moving the cursor") [T4-E6]
    #[test]
    fn browses_history_directly_without_first_moving_the_cursor() {
        // setKeybindings mutates process-global state — hold the crate-wide
        // test lock for the duration of this test.
        let _guard = global_test_lock();
        let _restore = RestoreDefaultKeybindings;

        let mut config = KeybindingsConfig::default();
        config.tui_editor_history_previous = Some(vec!["ctrl+p".to_string()]);
        config.tui_editor_history_next = Some(vec!["ctrl+n".to_string()]);
        set_keybindings(KeybindingsManager::new(tui_keybinding_definitions(), config));

        let mut editor = create_editor();
        editor.add_to_history("older prompt");
        editor.add_to_history("newer\nmultiline prompt");
        editor.set_text("draft");
        editor.handle_input("\x1b[D");
        editor.handle_input("\x1b[D");

        editor.handle_input("\x10"); // Ctrl+P
        assert_eq!(editor.get_text(), "newer\nmultiline prompt");
        assert_eq!(editor.get_cursor(), (0, 0));

        editor.handle_input("\x10"); // Ctrl+P
        assert_eq!(editor.get_text(), "older prompt");

        editor.handle_input("\x0e"); // Ctrl+N
        assert_eq!(editor.get_text(), "newer\nmultiline prompt");
        assert_eq!(editor.get_cursor(), (1, 16));

        editor.handle_input("\x0e"); // Ctrl+N
        assert_eq!(editor.get_text(), "draft");
        assert_eq!(editor.get_cursor(), (0, 3));
    }

    // =========================================================================
    // E4 sanity tests (not upstream ports) — exercise the E4-owned method
    // bodies directly without going through [T4-E6] handle_input dispatch
    // or [T4-E2]/[T4-E3] method bodies, so the history/kill/yank/undo logic
    // in this file is verified even before the sibling delegates land.
    // State is set via the `pub(crate)` fields instead of
    // `setTextInternal`/`setCursorCol`.
    // =========================================================================

    #[test]
    fn sanity_add_to_history_trims_skips_dups_and_caps() {
        let mut editor = create_editor();

        editor.add_to_history("");
        editor.add_to_history("   ");
        assert!(editor.history.is_empty());

        editor.add_to_history("valid");
        editor.add_to_history("valid"); // consecutive duplicate
        assert_eq!(editor.history, vec!["valid"]);

        editor.add_to_history("other");
        editor.add_to_history("valid"); // non-consecutive duplicate
        assert_eq!(editor.history, vec!["valid", "other", "valid"]);

        for i in 0..105 {
            editor.add_to_history(&format!("prompt {i}"));
        }
        assert_eq!(editor.history.len(), 100);
        assert_eq!(editor.history[0], "prompt 104"); // most recent
        assert_eq!(editor.history[99], "prompt 5"); // oldest kept
    }

    #[test]
    fn sanity_navigate_history_bounds_and_draft_exit() {
        let mut editor = create_editor();

        // Empty history: no-op even though index math would move.
        editor.navigate_history(-1);
        assert_eq!(editor.history_index, -1);
        assert_eq!(editor.get_text(), "");

        editor.add_to_history("first");
        editor.add_to_history("second");

        // Down from -1: newIndex = -2 < -1 → return.
        editor.navigate_history(1);
        assert_eq!(editor.history_index, -1);

        // Entering browsing captures the draft; exiting via Down restores it.
        // (setTextInternal is [T4-E2]; simulate its end-placement effect by
        // setting state directly for the entry point check.)
        editor.state = EditorState {
            lines: vec!["draft line".to_string()],
            cursor_line: 0,
            cursor_col: 0,
        };
        editor.navigate_history(-1); // enter browsing at "second" (start placement)
        assert_eq!(editor.history_index, 0);
        assert!(editor.history_draft.is_some());
        assert_eq!(editor.undo_stack.len(), 1); // snapshot pushed on entry

        // Up past oldest: newIndex = 1 >= len → no-op.
        editor.state.cursor_line = 0;
        editor.navigate_history(-1); // to "first"
        assert_eq!(editor.history_index, 1);
        editor.navigate_history(-1); // newIndex = 2 >= 2 → stays
        assert_eq!(editor.history_index, 1);

        // Down back to -1 restores the draft state (no setCursorCol call).
        editor.navigate_history(1);
        editor.navigate_history(1);
        assert_eq!(editor.history_index, -1);
        assert_eq!(editor.state.lines, vec!["draft line".to_string()]);
        assert_eq!(editor.state.cursor_col, 0);
        assert!(editor.history_draft.is_none());
    }

    #[test]
    fn sanity_exit_history_browsing_and_undo_noop() {
        let mut editor = create_editor();

        editor.add_to_history("entry");
        editor.history_index = 3;
        editor.history_draft = Some(EditorState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        });
        editor.exit_history_browsing();
        assert_eq!(editor.history_index, -1);
        assert!(editor.history_draft.is_none());

        // undo with empty stack: no-op, no onChange.
        let changes = Rc::new(std::cell::Cell::new(0u32));
        editor.on_change = Some(Box::new({
            let changes = changes.clone();
            move |_t: &str| changes.set(changes.get() + 1)
        }));
        editor.undo();
        assert_eq!(changes.get(), 0);
    }

    #[test]
    fn sanity_undo_restores_state_pastes_and_resets_action() {
        use std::collections::BTreeMap;

        let mut editor = create_editor();

        editor.state = EditorState {
            lines: vec!["before".to_string()],
            cursor_line: 0,
            cursor_col: 6,
        };
        editor.pastes.insert(1, "one".to_string());
        editor.paste_counter = 1;
        editor.last_action = Some(EditorAction::Kill);
        editor.preferred_visual_col = Some(4);
        editor.push_undo_snapshot();

        // Mutate everything the snapshot covers.
        editor.state = EditorState {
            lines: vec!["after".to_string(), String::new()],
            cursor_line: 1,
            cursor_col: 0,
        };
        editor.pastes = BTreeMap::new();
        editor.paste_counter = 9;

        let changes = Rc::new(RefCell::new(Vec::new()));
        editor.on_change = Some(Box::new({
            let changes = changes.clone();
            move |t: &str| changes.borrow_mut().push(t.to_string())
        }));

        editor.undo();
        assert_eq!(editor.state.lines, vec!["before".to_string()]);
        assert_eq!(editor.state.cursor_line, 0);
        assert_eq!(editor.state.cursor_col, 6);
        assert_eq!(editor.pastes.get(&1).map(String::as_str), Some("one"));
        assert_eq!(editor.paste_counter, 1);
        assert_eq!(editor.last_action, None);
        assert_eq!(editor.preferred_visual_col, None);
        assert_eq!(*changes.borrow(), vec!["before".to_string()]);
        assert!(editor.undo_stack.is_empty());
    }

    #[test]
    fn sanity_yank_guards() {
        let mut editor = create_editor();

        // Empty kill ring: yank is a no-op (no undo snapshot).
        editor.yank();
        assert_eq!(editor.undo_stack.len(), 0);

        // yankPop without a preceding yank: no-op.
        editor.kill_ring.push("text", false, false);
        editor.yank_pop();
        assert_eq!(editor.undo_stack.len(), 0);

        // yankPop with a single entry: no-op even after yank.
        editor.yank();
        editor.yank_pop();
        assert_eq!(editor.undo_stack.len(), 1);
    }

    #[test]
    fn sanity_insert_and_delete_yanked_text_single_line() {
        let mut editor = create_editor();

        // "hello world", cursor at col 6 (after "hello ").
        editor.state = EditorState {
            lines: vec!["hello world".to_string()],
            cursor_line: 0,
            cursor_col: 6,
        };
        editor.insert_yanked_text("X");
        // setCursorCol is [T4-E3]; verify the splice only (cursor clamping
        // is E3's concern). Col is passed through as 6 + 1.
        assert_eq!(editor.state.lines, vec!["hello Xworld".to_string()]);
        assert_eq!(editor.state.cursor_col, 7);

        // deleteYankedText removes "X" backward from the cursor.
        editor.kill_ring.push("X", false, false);
        editor.delete_yanked_text();
        assert_eq!(editor.state.lines, vec!["hello world".to_string()]);
        assert_eq!(editor.state.cursor_col, 6);

        // Empty peek: delete is a no-op.
        editor.kill_ring = crate::kill_ring::KillRing::new();
        editor.delete_yanked_text();
        assert_eq!(editor.state.lines, vec!["hello world".to_string()]);
    }

    #[test]
    fn sanity_insert_yanked_text_multiline() {
        let mut editor = create_editor();

        // "hello world", cursor at col 6.
        editor.state = EditorState {
            lines: vec!["hello world".to_string()],
            cursor_line: 0,
            cursor_col: 6,
        };
        editor.insert_yanked_text("A\nB");
        assert_eq!(editor.state.lines, vec!["hello A".to_string(), "Bworld".to_string()]);
        assert_eq!(editor.state.cursor_line, 1);
        assert_eq!(editor.state.cursor_col, 1); // "B".length

        // deleteYankedText (multiline) merges back.
        editor.kill_ring.push("A\nB", false, false);
        editor.delete_yanked_text();
        assert_eq!(editor.state.lines, vec!["hello world".to_string()]);
        assert_eq!(editor.state.cursor_line, 0);
        assert_eq!(editor.state.cursor_col, 6);
    }

    #[test]
    fn sanity_insert_yanked_text_exits_history_browsing() {
        let mut editor = create_editor();

        editor.add_to_history("entry");
        editor.history_index = 0;
        editor.history_draft = Some(EditorState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        });
        editor.kill_ring.push("y", false, false);
        editor.insert_yanked_text("y");
        assert_eq!(editor.history_index, -1);
        assert!(editor.history_draft.is_none());
    }

    #[test]
    fn sanity_yank_pop_rotates_and_replaces() {
        let mut editor = create_editor();

        // Ring: ["FIRST", "SECOND"]; cursor at end of empty single line.
        editor.kill_ring.push("FIRST", false, false);
        editor.kill_ring.push("SECOND", false, false);
        editor.state = EditorState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        };

        editor.yank();
        assert_eq!(editor.get_text(), "SECOND");
        assert_eq!(editor.last_action, Some(EditorAction::Yank));

        editor.yank_pop();
        assert_eq!(editor.get_text(), "FIRST");
        // Ring rotated: SECOND moved to front, FIRST now at end.
        assert_eq!(editor.kill_ring.peek(), Some("FIRST"));
        assert_eq!(editor.last_action, Some(EditorAction::Yank));
        assert_eq!(editor.undo_stack.len(), 2); // yank + yankPop each pushed
    }
}
