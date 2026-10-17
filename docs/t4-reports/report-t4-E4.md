# T4 delegate E4 — editor_history.rs (history / kill ring / yank / undo)

**Status: COMPLETE.** All 67 tests in `crates/pi-tui/src/components/editor_history.rs`
pass; full crate `cargo test -p pi-tui --lib` → **692 passed, 0 failed** (19 ignored
are pre-existing elsewhere). `cargo build -p pi-tui` → 0 warnings; the file-level
`#![allow(unused_variables, dead_code)]` was removed.

## Methods implemented (editor.ts line refs in doc comments)

- `add_to_history` (editor.ts:428-438) — trim, skip empty + consecutive dups,
  `insert(0, …)` unshift semantics, pop at >100.
- `navigate_history` (456-486) — Up=-1/Down=1, bounds guard, undo snapshot +
  draft capture on first entry (`-1 → >=0`), draft restore on return to -1
  (resets preferredVisualCol/snappedFromCursorCol/scrollOffset + onChange),
  `set_text_internal` with start/end cursor placement.
- `exit_history_browsing` (488-492).
- `yank` (1998-2011), `yank_pop` (2013-2033, rotate + delete/insert via kill ring).
- `insert_yanked_text` (2035-2076) — single-line and multi-line splice paths,
  UTF-16 columns via `slice_utf16`/`utf16_len`.
- `delete_yanked_text` (2078-2114) — single-line backward delete + multi-line
  splice merge; underflow-prone subtractions use `saturating_sub` (mirrors JS
  slice/`Math.max(0,…)` clamping; never hit by upstream tests).
- `push_undo_snapshot` (2116-2118) — clones state + pastes + paste_counter into
  `EditorSnapshot` (one extra clone through `UndoStack::push(&S)`).
- `undo` (2120-2136) — exit history browsing, pop, restore state/pastes/
  paste_counter, reset lastAction + preferredVisualCol, fire onChange.

**Fidelity note (checked exactly, per instructions):** TS `undo()` does NOT
reset `snappedFromCursorCol` — only `navigateHistory`'s draft-restore path
does (editor.ts:477). I followed the TS line-by-line; the task brief's mention
of snappedFromCursorCol in undo does not match the ground truth.

## Tests ported (all pass — E2/E3/E6 landed mid-flight, so the faithful
`handle_input`-driven ports run against real dispatch)

- **editor.test.ts:47-316** history block — 15/15.
- **editor.test.ts:1201-1596** kill-ring block — 20/20 (assignment listed 16;
  the block actually contains 20 tests, all ported).
- **editor.test.ts:1598-2096** undo block — 22/22 ported, excluding the async
  autocomplete-undo tests (2096+, D1b's) and the provider-paste test at 1820
  ("does not trigger autocomplete during single-line paste", D1b's). Includes
  "decodes CSI-u Ctrl+letter sequences inside bracketed paste" (in range).
- **editor-history-keybindings.test.ts** — 1/1. Holds
  `test_support::global_test_lock()`, remaps `tui_editor_history_previous`/
  `tui_editor_history_next` to ctrl+p/ctrl+n via
  `set_keybindings(KeybindingsManager::new(tui_keybinding_definitions(), config))`,
  and restores defaults via a Drop guard (port of the TS `afterEach`, robust
  against assertion panics).
- 9 extra `sanity_*` tests (clearly marked "not upstream ports") that drive
  the E4-owned methods directly via `pub(crate)` state manipulation — added so
  the E4 logic was verifiable before E2/E3/E6 landed. They still pass; happy
  for the coordinator to drop them if upstream-only tests are preferred.

## [T4-E6]-blocked tests

None remain — E6's `handle_input` dispatch landed while E4 was in flight, so
all `handle_input`-based ports pass. No deviations from faithful TS porting
were needed (every test uses `Component::handle_input` exactly like the TS
`handleInput`).

## Notes for the coordinator

- Callback captures in Rust tests use `Rc<RefCell<…>>` (E6 hit the same
  `'static`-box issue; my pattern may be useful as a fleet convention).
- Did not touch any file other than `editor_history.rs`. No git commit.
