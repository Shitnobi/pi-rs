# T4 / D1a — Editor core port (editor.rs)

Delegate: D1a (respawn). Target: `crates/pi-tui/src/components/editor.rs` —
`Editor` class from `packages/tui/src/components/editor.ts` (2473 LOC), minus
the async autocomplete pipeline (D1b), plus all non-autocomplete test ports
from `test/editor.test.ts`.

## Design notes

- **UTF-16 indexing** everywhere (per T4_PLAN §1): `cursor_col`,
  chunk indices, kill-ring delete math — JS `String` semantics. Private
  helpers `slice_utf16` (JS `slice`), `js_substring`, `js_index_of`,
  `js_last_index_of`, `code_unit_at` (JS `charCodeAt`) in editor.rs.
- **TuiHost**: `tui: Rc<RefCell<dyn TuiHost>>`; only `request_render()` +
  `terminal_rows()` (editor.ts:402,414,537,1963,2389,2408,2413).
- **EditorTheme**: `{ border_color: Box<dyn Fn(&str)->String>,
  select_list: Rc<SelectListTheme> }`. `Rc` (not plain value) because
  `createAutocompleteList` (editor.ts:2277-2284) must hand the SAME theme
  object to the owned `SelectList`, and `SelectListTheme` holds
  `Box<dyn Fn>` (not Clone). Default = identity fns.
  The editor stores `border_color` as its own `pub` field (TS `borderColor`
  is a mutable public field initialized from `theme.borderColor`) plus
  `select_list_theme: Rc<SelectListTheme>` (the only other `this.theme` use,
  editor.ts:2280).
- **Callbacks**: `on_submit`/`on_change: Option<Box<dyn FnMut(&str)>>`,
  `disable_submit: bool` — pub fields, mirroring TS.
- **pastes**: `BTreeMap<u32, String>` (TS `Map<number,string>`; only
  ordering-sensitive use — editor.ts:1382-1389 — sorts keys ascending, which
  BTreeMap gives directly).
- **Trigger/debounce patterns** (editor.ts:280-296): no regex crate —
  implemented as an explicit end-anchored matcher
  `autocomplete_token_pattern_test(text, chars, debounce)`; character
  classes: separator = `is_js_whitespace || is_cjk_punctuation_char`
  (mirrors autocomplete.rs's private `is_autocomplete_separator`, which is
  the port of utils.ts:58-63), boundary = start-of-string OR separator
  (utils.ts:63 includes `^`), wrappers = `( [ { < \``.
