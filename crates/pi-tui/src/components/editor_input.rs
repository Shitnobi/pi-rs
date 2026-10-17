//! The `handleInput` dispatch (editor.ts:696-998): jump-mode capture,
//! bracketed-paste buffering, keybinding dispatch, autocomplete-mode key
//! routing, printable-key insertion.
//!
//! **Owner: T4 delegate E6.** Ported 1:1 from editor.ts:696-998; TS line
//! references inline. The action methods dispatched to live in the sibling
//! `editor_text.rs` (E2) / `editor_cursor.rs` (E3) / `editor_history.rs`
//! (E4) files; the autocomplete guard chain (`cancel_autocomplete`,
//! `handle_tab_completion`, …) is real in `editor.rs`.

use crate::component::Component; // SelectList::handle_input (select-list.ts:140)
use crate::components::autocomplete::AutocompleteItem;
use crate::components::select_list::SelectItem;
use crate::keybindings::get_keybindings;
use crate::keys::{decode_printable_key, matches_key};
use crate::utils::utf16_len;

use super::editor::{js_index_of, slice_utf16, Editor, JumpDirection};

/// TS is structurally typed: `autocompleteList.getSelectedItem()` returns a
/// `SelectItem`, which editor.ts:771/790 passes straight into
/// `provider.applyCompletion(..., item: AutocompleteItem, ...)`. The two
/// shapes are field-identical; the Rust port copies field-by-field
/// (`applyCompletion` only reads the fields).
fn select_item_to_autocomplete(item: &SelectItem) -> AutocompleteItem {
    AutocompleteItem {
        value: item.value.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
    }
}

impl Editor {
    /// `handleInput(data)` (editor.ts:696-998) — the inherent method behind
    /// `Component::handle_input`.
    pub(crate) fn handle_input_data(&mut self, data: &str) {
        // editor.ts:697
        let kb = get_keybindings();
        let mut data = data;

        // Handle character jump mode (awaiting next character to jump to)
        // (editor.ts:700-720)
        if let Some(direction) = self.jump_mode {
            // Cancel if the hotkey is pressed again (editor.ts:702-706)
            if kb.matches(data, "tui.editor.jumpForward")
                || kb.matches(data, "tui.editor.jumpBackward")
            {
                self.jump_mode = None;
                return;
            }

            // editor.ts:708: `decodePrintableKey(data) ??
            // (data.charCodeAt(0) >= 32 ? data : undefined)`
            let printable = decode_printable_key(data).or_else(|| {
                match data.chars().next() {
                    Some(c) if (c as u32) >= 32 => Some(data.to_string()),
                    _ => None,
                }
            });
            if let Some(printable) = printable {
                // Printable character - perform the jump (editor.ts:710-714)
                self.jump_mode = None;
                self.jump_to_char(&printable, direction);
                return;
            }

            // Control character - cancel and fall through to normal handling
            // (editor.ts:717-719)
            self.jump_mode = None;
        }

        // Handle bracketed paste mode (editor.ts:722-725)
        let stripped;
        if data.contains("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            // JS `replace` with a string pattern replaces the FIRST
            // occurrence only.
            stripped = data.replacen("\x1b[200~", "", 1);
            data = &stripped;
        }

        if self.is_in_paste {
            // editor.ts:728-743
            self.paste_buffer.push_str(data);
            if let Some(end_index) = js_index_of(&self.paste_buffer, "\x1b[201~", 0) {
                let paste_content = slice_utf16(&self.paste_buffer, 0, end_index);
                if !paste_content.is_empty() {
                    self.handle_paste(&paste_content);
                }
                self.is_in_paste = false;
                // `substring(endIndex + 6)` — the end marker is 6 UTF-16
                // units (`\x1b[201~`), read from `pasteBuffer` AFTER
                // `handlePaste` like TS does.
                let remaining = slice_utf16(
                    &self.paste_buffer,
                    end_index + 6,
                    utf16_len(&self.paste_buffer),
                );
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input_data(&remaining);
                }
                return;
            }
            return;
        }

        // Ctrl+C - let parent handle (exit/clear) (editor.ts:745-747)
        if kb.matches(data, "tui.input.copy") {
            return;
        }

