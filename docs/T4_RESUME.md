# T4 RESUME — Editor port

Read `docs/T4_PLAN.md` first (wave plan + binding contracts). This file is
the live checkpoint; update it as work lands.

## Current state (checkpoint 2026-10-17, AFTER Wave B landing)

**Wave B COMPLETE (uncommitted at time of writing → commit follows this
doc update):**

- ✅ **E2** — `editor_text.rs` (1952 lines): text mutation + ported tests
  (backslash-enter, CSI-u/unicode/emoji, setText/insert, paste markers incl.
  getExpandedText + marker snap navigation, CRLF). Report:
  `docs/t4-reports/report-t4-E2.md`.
- ✅ **E3** — `editor_cursor.rs` (1596 lines): cursor movement/visual map/
  sticky column/jump mode + ported tests. Report: `report-t4-E3.md`.
- ✅ **E4** — `editor_history.rs` (1962 lines): history/kill ring/undo +
  editor-history-keybindings.test.ts port (RestoreDefaultKeybindings Drop
  guard + global_test_lock). Report: `report-t4-E4.md`.
- ✅ **E5** — `editor_render.rs` (825 lines): render_editor/layout_text/
  handle_mouse_impl + tests. Report: `report-t4-E5.md`.
- ✅ **E6** — `editor_input.rs` (~940 lines): full handleInput dispatch
  (editor.ts:696-998) + derived dispatch tests. Report: `report-t4-E6.md`.
- Test status: **802 passed / 0 failed / 1 ignored**, `cargo build
  --workspace` zero warnings, parity still 447/447 (tui-editor 118 remain
  0/118 pending Wave C harness).
- Integration fixes by coordinator after fleet landing: unignored E6's 19
  stub-gated tests; fixed 2 wrong derived tests (Alt+Enter ≠ Enter for
  `shouldSubmitOnBackslashEnter`; undo of "abb" restores "" due to word-char
  coalescing).

**KEYBINDINGS-RACE LESSON (do not reintroduce):** a unit test that calls
`set_keybindings()` mutates process-global state while `cargo test` runs
test fns in PARALLEL THREADS — `global_test_lock()` only serializes tests
that also acquire it, so a remapped-keybindings test raced (and broke)
every other test pressing Enter. The positive `shouldSubmitOnBackslashEnter`
path (remap submit=shift+enter) is therefore **covered only by the
tui-editor parity differential** (per-process, deterministic), noted in
`editor_input.rs`. The negative path (Alt+Enter before backslash → newline,
no submit) IS unit-tested.

**Wave A (prior checkpoint, commit cfafe3c):**

- ✅ **D2 COMPLETE** — `components/editor_word_wrap.rs` (1042 lines):
  `is_paste_marker`, `segment_graphemes_with_markers`,
  `segment_words_with_markers`, `word_wrap_line` + 30/30 ported tests.
  See `docs/t4-reports/report-t4-word-wrap.md`.
  - KNOWN ISSUE (upstream bug, faithfully replicated): `wordWrapLine`
    recurses infinitely when a single grapheme is wider than `maxWidth`
    (e.g. `"你"` at width 1). TS stack-overflows too. Never hit by upstream
    tests (widths ≥ 8). Document in TRACKING.md at T4 close.
- ✅ **D3 COMPLETE (TS side)** — parity `tui-editor` group: 118 cases in
  `parity/cases.mjs` + handler in `parity/ts-harness.js`. `run-parity.mjs
  --filter tui-editor` shows 0/118 PASS because the **Rust harness side
  does not exist yet** (Wave C). See `docs/t4-reports/report-t4-parity-ts.md`
  (contract notes for the Rust side: identity theme, renderRequests=0 for
  non-autocomplete cases, frames include APC fake-cursor markers).
