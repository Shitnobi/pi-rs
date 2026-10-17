# T4 Report — Delegate E2 (editor_text.rs)

**Status: COMPLETE.** All stubs implemented, 42 upstream tests ported,
`cargo test -p pi-tui --lib editor_text` → **42 passed / 0 failed**, full
crate `cargo test -p pi-tui --lib` → **784 passed / 0 failed**, zero build
warnings from this file. File-level `#![allow(unused_variables, dead_code)]`
removed. Only `crates/pi-tui/src/components/editor_text.rs` was touched.

## Implemented methods (editor.ts line refs)

All 19 stub signatures kept exactly as bound; bodies ported 1:1:

- `is_at_start_of_message` (2195-2200) — `isSlashMenuAllowed` (2190-2193,
  `cursorLine === 0`) inlined.
- `is_in_slash_command_context` (2202-2216) — JS `trimStart` semantics via
  `is_js_whitespace`.
- `set_text` (1115-1132) — undo snapshot only when content differs;
  clears paste registry + counter.
- `set_text_internal` (494-505) — start/end cursor placement, UTF-16 col.
- `expand_paste_markers` (1090-1101) — per-paste marker replacement.
- `get_expanded_text` (1103-1105).
- `insert_text_at_cursor` (1134-1146) — atomic for undo.
- `insert_text_at_cursor_internal` (1157-1199) — single/multi-line splice.
- `normalize_text` (1148-1155, module fn) — CRLF/CR→LF, tab→4 spaces.
  `None` (TS `undefined`, never actually passed) → `""`.
- `insert_character` (1201-1258) — fish-style undo coalescing + full
  autocomplete auto-trigger chain (`try_trigger_autocomplete` /
  `update_autocomplete` — real sync surface in editor.rs).
- `handle_paste` (1260-1326) — incl. the CSI-u Ctrl+letter decode
  (1276-1286) written as an explicit matcher (no regex crate), control-char
  filter, file-path space prepend (`/\w/` check), large-paste marker
  (>10 lines or >1000 UTF-16 chars).
- `add_new_line` (1328-1351).
- `should_submit_on_backslash_enter` (1353-1362) — reads the global
  keybindings manager (`get_keybindings().get_keys("tui.input.submit")`)
  instead of the TS `kb` param (stub signature is binding, no param).
- `submit_value` (1364-1377) — expands markers, JS-trims, resets state +
  undo stack, fires onChange("") then onSubmit(result).
- `handle_backspace` (1380-1467) — grapheme-aware delete; PASTE_MARKER_SINGLE
  branch deletes registry entry, shifts higher ids down in ascending order,
  renumbers all marker-shaped text via the PASTE_MARKER_REGEX replacer
  (1414-1420); autocomplete update/re-trigger tail.
- `delete_to_start_of_line` (1625-1658), `delete_to_end_of_line`
  (1660-1690), `delete_word_backwards` (1692-1735),
  `delete_word_forward` (1737-1777), `handle_forward_delete` (1779-1834) —
  kill-ring pushes with prepend/accumulate exactly as TS.

## Private helpers added in this file

- `match_paste_marker_at` / `find_paste_marker_matches` —
  `PASTE_MARKER_REGEX` (editor.ts:32) matchAll as explicit scanner;
  captures group 1 (id) and group 2 (suffix incl. leading space + unit
  word) — group 2 semantics required by the renumber replacer.
- `renumber_paste_markers_in` — the handleBackspace `line.replace(...)`.
- `decode_csi_u_in_paste` — the tmux CSI-u decode inside handlePaste.
- `is_js_word_char` (`/\w/`), `js_trim_start`, `char_at_utf16`.

## Ported tests (editor.test.ts)

- 292-316 public state accessors (2)
- 320-376 Backslash+Enter block (5)
- 378-401 Kitty CSI-u block (3)
- 404-553 Unicode block incl. setText paste simulation, Ctrl+A insert,
  Ctrl+W/Alt+Backspace deletes (10)
- 1888-1925 insertTextAtCursor multiline + CRLF/CR (2)
- 3815-4378 paste-marker block (20), EXCLUDING 4124
  ("wordWrapLine re-checks overflow" — D2 already ported it):
  3815, 3821, 3846, 3868, 3890, 3906, 3930, 3952, 3967, 3986, 4007, 4022,
  4051, 4066, 4089, 4153, 4175, 4210, 4252, 4313, 4354.

Test helpers: `paste_with_marker` / `big_paste` (test.ts:3804-3814),
`first_lines_marker` / `all_lines_markers` (regex-free
`/\[paste #\d+ \+\d+ lines\]/` equivalents). on_submit captures use
`Rc<RefCell<…>>` + `move` closures (same pattern as E4/E6 — the field is
`Box<dyn FnMut(&str)>` with implicit `'static`).

## Verification

- `cargo test -p pi-tui --lib editor_text` → 42 passed / 0 failed.
- `cargo test -p pi-tui --lib editor` → 188 passed (E2+E3+E4+E5+E6+D2).
- `cargo test -p pi-tui --lib` → 784 passed / 0 failed / 19 ignored
  (ignored ones are pre-existing, in other modules).
- `cargo build -p pi-tui` → zero warnings.

## Divergence notes (all documented in-code)

1. **`should_submit_on_backslash_enter(data)`** takes no `kb` param
   (binding stub signature); reads the global keybindings manager instead.
2. **`paste_counter` saturating sub** in handleBackspace: TS
   `pasteCounter--` could theoretically go negative if a marker-shaped
   grapheme had an unregistered id; unreachable because atomic marker
   segments only form for ids in `valid_paste_ids`. Used
   `saturating_sub(1)`.
3. **Marker regex leading zeros**: `[paste #007]` parses id=7 and
   renumbers to `[paste #6]` — same as TS `Number()` + template rebuild.
4. **UTF-16**: all cursor/length math via `slice_utf16`/`utf16_len`
   (emoji tests confirm astral handling).
5. `expand_paste_markers` iterates the `BTreeMap` in ascending id order;
   TS iterates Map insertion order. These coincide except after
   handleBackspace renumbering in pathological cases (insertion order
   after delete+reinsert can differ, e.g. {3,2,4}); no upstream test
   observes the difference (a paste's content would itself have to
   contain a marker-shaped string for a *different* live id).

No TODOs remain in this file. No commits made.