        // Undo (editor.ts:749-752)
        if kb.matches(data, "tui.editor.undo") {
            self.undo();
            return;
        }

        // Handle autocomplete mode (editor.ts:754-830)
        if self.autocomplete_state.is_some() && self.autocomplete_list.is_some() {
            if kb.matches(data, "tui.select.cancel") {
                self.cancel_autocomplete();
                return;
            }

            if kb.matches(data, "tui.select.up") || kb.matches(data, "tui.select.down") {
                if let Some(list) = self.autocomplete_list.as_mut() {
                    list.handle_input(data);
                }
                return;
            }

            if kb.matches(data, "tui.input.tab") {
                // editor.ts:769-786
                let selected = self
                    .autocomplete_list
                    .as_ref()
                    .and_then(|list| list.get_selected_item())
                    .map(select_item_to_autocomplete);
                if let (Some(selected), Some(provider)) =
                    (selected, self.autocomplete_provider.clone())
                {
                    self.push_undo_snapshot();
                    self.last_action = None;
                    let result = provider.apply_completion(
                        &self.state.lines,
                        self.state.cursor_line,
                        self.state.cursor_col,
                        &selected,
                        &self.autocomplete_prefix,
                    );
                    self.state.lines = result.lines;
                    self.state.cursor_line = result.cursor_line;
                    self.set_cursor_col(result.cursor_col);
                    self.cancel_autocomplete();
                    if self.on_change.is_some() {
                        let text = self.get_text();
                        if let Some(on_change) = self.on_change.as_mut() {
                            on_change(&text);
                        }
                    }
                }
                return;
            }

            if kb.matches(data, "tui.select.confirm") {
                // editor.ts:788-830
                let selected = self
                    .autocomplete_list
                    .as_ref()
                    .and_then(|list| list.get_selected_item())
                    .map(select_item_to_autocomplete);
                if let (Some(selected), Some(provider)) =
                    (selected, self.autocomplete_provider.clone())
                {
                    self.push_undo_snapshot();
                    self.last_action = None;
                    let result = provider.apply_completion(
                        &self.state.lines,
                        self.state.cursor_line,
                        self.state.cursor_col,
                        &selected,
                        &self.autocomplete_prefix,
                    );
                    self.state.lines = result.lines;
                    self.state.cursor_line = result.cursor_line;
                    self.set_cursor_col(result.cursor_col);

                    if self.autocomplete_prefix.starts_with('/') {
                        self.cancel_autocomplete();
                        // Fall through to submit (editor.ts:816-818)
                    } else {
                        self.cancel_autocomplete();
                        if self.on_change.is_some() {
                            let text = self.get_text();
                            if let Some(on_change) = self.on_change.as_mut() {
                                on_change(&text);
                            }
                        }
                        return;
                    }
                }
            }
        }

        // Tab - trigger completion (editor.ts:833-836)
        if kb.matches(data, "tui.input.tab") && self.autocomplete_state.is_none() {
            self.handle_tab_completion();
            return;
        }

        // Deletion actions (editor.ts:838-861)
        if kb.matches(data, "tui.editor.deleteToLineEnd") {
            self.delete_to_end_of_line();
            return;
        }
        if kb.matches(data, "tui.editor.deleteToLineStart") {
            self.delete_to_start_of_line();
            return;
        }
        if kb.matches(data, "tui.editor.deleteWordBackward") {
            self.delete_word_backwards();
            return;
        }
        if kb.matches(data, "tui.editor.deleteWordForward") {
            self.delete_word_forward();
            return;
        }
        if kb.matches(data, "tui.editor.deleteCharBackward") || matches_key(data, "shift+backspace")
        {
            self.handle_backspace();
            return;
        }
        if kb.matches(data, "tui.editor.deleteCharForward") || matches_key(data, "shift+delete") {
            self.handle_forward_delete();
            return;
        }

        // Kill ring actions (editor.ts:863-869)
        if kb.matches(data, "tui.editor.yank") {
            self.yank();
            return;
        }
        if kb.matches(data, "tui.editor.yankPop") {
            self.yank_pop();
            return;
        }

