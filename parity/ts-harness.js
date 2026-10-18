#!/usr/bin/env node
/**
 * TS-side parity harness.
 *
 * Drives the ORIGINAL pi coding-agent tools (built dist) with a JSON case on
 * stdin: { "tool": "...", "args": { ... } }. The working directory is passed
 * as argv[2]. Prints the tool result as JSON on stdout (same shape the Rust
 * harness emits), or { "error": "<message>" } when the tool throws.
 *
 * Mirrors the pi agent loop: `tool.prepareArguments(args)` is applied before
 * `tool.execute(toolCallId, args)`.
 *
 * Module-level entries (not AgentTools) drive the dist modules directly,
 * mirroring the corresponding Rust harness entries:
 * - "bash-executor": executeBashWithOperations with local shell operations
 * - "output-accumulator": OutputAccumulator append/finish/snapshot/...
 * - "messages": core/messages.js pure functions
 */
const path = require("path");
const DIST = process.env.PI_DIST || "/tmp/pi-repo/packages/coding-agent/dist/index.js";
const distDir = path.dirname(DIST);
const m = require(DIST);
const FACTORIES = {
	read: m.createReadTool,
	write: m.createWriteTool,
	edit: m.createEditTool,
	bash: m.createBashTool,
	grep: m.createGrepTool,
	find: m.createFindTool,
	ls: m.createLsTool,
};

