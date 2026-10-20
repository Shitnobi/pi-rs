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
 * - "mcp-servers": core/mcp-servers.js pure functions
 * - "mcp-tools": extensions/mcp/tools.js pure conversion functions (the
 *   output saver is a deterministic fake on both sides)
 * - "tool-search": extensions/tool-search/tool.ts tokenizer/document/ranker
 * - "config-value": core/resolve-config-value.js config resolution
 * - "http-dispatcher": core/http-dispatcher.js idle-timeout parse/format
 * - "auth-guidance": core/auth-guidance.js messages (docs path normalized)
 * - "models-store": core/models-store.js in-memory and file stores
 * - "runtime-credentials": core/runtime-credentials.js overlay over the
 *   real pi-ai InMemoryCredentialStore
 * - "virtual-models": core/virtual-models.js virtual model helpers
 * - "provider-attribution": core/provider-attribution.js header merge
 * - "model-config": core/model-config.js load with injected inputs
 * - "model-resolver": core/model-resolver.js resolution functions
 * - "auth-storage": core/auth-storage.js stores (read-only, in-memory,
 *   sequence) with per-case temp files and injected env
 * - "remote-catalog": core/remote-catalog-provider.js overlays with a
 *   scripted fetch response and Date.now
 * - "provider-composer": core/provider-composer.js composition over
 *   scripted base/extension/modelConfig inputs
 * - "event-bus": core/event-bus.js script-driven bus with numbered handlers
 *   and captured console.error handler-error reports
 * - "timings": core/timings.js script-driven timings with an injected
 *   Date.now clock and captured console.error print stream
 * - "slash-commands": core/slash-commands.js builtin list + serde round-trip
 * - "settings-defaults": core/settings-defaults.js SETTINGS_DEFAULTS
 * - "settings-schema": core/settings-schema.js Compile(SettingsSchema)
 *   .Errors(value) rendered through formatValidationPath
 */