        // Dedicated history actions always browse entries instead of moving
        // the cursor (editor.ts:871-879)
        if kb.matches(data, "tui.editor.historyPrevious") {
            self.cancel_autocomplete();
            self.navigate_history(-1);
            return;
        }
        if kb.matches(data, "tui.editor.historyNext") {
            self.cancel_autocomplete();
            self.navigate_history(1);
            return;
        }

        // Cursor movement actions (editor.ts:881-895)
        if kb.matches(data, "tui.editor.cursorLineStart") {
            self.move_to_line_start();
            return;
        }
        if kb.matches(data, "tui.editor.cursorLineEnd") {
            self.move_to_line_end();
            return;
        }
        if kb.matches(data, "tui.editor.cursorWordLeft") {
            self.move_word_backwards();
            return;
        }
        if kb.matches(data, "tui.editor.cursorWordRight") {
            self.move_word_forwards();
            return;
        }

        // New line (editor.ts:897-913)
        // `data.charCodeAt(0)` — first UTF-16 code unit (None = NaN).
        let char_code_0 = data.chars().next().map(|c| c as u32);
        let is_new_line = kb.matches(data, "tui.input.newLine")
            || (char_code_0 == Some(10) && utf16_len(data) > 1)
            || data == "\x1b\r"
            || data == "\x1b[13;2~"
            || (utf16_len(data) > 1 && data.contains('\x1b') && data.contains('\r'))
            || (data == "\n" && utf16_len(data) == 1);
        if is_new_line {
            if self.should_submit_on_backslash_enter(data) {
                self.handle_backspace();
                self.submit_value();
                return;
            }
            self.add_new_line();
            return;
        }

        // Submit (Enter) (editor.ts:915-930)
        if kb.matches(data, "tui.input.submit") {
            if self.disable_submit {
                return;
            }

            // Workaround for terminals without Shift+Enter support:
            // If char before cursor is \, delete it and insert newline
            // instead of submitting (editor.ts:920-926).
            let current_line = self
                .state
                .lines
                .get(self.state.cursor_line)
                .cloned()
                .unwrap_or_default();
            if self.state.cursor_col > 0
                && slice_utf16(&current_line, self.state.cursor_col - 1, self.state.cursor_col)
                    == "\\"
            {
                self.handle_backspace();
                self.add_new_line();
                return;
            }

            self.submit_value();
            return;
        }

        // Arrow key navigation (with history support) (editor.ts:932-961)
        if kb.matches(data, "tui.editor.cursorUp") {
            if self.is_on_first_visual_line()
                && (self.is_editor_empty() || self.history_index > -1 || self.state.cursor_col == 0)
            {
                self.navigate_history(-1);
            } else if self.is_on_first_visual_line() {
                // Already at top - jump to start of line
                self.move_to_line_start();
            } else {
                self.move_cursor(-1, 0);
            }
            return;
        }
        if kb.matches(data, "tui.editor.cursorDown") {
            if self.history_index > -1 && self.is_on_last_visual_line() {
                self.navigate_history(1);
            } else if self.is_on_last_visual_line() {
                // Already at bottom - jump to end of line
                self.move_to_line_end();
            } else {
                self.move_cursor(1, 0);
            }
            return;
        }
        if kb.matches(data, "tui.editor.cursorRight") {
            self.move_cursor(0, 1);
            return;
        }
        if kb.matches(data, "tui.editor.cursorLeft") {
            self.move_cursor(0, -1);
            return;
        }

        // Page up/down - scroll by page and move cursor (editor.ts:963-968)
        if kb.matches(data, "tui.editor.pageUp") {
            self.page_scroll(-1);
            return;
        }
        if kb.matches(data, "tui.editor.pageDown") {
            self.page_scroll(1);
            return;
        }

        // Character jump mode triggers (editor.ts:970-976)
        if kb.matches(data, "tui.editor.jumpForward") {
            self.jump_mode = Some(JumpDirection::Forward);
            return;
        }
        if kb.matches(data, "tui.editor.jumpBackward") {
            self.jump_mode = Some(JumpDirection::Backward);
            return;
        }

        // Shift+Space - insert regular space (editor.ts:978-981)
        if matches_key(data, "shift+space") {
            self.insert_character(" ", false);
            return;
        }

