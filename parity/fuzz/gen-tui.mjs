/**
 * TUI batter generators — component trees, editor scripts, screen scripts.
 *
 * Vocabulary mirrors the harness contracts 1:1 (ts-harness.js / pi-cli
 * main.rs):
 *   - tui-components: `{ cases: [{ name, component: <spec tree>, widths,
 *     script }] }` (native batch; one payload = many sub-cases).
 *   - tui-editor: one case = `{ name, group: "tui-editor",
 *     theme: "default", width?, options?, focused?, disableSubmit?,
 *     history?, setText?, script: [ops] }` (tool-batch item).
 *   - tui-main / tui-alt: one case = `{ name, cols, rows, capabilities?,
 *     showHardwareCursor?, clearOnShrink?, altOptions?, screenLines?,
 *     script: [ops] }` (tool-batch item).
 *
 * Refs: every built component registers "c0", "c1", ... in build order
 * (pre-order, children before parents), accumulating across ALL ops in a
 * script. The screen generators track the registry size so `setFocus` /
 * `setLayoutRoot` only emit valid refs.
 */
import { pick, int, bool, sampleUnicode, sampleKeyString } from "./lib.mjs";

// ---------------------------------------------------------------- text

const WORDS = [
	"hello", "world", "foo", "bar", "baz", "a", "bb", "test", "line", "x y z",
	"the quick brown fox", "one\ntwo", "a\tb", "  spaced  ", "", "123",
];

/** Component/editor text content — bounded to keep renders small. */
export function sampleText(r, { maxLen = 24 } = {}) {
	const roll = r();
	if (roll < 0.4) return pick(r, WORDS);
	if (roll < 0.6) return sampleUnicode(r, { maxLen });
	if (roll < 0.75) return pick(r, WORDS).slice(0, int(r, 0, 6)) + pick(r, ["\n", "\t", " ", ""]);
	if (roll < 0.9) return pick(r, WORDS) + (bool(r) ? "\n" + pick(r, WORDS) : "");
	return sampleUnicode(r, { maxLen: Math.min(maxLen, 8) });
}

// ------------------------------------------------------ component trees

/**
 * Random component spec tree. `vocab`:
 *   - "components" — the tui-components intersection (no scroll-view
 *     scrollbar options, which only the screen harnesses map)
 *   - "screen" — the full tui-main/tui-alt vocabulary
 */
