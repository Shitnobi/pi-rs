# pi-tui Port Plan — Full TUI with 1:1 Parity

Port of `@earendil-works/pi-tui` (`/tmp/pi-repo/packages/tui`, ~19.5k LOC src,
50+ upstream test files) into `crates/pi-tui`, using and extending the existing
differential parity framework in `parity/`.

**Ground truth:** the TypeScript original running under Node 24 / V8 — not the
Unicode standard, not "equivalent" Rust crates. Where V8 embeds its own data
(Unicode property regexes, `Intl.Segmenter`, `RGI_Emoji`), we extract that data
from the runtime itself and embed it in Rust (1:1 by construction), then verify
differentially.

## Scope

**In:** the whole `packages/tui` library — terminal abstraction (`terminal.ts`,
`stdin-buffer.ts`, `native-*`), input (`keys.ts`, `keybindings.ts`), width/text
engine (`utils.ts`, `get-east-asian-width`), colors (`colors.ts`, `oklab.ts`),
layout (`layout.ts`, `layout-node.ts`), all 18 components (`components/*`),
autocomplete, fuzzy, word-navigation, kill-ring, undo-stack, the differential
renderer (`tui.ts`), main/alt screens (`tui-main-screen.ts`, `tui-alt-screen.ts`,
`alt-screen-search.ts`, `wheel-scroll.ts`), markdown, latex, terminal-image
(kitty), program-status, editor-component.

**Out (later, after the agent-session layer):** `coding-agent`'s
`modes/interactive/` (7.1k-LOC app layer: interactive-mode, chat-viewport,
session-share, …). The TUI *library* is self-contained and ported first.

## Why the TUI is portable now

- It is a library with a pure core: `Component.render(width) -> string[]`.
  The entire component layer is differentially testable with **no terminal** —
  same constructor calls + events, compare line arrays as canonical JSON.
- The renderer (`tui.ts`) only talks to a `Terminal` interface
  (`start/write/columns/rows`). A `RecordingTerminal` on both sides captures
  the exact escape-sequence byte stream per frame for 1:1 comparison.
- Upstream's own tests already run headless via `test/virtual-terminal.ts`
  (xterm.js) and `node --test` — they import `src/*.ts` directly under Node 24
  type-stripping, so the TS side needs no build step.

## Parity methodology (four layers)

1. **Pure-module differential** (existing harness, new `tui-*` entries):
   `visibleWidth`, `sliceByColumn`/`sliceWithWidth`, `normalizeTerminalOutput`,
   `extractSegments`/ANSI tracking, wrap/truncate, `fuzzy*`, word-navigation,
   oklab, key parsing, layout measurement — `{fn, args}` JSON in, canonical
   JSON out, exactly like the `messages`/`session-manager` entries.
2. **Render-line differential**: construct a component from JSON-describable
   state, apply a scripted event sequence, call `render(width)` at scripted
   widths; compare the returned `string[]`. Covers every component.
3. **Write-stream differential**: drive `Tui` (the renderer) against a
   `RecordingTerminal` on both sides — same request/render/resize/mouse
   script — and compare the captured `write()` byte streams frame-by-frame
   (base64 in canonical JSON). Catches damage-tracking, cursor-move, SGR,
   alt-screen enter/exit divergence that layer 2 cannot see.
4. **Upstream-test port**: port the 50+ upstream test files to Rust unit
   tests with the same expected values (they encode real regressions: CJK
   boundary, regional-indicator width, sigwinch-EACCES, slice-by-column ANSI
   order, overlay style leak, …). These run without Node and are the first
   line of defense; the differential layers are the proof.

## The Unicode data problem (the core 1:1 risk)

`utils.ts` width logic depends on V8-specific data:

