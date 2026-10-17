# D3 — tui-editor parity TS side (subagent 20261017_74)

## Status: COMPLETE

- [x] Read T4_PLAN.md §6, ts-harness.js, cases.mjs, editor.ts, keybindings.ts,
      keys.ts, tui.ts, test helpers (virtual-terminal.ts, test-themes.ts)
- [x] Append "tui-editor" handler to parity/ts-harness.js (+93 lines)
- [x] Append tui-editor case group to parity/cases.mjs (+209 lines, 118 cases)
- [x] Ran ALL 118 cases through `node parity/ts-harness.js <dir>` — 0 errors,
      0 contract-shape violations, 0 nondeterministic outputs (each case run
      twice, byte-identical JSON)
- [x] Semantic spot-checks (16 cases): submit events, scroll "↑ 5 more"
      border, paste-marker delete/renumber/undo, backslash-Enter, sticky
      column, CSI-u, focused CURSOR_MARKER, padding render — all exercise the
      intended code paths
- [x] `node parity/run-parity.mjs --filter tui-editor/typing-ascii`: TS side
      emits the contract JSON via the real runner path; Rust side fails with
      "unknown tool: tui-editor" — expected (Wave C implements it)
- [x] No git commit. Only the two owned files changed
      (`crates/.../editor_word_wrap.rs` shows as modified in git status, but
      that is D2's concurrent work — untouched by this subagent)

## Deliverables

### parity/ts-harness.js — "tui-editor" handler
- Fake TUI: `{ renderRequests, requestRender() { this.renderRequests++ },
  terminal: { rows: 24, cols: 80 } }`.
- Identity theme preset "default": `borderColor` + all SelectListTheme
  members are `(t) => t` (no chalk; Rust mirrors identity, like the
  tui-components "identity" presets).
- `new Editor(fakeTui, identityEditorTheme, case.options ?? {})`;
  `onChange`/`onSubmit` record into arrays; `case.history` → addToHistory;
  `case.setText` → setText.
- Script ops (exactly §6): `{input}` → handleInput, `{setText}`,
  `{insertTextAtCursor}`, `{addToHistory}`, `{render: width}` →
  frames.push({width, lines: editor.render(width)}), `{tickMs: n}` →
  virtual-clock advance firing due setTimeout callbacks (patched
  setTimeout/clearTimeout, same pattern as tui-stdin-buffer; due callbacks
  fire in scheduling order).
- Output (exact §6 shape, plain JSON.stringify like the other handlers;
  run-parity canonicalizes with sorted keys on comparison):
  `{ name, text, expandedText, cursor: {line, col}, lines, frames,
  renderRequests, events: { onChange: [...], onSubmit: [...] } }`.

### Contract extensions beyond §6 (for Wave C to mirror)
1. Case-level `focused?: boolean` → `editor.focused = true` before the
   script, so frames exercise CURSOR_MARKER (APC `\x1b_pi:c\x07`) emission.
   Unfocused frames still contain fake-cursor markers
   (`\x1b_pi:fc\x07…\x1b_pi:/fc\x07`) — renderFakeCursor is unconditional.
2. Case-level `disableSubmit?: boolean` → `editor.disableSubmit = true`
   (covers Enter no-op / backslash-Enter disabled paths).
3. Script op `{render: true}` falls back to case `width ?? 80` (all cases
   use explicit numeric widths; the plan's `{render: width}` form is
   primary).

### parity/cases.mjs — tui-editor group (118 cases, unique names)
Coverage (non-autocomplete as instructed):
- typing: ASCII, umlauts, astral emoji, ZWJ family, CJK, mixed, chunked
- cursor movement: arrows, clamping, Home/End, Ctrl+A/E, Ctrl+Left/Right,
  Alt+B/F, word moves over CJK and emoji, multiline up/down
- newlines: Alt+Enter (`\x1b\r`), Ctrl+J, CSI 13;2~; backslash-Enter;
  Enter submit (incl. empty + multiline), submit clearing, disableSubmit
- deletion: backspace over multi-code-unit emoji/ZWJ, line-join, forward
  delete, Ctrl+W, Alt+Backspace, Alt+D, punctuation words
- kill ring: kill+yank, midline, consecutive-kill accumulation, Ctrl+U,
  yank-pop cycling, empty ring, yank-pop without prior yank
- undo: word coalescing, space+word grouping, boundary no-ops, newline,
  delete, yank, paste, setText, post-submit (cleared stack), retype boundary
- history: up/down incl. draft preservation, Up-from-text (line-start
  first), empty editor, Down-not-browsing, mid-script add, duplicate/blank
  skip, multiline entry, submit reading an expanded paste marker
- jump mode: forward, backward (`\x1b\x1d`), cross-line, repeat-cancel,
  no-match, control-char cancel
- sticky column: clamp through shorter line, restore, reset on typing and
  on horizontal movement
- word wrap: widths 6–80, CJK, wide emoji, URL, multiple spaces, long word
  force-break, paddingX=2, cursor mid-frame, empty, multiline
- paste: large (12/15 lines, 1001 chars → `[paste #N …]` markers), small,
  single-line, split-across-inputs, trailing input after `\x1b[201~`,
  marker navigation (atomic), backspace deleting marker (registry cleanup
  + renumber), forward-delete marker (registry NOT updated — ground-truth
  quirk worth mirroring), undo, two markers renumbering
  (`\x1b[1;5D` + `\x7f`), CRLF, control-char filtering incl. CSI-u
  `\x1b[107;5u` → Ctrl+K decode, submit expansion, unterminated paste,
  path paste space-prepend
- scroll: down border `↑ N more`, Up-at-top, page down/up, narrow border
  truncation
- setText CRLF/CR/tab normalization, insertTextAtCursor (mid-line,
  multiline CRLF, empty no-op)
- CSI-u: plain, shifted (`\x1b[97:65;2u`), xterm modifyOtherKeys
  (`\x1b[27;2;98~`), ctrl-modified rejected (→ line-start binding),
  emoji codepoint
- `{tickMs}` no-op (no provider → no timers; op still contract-mandated)
- focused render (CURSOR_MARKER)

Notes for Wave C (Rust side):
- Key sequences used are in the case sources (escaped literals: `\x1b`,
  `\x7f`, `\r`, `\x01`…). Render frames include APC fake-cursor markers
  and `─` box-drawing borders; identity theme means borders pass through
  unstyled.
- `renderRequests` is 0 for all non-autocomplete cases (editor.ts only
  calls requestRender from setPaddingX/setAutocompleteMaxVisible and the
  autocomplete request path) — the Rust FakeTui counter must mirror this.
- maxVisibleLines in render = max(5, floor(rows*0.3)) = 7 for rows=24;
  layoutWidth = width - 2*paddingX - (paddingX ? 0 : 1).

Verification scripts (scratch, outside the repo): /tmp/verify-tui-editor.mjs
(full 118-case shape + determinism driver), /tmp/spot-tui-editor.mjs
(semantic spot-checks). Probe JSONs in /tmp.
