// T5 Wave C differential parity cases — interaction-focused scenarios.
//
// Ports of the upstream TS tests in /tmp/pi-repo/packages/tui/test:
//   - wheel-scroll.test.ts          (WheelScrollAccelerator behaviors, driven
//                                    through real SGR wheel sequences and the
//                                    virtual clock instead of unit-level
//                                    accelerator.next() calls)
//   - tui-overlay-style-leak.test.ts (SGR reset dropped past the last visible
//                                    column / by overlay slicing)
//   - overlay-options.test.ts        (overlay geometry: anchors, percent
//                                    sizes, margins, offsets, maxHeight,
//                                    absolute positioning, stacked overlays,
//                                    content wider than the declared width)
//   - overlay-non-capturing.test.ts  (focus preservation / input routing with
//                                    non-capturing overlays)
//
// Adaptations required by the harness contract:
// - Upstream unit tests construct WheelScrollAccelerator directly with
//   explicit timestamps; here the same timing matrices are replayed as SGR
//   wheel events separated by tickMs() (the virtual clock backs
//   performance.now() inside the alt screen). The alt screen's default is
//   wheelScrollLines: 1, so "auto" cases pass it via altOptions. The
//   accelerate=false variant ("terminal already accelerates", macOS local)
//   cannot be forced through altOptions and is intentionally skipped.
// - Upstream StaticOverlay/StaticLines raw components map to txt() with
//   (0,0) padding; raw SGR/OSC-8 bytes inside the text exercise the same
//   ANSI-aware wrap/composite paths (wrapTextWithAnsi/visibleWidth).
// - Overlay-handle APIs (handle.focus()/unfocus()/setHidden()/hide(),
//   dynamic visible callbacks, unfocus targets) are out of contract; only
//   the portable subset is ported: nonCapturing creation, hideOverlay()
//   (topmost), setFocus(base ref|null) and raw input routing.
// - Focus routing is observed through the components it drives: the Input
//   editor (typed characters) and SelectList overlays (arrow-key selection
//   movement) rewrite rendered lines, so the write-stream differential
//   shows where each keystroke landed.

import {
	tm, ta, txt, slist, sitem,
	start, stop, key, tick, focus, addChild, showOverlay, hideOverlay,
	scrollBy,
} from "../wavec-helpers.mjs";

const lines = (count, prefix = "line ") =>
	Array.from({ length: count }, (_, index) => `${prefix}${index + 1}`).join("\n");

// SGR mouse wheel (1-based col;row from the top-left): 64 = wheel up, 65 = wheel down.
const wheelUp = (col, row) => key(`\x1b[<64;${col};${row}M`);
const wheelDown = (col, row) => key(`\x1b[<65;${col};${row}M`);
const wd = () => wheelDown(1, 1);
const wu = () => wheelUp(1, 1);
// The alt screen follows the end of the transcript, so the viewport starts
// at the bottom; park it at the top first so timed wheel-down events
// actually move it (scrollBy is clamped, -100 => viewportTop 0).
const toTop = () => scrollBy(-100);

const overlayList = (prefix, count) =>
	slist(Array.from({ length: count }, (_, i) => sitem(`${prefix}${i}`, `${prefix} item ${i}`)), {
		maxVisible: count,
	});

