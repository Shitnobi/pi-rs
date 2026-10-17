# T4-E5 report — editor_render.rs (render / layoutText / handleMouse)

Owner: E5. File: `crates/pi-tui/src/components/editor_render.rs` (sole file touched).
TS ground truth: `/tmp/pi-repo/packages/tui/src/components/editor.ts` (521-631, 633-694, 998-1084).
No git commit made.

## STATUS: COMPLETE ✅

- `cargo test -p pi-tui --lib editor_render` → **14/14 passed**, 0 ignored.
- Full suite: **784 passed, 0 failed, 19 ignored** (all 19 ignores are siblings',
  pre-existing; see "unblock note" below).
- `cargo build -p pi-tui` → **0 warnings** from my file (file-level
  `#![allow(unused_variables, dead_code)]` removed as required).
- `rustfmt --edition 2021` applied.

## Progress log

- [17:35] Recon complete. Read T4_PLAN/T4_RESUME, editor.rs (struct, helpers,
  borders, Component impl), component.rs (mouse types, CURSOR_MARKER,
  render_fake_cursor), editor_word_wrap.rs API, select_list.rs forwarding +
  test conventions, TS editor.ts 494-694 + 998-1084, TS tests 707-946.
- KEY FINDING: siblings E2 (editor_text), E3 (editor_cursor), E4
  (editor_history), E6 (editor_input) had ALL landed by the time I started
  (only D1b autocomplete pipeline still todo!()). My ported tests therefore
  drive `set_text`/`handle_input` exactly like the TS tests — **no
  `#[ignore]` fallbacks and no state-construction shims were needed**.
- [17:36] Implemented `render_editor`, `layout_text`, `handle_mouse_impl`
  (+ private `forward_mouse_to_autocomplete_list` helper for the
  SelectList forwarding that TS inlines at editor.ts:642-651).
- [17:38] Ported all 14 assigned upstream tests; passed first run.
- [17:40] rustfmt + full-suite + ignored-suite verification; report.

## Implementation notes (fidelity decisions)

- **render_editor (editor.ts:521-631)**: 1:1 line-by-line port —
  maxPadding/paddingX/contentWidth/layoutWidth geometry (528-531), cursor
  line clamp + scroll-offset adjustment with
  `maxVisibleLines = max(5, floor(terminalRows * 0.3))` (539-556), visible
  slice, top/bottom borders via `render_top_border/render_bottom_border`,
  fake cursor via `render_fake_cursor` with first-grapheme replacement vs
  end-of-line highlighted space + `cursorInPadding` right-padding trim
  (578-609), `CURSOR_MARKER` emitted only when focused (IME), autocomplete
  list appended when `autocomplete_state` + `autocomplete_list` are both
  set (615-630). Stores `last_width`, `rendered_visible_line_count`,
  `rendered_autocomplete_height` exactly as TS.
- **layout_text (editor.ts:998-1084)**: empty-editor fast path; per-line
  fits-in-one-layout-line path (cursor_pos = state.cursor_col); wrapped
  path via `word_wrap_line(line, contentWidth,
  Some(&self.segment_graphemes(&line)))` with TS's last-chunk
  (`cursorPos >= startIndex`) vs non-last-chunk
  (`[startIndex, endIndex)` + clamp-to-text-UTF-16-length) cursor
  assignment. All cursor/chunk indices are UTF-16 code units
  (`slice_utf16`, `utf16_len`).
- **handle_mouse_impl (editor.ts:633-694)**: autocomplete-row forwarding
  (634-651) re-clamps paddingX/contentWidth from `event.width`, retargets
  x/y/width/height onto the owned SelectList and returns
  `{...result, focus: true}` as
  `Some(ComponentMouseResult::Result(TuiMouseEventResult{focus: Some(true), ..}))`;
  non-click/non-left returns None (renderer-level selection runs);
  click-to-position walks `build_visual_line_map(self.last_width)` +
  grapheme walk with the `isLastSegment`/`lastGraphemeIndex` backtrack,
  then `set_cursor_col`, `last_action = None`, `exit_history_browsing`,
  conditional `update_autocomplete`, returning
  `{handled: true, focus: true}`.
- **SelectList forwarding mechanics**: the list is hosted by value, so it is
  temporarily moved into an `Rc<RefCell<SelectList>>`, coerced to
  `ComponentRef`, dispatched via `Component::handle_mouse` (genuine
  self-ref passed), then restored via `Rc::try_unwrap`. The `Forwarded`
  arm is defensive only (SelectList never forwards).
- TS `Math.floor((width-1)/2)` on usize is computed in i64 then clamped
  (`max(0, ·)`) to avoid usize underflow at width 0 — result identical to
  TS after the clamp (the only divergence, width 0, trunc-vs-floor, both
  clamp to 0).

## Ported tests (editor.test.ts)

Scroll indicators:
- "centers scroll indicators on wide borders" (707-720)
- "keeps truncated scroll indicators within width and preserves their color
  (issue #6962)" (722-744)

Grapheme-aware text wrapping:
- "wraps lines correctly when text contains wide emojis" (747-760)
- "wraps long text with emojis at correct positions" (762-775)
- "renders isolated Thai and Lao AM clusters without width drift" (777-787)
- "wraps CJK characters correctly (each is 2 columns wide)" (789-808)
- "handles mixed ASCII and wide characters in wrapping" (810-823)
- "renders cursor correctly on wide characters" (825-837)
- "does not exceed terminal width with emoji at wrap boundary" (839-851)
- "shows cursor at end of line before wrap, wraps on next char" (853-877)

Word wrapping:
- "wraps at word boundaries instead of mid-word" (880-899)
- "does not start lines with leading whitespace after word wrap" (901-919)
- "breaks long words (URLs) at character level" (921-933)
- "preserves multiple spaces within words on same line" (935-946)

TS `stripVTControlCharacters`/`stripTerminalSequences` mirrored with
`utils::strip_terminal_sequences`; `visibleWidth` with `utils::visible_width`;
`FAKE_CURSOR_START`/`render_fake_cursor` from `crate::component`; test TUI via
`create_test_tui_sized(cols, 24)` (mirrors `createTestTUI(cols)` default rows).

## Unblock note for the coordinator (files I must not touch)

- `editor_input.rs:499` — E6's `#[ignore]`d
  `shows_cursor_at_end_of_line_before_wrap_wraps_on_next_char`
  (`// [T4-E2]/[T4-E5] unignore once landed`) now PASSES with E2+E5 landed
  (verified via `cargo test -- --ignored`). Safe to unignore.
- Running the whole `--ignored` set: 17 passed, 2 failed — both failures are
  E6/E4-scope, NOT render-related (they never call render):
  - `editor_input::tests::undo_binding_routed_to_undo` (undo → E4/E6)
  - `editor_input::tests::backslash_enter_submits_via_should_submit_on_backslash_enter`
    (submit path → E2/E6)
  Remaining ignores blocked on D1b (autocomplete) will still panic until it
  lands.

## Known non-issues

- `handle_mouse_impl` calls `set_cursor_col`/`exit_history_browsing`/
  `update_autocomplete` — all landed except the D1b async pipeline behind
  `update_autocomplete` (synchronous surface is real in editor.rs; it only
  schedules, never panics).
- No upstream tests in my assigned ranges exercise `handleMouse`; it is
  ported 1:1 but test-coverage comes later (mouse tests are other blocks /
  Wave C parity).