        // editor.ts:983-986
        let printable = decode_printable_key(data);
        if let Some(printable) = printable {
            self.insert_character(&printable, false);
            return;
        }

        // Regular characters (editor.ts:995-998)
        if char_code_0.map_or(false, |c| c >= 32) {
            self.insert_character(data, false);
        }
    }
}

// =============================================================================
// Tests (ported from packages/tui/test/editor.test.ts + derived dispatch
// tests for editor.ts:696-998)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::autocomplete::{
        AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions, CompletionResult,
        SuggestionsOptions,
    };
    use crate::components::editor::{AutocompleteMode, EditorTheme};
    use crate::components::select_list::{SelectItem, SelectList, SelectListTheme};
    use crate::test_support::create_test_tui;
    use crate::test_support::global_test_lock;
    use crate::keybindings::{
        set_keybindings, KeybindingsConfig, KeybindingsManager, tui_keybinding_definitions,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    fn make_editor() -> Editor {
        Editor::new(create_test_tui(), EditorTheme::default())
    }

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Restores the default global keybindings on drop (see
    /// editor_history.rs — the TS `afterEach` reset).
    struct RestoreDefaultKeybindings;

    impl Drop for RestoreDefaultKeybindings {
        fn drop(&mut self) {
            set_keybindings(KeybindingsManager::new(
                tui_keybinding_definitions(),
                KeybindingsConfig::default(),
            ));
        }
    }

    /// Set up an active autocomplete UI (state + owned SelectList) without
    /// going through the async pipeline — the dispatch only checks
    /// `autocompleteState && autocompleteList`.
    fn activate_autocomplete(editor: &mut Editor, values: &[&str]) {
        let items: Vec<SelectItem> = values
            .iter()
            .map(|v| SelectItem::new(v.to_string(), v.to_string()))
            .collect();
        editor.autocomplete_state = Some(AutocompleteMode::Regular);
        editor.autocomplete_list = Some(SelectList::new(items, 5, SelectListTheme::default()));
        editor.autocomplete_prefix = "/".to_string();
    }

    /// Minimal autocomplete provider for the apply-completion dispatch
    /// tests: replaces the current token with the selected item's value.
    struct TestProvider;

    impl AutocompleteProvider for TestProvider {
        fn get_suggestions(
            &self,
            _lines: &[String],
            _cursor_line: usize,
            _cursor_col: usize,
            _options: &SuggestionsOptions,
        ) -> Option<AutocompleteSuggestions> {
            None
        }

        fn apply_completion(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
            item: &AutocompleteItem,
            prefix: &str,
        ) -> CompletionResult {
            // Insert the item value in place of the prefix before the cursor.
            let line = &lines[cursor_line];
            let start = cursor_col.saturating_sub(utf16_len(prefix));
            let before = slice_utf16(line, 0, start);
            let after = slice_utf16(line, cursor_col, utf16_len(line));
            let new_line = format!("{before}{}{after}", item.value);
            let mut lines = lines.to_vec();
            lines[cursor_line] = new_line;
            CompletionResult {
                lines,
                cursor_line,
                cursor_col: start + utf16_len(&item.value),
            }
        }
    }

    // -------------------------------------------------------------------------
    // Ported upstream tests (editor.test.ts)
    // -------------------------------------------------------------------------

    /// "moves cursor to document start on Ctrl+A and inserts at the
    /// beginning" (editor.test.ts:513-524).
    #[test]
        fn moves_cursor_to_document_start_on_ctrl_a_and_inserts_at_beginning() {
        let mut editor = make_editor();

        editor.handle_input_data("a");
        editor.handle_input_data("b");
        editor.handle_input_data("\x01"); // Ctrl+A (move to start)
        editor.handle_input_data("x"); // Insert at start

        assert_eq!(editor.get_text(), "xab");
    }

    /// "shows cursor at end of line before wrap, wraps on next char"
    /// (editor.test.ts:861-877) — input-side companion of the render tests;
    /// asserts the input dispatch inserts the wrapping character.
    #[test]
        fn shows_cursor_at_end_of_line_before_wrap_wraps_on_next_char() {
        use crate::component::render_fake_cursor;
        use crate::test_support::create_test_tui_sized;

        let width = 10;
        for padding_x in [0.0, 1.0] {
            let mut editor = Editor::with_options(
                create_test_tui_sized(width + padding_x as usize, 24),
                EditorTheme::default(),
                crate::components::editor::EditorOptions {
                    padding_x: Some(padding_x),
                    autocomplete_max_visible: None,
                },
            );

            // Type 9 chars → fills layoutWidth exactly, cursor at end on
            // same line
            for ch in "aaaaaaaaa".chars() {
                editor.handle_input_data(&ch.to_string());
            }
            let lines = editor.render(width + padding_x as usize);
            let content_lines = &lines[1..lines.len() - 1];
            assert_eq!(content_lines.len(), 1, "Should be 1 content line before wrap");
            assert!(
                content_lines[0].ends_with(&render_fake_cursor(" ")),
                "Cursor should be at end of line"
            );

            // Type 1 more → text wraps to second line
            editor.handle_input_data("a");
            let lines = editor.render(width + padding_x as usize);
            let content_lines = &lines[1..lines.len() - 1];
            assert_eq!(content_lines.len(), 2, "Should wrap to 2 content lines");
        }
    }

    // -------------------------------------------------------------------------
    // Derived dispatch tests (editor.ts:696-998) — no upstream equivalent at
    // the handleInput level; expectations derived from the TS dispatch body.
    // -------------------------------------------------------------------------

    /// derived (editor.ts:745-747): Ctrl+C is swallowed so the parent can
    /// handle exit/clear — the editor state must not change.
    #[test]
    fn ctrl_c_is_ignored_by_editor() {
        let mut editor = make_editor();
        editor.handle_input_data("\x03");
        assert_eq!(editor.get_text(), "");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    /// derived (editor.ts:995-998): unmatched control bytes and bare escape
    /// fall through every branch and are ignored.
    #[test]
    fn unmatched_control_bytes_are_ignored() {
        let mut editor = make_editor();
        editor.handle_input_data("\x07"); // Ctrl+G — unbound
        editor.handle_input_data("\x1b"); // bare escape, no autocomplete
        editor.handle_input_data("");
        assert_eq!(editor.get_text(), "");
        assert_eq!(editor.get_cursor(), (0, 0));
    }

    /// derived (editor.ts:702-706): pressing the jump hotkey again while in
    /// jump mode cancels it (no `jumpToChar` call).
    #[test]
    fn jump_mode_cancelled_when_hotkey_pressed_twice() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1d"); // Ctrl+] — jumpForward
        assert_eq!(editor.jump_mode, Some(JumpDirection::Forward));
        editor.handle_input_data("\x1d"); // hotkey again → cancel
        assert_eq!(editor.jump_mode, None);
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:708-719): escape is not printable → cancels jump
    /// mode and falls through to normal (ignored) handling.
    #[test]
    fn jump_mode_cancelled_on_escape() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1d");
        editor.handle_input_data("\x1b");
        assert_eq!(editor.jump_mode, None);
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:717-719): a control character cancels jump mode
    /// and falls through to normal handling (here: ignored).
    #[test]
    fn jump_mode_cancelled_on_control_char_and_falls_through() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1d");
        editor.handle_input_data("\x07"); // Ctrl+G — cancels jump, unbound
        assert_eq!(editor.jump_mode, None);
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:710-714): while in jump mode the next printable
    /// character is captured and routed to `jumpToChar` instead of being
    /// inserted.
    #[test]
        fn jump_mode_captures_next_printable_char() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello world".to_string()];
        editor.state.cursor_col = 0;
        editor.handle_input_data("\x1d"); // jumpForward
        editor.handle_input_data("w"); // captured → jump to 'w'
        assert_eq!(editor.jump_mode, None);
        // jumpToChar("w", forward) lands on the 'w' of "world" (col 6).
        assert_eq!(editor.get_cursor(), (0, 6));
        assert_eq!(editor.get_text(), "hello world"); // not inserted
    }

    /// derived (editor.ts:722-743): the bracketed-paste begin marker starts
    /// buffering; subsequent chunks accumulate in `pasteBuffer` and are NOT
    /// inserted until the end marker arrives.
    #[test]
    fn bracketed_paste_begin_buffers_input() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1b[200~hel");
        assert!(editor.is_in_paste);
        assert_eq!(editor.paste_buffer, "hel");
        editor.handle_input_data("lo");
        assert_eq!(editor.paste_buffer, "hello");
        // Nothing inserted yet, and buffered data must not be dispatched.
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:730-737): an empty paste (begin immediately
    /// followed by end) flushes nothing — `handlePaste` is skipped.
    #[test]
    fn bracketed_paste_empty_content_flushes_nothing() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1b[200~");
        editor.handle_input_data("\x1b[201~");
        assert!(!editor.is_in_paste);
        assert_eq!(editor.paste_buffer, "");
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:730-743): on the end marker the buffered content
    /// is flushed via `handlePaste` and any trailing bytes after the marker
    /// are re-dispatched as fresh input.
    #[test]
        fn bracketed_paste_flushes_via_handle_paste() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1b[200~pasted");
        editor.handle_input_data(" text\x1b[201~z");
        assert!(!editor.is_in_paste);
        assert_eq!(editor.paste_buffer, "");
        assert_eq!(editor.get_text(), "pasted textz"); // 'z' re-dispatched
    }

    /// derived (editor.ts:722-727): begin + end markers in one chunk with
    /// content between them flush immediately.
    #[test]
        fn bracketed_paste_begin_and_end_in_same_chunk() {
        let mut editor = make_editor();
        editor.handle_input_data("\x1b[200~hi\x1b[201~");
        assert!(!editor.is_in_paste);
        assert_eq!(editor.get_text(), "hi");
    }

    /// derived (editor.ts:756-759): escape cancels an active autocomplete
    /// (state, list, and prefix cleared).
    #[test]
    fn escape_cancels_active_autocomplete() {
        let mut editor = make_editor();
        activate_autocomplete(&mut editor, &["/help", "/hello"]);
        editor.handle_input_data("\x1b");
        assert_eq!(editor.autocomplete_state, None);
        assert!(editor.autocomplete_list.is_none());
        assert_eq!(editor.autocomplete_prefix, "");
    }

    /// derived (editor.ts:761-766): while autocomplete is showing, Up/Down
    /// are routed to the owned SelectList instead of moving the cursor.
    #[test]
    fn up_down_routed_to_autocomplete_list() {
        let mut editor = make_editor();
        activate_autocomplete(&mut editor, &["/a", "/b", "/c"]);
        editor.handle_input_data("\x1b[B"); // Down
        let selected = editor
            .autocomplete_list
            .as_ref()
            .and_then(|l| l.get_selected_item().cloned());
        assert_eq!(selected.map(|i| i.value), Some("/b".to_string()));
        editor.handle_input_data("\x1b[A"); // Up
        let selected = editor
            .autocomplete_list
            .as_ref()
            .and_then(|l| l.get_selected_item().cloned());
        assert_eq!(selected.map(|i| i.value), Some("/a".to_string()));
        // Cursor/text untouched by arrow keys while autocomplete is active.
        assert_eq!(editor.get_cursor(), (0, 0));
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:769-786): Tab with autocomplete showing but no
    /// provider is a no-op (no apply, autocomplete survives).
    #[test]
    fn tab_with_autocomplete_and_no_provider_is_noop() {
        let mut editor = make_editor();
        activate_autocomplete(&mut editor, &["/a", "/b"]);
        editor.handle_input_data("\t");
        assert_eq!(editor.autocomplete_state, Some(AutocompleteMode::Regular));
        assert!(editor.autocomplete_list.is_some());
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:833-836): Tab without autocomplete triggers
    /// `handleTabCompletion`, which returns early when no provider is set.
    #[test]
    fn tab_without_autocomplete_and_no_provider_is_noop() {
        let mut editor = make_editor();
        editor.handle_input_data("\t");
        assert_eq!(editor.get_text(), "");
        assert_eq!(editor.autocomplete_state, None);
    }

    /// derived (editor.ts:915-918): with `disableSubmit` set, Enter is
    /// swallowed (no submit, no newline).
    #[test]
    fn enter_with_disable_submit_does_nothing() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello".to_string()];
        editor.state.cursor_col = 5;
        editor.disable_submit = true;
        let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |text| sink.borrow_mut().push(text.to_string())));
        editor.handle_input_data("\r");
        assert!(submitted.borrow().is_empty());
        assert_eq!(editor.get_text(), "hello");
        assert_eq!(editor.get_lines().len(), 1);
    }

    /// derived (editor.ts:929): Enter submits via `submitValue` (clears the
    /// editor and fires `onSubmit`).
    #[test]
        fn enter_submits_value() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello".to_string()];
        editor.state.cursor_col = 5;
        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |text| sink.borrow_mut().push(text.to_string())));
        editor.handle_input_data("\r");
        assert_eq!(submitted.borrow().as_slice(), ["hello".to_string()]);
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:920-926): Enter with a `\` immediately before the
    /// cursor deletes the backslash and inserts a newline instead of
    /// submitting.
    #[test]
        fn enter_after_backslash_inserts_newline_instead_of_submitting() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello \\".to_string()];
        editor.state.cursor_col = 7;
        let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |text| sink.borrow_mut().push(text.to_string())));
        editor.handle_input_data("\r");
        assert!(submitted.borrow().is_empty());
        assert_eq!(editor.get_lines(), vec!["hello ".to_string(), String::new()]);
    }

    /// derived (editor.ts:886-907): Alt+Enter is a newLine key but NOT
    /// `matchesKey(data, "enter")`, so `shouldSubmitOnBackslashEnter`
    /// returns false even before a backslash → plain newline.
    ///
    /// NOTE: the *positive* `shouldSubmitOnBackslashEnter` path (plain Enter
    /// + remapped `tui.input.submit` = shift+enter → backspace + submit,
    /// editor.ts:1353-1362) requires mutating the process-global keybindings,
    /// which races with the parallel test threads — it is covered by the
    /// tui-editor parity differential instead (per-process, deterministic;
    /// see docs/T4_RESUME.md Wave C).
    #[test]
    fn alt_enter_before_backslash_adds_newline_without_submitting() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello \\".to_string()];
        editor.state.cursor_col = 7;
        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |text| sink.borrow_mut().push(text.to_string())));
        editor.handle_input_data("\x1b\r"); // Alt+Enter → newLine path
        assert!(submitted.borrow().is_empty());
        assert_eq!(editor.get_lines(), vec!["hello \\".to_string(), String::new()]);
    }

    /// derived (editor.ts:889-895): Ctrl+Left/Right route to word movement.
    #[test]
        fn ctrl_arrows_routed_to_word_movement() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello world".to_string()];
        editor.state.cursor_col = 11;
        editor.handle_input_data("\x1b[1;5D"); // Ctrl+Left
        assert_eq!(editor.get_cursor(), (0, 6));
        editor.handle_input_data("\x1b[1;5C"); // Ctrl+Right
        assert_eq!(editor.get_cursor(), (0, 11));
    }

    /// derived (editor.ts:963-968): PageUp/PageDown route to `pageScroll`.
    #[test]
        fn page_up_down_routed_to_page_scroll() {
        let mut editor = make_editor();
        // 50 lines so the editor scrolls.
        editor.state.lines = (0..50).map(|i| format!("line{i}")).collect();
        editor.state.cursor_line = 49;
        editor.state.cursor_col = 0;
        editor.handle_input_data("\x1b[5~"); // PageUp
        assert!(editor.scroll_offset < 49 || editor.state.cursor_line < 49);
        editor.handle_input_data("\x1b[6~"); // PageDown
        assert!(editor.state.cursor_line > 0 || editor.scroll_offset > 0);
    }

    /// derived (editor.ts:846-849): Alt+D routes to `deleteWordForward`.
    #[test]
        fn alt_d_routed_to_delete_word_forward() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello world".to_string()];
        editor.state.cursor_col = 0;
        editor.handle_input_data("\x1bd"); // Alt+D
        assert_eq!(editor.get_text(), " world");
    }

    /// derived (editor.ts:983-998): plain printable characters are inserted
    /// via the `charCodeAt(0) >= 32` fallback.
    #[test]
        fn plain_printable_inserted_via_fallback() {
        let mut editor = make_editor();
        editor.handle_input_data("a");
        editor.handle_input_data("b");
        assert_eq!(editor.get_text(), "ab");
        assert_eq!(editor.get_cursor(), (0, 2));
    }

    /// derived (editor.ts:978-981): Shift+Space inserts a regular space.
    #[test]
        fn shift_space_inserts_regular_space() {
        let mut editor = make_editor();
        editor.handle_input_data("a");
        editor.handle_input_data("\x1b[32;2u"); // kitty shift+space
        editor.handle_input_data("b");
        assert_eq!(editor.get_text(), "a b");
    }

    /// derived (editor.ts:769-786): Tab with autocomplete showing and a
    /// provider applies the selected completion.
    #[test]
        fn tab_with_autocomplete_and_provider_applies_completion() {
        let mut editor = make_editor();
        editor.state.lines = vec!["/he".to_string()];
        editor.state.cursor_col = 3;
        editor.autocomplete_provider = Some(Rc::new(TestProvider));
        activate_autocomplete(&mut editor, &["/help"]);
        editor.autocomplete_prefix = "/he".to_string();
        let changes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&changes);
        editor.on_change = Some(Box::new(move |t| sink.borrow_mut().push(t.to_string())));

        editor.handle_input_data("\t");
        assert_eq!(editor.get_text(), "/help");
        assert_eq!(editor.autocomplete_state, None);
        assert_eq!(changes.borrow().as_slice(), ["/help".to_string()]);
    }

    /// derived (editor.ts:788-830): Enter with autocomplete showing applies
    /// the selected completion; a `/` prefix falls through to submit.
    #[test]
        fn enter_with_autocomplete_confirm_applies_and_submits() {
        let mut editor = make_editor();
        editor.state.lines = vec!["/he".to_string()];
        editor.state.cursor_col = 3;
        editor.autocomplete_provider = Some(Rc::new(TestProvider));
        activate_autocomplete(&mut editor, &["/help"]);
        editor.autocomplete_prefix = "/he".to_string();
        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&submitted);
        editor.on_submit = Some(Box::new(move |t| sink.borrow_mut().push(t.to_string())));

        editor.handle_input_data("\r");
        // Slash-prefixed confirm falls through to submit after applying.
        assert_eq!(submitted.borrow().as_slice(), ["/help".to_string()]);
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:749-752): the undo binding routes to `undo()`.
    #[test]
        fn undo_binding_routed_to_undo() {
        let mut editor = make_editor();
        editor.handle_input_data("a");
        editor.handle_input_data("b");
        editor.handle_input_data("b"); // coalesced typing
        editor.handle_input_data("\x1f"); // Ctrl+- / Ctrl+_
        // "a" pushed the pre-unit snapshot (empty state); "b","b" coalesced
        // into the same unit — one undo restores "" (upstream
        // "coalesces consecutive word characters into one undo unit").
        assert_eq!(editor.get_text(), "");
    }

    /// derived (editor.ts:932-945): Up on the first visual line of an empty
    /// editor browses history instead of moving the cursor.
    #[test]
        fn up_arrow_on_empty_editor_browses_history() {
        let mut editor = make_editor();
        editor.history = vec!["previous".to_string()];
        editor.handle_input_data("\x1b[A"); // Up
        assert_eq!(editor.get_text(), "previous");
        assert_eq!(editor.history_index, 0);
    }

    /// derived (editor.ts:838-861): the deletion bindings route to their
    /// editor-text actions.
    #[test]
        fn deletion_bindings_routed() {
        let mut editor = make_editor();
        editor.state.lines = vec!["hello world".to_string()];
        editor.state.cursor_col = 5;
        editor.handle_input_data("\x15"); // Ctrl+U — deleteToLineStart
        assert_eq!(editor.get_text(), " world");
        editor.handle_input_data("\x0b"); // Ctrl+K — deleteToLineEnd
        assert_eq!(editor.get_text(), "");
        editor.handle_input_data("ab");
        editor.handle_input_data("\x7f"); // Backspace
        assert_eq!(editor.get_text(), "a");
    }
}