const path = require("path");
const DIST = process.env.PI_DIST || "/tmp/pi-repo/packages/coding-agent/dist/index.js";
const distDir = path.dirname(DIST);
// The `crypto` ESM namespace snapshots named exports at creation (probed:
// patching require("crypto").randomUUID AFTER any ESM `import ... from
// "crypto"` is invisible to the importer). Install a pass-through dispatcher
// BEFORE the dist graph is loaded so the settings-manager entry can swap in
// a deterministic generator per case via globalThis.__piParityUuidOverride.
const __realRandomUUID = require("crypto").randomUUID;
require("crypto").randomUUID = function randomUUID(...args) {
	const override = globalThis.__piParityUuidOverride;
	if (override) return override();
	return __realRandomUUID.apply(this, args);
};
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
	if (tool === "tui-latex") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const lx = await import(pathToFileURL(TUI_SRC + "/latex.ts").href);
		switch (args.fn) {
			case "render":
				// null (not undefined) so both sides serialize identically.
				return { result: lx.renderLatex(args.source, args.display ? { display: true } : {}) ?? null };
			default:
				throw new Error("unknown tui-latex fn: " + args.fn);
		}
	}
	if (tool === "tui-markdown-render") {
		const { pathToFileURL } = require("url");
		const TUI_SRC = process.env.PI_TUI_SRC || "/tmp/pi-repo/packages/tui/src";
		const md = await import(pathToFileURL(TUI_SRC + "/components/markdown.ts").href);
		switch (args.fn) {
			case "render": {
				// Deterministic marker theme shared 1:1 with the Rust harness.
				const theme = {
					heading: (t) => `#<${t}>`,
					link: (t) => `L<${t}>`,
					linkUrl: (t) => `U<${t}>`,
					code: (t) => `C<${t}>`,
					codeBlock: (t) => `CB<${t}>`,
					codeBlockBorder: (t) => `B<${t}>`,
					quote: (t) => `Q<${t}>`,
					quoteBorder: (t) => `|`,
					hr: (t) => `-${t}-`,
					listBullet: (t) => `*${t}`,
					bold: (t) => `**${t}**`,
					italic: (t) => `_${t}_`,
					strikethrough: (t) => `~${t}~`,
					underline: (t) => `_<${t}>_`,
				};
				const options = {};
				if (args.options) {
					if (args.options.preserveOrderedListMarkers !== undefined)
						options.preserveOrderedListMarkers = args.options.preserveOrderedListMarkers;
					if (args.options.preserveBackslashEscapes !== undefined)
						options.preserveBackslashEscapes = args.options.preserveBackslashEscapes;
					if (args.options.renderLatex !== undefined)
						options.renderLatex = args.options.renderLatex;
				}
				const component = new md.Markdown(
					args.source,
					args.paddingX ?? 0,
					args.paddingY ?? 0,
					theme,
					undefined,
					options,
				);
				return { result: component.render(args.width ?? 40) };
			}
			default:
				throw new Error("unknown tui-markdown-render fn: " + args.fn);
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
		// Deterministic loader timers: patch setInterval/clearInterval so
		// each loader's animation callback is stored (never scheduled); a
		// script "tick" invokes ALL stored callbacks synchronously —
		// mirroring a real event loop (and the Rust harness's
		// loader-registry tick), including nested loaders.
		let intervalFns = new Map();
		let nextIntervalId = 1;
		const origSetInterval = globalThis.setInterval;
		const origClearInterval = globalThis.clearInterval;
		globalThis.setInterval = (fn) => {
			const id = nextIntervalId++;
			intervalFns.set(id, fn);
			return id;
		};
		globalThis.clearInterval = (id) => {
			intervalFns.delete(id);
		};
		try {
			const results = [];
			for (const c of args.cases || []) {
				intervalFns.clear();
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
						for (const fn of [...intervalFns.values()]) fn();
					}
					if (step.setFilter !== undefined && component.setFilter)
						component.setFilter(step.setFilter);
					for (const op of step.scroll || []) {
						if (op.updateLayout && component.updateLayout)
							component.updateLayout(op.updateLayout[0], op.updateLayout[1], () => {});
						if (op.scrollTo !== undefined && component.scrollTo)
							component.scrollTo(op.scrollTo, op.disableFollow ? { disableFollow: true } : {});
						if (op.scrollBy !== undefined && component.scrollBy) component.scrollBy(op.scrollBy);
						if (op.scrollToStart && component.scrollToStart) component.scrollToStart();
						if (op.scrollToEnd && component.scrollToEnd) component.scrollToEnd();
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
						// Rust harness defaults missing data to "" — align.
						term.sendInput(op.data ?? "");
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
						if (op.cols === undefined) throw new Error("resize requires cols");
						if (op.rows === undefined) throw new Error("resize requires rows");
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
						if (op.lines === undefined) throw new Error("scrollBy requires lines");
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
	if (tool === "mcp-servers") {
		const mcps = require(path.join(distDir, "core", "mcp-servers.js"));
		switch (args.fn) {
			case "createToolNameMatcher": {
				const matcher = mcps.createToolNameMatcher(args.entries);
				return { matches: (args.names || []).map((name) => matcher(name)) };
			}
			case "isMcpToolName":
				return { results: args.names.map((name) => mcps.isMcpToolName(name)) };
			case "mcpNamespace":
				return { namespaces: args.servers.map((server) => mcps.mcpNamespace(server)) };
			case "validateMcpServerConfig": {
				const result = mcps.validateMcpServerConfig(args.name, args.raw);
				return { result: typeof result === "string" ? { error: result } : result };
			}
			case "getMcpToolExposure": {
				const config = mcps.validateMcpServerConfig(args.name, args.raw);
				if (typeof config === "string") throw new Error(config);
				return { exposures: args.toolNames.map((name) => mcps.getMcpToolExposure(config, name)) };
			}
			case "isLoopbackRedirectUri":
				return { results: args.values.map((value) => mcps.isLoopbackRedirectUri(value)) };
			case "parseUrl": {
				// WHATWG `new URL()`, the spec the Rust hand-rolled parser
				// mirrors for the inputs this module sees.
				const results = args.inputs.map((input) => {
					try {
						const url = new URL(input);
						return {
							protocol: url.protocol,
							hostname: url.hostname,
							port: url.port,
							pathname: url.pathname,
							search: url.search,
							hash: url.hash,
						};
					} catch {
						return null;
					}
				});
				return { results };
			}
			default:
				throw new Error("unknown mcp-servers fn: " + args.fn);
		}
	}
	if (tool === "mcp-tools") {
		const mt = require(path.join(distDir, "extensions", "mcp", "tools.js"));
		// Deterministic fake output saver, mirrored by the Rust harness.
		const fakeSaver = async (_data, extension) => "/tmp/pi-mcp-out" + extension;
		switch (args.fn) {
			case "createMcpToolName":
				return {
					results: args.calls.map(({ server, tool, taken }) =>
						mt.createMcpToolName(server, tool, (name) => (taken || []).includes(name)),
					),
				};
			case "createMcpResultSchema":
				return { schema: mt.createMcpResultSchema(args.structuredContentSchema) };
			case "toToolExposure":
				return { results: args.exposures.map((exposure) => mt.toToolExposure(exposure)) };
			case "toModelContent": {
				const content = await mt.toModelContent(args.server, args.blocks, {
					saveOutput: fakeSaver,
					readableResources: args.readableResources,
				});
				return { content };
			}
			case "convertMcpResult": {
				const result = await mt.convertMcpResult(args.server, args.tool, args.result, {
					saveOutput: fakeSaver,
					readableResources: args.readableResources,
				});
				return result;
			}
			case "limitMcpContent": {
				const { content, fullOutputPath } = await mt.limitMcpContent(args.content, fakeSaver);
				return { content, ...(fullOutputPath ? { fullOutputPath } : {}) };
			}
			default:
				throw new Error("unknown mcp-tools fn: " + args.fn);
		}
	}
	if (tool === "tool-search") {
		const tsm = require(path.join(distDir, "extensions", "tool-search", "tool.js"));
		switch (args.fn) {
			case "tokenize":
				return { tokens: args.texts.map((text) => tsm.tokenize(text)) };
			case "createToolSearchDocument": {
				const document = tsm.createToolSearchDocument(args.tool, args.namespace);
				return { name: document.name, text: document.text };
			}
			case "rank": {
				const ranker = new tsm.Bm25Ranker({ k1: args.k1, b: args.b });
				return { matches: ranker.rank(args.query, args.documents, args.limit) };
			}
			default:
				throw new Error("unknown tool-search fn: " + args.fn);
		}
	}
	if (tool === "config-value") {
		const rcv = require(path.join(distDir, "core", "resolve-config-value.js"));
		// Defeat the process-lifetime command cache so each case executes fresh.
		rcv.clearConfigValueCache();
		switch (args.fn) {
			case "getConfigValueEnvVarName":
				return { result: rcv.getConfigValueEnvVarName(args.config) ?? null };
			case "getConfigValueEnvVarNames":
				return { result: rcv.getConfigValueEnvVarNames(args.config) };
			case "getMissingConfigValueEnvVarNames":
				return { result: rcv.getMissingConfigValueEnvVarNames(args.config, args.env) };
			case "isCommandConfigValue":
				return { result: rcv.isCommandConfigValue(args.config) };
			case "isConfigValueConfigured":
				return { result: rcv.isConfigValueConfigured(args.config, args.env) };
			case "resolveConfigValue":
				// Commands execute for real (execSync) — intended on both sides.
				return { result: rcv.resolveConfigValue(args.config, args.env) ?? null };
			case "resolveConfigValueOrThrow":
				return { result: rcv.resolveConfigValueOrThrow(args.config, args.description, args.env) };
			case "resolveHeaders":
				return { result: rcv.resolveHeaders(args.headers, args.env) ?? null };
			case "resolveHeadersOrThrow":
				return { result: rcv.resolveHeadersOrThrow(args.headers, args.description, args.env) ?? null };
			default:
				throw new Error("unknown config-value fn: " + args.fn);
		}
	}
	if (tool === "http-dispatcher") {
		const hd = require(path.join(distDir, "core", "http-dispatcher.js"));
		switch (args.fn) {
			case "parseHttpIdleTimeoutMs":
				return { result: hd.parseHttpIdleTimeoutMs(args.value) ?? null };
			case "formatHttpIdleTimeoutMs":
				return { result: hd.formatHttpIdleTimeoutMs(args.timeoutMs) };
			default:
				throw new Error("unknown http-dispatcher fn: " + args.fn);
		}
	}
	if (tool === "auth-guidance") {
		const ag = require(path.join(distDir, "core", "auth-guidance.js"));
		const configMod = require(path.join(distDir, "config.js"));
		// The guidance strings embed getDocsPath() (package-absolute,
		// machine-specific). Normalize it to "<docs>"; the Rust harness does
		// the same substitution with its own docsPath.
		const realDocs = configMod.getDocsPath();
		const normalize = (s) => String(s).split(realDocs).join("<docs>");
		switch (args.fn) {
			case "getProviderLoginHelp":
				return { result: normalize(ag.getProviderLoginHelp()) };
			case "formatNoModelsAvailableMessage":
				return { result: normalize(ag.formatNoModelsAvailableMessage()) };
			case "formatNoModelSelectedMessage":
				return { result: normalize(ag.formatNoModelSelectedMessage()) };
			case "formatNoApiKeyFoundMessage":
				return { result: normalize(ag.formatNoApiKeyFoundMessage(args.provider)) };
			default:
				throw new Error("unknown auth-guidance fn: " + args.fn);
		}
	}
	if (tool === "models-store") {
		const ms = require(path.join(distDir, "core", "models-store.js"));
		const runSteps = async (store, steps, reads) => {
			for (const step of steps || []) {
				switch (step.op) {
					case "write":
						await store.write(step.providerId, step.entry);
						break;
					case "read":
						reads.push((await store.read(step.providerId)) ?? null);
						break;
					case "delete":
						await store.delete(step.providerId);
						break;
					default:
						throw new Error("unknown models-store step op: " + step.op);
				}
			}
		};
		switch (args.fn) {
			case "inMemory": {
				const store = new ms.InMemoryCodingAgentModelsStore();
				const reads = [];
				await runSteps(store, args.steps, reads);
				return { result: { reads } };
			}
			case "file": {
				const os = require("os");
				const fs = require("fs");
				const dir = path.join(os.tmpdir(), "pi-parity-models-store", String(process.pid));
				const abs = path.join(dir, args.path);
				fs.mkdirSync(path.dirname(abs), { recursive: true });
				const reads = [];
				try {
					if (typeof args.initialContent === "string") {
						fs.writeFileSync(abs, args.initialContent);
					} else {
						// "else leave absent": guarantee absence even when a
						// prior case in the same process touched the path.
						fs.rmSync(abs, { force: true });
					}
					const store = new ms.FileModelsStore(abs);
					await runSteps(store, args.steps, reads);
					const content = fs.existsSync(abs) ? fs.readFileSync(abs, "utf-8") : null;
					return { result: { reads, content } };
				} finally {
					fs.rmSync(dir, { recursive: true, force: true });
				}
			}
			default:
				throw new Error("unknown models-store fn: " + args.fn);
		}
	}
	if (tool === "runtime-credentials") {
		const rcm = require(path.join(distDir, "core", "runtime-credentials.js"));
		// The REAL InMemoryCredentialStore from the pi-ai package (not a fake).
		const { pathToFileURL } = require("url");
		const CREDENTIAL_STORE_SRC =
			process.env.PI_AI_CREDENTIAL_STORE || "/tmp/pi-repo/packages/ai/dist/auth/credential-store.js";
		const aiAuth = await import(pathToFileURL(CREDENTIAL_STORE_SRC).href);
		const store = new aiAuth.InMemoryCredentialStore();
		for (const backing of args.backing || []) {
			const credential = backing.credential;
			await store.modify(backing.providerId, async () => credential);
		}
		const rc = new rcm.RuntimeCredentials(store);
		const has = [];
		const reads = [];
		const lists = [];
		const modifies = [];
		for (const step of args.steps || []) {
			switch (step.op) {
				case "set":
					rc.setRuntimeApiKey(step.providerId, step.key);
					break;
				case "remove":
					rc.removeRuntimeApiKey(step.providerId);
					break;
				case "has":
					has.push(rc.hasRuntimeApiKey(step.providerId));
					break;
				case "read":
					reads.push((await rc.read(step.providerId)) ?? null);
					break;
				case "list":
					lists.push(await rc.list());
					break;
				case "delete":
					await rc.delete(step.providerId);
					break;
				case "modify":
					modifies.push(
						(await rc.modify(step.providerId, async () => (step.set === undefined ? undefined : step.set))) ??
							null,
					);
					break;
				default:
					throw new Error("unknown runtime-credentials step op: " + step.op);
			}
		}
		return { result: { has, reads, lists, modifies } };
	}
	if (tool === "virtual-models") {
		const vm = require(path.join(distDir, "core", "virtual-models.js"));
		switch (args.fn) {
			case "isVirtualModel":
				return { result: vm.isVirtualModel(args.model) };
			case "findLatestResponse":
				return { result: vm.findLatestResponse(args.messages) ?? null };
			case "getBranchSelection": {
				const models = args.models || [];
				const result = vm.getBranchSelection(args.branch, (provider, modelId) =>
					models.find((model) => model.provider === provider && model.id === modelId),
				);
				return { result: result ?? null };
			}
			case "getVirtualModelState":
				return { result: vm.getVirtualModelState(args.branch, args.provider, args.modelId) ?? null };
			case "createVirtualModel":
				return { result: { model: vm.createVirtualModel(args.definition) } };
			default:
				throw new Error("unknown virtual-models fn: " + args.fn);
		}
	}
	if (tool === "provider-attribution") {
		const pa = require(path.join(distDir, "core", "provider-attribution.js"));
		switch (args.fn) {
			case "mergeProviderAttributionHeaders": {
				// isInstallTelemetryEnabled consults process.env.PI_TELEMETRY
				// first; the harness owns env concerns, so clear it for the
				// call and let the stub settings manager decide.
				delete process.env.PI_TELEMETRY;
				const settingsManager = { getEnableInstallTelemetry: () => args.telemetryEnabled };
				const result = pa.mergeProviderAttributionHeaders(
					args.model,
					settingsManager,
					args.sessionId ?? undefined,
					...(args.headerSources || []).map((headers) => (headers === null ? undefined : headers)),
				);
				return { result: result ?? null };
			}
			default:
				throw new Error("unknown provider-attribution fn: " + args.fn);
		}
	}
	if (tool === "model-config") {
		const mc = require(path.join(distDir, "core", "model-config.js"));
		const { stripJsonComments } = require(path.join(distDir, "utils", "json.js"));
		const { stripBom } = require(path.join(distDir, "utils", "text.js"));
		// The compiled validator is module-private in model-config.js, so
		// rebuild it from the exported ModelsConfigSchema with the REAL
		// typebox Compile, resolved from the coding-agent package's
		// node_modules (same version the dist module itself loads).
		const { createRequire } = require("module");
		const pkgRequire = createRequire(path.join(distDir, "core", "model-config.js"));
		const { Compile } = pkgRequire("typebox/compile");
		const validate = Compile(mc.ModelsConfigSchema);
		// formatValidationPath is module-private; replicate it verbatim.
		const formatValidationPath = (error) => {
			if (error.keyword === "required") {
				const requiredProperties = error.params?.requiredProperties;
				const requiredProperty = requiredProperties?.[0];
				if (requiredProperty) {
					const basePath = error.instancePath.replace(/^\//, "").replace(/\//g, ".");
					return basePath ? `${basePath}.${requiredProperty}` : requiredProperty;
				}
			}
			const p = error.instancePath.replace(/^\//, "").replace(/\//g, ".");
			return p || "root";
		};
		switch (args.fn) {
			case "load": {
				// Mirror ModelConfig.load(modelsJsonPath) with injected inputs
				// instead of the fs (deterministic parity):
				// path null → modelsJsonPath undefined → empty config.
				if (args.path === null || args.path === undefined) {
					return { result: { providers: {}, providerIds: [], error: null } };
				}
				if (typeof args.content === "string") {
					let parsed;
					try {
						parsed = JSON.parse(stripJsonComments(stripBom(args.content)));
					} catch (error) {
						return {
							result: {
								providers: {},
								providerIds: [],
								error: `Failed to parse models.json: ${error instanceof Error ? error.message : error}\n\nFile: ${args.path}`,
							},
						};
					}
					if (!validate.Check(parsed)) {
						const errors =
							[...validate.Errors(parsed)]
								.map((error) => `  - ${formatValidationPath(error)}: ${error.message}`)
								.join("\n") || "Unknown schema error";
						return {
							result: {
								providers: {},
								providerIds: [],
								error: `Invalid models.json schema:\n${errors}\n\nFile: ${args.path}`,
							},
						};
					}
					// deepFreeze(structuredClone(x)) is JSON-identical to the
					// raw parsed provider; keep parsed key order.
					return { result: { providers: parsed.providers, providerIds: Object.keys(parsed.providers), error: null } };
				}
				if (typeof args.ioError === "string") {
					return {
						result: {
							providers: {},
							providerIds: [],
							error: `Failed to load models.json: ${args.ioError}\n\nFile: ${args.path}`,
						},
					};
				}
				// content null + ioError null → readFile ENOENT → empty config.
				return { result: { providers: {}, providerIds: [], error: null } };
			}
			default:
				throw new Error("unknown model-config fn: " + args.fn);
		}
	}
	if (tool === "model-resolver") {
		const mr = require(path.join(distDir, "core", "model-resolver.js"));
		// Reduced ModelRuntime over the injected args (deterministic, no env).
		const modelRuntime = {
			getModels: () => args.models,
			getModel: (provider, modelId) =>
				args.models.find((model) => model.provider === provider && model.id === modelId),
			getAvailableSnapshot: () => args.availableModels,
			hasConfiguredAuth: (provider) => (args.configuredAuth || []).includes(provider),
		};
		switch (args.fn) {
			case "resolveModelReference":
				// Verbatim: undefined fields drop out of JSON.stringify.
				return {
					result: mr.resolveModelReference(
						{ text: args.text, provider: args.provider, thinkingSuffix: args.thinkingSuffix, fuzzy: args.fuzzy },
						args.models,
						(provider) => (args.usable || []).includes(provider),
					),
				};
			case "findExactModelReferenceMatch":
				return { result: mr.findExactModelReferenceMatch(args.modelReference, args.models) ?? null };
			case "resolveModelScopeFromModels":
				return { result: mr.resolveModelScopeFromModels(args.patterns, args.models) };
			case "resolveCliModel":
				return {
					result: mr.resolveCliModel({
						cliProvider: args.cliProvider,
						cliModel: args.cliModel,
						cliThinking: args.cliThinking,
						inlineApiKey: args.inlineApiKey,
						modelRuntime,
					}),
				};
			case "findInitialModel": {
				const r = await mr.findInitialModel({
					scopedModels: args.scopedModels || [],
					isContinuing: args.isContinuing,
					defaultProvider: args.defaultProvider,
					defaultModelId: args.defaultModelId,
					defaultThinkingLevel: args.defaultThinkingLevel,
					modelThinkingLevels: args.modelThinkingLevels,
					modelRuntime,
				});
				return { result: { model: r.model ?? null, thinkingLevel: r.thinkingLevel, fallbackMessage: r.fallbackMessage ?? null } };
			}
			case "restoreModelFromSession": {
				// shouldPrintMessages: false — no console output.
				const r = await mr.restoreModelFromSession(
					args.savedProvider,
					args.savedModelId,
					args.currentModel ?? undefined,
					false,
					modelRuntime,
				);
				return { result: { model: r.model ?? null, fallbackMessage: r.fallbackMessage ?? null } };
			}
			default:
				throw new Error("unknown model-resolver fn: " + args.fn);
		}
	}
	if (tool === "auth-storage") {
		const as = require(path.join(distDir, "core", "auth-storage.js"));
		const fs = require("fs");
		const os = require("os");
		const rcv = require(path.join(distDir, "core", "resolve-config-value.js"));
		rcv.clearConfigValueCache();
		// Deterministic ambient env (the Rust harness injects the same map).
		for (const key of Object.keys(process.env)) delete process.env[key];
		Object.assign(process.env, args.env ?? {});
		let counter = 0;
		const tempFile = (content) => {
			const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-auth-storage-"));
			const file = path.join(dir, "case-" + counter++ + ".json");
			if (typeof content === "string") fs.writeFileSync(file, content);
			return file;
		};
		switch (args.fn) {
			case "readOnlyLoad": {
				const storage = new as.ReadOnlyAuthStorage(tempFile(args.content));
				return { result: storage.load() };
			}
			case "readOnlyRead": {
				const storage = new as.ReadOnlyAuthStorage(tempFile(args.content));
				return { result: (await storage.read(args.providerId)) ?? null };
			}
			case "readOnlyList": {
				const storage = new as.ReadOnlyAuthStorage(tempFile(args.content));
				return { result: await storage.list() };
			}
			case "readOnlyModify": {
				const storage = new as.ReadOnlyAuthStorage(tempFile(args.content));
				return { result: await storage.modify(args.providerId ?? "p", async (current) => current) };
			}
			case "readOnlyDelete": {
				const storage = new as.ReadOnlyAuthStorage(tempFile(args.content));
				return { result: await storage.delete(args.providerId ?? "p") };
			}
			case "inMemoryBackend": {
				const backend = new as.InMemoryAuthStorageBackend();
				if (typeof args.initial === "string") {
					backend.withLock(() => ({ result: undefined, next: args.initial }));
				}
				const results = [];
				for (const lock of args.locks ?? []) {
					results.push(
						backend.withLock(() => ({
							result: lock.result ?? null,
							next: typeof lock.next === "string" ? lock.next : undefined,
						})),
					);
				}
				return { result: { results, value: backend.value ?? null } };
			}
			case "authStorageSequence": {
				const storage = as.AuthStorage.inMemory(args.data ?? {});
				const results = [];
				for (const step of args.steps ?? []) {
					switch (step.op) {
						case "read":
							results.push((await storage.read(step.providerId)) ?? null);
							break;
						case "modify":
							results.push(
								(await storage.modify(step.providerId, async () => step.set ?? undefined)) ?? null,
							);
							break;
						case "delete":
							await storage.delete(step.providerId);
							results.push(null);
							break;
						case "list":
							results.push(await storage.list());
							break;
						case "reload":
							storage.reload();
							results.push(null);
							break;
						default:
							throw new Error("unknown auth-storage step op: " + step.op);
					}
				}
				return {
					result: {
						results,
						content: storage.storage.value ?? null,
					},
				};
			}
			case "readStoredCredential": {
				return { result: (await as.readStoredCredential(args.providerId, tempFile(args.content))) ?? null };
			}
			default:
				throw new Error("unknown auth-storage fn: " + args.fn);
		}
	}
	if (tool === "remote-catalog") {
		const rcp = require(path.join(distDir, "core", "remote-catalog-provider.js"));
		const rcv = require(path.join(distDir, "core", "resolve-config-value.js"));
		rcv.clearConfigValueCache();
		// Scripted clock and fetch: the response is injected via args.
		Date.now = () => args.now ?? 0;
		const spec = args.response ?? null;
		const makeResponse = () => {
			const status = spec?.status ?? 200;
			return {
				status,
				ok: status >= 200 && status < 300,
				headers: {
					get: (name) => {
						if (name === "last-modified") return spec?.lastModified ?? null;
						if (name === "etag") return spec?.etag ?? null;
						return null;
					},
				},
				json: async () => spec?.body ?? null,
				text: async () => "",
				body: undefined,
			};
		};
		globalThis.fetch = async () => makeResponse();
		const chatOnly = (models) => models.filter((model) => !model.type || model.type === "chat");
		const baseProvider = {
			id: args.providerId,
			name: "base",
			auth: {},
			getModels: () => chatOnly(args.models ?? []),
			getAllModels: () => args.allModels ?? args.models ?? [],
			stream: () => {},
			streamSimple: () => {},
		};
		const publishes = [];
		const refreshContext = () => ({
			stored: args.stored,
			publish: async (publication) => {
				publishes.push({ persist: publication.persist ?? null, update: !!publication.update });
				publication.update?.();
				return true;
			},
			allowNetwork: args.allowNetwork ?? true,
			force: args.force ?? false,
			signal: new AbortController().signal,
		});
		switch (args.fn) {
			case "withRemoteCatalog": {
				const provider = rcp.withRemoteCatalog(baseProvider, undefined, args.localGeneratedAt);
				await provider.refreshModels(refreshContext());
				return {
					result: {
						getModels: provider.getModels(),
						getAllModels: provider.getAllModels(),
						publishes,
					},
				};
			}
			case "withPersonalizedCatalog": {
				const op = args.personalize?.op ?? "identity";
				const ids = args.personalize?.ids ?? [];
				const personalize = async (catalog) => {
					if (op === "empty") return [];
					if (op === "reverse") return [...catalog].reverse();
					if (op === "ids") return catalog.filter((model) => ids.includes(model.id));
					return catalog;
				};
				const provider = rcp.withPersonalizedCatalog(baseProvider, personalize, undefined, args.localGeneratedAt);
				await provider.refreshModels(refreshContext());
				return {
					result: {
						getModels: provider.getModels(),
						getAllModels: provider.getAllModels(),
						publishes,
					},
				};
			}
			default:
				throw new Error("unknown remote-catalog fn: " + args.fn);
		}
	}
	if (tool === "provider-composer") {
		const pc = require(path.join(distDir, "core", "provider-composer.js"));
		const rcv = require(path.join(distDir, "core", "resolve-config-value.js"));
		rcv.clearConfigValueCache();
		for (const key of Object.keys(process.env)) delete process.env[key];
		Object.assign(process.env, args.env ?? {});
		const chatOnly = (models) => models.filter((model) => !model.type || model.type === "chat");
		const buildBase = () => {
			const spec = args.base;
			if (!spec) return undefined;
			const models = spec.models ?? [];
			const provider = {
				id: spec.id ?? "base",
				name: spec.name ?? "base",
				baseUrl: spec.baseUrl,
				headers: spec.headers,
				auth: {},
				getModels: () => chatOnly(models),
				getAllModels: () => spec.allModels ?? models,
				stream: () => {},
				streamSimple: () => {},
			};
			if (spec.auth?.apiKey) {
				const key = spec.auth.apiKey;
				provider.auth.apiKey = {
					name: key.name ?? "API key",
					...("check" in key
						? {
								check: async () =>
									key.check && typeof key.check === "object"
										? { type: "api_key", source: key.check.source }
										: undefined,
							}
						: {}),
					resolve: async () =>
						key.resolve && typeof key.resolve === "object" ? { auth: key.resolve.auth } : undefined,
				};
			}
			if (spec.auth?.oauth) {
				const oauth = spec.auth.oauth;
				provider.auth.oauth = {
					name: oauth.name ?? "OAuth",
					isSubscription: oauth.isSubscription,
					toAuth: async () => {
						if (oauth.toAuth?.error) throw new Error(oauth.toAuth.error);
						return oauth.toAuth ?? {};
					},
				};
			}
			if (spec.refreshModels) {
				provider.refreshModels = async () => {
					if (spec.refreshModels.error) throw new Error(spec.refreshModels.error);
				};
			}
			return provider;
		};
		const buildModelConfig = () => ({ getProvider: (id) => args.modelConfig?.providers?.[id] });
		const providerEntry = () => args.modelConfig?.providers?.[args.providerId];
		const applyModifyModels = (op, models) => {
			if (op === "reverse") return [...models].reverse();
			if (op === "dropFirst") return models.slice(1);
			if (op === "prefixIds") return models.map((model) => ({ ...model, id: "x-" + model.id }));
			return models;
		};
		const buildExtension = () => {
			const spec = args.extension;
			if (!spec) return undefined;
			const extension = {
				name: spec.name,
				baseUrl: spec.baseUrl,
				apiKey: spec.apiKey,
				api: spec.api,
				headers: spec.headers,
				authHeader: spec.authHeader,
				models: spec.models,
			};
			if (spec.oauth) {
				const oauth = spec.oauth;
				extension.oauth = {
					name: oauth.name,
					isSubscription: oauth.isSubscription,
					login: async () => {
						throw new Error("login not exercised");
					},
					refreshToken: async () => {
						throw new Error("refreshToken not exercised");
					},
					getApiKey: (credential) =>
						typeof oauth.getApiKey === "string"
							? oauth.getApiKey
							: (credential?.[oauth.getApiKey?.field ?? "access"] ?? ""),
					...(oauth.modifyModels
						? { modifyModels: (models) => applyModifyModels(oauth.modifyModels.op, models) }
						: {}),
				};
			}
			if (spec.refreshModels) {
				extension.refreshModels = async () => {
					if (spec.refreshModels.error) throw new Error(spec.refreshModels.error);
					return spec.refreshModels.models ?? [];
				};
			}
			if (spec.images) {
				extension.images = {};
				for (const [api, impl] of Object.entries(spec.images)) {
					extension.images[api] = { generateImages: async () => ({ marker: impl.marker }) };
				}
			}
			if (spec.classifiers) {
				extension.classifiers = {};
				for (const [api, impl] of Object.entries(spec.classifiers)) {
					extension.classifiers[api] = { classify: async () => ({ marker: impl.marker }) };
				}
			}
			return extension;
		};
		const ctx = { env: async (name) => args.env?.[name], fileExists: async () => false };
		const composeProvider = () => pc.composeModelProvider(args.providerId, buildBase(), buildModelConfig(), buildExtension());
		switch (args.fn) {
			case "validateExtensionProvider":
				pc.validateExtensionProvider(args.providerId, buildBase(), providerEntry(), buildExtension());
				return { result: null };
			case "composeModelProvider": {
				const provider = composeProvider();
				return {
					result: {
						name: provider.name,
						baseUrl: provider.baseUrl ?? null,
						headers: provider.headers ?? null,
						getModels: provider.getModels(),
						getAllModels: provider.getAllModels(),
						apiKey: provider.auth.apiKey?.name ?? null,
						oauth: provider.auth.oauth?.name ?? null,
						hasRefreshModels: !!provider.refreshModels,
						hasGenerateImages: !!provider.generateImages,
						hasClassify: !!provider.classify,
					},
				};
			}
			case "refreshModels": {
				const provider = composeProvider();
				if (!provider.refreshModels) throw new Error("composed provider has no refreshModels");
				const published = [];
				const controller = new AbortController();
				if (args.aborted) controller.abort();
				await provider.refreshModels({
					credential: args.credential,
					publish: async (publication) => {
						published.push({ update: !!publication.update });
						publication.update?.();
						return args.publishAccept ?? true;
					},
					allowNetwork: true,
					force: false,
					signal: controller.signal,
				});
				return {
					result: {
						getModels: provider.getModels(),
						getAllModels: provider.getAllModels(),
						published: published[0] ?? null,
					},
				};
			}
			case "resolveConfiguredModelHeaders":
				return {
					result:
						pc.resolveConfiguredModelHeaders(args.model, providerEntry(), buildExtension(), args.env) ?? null,
				};
			case "resolveCompatibilityRequestConfig": {
				const config = pc.resolveCompatibilityRequestConfig(args.model, providerEntry(), buildExtension());
				return { result: { headers: config.headers ?? null, authHeader: config.authHeader } };
			}
			case "configuredRequestAuthStatus":
				return { result: pc.configuredRequestAuthStatus(providerEntry(), buildExtension()) ?? null };
			case "apiKeyCheck": {
				const provider = composeProvider();
				const auth = provider.auth.apiKey;
				if (!auth) throw new Error("no composed API key auth");
				const credential = args.credential?.type === "api_key" ? args.credential : undefined;
				const check = await auth.check({ ctx, credential, signal: new AbortController().signal });
				return { result: check ?? null };
			}
			case "apiKeyResolve": {
				const provider = composeProvider();
				const auth = provider.auth.apiKey;
				if (!auth) throw new Error("no composed API key auth");
				const credential = args.credential?.type === "api_key" ? args.credential : undefined;
				const resolved = await auth.resolve({ ctx, credential, signal: new AbortController().signal });
				return {
					result: resolved
						? { auth: resolved.auth, env: resolved.env ?? null, source: resolved.source ?? null }
						: null,
				};
			}
			case "oauthToAuth": {
				const provider = composeProvider();
				const oauth = provider.auth.oauth;
				if (!oauth) throw new Error("no composed OAuth auth");
				if (args.credential?.type !== "oauth") throw new Error("oauthToAuth requires an OAuth credential");
				return { result: await oauth.toAuth(args.credential) };
			}
			case "generateImages": {
				const provider = composeProvider();
				const result = provider.generateImages
					? await provider.generateImages(args.model, {}, undefined)
					: null;
				if (result) result.timestamp = 0;
				return { result };
			}
			case "classify": {
				const provider = composeProvider();
				const result = provider.classify ? await provider.classify(args.model, {}, undefined) : null;
				if (result) result.timestamp = 0;
				return { result };
			}
			default:
				throw new Error("unknown provider-composer fn: " + args.fn);
		}
	}
	if (tool === "model-runtime" || tool === "model-registry") {
		const mr = require(path.join(distDir, "core", "model-runtime.js"));
		const reg = require(path.join(distDir, "core", "model-registry.js"));
		// Env scrub: both sides own the environment completely.
		for (const key of Object.keys(process.env)) delete process.env[key];
		Object.assign(process.env, args.env ?? {});
		delete process.env.PI_OFFLINE;
		if (args.offline) process.env.PI_OFFLINE = "1";
		// Scripted clock.
		Date.now = () => args.nowMs ?? 0;
		// LAZY scripted catalog fetch: only errors when the runtime actually
		// performs a network request without a spec (Rust catalog-hook mirror).
		const fetchSpec = args.fetch ?? null;
		globalThis.fetch = async () => {
			if (!fetchSpec) throw new Error("no scripted fetch response");
			const status = fetchSpec.status ?? 200;
			return {
				status,
				ok: status >= 200 && status < 300,
				headers: {
					get: (name) =>
						name === "last-modified" ? (fetchSpec.lastModified ?? null) : name === "etag" ? (fetchSpec.etag ?? null) : null,
				},
				json: async () => fetchSpec.body ?? null,
				text: async () => "",
			};
		};
		const { mkdtempSync, writeFileSync } = require("fs");
		const os = require("os");
		const tmp = mkdtempSync(path.join(os.tmpdir(), "pi-mr-"));
		const authPath = path.join(tmp, "auth.json");
		writeFileSync(authPath, args.authJson ?? "{}");
		let modelsPath;
		if (typeof args.modelsJson === "string") {
			modelsPath = path.join(tmp, "models.json");
			writeFileSync(modelsPath, args.modelsJson);
		}
		// Recording models store (seeded from args.stored).
		const storeOps = [];
		const storedEntries = new Map(Object.entries(args.stored ?? {}));
		const clone = (v) => JSON.parse(JSON.stringify(v));
		const modelsStore = {
			read: async (providerId) => {
				storeOps.push({ op: "read", providerId });
				const entry = storedEntries.get(providerId);
				return entry === undefined ? undefined : clone(entry);
			},
			write: async (providerId, entry) => {
				const value = clone(entry);
				storeOps.push({ op: "write", providerId, entry: value });
				storedEntries.set(providerId, value);
			},
			delete: async (providerId) => {
				storeOps.push({ op: "delete", providerId });
				storedEntries.delete(providerId);
			},
		};
		const routes = [];
		const runtime = await mr.ModelRuntime.create({
			authPath,
			modelsPath: typeof args.modelsJson === "string" ? modelsPath : null,
			modelsStore,
			allowModelNetwork: args.allowModelNetwork ?? false,
			refreshOnCreate: args.refreshOnCreate ?? false,
		});
		const target = tool === "model-registry" ? new reg.ModelRegistry(runtime) : runtime;

		const stepError = (e) => {
			const causes = [];
			let cause = e && e.cause;
			while (cause) {
				causes.push(cause instanceof Error ? cause.message : String(cause));
				cause = cause && cause.cause;
			}
			return { error: e instanceof Error ? e.message : String(e), causes };
		};
		const providerSummary = (p) =>
			p
				? { id: p.id, name: p.name, baseUrl: p.baseUrl ?? null, headers: p.headers ? { ...p.headers } : null }
				: null;
		const checkValue = (check) =>
			check ? (check.source !== undefined ? { source: check.source, type: check.type } : { type: check.type }) : null;
		const authResultValue = (resolution) =>
			resolution
				? {
						auth: resolution.auth,
						env: resolution.env ? { ...resolution.env } : null,
						source: resolution.source ?? null,
					}
				: null;
		const chatModelRef = (step) => {
			const model = runtime.getModel(step.model?.provider, step.model?.modelId);
			if (!model) throw new Error(`model not found: ${step.model?.provider}/${step.model?.modelId}`);
			return model;
		};
		const typedModelRef = (step, type) => {
			const model = runtime.getModelOfType(type, step.model?.provider, step.model?.modelId);
			if (!model) throw new Error(`model not found: ${step.model?.provider}/${step.model?.modelId}`);
			return model;
		};
		const modelTypeArg = (step) => {
			if (step.type === "chat" || step.type === "image" || step.type === "classifier") return step.type;
			throw new Error(`unknown model type: ${JSON.stringify(step.type)}`);
		};
		const normalizeTimestamp = (value) =>
			value && typeof value === "object" && "timestamp" in value ? { ...value, timestamp: 0 } : value;
		const buildNativeProvider = (spec) => {
			const chatOnly = (models) => (models ?? []).filter((model) => !model.type || model.type === "chat");
			const provider = {
				id: spec.id,
				name: spec.name ?? spec.id,
				baseUrl: spec.baseUrl,
				headers: spec.headers,
				auth: {},
				getModels: () => chatOnly(spec.models),
				getAllModels: () => spec.models ?? [],
				stream: () => {},
				streamSimple: () => {},
			};
			for (const impl of Object.values(spec.images ?? {})) {
				provider.generateImages = async () => ({ marker: impl.marker ?? "images" });
				break;
			}
			for (const impl of Object.values(spec.classifiers ?? {})) {
				provider.classify = async () => ({ marker: impl.marker ?? "classify" });
				break;
			}
			return provider;
		};
		const buildExtensionConfig = (spec) => {
			const config = {};
			if (spec.name !== undefined) config.name = spec.name;
			if (spec.baseUrl !== undefined) config.baseUrl = spec.baseUrl;
			if (spec.apiKey !== undefined) config.apiKey = spec.apiKey;
			if (spec.api !== undefined) config.api = spec.api;
			if (spec.headers !== undefined) config.headers = spec.headers;
			if (spec.authHeader !== undefined) config.authHeader = spec.authHeader;
			if (spec.models !== undefined) config.models = spec.models;
			if (spec.oauth) {
				const oauth = spec.oauth;
				config.oauth = {
					name: oauth.name,
					isSubscription: oauth.isSubscription,
					login: async () => {
						throw new Error("login not exercised");
					},
					refreshToken: async () => {
						throw new Error("refreshToken not exercised");
					},
					getApiKey: (credential) =>
						typeof oauth.getApiKey === "string"
							? oauth.getApiKey
							: (credential?.[oauth.getApiKey?.field ?? "access"] ?? ""),
				};
			}
			if (spec.refreshModels) {
				const rms = spec.refreshModels;
				config.refreshModels = async () => {
					if (rms.error) throw new Error(rms.error);
					return rms.models ?? [];
				};
			}
			if (spec.images) {
				config.images = {};
				for (const [api, impl] of Object.entries(spec.images)) {
					config.images[api] = { generateImages: async () => ({ marker: impl.marker ?? "images" }) };
				}
			}
			if (spec.classifiers) {
				config.classifiers = {};
				for (const [api, impl] of Object.entries(spec.classifiers)) {
					config.classifiers[api] = { classify: async () => ({ marker: impl.marker ?? "classify" }) };
				}
			}
			return config;
		};
		const configSummary = (config) => {
			if (!config) return null;
			const out = {};
			if (config.name !== undefined) out.name = config.name;
			if (config.baseUrl !== undefined) out.baseUrl = config.baseUrl;
			if (config.apiKey !== undefined) out.apiKey = config.apiKey;
			if (config.api !== undefined) out.api = config.api;
			if (config.headers !== undefined) out.headers = { ...config.headers };
			if (config.authHeader !== undefined) out.authHeader = config.authHeader;
			if (config.models !== undefined) out.models = config.models;
			out.oauth = config.oauth
				? { name: config.oauth.name, isSubscription: config.oauth.isSubscription ?? null }
				: null;
			out.hasRefreshModels = config.refreshModels !== undefined;
			out.images = config.images ? Object.keys(config.images) : null;
			out.classifiers = config.classifiers ? Object.keys(config.classifiers) : null;
			return out;
		};
		const standIn = (provider, id) => ({
			id,
			name: id,
			api: "a",
			provider,
			baseUrl: "",
			input: ["text"],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			reasoning: false,
			contextWindow: 0,
			maxTokens: 0,
		});
		const buildVirtualModel = (spec) => {
			const routeSpec = spec.route ?? {};
			return {
				provider: spec.provider,
				id: spec.id,
				name: spec.name ?? spec.id,
				thinkingLevels: spec.thinkingLevels,
				contextWindow: spec.contextWindow,
				maxTokens: spec.maxTokens,
				route: (request) => {
					if (routeSpec.echo) {
						routes.push({
							model: { provider: request.model.provider, id: request.model.id },
							thinkingLevel: request.thinkingLevel,
							reason: request.reason,
							previous: request.previous
								? {
										model: { provider: request.previous.model.provider, id: request.previous.model.id },
										thinkingLevel: request.previous.thinkingLevel ?? null,
									}
								: null,
							failed: request.failed
								? {
										model: { provider: request.failed.model.provider, id: request.failed.model.id },
										thinkingLevel: request.failed.thinkingLevel ?? null,
										message: request.failed.message,
									}
								: null,
							state: request.state ?? null,
							messages: request.messages,
						});
					}
					return {
						model: standIn(routeSpec.provider, routeSpec.model),
						thinkingLevel: routeSpec.thinkingLevel ?? "off",
						state: routeSpec.state ?? undefined,
					};
				},
			};
		};
		const noopInteraction = {
			prompt: async () => {
				throw new Error("login prompt not exercised");
			},
			notify: () => {},
		};
		const settleRefresh = async () => {
			await runtime.refresh({ allowNetwork: false });
		};
		const refreshOptions = (step) => ({
			allowNetwork: step.allowNetwork,
			providers: step.providers,
			force: step.force ?? false,
		});
		const refreshResultValue = (result) => ({
			aborted: result.aborted,
			errors: Object.fromEntries([...result.errors].map(([id, error]) => [id, error.message])),
		});
		const resolvedAuthValue = (auth) => {
			if (!auth) return null;
			const out = { ok: auth.ok };
			if (auth.ok) {
				if (auth.apiKey !== undefined) out.apiKey = auth.apiKey;
				if (auth.headers !== undefined) out.headers = { ...auth.headers };
				if (auth.baseUrl !== undefined) out.baseUrl = auth.baseUrl;
				if (auth.env !== undefined) out.env = { ...auth.env };
			} else {
				out.error = auth.error;
			}
			return out;
		};
		const req = (step) => step.providerId ?? undefined;

		async function runStep(step) {
			const fn = step.fn;
			let value;
			switch (fn) {
				// ---- ModelRuntime surface ----
				case "getProviders":
					value = runtime.getProviders().map(providerSummary);
					break;
				case "getModels":
					value = runtime.getModels(req(step));
					break;
				case "getModel":
					value = runtime.getModel(step.providerId, step.modelId) ?? null;
					break;
				case "getAllModels":
					value = runtime.getAllModels(req(step));
					break;
				case "getModelsOfType":
					value = runtime.getModelsOfType(modelTypeArg(step), req(step));
					break;
				case "getModelOfType":
					value = runtime.getModelOfType(modelTypeArg(step), step.providerId, step.modelId) ?? null;
					break;
				case "getError":
					value = target.getError() ?? null;
					break;
				case "getAvailableSnapshot":
					value = runtime.getAvailableSnapshot();
					break;
				case "getAvailable":
					value = tool === "model-registry" ? target.getAvailable() : await runtime.getAvailable(req(step));
					break;
				case "checkAuth":
					value = checkValue(await runtime.checkAuth(step.providerId));
					break;
				case "hasConfiguredAuth":
					value =
						tool === "model-registry"
							? target.hasConfiguredAuth(chatModelRef(step))
							: runtime.hasConfiguredAuth(step.providerId);
					break;
				case "isUsingOAuth":
					value =
						tool === "model-registry"
							? target.isUsingOAuth(chatModelRef(step))
							: runtime.isUsingOAuth(step.providerId);
					break;
				case "isUsingSubscription":
					value = runtime.isUsingSubscription(step.providerId);
					break;
				case "getProviderAuthStatus":
					value = target.getProviderAuthStatus(step.providerId);
					break;
				case "getRegisteredProviderIds":
					value = target.getRegisteredProviderIds();
					break;
				case "getRegisteredProviderConfig":
					value = configSummary(target.getRegisteredProviderConfig(step.providerId));
					break;
				case "getRegisteredNativeProvider":
					value = providerSummary(target.getRegisteredNativeProvider(step.providerId));
					break;
				case "getAuth":
					value = authResultValue(await runtime.getAuth(step.providerId));
					break;
				case "getAuthModel":
					value = authResultValue(await runtime.getAuth(chatModelRef(step)));
					break;
				case "getCompatibilityRequestConfig": {
					const compatibility = runtime.getCompatibilityRequestConfig(chatModelRef(step));
					value = {
						headers: compatibility.headers ? { ...compatibility.headers } : null,
						authHeader: compatibility.authHeader,
					};
					break;
				}
				case "setRuntimeApiKey":
					await runtime.setRuntimeApiKey(step.providerId, step.apiKey);
					value = null;
					break;
				case "removeRuntimeApiKey":
					await runtime.removeRuntimeApiKey(step.providerId);
					value = null;
					break;
				case "listCredentials":
					value = (await runtime.listCredentials()).map((entry) => ({
						providerId: entry.providerId,
						type: entry.type,
					}));
					break;
				case "login":
					value = await runtime.login(
						step.providerId,
						step.type === "oauth" ? "oauth" : "api_key",
						noopInteraction,
					);
					break;
				case "logout":
					await runtime.logout(step.providerId);
					value = null;
					break;
				case "registerProvider": {
					if (tool === "model-registry") {
						target.registerProvider(step.providerId, buildExtensionConfig(step.config));
					} else {
						runtime.registerProvider(step.providerId, buildExtensionConfig(step.config));
					}
					if (step.settle) await settleRefresh();
					value = null;
					break;
				}
				case "registerNativeProvider": {
					const provider = buildNativeProvider(step.provider);
					if (tool === "model-registry") {
						target.registerProvider(provider);
					} else {
						runtime.registerNativeProvider(provider);
					}
					if (step.settle) await settleRefresh();
					value = null;
					break;
				}
				case "unregisterProvider":
					target.unregisterProvider(step.providerId);
					if (step.settle) await settleRefresh();
					value = null;
					break;
				case "registerVirtualModel":
					target.registerVirtualModel(buildVirtualModel(step.definition));
					if (step.settle) await settleRefresh();
					value = null;
					break;
				case "unregisterVirtualModel":
					target.unregisterVirtualModel(step.providerId, step.modelId);
					if (step.settle) await settleRefresh();
					value = null;
					break;
				case "resolveModel": {
					const model = chatModelRef(step);
					const route = await runtime.resolveModel(model, step.messages ?? [], {
						reason: step.reason ?? "direct",
						thinkingLevel: step.thinkingLevel ?? "off",
						failed: step.failed ?? undefined,
						state: step.state ?? undefined,
					});
					value = { model: route.model, thinkingLevel: route.thinkingLevel, state: route.state ?? null };
					break;
				}
				case "refresh":
					value = refreshResultValue(await target.refresh(refreshOptions(step)));
					break;
				case "generateImages":
					value = normalizeTimestamp(await target.generateImages(typedModelRef(step, "image"), step.context ?? {}));
					break;
				case "classify":
					value = normalizeTimestamp(await target.classify(typedModelRef(step, "classifier"), step.context ?? {}));
					break;
				// ---- ModelRegistry facade-only reads ----
				case "getAll":
					value = target.getAll();
					break;
				case "find":
					value = target.find(step.providerId, step.modelId) ?? null;
					break;
				case "findOfType":
					value = target.findOfType(modelTypeArg(step), step.providerId, step.modelId) ?? null;
					break;
				case "getApiKeyAndHeaders":
					value = resolvedAuthValue(await target.getApiKeyAndHeaders(chatModelRef(step)));
					break;
				case "getProviderDisplayName":
					value = target.getProviderDisplayName(step.providerId);
					break;
				case "getProviderAuth":
					value = authResultValue(await target.getProviderAuth(step.providerId));
					break;
				case "getApiKeyForProvider":
					value = (await target.getApiKeyForProvider(step.providerId)) ?? null;
					break;
				case "getAvailableOfType":
					value = await target.getAvailableOfType(modelTypeArg(step), req(step));
					break;
				case "getProvider":
					value = providerSummary(target.getProvider(step.providerId));
					break;
				default:
					throw new Error(`unknown ${tool} fn: ${fn}`);
			}
			return { result: value === undefined ? null : value };
		}

		const steps = Array.isArray(args.steps) ? args.steps : [args];
		const results = [];
		let settleSeen = false;
		for (const step of steps) {
			if (step.settle) settleSeen = true;
			try {
				results.push(await runStep(step));
			} catch (e) {
				results.push(stepError(e));
			}
		}
		const out = { results };
		if (!settleSeen) out.storeOps = storeOps;
		out.routes = routes;
		return { result: out };
	}

	// ---- landed core-module wave (event-bus, timings, slash-commands, -------
	// settings-defaults, settings-schema); mirrors pi-cli core_harness.rs.

	if (tool === "event-bus") {
		// Script-driven createEventBus() with numbered handlers. The safe
		// handler reports caught errors via console.error(template, err);
		// handlers throw STRINGS so the captured report renders identically
		// to the Rust error sink ("Event handler error (ch): msg").
		const { createEventBus } = require(path.join(distDir, "core", "event-bus.js"));
		const ops = args.ops;
		if (!Array.isArray(ops)) throw new Error("missing ops");
		const log = [];
		const errors = [];
		const realError = console.error;
		console.error = (...msgs) => errors.push(msgs.join(" "));
		const bus = createEventBus();
		// id -> { channel, unsub: fn | null }; entries persist after off so
		// ids are never reused (mirrors the Rust live map).
		const live = new Map();
		const offId = (id) => {
			const entry = live.get(id);
			if (entry && entry.unsub) {
				const unsub = entry.unsub;
				entry.unsub = null;
				unsub();
			}
		};
		const isId = (v) => typeof v === "number" && Number.isInteger(v) && v >= 0;
		try {
			for (const op of ops) {
				const kind = op.op;
				if (typeof kind !== "string") throw new Error("missing op");
				if (kind === "on" || kind === "once") {
					if (typeof op.channel !== "string") throw new Error("missing channel");
					if (!isId(op.handler)) throw new Error("missing handler");
					if (op.throws !== undefined && typeof op.throws !== "string") throw new Error("throws must be a string");
					if (op.clear !== undefined && typeof op.clear !== "boolean") throw new Error("clear must be a boolean");
					if (op.unsub !== undefined && !Array.isArray(op.unsub)) throw new Error("unsub must be an array");
					const unsubIds = op.unsub || [];
					for (const u of unsubIds) if (!isId(u)) throw new Error("unsub entries must be ids");
					const id = op.handler;
					if (live.has(id)) throw new Error(`handler ${id} already registered`);
					const once = kind === "once";
					const unsub = bus.on(op.channel, (data) => {
						// Node `once` removes the listener before invoking it.
						if (once) offId(id);
						log.push(op.channel + ":" + JSON.stringify(data));
						for (const target of unsubIds) offId(target);
						if (op.clear) bus.clear();
						if (op.throws !== undefined) throw op.throws;
					});
					live.set(id, { channel: op.channel, unsub });
				} else if (kind === "off") {
					if (typeof op.channel !== "string") throw new Error("missing channel");
					if (op.handler !== undefined && !isId(op.handler)) throw new Error("handler must be an id");
					if (op.handler !== undefined) {
						offId(op.handler);
					} else {
						for (const [id, entry] of [...live]) {
							if (entry.channel === op.channel && entry.unsub) offId(id);
						}
					}
				} else if (kind === "emit") {
					if (typeof op.channel !== "string") throw new Error("missing channel");
					bus.emit(op.channel, op.data === undefined ? null : op.data);
				} else if (kind === "clear") {
					bus.clear();
				} else {
					throw new Error("unknown op: " + kind);
				}
			}
		} finally {
			console.error = realError;
		}
		return { result: { log, errors } };
	}

	if (tool === "timings") {
		// Script-driven core/timings.js with an injected clock. ENABLED is
		// captured from PI_TIMING at module load, so the env var is set
		// before a cache-busting dynamic import (fresh instance per case).
		const { pathToFileURL } = require("url");
		const enabled = args.enabled === undefined ? true : args.enabled;
		if (typeof enabled !== "boolean") throw new Error("enabled must be a boolean");
		const steps = args.steps === undefined ? [] : args.steps;
		if (!Array.isArray(steps)) throw new Error("missing steps");
		process.env.PI_TIMING = enabled ? "1" : "0";
		globalThis.__piParityTimingsSeq = (globalThis.__piParityTimingsSeq || 0) + 1;
		const t = await import(
			pathToFileURL(path.join(distDir, "core", "timings.js")).href +
				"?parity=" + globalThis.__piParityTimingsSeq
		);
		const namespace = (step) => {
			if (step.namespace === undefined || step.namespace === "main") return "main";
			if (step.namespace === "extensions") return "extensions";
			throw new Error("unknown namespace: " + step.namespace);
		};
		let now = 1000;
		const realDateNow = Date.now;
		Date.now = () => now;
		const outputs = [];
		const realError = console.error;
		let capturing = false;
		const chunks = [];
		console.error = (...msgs) => {
			if (capturing) chunks.push(msgs.join(" "));
			else realError(...msgs);
		};
		try {
			for (const step of steps) {
				switch (step.step) {
					case "reset":
						t.resetTimings(namespace(step));
						break;
					case "time":
						if (typeof step.label !== "string") throw new Error("missing label");
						t.time(step.label, namespace(step));
						break;
					case "advance":
						if (typeof step.ms !== "number" || !Number.isInteger(step.ms)) throw new Error("missing ms");
						now += step.ms;
						break;
					case "set":
						if (typeof step.ms !== "number" || !Number.isInteger(step.ms)) throw new Error("missing ms");
						now = step.ms;
						break;
					case "print":
						capturing = true;
						try {
							t.printTimings();
						} finally {
							capturing = false;
						}
						// Each console.error(msg) line contributes msg + "\n".
						outputs.push(chunks.map((c) => c + "\n").join(""));
						chunks.length = 0;
						break;
					default:
						throw new Error("unknown step: " + step.step);
				}
			}
		} finally {
			console.error = realError;
			Date.now = realDateNow;
		}
		return { result: { outputs } };
	}

	if (tool === "slash-commands") {
		const { BUILTIN_SLASH_COMMANDS } = require(path.join(distDir, "core", "slash-commands.js"));
		if (typeof args.op !== "string") throw new Error("missing op");
		if (args.op === "list") {
			// APP_NAME is baked from package.json at module load ("pi" in the
			// shipped package) and is not injectable; args.app is ignored.
			return { result: { commands: BUILTIN_SLASH_COMMANDS.map((c) => ({ ...c })) } };
		}
		if (args.op === "roundtrip") {
			if (typeof args.kind !== "string") throw new Error("missing kind");
			if (args.kind !== "builtin" && args.kind !== "info") throw new Error("unknown kind: " + args.kind);
			const value = args.value;
			if (value === undefined || value === null || typeof value !== "object" || Array.isArray(value)) {
				throw new Error("missing value");
			}
			// The TS module performs no runtime validation: return as-is.
			return { result: { value: JSON.parse(JSON.stringify(value)) } };
		}
		throw new Error("unknown op: " + args.op);
	}

	if (tool === "settings-defaults") {
		const { SETTINGS_DEFAULTS } = require(path.join(distDir, "core", "settings-defaults.js"));
		if (args.calls === undefined) return { result: { defaults: SETTINGS_DEFAULTS } };
		if (typeof args.calls !== "number" || !Number.isInteger(args.calls) || args.calls < 0) {
			throw new Error("calls must be a non-negative integer");
		}
		const defaults = [];
		for (let i = 0; i < args.calls; i++) defaults.push(SETTINGS_DEFAULTS);
		return { result: { defaults } };
	}

	if (tool === "settings-schema") {
		const { pathToFileURL } = require("url");
		const { createRequire } = require("module");
		const { SettingsSchema } = await import(
			pathToFileURL(path.join(distDir, "core", "settings-schema.js")).href
		);
		// Resolve typebox from the dist tree (ts-harness has no node_modules).
		const distRequire = createRequire(path.join(distDir, "core", "anchor.js"));
		const { Compile } = distRequire("typebox/compile");
		if (args.settings === undefined) throw new Error("missing settings");
		const compiled = Compile(SettingsSchema);
		const errors = [...compiled.Errors(args.settings)].map((error) => ({
			path: formatValidationPath(error),
			instancePath: error.instancePath,
			message: error.message,
		}));
		return { result: { errors } };
	}

	if (tool === "settings-manager") {
		// Script-driven core/settings-manager.js (fresh cache-busted instance
		// per case). crypto.randomUUID is overridden via the startup dispatcher
		// (see top of file) and env /
		// process.platform are scrubbed+injected so both sides see the same
		// ambient state; everything is restored in the finally block.
		const { pathToFileURL } = require("url");
		const fs = require("fs");
		const os = require("os");
		const mode = args.mode;
		if (typeof mode !== "string") throw new Error("missing mode");
		if (mode !== "memory" && mode !== "files" && mode !== "inMemory") {
			throw new Error("unknown settings-manager mode: " + mode);
		}
		const storageIn = args.storage ?? {};
		for (const key of ["global", "project"]) {
			const v = storageIn[key];
			if (v !== undefined && v !== null && typeof v !== "string") {
				throw new Error(`storage.${key} must be a string or null`);
			}
		}
		const optionsIn = args.options ?? {};
		if (optionsIn.projectTrusted !== undefined && typeof optionsIn.projectTrusted !== "boolean") {
			throw new Error("options.projectTrusted must be a boolean");
		}
		if (optionsIn.env !== undefined) {
			if (optionsIn.env === null || typeof optionsIn.env !== "object" || Array.isArray(optionsIn.env)) {
				throw new Error("options.env must be an object");
			}
			for (const v of Object.values(optionsIn.env)) {
				if (typeof v !== "string") throw new Error("options.env values must be strings");
			}
		}
		if (optionsIn.platform !== undefined && typeof optionsIn.platform !== "string") {
			throw new Error("options.platform must be a string");
		}
		if (optionsIn.uuids !== undefined) {
			if (!Array.isArray(optionsIn.uuids) || optionsIn.uuids.some((v) => typeof v !== "string")) {
				throw new Error("options.uuids must be an array of strings");
			}
		}
		const ops = args.ops;
		if (!Array.isArray(ops)) throw new Error("missing ops");

		const uuids = optionsIn.uuids ?? [];
		let uuidI = 0;
		const nextUuid = () => {
			const i = uuidI++;
			return uuids.length === 0 ? "uuid-" + (i + 1) : uuids[Math.min(i, uuids.length - 1)];
		};
		globalThis.__piParityUuidOverride = nextUuid;
		const savedEnv = { ...process.env };
		for (const key of Object.keys(process.env)) delete process.env[key];
		Object.assign(process.env, optionsIn.env ?? {});
		const platDesc = Object.getOwnPropertyDescriptor(process, "platform");
		Object.defineProperty(process, "platform", {
			value: optionsIn.platform ?? "linux",
			configurable: true,
			writable: true,
		});
		globalThis.__piParitySmSeq = (globalThis.__piParitySmSeq || 0) + 1;
		const sm = await import(
			pathToFileURL(path.join(distDir, "core", "settings-manager.js")).href +
				"?parity=" + globalThis.__piParitySmSeq
		);
		let cwdDir;
		let agentDir;
		try {
			const options =
				optionsIn.projectTrusted !== undefined ? { projectTrusted: optionsIn.projectTrusted } : {};
			let manager;
			let memStorage;
			if (mode === "memory") {
				memStorage = new sm.InMemorySettingsStorage();
				if (typeof storageIn.global === "string") memStorage.withLock("global", () => storageIn.global);
				if (typeof storageIn.project === "string") memStorage.withLock("project", () => storageIn.project);
				manager = sm.SettingsManager.fromStorage(memStorage, options);
			} else if (mode === "inMemory") {
				manager = sm.SettingsManager.inMemory(args.settings === undefined ? {} : args.settings, options);
			} else {
				cwdDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-sm-cwd-"));
				agentDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-sm-agent-"));
				if (typeof storageIn.global === "string") {
					fs.writeFileSync(path.join(agentDir, "settings.json"), storageIn.global);
				}
				if (typeof storageIn.project === "string") {
					fs.mkdirSync(path.join(cwdDir, ".pi"), { recursive: true });
					fs.writeFileSync(path.join(cwdDir, ".pi", "settings.json"), storageIn.project);
				}
				manager = sm.SettingsManager.create(cwdDir, agentDir, options);
			}
			const normPath = (p) =>
				typeof p === "string" ? p.split(agentDir).join("<agentDir>").split(cwdDir).join("<cwd>") : p;
			const globalPath = () => path.join(agentDir, "settings.json");
			const projectPath = () => path.join(cwdDir, ".pi", "settings.json");

			// Argument validation with Rust-identical messages.
			const ck = (method, list, tags) => {
				if (list.length !== tags.length) {
					throw new Error(`settings-manager: ${method} expects ${tags.length} argument(s), got ${list.length}`);
				}
				tags.forEach((tag, i) => {
					const a = list[i];
					const bad = (what) => {
						throw new Error(`settings-manager: ${method} argument ${i} must be ${what}`);
					};
					if (tag === "s" && typeof a !== "string") bad("a string");
					if (tag === "b" && typeof a !== "boolean") bad("a boolean");
					if (tag === "n" && typeof a !== "number") bad("a number");
					if (tag === "s?" && a !== null && typeof a !== "string") bad("a string or null");
					if (tag === "sa" && !(Array.isArray(a) && a.every((x) => typeof x === "string"))) bad("an array of strings");
					if (
						tag === "sa?" &&
						a !== null &&
						!(Array.isArray(a) && a.every((x) => typeof x === "string"))
					)
						bad("an array of strings or null");
					if (tag === "va" && !Array.isArray(a)) bad("an array");
					if (
						tag === "m?" &&
						a !== null &&
						!(typeof a === "object" && !Array.isArray(a) && typeof a.provider === "string" && typeof a.id === "string")
					)
						bad("a model or null");
					if (tag === "w" && a !== "auto" && typeof a !== "number") bad('"auto" or a number');
				});
			};
			const METHODS = {
				getSettings: { t: [], r: (m) => m.getSettings() },
				getGlobalSettings: { t: [], r: (m) => m.getGlobalSettings() },
				getProjectSettings: { t: [], r: (m) => m.getProjectSettings() },
				isProjectTrusted: { t: [], r: (m) => m.isProjectTrusted() },
				getLastChangelogVersion: { t: [], r: (m) => m.getLastChangelogVersion() },
				setLastChangelogVersion: { t: ["s"], r: (m, a) => m.setLastChangelogVersion(a[0]) },
				getSessionDir: { t: [], r: (m) => m.getSessionDir() },
				getDefaultProvider: { t: [], r: (m) => m.getDefaultProvider() },
				getDefaultModel: { t: [], r: (m) => m.getDefaultModel() },
				setDefaultProvider: { t: ["s"], r: (m, a) => m.setDefaultProvider(a[0]) },
				setDefaultModel: { t: ["s"], r: (m, a) => m.setDefaultModel(a[0]) },
				setDefaultModelAndProvider: { t: ["s", "s"], r: (m, a) => m.setDefaultModelAndProvider(a[0], a[1]) },
				getSteeringMode: { t: [], r: (m) => m.getSteeringMode() },
				setSteeringMode: { t: ["s"], r: (m, a) => m.setSteeringMode(a[0]) },
				getFollowUpMode: { t: [], r: (m) => m.getFollowUpMode() },
				setFollowUpMode: { t: ["s"], r: (m, a) => m.setFollowUpMode(a[0]) },
				getThemeSetting: { t: [], r: (m) => m.getThemeSetting() },
				getTheme: { t: [], r: (m) => m.getTheme() },
				setTheme: { t: ["s"], r: (m, a) => m.setTheme(a[0]) },
				getDefaultThinkingLevel: { t: [], r: (m) => m.getDefaultThinkingLevel() },
				setDefaultThinkingLevel: { t: ["s"], r: (m, a) => m.setDefaultThinkingLevel(a[0]) },
				getModelThinkingLevel: { t: ["s", "s"], r: (m, a) => m.getModelThinkingLevel(a[0], a[1]) },
				getAllModelThinkingLevels: { t: [], r: (m) => m.getAllModelThinkingLevels() },
				setModelThinkingLevel: { t: ["s", "s", "s"], r: (m, a) => m.setModelThinkingLevel(a[0], a[1], a[2]) },
				removeModelThinkingLevel: { t: ["s", "s"], r: (m, a) => m.removeModelThinkingLevel(a[0], a[1]) },
				getTransport: { t: [], r: (m) => m.getTransport() },
				setTransport: { t: ["s"], r: (m, a) => m.setTransport(a[0]) },
				getSessionShareDestination: { t: [], r: (m) => m.getSessionShareDestination() },
				setSessionShareDestination: { t: ["s"], r: (m, a) => m.setSessionShareDestination(a[0]) },
				getCompactionEnabled: { t: [], r: (m) => m.getCompactionEnabled() },
				setCompactionEnabled: { t: ["b"], r: (m, a) => m.setCompactionEnabled(a[0]) },
				getCompactionReserveTokens: { t: ["m?"], r: (m, a) => m.getCompactionReserveTokens(a[0]) },
				getCompactionKeepRecentTokens: { t: ["m?"], r: (m, a) => m.getCompactionKeepRecentTokens(a[0]) },
				getCompactionSettings: { t: ["m?"], r: (m, a) => m.getCompactionSettings(a[0]) },
				getBranchSummarySettings: { t: [], r: (m) => m.getBranchSummarySettings() },
				getBranchSummarySkipPrompt: { t: [], r: (m) => m.getBranchSummarySkipPrompt() },
				getRetryEnabled: { t: [], r: (m) => m.getRetryEnabled() },
				setRetryEnabled: { t: ["b"], r: (m, a) => m.setRetryEnabled(a[0]) },
				getRetrySettings: { t: [], r: (m) => m.getRetrySettings() },
				getHttpIdleTimeoutMs: { t: [], r: (m) => m.getHttpIdleTimeoutMs() },
				setHttpIdleTimeoutMs: { t: ["n"], r: (m, a) => m.setHttpIdleTimeoutMs(a[0]) },
				getCacheWarmingMode: { t: [], r: (m) => m.getCacheWarmingMode() },
				setCacheWarmingMode: { t: ["s"], r: (m, a) => m.setCacheWarmingMode(a[0]) },
				getProviderRetrySettings: { t: [], r: (m) => m.getProviderRetrySettings() },
				getWebSocketConnectTimeoutMs: { t: [], r: (m) => m.getWebSocketConnectTimeoutMs() },
				getHideThinkingBlock: { t: [], r: (m) => m.getHideThinkingBlock() },
				setHideThinkingBlock: { t: ["b"], r: (m, a) => m.setHideThinkingBlock(a[0]) },
				getShowCacheMissNotices: { t: [], r: (m) => m.getShowCacheMissNotices() },
				setShowCacheMissNotices: { t: ["b"], r: (m, a) => m.setShowCacheMissNotices(a[0]) },
				getExternalEditorCommand: { t: [], r: (m) => m.getExternalEditorCommand() },
				getShellPath: { t: [], r: (m) => m.getShellPath() },
				setShellPath: { t: ["s?"], r: (m, a) => m.setShellPath(a[0]) },
				getQuietStartup: { t: [], r: (m) => m.getQuietStartup() },
				setQuietStartup: { t: ["v"], r: (m, a) => m.setQuietStartup(a[0]) },
				getDefaultProjectTrust: { t: [], r: (m) => m.getDefaultProjectTrust() },
				setDefaultProjectTrust: { t: ["s"], r: (m, a) => m.setDefaultProjectTrust(a[0]) },
				getShellCommandPrefix: { t: [], r: (m) => m.getShellCommandPrefix() },
				setShellCommandPrefix: { t: ["s?"], r: (m, a) => m.setShellCommandPrefix(a[0]) },
				getNpmCommand: { t: [], r: (m) => m.getNpmCommand() },
				setNpmCommand: { t: ["sa?"], r: (m, a) => m.setNpmCommand(a[0]) },
				getCollapseChangelog: { t: [], r: (m) => m.getCollapseChangelog() },
				setCollapseChangelog: { t: ["b"], r: (m, a) => m.setCollapseChangelog(a[0]) },
				getEnableInstallTelemetry: { t: [], r: (m) => m.getEnableInstallTelemetry() },
				setEnableInstallTelemetry: { t: ["b"], r: (m, a) => m.setEnableInstallTelemetry(a[0]) },
				getEnableAnalytics: { t: [], r: (m) => m.getEnableAnalytics() },
				getTrackingId: { t: [], r: (m) => m.getTrackingId() },
				setEnableAnalytics: { t: ["b"], r: (m, a) => m.setEnableAnalytics(a[0]) },
				getOrCreateDeviceId: { t: [], r: (m) => m.getOrCreateDeviceId() },
				getPackages: { t: [], r: (m) => m.getPackages() },
				setPackages: { t: ["va"], r: (m, a) => m.setPackages(a[0]) },
				setProjectPackages: { t: ["va"], r: (m, a) => m.setProjectPackages(a[0]) },
				getExtensionPaths: { t: [], r: (m) => m.getExtensionPaths() },
				setExtensionPaths: { t: ["va"], r: (m, a) => m.setExtensionPaths(a[0]) },
				setProjectExtensionPaths: { t: ["va"], r: (m, a) => m.setProjectExtensionPaths(a[0]) },
				getSkillPaths: { t: [], r: (m) => m.getSkillPaths() },
				setSkillPaths: { t: ["va"], r: (m, a) => m.setSkillPaths(a[0]) },
				setProjectSkillPaths: { t: ["va"], r: (m, a) => m.setProjectSkillPaths(a[0]) },
				getPromptTemplatePaths: { t: [], r: (m) => m.getPromptTemplatePaths() },
				setPromptTemplatePaths: { t: ["va"], r: (m, a) => m.setPromptTemplatePaths(a[0]) },
				setProjectPromptTemplatePaths: { t: ["va"], r: (m, a) => m.setProjectPromptTemplatePaths(a[0]) },
				getThemePaths: { t: [], r: (m) => m.getThemePaths() },
				setThemePaths: { t: ["va"], r: (m, a) => m.setThemePaths(a[0]) },
				setProjectThemePaths: { t: ["va"], r: (m, a) => m.setProjectThemePaths(a[0]) },
				getEnableSkillCommands: { t: [], r: (m) => m.getEnableSkillCommands() },
				setEnableSkillCommands: { t: ["b"], r: (m, a) => m.setEnableSkillCommands(a[0]) },
				getThinkingBudgets: { t: [], r: (m) => m.getThinkingBudgets() },
				getTerminalCapabilityOverrides: { t: [], r: (m) => m.getTerminalCapabilityOverrides() },
				getShowImages: { t: [], r: (m) => m.getShowImages() },
				setShowImages: { t: ["b"], r: (m, a) => m.setShowImages(a[0]) },
				getImageWidthCells: { t: [], r: (m) => m.getImageWidthCells() },
				setImageWidthCells: { t: ["n"], r: (m, a) => m.setImageWidthCells(a[0]) },
				getClearOnShrink: { t: [], r: (m) => m.getClearOnShrink() },
				setClearOnShrink: { t: ["b"], r: (m, a) => m.setClearOnShrink(a[0]) },
				getShowTerminalProgress: { t: [], r: (m) => m.getShowTerminalProgress() },
				setShowTerminalProgress: { t: ["b"], r: (m, a) => m.setShowTerminalProgress(a[0]) },
				getTuiMode: { t: [], r: (m) => m.getTuiMode() },
				setTuiMode: { t: ["s"], r: (m, a) => m.setTuiMode(a[0]) },
				getFullscreenExitOutput: { t: [], r: (m) => m.getFullscreenExitOutput() },
				setFullscreenExitOutput: { t: ["s"], r: (m, a) => m.setFullscreenExitOutput(a[0]) },
				getFullscreenScrollbar: { t: [], r: (m) => m.getFullscreenScrollbar() },
				setFullscreenScrollbar: { t: ["s"], r: (m, a) => m.setFullscreenScrollbar(a[0]) },
				getFullscreenCopyOnSelect: { t: [], r: (m) => m.getFullscreenCopyOnSelect() },
				setFullscreenCopyOnSelect: { t: ["b"], r: (m, a) => m.setFullscreenCopyOnSelect(a[0]) },
				getFullscreenWheelScrollLines: { t: [], r: (m) => m.getFullscreenWheelScrollLines() },
				setFullscreenWheelScrollLines: { t: ["w"], r: (m, a) => m.setFullscreenWheelScrollLines(a[0]) },
				getImageAutoResize: { t: [], r: (m) => m.getImageAutoResize() },
				setImageAutoResize: { t: ["b"], r: (m, a) => m.setImageAutoResize(a[0]) },
				getBlockImages: { t: [], r: (m) => m.getBlockImages() },
				setBlockImages: { t: ["b"], r: (m, a) => m.setBlockImages(a[0]) },
				getEnabledModels: { t: [], r: (m) => m.getEnabledModels() },
				setEnabledModels: { t: ["sa?"], r: (m, a) => m.setEnabledModels(a[0]) },
				getDefaultTools: { t: [], r: (m) => m.getDefaultTools() },
				getDoubleEscapeAction: { t: [], r: (m) => m.getDoubleEscapeAction() },
				setDoubleEscapeAction: { t: ["s"], r: (m, a) => m.setDoubleEscapeAction(a[0]) },
				getTreeFilterMode: { t: [], r: (m) => m.getTreeFilterMode() },
				setTreeFilterMode: { t: ["s"], r: (m, a) => m.setTreeFilterMode(a[0]) },
				getShowHardwareCursor: { t: [], r: (m) => m.getShowHardwareCursor() },
				setShowHardwareCursor: { t: ["b"], r: (m, a) => m.setShowHardwareCursor(a[0]) },
				getEditorPaddingX: { t: [], r: (m) => m.getEditorPaddingX() },
				setEditorPaddingX: { t: ["n"], r: (m, a) => m.setEditorPaddingX(a[0]) },
				getOutputPad: { t: [], r: (m) => m.getOutputPad() },
				setOutputPad: { t: ["n"], r: (m, a) => m.setOutputPad(a[0]) },
				getAutocompleteMaxVisible: { t: [], r: (m) => m.getAutocompleteMaxVisible() },
				setAutocompleteMaxVisible: { t: ["n"], r: (m, a) => m.setAutocompleteMaxVisible(a[0]) },
				getCodeBlockIndent: { t: [], r: (m) => m.getCodeBlockIndent() },
				getMermaidRenderingMode: { t: [], r: (m) => m.getMermaidRenderingMode() },
				setMermaidRenderingMode: { t: ["s"], r: (m, a) => m.setMermaidRenderingMode(a[0]) },
				getWarnings: { t: [], r: (m) => m.getWarnings() },
				setWarnings: { t: ["v"], r: (m, a) => m.setWarnings(a[0]) },
			};

			const out = [];
			// The op loop must not await between ops: the writeQueue only
			// advances at the explicit flush/reload ops (Rust drains its
			// pending-write deque only there too).
			for (const op of ops) {
				try {
					if (op.op === "get" || op.op === "set") {
						const method = op.method;
						if (typeof method !== "string") throw new Error("missing method");
						const list = Array.isArray(op.args) ? op.args : [];
						if (method === "flush" || method === "reload") {
							ck(method, list, []);
							await manager[method]();
							out.push(null);
						} else {
							const spec = METHODS[method];
							if (!spec) throw new Error("unknown settings-manager method: " + method);
							ck(method, list, spec.t);
							const value = spec.r(manager, list);
							out.push(value === undefined ? null : value);
						}
					} else if (op.op === "flush") {
						await manager.flush();
						out.push(null);
					} else if (op.op === "reload") {
						await manager.reload();
						out.push(null);
					} else if (op.op === "drainErrors") {
						out.push(
							manager.drainErrors().map((e) => {
								const entry = { scope: e.scope };
								if (e.path) entry.path = normPath(e.path);
								entry.message = e.error && e.error.message !== undefined ? e.error.message : String(e.error);
								return entry;
							}),
						);
					} else if (op.op === "setProjectTrusted") {
						const list = Array.isArray(op.args) ? op.args : [];
						ck("setProjectTrusted", list, ["b"]);
						manager.setProjectTrusted(list[0]);
						out.push(null);
					} else if (op.op === "applyOverrides") {
						const list = Array.isArray(op.args) ? op.args : [];
						ck("applyOverrides", list, ["v"]);
						manager.applyOverrides(list[0]);
						out.push(null);
					} else if (op.op === "storage") {
						if (mode === "inMemory") throw new Error("storage op is unavailable in inMemory mode");
						if (mode === "memory") {
							out.push({ global: memStorage.global ?? null, project: memStorage.project ?? null });
						} else {
							const read = (p) => (fs.existsSync(p) ? fs.readFileSync(p, "utf8") : null);
							out.push({ global: read(globalPath()), project: read(projectPath()) });
						}
					} else if (op.op === "external") {
						if (mode === "inMemory") throw new Error("external op is unavailable in inMemory mode");
						const list = Array.isArray(op.args) ? op.args : [];
						ck("external", list, ["v"]);
						const spec = list[0];
						if (spec === null || typeof spec !== "object") throw new Error("missing external spec");
						if (typeof spec.scope !== "string") throw new Error("missing scope");
						if (spec.scope !== "global" && spec.scope !== "project") {
							throw new Error("settings-manager: external op scope must be global or project, got " + spec.scope);
						}
						if (spec.content !== undefined && spec.content !== null && typeof spec.content !== "string") {
							throw new Error("settings-manager: external content must be a string or null");
						}
						if (mode === "memory") {
							memStorage[spec.scope] = spec.content ?? null;
						} else {
							const p = spec.scope === "global" ? globalPath() : projectPath();
							if (spec.content === null || spec.content === undefined) {
								fs.rmSync(p, { force: true });
							} else {
								fs.mkdirSync(path.dirname(p), { recursive: true });
								fs.writeFileSync(p, spec.content);
							}
						}
						out.push(null);
					} else if (op.op === "pure") {
						const fname = op.fn;
						if (typeof fname !== "string") throw new Error("missing fn");
						const list = Array.isArray(op.args) ? op.args : [];
						if (fname === "applyToolModifiers") {
							ck(fname, list, ["sa", "sa"]);
							out.push(sm.applyToolModifiers(list[0], list[1]));
						} else if (fname === "getToolListError") {
							ck(fname, list, ["sa"]);
							out.push(sm.getToolListError(list[0]) ?? null);
						} else if (fname === "isToolModifier") {
							ck(fname, list, ["v"]);
							out.push(sm.isToolModifier(list[0]));
						} else if (fname === "DEFAULT_TOOL_NAMES") {
							ck(fname, list, []);
							out.push([...sm.DEFAULT_TOOL_NAMES]);
						} else {
							throw new Error("unknown settings-manager pure fn: " + fname);
						}
					} else {
						throw new Error("unknown settings-manager op: " + op.op);
					}
				} catch (e) {
					out.push({ thrown: e && e.message !== undefined ? e.message : String(e) });
				}
			}
			return { result: { out } };
		} finally {
			globalThis.__piParityUuidOverride = null;
			for (const key of Object.keys(process.env)) delete process.env[key];
			Object.assign(process.env, savedEnv);
			if (platDesc) Object.defineProperty(process, "platform", platDesc);
			if (cwdDir) fs.rmSync(cwdDir, { recursive: true, force: true });
			if (agentDir) fs.rmSync(agentDir, { recursive: true, force: true });
		}
	}

	return undefined;
}

/**
 * `formatValidationPath` from core/model-config.ts, shared by the
 * settings-schema harness arm: dotted instance path, `required` errors point
 * at the first missing property, an empty base renders as "root". Mirrors
 * `SettingsValidationError::format_path` on the Rust side.
 */
function formatValidationPath(error) {
	if (error.keyword === "required") {
		const requiredProperties = error.params?.requiredProperties;
		const requiredProperty = requiredProperties?.[0];
		if (requiredProperty) {
			const basePath = error.instancePath.replace(/^\//, "").replace(/\//g, ".");
			return basePath ? `${basePath}.${requiredProperty}` : requiredProperty;
		}
	}
	const path = error.instancePath.replace(/^\//, "").replace(/\//g, ".");
	return path || "root";
}

let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => (input += d));
process.stdin.on("end", async () => {
	const cwd = process.argv[2];
	// --batch: stdin is a JSON array of {tool, args}; stdout is an array of
	// per-case results (same shapes as single mode; errors in-band). Used by
	// the parity/fuzz batter framework to amortize process startup.
	if (process.argv.includes("--batch")) {
		const payloads = JSON.parse(input);
		if (!Array.isArray(payloads)) throw new Error("--batch expects a JSON array");
		const out = [];
		for (const { tool, args } of payloads) {
			try {
				out.push(await runOne(tool, args, cwd));
			} catch (e) {
				out.push({ error: errMsg(e) });
			}
		}
		process.stdout.write(JSON.stringify(out));
		return;
	}
	const { tool, args } = JSON.parse(input);
	try {
		process.stdout.write(JSON.stringify(await runOne(tool, args, cwd)));
	} catch (e) {
		process.stdout.write(JSON.stringify({ error: errMsg(e) }));
	}
});

/**
 * Error message for in-band results. Both implementations report
 * "Debug log written to: <path>" with an environment-specific path
 * (logDirectory/tmpdir + timestamped filename) on the rendered-line-width
 * crash; redact it so the two sides compare equal.
 */
function errMsg(e) {
	const msg = e && e.message !== undefined ? e.message : String(e);
	return msg.replace(/Debug log written to: .*/g, "Debug log written to: <redacted>");
}

/** Run a single {tool, args} payload (module entry first, then AgentTool). */
async function runOne(tool, args, cwd) {
	const moduleResult = await runModule(tool, args, cwd);
	if (moduleResult !== undefined) {
		return moduleResult;
	}
	const t = FACTORIES[tool](cwd);
	const prepared = t.prepareArguments ? t.prepareArguments(args) : args;
	return await t.execute("parity-call", prepared);
}