- **D1b stubs** — deviation from the stub list, documented: the task listed
  `cancelAutocomplete` etc. as `todo!()`, but `setText`/`insertTextAtCursor`/
  `addNewLine`/`submitValue`/history-branch of `handleInput` all call
  `cancelAutocomplete()`, and those run in every D1a test. So all PURE
  autocomplete methods are implemented for real; only the async
  request pipeline is stubbed:
  - real: `set_autocomplete_provider`, `set_autocomplete_trigger_characters`,
    `try_trigger_autocomplete`, `handle_tab_completion`,
    `handle_slash_command_completion`, `force_file_autocomplete`,
    `request_autocomplete` (up to the async handoff),
    `get_autocomplete_debounce_ms`, `cancel_autocomplete_request`,
    `clear_autocomplete_ui`, `cancel_autocomplete`, `is_showing_autocomplete`,
    `get_best_autocomplete_match_index`
  - `todo!("[T4-D1b]")`: `start_autocomplete_request` (async pipeline —
    TS `startAutocompleteRequest`/`runAutocompleteRequest` promise chain),
    `update_autocomplete`, `apply_autocomplete_suggestions`,
    `is_autocomplete_request_current`, `create_autocomplete_list`
    (its `onSelect` closure needs `this` → D1b's shared-ref wiring), and the
    two `SelectList`-forwarding call sites in `handle_input`/`handle_mouse`
    (owned SelectList has no `ComponentRef` yet). All unreachable in D1a:
    `autocomplete_state` can only become non-None via
    `apply_autocomplete_suggestions` (stubbed).
  - debounce timer: `autocomplete_debounce_timer:
    Option<PendingAutocompleteRequest>` (records start_token/force/
    explicit_tab; the firing is D1b). No threads/timers.
- `handleInput`'s autocomplete-mode cancel/tab/confirm branches are ported
  fully (synchronous provider calls exist on the Rust `AutocompleteProvider`
  trait).

## TS line → Rust mapping (editor.ts)

| TS | Rust | notes |
|---|---|---|
| 220-236 EditorState etc | `EditorState`/`EditorSnapshot`/`LayoutLine`/`EditorTheme`/`EditorOptions` | Clone for UndoStack |
| 268-296 patterns | `autocomplete_token_pattern_test` | explicit matcher |
| 298-313 createScrollBorder | `create_scroll_border` (pub module fn) | |
| 316-519 fields+ctor | `Editor` struct, `new`/`with_options` | |
| 521-527 validPasteIds/segment | `valid_paste_ids`, `segment_graphemes`/`segment_words` | delegates to editor_word_wrap |
| 529-553 padding/maxVisible | `get/set_padding_x`, `get/set_autocomplete_max_visible` | f64 params (JS number) |
| 555-566 setAutocompleteProvider | real (see stubs note) | |
| 570-583 addToHistory | `add_to_history` | |
| 585-603 isEditorEmpty/isOnFirst/LastVisualLine | private | |
| 605-649 navigateHistory/exitHistoryBrowsing | `navigate_history(dir)`, `exit_history_browsing` | history_index: i64 (-1 = off) |
| 651-669 setTextInternal | `set_text_internal` | |
| 671-674 invalidate | trait default no-op | |
| 676-684 borders | `render_top_border`/`render_bottom_border` (pub) | |
| 521-633 render (method 521? actual 522-633) | `Component::render` | |
| 633-696 handleMouse | `Component::handle_mouse` | autocomplete forwarding → todo!() [T4-D1b] |
| 696-998 handleInput | `Component::handle_input` | |
| 998-1086 layoutText | `layout_text` | |
| 1088-1146 getText/expand/getLines/getCursor/setText | pub methods | |
| 1165-1204 normalizeText/insertTextAtCursorInternal | `normalize_text` (module fn), `insert_text_at_cursor_internal` | |
| 1210-1283 insertCharacter | `insert_character` | |
| 1285-1354 handlePaste | `handle_paste` + `decode_csi_u_in_paste` | |
| 1356-1377 addNewLine | `add_new_line` | |
| 1379-1392 shouldSubmitOnBackslashEnter | `should_submit_on_backslash_enter` | |
| 1394-1410 submitValue | `submit_value` | |
| 1412-1504 handleBackspace | `handle_backspace` + `replace_all_markers` | |
| 1506-1512 setCursorCol | `set_cursor_col` | |
| 1514-1606 moveToVisualLine | `move_to_visual_line` (recursive) | i64 visual-col math |
| 1608-1658 computeVerticalMoveColumn | `compute_vertical_move_column` | sticky decision table |
| 1660-1673 moveToLineStart/End | | |
| 1675-1757 deleteToStart/EndOfLine | | |
| 1759-1848 deleteWordBackwards/Forward | | |
| 1850-1904 handleForwardDelete | | |
| 1906-1944 buildVisualLineMap | `build_visual_line_map` → `Vec<VisualLineRef>` | |
| 1946-1986 findVisualLineAt/findCurrentVisualLine | | |
| 1988-2062 moveCursor | | |
| 2064-2077 pageScroll | | |
| 2079-2100 moveWordBackwards | via `find_word_backward` + `segment_words_with_markers` | |
| 2100-2187 yank/yankPop/insert/deleteYankedText | | |
| 2189-2194 pushUndoSnapshot | | |
| 2120-2135 undo | | |
| 2137-2165 jumpToChar | `jump_to_char` + JS indexOf/lastIndexOf semantics | |
| 2167-2185 moveWordForwards | | |
| 2187-2199 slash-menu predicates | | |
| 2201-2275 autocomplete (pure parts) | real; async parts stubbed | |

## Test port status

(ported inline `#[cfg(test)]`; TS test line → Rust test name)

- [x] history block 47-316 (15 tests)
- [x] accessors 292-316 (2)
- [x] backslash-enter 320-376 (5)
- [x] CSI-u / unicode 378-523 (13)
- [x] Ctrl+A / Ctrl+W / word movement + CJK 513-705 (5)
- [x] scroll indicators 707-744 (2)
- [x] render/wrap-via-editor 745-968 (14; 971-1200 are wordWrapLine-direct, D2 already ported)
- [x] kill ring 1201-1596 (16)
- [x] undo 1598-2096 (22; skipped 1820 provider-paste test + 2096 "undoes autocomplete" → D1b)
- [x] jump mode 3079-3275 (13)
- [x] sticky column 3311-3813 (18)
- [x] paste markers 3815-4353 (21; skipped 4124 wordWrapLine-direct → D2)
- [x] editor-history-keybindings.test.ts (1; global_test_lock + restore)

## Open questions / notes for D1b

- `create_autocomplete_list` onSelect closure self-reference; SelectList
  forwarding in handle_input/handle_mouse needs ComponentRef wiring.
- `start_autocomplete_request` todo: model promise chain as explicit
  run/flush steps (see T4_PLAN async note).
- autocomplete.rs uses CHAR indices while editor uses UTF-16 — the boundary
  conversion is D1b's to handle (apply_completion receives editor state).
