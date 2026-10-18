// T5 Wave C — tui-alt differential parity cases.
//
// Ported from /tmp/pi-repo/packages/tui/test/tui-alt-screen.test.ts (upstream
// alt-screen behavior tests) onto the parity harness script vocabulary.
// Adaptations required by the harness contract:
// - Text components always use (0,0) padding like the upstream tests.
// - `waitForRender()` maps to tick(20) (render interval 16ms).
// - Upstream `text.setText(...)` (content mutation) is approximated by
//   addChild() of follow-up content — the alt-screen root is a scroll
//   container, so appended lines exercise the same follow/preserve logic.
// - Custom test components (recording editors, handleMouse objects) are
//   approximated with the Input component where the routing behavior
//   (focus capture / key deferral) is what matters.
// - Upstream scrollToBottom()/scrollToTop() map to scrollBy(±large) (clamped).
//
// Out of contract (intentionally not ported — see report): terminal images
// (Kitty/iTerm2), env-driven mouse-tracking modes, platform-specific
// right-click paste, scrollToEndIndicator / openUrl / copySelection /
// searchNavigationButtonStyle callbacks, runtime setWheelScrollLines,
// setKeybindings overrides, and AltScreenSearchComponent unit tests.

import {
	ta, txt, ent, hstk, vstk, inp, slist, svw, mr, sitem,
	start, stop, key, tick, resize, focus, addChild, showOverlay, hideOverlay,
	requestRender, renderNow, setLayoutRoot, scrollBy, flash,
} from "../wavec-helpers.mjs";

const lines = (count, prefix = "line ") =>
	Array.from({ length: count }, (_, index) => `${prefix}${index + 1}`).join("\n");

// SGR mouse helpers (1-based col;row from the top-left).
const press = (col, row) => key(`\x1b[<0;${col};${row}M`);
const release = (col, row) => key(`\x1b[<0;${col};${row}m`);
const drag = (col, row) => key(`\x1b[<32;${col};${row}M`);
const motion = (col, row) => key(`\x1b[<35;${col};${row}M`);
const wheelUp = (col, row) => key(`\x1b[<64;${col};${row}M`);
const wheelDown = (col, row) => key(`\x1b[<65;${col};${row}M`);

const FAKE_CURSOR_START = "\x1b_pi:fc\x07";
const FAKE_CURSOR_END = "\x1b_pi:/fc\x07";
const OSC133_ZONE_START = "\x1b]133;A\x07";

// ---------------------------------------------------------------------------
// Viewport / scrolling
// ---------------------------------------------------------------------------

