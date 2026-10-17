# T4-E3 report — editor_cursor.rs

## Progress
- [x] Read T4_PLAN/T4_RESUME, TS ground truth 1469-1623, 1836-2200, handleInput dispatch 696-998.
- [x] Implemented all 12 methods: set_cursor_col, move_to_visual_line, compute_vertical_move_column
      (+&self -> &mut self: TS mutates preferredVisualCol; no external callers — verified by grep),
      move_to_line_start/end, build_visual_line_map, find_visual_line_at, find_current_visual_line,
      move_cursor, page_scroll, move_word_backwards/forwards, jump_to_char. Compiles clean.
- [ ] Test port (575-705, 3079-3275, 3311-3813) — in progress.

- [x] Ported 36 tests (editor.test.ts:575-705 word nav x3; 3079-3275 jump mode x14;
      3311-3813 sticky column x19 — 36 total). ALL 36 PASS
      (`cargo test -p pi-tui --lib editor_cursor`), zero warnings from editor_cursor.rs,
      zero `todo!()` remaining, file-level `#![allow]` removed.
- Note: my run initially failed to COMPILE because E2's editor_text.rs landed mid-flight
      with test-module errors (EditorTheme import + 'static closure lifetimes). E2 fixed
      them within ~90s; verified my 36 tests pass on the fixed tree.

## Design notes for the coordinator
1. **`compute_vertical_move_column` signature changed `&self` -> `&mut self`** — the TS
   (editor.ts:1581-1612) mutates `this.preferredVisualCol` inside the decision table.
   Grep confirmed no cross-file callers (only `move_to_visual_line` calls it), so the
   binding contract is preserved in practice.
2. **Test harness** (`feed`/`set_text`/`render`/`insert_character`/`backspace`/`undo`
   inside `#[cfg(test)]`): E6's `handle_input` and E2's `set_text` were still `todo!()`
   stubs when I wrote the tests, so the test module drives the editor through a local
   movement-focused port of the handleInput branches (editor.ts:696-998) covering
   exactly the key sequences in my assigned test blocks, plus minimal faithful
   emulations of setText (1115-1130), render's lastWidth effect (521-531),
   insertCharacter's undo coalescing (1201-1230), handleBackspace's grapheme-delete
   path (1380-1405) and undo (2120-2136). Each helper documents the TS lines it
   mirrors. **Integration follow-up (optional):** once E6 lands, these tests could be
   switched to `editor.handle_input(...)` verbatim — the harness is isolated in the
   test module, so nothing else depends on it.
3. `jump_to_char` uses `js_index_of`/`js_last_index_of`; JS `lastIndexOf(char, -1)`
   clamps position to 0, which the harness-port reproduces via `v.max(0)`.

## Status: COMPLETE — all 12 methods implemented, 36/36 assigned tests passing.