| Data | V8 source | Port strategy |
|---|---|---|
| East Asian Width ranges | `get-east-asian-width` npm (generated tables) | copy the range tables verbatim into Rust (they are the same data V8 code consumes) |
| `\p{Default_Ignorable_Code_Point}`, `\p{Control}`, `\p{Mark}`, `\p{Format}`, `\p{Surrogate}`, `\p{Grapheme_Extend}`, `\p{Spacing_Mark}`, `\p{Extended_Pictographic}`, `\p{Emoji_Modifier}`, `\p{Regional_Indicator}`, `\p{Join_Control}` | V8 regex property data | **scan all 0x110000 codepoints per property against the live regex** and emit range tables (script: `scripts/gen-unicode-tables.mjs`, output checked in) |
| `\p{RGI_Emoji}` (`v` flag) — a **string set** (ZWJ families, flags, keycaps) | V8 Unicode-sets data | candidates from UCD 15.1 `emoji-test.txt` (fetched, verified) + brute-forced singles/keycaps/flags/tags; every candidate verified against `/^\p{RGI_Emoji}$/v` in Node; only regex-verified sequences are emitted. V8 wins over UCD on mismatch |
| `Intl.Segmenter` grapheme boundaries | ICU (UAX #29 w/ GB9c) | implement UAX #29 rules in Rust over the dumped GCB-derivable tables (`Grapheme_Extend`, `Spacing_Mark`, `Extended_Pictographic`, `Regional_Indicator`, Hangul L/V/T/LV/LVT from codepoint arithmetic, `Prepend` — **not regex-exposable; sourced from UCD `DerivedCoreProperties`/`emoji-data` candidate lists and verified indirectly**), then **differential-fuzz against `Intl.Segmenter`** over a wide corpus; rule divergences (GB9c conjuncts, GB11 emoji sequences) patched until corpora agree |

Regeneration: `node scripts/gen-unicode-tables.mjs > crates/pi-tui/src/unicode/tables.rs`
— deterministic, no network needed after the one-time UCD fetch (kept in
`scripts/data/`).

## Tranches

- **T1 — Foundations (pure)**: crate scaffold; `unicode/` (tables + east-asian
  lookup + UAX #29 segmenter); `utils.rs` core (ANSI code extraction, OSC-8
  hyperlinks, `visibleWidth`, `stripTerminalSequences`,
  `normalizeTerminalOutput`, `asciiVisibleWidth`, truncate-to-width,
  slice-by-column/width, grapheme cell ranges); `fuzzy.rs`, `word_navigation.rs`,
  `kill_ring.rs`, `undo_stack.rs`, `oklab.rs`/`colors.rs`, `program_status.rs`.
  Tests: ported upstream unit tests + `tui-utils` parity entry.
- **T2 — Input**: `keys.rs` (1,455 LOC parser), `keybindings.rs`,
  `stdin_buffer.rs` (ESC accumulation, bracketed paste, focus reporting),
  `native_modifiers.rs`, `native_module_path.rs`, `native_platform.rs`
  (Linux clipboard via the same external commands), `terminal.rs` (Terminal
  trait, ProcessTerminal with raw-mode via libc, kitty negotiation state
  machine). Tests: `keys/stdin-buffer/keybindings/native-*` upstream ports +
  parity on the pure parsers (byte-string → event JSON).
- **T3 — Layout + simple components**: `layout.rs`/`layout_node.rs`; `Text`,
  `TruncatedText`, `Spacer`, `Box`, `HStack`/`VStack`/`Stack`, `Loader`,
  `CancellableLoader`, `Input`, `MouseRegion`, `ScrollView`, `SelectList`,
  `SettingsList`, `autocomplete.rs`. Render-line differential + upstream ports.
- **T4 — Editor**: `editor.rs` (2,473 LOC — the largest single module),
  `editor_component.rs`, undo/kill-ring integration, history keybindings.
  Event-sequence differential (scripted key streams → state + render).
- **T5 — Renderer + screens**: `tui.rs` (differential frame rendering, mouse
  dispatch, focus), `tui_main_screen.rs`, `tui_alt_screen.rs` (1,786 LOC),
  `alt_screen_search.rs`, `wheel_scroll.rs`, `tui_renderer`. Write-stream
  differential with RecordingTerminal on both sides.
- **T6 — Media**: `markdown.rs` + `Marked` (see below), `latex.rs`,
  `terminal_image.rs` (kitty protocol; graphics detection), `image.rs`.
- **T7 — Batter (plan only until T1–T6 green)** — see below.

### Marked (markdown) strategy — decision needed in T6

`markdown.ts` renders `Marked` token streams. Options:
(a) hand-port `marked`'s lexer to Rust (big, but pure and well-tested);
(b) run the real `marked` under an embedded JS engine (rquickjs) — the
"photon-wasm trick": exact tokens by construction, at the cost of a JS-engine
dependency;
(c) harness-side token generation + a smaller Rust parser.
Recommendation: (a) for block-level + inline token surface actually consumed
by `markdown.ts`, gated by a `marked-lex` differential entry with fuzzed
markdown corpora. Revisit (b) if token parity proves too costly.

## Battering framework (post-TUI expansion — plan)

Once T1–T6 are green, expand into `parity/fuzz/`:

1. **Seeded generators** (shared determinism: harness generates cases in JS
   with a seeded xorshift PRNG, emits case files; Rust side replays them —
   no cross-language PRNG needed):
   - unicode corpus sampler: combining marks, ZWJ chains, CJK, EAW-ambiguous,
     tabs, control chars, lone surrogates in UTF-16, ANSI-dense strings;
   - random key/mouse event streams (valid + malformed escape fragments);
   - random component trees + resize sequences (including width 0/1).
2. **Differential fuzz targets** per layer: `visibleWidth`/slice (exhaustive
   over generated corpus), editor state machines, layout measurement,
   renderer write-streams (frame-hash compare + first-divergent-frame report).
3. **Intl.Segmenter vs Rust segmenter** corpus battering (the GB9c/GB11 risk).
4. **Shrinker**: on divergence, delta-debug the input to a minimal case, write
   it to `parity/regressions/` as a permanent case (auto-added to the suite).
5. **Scale gate**: nightly-style full run — every generated case × both sides,
   wall-clock budgeted, with a flakiness re-run pass (mirrors the existing
   "stable across repeated runs" discipline).
6. Metrics: case count, divergence count, first-divergence latency; tracked in
   TRACKING.md per tranche like the current parity counts.

## Gaps register (TUI-specific, extends TRACKING.md)

| ID | Area | Description |
|----|------|-------------|
| T1 | segmenter | `Intl.Segmenter` = ICU; Rust port is UAX #29 over V8-regex-dumped tables. Any ICU-vs-rules divergence (GB9c Indic conjuncts) must be found by differential fuzzing and patched; `Prepend` set not regex-exposable (sourced from UCD + verified) |
| T2 | marked | markdown token parity strategy undecided until T6 (see above) |
| T3 | native | clipboard native modules (Linux C prebuilds) — port the *behavior* (same external tools/commands), not the N-API binary |
| T4 | windows | upstream `native/win32` + `powershell` equivalents intentionally out of scope (matches G4) |
| T5 | timing | renderer timing paths (requestAnimationFrame-style batching, progress keepalive 1000 ms) are wall-clock dependent; parity cases pin timing via injected clock/short sequences only |
| T6 | pi-rust/ | the morning's `pi-rust/` scaffold (monorepo-style, unbuilt) is superseded by pi-rs; not deleted pending user confirmation |

## Test-count expectations

Upstream test files to port: ~55. Existing suite: 239 parity + 174 unit.
Target at end of T6: ≥ 600 parity cases (TUI modules + render-line +
write-stream), ≥ 400 Rust unit tests, plus the fuzz corpora.