export function genComponent(r, depth = 0, vocab = "components") {
	const simple = ["text", "truncated-text", "spacer"];
	const all = [
		...simple, "box", "h-stack", "v-stack", "loader", "cancellable-loader",
		"mouse-region", "scroll-view", "input", "select-list", "settings-list",
	];
	const type = pick(r, depth >= 2 ? simple : all);
	switch (type) {
		case "text": {
			const s = { type, text: sampleText(r) };
			if (bool(r)) s.paddingX = int(r, 0, 3);
			if (bool(r)) s.paddingY = int(r, 0, 3);
			if (bool(r)) s.bg = pick(r, ["identity", "bracket"]);
			return s;
		}
		case "truncated-text": {
			const s = { type, text: sampleText(r) };
			if (bool(r)) s.paddingX = int(r, 0, 2);
			if (bool(r)) s.paddingY = int(r, 0, 2);
			return s;
		}
		case "spacer":
			return bool(r) ? { type, lines: int(r, 0, 3) } : { type };
		case "box": {
			const s = { type, children: [] };
			const n = int(r, 1, 3);
			for (let i = 0; i < n; i++) s.children.push(genComponent(r, depth + 1, vocab));
			if (bool(r)) s.paddingX = int(r, 0, 2);
			if (bool(r)) s.paddingY = int(r, 0, 2);
			if (bool(r)) s.bg = pick(r, ["identity", "bracket"]);
			return s;
		}
		case "h-stack":
		case "v-stack": {
			const entries = [];
			const n = int(r, 1, 3);
			for (let i = 0; i < n; i++) {
				const e = { component: genComponent(r, depth + 1, vocab) };
				if (bool(r)) e.basis = int(r, 0, 10);
				if (bool(r)) e.grow = int(r, 0, 3);
				if (bool(r)) e.shrink = int(r, 0, 2);
				if (bool(r)) e.minSize = int(r, 0, 5);
				if (bool(r)) e.maxSize = int(r, 0, 8);
				if (bool(r)) e.visible = pick(r, ["always", "never"]);
				entries.push(e);
			}
			const s = { type, entries };
			if (bool(r)) s.gap = int(r, 0, 2);
			if (bool(r)) s.align = pick(r, ["start", "center", "end", "stretch"]);
			return s;
		}
		case "loader": {
			const s = { type };
			if (bool(r)) s.message = sampleText(r, { maxLen: 12 });
			if (bool(r)) s.frames = ["-", "\\", "|", "/"].slice(0, int(r, 1, 4));
			if (bool(r)) s.intervalMs = pick(r, [0, 10, 50, 120]);
			return s;
		}
		case "cancellable-loader": {
			const s = { type, onAbort: "record" };
			if (bool(r)) s.message = sampleText(r, { maxLen: 12 });
			return s;
		}
		case "mouse-region": {
			const s = { type, child: genComponent(r, depth + 1, vocab) };
			if (bool(r)) s.onMouse = "record";
			return s;
		}
		case "scroll-view": {
			const s = { type, child: genComponent(r, depth + 1, vocab) };
			if (bool(r)) s.follow = bool(r);
			if (bool(r)) s.primary = bool(r);
			if (vocab === "screen") {
				if (bool(r)) s.overscroll = bool(r);
				if (bool(r)) s.scrollbar = bool(r);
				if (bool(r)) s.scrollbarTrackStyle = "identity";
				if (bool(r)) s.scrollbarThumbStyle = "identity";
				if (bool(r)) s.scrollbarHideDelayMs = pick(r, [0, 10, 100]);
			}
			return s;
		}
		case "input": {
			const s = { type };
			if (bool(r)) s.prompt = sampleText(r, { maxLen: 6 });
			if (bool(r)) s.placeholder = sampleText(r, { maxLen: 10 });
			if (bool(r)) s.focused = bool(r);
			if (bool(r)) s.onSubmit = "record";
			if (bool(r)) s.onEscape = "record";
			return s;
		}
		case "select-list": {
			const items = [];
			const n = int(r, 0, 5);
			for (let i = 0; i < n; i++) {
				const it = { value: "v" + i, label: pick(r, WORDS) };
				if (bool(r)) it.description = pick(r, WORDS);
				items.push(it);
			}
			const s = { type, items };
			if (bool(r)) s.maxVisible = int(r, 1, 4);
			if (bool(r)) s.onSelect = "record";
			if (bool(r)) s.onCancel = "record";
			if (bool(r)) s.onSelectionChange = "record";
			if (bool(r)) s.layoutOptions = {
				minPrimaryColumnWidth: int(r, 0, 10),
				maxPrimaryColumnWidth: int(r, 0, 20),
			};
			return s;
		}
		case "settings-list": {
			const items = [];
			const n = int(r, 1, 4);
			for (let i = 0; i < n; i++) {
				const it = { id: "s" + i, label: pick(r, WORDS), currentValue: pick(r, ["on", "off", "auto"]) };
				if (bool(r)) it.description = pick(r, WORDS);
				if (bool(r)) it.values = ["on", "off", "auto"].slice(0, int(r, 1, 3));
				items.push(it);
			}
			const s = { type, items };
			if (bool(r)) s.maxVisible = int(r, 1, 4);
			if (bool(r)) s.enableSearch = bool(r);
			if (bool(r)) s.onChange = "record";
			if (bool(r)) s.onCancel = "record";
			return s;
		}
	}
}

/** Number of refs a spec tree registers when built (one per node). */
export function specCount(spec) {
	if (!spec || typeof spec !== "object") return 0;
	let n = 1;
	for (const c of spec.children || []) n += specCount(c);
	for (const e of spec.entries || []) n += specCount(e.component);
	if (spec.child !== undefined) n += specCount(spec.child);
	return n;
}

// -------------------------------------------------------- key pools

