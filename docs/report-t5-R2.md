# T5 Redteam R2 — Main-screen diff algorithm vs write stream

Date: 2026-10-18. Reviewer: coordinator. Method: line-by-line TS
(tui-main-screen.ts doRender 247-556, tui.ts render scheduler 988-1050)
vs Rust (crates/pi-tui/src/tui_main_screen.rs do_render/full_render/
position_hardware_cursor, tui.rs request scheduler).

## Scope reviewed

| TS | Rust | Verdict |
|---|---|---|
| doRender preamble (viewport math, widthChanged/heightChanged, previousBufferLength) | do_render 489-524 | MATCH — prevViewportTop recompute on height change identical |
| resolveFakeCursors → compositeOverlays → extractCursorPosition → applyLineResets order | same order in do_render | MATCH (hasOverlayEntries == overlayStack.length > 0, tui.ts:538) |
| fullRender(clear) (278-318) | full_render | MATCH — 2026 begin, kitty delete + 2J/H/3J on clear, image-reserved-rows branch (incl. `imageReservedRows <= height` gate), 2026 end, cursorRow/hardwareCursorRow, maxLinesRendered reset-vs-max, previousViewportTop = max(0, bufferLength - height), positionHardwareCursor, previous* bookkeeping |
| first-render / widthChanged / heightChanged && !isTermuxSession / clearOnShrink early returns | same four gates in order | MATCH (incl. logRedraw reasons) |
| first/last changed scan + appendedLines adjust + kitty expandChangedRange + appendStart | 605-640 | MATCH |
| no-changes branch (positionHardwareCursor + prev bookkeeping only) | 640-646 | MATCH |
| deleted-lines branch (targetRow < prevViewportTop → full; extraLines > height → full; move, clear loop with 1B stepping, moveBack) | 646-712 | MATCH |
| firstChanged < prevViewportTop → full | 713-726 | MATCH |
| scroll-down preamble (moveTargetRow > prevViewportBottom: clamp currentScreenRow, moveToBottom, \r\n repeat scroll, viewport adjust) | 727-757 | MATCH |
| lineDiff movement + appendStart \r\n vs \r | 758-770 | MATCH |
| changed-lines loop (kitty image pre-clear scroll guard → full; crash-log + stop + throw on width overflow) | 771-852 | MATCH — Rust panics with the same message; crash log written to log_directory ?? tmp |
| shrink tail (moveDown to end, extra \r\n 2K loop, moveBack extraLines A) | 854-871 | MATCH |
| cursor tracking (cursorRow = len-1, hardwareCursorRow = finalCursorRow, maxLinesRendered, previousViewportTop = max(prev, finalCursorRow-height+1)) | 873-890 | MATCH |
| positionHardwareCursor (626-656) | position_hardware_cursor | MATCH — null/empty → hideCursor; clamp row; rowDelta B/A; col+1 G; show/hide by getShowHardwareCursor |
| requestRender(force) / renderNow(force) resetRenderState gating (tui.ts:994, 1003) | request_render/render_now call reset_render_state_hook on force; request_render_state(true) (requestImmediateRender path) does NOT — matches TS | MATCH |
| requestImmediateRender nextTick (cancel timer, immediateRenderScheduled gate, stopped/renderRequested check) | NextTickKind::ImmediateRender queue | MATCH (exercised by parity case tui-main/input-priority) |

## Detailed findings

1. **positionHardwareCursor col clamp** — TS computes
   `targetCol = Math.max(0, cursorPos.col)`; Rust uses `col as i64` with no
   clamp. Unreachable in practice: `extractCursorPosition` derives the
   column from `visibleWidth(prefix)`, always >= 0. No behavioral gap;
   noting for completeness. No fix made.
2. **BoundedTerminalWriter chunking** — TS and Rust both buffer and flush
   in bounded chunks; chunk boundaries are invisible to the recorded write
   stream (concatenated), so no parity surface. MATCH.
3. **Crash path** — TS throws an Error after stop(); Rust panics with the
   same message after stop(). Harness-level difference only (process
   abort vs exception); the write stream up to that point is identical.
   Acceptable divergence, documented.
4. **render scheduler** — requestRender(force) → resetRenderState +
   requestImmediateRender; non-force → nextTick(scheduleRender) with the
   16ms throttle; renderNow(force) resets state. All three paths match,
   including the immediate-render preemption of a queued throttled frame
   (tui.ts:1023-1025 comment). Parity cases diff-spinner-* and
   input-priority exercise the throttle/immediate interplay byte-identically.

## Differential coverage

main-cases.mjs (32 cases: tui-render/tui-shrink/tui-cell-size-input
ports) + style-leak cases exercise: first render, incremental diffs,
append paths, shrink/clearOnShrink, resize width/height, cell-size
queries, hardware cursor positioning (incl. the \x1b[3G writes), loader
frame diffs. All pass byte-identical (665/665 overall).

## Verdict

**No behavioral divergences found. R2 clean.** Two documented
non-behavioral notes (col clamp unreachable; panic vs throw on the
crash path). No code changes made.
