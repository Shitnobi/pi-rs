# T4 — Editor (editor.ts + editor-component.ts)

Target: `/tmp/pi-repo/packages/tui/src/components/editor.ts` (2473 LOC, largest
module) + `src/editor-component.ts` (74 LOC, an interface only).
Upstream tests: `test/editor.test.ts` (4380 lines, 192 tests) +
`test/editor-history-keybindings.test.ts` (1 test).

## Status

- [x] Recon: dependencies all exist in pi-tui (kill_ring, undo_stack,
      word_navigation with custom-segment callback, unicode::word_break,
      keys::decode_printable_key/matches_key, keybindings global manager,
      select_list, autocomplete provider trait).
- [x] Foundations: `TuiHost` trait (component.rs), `FakeTui` (test_support.rs).
- [ ] Wave A: D1a editor core, D2 word-wrap module, D3 parity TS side.
- [ ] Wave B: D1b editor autocomplete integration, test-port fleet.
- [ ] Wave C: parity Rust side (tui-editor group), redteams.
- [ ] Final gate: full tests + parity + TRACKING.md + commit.

## Core design decisions

### 1. UTF-16 code-unit indexing (CRITICAL)

TS `cursorCol`, `startIndex`/`endIndex`, `seg.index`, `getCursor().col` are
all **UTF-16 code units** (JS `String` semantics). Emojis in the upstream
tests are astral (😀 = 1 Rust char = 2 UTF-16 units). The editor port uses
UTF-16 units for ALL cursor/segment positions, exactly like the rest of the
port (see `word_navigation.rs` header). Slicing via `utf16`-aware helpers
(`utils::utf16_len`, `String::encode_utf16`, see `u16_chars` in
word_navigation.rs for the pattern).

### 2. TuiHost abstraction (T5 dependency avoided)

`this.tui` is used only for `requestRender()` and `terminal.rows`
(editor.ts:402,414,537,1963,2389,2408,2413). Defined in `component.rs`:

```rust
pub trait TuiHost {
    fn request_render(&mut self);
    fn terminal_rows(&self) -> usize;
}
```

`Editor` stores `tui: Rc<RefCell<dyn TuiHost>>`. `TuiMainScreen` (T5) will
implement it later. Tests use `test_support::FakeTui` (cols/rows +
`render_requests` counter, mirroring `createTestTUI(cols=80, rows=24)`).

### 3. Module layout (strict single-file ownership)

- `crates/pi-tui/src/components/editor.rs` — the whole `Editor` class
  (D1a core: state, text ops, cursor movement, history, kill/yank, undo,
  jump mode, sticky column, render, layout, mouse, handleInput dispatch;
  D1b: autocomplete methods). Mirrors editor.ts structure, TS line
  references in comments.
- `crates/pi-tui/src/components/editor_word_wrap.rs` — pure functions:
  `is_paste_marker`, `segment_graphemes_with_markers`,
  `segment_words_with_markers`, `word_wrap_line` (D2).
- `crates/pi-tui/src/components/editor_component.rs` — the
  `EditorComponent` trait (editor-component.ts) — lands with D1b (needs the
  Editor-backed shape finalized).

### 4. editor_word_wrap.rs API contract (D2)

```rust
pub struct TextChunk { pub text: String, pub start_index: usize, pub end_index: usize } // UTF-16 indices
pub struct GraphemeSegment { pub segment: String, pub index: usize } // UTF-16 index
pub fn is_paste_marker(segment: &str) -> bool; // PASTE_MARKER_SINGLE
pub fn segment_graphemes_with_markers(text: &str, valid_ids: &HashSet<u32>) -> Vec<GraphemeSegment>;
pub fn segment_words_with_markers(text: &str, valid_ids: &HashSet<u32>) -> Vec<WordSegment>; // unicode::word_break::WordSegment, is_word_like from base segmenter
pub fn word_wrap_line(line: &str, max_width: usize, pre_segmented: Option<&[GraphemeSegment]>) -> Vec<TextChunk>;
```

`segmentWithMarkers` (editor.ts:49-105): run the base segmenter, then merge
runs that form a valid paste marker (`[paste #N …]` with N in valid_ids)
into one atomic segment. Word granularity must preserve `is_word_like`
from the base segmenter (word_navigation consumes it via
`WordNavigationOptions.segment` + `is_atomic_segment: is_paste_marker`).

### 5. Editor public API (D1a; mirrors TS)

`Editor::new(tui: Rc<RefCell<dyn TuiHost>>, theme: EditorTheme)` (+ `with_options`
for EditorOptions), `get_text`, `set_text`, `get_expanded_text`, `get_lines`,
`get_cursor -> {line, col}` (UTF-16 col), `insert_text_at_cursor`,
`add_to_history`, `set_padding_x`, `get_padding_x`,
`set_autocomplete_max_visible`, `get_autocomplete_max_visible`,
`set_autocomplete_provider`, `is_showing_autocomplete`, `border_color`
(field), `on_submit`/`on_change`/`disable_submit` fields, `invalidate`,
`render(width) -> Vec<String>`, `handle_input(&str)`, `handle_mouse`,
implements `Component` + `Focusable`.

`EditorTheme { border_color: Box<dyn Fn(&str) -> String>, select_list: SelectListTheme }`.

### 6. Parity contract — tui-editor group (D3 TS side; Rust side Wave C)

Case shape (cases.mjs):

```js
{ name, group: "tui-editor", theme: "default", width?: 80,
  options?: { paddingX, autocompleteMaxVisible },
  history?: string[], setText?: string,
  script: [ {input: "…"} | {setText: "…"} | {insertTextAtCursor: "…"}
          | {addToHistory: "…"} | {render: width} | {tickMs: n} ] }
```

Output (canonical JSON, both sides):

```js
{ name, text, expandedText, cursor: {line, col}, lines,
  frames: [{width, lines: [...]}],            // one per {render}
  renderRequests,                              // FakeTui counter
  events: { onChange: [...], onSubmit: [...] } }
```

TS harness constructs `new Editor(fakeTui, defaultEditorTheme)` where
fakeTui = `{ requestRender(){count++}, terminal: {rows: 24} }`. Timer
control: patch `setTimeout` (same technique as tui-stdin-buffer) so
`{tickMs}` fires debounces deterministically.

## Wave plan

- **Wave A (parallel, disjoint files):**
  - D1a: `editor.rs` core port + port of all non-autocomplete,
    non-word-wrap tests from editor.test.ts (inline `#[cfg(test)]`).
  - D2: `editor_word_wrap.rs` + port of word-wrap test block
    (editor.test.ts ~947-1200 + 4124-4153).
  - D3: parity/ts-harness.js `tui-editor` handler + cases.mjs cases
    (non-autocomplete first), TS-only.
- **Wave B:** D1b autocomplete methods in editor.rs + async autocomplete
  test ports + editor_component.rs; Rust-side parity harness
  (pi-cli `harness tui-editor`) — coordinator.
- **Wave C:** redteams (read-only) on editor semantics vs TS.
- **Final gate:** full workspace tests, parity 100%, TRACKING.md, commit.

## Notes

- Async autocomplete tests use `flushAutocomplete()` (microtask +
  setImmediate); the Rust side models promise resolution as explicit
  `run_async()`/flush steps — like StdinBuffer's `fire_timeout`.
- `setKeybindings` global mutation in the keybindings test requires
  `test_support::global_test_lock()` + restoring defaults after.
- editor-component.ts is just an interface → Rust trait `EditorComponent`
  (object-safe, mirrors optional methods with default impls).