/** Editor-relevant input strings (handleInput key parsing). */
const EDITOR_KEYS = [
	"a", "b", "z", "hello", " ", "  ", "é", "😀", "你", "👨‍👩‍👧‍👦",
	"\x1b[D", "\x1b[C", "\x1b[A", "\x1b[B",            // arrows (up/down = history)
	"\x01", "\x05",                                      // ctrl-a / ctrl-e
	"\x1b[H", "\x1b[F",                                  // home / end
	"\x7f", "\x1b[3~",                                   // backspace / delete
	"\x17", "\x1b\x7f", "\x15",                          // ctrl-w / alt-bs / ctrl-u
	"\x19",                                              // yank
	"\x1bb", "\x1bf", "\x1bd",                           // alt-b / alt-f / alt-d
	"\x1b[1;5D", "\x1b[1;5C",                            // word left / right
	"\r", "\n", "\x1b\r", "\x1b[13;2~",                  // enter variants
	"\\", "\t", "\x1b[Z", "\x1b",                        // misc
	"\x12", "\x10", "\x0e",                              // ctrl-r / ctrl-p / ctrl-n
];

export function sampleEditorKey(r, { paste = true } = {}) {
	const roll = r();
	if (roll < 0.7) return pick(r, EDITOR_KEYS);
	// Paste: the editor turns the pasted content into an atomic "[paste
	// #N M chars]" marker grapheme. `wordWrapLine` infinitely recurses when
	// a single grapheme is wider than the render width (upstream bug,
	// present in BOTH implementations — TS RangeError, Rust stack-overflow
	// abort), so pastes are only emitted when every render width in the
	// case is >= PASTE_SAFE_WIDTH.
	if (roll < 0.8) {
		return paste
			? "\x1b[200~" + sampleText(r, { maxLen: 10 }) + "\x1b[201~"
			: pick(r, EDITOR_KEYS);
	}
	if (roll < 0.9) return sampleUnicode(r, { maxLen: 3 });
	return sampleKeyString(r, { maxLen: 4 }); // malformed fragments
}

// ------------------------------------------------------ tui-components

const COMPONENT_WIDTHS = [0, 1, 2, 3, 5, 7, 10, 13, 20, 40, 80];

function genComponentStep(r) {
	const step = {};
	const kind = int(r, 0, 6);
	if (kind <= 1) {
		step.inputs = [];
		const n = int(r, 1, 3);
		for (let i = 0; i < n; i++) step.inputs.push(pick(r, EDITOR_KEYS));
	} else if (kind === 2) {
		step.ticks = int(r, 1, 4);
	} else if (kind === 3) {
		step.setFilter = pick(r, ["", "a", "he", "zz", "o"]);
	} else if (kind === 4) {
		step.scroll = [];
		const n = int(r, 1, 2);
		for (let i = 0; i < n; i++) {
			const op = {};
			const k = int(r, 0, 4);
			if (k === 0) op.updateLayout = [int(r, 0, 20), int(r, 0, 10)];
			else if (k === 1) op.scrollTo = int(r, 0, 20);
			else if (k === 2) op.scrollBy = int(r, -10, 10);
			else if (k === 3) op.scrollToStart = true;
			else op.scrollToEnd = true;
			if (bool(r)) op.disableFollow = true;
			step.scroll.push(op);
		}
	} else if (kind === 5) {
		const m = {
			type: pick(r, ["press", "release", "click", "move", "wheel"]),
			button: pick(r, ["left", "right", "middle", "none"]),
			x: int(r, 0, 15),
			y: int(r, 0, 8),
			width: int(r, 1, 20),
			height: int(r, 1, 10),
		};
		if (bool(r)) m.shift = bool(r);
		if (bool(r)) m.alt = bool(r);
		if (bool(r)) m.ctrl = bool(r);
		if (m.type === "wheel") m.wheelDelta = int(r, -5, 5);
		if (m.type === "click") m.clickCount = int(r, 1, 3);
		step.mouse = m;
	}
	if (bool(r)) step.render = true;
	return step;
}

export const COMPONENTS_PER_PAYLOAD = 20;

/** One tui-components payload = ~20 sub-cases (native batch). */
export function genComponentsPayload(r) {
	const cases = [];
	for (let i = 0; i < COMPONENTS_PER_PAYLOAD; i++) {
		const c = { name: "b" + i, component: genComponent(r, 0, "components") };
		const widths = [];
		const w = int(r, 1, 2);
		for (let j = 0; j < w; j++) widths.push(pick(r, COMPONENT_WIDTHS));
		if (widths.length) c.widths = widths;
		const steps = [];
		const n = int(r, 0, 4);
		for (let j = 0; j < n; j++) steps.push(genComponentStep(r));
		if (steps.length) c.script = steps;
		cases.push(c);
	}
	return { cases };
}