export const interactionCases = [
	// ---------------------------------------------------------------------------
	// wheel-scroll.test.ts — "WheelScrollAccelerator"
	// "uses fixed line counts regardless of timing": an isolated (or slow)
	// wheel spin moves one line per event in auto mode — notches 150–250ms
	// apart never reach the 100ms reference velocity, so every event moves
	// exactly 1 line (viewportTop 4 after 4 events).
	ta("wheel-auto-slow-notches-one-line", [
		addChild(txt(lines(30), 0, 0)),
		start(),
		tick(20),
		toTop(),
		tick(20),
		wd(), tick(250),
		wd(), tick(250),
		wd(), tick(150),
		wd(), tick(150),
	], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: "auto" } }),

	// "scales auto mode with wheel velocity": notches 20ms apart (fast spin)
	// move 1 then 5 lines per event (REFERENCE_GAP 100ms / 20ms avg = 5,
	// capped at MAX_AUTO_LINES 6) — viewportTop 16 after 4 events.
	ta("wheel-auto-velocity-scale", [
		addChild(txt(lines(30), 0, 0)),
		start(),
		tick(20),
		toTop(),
		tick(20),
		wd(), tick(20),
		wd(), tick(20),
		wd(), tick(20),
		wd(), tick(20),
	], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: "auto" } }),

	// "does not accelerate bursts of events for a single notch": events 3ms
	// apart (under BURST_GAP_MS 5) belong to one high-resolution notch and
	// move one line each — no acceleration.
	ta("wheel-auto-burst-single-notch", [
		addChild(txt(lines(30), 0, 0)),
		start(),
		tick(20),
		toTop(),
		tick(20),
		wd(), tick(3),
		wd(), tick(3),
		wd(), tick(3),
		wd(), tick(20),
	], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: "auto" } }),

	// "resets acceleration on direction changes and pauses": three fast
	// down-notches (1,5,5), a direction flip to up (1, opposite direction),
	// then after a 440ms pause (> GESTURE_GAP_MS 200) a fresh down gesture
	// starts over at 1 and re-accelerates to 5. Net viewportTop 16
	// (17 down, 1 up).
	ta("wheel-auto-direction-and-pause-reset", [
		addChild(txt(lines(30), 0, 0)),
		start(),
		tick(20),
		toTop(),
		tick(20),
		wd(), tick(20),
		wd(), tick(20),
		wd(), tick(20),
		wu(), tick(440),
		wd(), tick(20),
		wd(), tick(20),
	], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: "auto" } }),

	// "carries fractional lines between events": notches 40ms apart average
	// 2.5 lines/event; the fractional remainder carries over, producing the
	// 1,2,3,2,3 line sequence (viewportTop 11).
	ta("wheel-auto-fractional-carry", [
		addChild(txt(lines(30), 0, 0)),
		start(),
		tick(20),
		toTop(),
		tick(20),
		wd(), tick(40),
		wd(), tick(40),
		wd(), tick(40),
		wd(), tick(40),
		wd(), tick(40),
	], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: "auto" } }),

	// "uses fixed line counts regardless of timing" (fractional variant):
	// wheelScrollLines: 0.5 floors to 0 and is clamped up to 1, so even a
	// fast 20ms spin moves one line per event — viewportTop 4.
	ta("wheel-fractional-lines-clamp-one", [
		addChild(txt(lines(30), 0, 0)),
		start(),
		tick(20),
		toTop(),
		tick(20),
		wd(), tick(20),
		wd(), tick(20),
		wd(), tick(20),
		wd(), tick(20),
	], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: 0.5 } }),

	// ---------------------------------------------------------------------------
	// tui-overlay-style-leak.test.ts — "TUI overlay compositing"
	// "should not leak styles when a trailing reset sits beyond the last
	// visible column (no overlay)": an italic SGR span covers the full 20-col
	// row so its reset lands past the last visible column; the row below
	// ("INPUT") must not inherit the italic state.
	tm("style-leak-reset-beyond-width", [
		addChild(txt("\x1b[3m" + "X".repeat(20) + "\x1b[23m\nINPUT", 0, 0)),
		start(),
		tick(20),
	], { cols: 20, rows: 6 }),

	// "should not leak styles when overlay slicing drops trailing SGR
	// resets": same styled base line, with a 3-column overlay compositing
	// over the middle of it; slicing the base line must not drop the reset
	// and leak italic onto the "INPUT" row below.
	tm("style-leak-overlay-slicing", [
		addChild(txt("\x1b[3m" + "X".repeat(20) + "\x1b[23m\nINPUT", 0, 0)),
		showOverlay(txt("OVR", 0, 0), { row: 0, col: 5, width: 3 }),
		start(),
		tick(20),
	], { cols: 20, rows: 6 }),

	// ---------------------------------------------------------------------------
	// overlay-options.test.ts — "anchor positioning" / "margin" / "offset"
	// The full anchor matrix not already covered by main-cases.mjs
	// (top-left, top-center, bottom-left, top-right, bottom-center,
	// left-center, right-center), each shown then hidden so every placement
	// gets its own composited frame.
	tm("overlay-anchor-matrix", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("TL", 0, 0), { anchor: "top-left", width: 10 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("TC", 0, 0), { anchor: "top-center", width: 10 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("BL", 0, 0), { anchor: "bottom-left", width: 10 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("TR", 0, 0), { anchor: "top-right", width: 10 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("BC", 0, 0), { anchor: "bottom-center", width: 10 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("LC", 0, 0), { anchor: "left-center", width: 10 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("RC", 0, 0), { anchor: "right-center", width: 10 }),
		tick(40),
	], { cols: 40, rows: 12 }),

	// "width percentage" + "should respect minWidth when widthPercent results
	// in smaller width": "50%" of a 40-col terminal is 20 columns; "10%" (4)
	// clamps up to minWidth 30; "90%" is 36.
	tm("overlay-width-percent-minwidth", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("test", 0, 0), { width: "50%" }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("min", 0, 0), { width: "10%", minWidth: 30 }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("wide", 0, 0), { width: "90%" }),
		tick(40),
	], { cols: 40, rows: 10 }),

	// "should clamp negative margins to zero" / "margin as number" /
	// "margin object": margin 5 pushes to row 5 col 5; {top:2,left:3} to
	// row 2 col 3; negative margins clamp back to row 0 col 0.
	tm("overlay-margin-variants", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("MARGIN", 0, 0), { anchor: "top-left", width: 10, margin: 5 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("OBJ", 0, 0), {
			anchor: "top-left",
			width: 10,
			margin: { top: 2, left: 3, right: 0, bottom: 0 },
		}),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("NEG", 0, 0), {
			anchor: "top-left",
			width: 12,
			margin: { top: -5, left: -10, right: 0, bottom: 0 },
		}),
		tick(40),
	], { cols: 40, rows: 12 }),

	// "should position with rowPercent and colPercent" / "rowPercent 0" /
	// "rowPercent 100": "50%"/"50%" centers both axes; "0%" pins to the top
	// row; "100%" pins to the last row.
	tm("overlay-rowcol-percent", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("PCT", 0, 0), { width: 10, row: "50%", col: "50%" }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("TOP", 0, 0), { width: 10, row: "0%" }),
		tick(40),
		hideOverlay(),
		tick(40),
		showOverlay(txt("BOTTOM", 0, 0), { width: 10, row: "100%" }),
		tick(40),
	], { cols: 40, rows: 12 }),

	// "should truncate overlay to maxHeight" / "maxHeightPercent": 5 lines
	// clamp to 3 rows; 10 lines clamp to "50%" of a 10-row terminal (5).
	tm("overlay-maxheight-truncate", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("Line 1\nLine 2\nLine 3\nLine 4\nLine 5", 0, 0), { maxHeight: 3 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9\nL10", 0, 0), { maxHeight: "50%" }),
		tick(40),
	], { cols: 40, rows: 10 }),

	// "row and col should override anchor": absolute row 3 / col 5 wins over
	// a bottom-right anchor.
	tm("overlay-absolute-overrides-anchor", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("ABSOLUTE", 0, 0), { anchor: "bottom-right", row: 3, col: 5, width: 10 }),
		start(),
		tick(20),
	], { cols: 40, rows: 12 }),

	// "should truncate overlay lines that exceed declared width": the overlay
	// declares width 20 but its content is far wider; the Text component
	// wraps at the declared width so no composited line exceeds it, and an
	// overlay parked at the right terminal edge (col 60, width 20 in 80 cols)
	// must fit exactly.
	tm("overlay-overflow-and-edge", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("X".repeat(100), 0, 0), { width: 20 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("X".repeat(50), 0, 0), { col: 60, width: 20 }),
		tick(40),
	], { cols: 80, rows: 24 }),

	// "should handle overlay with complex ANSI sequences without crashing":
	// true-color SGR spans plus an OSC 8 hyperlink repeated across long
	// lines, composited at declared width 60.
	tm("overlay-complex-ansi", [
		addChild(txt("base", 0, 0)),
		showOverlay(
			txt(
				"\x1b[48;2;40;50;40m \x1b[38;2;128;128;128mSome styled content\x1b[39m\x1b[49m" +
					"\x1b]8;;http://example.com\x07link\x1b]8;;\x07" +
					" more content ".repeat(10),
				0,
				0,
			),
			{ width: 60 },
		),
		start(),
		tick(20),
	], { cols: 80, rows: 24 }),

	// "should handle wide characters at overlay boundary": CJK content (2
	// columns per char) wrapped at an odd declared width 15 must split a wide
	// char cell cleanly at the boundary.
	tm("overlay-wide-chars-boundary", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("中文日本語한글テスト漢字", 0, 0), { width: 15 }),
		start(),
		tick(20),
	], { cols: 80, rows: 24 }),

	// "stacked overlays": a later overlay at the same position renders on
	// top ("SECOND" covers "FIRST"); overlays at different positions do not
	// interfere; hideOverlay() removes them in stack order (hiding the top
	// "SECOND" reveals "FIRST" underneath).
	tm("overlay-stacked-order", [
		addChild(txt("base", 0, 0)),
		showOverlay(txt("FIRST-OVERLAY", 0, 0), { anchor: "top-left", width: 20 }),
		showOverlay(txt("SECOND", 0, 0), { anchor: "top-left", width: 10 }),
		start(),
		tick(20),
		hideOverlay(),
		tick(40),
		showOverlay(txt("TL", 0, 0), { anchor: "top-left", width: 15 }),
		showOverlay(txt("BTM-RIGHT", 0, 0), { anchor: "bottom-right", width: 15 }),
		tick(40),
	], { cols: 40, rows: 12 }),

	// "hiding after stop" (upstream issue #10026): hideOverlay() called on a
	// stopped screen must leave the cursor visible (no dangling hide with no
	// matching show in the write stream).
	tm("overlay-hide-after-stop-cursor", [
		addChild(txt("base", 0, 0)),
		start(),
		tick(20),
		showOverlay(txt("OVERLAY", 0, 0), { nonCapturing: true }),
		tick(40),
		stop(),
		hideOverlay(),
	], { cols: 40, rows: 10 }),

	// ---------------------------------------------------------------------------
	// overlay-non-capturing.test.ts — focus management / input routing.
	// Upstream observes routing with a recording editor; here the base
	// "editor" is a focused select list, so an arrow key moves whichever
	// list currently holds focus — the rendered selection marker shows
	// where each keystroke landed. (The Input editor is avoided as the base
	// because its hardware-cursor positioning writes currently diverge
	// between the TS and Rust harnesses.)
	// "non-capturing overlay preserves focus on creation" +
	// "passive non-capturing overlay does not regain input": showing a
	// non-capturing overlay over a focused base list leaves focus untouched,
	// and the arrow key keeps driving the base list, not the overlay.
	tm("nc-input-routes-to-editor", [
		addChild(overlayList("ed", 3)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(txt("PASSIVE", 0, 0), { anchor: "center", width: 20, nonCapturing: true }),
		tick(40),
		key("\x1b[B"),
		tick(20),
	], { cols: 40, rows: 12 }),

	// "hideOverlay() does not reassign focus when topmost overlay is
	// non-capturing": a capturing select-list overlay is focused (arrow key
	// moves its selection), a non-capturing overlay is stacked above it, and
	// hiding that topmost overlay leaves the capturing list focused — the
	// next arrow key still drives the overlay list, not the base.
	tm("nc-hide-topmost-keeps-capture", [
		addChild(overlayList("ed", 3)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(overlayList("cap", 3), { anchor: "center", width: 20 }),
		tick(40),
		key("\x1b[B"),
		tick(20),
		showOverlay(txt("NC", 0, 0), { row: 0, col: 0, nonCapturing: true }),
		tick(40),
		hideOverlay(),
		tick(40),
		key("\x1b[B"),
		tick(20),
	], { cols: 40, rows: 12 }),

	// "capturing overlay removed with non-capturing below restores focus to
	// editor": hiding the capturing overlay skips the non-capturing one below
	// it and restores focus to the base list, whose selection then moves.
	tm("nc-capture-hide-restores-editor", [
		addChild(overlayList("ed", 3)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(txt("NC", 0, 0), { row: 0, col: 0, nonCapturing: true }),
		tick(40),
		showOverlay(overlayList("cap", 3), { anchor: "center", width: 20 }),
		tick(40),
		key("\x1b[B"),
		tick(20),
		hideOverlay(),
		tick(40),
		key("\x1b[B"),
		tick(20),
	], { cols: 40, rows: 12 }),

	// "multiple capturing and non-capturing overlays restore focus through
	// removals": c1/n1/c2/n2 stacked; hiding n2 then c2 restores focus to c1
	// (the next arrow key moves c1's selection); hiding n1 then c1 finally
	// restores the base list, whose selection moves.
	tm("nc-overlay-restore-chain", [
		addChild(overlayList("ed", 3)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(overlayList("c1", 3), { anchor: "center", width: 20 }),
		tick(40),
		showOverlay(txt("N1", 0, 0), { row: 0, col: 0, nonCapturing: true }),
		tick(40),
		showOverlay(overlayList("c2", 3), { anchor: "center", width: 20 }),
		tick(40),
		showOverlay(txt("N2", 0, 0), { row: 1, col: 0, nonCapturing: true }),
		tick(40),
		hideOverlay(), // hides N2
		tick(40),
		hideOverlay(), // hides C2 -> focus back to C1
		tick(40),
		key("\x1b[B"),
		tick(20),
		hideOverlay(), // hides N1
		tick(40),
		hideOverlay(), // hides C1 -> focus back to base list
		tick(40),
		key("\x1b[B"),
		tick(20),
	], { cols: 40, rows: 12 }),

	// "handleInput restores focus to a visible focused overlay after base
	// focus steal": the capturing overlay holds focus; an explicit setFocus
	// back to the base list steals focus, but the next keystroke is still
	// routed to the overlay (its selection moves, the base list does not).
	tm("nc-base-steal-restore", [
		addChild(overlayList("ed", 3)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(overlayList("ov", 3), { anchor: "center", width: 20 }),
		tick(40),
		focus("c0"),
		tick(20),
		key("\x1b[B"),
		tick(20),
	], { cols: 40, rows: 12 }),

	// "setFocus(null) explicitly clears visible overlay restore": with the
	// capturing overlay shown and focus explicitly cleared, the keystroke is
	// dropped — nobody is focused, the overlay never receives it.
	tm("nc-setfocus-null-drops-input", [
		addChild(overlayList("ed", 3)),
		focus("c0"),
		start(),
		tick(20),
		showOverlay(overlayList("ov", 3), { anchor: "center", width: 20 }),
		tick(40),
		focus(null),
		tick(20),
		key("\x1b[B"),
		tick(20),
	], { cols: 40, rows: 12 }),
];
