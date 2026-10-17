# T4-E6 Report — editor_input.rs (handleInput dispatch, editor.ts:696-998)

**Status: COMPLETE.** Dispatch ported 1:1 with TS line refs; 30 tests in
`#[cfg(test)]` (12 passing now, 18 `#[ignore]`d pending E2–E5/D1b method
bodies). File-level `#![allow(unused_variables, dead_code)]` removed; the
file compiles with zero warnings of its own.

`cargo test -p pi-tui --lib editor_input` → **12 passed; 0 failed; 18
ignored** (as of final run; other delegates' files were mid-edit during
some runs — any crate-level failures were in their files, not mine).

## Implementation notes (fidelity decisions)

- **Order of dispatch** mirrors editor.ts exactly: jump-mode capture
  (700-720) → bracketed paste (722-743) → Ctrl+C copy (745-747) → undo
  (749-752) → autocomplete-mode routing (754-830) → Tab trigger (833-836)
  → deletions (838-861) → kill ring (863-869) → dedicated history
  (871-879) → line/word movement (881-895) → new line (897-913) → submit
  (915-930) → arrows (932-961) → page scroll (963-968) → jump triggers
  (970-976) → shift+space (978-981) → `decodePrintableKey` (983-986) →
  `charCodeAt(0) >= 32` fallback (995-998).
- **Paste marker replace**: JS `replace(str)` swaps only the FIRST
  occurrence → `replacen(..., 1)`. End-marker search via `js_index_of`
  (UTF-16); `substring(endIndex + 6)` via `slice_utf16` (end marker
  `\x1b[201~` = 6 UTF-16 units); `remaining` is read from `pasteBuffer`
  AFTER `handlePaste` exactly like TS; recursion via
  `self.handle_input_data(&remaining)`.
- **SelectItem vs AutocompleteItem**: TS passes
  `autocompleteList.getSelectedItem()` (a `SelectItem`) straight into
  `provider.applyCompletion(..., item: AutocompleteItem, ...)` via
  structural typing. Rust types are distinct, so I added a private
  field-copy helper `select_item_to_autocomplete` in this file
  (behaviorally identical — `applyCompletion` only reads fields).
- **`onChange` borrow workaround**: TS `this.onChange(this.getText())` —
  Rust computes `let text = self.get_text();` first, then invokes the
  boxed callback (same evaluation order as JS).
- **`insertCharacter(char)`** default `skipUndoCoalescing=false` → all
  dispatch call sites pass `false`.
- **`charCodeAt(0)`** modeled as `data.chars().next().map(|c| c as u32)`
  (None = JS NaN); `data.length` as `utf16_len(data)`. Predicate outcomes
  identical (surrogates and astral codepoints are both ≥ 32 / ≠ 10).
- **Autocomplete confirm with `/` prefix** falls through to submit
  (editor.ts:816-818); no-selection / no-provider confirm falls through to
  the rest of the dispatch — both preserved.
- Up/Down with autocomplete active route to `SelectList::handle_input`
  (Component trait method; needed `use crate::component::Component`).

## Tests

Ported from editor.test.ts:
- 513-524 "moves cursor to document start on Ctrl+A and inserts at the
  beginning" — `#[ignore]` [E2/E3]
- 861-877 "shows cursor at end of line before wrap, wraps on next char"
  (input-side companion; render assertions included) — `#[ignore]` [E2/E5]

Derived dispatch tests (marked "derived" in comments), **passing now**:
- `ctrl_c_is_ignored_by_editor` (745-747)
- `unmatched_control_bytes_are_ignored` (995-998 + bare escape)
- `jump_mode_cancelled_when_hotkey_pressed_twice` (702-706)
- `jump_mode_cancelled_on_escape` (708-719)
- `jump_mode_cancelled_on_control_char_and_falls_through` (717-719)
- `bracketed_paste_begin_buffers_input` (722-729)
- `bracketed_paste_empty_content_flushes_nothing` (730-737)
- `escape_cancels_active_autocomplete` (756-759)
- `up_down_routed_to_autocomplete_list` (761-766)
- `tab_with_autocomplete_and_no_provider_is_noop` (769-786)
- `tab_without_autocomplete_and_no_provider_is_noop` (833-836)
- `enter_with_disable_submit_does_nothing` (915-918)

Derived, **`#[ignore]`d — blocked TODO map** (each carries
`// [T4-Ex] unignore once landed`):
- [E2] `bracketed_paste_flushes_via_handle_paste`,
  `bracketed_paste_begin_and_end_in_same_chunk` (handle_paste),
  `enter_submits_value` (submit_value), `enter_after_backslash_inserts_
  newline_instead_of_submitting` (handle_backspace/add_new_line),
  `backslash_enter_submits_via_should_submit_on_backslash_enter`
  (should_submit_on_backslash_enter/submit_value),
  `alt_d_routed_to_delete_word_forward` (delete_word_forward),
  `plain_printable_inserted_via_fallback`, `shift_space_inserts_regular_
  space` (insert_character), `deletion_bindings_routed`
  (delete_to_start/end_of_line, handle_backspace)
- [E3] `jump_mode_captures_next_printable_char` (jump_to_char),
  `ctrl_arrows_routed_to_word_movement` (move_word_*),
  `page_up_down_routed_to_page_scroll` (page_scroll)
- [E3/E4] `up_arrow_on_empty_editor_browses_history`
  (build_visual_line_map / navigate_history)
- [E4] `undo_binding_routed_to_undo` (undo)
- [E4/D1b] `tab_with_autocomplete_and_provider_applies_completion`,
  `enter_with_autocomplete_confirm_applies_and_submits`
  (push_undo_snapshot / set_cursor_col; uses an inline `TestProvider`
  implementing `AutocompleteProvider::apply_completion`)
- [E2/E5] `shows_cursor_at_end_of_line_before_wrap_wraps_on_next_char`
  (insert_character / render_editor)

Note for whoever unignores: the ignored tests set up state via direct
`editor.state.lines` / `state.cursor_col` field assignment (pub(crate))
rather than `set_text`, so they only depend on the *dispatched action*
bodies, not on E2's `set_text`.

## Notes / minor observations

- `tui.editor.historyPrevious` / `historyNext` have EMPTY default keys
  (both TS and Rust keybindings.rs) — dedicated-history dispatch is
  unreachable with default bindings; covered indirectly by the Up-arrow
  history test.
- `matches_key("\x1b[32;2u", "shift+space")` parses kitty CSI-u
  independent of protocol state (verified in keys.rs), so the
  shift+space test input is valid.
- No files other than `editor_input.rs` were touched; no commits made.