// ---------------------------------------------------------- tui-editor

/** One tui-editor case (tool-batch item). */
export function genEditor(r) {
	const c = { name: "f", group: "tui-editor", theme: "default", script: [] };
	// Narrow mode (widths 2..23) skips paste inputs: paste markers are
	// ~19+ cells wide and wordWrapLine infinitely recurses when a single
	// grapheme exceeds the render width (upstream bug in BOTH
	// implementations — see sampleEditorKey). Regular graphemes are at
	// most 2 cells wide, so width >= 2 is safe without pastes.
	const narrow = bool(r);
	const wMin = narrow ? 2 : 24;
	if (bool(r)) c.width = int(r, wMin, 40);
	if (bool(r)) {
		c.options = { paddingX: int(r, 0, 3) };
		if (bool(r)) c.options.autocompleteMaxVisible = int(r, 1, 5);
	}
	if (bool(r)) c.focused = bool(r);
	if (bool(r)) c.disableSubmit = bool(r);
	if (bool(r)) {
		const h = [];
		const n = int(r, 0, 3);
		for (let i = 0; i < n; i++) h.push(sampleText(r, { maxLen: 16 }));
		c.history = h;
	}
	if (bool(r)) c.setText = sampleText(r, { maxLen: 30 });
	const n = int(r, 3, 24);
	for (let i = 0; i < n; i++) {
		const roll = r();
		if (roll < 0.68) c.script.push({ input: sampleEditorKey(r, { paste: !narrow }) });
		else if (roll < 0.74) c.script.push({ setText: sampleText(r, { maxLen: 16 }) });
		else if (roll < 0.8) c.script.push({ insertTextAtCursor: sampleText(r, { maxLen: 8 }) });
		else if (roll < 0.84) c.script.push({ addToHistory: sampleText(r, { maxLen: 12 }) });
		else if (roll < 0.92) c.script.push({ render: bool(r) ? true : int(r, wMin, 40) });
		else c.script.push({ tickMs: int(r, 0, 60) });
	}
	return c;
}

// ------------------------------------------------- tui-main / tui-alt

const TERMINAL_REPLIES = [
	"\x1b[?1u", "\x1b[?0u", "\x1b[?u", "\x1b[?62;c",
	"\x1b[>1;10;0c", "\x1b[?1;2c",
	"\x1b[?2026;2$y", "\x1b[?2026;1$y", "\x1b[?2026;0$y",
	"\x1b]8;;https://x.dev\x1b\\", "\x1b]8;;\x1b\\",
	"\x1b_pi:fc\x07", "\x1b_pi:/fc\x07",
];

/** Screen input strings: keys, SGR mouse, focus reporting, paste, soup. */
export function sampleScreenInput(r) {
	const roll = r();
	if (roll < 0.28) return pick(r, ["a", "b", "hello", " ", "x y", "é", "😀", "你"]);
	if (roll < 0.46) return pick(r, EDITOR_KEYS);
	if (roll < 0.62) {
		// SGR mouse: press/drag/motion/wheel with optional modifiers.
		const btn = pick(r, [0, 1, 2, 32, 34, 35, 64, 65, 66, 4, 8, 16, 68, 72, 80]);
		const col = int(r, 1, 20);
		const row = int(r, 1, 10);
		const kind = pick(r, ["M", "m"]);
		return `\x1b[<${btn};${col};${row}${kind}`;
	}
	if (roll < 0.68) return pick(r, ["\x1b[I", "\x1b[O"]); // focus reporting
	if (roll < 0.76) return "\x1b[200~" + sampleText(r, { maxLen: 10 }) + "\x1b[201~"; // paste
	if (roll < 0.88) return sampleUnicode(r, { maxLen: 4 });
	return sampleKeyString(r, { maxLen: 6 }); // malformed fragments
}