/** Drive module-level APIs (mirrors pi-cli's module harness entries). */
async function runModule(tool, args, cwd) {
	if (tool === "tui-utils") {
		// The TUI package has no dist build; import the TS source directly
		// (Node 24 type-stripping) so we always test the original source.
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src/utils.ts";
		const u = await import(pathToFileURL(TUI_SRC).href);
		switch (args.fn) {
			case "visibleWidth":
				return { result: u.visibleWidth(args.text) };
			case "stripTerminalSequences":
				return { result: u.stripTerminalSequences(args.text) };
			case "normalizeTerminalOutput":
				return { result: u.normalizeTerminalOutput(args.text) };
			case "getActiveBackgroundAnsi":
				return { result: u.getActiveBackgroundAnsi(args.text) };
			case "getGraphemeCellRange": {
				const range = u.getGraphemeCellRange(args.line, args.column);
				return { result: range === undefined ? null : range };
			}
			case "getOsc8LinkAtColumn": {
				const link = u.getOsc8LinkAtColumn(args.line, args.column);
				return { result: link === undefined ? null : link };
			}
			case "truncateToWidth":
				return {
					result: u.truncateToWidth(args.text, args.maxWidth, args.ellipsis ?? "...", args.pad ?? false),
				};
			case "sliceByColumn":
				return { result: u.sliceByColumn(args.line, args.startCol, args.length, args.strict ?? false) };
			case "sliceWithWidth": {
				const r = u.sliceWithWidth(args.line, args.startCol, args.length, args.strict ?? false);
				return { result: { text: r.text, width: r.width } };
			}
			case "extractSegments": {
				const r = u.extractSegments(args.line, args.beforeEnd, args.afterStart, args.afterLen, args.strictAfter ?? false);
				return {
					result: {
						before: r.before,
						beforeWidth: r.beforeWidth,
						after: r.after,
						afterWidth: r.afterWidth,
					},
				};
			}
			case "wrapTextWithAnsi":
				return { result: u.wrapTextWithAnsi(args.text, args.width) };
			case "isWhitespaceChar":
				return { result: u.isWhitespaceChar(args.char) };
			case "isPunctuationChar":
				return { result: u.isPunctuationChar(args.char) };
			default:
				throw new Error("unknown tui-utils fn: " + args.fn);
		}
	}
	if (tool === "tui-fuzzy") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const f = await import(pathToFileURL(TUI_SRC + "/fuzzy.ts").href);
		try {
			switch (args.fn) {
				case "fuzzyMatch":
					return { result: f.fuzzyMatch(args.query, args.text) };
				case "fuzzyFilter": {
					const textField = args.textField;
					const getText = textField ? (item) => String(item[textField]) : (item) => item;
					return { result: f.fuzzyFilter(args.items, args.query, getText) };
				}
				default:
					throw new Error("unknown tui-fuzzy fn: " + args.fn);
			}
		} catch (e) {
			return { error: String(e.message) };
		}
	}
	if (tool === "tui-word-navigation") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const wn = await import(pathToFileURL(TUI_SRC + "/word-navigation.ts").href);
		const { getWordSegmenter } = await import(pathToFileURL(TUI_SRC + "/utils.ts").href);
		const atomic = Array.isArray(args.atomic) ? args.atomic : null;
		const options = atomic ? { isAtomicSegment: (s) => atomic.includes(s) } : undefined;
		switch (args.fn) {
			case "segmentWords":
				return {
					result: args.texts.map((text) =>
						[...getWordSegmenter().segment(text)].map(({ segment, isWordLike }) => ({
							segment,
							isWordLike,
						})),
					),
				};
			case "findWordBackward":
				return { result: args.cursors.map((c) => wn.findWordBackward(args.text, c, options)) };
			case "findWordForward":
				return { result: args.cursors.map((c) => wn.findWordForward(args.text, c, options)) };
			default:
				throw new Error("unknown tui-word-navigation fn: " + args.fn);
		}
	}
	if (tool === "tui-kill-ring") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const { KillRing } = await import(pathToFileURL(TUI_SRC + "/kill-ring.ts").href);
		const ring = new KillRing();
		const results = [];
		for (const op of args.ops || []) {
			switch (op.op) {
				case "push":
					ring.push(op.text, { prepend: !!op.prepend, accumulate: !!op.accumulate });
					break;
				case "peek":
					results.push(ring.peek() ?? null);
					break;
				case "rotate":
					ring.rotate();
					break;
				case "length":
					results.push(ring.length);
					break;
				default:
					throw new Error("unknown kill-ring op: " + op.op);
			}
		}
		return { result: results };
	}
	if (tool === "tui-program-status") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const p = await import(pathToFileURL(TUI_SRC + "/program-status.ts").href);
		try {
			switch (args.fn) {
				case "formatProgramStatus":
					return { result: p.formatProgramStatus(args) };
				case "isProgramStatusReply":
					return { result: p.isProgramStatusReply(args.sequence) };
				default:
					throw new Error("unknown tui-program-status fn: " + args.fn);
			}
		} catch (e) {
			return { error: String(e.message) };
		}
	}
	if (tool === "tui-colors") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const c = await import(pathToFileURL(TUI_SRC + "/colors.ts").href);
		/** Parse a color argument: string (parsed), number (indexed), or object. */
		const color = (value) => {
			if (typeof value === "string" || typeof value === "number") return c.parseColor(value);
			return value;
		};
		try {
			switch (args.fn) {
				case "parseColor":
					return { result: c.parseColor(args.value) };
				case "indexedColor":
					return { result: c.indexedColor(args.index) };
				case "rgbColor":
					return { result: c.rgbColor(args.r, args.g, args.b) };
				case "oklchColor":
					return { result: c.oklchColor(args.l, args.c, args.h) };
				case "okhslColor":
					return { result: c.okhslColor(args.h, args.s, args.l) };
				case "colorToRgb":
					return { result: c.colorToRgb(color(args.color)) };
				case "colorToOklch":
					return { result: c.colorToOklch(color(args.color)) };
				case "colorToOkhsl":
					return { result: c.colorToOkhsl(color(args.color)) };
				case "colorToHex":
					return { result: c.colorToHex(color(args.color)) };
				case "mixColors":
					return {
						result: c.mixColors(
							color(args.first),
							color(args.second),
							args.amount,
							args.space ?? "oklch",
						),
					};
				case "foregroundAnsi":
					return { result: c.foregroundAnsi(color(args.color), args.mode) };
				case "backgroundAnsi":
					return { result: c.backgroundAnsi(color(args.color), args.mode) };
				case "styleText":
					return { result: c.styleText(args.text, args.style, args.mode) };
				case "styleTextWithAnsi":
					return {
						result: c.styleTextWithAnsi(args.text, args.fgAnsi ?? undefined, args.bgAnsi ?? undefined, args.options ?? {}),
					};
				default:
					throw new Error("unknown tui-colors fn: " + args.fn);
			}
		} catch (e) {
			return { error: String(e.message) };
		}
	}
	if (tool === "tui-keys") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const k = await import(pathToFileURL(TUI_SRC + "/keys.ts").href);
		const results = [];
		for (const c of args.cases || []) {
			// Set the global kitty flag per sub-case, then reset it so case
			// ordering can never leak state (mirrors the Rust harness).
			k.setKittyProtocolActive(!!c.kitty);
			const entry = {
				parseKey: k.parseKey(c.input) ?? null,
				decodePrintableKey: k.decodePrintableKey(c.input) ?? null,
				decodeKittyPrintable: k.decodeKittyPrintable(c.input) ?? null,
				isKeyRelease: k.isKeyRelease(c.input),
				isKeyRepeat: k.isKeyRepeat(c.input),
			};
			if (Array.isArray(c.keyIds)) {
				entry.matches = {};
				for (const id of c.keyIds) entry.matches[id] = k.matchesKey(c.input, id);
			}
			results.push(entry);
			k.setKittyProtocolActive(false);
		}
		return { results };
	}
	if (tool === "tui-stdin-buffer") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const { StdinBuffer } = await import(pathToFileURL(TUI_SRC + "/stdin-buffer.ts").href);
		const k = await import(pathToFileURL(TUI_SRC + "/keys.ts").href);
		// Deterministic timers: patch setTimeout/clearTimeout (the module only
		// touches them inside process()/flush(), so patching after import is
		// safe) so scheduled timeouts never depend on real time. The pending
		// timer's callback is invoked synchronously exactly where the case
		// script says — mirroring the Rust harness's fire_timeout().
		let pendingTimerFn = null;
		const origSetTimeout = globalThis.setTimeout;
		const origClearTimeout = globalThis.clearTimeout;
		globalThis.setTimeout = (fn) => {
			pendingTimerFn = fn;
			return 1;
		};
		globalThis.clearTimeout = (id) => {
			if (id === 1) pendingTimerFn = null;
		};
		const toUnits = (s) => {
			const out = [];
			for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i));
			return out;
		};
		try {
			const results = [];
			for (const c of args.cases || []) {
				k.setKittyProtocolActive(!!c.kitty);
				const buffer = new StdinBuffer({
					timeout: c.timeoutMs,
					escapeTimeout: c.escapeTimeoutMs,
				});
				const events = [];
				buffer.on("data", (s) => events.push({ type: "data", units: toUnits(s) }));
				buffer.on("paste", (s) => events.push({ type: "paste", units: toUnits(s) }));
				const script = c.script || {};
				const chunks = script.chunks || [];
				const fireAfterEach = !!script.fireTimeoutAfterEachChunk;
				const fireAt = new Set(script.fireTimeoutAt || []);
				const fire = () => {
					if (pendingTimerFn) {
						const fn = pendingTimerFn;
						pendingTimerFn = null;
						fn();
					}
				};
				for (let i = 0; i < chunks.length; i++) {
					const chunk = chunks[i];
					if (typeof chunk === "string") buffer.process(chunk);
					else if (chunk && Array.isArray(chunk.units))
						buffer.process(String.fromCharCode(...chunk.units));
					else if (chunk && Array.isArray(chunk.bytes)) buffer.process(Buffer.from(chunk.bytes));
					else throw new Error("invalid chunk");
					if (fireAfterEach || fireAt.has(i)) fire();
				}
				results.push({
					events,
					buffer: toUnits(buffer.getBuffer()),
					pendingTimeout: pendingTimerFn
						? buffer.getBuffer() === "\x1b"
							? "escape"
							: "sequence"
						: null,
				});
				buffer.destroy();
				k.setKittyProtocolActive(false);
			}
			return { results };
		} finally {
			globalThis.setTimeout = origSetTimeout;
			globalThis.clearTimeout = origClearTimeout;
		}
	}
	if (tool === "tui-markdown-lex") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const md = await import(pathToFileURL(TUI_SRC + "/components/markdown.ts").href);
		switch (args.fn) {
			case "lex":
				// Token trees are plain JSON: undefined fields vanish under
				// JSON.stringify, matching the Rust serialization contract.
				return { result: md.lexMarkdown(args.source) };
			default:
				throw new Error("unknown tui-markdown-lex fn: " + args.fn);
		}
	}
	if (tool === "tui-terminal-image") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const ti = await import(pathToFileURL(TUI_SRC + "/terminal-image.ts").href);
		// Stateful sub-cases: setup capabilities / cell dimensions first.
		if (args.setup) {
			if (args.setup.capabilities) ti.setCapabilities(args.setup.capabilities);
			if (args.setup.cellDimensions) ti.setCellDimensions(args.setup.cellDimensions);
		}
		try {
			switch (args.fn) {
				case "encodeKitty":
					return { result: ti.encodeKitty(args.base64Data, args.options ?? {}) };
				case "encodeITerm2":
					return { result: ti.encodeITerm2(args.base64Data, args.options ?? {}) };
				case "calculateImageCellSize":
					return {
						result: ti.calculateImageCellSize(
							args.imageDimensions,
							args.maxWidthCells,
							args.maxHeightCells ?? undefined,
							args.cellDimensions ?? { widthPx: 9, heightPx: 18 },
							args.optimizeAspectRatio ?? false,
						),
					};
				case "calculateImageRows":
					return {
						result: ti.calculateImageRows(
							args.imageDimensions,
							args.targetWidthCells,
							args.cellDimensions ?? { widthPx: 9, heightPx: 18 },
						),
					};
				case "getPngDimensions":
					return { result: ti.getPngDimensions(args.base64Data) ?? null };
				case "getGifDimensions":
					return { result: ti.getGifDimensions(args.base64Data) ?? null };
				case "getWebpDimensions":
					return { result: ti.getWebpDimensions(args.base64Data) ?? null };
				case "getImageDimensions":
					return { result: ti.getImageDimensions(args.base64Data, args.mimeType) ?? null };
				case "isImageLine":
					return { result: ti.isImageLine(args.line) };
				case "renderImage": {
					const result = ti.renderImage(args.base64Data, args.imageDimensions, args.options ?? {});
					return { result };
				}
				case "renderImageAndCrop": {
					// renderImage + cropKittyImageLine + getKittyImagePlacement
					// (exercises the metadata registry end-to-end).
					const result = ti.renderImage(args.base64Data, args.imageDimensions, args.options ?? {});
					if (!result) return { result: null };
					const cropped = ti.cropKittyImageLine(result.sequence, args.hiddenRows, args.visibleRows);
					return {
						result: {
							...result,
							cropped,
							placement: ti.getKittyImagePlacement(cropped) ?? null,
						},
					};
				}
				case "hyperlink":
					return { result: ti.hyperlink(args.text, args.url) };
				case "imageFallback":
					return { result: ti.imageFallback(args.mimeType, args.dimensions ?? undefined, args.filename ?? undefined) };
				default:
					throw new Error("unknown tui-terminal-image fn: " + args.fn);
			}
		} finally {
			if (args.setup) {
				ti.resetCapabilitiesCache();
				ti.setCellDimensions({ widthPx: 9, heightPx: 18 });
			}
		}
	}
	if (tool === "tui-terminal-colors") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const t = await import(pathToFileURL(TUI_SRC + "/terminal-colors.ts").href);
		switch (args.fn) {
			case "parseOscColorResponse": {
				const result = t.parseOscColorResponse(args.data) ?? null;
				return { result: result && result.rgb === undefined ? { ...result, rgb: null } : result };
			}
			case "parseTerminalColorSchemeReport":
				return { result: t.parseTerminalColorSchemeReport(args.data) ?? null };
			default:
				throw new Error("unknown tui-terminal-colors fn: " + args.fn);
		}
	}
	if (tool === "tui-components") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const tui = await import(pathToFileURL(TUI_SRC + "/tui.ts").href);
		const load = (name) => import(pathToFileURL(TUI_SRC + "/components/" + name + ".ts").href);
		const [textMod, ttMod, spacerMod, boxMod, hsMod, vsMod, loaderMod, clMod, mrMod, svMod, inputMod, slMod, stlMod] =
			await Promise.all([
				load("text"), load("truncated-text"), load("spacer"), load("box"), load("h-stack"), load("v-stack"),
				load("loader"), load("cancellable-loader"), load("mouse-region"), load("scroll-view"), load("input"),
				load("select-list"), load("settings-list"),
			]);
		const bgPreset = (preset) => {
			if (preset === "identity") return (t) => t;
			if (preset === "bracket") return (t) => `[${t}]`;
			return undefined;
		};
		const stylePreset = () => (t) => t;
		const serializeMouse = (ev) => ({
			kind: "mouse",
			type: ev.type,
			button: ev.button,
			x: ev.x,
			y: ev.y,
			width: ev.width,
			height: ev.height,
			shift: ev.shift,
			alt: ev.alt,
			ctrl: ev.ctrl,
			wheelDelta: ev.wheelDelta ?? null,
			clickCount: ev.clickCount ?? null,
		});
		const serializeItem = (item) =>
			item ? { value: item.value, label: item.label, description: item.description ?? null } : null;
		const build = (spec, log) => {
			switch (spec.type) {
				case "text":
					return new textMod.Text(spec.text ?? "", spec.paddingX ?? 1, spec.paddingY ?? 1, bgPreset(spec.bg));
				case "truncated-text":
					return new ttMod.TruncatedText(spec.text ?? "", spec.paddingX ?? 0, spec.paddingY ?? 0);
				case "spacer":
					return new spacerMod.Spacer(spec.lines ?? 1);
				case "box": {
					const box = new boxMod.Box(spec.paddingX ?? 1, spec.paddingY ?? 1, bgPreset(spec.bg));
					for (const child of spec.children || []) box.addChild(build(child, log));
					return box;
				}
				case "h-stack":
				case "v-stack": {
					const children = (spec.entries || []).map((entry) => {
						const component = build(entry.component, log);
						const opts = {};
						if (entry.basis !== undefined) opts.basis = entry.basis;
						if (entry.grow !== undefined) opts.grow = entry.grow;
						if (entry.shrink !== undefined) opts.shrink = entry.shrink;
						if (entry.minSize !== undefined) opts.minSize = entry.minSize;
						if (entry.maxSize !== undefined) opts.maxSize = entry.maxSize;
						if (entry.visible !== undefined)
							opts.visible = entry.visible === "always" ? () => true : () => false;
						return Object.keys(opts).length ? { component, ...opts } : component;
					});
					const options = {};
					if (spec.gap !== undefined) options.gap = spec.gap;
					if (spec.align !== undefined) options.align = spec.align;
					return spec.type === "h-stack"
						? new hsMod.HStack(children, options)
						: new vsMod.VStack(children, options);
				}
				case "loader":
					return new loaderMod.Loader(
						null,
						stylePreset(),
						stylePreset(),
						spec.message ?? "Loading...",
						spec.frames !== undefined || spec.intervalMs !== undefined
							? {
									...(spec.frames !== undefined ? { frames: spec.frames } : {}),
									...(spec.intervalMs !== undefined ? { intervalMs: spec.intervalMs } : {}),
								}
							: undefined,
					);
				case "cancellable-loader": {
					const loader = new clMod.CancellableLoader(
						null,
						stylePreset(),
						stylePreset(),
						spec.message ?? "Loading...",
						undefined,
					);
					if (spec.onAbort === "record") loader.onAbort = () => log.push({ kind: "abort" });
					return loader;
				}
				case "mouse-region":
					return new mrMod.MouseRegion(
						build(spec.child, log),
						spec.onMouse === "record"
							? (ev) => {
									log.push(serializeMouse(ev));
									return undefined;
								}
							: () => undefined,
					);
				case "scroll-view":
					return new svMod.ScrollView(build(spec.child, log), {
						...(spec.follow !== undefined ? { follow: spec.follow } : {}),
						...(spec.primary !== undefined ? { primary: spec.primary } : {}),
					});
				case "input": {
					const input = new inputMod.Input({
						...(spec.prompt !== undefined ? { prompt: spec.prompt } : {}),
						...(spec.placeholder !== undefined ? { placeholder: spec.placeholder } : {}),
					});
					if (spec.focused) input.focused = true;
					if (spec.onSubmit === "record") input.onSubmit = (value) => log.push({ kind: "submit", value });
					if (spec.onEscape === "record") input.onEscape = () => log.push({ kind: "escape" });
					return input;
				}
				case "select-list": {
					const items = (spec.items || []).map((it) => ({
						value: it.value,
						label: it.label,
						...(it.description !== undefined ? { description: it.description } : {}),
					}));
					const theme = {
						selectedPrefix: (t) => t,
						selectedText: (t) => t,
						description: (t) => t,
						scrollInfo: (t) => t,
						noMatch: (t) => t,
					};
					const layout = {};
					if (spec.layoutOptions) {
						if (spec.layoutOptions.minPrimaryColumnWidth !== undefined)
							layout.minPrimaryColumnWidth = spec.layoutOptions.minPrimaryColumnWidth;
						if (spec.layoutOptions.maxPrimaryColumnWidth !== undefined)
							layout.maxPrimaryColumnWidth = spec.layoutOptions.maxPrimaryColumnWidth;
					}
					const list = new slMod.SelectList(items, spec.maxVisible ?? 5, theme, layout);
					if (spec.onSelect === "record")
						list.onSelect = (it) => log.push({ kind: "select", ...serializeItem(it) });
					if (spec.onCancel === "record") list.onCancel = () => log.push({ kind: "cancel" });
					if (spec.onSelectionChange === "record")
						list.onSelectionChange = (it) => log.push({ kind: "selectionChange", ...serializeItem(it) });
					return list;
				}
				case "settings-list": {
					const items = (spec.items || []).map((it) => {
						const out = { id: it.id, label: it.label, currentValue: it.currentValue };
						if (it.description !== undefined) out.description = it.description;
						if (it.values !== undefined) out.values = it.values;
						return out;
					});
					const theme = {
						label: (t) => t,
						value: (t) => t,
						description: (t) => t,
						cursor: "> ",
						hint: (t) => t,
					};
					const onChange =
						spec.onChange === "record"
							? (id, value) => log.push({ kind: "change", id, value })
							: () => {};
					const onCancel =
						spec.onCancel === "record" ? () => log.push({ kind: "cancel" }) : () => {};
					return new stlMod.SettingsList(items, spec.maxVisible ?? 5, theme, onChange, onCancel, {
						enableSearch: spec.enableSearch ?? false,
					});
				}
				default:
					throw new Error("unknown tui-components type: " + spec.type);
			}
		};
		// Deterministic loader timers: patch setInterval/clearInterval so the
		// loader's animation callback is stored (never scheduled); a script
		// "tick" invokes it synchronously — mirroring the Rust harness's
		// Loader::tick().
		let intervalFn = null;
		const origSetInterval = globalThis.setInterval;
		const origClearInterval = globalThis.clearInterval;
		globalThis.setInterval = (fn) => {
			intervalFn = fn;
			return 2;
		};
		globalThis.clearInterval = (id) => {
			if (id === 2) intervalFn = null;
		};
		try {
			const results = [];
			for (const c of args.cases || []) {
				intervalFn = null;
				const log = [];
				const component = build(c.component, log);
				const frames = [];
				let lastWidth = 40;
				const doRender = (w) => {
					lastWidth = w;
					frames.push({ width: w, lines: component.render(w) });
				};
				for (const w of c.widths || []) doRender(w);
				for (const step of c.script || []) {
					for (const s of step.inputs || []) {
						if (component.handleInput) component.handleInput(s);
					}
					for (let i = 0; i < (step.ticks || 0); i++) {
						if (intervalFn) intervalFn();
					}
					if (step.setFilter !== undefined && component.setFilter)
						component.setFilter(step.setFilter);
					for (const op of step.scroll || []) {
						if (op.updateLayout) component.updateLayout(op.updateLayout[0], op.updateLayout[1], () => {});
						if (op.scrollTo !== undefined)
							component.scrollTo(op.scrollTo, op.disableFollow ? { disableFollow: true } : {});
						if (op.scrollBy !== undefined) component.scrollBy(op.scrollBy);
						if (op.scrollToStart) component.scrollToStart();
						if (op.scrollToEnd) component.scrollToEnd();
					}
					if (step.mouse) {
						const m = step.mouse;
						const ev = {
							type: m.type,
							button: m.button ?? "none",
							x: m.x,
							y: m.y,
							screenX: m.x,
							screenY: m.y,
							width: m.width,
							height: m.height,
							shift: !!m.shift,
							alt: !!m.alt,
							ctrl: !!m.ctrl,
						};
						if (m.wheelDelta !== undefined) ev.wheelDelta = m.wheelDelta;
						if (m.clickCount !== undefined) ev.clickCount = m.clickCount;
						const res = tui.dispatchMouseEvent(component, ev);
						log.push({
							kind: "dispatch",
							result: res
								? {
										handled: res.handled,
										capture: res.capture ?? null,
										focus: res.focus ?? null,
										render: res.render ?? null,
									}
								: null,
						});
					}
					if (step.render) doRender(step.width ?? lastWidth);
				}
				const state = {};
				if (c.component.type === "input") state.value = component.getValue();
				if (c.component.type === "select-list") state.selected = serializeItem(component.getSelectedItem());
				if (c.component.type === "cancellable-loader") state.aborted = component.aborted;
				if (c.component.type === "scroll-view") {
					state.scrollTop = component.scrollTop;
					state.viewportHeight = component.viewportHeight;
					state.followingEnd = component.isFollowingEnd;
				}
				results.push({ name: c.name, frames, events: log, state });
			}
			return { results };
		} finally {
			globalThis.setInterval = origSetInterval;
			globalThis.clearInterval = origClearInterval;
		}
	}
	if (tool === "tui-editor") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const { Editor } = await import(pathToFileURL(TUI_SRC + "/components/editor.ts").href);
		// Identity theme (preset "default"): borderColor and every selectList
		// member are (t) => t — the Rust side mirrors this exactly (like the
		// tui-components "identity" presets). No chalk anywhere.
		const identityEditorTheme = {
			borderColor: (t) => t,
			selectList: {
				selectedPrefix: (t) => t,
				selectedText: (t) => t,
				description: (t) => t,
				scrollInfo: (t) => t,
				noMatch: (t) => t,
			},
		};
		// Deterministic timers: patch setTimeout/clearTimeout so debounce
		// timers (20ms) never depend on real time. A virtual clock is advanced
		// by script op {tickMs: n}; due callbacks fire in scheduling order —
		// mirroring the Rust harness's fire_due_timers().
		let clock = 0;
		let timerSeq = 0;
		const timers = new Map(); // id -> { fn, at, seq }
		const origSetTimeout = globalThis.setTimeout;
		const origClearTimeout = globalThis.clearTimeout;
		globalThis.setTimeout = (fn, delay) => {
			timerSeq += 1;
			timers.set(timerSeq, { fn, at: clock + Math.max(0, Number(delay) || 0), seq: timerSeq });
			return timerSeq;
		};
		globalThis.clearTimeout = (id) => {
			timers.delete(id);
		};
		const fireDue = (now) => {
			for (;;) {
				let next = null;
				for (const t of timers.values()) {
					if (t.at <= now && (next === null || t.seq < next.seq)) next = t;
				}
				if (next === null) return;
				timers.delete(next.seq);
				next.fn();
			}
		};
		try {
			// One §6 case per invocation: args = the case itself.
			const c = args;
			const fakeTui = {
				renderRequests: 0,
				requestRender() {
					this.renderRequests++;
				},
				terminal: { rows: 24, cols: 80 },
			};
			const editor = new Editor(fakeTui, identityEditorTheme, c.options ?? {});
			const onChange = [];
			const onSubmit = [];
			editor.onChange = (text) => onChange.push(text);
			editor.onSubmit = (text) => onSubmit.push(text);
			if (c.focused) editor.focused = true;
			if (c.disableSubmit) editor.disableSubmit = true;
			for (const h of c.history ?? []) editor.addToHistory(h);
			if (c.setText !== undefined) editor.setText(c.setText);
			const frames = [];
			const doRender = (w) => frames.push({ width: w, lines: editor.render(w) });
			for (const op of c.script ?? []) {
				if (op.input !== undefined) editor.handleInput(op.input);
				else if (op.setText !== undefined) editor.setText(op.setText);
				else if (op.insertTextAtCursor !== undefined) editor.insertTextAtCursor(op.insertTextAtCursor);
				else if (op.addToHistory !== undefined) editor.addToHistory(op.addToHistory);
				else if (op.render !== undefined)
					doRender(op.render === true ? (c.width ?? 80) : op.render);
				else if (op.tickMs !== undefined) {
					clock += op.tickMs;
					fireDue(clock);
				} else throw new Error("invalid tui-editor script op: " + JSON.stringify(Object.keys(op)));
			}
			return {
				name: c.name,
				text: editor.getText(),
				expandedText: editor.getExpandedText(),
				cursor: editor.getCursor(),
				lines: editor.getLines(),
				frames,
				renderRequests: fakeTui.renderRequests,
				events: { onChange, onSubmit },
			};
		} finally {
			globalThis.setTimeout = origSetTimeout;
			globalThis.clearTimeout = origClearTimeout;
		}
	}
	if (tool === "tui-main" || tool === "tui-alt") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const tui = await import(pathToFileURL(TUI_SRC + "/tui.ts").href);
		const mainMod = await import(pathToFileURL(TUI_SRC + "/tui-main-screen.ts").href);
		const altMod = await import(pathToFileURL(TUI_SRC + "/tui-alt-screen.ts").href);
		const tiMod = await import(pathToFileURL(TUI_SRC + "/terminal-image.ts").href);
		const load = (name) => import(pathToFileURL(TUI_SRC + "/components/" + name + ".ts").href);
		const [textMod, ttMod, spacerMod, boxMod, hsMod, vsMod, loaderMod, clMod, mrMod, svMod, inputMod, slMod, stlMod] =
			await Promise.all([
				load("text"), load("truncated-text"), load("spacer"), load("box"), load("h-stack"), load("v-stack"),
				load("loader"), load("cancellable-loader"), load("mouse-region"), load("scroll-view"), load("input"),
				load("select-list"), load("settings-list"),
			]);
		const bgPreset = (preset) => {
			if (preset === "identity") return (t) => t;
			if (preset === "bracket") return (t) => `[${t}]`;
			return undefined;
		};
		const stylePreset = () => (t) => t;

		// Deterministic environment: the TS screens read TERM/TMUX/ZELLIJ/
		// STY (mouse sequence), TERMUX_VERSION (main screen), WEZTERM_PANE/
		// TERM_PROGRAM (WezTerm interleave), SSH_* (wheel scroll), and the
		// PI_* overrides (capabilities). The harness pins an empty env on
		// both sides (T5_PLAN divergences) and pins capabilities explicitly.
		const ENV_KEYS = [
			"TERM", "TMUX", "ZELLIJ", "STY", "TERMUX_VERSION", "WEZTERM_PANE", "TERM_PROGRAM",
			"COLORTERM", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY",
			"PI_IMAGE_PROTOCOL", "PI_TRUE_COLOR", "PI_HYPERLINKS",
			"PI_TUI_DEBUG", "PI_TUI_DEBUG_REDRAW",
		];
		const savedEnv = {};
		for (const key of ENV_KEYS) {
			if (process.env[key] !== undefined) savedEnv[key] = process.env[key];
			delete process.env[key];
		}

		// Virtual clock: performance.now / Date.now / setTimeout /
		// clearTimeout / setInterval / clearInterval / process.nextTick are
		// all patched. `tickMs` advances the clock; due callbacks fire in
		// (deadline, scheduling order) with the nextTick queue drained after
		// each callback — mirroring Node's event loop and the Rust
		// harness's advance_clock().
		let clock = 0;
		let timerSeq = 0;
		const timeouts = new Map(); // id -> { fn, at, seq }
		const intervals = new Map(); // id -> { fn, period, nextAt, seq }
		const nextTicks = [];
		const origSetTimeout = globalThis.setTimeout;
		const origClearTimeout = globalThis.clearTimeout;
		const origSetInterval = globalThis.setInterval;
		const origClearInterval = globalThis.clearInterval;
		const origNextTick = process.nextTick.bind(process);
		const origPerfNow = globalThis.performance.now.bind(globalThis.performance);
		const origDateNow = Date.now;
		const handleOf = (id) => ({ __harnessTimerId: id, unref() {}, ref() {} });
		const idOf = (h) => (typeof h === "object" && h !== null && "__harnessTimerId" in h ? h.__harnessTimerId : h);
		globalThis.setTimeout = (fn, delay) => {
			timerSeq += 1;
			timeouts.set(timerSeq, { fn, at: clock + Math.max(0, Number(delay) || 0), seq: timerSeq });
			return handleOf(timerSeq);
		};
		globalThis.clearTimeout = (h) => {
			timeouts.delete(idOf(h));
		};
		globalThis.setInterval = (fn, period) => {
			timerSeq += 1;
			const p = Math.max(1, Number(period) || 0);
			intervals.set(timerSeq, { fn, period: p, nextAt: clock + p, seq: timerSeq });
			return handleOf(timerSeq);
		};
		globalThis.clearInterval = (h) => {
			intervals.delete(idOf(h));
		};
		process.nextTick = (fn, ...args) => {
			nextTicks.push(() => fn(...args));
		};
		globalThis.performance.now = () => clock;
		Date.now = () => Math.floor(clock);
		const drainNextTicks = () => {
			while (nextTicks.length) {
				const batch = nextTicks.splice(0, nextTicks.length);
				for (const fn of batch) fn();
			}
		};
		const advance = (ms) => {
			clock += ms;
			for (;;) {
				drainNextTicks();
				let best = null;
				for (const [id, t] of timeouts) {
					if (t.at <= clock && (best === null || t.at < best.at || (t.at === best.at && t.seq < best.seq)))
						best = { kind: "timeout", id, at: t.at, seq: t.seq };
				}
				for (const [id, t] of intervals) {
					if (t.nextAt <= clock && (best === null || t.nextAt < best.at || (t.nextAt === best.at && t.seq < best.seq)))
						best = { kind: "interval", id, at: t.nextAt, seq: t.seq };
				}
				if (best === null) return;
				if (best.kind === "timeout") {
					const t = timeouts.get(best.id);
					timeouts.delete(best.id);
					if (t) t.fn();
				} else {
					const t = intervals.get(best.id);
					if (!t) continue;
					t.nextAt += t.period;
					t.fn();
				}
			}
		};

		// RecordingTerminal: implements the TS Terminal interface with the
		// same observability as upstream's VirtualTerminal — only write()
		// lands in the recorded stream; hideCursor/showCursor are counted;
		// everything else is a black hole. kittyProtocolActive mirrors the
		// VirtualTerminal (always true).
		class RecordingTerminal {
			constructor(cols, rows) {
				this._cols = cols;
				this._rows = rows;
				this.writes = [];
				this.hideCount = 0;
				this.showCount = 0;
				this.inputHandler = undefined;
				this.resizeHandler = undefined;
			}
			start(onInput, onResize) {
				this.inputHandler = onInput;
				this.resizeHandler = onResize;
			}
			async drainInput() {}
			stop() {
				this.inputHandler = undefined;
				this.resizeHandler = undefined;
			}
			write(data) {
				this.writes.push(data);
			}
			get columns() {
				return this._cols;
			}
			get rows() {
				return this._rows;
			}
			get kittyProtocolActive() {
				return true;
			}
			moveBy() {}
			hideCursor() {
				this.hideCount++;
			}
			showCursor() {
				this.showCount++;
			}
			clearLine() {}
			clearFromCursor() {}
			clearScreen() {}
			setTitle() {}
			setProgress() {}
			setProgramStatus() {}
			sendInput(data) {
				if (this.inputHandler) this.inputHandler(data);
			}
			resize(cols, rows) {
				if (cols === this._cols && rows === this._rows) return;
				this._cols = cols;
				this._rows = rows;
				if (this.resizeHandler) this.resizeHandler();
			}
		}

		try {
			// One case per invocation: args = the case itself.
			const c = args;
			const caps = c.capabilities ?? { images: null, trueColor: true, hyperlinks: true };
			tiMod.setCapabilities({
				images: caps.images ?? null,
				trueColor: caps.trueColor ?? true,
				hyperlinks: caps.hyperlinks ?? true,
			});

			// Component spec builder (same vocabulary as the tui-components
			// tool, plus scroll-view scrollbar options). Every built
			// component is registered under an auto ref ("c0", "c1", …) in
			// build order so script ops can reference it (setFocus) and the
			// output can name the focused component.
			const registry = new Map();
			const build = (spec) => {
				const register = (component) => {
					const ref = "c" + registry.size;
					registry.set(ref, component);
					return component;
				};
				switch (spec.type) {
					case "text":
						return register(new textMod.Text(spec.text ?? "", spec.paddingX ?? 1, spec.paddingY ?? 1, bgPreset(spec.bg)));
					case "truncated-text":
						return register(new ttMod.TruncatedText(spec.text ?? "", spec.paddingX ?? 0, spec.paddingY ?? 0));
					case "spacer":
						return register(new spacerMod.Spacer(spec.lines ?? 1));
					case "box": {
						const box = new boxMod.Box(spec.paddingX ?? 1, spec.paddingY ?? 1, bgPreset(spec.bg));
						for (const child of spec.children || []) box.addChild(build(child));
						return register(box);
					}
					case "h-stack":
					case "v-stack": {
						const children = (spec.entries || []).map((entry) => {
							const component = build(entry.component);
							const opts = {};
							if (entry.basis !== undefined) opts.basis = entry.basis;
							if (entry.grow !== undefined) opts.grow = entry.grow;
							if (entry.shrink !== undefined) opts.shrink = entry.shrink;
							if (entry.minSize !== undefined) opts.minSize = entry.minSize;
							if (entry.maxSize !== undefined) opts.maxSize = entry.maxSize;
							if (entry.visible !== undefined)
								opts.visible = entry.visible === "always" ? () => true : () => false;
							return Object.keys(opts).length ? { component, ...opts } : component;
						});
						const options = {};
						if (spec.gap !== undefined) options.gap = spec.gap;
						if (spec.align !== undefined) options.align = spec.align;
						return register(
							spec.type === "h-stack" ? new hsMod.HStack(children, options) : new vsMod.VStack(children, options),
						);
					}
					case "loader":
						return register(
							new loaderMod.Loader(null, stylePreset(), stylePreset(), spec.message ?? "Loading...", {
								...(spec.frames !== undefined ? { frames: spec.frames } : {}),
								...(spec.intervalMs !== undefined ? { intervalMs: spec.intervalMs } : {}),
							}),
						);
					case "cancellable-loader":
						return register(
							new clMod.CancellableLoader(null, stylePreset(), stylePreset(), spec.message ?? "Loading...", undefined),
						);
					case "mouse-region":
						return register(new mrMod.MouseRegion(build(spec.child), () => undefined));
					case "scroll-view": {
						const options = {};
						if (spec.follow !== undefined) options.follow = spec.follow;
						if (spec.primary !== undefined) options.primary = spec.primary;
						if (spec.overscroll !== undefined) options.overscroll = spec.overscroll;
						if (spec.scrollbar !== undefined) options.scrollbar = spec.scrollbar;
						if (spec.scrollbarTrackStyle === "identity") options.scrollbarTrackStyle = stylePreset();
						if (spec.scrollbarThumbStyle === "identity") options.scrollbarThumbStyle = stylePreset();
						if (spec.scrollbarHideDelayMs !== undefined) options.scrollbarHideDelayMs = spec.scrollbarHideDelayMs;
						return register(new svMod.ScrollView(build(spec.child), options));
					}
					case "input": {
						const input = new inputMod.Input({
							...(spec.prompt !== undefined ? { prompt: spec.prompt } : {}),
							...(spec.placeholder !== undefined ? { placeholder: spec.placeholder } : {}),
						});
						if (spec.focused) input.focused = true;
						return register(input);
					}
					case "select-list": {
						const items = (spec.items || []).map((it) => ({
							value: it.value,
							label: it.label,
							...(it.description !== undefined ? { description: it.description } : {}),
						}));
						const theme = {
							selectedPrefix: (t) => t,
							selectedText: (t) => t,
							description: (t) => t,
							scrollInfo: (t) => t,
							noMatch: (t) => t,
						};
						return register(new slMod.SelectList(items, spec.maxVisible ?? 5, theme, {}));
					}
					case "settings-list": {
						const items = (spec.items || []).map((it) => ({
							id: it.id,
							label: it.label,
							currentValue: it.currentValue,
							...(it.description !== undefined ? { description: it.description } : {}),
							...(it.values !== undefined ? { values: it.values } : {}),
						}));
						const theme = { label: (t) => t, value: (t) => t, description: (t) => t, cursor: "> ", hint: (t) => t };
						return register(
							new stlMod.SettingsList(items, spec.maxVisible ?? 5, theme, () => {}, () => {}, {
								enableSearch: spec.enableSearch ?? false,
							}),
						);
					}
					default:
						throw new Error("unknown tui-main/tui-alt component type: " + spec.type);
				}
			};
			const refOf = (spec) => {
				if (spec.ref !== undefined) {
					const component = registry.get(spec.ref);
					if (!component) throw new Error("unknown component ref: " + spec.ref);
					return component;
				}
				return build(spec.component ?? spec);
			};
			const overlayOptions = (o) => {
				const out = {};
				if (!o) return out;
				if (o.width !== undefined) out.width = o.width;
				if (o.minWidth !== undefined) out.minWidth = o.minWidth;
				if (o.maxHeight !== undefined) out.maxHeight = o.maxHeight;
				if (o.anchor !== undefined) out.anchor = o.anchor;
				if (o.offsetX !== undefined) out.offsetX = o.offsetX;
				if (o.offsetY !== undefined) out.offsetY = o.offsetY;
				if (o.row !== undefined) out.row = o.row;
				if (o.col !== undefined) out.col = o.col;
				if (o.margin !== undefined) out.margin = o.margin;
				if (o.visible === "always") out.visible = () => true;
				if (o.nonCapturing !== undefined) out.nonCapturing = o.nonCapturing;
				return out;
			};

			const term = new RecordingTerminal(c.cols ?? 80, c.rows ?? 24);
			let screen;
			if (tool === "tui-alt") {
				const o = c.altOptions ?? {};
				const options = {};
				if (o.wheelScrollLines !== undefined) options.wheelScrollLines = o.wheelScrollLines;
				if (o.mouse !== undefined) options.mouse = o.mouse;
				if (o.searchMatchStyle === "identity") options.searchMatchStyle = (t) => t;
				if (o.searchCurrentMatchStyle === "identity") options.searchCurrentMatchStyle = (t) => t;
				if (o.copyOnSelect !== undefined) options.copyOnSelect = o.copyOnSelect;
				screen = new altMod.TuiAltScreen(term, c.showHardwareCursor ?? false, undefined, options);
			} else {
				screen = new mainMod.TuiMainScreen(term, c.showHardwareCursor ?? false, undefined);
			}
			if (c.clearOnShrink) screen.setClearOnShrink(true);

			for (const op of c.script ?? []) {
				switch (op.type) {
					case "addChild":
						screen.addChild(build(op.component));
						break;
					case "input":
					case "terminalReply":
						term.sendInput(op.data);
						break;
					case "setFocus":
						screen.setFocus(op.ref === null ? null : refOf({ ref: op.ref }));
						break;
					case "showOverlay":
						screen.showOverlay(build(op.component), overlayOptions(op.options));
						break;
					case "hideOverlay":
						screen.hideOverlay();
						break;
					case "requestRender":
						screen.requestRender(op.force ?? false);
						break;
					case "renderNow":
						screen.renderNow(op.force ?? false);
						break;
					case "resize":
						term.resize(op.cols, op.rows);
						break;
					case "tickMs":
						advance(op.n);
						break;
					case "start":
						screen.start();
						break;
					case "stop":
						screen.stop({ preserveScreen: op.preserveScreen ?? false });
						break;
					case "setLayoutRoot":
						if (tool !== "tui-alt") throw new Error("setLayoutRoot is alt-only");
						screen.setLayoutRoot(op.component ? build(op.component) : undefined);
						break;
					case "scrollBy":
						if (tool !== "tui-alt") throw new Error("scrollBy is alt-only");
						screen.scrollBy(op.lines);
						break;
					case "flash":
						if (tool !== "tui-alt") throw new Error("flash is alt-only");
						screen.flash(op.message, op.durationMs);
						break;
					default:
						throw new Error("invalid " + tool + " script op: " + JSON.stringify(Object.keys(op)));
				}
				drainNextTicks();
			}

			// Frames: the concatenated write stream split at every
			// synchronized-output begin marker — one segment per render
			// (plus any pre-first-render writes as a leading segment).
			const BEGIN = "\x1b[?2026h";
			const stream = term.writes.join("");
			const parts = stream.split(BEGIN);
			const frames = [];
			if (parts[0] !== "") frames.push(parts[0]);
			for (let i = 1; i < parts.length; i++) frames.push(BEGIN + parts[i]);
			let focused = null;
			const focusedComponent = screen.getFocusedComponent();
			if (focusedComponent) {
				for (const [ref, component] of registry) {
					if (component === focusedComponent) {
						focused = ref;
						break;
					}
				}
			}
			const out = {
				name: c.name,
				frames,
				writes: term.writes.slice(),
				fullRedraws: screen.fullRedraws,
				hideCursor: term.hideCount,
				showCursor: term.showCount,
				focused,
			};
			if (tool === "tui-alt") {
				out.viewportTop = screen.viewportTop;
				out.followingOutput = screen.isFollowingOutput;
				if (c.screenLines) out.screenLines = screen.getScreenLines();
			}
			return out;
		} finally {
			globalThis.setTimeout = origSetTimeout;
			globalThis.clearTimeout = origClearTimeout;
			globalThis.setInterval = origSetInterval;
			globalThis.clearInterval = origClearInterval;
			process.nextTick = origNextTick;
			globalThis.performance.now = origPerfNow;
			Date.now = origDateNow;
			for (const key of ENV_KEYS) delete process.env[key];
			for (const [key, value] of Object.entries(savedEnv)) process.env[key] = value;
			tiMod.resetCapabilitiesCache();
		}
	}
	if (tool === "math") {
		const buf = new ArrayBuffer(8);
		const view = new DataView(buf);
		const toHex = (f) => {
			view.setFloat64(0, f, false);
			const hex = Array.from({ length: 8 }, (_, i) => view.getUint8(i).toString(16).padStart(2, "0")).join("");
			return hex;
		};
		const results = [];
		for (const op of args.ops) {
			const fn = Math[op.fn];
			if (typeof fn !== "function") throw new Error("unknown math fn: " + op.fn);
			results.push(toHex(fn(...op.args)));
		}
		return { results };
	}
	if (tool === "bash-executor") {
		const { executeBashWithOperations } = require(path.join(distDir, "core", "bash-executor.js"));
		const { createLocalBashOperations } = require(path.join(distDir, "core", "tools", "bash.js"));
		return await executeBashWithOperations(args.command, cwd, createLocalBashOperations(), {});
	}
	if (tool === "output-accumulator") {
		const { OutputAccumulator } = require(path.join(distDir, "core", "tools", "output-accumulator.js"));
		const acc = new OutputAccumulator({
			maxLines: args.maxLines,
			maxBytes: args.maxBytes,
			tempFilePrefix: args.tempFilePrefix,
		});
		for (const chunk of args.chunks || []) {
			acc.append(Buffer.from(chunk, "base64"));
		}
		const out = {};
		if (args.finish !== false) acc.finish();
		if (args.snapshot !== false) {
			out.snapshot = acc.snapshot({ persistIfTruncated: args.persistIfTruncated !== false });
		}
		await acc.closeTempFile();
		if (args.readFullOutputMaxBytes != null) {
			out.full = await acc.readFullOutput(args.readFullOutputMaxBytes);
		}
		out.lastLineBytes = acc.getLastLineBytes();
		return out;
	}
	if (tool === "session-manager") {
		const sm = require(path.join(distDir, "core", "session-manager.js"));
		switch (args.fn) {
			case "assertValidSessionId":
				sm.assertValidSessionId(args.id);
				return { valid: true };
			case "migrateSessionEntries": {
				const entries = args.entries;
				sm.migrateSessionEntries(entries);
				return { entries };
			}
			case "parseSessionEntries":
				return { entries: sm.parseSessionEntries(args.content) };
			case "getLatestCompactionEntry":
				return { entry: sm.getLatestCompactionEntry(args.entries) };
			case "sessionEntryToContextMessages":
				return { messages: sm.sessionEntryToContextMessages(args.entry) };
			case "buildContextEntries":
				return { entries: sm.buildContextEntries(args.entries, args.leafId) };
			case "buildSessionProjection":
				return sm.buildSessionProjection(args.entries, args.leafId);
			case "buildSessionContext":
				return sm.buildSessionContext(args.entries, args.leafId);
			case "getDefaultSessionDir": {
				const agentDir = args.agentDir !== undefined ? path.resolve(cwd, args.agentDir) : undefined;
				return { dir: sm.getDefaultSessionDir(args.cwd, agentDir) };
			}
			case "loadEntriesFromFile":
				return { entries: sm.loadEntriesFromFile(path.resolve(cwd, args.filePath)) };
			case "findMostRecentSession": {
				const sessionDir = path.resolve(cwd, args.sessionDir);
				return { path: sm.findMostRecentSession(sessionDir, args.cwd) };
			}
			default:
				throw new Error("unknown session-manager fn: " + args.fn);
		}
	}
	if (tool === "compaction-utils") {
		const cu = require(path.join(distDir, "core", "compaction", "utils.js"));
		const toFileOps = (o) => ({
			read: new Set(o?.read ?? []),
			written: new Set(o?.written ?? []),
			edited: new Set(o?.edited ?? []),
		});
		const fromFileOps = (f) => ({
			read: [...f.read],
			written: [...f.written],
			edited: [...f.edited],
		});
		switch (args.fn) {
			case "createFileOps":
				return { result: fromFileOps(cu.createFileOps()) };
			case "extractFileOpsFromMessage": {
				const fileOps = toFileOps(args.fileOps);
				cu.extractFileOpsFromMessage(args.message, fileOps);
				return { result: fromFileOps(fileOps) };
			}
			case "computeFileLists":
				return { result: cu.computeFileLists(toFileOps(args.fileOps)) };
			case "formatFileOperations":
				return { result: cu.formatFileOperations(args.readFiles ?? [], args.modifiedFiles ?? []) };
			case "serializeConversation":
				return { result: cu.serializeConversation(args.messages ?? []) };
			case "summarizationSystemPrompt":
				return { result: cu.SUMMARIZATION_SYSTEM_PROMPT };
			default:
				throw new Error("unknown compaction-utils fn: " + args.fn);
		}
	}
	if (tool === "compaction") {
		const c = require(path.join(distDir, "core", "compaction", "compaction.js"));
		const sm = require(path.join(distDir, "core", "session-manager.js"));
		switch (args.fn) {
			case "defaultSettings":
				return { result: c.DEFAULT_COMPACTION_SETTINGS };
			case "calculateContextTokens":
				return { result: c.calculateContextTokens(args.usage) };
			case "getLastAssistantUsage":
				return { result: c.getLastAssistantUsage(args.entries) ?? null };
			case "estimateContextTokens":
				return { result: c.estimateContextTokens(args.messages) };
			case "estimateProjectedContextTokens":
				return {
					result: c.estimateProjectedContextTokens(sm.buildSessionProjection(args.entries), args.branchEntries),
				};
			case "shouldCompact":
				return { result: c.shouldCompact(args.contextTokens, args.contextWindow, args.settings) };
			case "estimateTokens":
				return { result: c.estimateTokens(args.message) };
			case "findCutPoint":
				return {
					result: c.findCutPoint(
						args.entries,
						args.startIndex ?? 0,
						args.endIndex ?? args.entries.length,
						args.keepRecentTokens,
					),
				};
			case "findTurnStartIndex":
				return { result: c.findTurnStartIndex(args.entries, args.entryIndex, args.startIndex ?? 0) };
			case "getSummarizationFailure":
				return { result: c.getSummarizationFailure(args.response, args.label) ?? null };
			case "prepareCompaction": {
				const prep = c.prepareCompaction(args.entries, args.settings);
				if (!prep) return { result: null };
				return {
					result: {
						...prep,
						fileOps: {
							read: [...prep.fileOps.read],
							written: [...prep.fileOps.written],
							edited: [...prep.fileOps.edited],
						},
					},
				};
			}
			default:
				throw new Error("unknown compaction fn: " + args.fn);
		}
	}
	if (tool === "nested-calls") {
		const nc = require(path.join(distDir, "core", "nested-tool-calls.js"));
		switch (args.fn) {
			case "limits":
				return { result: nc.NESTED_CALL_LIMITS };
			case "recorder": {
				const recorder = new nc.NestedCallRecorder();
				const records = [];
				const results = [];
				for (const op of args.ops ?? []) {
					switch (op.op) {
						case "start": {
							const rec = recorder.start(op.toolCall);
							if (rec) records.push(rec);
							results.push({ index: rec ? records.length - 1 : null });
							break;
						}
						case "finish":
							recorder.finish(
								op.index === null || op.index === undefined ? undefined : records[op.index],
								op.isError,
								op.error ?? "",
							);
							results.push({});
							break;
						case "addUsage":
							recorder.addUsage(op.usage);
							results.push({});
							break;
						case "totalUsage":
							results.push({ result: recorder.totalUsage ?? null });
							break;
						case "snapshot": {
							const snap = recorder.snapshot();
							if (snap) {
								for (const call of snap.calls) {
									if (call.durationMs !== undefined) call.durationMs = 0;
								}
							}
							results.push({ result: snap ?? null });
							break;
						}
						default:
							throw new Error("unknown nested-calls op: " + op.op);
					}
				}
				return { results };
			}
			default:
				throw new Error("unknown nested-calls fn: " + args.fn);
		}
	}
	if (tool === "messages") {
		const msgs = require(path.join(distDir, "core", "messages.js"));
		switch (args.fn) {
			case "bashExecutionToText":
				return { text: msgs.bashExecutionToText(args.message) };
			case "convertToLlm":
				return { messages: msgs.convertToLlm(args.messages) };
			case "createBranchSummaryMessage":
				return msgs.createBranchSummaryMessage(args.summary, args.fromId, args.timestamp);
			case "createCompactionSummaryMessage":
				return msgs.createCompactionSummaryMessage(args.summary, args.tokensBefore, args.timestamp);
			case "createCustomMessage":
				return msgs.createCustomMessage(args.customType, args.content, args.display, args.details, args.timestamp);
			default:
				throw new Error("unknown messages fn: " + args.fn);
		}
	}
	return undefined;
}

let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => (input += d));
process.stdin.on("end", async () => {
	const { tool, args } = JSON.parse(input);
	const cwd = process.argv[2];
	try {
		const moduleResult = await runModule(tool, args, cwd);
		if (moduleResult !== undefined) {
			process.stdout.write(JSON.stringify(moduleResult));
			return;
		}
		const t = FACTORIES[tool](cwd);
		const prepared = t.prepareArguments ? t.prepareArguments(args) : args;
		const result = await t.execute("parity-call", prepared);
		process.stdout.write(JSON.stringify(result));
	} catch (e) {
		process.stdout.write(
			JSON.stringify({ error: e && e.message !== undefined ? e.message : String(e) }),
		);
	}
});
