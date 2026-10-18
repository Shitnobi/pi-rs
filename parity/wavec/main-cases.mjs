// T5 Wave C differential parity cases — tui-main screen.
//
// Ports of the upstream TS tests in /tmp/pi-repo/packages/tui/test:
//   - tui-render.test.ts   (render scheduling, resize, content shrinkage,
//                            differential rendering, fake cursor)
//   - tui-shrink.test.ts    (content shrinking to zero)
//   - tui-cell-size-input.test.ts (cell-size response consumption, bare ESC)
//
// The upstream tests mutate a raw TestComponent's `lines` array between
// requestRender() calls. The parity harness only builds components from the
// tui-components vocabulary, so each mutation lever is mapped to the closest
// in-vocabulary mechanism:
//   - "component.lines = [...]"     -> input typing (value line changes),
//                                       loader spinner frames (middle line),
//                                       select/settings list navigation and
//                                       search filtering (line sets change)
//   - content growth/shrink         -> showOverlay/hideOverlay (overlays pad
//                                       the composited line count to terminal
//                                       height, so hiding one shrinks content)
//   - tui.clear()                   -> overlay over an empty base, then hide
// Upstream tests that require raw escape/kitty-image injection, PI_TUI_DEBUG
// env, TERMUX env, or the over-wide-line crash path are intentionally skipped
// (out of the Wave C contract, see wavec-helpers.mjs header).

import {
	tm, txt, ttxt, spc, boxc, ent, hstk, vstk, inp, slist, stlist, ld, cld, svw,
	sitem, sit, start, stop, key, reply, tick, resize, focus, addChild,
	showOverlay, hideOverlay, requestRender, renderNow,
} from "../wavec-helpers.mjs";

const lines = (n, prefix = "L") => Array.from({ length: n }, (_, i) => `${prefix}${i}`).join("\n");

