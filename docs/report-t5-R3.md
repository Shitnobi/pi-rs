# T5 Redteam R3 — Alt-screen selection + search

Date: 2026-10-18. Reviewer: coordinator. Method: line-by-line TS
(tui-alt-screen.ts selection 1128-1490, alt-screen-search.ts in full)
vs Rust (alt_screen_selection.rs, alt_screen_search.rs,
alt_screen_input.rs search integration); differential coverage
cross-checked against parity/wavec/alt-cases.mjs.

## Scope reviewed

### Selection
| TS | Rust | Verdict |
|---|---|---|
| getScrollSelectionPoint / getSelectionPoint (1128-1156) | get_selection_point / scroll variant | MATCH (visible clamp, maxContentRow, col clamp) |
| getSelectionSourceLine (1168-1174) | get_selection_source_line | MATCH |
| getWordSelection (1176-1211) | get_word_selection | MATCH — joiner = TERMINAL_WORD_SELECTION_JOINERS {"/","-"} (redundant but harmless extra matches! guard in Rust), segment column spans, canJoin walk both directions, boundary end point |
| getLineSelection (1213-1218) | get_line_selection | MATCH |
| updateSelectionFocus (1220-1238) | update_selection_focus | MATCH (anchor flip on target-before-initial) |
| getClickCount (1240-1256) | get_click_count | MATCH (row/scrollView/word-extent equality, 500ms virtual window, 1→2→3→1 cycle, no-word clears history) |
| updateSelectionAutoScroll / autoScrollSelection / stop (1266-1319) | update_selection_auto_scroll / auto_scroll_selection / stop | MATCH — incl. the scrollBy-remaining==direction stop rule and the pointer-clamped focus update. (Pump fired-flag bug found earlier this session and fixed: pump_screen_timers_until now reports selection fires.) |
| handleSelectionMouseEvent (1321-1398) | handle_selection_mouse_event | MATCH — release path (press-active gate, updateFocus, isClick, pressedUrl+openUrl best-effort, click re-dispatch through overlay then layout, copy-on-select), motion path (dragged, lastClick/pressedUrl clear, auto-scroll arm), press path (scroll-view pick gated on no overlay + layout, word/line range by click count, granularity, initial range, OSC8 pressedUrl via previousScreen clamp) |
| getSelectionBounds (1401-1416) | get_selection_bounds | MATCH |
| getSelectionColumns (1418-1440) | get_selection_columns | MATCH — grapheme cell range on start, boundary flag on end (min vs +1 fallback), min/max column clamps |
| getActiveSelectionText (1439-1460) | get_active_selection_text | MATCH — scrollContentLines vs previousScreen source, per-row slice, trimEnd, empty → None |
| copySelectionToClipboard / copyTextToClipboard (1462-1486) | copy paths | MATCH — injected copySelection result enum (true/string/false) models TS; OSC 52 fallback `\x1b]52;c;<base64>\x07` + "Copied!" flash |

### Search
| TS | Rust | Verdict |
|---|---|---|
| buildSearchCorpus (26-88) | build_search_corpus | MATCH — printable-ASCII fast path with per-word linearColumns spans, grapheme path otherwise, pendingSeparator single-space joins |
| normalizeQuery / escapeRegExp | normalize_query / (literal scan) | MATCH — the escaped-regex + matchAll is modeled as a case-folded literal scan (see findings) |
| findSearchCorpusMatches (116-148) | find_search_corpus_matches | MATCH — spanIndex advance, segment merge on same row + startCol ≤ prev endCol, linear vs grapheme column mapping |
| AltScreenSearchIndex cache (155-184) | AltScreenSearchIndex | MATCH |
| getAltScreenSearchMatchKey | get_alt_screen_search_match_key | MATCH |
| AltScreenSearchComponent (render/handleInput/focused/getNavigationDirectionAt/setHoveredNavigationDirection) | AltScreenSearchComponent | MATCH — button geometry (previous/next start/end, row 2), controls-width fallback to bare arrows, focus propagation to inner Input |
| toggleSearch / closeSearch / updateSearchQuery / navigateSearch (520-572) | toggle_search / close_search / update_search_query / navigate_search | MATCH — overlay anchor top-right, width 40%, minWidth 32, margin 1; parked-query bridge for the TS closure seam |
| handleSearchMouseEvent / getSearchNavigationDirectionAt (574-595) | handle_search_mouse_event | MATCH |
| refreshSearch (595-663) | refresh_search_impl | MATCH — blank-query reset, changed/retain early return, selectedKey exact-index recovery, query-mode binary search on anchorRow, next/previous modular arithmetic, retain clamp, reveal scroll (viewport/3, disableFollow), scrolled return |

## Findings

1. **[FIXED] updateSearchQuery negative-index fallback** — TS
   `search.matches[search.selectedIndex]` is `undefined` for a negative
   index (anchor falls back to scrollTop); Rust used
   `.get(selected_index.max(0))`, which would read `matches[0]`.
   Unreachable today (selectedIndex is -1 only while matches is empty —
   refreshSearch assigns them together), but the port now guards
   `selected_index >= 0` explicitly (alt_screen_input.rs). Same hardening
   applied to the reveal lookup in refresh_search_impl.
2. **[Documented] Case-insensitive matching seam** — TS uses
   `new RegExp(query, "giu")` (JS simple Unicode case folding); the Rust
   port folds with `char::to_lowercase` plus a fold-position map. These
   differ for exotic code points (e.g. U+0130 `İ` full-lowercases to
   `i`+combining dot, so a folded `i` can match where JS /iu would not).
   Transcript search parity is unaffected for all realistic content; no
   case in the upstream test suite or parity suite distinguishes them.
3. **openUrl panic propagation** — TS wraps `openUrl` in try/catch; Rust
   documents that a panicking callback propagates. Harness callbacks are
   infallible; acceptable divergence, documented in source.
4. **selection-autoscroll pump** — the fired-flag bug (advance_clock
   defensive break stopping the interval after one tick) was found and
   fixed during Wave C triage (commit 187510c); re-verified here as part
   of this review.

## Differential coverage

alt-cases.mjs selection cases (character/word/line selection, drag,
multi-click, copy-on-select with OSC52 in the write stream, autoscroll
edge) and search cases (search-navigate-buttons, search-boxdrawing-not-
button, search-match-styles, search-editor-focus-restore,
search-scroll-while-focused) run byte-identical on both sides
(665/665 overall).

## Verdict

**One latent-seam hardening applied (unreachable state, now exact JS
semantics); no reachable behavioral divergences. R3 clean.**