function overlayOptions(r) {
	const o = {};
	if (bool(r)) o.width = int(r, 1, 30);
	if (bool(r)) o.minWidth = int(r, 0, 20);
	if (bool(r)) o.maxHeight = int(r, 0, 15);
	if (bool(r)) o.anchor = pick(r, ["center", "top-left", "top-right", "bottom-left", "bottom-right"]);
	if (bool(r)) o.offsetX = int(r, -3, 3);
	if (bool(r)) o.offsetY = int(r, -3, 3);
	if (bool(r)) o.row = int(r, 0, 10);
	if (bool(r)) o.col = int(r, 0, 20);
	if (bool(r)) o.margin = int(r, 0, 3);
	if (bool(r)) o.visible = "always";
	if (bool(r)) o.nonCapturing = bool(r);
	return o;
}

/** One tui-main / tui-alt case (tool-batch item). */
export function genScreen(r, alt) {
	const c = { name: "f", script: [] };
	c.cols = pick(r, [10, 13, 20, 40, 80]);
	c.rows = pick(r, [3, 4, 5, 10, 24]);
	if (bool(r)) c.showHardwareCursor = bool(r);
	if (bool(r)) c.clearOnShrink = bool(r);
	if (bool(r)) {
		c.capabilities = {
			images: pick(r, [null, null, "kitty"]),
			trueColor: bool(r),
			hyperlinks: bool(r),
		};
	}
	if (alt) {
		if (bool(r)) {
			c.altOptions = {};
			if (bool(r)) c.altOptions.wheelScrollLines = int(r, 1, 6);
			if (bool(r)) c.altOptions.mouse = bool(r);
			if (bool(r)) c.altOptions.copyOnSelect = bool(r);
			if (bool(r)) c.altOptions.searchMatchStyle = "identity";
			if (bool(r)) c.altOptions.searchCurrentMatchStyle = "identity";
		}
		if (r() < 0.3) c.screenLines = true;
	}
	let refs = 0;
	const n = int(r, 3, 18);
	for (let i = 0; i < n; i++) {
		const roll = r();
		if (roll < 0.16) {
			const spec = genComponent(r, 0, "screen");
			c.script.push({ type: "addChild", component: spec });
			refs += specCount(spec);
		} else if (roll < 0.4) {
			c.script.push({ type: "input", data: sampleScreenInput(r) });
		} else if (roll < 0.46) {
			c.script.push({ type: "terminalReply", data: pick(r, TERMINAL_REPLIES) });
		} else if (roll < 0.52 && refs > 0) {
			c.script.push({ type: "setFocus", ref: r() < 0.15 ? null : "c" + int(r, 0, refs - 1) });
		} else if (roll < 0.58) {
			const spec = genComponent(r, 0, "screen");
			const op = { type: "showOverlay", component: spec };
			if (bool(r)) op.options = overlayOptions(r);
			c.script.push(op);
			refs += specCount(spec);
		} else if (roll < 0.62) {
			c.script.push({ type: "hideOverlay" });
		} else if (roll < 0.68) {
			c.script.push({ type: "requestRender", force: bool(r) });
		} else if (roll < 0.74) {
			c.script.push({ type: "renderNow", force: bool(r) });
		} else if (roll < 0.8) {
			c.script.push({ type: "resize", cols: pick(r, [0, 1, 2, 5, 10, 20, 40, 80]), rows: pick(r, [0, 1, 2, 3, 5, 10, 24]) });
		} else if (roll < 0.86) {
			c.script.push({ type: "tickMs", n: pick(r, [0, 1, 5, 10, 20, 50, 120, 300]) });
		} else if (roll < 0.9) {
			c.script.push({ type: "start" });
		} else if (roll < 0.93) {
			c.script.push({ type: "stop", preserveScreen: bool(r) });
		} else if (alt) {
			const roll2 = r();
			if (roll2 < 0.4) {
				const op = { type: "setLayoutRoot" };
				if (bool(r)) {
					const spec = genComponent(r, 0, "screen");
					op.component = spec;
					refs += specCount(spec);
				}
				c.script.push(op);
			} else if (roll2 < 0.7) {
				c.script.push({ type: "scrollBy", lines: int(r, -20, 20) });
			} else {
				c.script.push({ type: "flash", message: pick(r, WORDS), durationMs: pick(r, [0, 10, 100, 1000]) });
			}
		} else {
			c.script.push({ type: "input", data: sampleScreenInput(r) });
		}
	}
	return c;
}