// Upstream: "renders a terminal-height viewport and preserves manual scroll
// position". Wheel-up breaks follow; appended content must not move the
// manually positioned viewport.
const viewportManualScroll = ta("viewport-manual-scroll", [
	addChild(txt(lines(10), 0, 0)),
	start(),
	tick(20),
	wheelUp(1, 1),
	tick(20),
	addChild(txt("line 11\nline 12", 0, 0)),
	requestRender(),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// scrollBy clamps at both ends (upstream uses scrollToTop/scrollToBottom).
const scrollByClamp = ta("scrollby-clamp", [
	addChild(txt(lines(10), 0, 0)),
	start(),
	tick(20),
	scrollBy(-100),
	tick(20),
	scrollBy(100),
	tick(20),
	scrollBy(-2),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "scrolls faster while Alt is held during wheel input" (72 = 64+8).
const wheelAltMultiplier = ta("wheel-alt-modifier", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	key("\x1b[<72;1;1M"),
	tick(20),
], { cols: 20, rows: 4 });

// Upstream: "ignores horizontal trackpad wheel events" (buttons 66/67).
const wheelHorizontalIgnored = ta("wheel-horizontal-ignored", [
	addChild(txt(lines(8), 0, 0)),
	start(),
	tick(20),
	key("\x1b[<66;1;1M"),
	key("\x1b[<67;1;1M"),
	tick(20),
], { cols: 20, rows: 4 });

// Upstream: "applies runtime wheel line count updates" — static
// wheelScrollLines option variant (runtime mutation is out of contract).
const wheelScrollLinesOption = ta("wheel-scroll-lines-option", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	wheelUp(1, 1),
	tick(20),
	wheelDown(1, 1),
	tick(20),
], { cols: 20, rows: 4, altOptions: { wheelScrollLines: 3 } });

// Upstream: "routes wheel input to the scroll view under the pointer".
const wheelUnderPointer = ta("wheel-under-pointer", [
	setLayoutRoot(
		hstk([
			ent(svw(txt("a1\na2\na3\na4\na5\na6\na7", 0, 0), { follow: "end", primary: true }), { basis: 10, shrink: 0 }),
			ent(svw(txt("b1\nb2\nb3\nb4\nb5\nb6\nb7", 0, 0), { follow: "end" }), { basis: 10, shrink: 0 }),
		]),
	),
	start(),
	tick(20),
	wheelUp(15, 1),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "chains unused wheel delta to an outer scroll view".
const wheelChainsToOuter = ta("wheel-chains-outer-scrollview", [
	setLayoutRoot(
		svw(
			vstk([
				ent(svw(txt("i1\ni2\ni3\ni4\ni5\ni6", 0, 0)), { basis: 2 }),
				ent(txt("tail1\ntail2\ntail3\ntail4\ntail5", 0, 0)),
			]),
			{ primary: true },
		),
	),
	start(),
	tick(20),
	wheelDown(1, 1),
	tick(20),
	wheelDown(1, 1),
	tick(20),
], { cols: 20, rows: 4, screenLines: true, altOptions: { wheelScrollLines: 3 } });

// Upstream: "keeps an explicit dock fixed while the transcript scrolls".
// Wheel over the dock falls back to the primary transcript; scrollBy(50)
// stands in for scrollToBottom().
const dockFixedWhileScrolling = ta("dock-fixed-while-scrolling", [
	setLayoutRoot(
		vstk([
			ent(svw(txt(lines(8), 0, 0), { follow: "end", primary: true }), { basis: 0, grow: 1, min: 1 }),
			ent(vstk([ent(txt("editor", 0, 0)), ent(txt("footer", 0, 0))]), { basis: "auto", min: 1 }),
		]),
	),
	start(),
	tick(20),
	wheelUp(1, 6),
	tick(20),
	scrollBy(50),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// Upstream: "does not vertically redispatch misses through horizontal layout
// containers" — a click on the plain-text pane must not reach the select list.
const hstackClickMiss = ta("hstack-click-miss", [
	setLayoutRoot(
		hstk([
			ent(slist([sitem("a", "A"), sitem("b", "B")], { maxVisible: 2 }), { basis: 10 }),
			ent(txt("plain", 0, 0), { basis: 10 }),
		]),
	),
	start(),
	tick(20),
	press(15, 1),
	release(15, 1),
	tick(20),
], { cols: 20, rows: 2, screenLines: true });

// ---------------------------------------------------------------------------
// Keyboard viewport navigation
// ---------------------------------------------------------------------------

// Upstream: "supports configurable keyboard viewport navigation with four
// rows of page overlap" (kitty CSI-u + legacy modifier forms).
const keyboardPageNavigation = ta("keyboard-page-navigation", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	key("\x1b[57421u"),
	key("\x1b[57421;1:3u"),
	tick(20),
	key("\x1b[57422u"),
	key("\x1b[57422;1:3u"),
	tick(20),
	key("\x1b[7^"),
	tick(20),
	key("\x1b[8^"),
	tick(20),
], { cols: 20, rows: 8, screenLines: true });

// Upstream: "routes Home and End to the focused component and Ctrl+Home/End
// to the transcript". Input stands in for the recording editor; unmodified
// Home/End/PageUp belong to it, Ctrl+Home/End + PageDown scroll the transcript.
const homeEndRouting = ta("home-end-routing", [
	setLayoutRoot(
		vstk([
			ent(svw(txt(lines(12), 0, 0), { follow: "end", primary: true }), { basis: 0, grow: 1, min: 1 }),
			ent(inp({ prompt: ">" }), { basis: 1, shrink: 0 }),
		]),
	),
	focus("c2"),
	start(),
	tick(20),
	key("\x1bOH"),
	key("\x1b[F"),
	key("\x1b[57423u"),
	key("\x1b[5;5~"),
	key("\x1b[6;5~"),
	tick(20),
	key("\x1b[1;5H"),
	tick(20),
	key("\x1b[1;5F"),
	tick(20),
	key("\x1b[57423;5u"),
	key("\x1b[57423;5:3u"),
	tick(20),
	key("\x1b[6~"),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// Upstream: "jumps between OSC 133 semantic prompt markers".
const osc133MarkerNavigation = ta("osc133-marker-navigation", [
	addChild(txt([1, 2, 3, 4].flatMap((m) => [`${OSC133_ZONE_START}message ${m}`, "detail"]).join("\n"), 0, 0)),
	start(),
	tick(20),
	key("\x1b[57419;6u"),
	key("\x1b[57419;6:3u"),
	tick(20),
	key("\x1b[1;6A"),
	tick(20),
	key("\x1b[57420;6u"),
	key("\x1b[57420;6:3u"),
	tick(20),
	key("\x1b[1;6B"),
	tick(20),
], { cols: 20, rows: 3, screenLines: true });

// ---------------------------------------------------------------------------
// Scrollbar
// ---------------------------------------------------------------------------

// Upstream: "reveals an auto scrollbar when the pointer enters its hidden
// track" (hide delay 20ms; leaving the track lets it expire).
const scrollbarAutoRevealHide = ta("scrollbar-auto-reveal-hide", [
	setLayoutRoot(svw(txt(lines(20), 0, 0), { primary: true, scrollbar: "auto", scrollbarHideDelayMs: 20 })),
	start(),
	tick(20),
	motion(10, 3),
	tick(20),
	motion(9, 3),
	tick(40),
	tick(20),
], { cols: 10, rows: 5, screenLines: true });

// Upstream: "jumps to a scrollbar track position and continues dragging from
// there" (press on the track jumps; drag continues; release without OSC 52).
const scrollbarDragJump = ta("scrollbar-drag-jump", [
	setLayoutRoot(svw(txt(lines(50), 0, 0), { primary: true, scrollbar: "always" })),
	start(),
	tick(20),
	press(10, 6),
	tick(20),
	drag(10, 10),
	tick(20),
	release(10, 10),
	tick(20),
], { cols: 10, rows: 10, screenLines: true });

// ---------------------------------------------------------------------------
// Flash messages
// ---------------------------------------------------------------------------

// Upstream: "stacks flash messages and collapses them as they expire".
const flashStackExpiry = ta("flash-stack-expiry", [
	addChild(txt("one\ntwo\nthree\nfour", 0, 0)),
	start(),
	tick(20),
	flash("First", 80),
	flash("Second", 500),
	tick(20),
	tick(100),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Default flash duration is 1000ms: still visible at 500ms, gone at 1600ms.
const flashDefaultDuration = ta("flash-default-duration", [
	addChild(txt("one\ntwo\nthree\nfour", 0, 0)),
	start(),
	tick(20),
	flash("hi"),
	tick(20),
	tick(500),
	tick(20),
	tick(1100),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// ---------------------------------------------------------------------------
// Mouse selection
// ---------------------------------------------------------------------------

// Upstream: "selects visible text with the mouse and copies it with OSC 52
// after a generic release" — includes inverse-video reapplication across an
// inline SGR reset and the "Copied!" flash.
const selectionCopyOsc52 = ta("selection-copy-osc52", [
	addChild(txt("\x1b[1mal\x1b[0mpha\nbeta\ngamma\ndelta", 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	drag(4, 2),
	key("\x1b[<3;4;2m"),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "keeps the selection highlight after a fake cursor cell".
const selectionFakeCursor = ta("selection-fake-cursor", [
	addChild(txt(`> ${FAKE_CURSOR_START}a${FAKE_CURSOR_END}bcd`, 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	drag(6, 1),
	key("\x1b[<3;6;1m"),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "leaves selections visible without copying when copyOnSelect is
// disabled" — highlight stays, no clipboard, no "Copied!".
const selectionCopyOnSelectFalse = ta("selection-copyonselect-false", [
	addChild(txt("alpha\nbeta\ngamma\ndelta", 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	drag(4, 2),
	release(4, 2),
	tick(20),
], { cols: 20, rows: 4, screenLines: true, altOptions: { copyOnSelect: false } });

// Upstream: "does not append whitespace to double-click word highlighting".
const selectionDoubleClickWord = ta("selection-doubleclick-word", [
	addChild(txt("foo  bar", 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	release(1, 1),
	press(3, 1),
	tick(20),
], { cols: 20, rows: 1, screenLines: true });

// Upstream: "coalesces slash and hyphen separated segments for double-click
// word selection" (path variant; copy text observable via OSC 52 writes).
const selectionWordSlash = ta("selection-word-slash", [
	addChild(txt("extensions/starline/fixed-editor/compositor.ts", 0, 0)),
	start(),
	tick(20),
	press(13, 1),
	release(13, 1),
	press(13, 1),
	release(13, 1),
	tick(20),
], { cols: 80, rows: 1, screenLines: true });

// Hyphen variant of the same behavior.
const selectionWordHyphen = ta("selection-word-hyphen", [
	addChild(txt("earendil-works/pi-tui", 0, 0)),
	start(),
	tick(20),
	press(11, 1),
	release(11, 1),
	press(11, 1),
	release(11, 1),
	tick(20),
], { cols: 80, rows: 1, screenLines: true });

// Upstream: "highlights a complete whitespace segment during a word drag".
const selectionWordDragWhitespace = ta("selection-word-drag-whitespace", [
	addChild(txt("foo  bar", 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	release(1, 1),
	press(2, 1),
	drag(4, 1),
	tick(20),
], { cols: 20, rows: 1, screenLines: true });

// Upstream: "selects whole words on double click, extends word drags, and
// selects lines on triple click".
const selectionMultiClick = ta("selection-multiclick-word-line", [
	addChild(txt("zero alpha beta\ngamma delta", 0, 0)),
	start(),
	tick(20),
	// Double-click "alpha" (second click on a different character).
	press(6, 1),
	release(6, 1),
	press(10, 1),
	release(10, 1),
	tick(20),
	// Double-click drag across the row boundary selects whole words.
	press(12, 1),
	release(12, 1),
	press(14, 1),
	drag(3, 2),
	release(3, 2),
	tick(20),
	// Triple-click selects the whole line.
	press(7, 2),
	release(7, 2),
	press(9, 2),
	release(9, 2),
	press(11, 2),
	release(11, 2),
	tick(20),
], { cols: 20, rows: 2, screenLines: true });

// Upstream: "does not repaint idle or zero-width selections on focus loss" —
// focus in/out with no active selection writes nothing; orphaned drag/release
// events after a plain click must not extend the zero-width anchor; a press
// without drag is cancelled by focus loss without repainting.
const selectionFocusLossIdle = ta("selection-focusloss-idle", [
	addChild(txt("alpha\nbeta\ngamma\ndelta", 0, 0)),
	start(),
	tick(20),
	key("\x1b[O"),
	key("\x1b[I"),
	tick(20),
	press(1, 1),
	release(1, 1),
	drag(4, 2),
	release(4, 2),
	tick(20),
	press(1, 3),
	tick(20),
	key("\x1b[O"),
	key("\x1b[I"),
	tick(20),
	drag(4, 2),
	release(4, 2),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "clears an active visible selection on focus loss and ignores
// orphan events" — the repaint after focus loss must drop the inverse video.
const selectionFocusLossClears = ta("selection-focusloss-clears", [
	addChild(txt("alpha\nbeta\ngamma\ndelta", 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	drag(4, 2),
	tick(20),
	key("\x1b[O"),
	key("\x1b[I"),
	tick(20),
	drag(4, 2),
	release(4, 2),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "retains a completed visible selection across focus changes" —
// no repaint on focus loss alone, but a forced full redraw keeps the
// inverse-video selection.
const selectionFocusRetained = ta("selection-focusloss-retained", [
	addChild(txt("alpha\nbeta\ngamma\ndelta", 0, 0)),
	start(),
	tick(20),
	press(1, 1),
	drag(4, 2),
	release(4, 2),
	tick(20),
	key("\x1b[O"),
	key("\x1b[I"),
	tick(20),
	renderNow(true),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "auto-scrolls and extends a drag selection held at the viewport
// edge" (selection auto-scroll interval 50ms).
const selectionAutoScrollEdge = ta("selection-autoscroll-edge", [
	addChild(txt(lines(10), 0, 0)),
	start(),
	tick(20),
	press(1, 3),
	drag(1, 1),
	tick(130),
	release(1, 1),
	tick(20),
], { cols: 20, rows: 4, screenLines: true });

// Upstream: "snaps mouse selection to CJK, emoji, and combining grapheme
// boundaries".
const selectionCjkGrapheme = ta("selection-cjk-grapheme", [
	addChild(txt("A界🙂éZ", 0, 0)),
	start(),
	tick(20),
	press(3, 1),
	drag(4, 1),
	release(4, 1),
	tick(20),
	press(5, 1),
	drag(2, 1),
	release(2, 1),
	tick(20),
	press(6, 1),
	drag(7, 1),
	release(7, 1),
	tick(20),
], { cols: 20, rows: 2, screenLines: true });

// Upstream: "dispatches clicks to nested mouse regions without breaking drag
// selection" — the MouseRegion handler is a no-op in this harness, so only
// the drag-selection-across-the-region path is observable.
const selectionAcrossMouseRegion = ta("selection-across-mouseregion", [
	addChild(mr(txt("clickable\nselectable", 0, 0))),
	start(),
	tick(20),
	press(2, 1),
	release(2, 1),
	tick(20),
	press(1, 1),
	drag(4, 2),
	release(4, 2),
	tick(20),
], { cols: 20, rows: 2, screenLines: true });

// ---------------------------------------------------------------------------
// Transcript search
// ---------------------------------------------------------------------------

// Upstream: "navigates transcript search with hoverable arrow buttons and
// toggles it with its shortcut" — hover the next-match button, click it
// (1/2 → 2/2), click the previous-match button (→ 1/2), toggle search off.
const searchNavigateButtons = ta("search-navigate-buttons", [
	addChild(txt("needle one\nmiddle\nneedle two\nend", 0, 0)),
	start(),
	tick(20),
	key("\x1b[102;6u"),
	tick(20),
	key("needle"),
	tick(20),
	motion(110, 4),
	tick(20),
	press(110, 4),
	release(110, 4),
	tick(20),
	press(94, 4),
	release(94, 4),
	tick(20),
	key("\x1b[102;6u"),
	tick(20),
], { cols: 120, rows: 6, screenLines: true });

// Upstream: "does not treat transcript box drawing as search navigation
// buttons" — a click on a box-drawing row must not navigate matches.
const searchBoxDrawingNotButton = ta("search-boxdrawing-not-button", [
	addChild(
		txt(
			[
				"needle one",
				"middle",
				"needle two",
				"filler",
				"┌────────────────────────────────────────┐",
				"│ box                                    │",
				"└────────────────────────────────────────┘",
				"end",
			].join("\n"),
			0,
			0,
		),
	),
	start(),
	tick(20),
	key("\x1b[102;6u"),
	tick(20),
	key("needle"),
	tick(20),
	press(24, 7),
	release(24, 7),
	tick(20),
], { cols: 80, rows: 10, screenLines: true });

// Upstream: "uses configured styles for current and non-current search
// matches" — identity style presets (default styles use SGR 4 / 1;7).
const searchMatchStyles = ta("search-match-styles", [
	addChild(txt("needle first\nmiddle\nneedle second\nend", 0, 0)),
	start(),
	tick(20),
	key("\x1b[102;6u"),
	tick(20),
	key("needle"),
	tick(20),
], { cols: 60, rows: 4, screenLines: true, altOptions: { searchMatchStyle: "identity", searchCurrentMatchStyle: "identity" } });

// Upstream: "searches the transcript with Ctrl+Shift+F and restores editor
// focus on close" — search unfollows the transcript, wheel scrolling while
// the search is focused still moves the viewport, ctrl+g / ctrl+shift+g
// navigate matches, Escape closes and restores editor focus.
const searchEditorFocusRestore = ta("search-editor-focus-restore", [
	setLayoutRoot(
		vstk([
			ent(
				svw(
					txt(
						Array.from({ length: 12 }, (_, index) => {
							if (index === 4) return "line 5 needle one";
							if (index === 9) return "line 10 needle two";
							return `line ${index + 1}`;
						}).join("\n"),
						0,
						0,
					),
					{ follow: "end", primary: true },
				),
				{ basis: 0, grow: 1, min: 1 },
			),
			ent(inp({ prompt: ">" }), { basis: 1, shrink: 0 }),
		]),
	),
	focus("c2"),
	start(),
	tick(20),
	key("\x1b[102;6u"),
	tick(20),
	key("needle"),
	tick(20),
	wheelUp(1, 4),
	wheelUp(1, 4),
	wheelUp(1, 4),
	wheelUp(1, 4),
	wheelUp(1, 4),
	wheelUp(1, 4),
	tick(20),
	key("\x07"),
	tick(20),
	key("\x1b[103;6u"),
	tick(20),
	key("\x1b"),
	tick(20),
	key("x"),
	tick(20),
], { cols: 60, rows: 8, screenLines: true });

// Upstream: "keeps viewport scrolling while transcript search is focused".
const searchScrollWhileFocused = ta("search-scroll-while-focused", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	key("\x1b[102;6u"),
	tick(20),
	key("\x1b[5~"),
	wheelUp(1, 4),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

// Upstream: "invalidates overlays with an explicit layout root" — an overlay
// shown over an explicit layout root must render on the next invalidation.
const overlayWithLayoutRoot = ta("overlay-with-layoutroot", [
	setLayoutRoot(txt("root", 0, 0)),
	start(),
	tick(20),
	showOverlay(txt("overlay", 0, 0), { anchor: "center" }),
	requestRender(),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// Upstream: "gives viewport keys to a focused overlay" — PageUp/PageDown/
// Home/End are deferred to the focused overlay (no viewport scroll); after
// hideOverlay they scroll the viewport again.
const overlayFocusedKeys = ta("overlay-focused-keys", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	showOverlay(inp({ prompt: "edit" })),
	tick(20),
	key("\x1b[5~"),
	key("\x1b[6~"),
	key("\x1bOH"),
	key("\x1bOF"),
	tick(20),
	hideOverlay(),
	tick(20),
	key("\x1b[5~"),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// Upstream: "keeps viewport scrolling when an overlay is not focused" — a
// non-capturing overlay must not swallow viewport keys or wheel events.
const overlayNonCapturing = ta("overlay-noncapturing-scroll", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	showOverlay(inp({ prompt: "side" }), { nonCapturing: true }),
	tick(20),
	key("\x1b[5~"),
	wheelUp(10, 3),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// Upstream (#10769): "scrolls the transcript on wheel input a focused
// overlay does not handle".
const overlayWheelUnhandled = ta("overlay-wheel-unhandled", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	showOverlay(inp({ prompt: "edit" }), { anchor: "top-left", width: 10 }),
	tick(20),
	wheelUp(3, 1),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// ---------------------------------------------------------------------------
// Lifecycle, redraw, resize
// ---------------------------------------------------------------------------

// Upstream: "restores keyboard state before leaving alt mode and prints the
// full document" — stop() disables the mouse, leaves alt mode and reprints
// the whole transcript on the main screen.
const stopRestoresDocument = ta("stop-restores-document", [
	addChild(txt("first\nsecond\nthird\nfourth\nfifth\nsixth", 0, 0)),
	start(),
	tick(20),
	stop(),
], { cols: 20, rows: 3 });

// stop(preserveScreen: true) variant of the shutdown path.
const stopPreserveScreen = ta("stop-preserve-screen", [
	addChild(txt("first\nsecond\nthird\nfourth\nfifth\nsixth", 0, 0)),
	start(),
	tick(20),
	stop({ preserveScreen: true }),
], { cols: 20, rows: 3 });

// Forced full redraw via renderNow(true) vs incremental requestRender(),
// plus a resize-driven full redraw.
const fullRedrawForce = ta("full-redraw-force", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	requestRender(false),
	tick(20),
	renderNow(true),
	tick(20),
	resize(24, 6),
	tick(20),
], { cols: 20, rows: 6, screenLines: true });

// Resize reflows the viewport (grow + shrink while following output).
const resizeViewport = ta("resize-viewport", [
	addChild(txt(lines(12), 0, 0)),
	start(),
	tick(20),
	resize(20, 4),
	tick(20),
	resize(30, 6),
	tick(20),
], { cols: 20, rows: 8, screenLines: true });

export const altCases = [
	// Viewport / scrolling
	viewportManualScroll,
	scrollByClamp,
	wheelAltMultiplier,
	wheelHorizontalIgnored,
	wheelScrollLinesOption,
	wheelUnderPointer,
	wheelChainsToOuter,
	dockFixedWhileScrolling,
	hstackClickMiss,
	// Keyboard navigation
	keyboardPageNavigation,
	homeEndRouting,
	osc133MarkerNavigation,
	// Scrollbar
	scrollbarAutoRevealHide,
	scrollbarDragJump,
	// Flash
	flashStackExpiry,
	flashDefaultDuration,
	// Selection
	selectionCopyOsc52,
	selectionFakeCursor,
	selectionCopyOnSelectFalse,
	selectionDoubleClickWord,
	selectionWordSlash,
	selectionWordHyphen,
	selectionWordDragWhitespace,
	selectionMultiClick,
	selectionFocusLossIdle,
	selectionFocusLossClears,
	selectionFocusRetained,
	selectionAutoScrollEdge,
	selectionCjkGrapheme,
	selectionAcrossMouseRegion,
	// Search
	searchNavigateButtons,
	searchBoxDrawingNotButton,
	searchMatchStyles,
	searchEditorFocusRestore,
	searchScrollWhileFocused,
	// Overlays
	overlayWithLayoutRoot,
	overlayFocusedKeys,
	overlayNonCapturing,
	overlayWheelUnhandled,
	// Lifecycle / redraw / resize
	stopRestoresDocument,
	stopPreserveScreen,
	fullRedrawForce,
	resizeViewport,
];