export const mainCases = [
	// ---------------------------------------------------------------------------
	// tui-render.test.ts — "TUI render scheduling"
	// "renders keyboard input without waiting for a throttled frame":
	// a queued throttled render must be preempted by keyboard input, which
	// renders synchronously (one frame per key, no tick needed in between).
	tm("input-priority", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		requestRender(), // queue a throttled render...
		key("a"), key("b"), key("c"), // ...keyboard input preempts it
		tick(20),
		stop(),
	], { cols: 40, rows: 10 }),

	// ---------------------------------------------------------------------------
	// tui-render.test.ts — "TUI resize handling"
	// "triggers full re-render when terminal height changes"
	tm("resize-height-full-redraw", [
		addChild(txt("Line 0\nLine 1\nLine 2", 0, 0)),
		start(),
		tick(20),
		resize(40, 15),
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// "triggers full re-render when terminal width changes"
	tm("resize-width-full-redraw", [
		addChild(txt("Line 0\nLine 1\nLine 2", 0, 0)),
		start(),
		tick(20),
		resize(60, 10),
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// Width-driven reflow: a wrapping Text occupies fewer lines after the
	// terminal widens, so rows below the new content must be cleared
	// (clearOnShrink path on top of the resize full redraw).
	tm("resize-wrap-reflow-shrink", [
		addChild(txt("alpha bravo charlie delta echo foxtrot golf hotel", 1, 0)),
		start(),
		tick(20),
		resize(40, 10),
		tick(40),
		stop(),
	], { cols: 20, rows: 10, clearOnShrink: true }),

	// ---------------------------------------------------------------------------
	// tui-render.test.ts — "TUI content shrinkage"
	// "clears empty rows when content shrinks significantly" (6 lines -> 2):
	// base content is 2 lines; an overlay pads the composited content to the
	// full terminal height; hiding it shrinks content back to 2 lines and the
	// vacated rows must be cleared by a full redraw.
	tm("shrink-clear-empty-rows", [
		addChild(txt("Line 0", 0, 0)),
		addChild(txt("Line 1", 0, 0)),
		showOverlay(txt(lines(4, "extra"), 0, 0), { row: 2, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		stop(),
	], { cols: 40, rows: 10, clearOnShrink: true }),

	// "handles shrink to single line"
	tm("shrink-to-single-line", [
		addChild(txt("Only line", 0, 0)),
		showOverlay(txt("a\nb\nc", 0, 0), { row: 1, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		stop(),
	], { cols: 40, rows: 10, clearOnShrink: true }),

	// "handles shrink to empty" (tui-render.test.ts) and
	// "clears all rendered lines when content shrinks to zero"
	// (tui-shrink.test.ts — upstream uses tui.clear(), which the harness
	// cannot call; the closest in-vocabulary equivalent is an overlay over an
	// empty base that is then hidden).
	tm("shrink-to-empty", [
		showOverlay(txt("first\nsecond\nthird", 0, 0), { row: 0, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		stop(),
	], { cols: 40, rows: 10, clearOnShrink: true }),

	// ---------------------------------------------------------------------------
	// tui-render.test.ts — "TUI differential rendering"
	// "renders correctly when only a middle line changes (spinner case)":
	// a Loader animates its indicator frame on an interval while the lines
	// above and below stay identical. (The harness builds the Loader without
	// a TUI reference, so each animation step is flushed with an explicit
	// requestRender — the upstream test's terminal.waitForRender().)
	tm("diff-spinner-middle-line", [
		addChild(txt("Header", 0, 0)),
		addChild(ld({ message: "Working", frames: ["|", "/", "-", "\\"], intervalMs: 50 })),
		addChild(txt("Footer", 0, 0)),
		start(),
		tick(20),
		tick(70), // spinner advances to "/"
		requestRender(),
		tick(20),
		tick(70), // "-"
		requestRender(),
		tick(20),
		tick(70), // "\\"
		requestRender(),
		tick(20),
		tick(70), // back to "|"
		requestRender(),
		tick(20),
		stop(),
	], { cols: 40, rows: 10 }),

	// "renders correctly when first line changes but rest stays same"
	tm("diff-first-line-change", [
		addChild(inp({})),
		addChild(txt("Line 1", 0, 0)),
		addChild(txt("Line 2", 0, 0)),
		focus("c0"),
		start(),
		tick(20),
		key("X"),
		tick(20),
		requestRender(true), // forced full redraw after a differential update
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// "renders correctly when last line changes but rest stays same"
	tm("diff-last-line-change", [
		addChild(txt("Line 0", 0, 0)),
		addChild(txt("Line 1", 0, 0)),
		addChild(inp({})),
		focus("c2"),
		start(),
		tick(20),
		key("Z"),
		tick(20),
		stop(),
	], { cols: 40, rows: 10 }),

	// "renders correctly when multiple non-adjacent lines change": two inputs
	// separated by a static text line; typing into each produces updates that
	// skip over the untouched line in between.
	tm("diff-non-adjacent-lines", [
		addChild(inp({})),
		addChild(txt("middle", 0, 0)),
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("x"),
		focus("c2"),
		key("y"),
		tick(20),
		stop(),
	], { cols: 40, rows: 10 }),

	// "tracks cursor correctly when content shrinks with unchanged remaining
	// lines": shrink the content (hide a non-capturing overlay), then change
	// line 0 — the differential update must land on the right row.
	tm("diff-cursor-tracking-after-shrink", [
		addChild(inp({})),
		addChild(txt("Line 2", 0, 0)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(txt("a\nb\nc", 0, 0), { row: 2, col: 0, nonCapturing: true }),
		tick(40),
		hideOverlay(),
		tick(40),
		key("C"),
		tick(20),
		stop(),
	], { cols: 40, rows: 10 }),

	// "handles transition from content to empty and back to content"
	tm("diff-empty-and-back", [
		showOverlay(txt("Line 0\nLine 1\nLine 2", 0, 0), { row: 0, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		addChild(txt("New Line 0", 0, 0)),
		addChild(txt("New Line 1", 0, 0)),
		requestRender(),
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// "full re-renders when deleted lines move the viewport upward":
	// a 12-line overlay on a 5-row terminal scrolls the viewport; hiding it
	// deletes the lines underneath and must reset the viewport with a full
	// redraw.
	tm("viewport-upward-full-redraw", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt(lines(12), 0, 0), { row: 0, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		stop(),
	], { cols: 20, rows: 5 }),

	// "appends after a shrink without another full redraw once the viewport is
	// reset": shrink (full redraw), then grow again — the append must stay on
	// the differential path.
	tm("append-after-shrink", [
		addChild(txt("Line 0", 0, 0)),
		addChild(txt("Line 1", 0, 0)),
		showOverlay(txt(lines(8), 0, 0), { row: 0, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("Line 2", 0, 0), { row: 2, col: 0 }),
		tick(40),
		stop(),
	], { cols: 20, rows: 5 }),

	// "clears stale content when maxLinesRendered was inflated by a transient
	// component": height churn (tall overlay -> hide -> shorter overlay ->
	// hide) around content changes; stale rows from the inflated high-water
	// mark must not survive.
	tm("stale-content-transient-inflation", [
		addChild(inp({})),
		focus("c0"),
		showOverlay(txt(lines(12), 0, 0), { row: 0, col: 0 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt(lines(8, "sel"), 0, 0), { row: 0, col: 0, nonCapturing: true }),
		tick(40),
		key("k"),
		tick(20),
		hideOverlay(),
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// "resets styles after each rendered line": a styled (italic) first line
	// must not bleed its SGR state into the following plain line.
	tm("style-reset-per-line", [
		addChild(txt("\x1b[3mItalic\nPlain", 0, 0)),
		start(),
		tick(20),
		stop(),
	], { cols: 20, rows: 6 }),

	// renderNow() / renderNow(true) — synchronous renders (used by the
	// upstream bounded-output tests; here just the differential vs forced
	// full-redraw paths with unchanged content).
	tm("render-now-forced", [
		addChild(txt("Line 0", 0, 0)),
		start(),
		tick(20),
		renderNow(),
		renderNow(true),
		stop(),
	], { cols: 40, rows: 10 }),

	// ---------------------------------------------------------------------------
	// tui-render.test.ts — "TUI fake cursor"
	// "draws the focused fake cursor in reverse video" (hardware cursor
	// hidden — default): focused input, typed value "abc".
	tm("fake-cursor-focused-reverse", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("a"),
		key("b"),
		key("c"),
		tick(20),
		stop(),
	], { cols: 20, rows: 5 }),

	// "...and omits it when the hardware cursor is shown": same scenario with
	// showHardwareCursor — the focused input's fake cursor is suppressed and
	// the hardware cursor is positioned instead.
	tm("fake-cursor-focused-hardware", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("a"),
		key("b"),
		key("c"),
		tick(20),
		stop(),
	], { cols: 20, rows: 5, showHardwareCursor: true }),

	// "keeps the fake cursor of an unfocused input when the hardware cursor is
	// shown": an input that is neither screen-focused nor component-focused
	// still renders its own reverse-video fake cursor, which the screen keeps
	// (only the *focused* component's cursor is suppressed in hardware-cursor
	// mode).
	tm("fake-cursor-unfocused-hardware", [
		addChild(inp({})),
		start(),
		tick(20),
		stop(),
	], { cols: 20, rows: 5, showHardwareCursor: true }),

	// "keeps the cursor when an overlay ends directly left of it": a 3-column
	// overlay at row 0 / col 0 composites over the start of the input line
	// while the input's cursor sits to the right of it.
	tm("overlay-left-of-cursor", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("abc"),
		showOverlay(txt("XXX", 0, 0), { row: 0, col: 0, width: 3, nonCapturing: true }),
		tick(40),
		stop(),
	], { cols: 20, rows: 5 }),

	// Same scenario with the hardware cursor shown (the upstream test loops
	// over both modes).
	tm("overlay-left-of-cursor-hardware", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("abc"),
		showOverlay(txt("XXX", 0, 0), { row: 0, col: 0, width: 3, nonCapturing: true }),
		tick(40),
		stop(),
	], { cols: 20, rows: 5, showHardwareCursor: true }),

	// ---------------------------------------------------------------------------
	// tui-cell-size-input.test.ts
	// "consumes cell size responses and still forwards later user input":
	// a CSI 6 ; height ; width t reply is consumed by the screen (never
	// reaches the focused input; triggers an invalidate + render), and a
	// later "q" is forwarded normally.
	tm("cellsize-response-consumed", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		reply("\x1b[6;20;10t"),
		tick(40),
		key("q"),
		tick(20),
		stop(),
	], { cols: 80, rows: 24 }),

	// "forwards bare escape even when a cell size query was sent at startup":
	// a lone ESC must be delivered to the focused component as-is (the input's
	// cancel handler is a no-op here) instead of being swallowed or merged
	// with the following keystroke.
	tm("cellsize-bare-escape", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("a"),
		key("b"),
		tick(20),
		key("\x1b"),
		key("c"),
		tick(20),
		stop(),
	], { cols: 80, rows: 24 }),

	// ---------------------------------------------------------------------------
	// Component-vocabulary coverage exercised by the upstream files' component
	// trees (text/stacks/boxes/padding/truncation/scroll-view rendering).
	tm("layouts-boxes-stacks", [
		addChild(hstk([ent(txt("left", 1, 0)), ent(txt("mid", 1, 0)), ent(txt("right", 1, 0))], { gap: 1 })),
		addChild(vstk([ent(txt("a", 0, 0)), ent(spc(2)), ent(txt("b", 0, 0))], { gap: 0 })),
		addChild(boxc([txt("in box", 1, 1)], 2, 2)),
		addChild(ttxt("truncated text that is quite long indeed", 1, 1)),
		addChild(svw(txt(lines(10), 0, 0), { follow: "end", scrollbar: "auto" })),
		start(),
		tick(20),
		stop(),
	], { cols: 40, rows: 14 }),

	// Overlay anchors / sizing (the resolveOverlayLayout matrix: anchors,
	// percent width, maxHeight, margin, offsets).
	tm("overlay-anchors-sizes", [
		addChild(txt("base line", 0, 0)),
		showOverlay(boxc([txt("centered", 0, 0)], 0, 0), { anchor: "center", width: 20 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(boxc([txt("br")], 0, 0), { anchor: "bottom-right", width: "25%", maxHeight: 3 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(boxc([txt("tl")], 0, 0), { anchor: "top-left", offsetX: 2, offsetY: 1, margin: 2 }),
		tick(40),
		hideOverlay(),
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// Overlay focus capture + restore: a capturing overlay takes focus while
	// shown (arrow keys drive its select list) and hideOverlay restores focus
	// to the previous input.
	tm("overlay-captures-focus", [
		addChild(inp({})),
		focus("c0"),
		start(),
		tick(20),
		key("a"),
		tick(20),
		showOverlay(
			slist([sitem("one", "One"), sitem("two", "Two"), sitem("three", "Three")], { maxVisible: 3 }),
			{ anchor: "center", width: 20 },
		),
		tick(40),
		key("\x1b[B"),
		tick(20),
		key("\x1b[B"),
		tick(20),
		hideOverlay(),
		tick(40),
		key("b"),
		tick(20),
		stop(),
	], { cols: 40, rows: 10 }),

	// Focus + input on a select list: selection movement and window scrolling
	// rewrite the visible lines (differential multi-line updates).
	tm("select-list-navigation", [
		addChild(slist(
			[sitem("a", "Alpha"), sitem("b", "Beta"), sitem("c", "Gamma"), sitem("d", "Delta"), sitem("e", "Epsilon")],
			{ maxVisible: 3 },
		)),
		focus("c0"),
		start(),
		tick(20),
		key("\x1b[B"),
		tick(20),
		key("\x1b[B"),
		tick(20),
		key("\x1b[B"), // scrolls the window past the end
		tick(20),
		key("\x1b[A"),
		tick(20),
		stop(),
	], { cols: 40, rows: 12 }),

	// Settings list: navigation, value cycling, and search filtering (the
	// search shrinks the rendered line set — clearOnShrink clears the rows
	// vacated by the filter).
	tm("settings-list-values-search", [
		addChild(stlist(
			[
				sit("theme", "Theme", "dark", { values: ["dark", "light"] }),
				sit("editor", "Editor", "vim"),
				sit("wrap", "Wrap Lines", "on", { values: ["on", "off"] }),
				sit("mouse", "Mouse", "off"),
			],
			{ maxVisible: 4, enableSearch: true },
		)),
		focus("c0"),
		start(),
		tick(20),
		key("\x1b[B"), // move to Editor
		tick(20),
		key("\x1b[B"), // move to Wrap Lines
		tick(20),
		key(" "), // cycle wrap value on -> off
		tick(20),
		key("w"), // search "w": only Wrap Lines matches (content shrink)
		tick(40),
		key("r"), // search "wr"
		tick(20),
		key("\x7f"), // backspace: "w"
		tick(20),
		key("\x1b"), // cancel routing (no visual change)
		tick(20),
		stop(),
	], { cols: 60, rows: 14, clearOnShrink: true }),

	// Cancellable loader: interval animation plus Escape-driven abort routing.
	// (Animation steps flushed with explicit requestRender, as above.)
	tm("cancellable-loader-escape", [
		addChild(cld({ message: "Working" })),
		focus("c0"),
		start(),
		tick(20),
		tick(90),
		requestRender(),
		tick(20),
		tick(90),
		requestRender(),
		tick(20),
		key("\x1b"),
		tick(40),
		stop(),
	], { cols: 40, rows: 10 }),

	// Teardown: stop() with preserveScreen exercises the alternate teardown
	// write path.
	tm("stop-preserve-screen", [
		addChild(txt("content", 0, 0)),
		start(),
		tick(20),
		stop({ preserveScreen: true }),
	], { cols: 40, rows: 10 }),
];