- ✅ **Coordinator foundation COMPLETE** — `components/editor.rs` (~830
  lines): types (EditorState/Snapshot/LayoutLine/VisualLineRef/
  EditorTheme/EditorOptions/enums), full struct (all fields, pub(crate)),
  ctor, trivial accessors, **synchronous autocomplete surface** (request
  scheduling/guard chain/debounce pattern/cancel — editor.ts:418-427,
  2262-2473), `autocomplete_token_pattern_test` (trigger/debounce regex
  port), `create_scroll_border`, UTF-16 helpers (`slice_utf16`,
  `utf16_to_byte`, `js_index_of`, `js_last_index_of`), `Component` +
  `Focusable` impls. Design decisions recorded in
  `docs/t4-reports/design-editor-core.md` (dead D1a delegate's report).
- ✅ **Stub files with BINDING signatures** (all compile, zero warnings,
  613 pi-tui tests green):
  - `editor_text.rs` [E2] — setText/insert/paste/newline/backspace/deletes/
    submit + slash predicates (editor.ts:1088-1467, 1759-1834, 2190-2216)
  - `editor_cursor.rs` [E3] — cursor movement/visual map/sticky/jump
    (editor.ts:1469-1612, 1836-2077, 2138-2186)
  - `editor_history.rs` [E4] — history/kill/yank/undo (editor.ts:428-505,
    1998-2136)
  - `editor_render.rs` [E5] — render/layoutText/handleMouse (editor.ts:521-694,
    998-1084)
  - `editor_input.rs` [E6] — handleInput dispatch (editor.ts:696-998)
  - `editor_autocomplete.rs` [D1b] — async pipeline (editor.ts:2324-2442);
    sync surface already in editor.rs
  Each stub carries its TS line range. **Cross-file signatures in the stubs
  are binding** — delegates own their file and may not change signatures
  other files call.

## NEXT STEPS (in order)

### 1. Spawn E2-E6 in PARALLEL (async delegates, one file each) — ✅ DONE

**STALE:** all five delegates landed; see "Current state" above.

> You are delegate E_n in the pi-rs TUI port. Read
> /home/shinobi/AI-Trident/pi-rs/docs/T4_PLAN.md and
> docs/T4_RESUME.md first. YOU OWN EXACTLY ONE FILE:
> crates/pi-tui/src/components/F.rs — replace every `todo!()` stub with the
> ported implementation from the TS ground truth
> /tmp/pi-repo/packages/tui/src/components/editor.ts (line ranges are in
> each stub's doc comment; port faithfully with TS line refs). Then port
> the assigned upstream test block(s) from
> /tmp/pi-repo/packages/tui/test/editor.test.ts into a `#[cfg(test)]`
> module at the end of your file. Remove the file-level
> `#![allow(unused_variables, dead_code)]` when done. Conventions: UTF-16
> code-unit indexing (helpers `slice_utf16`/`utf16_to_byte`/`js_index_of`/
> `js_last_index_of` in editor.rs); `crate::test_support::create_test_tui()`
> as the TUI; `EditorTheme::default()` (identity fns — assert on
> strip_terminal_sequences output where TS strips); follow test conventions
> in components/select_list.rs. Compile often: `cargo test -p pi-tui --lib
> editor`. START WRITING CODE IMMEDIATELY (a prior delegate died reading —
> read TS sections just-in-time). Append progress incrementally to
> /tmp/report-t4-E_n.md. Do NOT git commit; do NOT touch any other file.

Per-delegate test assignments (editor.test.ts line ranges):
- **E2 (editor_text.rs)**: 320-376 (backslash-enter), 378-512 (CSI-u/
  unicode/emoji insertion), 503-575 (setText/insert tests + Ctrl+W/
  Alt+Backspace deletes), 3815-4353 paste-marker block EXCEPT 4124
  (wordWrapLine-direct, already ported by D2) — includes getExpandedText
  (4153) and marker snap navigation (4175-4353), 1888-1925
  (insertTextAtCursor multiline/CRLF). Note: insert_character (E2) CALLS
  try_trigger_autocomplete/update_autocomplete — both are real (sync
  surface in editor.rs), so typing tests pass without a provider.
- **E3 (editor_cursor.rs)**: 575-705 (Ctrl+Left/Right word movement incl.
  CJK), 3079-3275 (jump mode), 3311-3813 (sticky column + resize).
- **E4 (editor_history.rs)**: 47-316 (history + accessors), 1201-1596
  (kill ring), 1598-2096 (undo, EXCEPT async autocomplete-undo tests
  2096+ → D1b), plus the whole editor-history-keybindings.test.ts (needs
  `crate::keybindings::{set_keybindings, KeybindingsManager,
  TUI_KEYBINDINGS}` + `test_support::global_test_lock()` + restore).
- **E5 (editor_render.rs)**: 707-744 (scroll borders), 745-968
  (render/wrap-via-editor tests — word_wrap_line is complete).
- **E6 (editor_input.rs)**: dispatch itself; port 292-316 (cursor/lines
  accessors) + any remaining non-autocomplete handleInput-level tests not
  covered above (grep for uncovered `it(` blocks after E2-E5 land).

### 2. After E2-E6 land: verify, fix, commit — ✅ DONE (this checkpoint)

802 passed / 0 failed / 1 ignored; build zero warnings; reports preserved
to docs/t4-reports/.

### 3. D1b — editor_autocomplete.rs + async tests — ✅ DONE

Delegate D1b landed 1773 lines (implementation + 27 ported async tests,
828 total green after coordinator fixes). See report-t4-D1b.md for the
flush/timer modeling. Coordinator fixes during integration: (a) trigger
pattern quoted-branch bug — TS `@"[^"]*` requires NO closing quote in
BOTH patterns and must fall through to the char-class alternative like
regex backtracking (this broke quoted-CJK-directory re-trigger);
(b) `run_pending_autocomplete_requests` made `pub` (host-facing flush API
alongside `tick_autocomplete_debounce`) which also clears the dead-code
warnings. `editor_component.rs` (editor-component.ts, 74 lines) ported by
coordinator: `EditorComponent: Component` trait (required
get_text/set_text/handle_input; optional members as default no-op /
fallback methods) + impl for Editor.

### 4. Wave C — Rust parity harness (tui-editor)

pi-cli `harness tui-editor` implementing docs/T4_PLAN.md §6 contract
(mirror `run_tui_components_harness` in crates/pi-cli/src/main.rs; identity
theme on both sides; script ops input/setText/insertTextAtCursor/
addToHistory/render/tickMs — tickMs drives the D1b flush). Then
`node parity/run-parity.mjs` → target 447+118 = 565/565. REBUILD RELEASE
FIRST (`cargo build --release`) — run-parity prefers target/release/pi-cli.

### 5. Wave C — redteams (read-only, /tmp reports)

2-3 read-only redteams diffing the Rust port against editor.ts
(text-ops/cursor semantics; render/layout; history/kill/undo). Fix
findings + regression tests.

### 6. Final gate

Full workspace tests, parity 100%, remove this file, TRACKING.md T4 entry
(document the wordWrapLine single-wide-grapheme infinite recursion as a
faithfully-replicated upstream bug), commit + push.

## Fleet lessons (this session)

- Delegates reliably land work but ALWAYS fail the result channel.
- Two consecutive single-delegate attempts at the whole editor.rs port
  died before writing code (too big). The fix: coordinator writes the
  struct + binding-signature stubs, then ONE FILE PER DELEGATE in
  parallel. D2 (1042-line file) and D3 (TS-only parity) completed cleanly
  under this model.
- Delegate reports MUST be written incrementally to /tmp (D1a's detailed
  design report survived only because it wrote as it went).
