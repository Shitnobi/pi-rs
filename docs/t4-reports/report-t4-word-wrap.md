# T4-D2 progress log — editor_word_wrap.rs

Delegate D2: pure word-wrap / paste-marker-segmentation functions.

## Log

- 13:05 — Read T4_PLAN.md §4, skeleton, editor.ts:1-216, utils.ts (cjkBreakRegex,
  isWhitespaceChar, visibleWidth), word-break API (`WordSegment { segment,
  is_word_like }` — no index field, so indices are accumulated via
  `utils::utf16_len`), test conventions in select_list.rs.
- 13:05 — Key semantic decisions (from source):
  - `isPasteMarker` = `segment.length >= 10 && /^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$/.test(segment)`
    — shape only; ID validity is the caller's job. Marker text is pure ASCII,
    and greedy `\d+` backtracking can never rescue a failed match (a shorter
    digit run is always followed by a digit, which can neither start the
    ` (…)` group nor be `]`), so a greedy-only manual matcher is exact.
  - `segmentWithMarkers` merge (editor.ts:71-86): the merged segment object is
    `{segment, index, input}` — it has **no** `isWordLike` property, so a merged
    marker word segment gets `is_word_like: false` (TS `undefined` → falsy).
  - word granularity: `WordSegment` has no index → indices accumulated
    per-segment in UTF-16 units while merging.
  - `cjkBreakRegex.test(grapheme)` is an unanchored test over the whole
    grapheme → "any code point in the segment is CJK-script". utils.rs has this
    as private `cjk_break_test`; reimplemented privately here (as instructed)
    on top of `crate::unicode::is_cjk_script`.
  - `isWhitespaceChar(grapheme)` is likewise `/\s/.test(grapheme)` → "any char
    in the segment is JS-whitespace" (crate::utils::is_whitespace_char over
    chars).
  - wordWrapLine widths tracked as i64 internally (TS number; avoids usize
    underflow panics where TS would produce a negative → false comparison).
- 13:10 — Writing implementation of all four functions + inline test module
  (16 wordWrapLine ports from editor.test.ts:974-1195, direct regression port of
  the 4124 overflow-recheck case as a pre-segmented wordWrapLine call, plus
  is_paste_marker / segment_graphemes_with_markers / segment_words_with_markers
  unit tests modeled on editor.test.ts:3815-4066 observable behavior).
- 13:12 — First full run: 26/30 passed; 4 failures were my own expected-index
  arithmetic (the upstream TS test comment "// 21 chars" for
  "[paste #1 +20 lines]" is wrong — `marker.length` is 20; upstream tests use
  `marker.length` so they pass regardless). Fixed expectations; 30/30 pass.
- 13:15 — Bit-exactness verified against the **live TS implementation**
  (node on /tmp/pi-repo):
  - wordWrapLine battery (40 cases: CJK, emoji, fullwidth, U+3000 ideographic
    space, tabs, ZWSP, invalid-ID marker text, pre-segmented merged marker,
    widths 1–12): **40/40 identical** chunk arrays incl. UTF-16 indices.
  - segmentation battery (66 cases: 11 texts × 6 valid-ID sets, grapheme +
    word granularity): **identical** except merged markers show
    `isWordLike: undefined` (JSON `null`) in TS vs `is_word_like: false` in
    Rust — the intended encoding of a falsy missing property (the TS merged
    object `{segment, index, input}` has no `isWordLike`; all consumers test
    truthiness). All other segments, indices and isWordLike values match.
  - Also confirmed exact chunk arrays for the 4124 overflow-recheck regression
    case and the oversized-atomic-segment cases (upstream asserts only
    invariants there; my ported tests assert the exact arrays TS produces).
- 13:18 — **Latent upstream bug documented (not introduced by port):**
  `wordWrapLine` recurses infinitely when a single grapheme's visible width
  exceeds `maxWidth` (e.g. `"你"` at width 1, or `"\t"` at width 2): the
  oversized-segment branch calls `wordWrapLine(grapheme, maxWidth)`, which
  re-segments the same single grapheme and re-enters the branch forever.
  Verified live: TS stack-overflows (RangeError). The Rust port replicates
  the algorithm faithfully and likewise overflows the stack — kept for
  behavioral fidelity (upstream editor tests never hit it: render widths in
  tests are ≥ 8 and markers are ASCII). Flagged here for the coordinator; a
  divergence guard would need a deliberate decision (TS parity vs safety).
- 13:20 — Removed the temporary parity-dump tests; final file is 1042 lines,
  rustfmt-clean (edition 2021), clippy-clean for this module (pre-existing
  warnings in other modules untouched).
- 13:21 — Final gates: `cargo test -p pi-tui --lib editor_word_wrap` →
  30/30 pass; full `cargo test -p pi-tui --lib` → 613 passed, 0 failed,
  1 ignored (pre-existing). No other file edited; no git commit run.

## Result

- `crates/pi-tui/src/components/editor_word_wrap.rs` complete:
  - `is_paste_marker` — PASTE_MARKER_SINGLE port (greedy-only manual matcher
    proven equivalent to the regex incl. backtracking collapse).
  - `segment_graphemes_with_markers` / `segment_words_with_markers` —
    segmentWithMarkers port with UTF-16 indices, valid-ID filtering,
    atomic merge; merged word markers carry `is_word_like: false`
    (= TS `undefined`).
  - `word_wrap_line` — full editor.ts:124-216 port: lineWidth fast path,
    wrap opportunities (whitespace→non-ws, paste-marker next, CJK adjacency
    via reimplemented `cjk_break_test`), overflow backtracking, force-break,
    oversized-atomic-segment grapheme re-wrap recursion, final chunk.
  - 30 inline tests: all 16 upstream wordWrapLine tests (editor.test.ts
    974-1195), the 4124 overflow-recheck regression (as an equivalent direct
    pre-segmented call, documented), is_paste_marker shape tests, and
    grapheme/word marker-segmentation unit tests derived from
    editor.test.ts:3815-4066 observable behavior.
- Known issue for coordinator: single-grapheme-wider-than-maxWidth infinite
  recursion (upstream bug, faithfully replicated — see log 13:18).
