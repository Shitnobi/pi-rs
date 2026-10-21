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
 * - "prompt-templates": core/prompt-templates.js pure ops + per-case temp
 *   fixture load (paths `<base>`-normalized, arrays order-insensitive)
 * - "skills": core/skills.js loadFromDir/load (same fixture style) +
 *   formatForPrompt over inline skill objects
 * - "agent-queue": packages/agent Agent queue surface (steer/followUp
 *   queues, modes, peek/clear) driven against a real Agent with a
 *   never-called dummy streamFn
 * - "declare-tool-changes": packages/agent first-request reduction — a real
 *   Agent (scripted streamFn recording the TranscriptContext messages,
 *   Date.now pinned to nowMs) run once over the pending batch
 * - "run-tool-call": packages/agent agent-loop runToolCall with scripted
 *   tools (update/return/throw steps), scripted prepareArguments,
 *   before/after hooks, and a pinned performance.now clock
 * - "stream-assistant": packages/agent agent-loop runAgentLoop driving
 *   streamAssistantResponse over a single scripted provider turn (events
 *   from `script`, final message from `result`; Date.now pinned to nowMs)
 * - "agent-loop": packages/agent agent-loop runAgentLoop over multiple
 *   scripted provider turns with scripted tools (update/return/throw/
 *   steer/followUp/abort steps), hook scripts (steering/followUp/
 *   finishTurn/prepareRequest/prepareNextTurn), before/after hooks, and
 *   pinned Date.now + performance.now clocks
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
				// The migration generates random 8-hex entry ids (randomUUID is
				// imported as ESM, invisible to the harness crypto override), so
				// rewrite exact-8-hex string values to <id8> — mirrored by the
				// Rust harness's normalize_id8_strings.
				const rewriteIds = (v) => {
					if (typeof v === "string") return /^[0-9a-f]{8}$/.test(v) ? "<id8>" : v;
					if (Array.isArray(v)) return v.map(rewriteIds);
					if (v !== null && typeof v === "object") {
						for (const k of Object.keys(v)) v[k] = rewriteIds(v[k]);
						return v;
					}
					return v;
				};
				rewriteIds(entries);
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

	if (tool === "prompt-templates") {
		// core/prompt-templates.js: pure ops plus a `load` op with a per-case
		// temp fixture. `files` is materialized under mkdtemp, `$HOME` is
		// injected so `~` expansion matches the Rust home_dir option, and
		// template/diagnostic arrays are base-normalized (`<base>`) and
		// sorted by (filePath, name) / (path, message) — readdir order
		// differs between the two sides.
		const { pathToFileURL } = require("url");
		const fs = require("fs");
		const os = require("os");
		const op = args.op;
		if (typeof op !== "string") throw new Error("missing op");
		const mod = await import(pathToFileURL(path.join(distDir, "core", "prompt-templates.js")).href);
		if (op === "parseCommandArgs") {
			if (typeof args.argsString !== "string") throw new Error("missing argsString");
			return { result: mod.parseCommandArgs(args.argsString) };
		}
		if (op === "substituteArgs") {
			if (typeof args.content !== "string") throw new Error("content must be a string");
			const list = args.args ?? [];
			if (!Array.isArray(list) || list.some((v) => typeof v !== "string")) {
				throw new Error("args must be an array of strings");
			}
			return { result: mod.substituteArgs(args.content, list) };
		}
		if (op === "expandPromptTemplate") {
			if (typeof args.text !== "string") throw new Error("text must be a string");
			const templatesIn = args.templates ?? [];
			if (!Array.isArray(templatesIn)) throw new Error("templates must be an array");
			const templates = templatesIn.map((t) => {
				if (t === null || typeof t !== "object" || Array.isArray(t)) {
					throw new Error("templates entries must be objects");
				}
				if (typeof t.name !== "string") throw new Error("templates entries must have a string name");
				if (t.content !== undefined && typeof t.content !== "string") {
					throw new Error("templates entry content must be a string");
				}
				if (t.description !== undefined && typeof t.description !== "string") {
					throw new Error("templates entry description must be a string");
				}
				return { name: t.name, description: t.description ?? "", content: t.content ?? "", filePath: "" };
			});
			return { result: mod.expandPromptTemplate(args.text, templates) };
		}
		if (op === "load") {
			const files = parityValidateFiles(args.files);
			const optionsIn = args.options ?? {};
			if (optionsIn === null || typeof optionsIn !== "object" || Array.isArray(optionsIn)) {
				throw new Error("options must be an object");
			}
			const cwdRel = parityOptString(optionsIn, "cwd", "options.cwd must be a string") ?? "";
			const agentRel = parityOptString(optionsIn, "agentDir", "options.agentDir must be a string") ?? "";
			const homeRel = parityOptString(optionsIn, "homeDir", "options.homeDir must be a string") ?? "home";
			const promptPaths = parityOptStrings(optionsIn, "promptPaths", "options.promptPaths must be an array of strings") ?? [];
			const includeDefaults = parityOptBool(optionsIn, "includeDefaults", "options.includeDefaults must be a boolean") ?? false;
			const base = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-pt-"));
			parityMaterialize(files, base);
			const savedHome = process.env.HOME;
			process.env.HOME = path.join(base, homeRel);
			try {
				const result = mod.loadPromptTemplates({
					cwd: parityJoinRel(base, cwdRel),
					agentDir: parityJoinRel(base, agentRel),
					promptPaths,
					includeDefaults,
				});
				return {
					result: {
						templates_or_skills: result.templates.map((t) => parityNormalizeBase(t, base)).sort(parityCmp2("filePath", "name")),
						diagnostics: result.diagnostics.map((d) => parityNormalizeBase(d, base)).sort(parityCmp2("path", "message")),
					},
				};
			} finally {
				if (savedHome === undefined) delete process.env.HOME;
				else process.env.HOME = savedHome;
				fs.rmSync(base, { recursive: true, force: true });
			}
		}
		throw new Error("unknown prompt-templates op: " + op);
	}

	if (tool === "skills") {
		// core/skills.js: loadFromDir / load with the same per-case fixture
		// style, plus formatForPrompt over inline skill objects.
		const { pathToFileURL } = require("url");
		const fs = require("fs");
		const os = require("os");
		const op = args.op;
		if (typeof op !== "string") throw new Error("missing op");
		const mod = await import(pathToFileURL(path.join(distDir, "core", "skills.js")).href);
		if (op === "loadFromDir") {
			const files = parityValidateFiles(args.files);
			if (typeof args.source !== "string") throw new Error("missing source");
			const dirRel = parityOptString(args, "dir", "dir must be a string") ?? "";
			const base = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-sk-"));
			parityMaterialize(files, base);
			try {
				const result = mod.loadSkillsFromDir({ dir: parityJoinRel(base, dirRel), source: args.source });
				return {
					result: {
						templates_or_skills: result.skills.map((s) => parityNormalizeBase(s, base)).sort(parityCmp2("name", "filePath")),
						diagnostics: result.diagnostics.map((d) => parityNormalizeBase(d, base)).sort(parityCmp2("path", "message")),
					},
				};
			} finally {
				fs.rmSync(base, { recursive: true, force: true });
			}
		}
		if (op === "load") {
			const files = parityValidateFiles(args.files);
			const optionsIn = args.options ?? {};
			if (optionsIn === null || typeof optionsIn !== "object" || Array.isArray(optionsIn)) {
				throw new Error("options must be an object");
			}
			const cwdRel = parityOptString(optionsIn, "cwd", "options.cwd must be a string") ?? "";
			const agentRel = parityOptString(optionsIn, "agentDir", "options.agentDir must be a string") ?? "";
			const homeRel = parityOptString(optionsIn, "homeDir", "options.homeDir must be a string") ?? "home";
			const skillPaths = parityOptStrings(optionsIn, "skillPaths", "options.skillPaths must be an array of strings") ?? [];
			const includeDefaults = parityOptBool(optionsIn, "includeDefaults", "options.includeDefaults must be a boolean") ?? false;
			const base = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-sk-"));
			parityMaterialize(files, base);
			const savedHome = process.env.HOME;
			process.env.HOME = path.join(base, homeRel);
			try {
				const result = mod.loadSkills({
					cwd: parityJoinRel(base, cwdRel),
					agentDir: parityJoinRel(base, agentRel),
					skillPaths,
					includeDefaults,
				});
				return {
					result: {
						templates_or_skills: result.skills.map((s) => parityNormalizeBase(s, base)).sort(parityCmp2("name", "filePath")),
						diagnostics: result.diagnostics.map((d) => parityNormalizeBase(d, base)).sort(parityCmp2("path", "message")),
					},
				};
			} finally {
				if (savedHome === undefined) delete process.env.HOME;
				else process.env.HOME = savedHome;
				fs.rmSync(base, { recursive: true, force: true });
			}
		}
		if (op === "formatForPrompt") {
			const skillsIn = args.skills ?? [];
			if (!Array.isArray(skillsIn)) throw new Error("skills must be an array");
			const skills = skillsIn.map((s) => {
				if (s === null || typeof s !== "object" || Array.isArray(s)) {
					throw new Error("skills entries must be objects");
				}
				for (const k of ["name", "description", "filePath"]) {
					if (typeof s[k] !== "string") {
						throw new Error("skills entries must have string name, description and filePath");
					}
				}
				return {
					name: s.name,
					description: s.description,
					filePath: s.filePath,
					disableModelInvocation: Boolean(s.disableModelInvocation),
				};
			});
			let tool = "read";
			if (args.fileReadTool !== undefined && args.fileReadTool !== null) {
				if (typeof args.fileReadTool !== "string") throw new Error("fileReadTool must be a string");
				tool = args.fileReadTool;
			}
			return { result: mod.formatSkillsForPrompt(skills, tool) };
		}
		throw new Error("unknown skills op: " + op);
	}

	if (tool === "sess-manager") {
		// core/session-manager.js (pi-core port). Per-case temp base with
		// optional `files` + `mtimes` (epoch ms) fixtures; results are
		// base-normalized and dynamic uuids/timestamps/8-hex ids rewritten on
		// BOTH sides (neither side can inject uuidv7/randomUUID/new Date).
		const fs = require("fs");
		const os = require("os");
		const sm = require(path.join(distDir, "core", "session-manager.js"));
		const op = args.op;
		if (typeof op !== "string") throw new Error("missing op");
		const base = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-sess-"));
		const rel = (p) => path.resolve(base, p);
		const zeroDur = null;
		try {
			parityMaterialize(parityValidateFiles(args.files), base);
			if (args.mtimes) {
				for (const [p, ms] of Object.entries(args.mtimes)) fs.utimesSync(rel(p), new Date(ms), new Date(ms));
			}
			const entriesOf = () => {
				if (!Array.isArray(args.entries)) throw new Error("missing entries");
				return args.entries;
			};
			const leaf = (v) => v;
			let out;
			if (op === "parseSessionEntries") {
				if (typeof args.content !== "string") throw new Error("missing content");
				out = { entries: sm.parseSessionEntries(args.content) };
			} else if (op === "migrateSessionEntries") {
				const entries = entriesOf();
				sm.migrateSessionEntries(entries);
				out = { entries };
			} else if (op === "getLatestCompactionEntry") {
				out = { entry: sm.getLatestCompactionEntry(entriesOf()) ?? null };
			} else if (op === "sessionEntryToContextMessages") {
				if (!args.entry) throw new Error("missing entry");
				out = { messages: sm.sessionEntryToContextMessages(args.entry) };
			} else if (op === "buildContextEntries") {
				out = { entries: sm.buildContextEntries(entriesOf(), leaf(args.leafId)) };
			} else if (op === "buildSessionProjection") {
				out = sm.buildSessionProjection(entriesOf(), leaf(args.leafId));
			} else if (op === "buildSessionContext") {
				out = sm.buildSessionContext(entriesOf(), leaf(args.leafId));
			} else if (op === "assertValidSessionId") {
				if (typeof args.id !== "string") throw new Error("missing id");
				sm.assertValidSessionId(args.id);
				out = { valid: true };
			} else if (op === "getDefaultSessionDir") {
				if (typeof args.cwd !== "string") throw new Error("missing cwd");
				if (typeof args.agentDir !== "string") throw new Error("missing agentDir");
				out = { dir: sm.getDefaultSessionDir(rel(args.cwd), rel(args.agentDir)) };
			} else if (op === "loadEntriesFromFile") {
				if (typeof args.filePath !== "string") throw new Error("missing filePath");
				out = { entries: sm.loadEntriesFromFile(rel(args.filePath)) };
			} else if (op === "findMostRecentSession") {
				if (typeof args.sessionDir !== "string") throw new Error("missing sessionDir");
				out = { path: sm.findMostRecentSession(rel(args.sessionDir), args.cwd) ?? null };
			} else if (op === "manager") {
				const factory = args.factory ?? "create";
				const opts = () =>
					args.id !== undefined || args.parentSession !== undefined
						? { id: args.id, parentSession: args.parentSession }
						: undefined;
				let mgr;
				if (factory === "create") {
					mgr = sm.SessionManager.create(rel(args.cwd ?? "proj"), rel(args.sessionDir ?? "sess"), opts());
				} else if (factory === "open") {
					if (typeof args.path !== "string") throw new Error("missing path");
					mgr = sm.SessionManager.open(
						rel(args.path),
						args.sessionDir !== undefined ? rel(args.sessionDir) : undefined,
						args.cwdOverride !== undefined ? rel(args.cwdOverride) : undefined,
					);
				} else if (factory === "continueRecent") {
					mgr = sm.SessionManager.continueRecent(rel(args.cwd ?? "proj"), rel(args.sessionDir ?? "sess"));
				} else if (factory === "inMemory") {
					mgr = sm.SessionManager.inMemory(rel(args.cwd ?? "proj"), opts(), args.entries ?? []);
				} else if (factory === "forkFrom") {
					if (typeof args.sourcePath !== "string") throw new Error("missing sourcePath");
					mgr = sm.SessionManager.forkFrom(rel(args.sourcePath), rel(args.targetCwd ?? "proj"), rel(args.sessionDir ?? "sess"), opts());
				} else {
					throw new Error("unknown sess-manager factory: " + factory);
				}
				if (!Array.isArray(args.ops)) throw new Error("missing ops");
				const sopOpts = (sop) =>
					sop.id !== undefined || sop.parentSession !== undefined
						? { id: sop.id, parentSession: sop.parentSession }
						: undefined;
				const proj = (p) => ({
					entries: p.entries.map((e) => ({ sourceEntry: e.sourceEntry, messages: e.messages })),
					messages: p.messages,
					thinkingLevel: p.thinkingLevel,
					model: p.model,
				});
				const ctx = (c) => ({ messages: c.messages, thinkingLevel: c.thinkingLevel, model: c.model });
				const out2 = [];
				for (const sop of args.ops) {
					try {
						if (typeof sop.op !== "string") throw new Error("missing op");
						let v;
						switch (sop.op) {
							case "getCwd": v = mgr.getCwd(); break;
							case "getSessionDir": v = mgr.getSessionDir(); break;
							case "getSessionId": v = mgr.getSessionId(); break;
							case "getSessionFile": v = mgr.getSessionFile() ?? null; break;
							case "getLeafId": v = mgr.getLeafId() ?? null; break;
							case "getLeafEntry": v = mgr.getLeafEntry() ?? null; break;
							case "getEntry": v = mgr.getEntry(sop.id) ?? null; break;
							case "getChildren": v = mgr.getChildren(sop.id); break;
							case "getLabel": v = mgr.getLabel(sop.id) ?? null; break;
							case "getEntryCount": v = mgr.getEntryCount(); break;
							case "isPersisted": v = mgr.isPersisted(); break;
							case "getSessionName": v = mgr.getSessionName() ?? null; break;
							case "getHeader": v = mgr.getHeader(); break;
							case "getEntries": v = mgr.getEntries(); break;
							case "getTree": v = mgr.getTree(); break;
							case "getBranch": v = mgr.getBranch(sop.fromId); break;
							case "buildContextEntries": v = mgr.buildContextEntries(); break;
							case "buildSessionProjection": v = proj(mgr.buildSessionProjection()); break;
							case "buildSessionContext": v = ctx(mgr.buildSessionContext()); break;
							case "newSession": v = mgr.newSession(sopOpts(sop)) ?? null; break;
							case "setSessionFile": mgr.setSessionFile(rel(sop.path)); v = null; break;
							case "appendMessage": v = mgr.appendMessage(sop.message); break;
							case "appendThinkingLevelChange": v = mgr.appendThinkingLevelChange(sop.thinkingLevel); break;
							case "appendModelChange": v = mgr.appendModelChange(sop.provider, sop.modelId); break;
							case "appendUsage": v = mgr.appendUsage(sop.kind, sop.provider, sop.model, sop.usage, sop.note); break;
							case "appendCompaction": v = mgr.appendCompaction(sop.summary, sop.firstKeptEntryId ?? null, sop.tokensBefore, sop.details, sop.fromHook, sop.usage); break;
							case "appendCustomEntry": v = mgr.appendCustomEntry(sop.customType, sop.data); break;
							case "appendSessionInfo": v = mgr.appendSessionInfo(sop.name); break;
							case "appendCustomMessageEntry": v = mgr.appendCustomMessageEntry(sop.customType, sop.content, sop.display, sop.details); break;
							case "appendContextEdit": v = mgr.appendContextEdit(sop.targetId, sop.replacement === undefined ? undefined : sop.replacement); break;
							case "appendLabelChange": v = mgr.appendLabelChange(sop.targetId, sop.label); break;
							case "branch": mgr.branch(sop.id); v = null; break;
							case "resetLeaf": mgr.resetLeaf(); v = null; break;
							case "branchWithSummary": v = mgr.branchWithSummary(sop.branchFromId ?? null, sop.summary, sop.details, sop.fromHook, sop.usage); break;
							case "createBranchedSession": v = mgr.createBranchedSession(sop.leafId) ?? null; break;
							case "readFile": {
								const p = sop.path !== undefined ? rel(sop.path) : mgr.getSessionFile();
								v = p && fs.existsSync(p) ? fs.readFileSync(p, "utf8") : null;
								break;
							}
							case "listDir": {
								const d = sop.dir !== undefined ? rel(sop.dir) : mgr.getSessionDir();
								v = fs.readdirSync(d).sort();
								break;
							}
							case "usesDefaultSessionDir":
								v = mgr.getSessionDir() === sm.getDefaultSessionDir(mgr.getCwd(), rel(sop.agentDir ?? "agent"));
								break;
							default: throw new Error("unknown sess-manager op: " + sop.op);
						}
						out2.push(v === undefined ? null : v);
					} catch (e) {
						out2.push({ thrown: e && e.message !== undefined ? e.message : String(e) });
					}
				}
				out = { out: out2 };
			} else {
				throw new Error("unknown sess-manager op: " + op);
			}
			return { result: sessNorm(out, base) };
		} catch (e) {
			// Factory-level errors embed the per-case temp base; normalize the
			// message like results so both harnesses compare equal.
			throw new Error(sessNorm(e && e.message !== undefined ? e.message : String(e), base));
		} finally {
			fs.rmSync(base, { recursive: true, force: true });
		}
	}

	if (tool === "nested-tool-calls") {
		const nc = require(path.join(distDir, "core", "nested-tool-calls.js"));
		if (!Array.isArray(args.ops)) throw new Error("missing ops");
		const rec = new nc.NestedCallRecorder();
		const handles = [];
		const out = [];
		for (const op of args.ops) {
			try {
				if (typeof op.op !== "string") throw new Error("missing op");
				let v;
				switch (op.op) {
					case "start": {
						if (typeof op.id !== "string") throw new Error("missing id");
						if (typeof op.name !== "string") throw new Error("missing name");
						const record = rec.start({ type: "toolCall", id: op.id, name: op.name, arguments: op.arguments });
						v = record === undefined ? null : handles.push(record) - 1;
						break;
					}
					case "finish":
						rec.finish(
							op.record === null || op.record === undefined ? undefined : handles[op.record],
							op.isError ?? false,
							op.errorText ?? "",
						);
						v = null;
						break;
					case "addUsage":
						if (!op.usage) throw new Error("missing usage");
						rec.addUsage(op.usage);
						v = null;
						break;
					case "totalUsage": v = rec.totalUsage ?? null; break;
					case "snapshot": {
						let s = rec.snapshot();
						if (s) s = { ...s, calls: s.calls.map((c) => ({ ...c, durationMs: c.durationMs === undefined ? undefined : 0 })) };
						v = s ?? null;
						break;
					}
					case "limits": v = { ...nc.NESTED_CALL_LIMITS }; break;
					default: throw new Error("unknown nested-tool-calls op: " + op.op);
				}
				out.push(v === undefined ? null : v);
			} catch (e) {
				out.push({ thrown: e && e.message !== undefined ? e.message : String(e) });
			}
		}
		return { result: { out } };
	}

	if (tool === "pi-manifest") {
		const fs = require("fs");
		const os = require("os");
		const pm = require(path.join(distDir, "core", "pi-manifest.js"));
		const op = args.op;
		if (typeof op !== "string") throw new Error("missing op");
		if (op !== "read") throw new Error("unknown pi-manifest op: " + op);
		if (typeof args.content !== "string") throw new Error("missing content");
		const base = fs.mkdtempSync(path.join(os.tmpdir(), "pi-parity-pm-"));
		try {
			const file = path.join(base, "package.json");
			fs.writeFileSync(file, args.content);
			return { result: pm.readPiManifest(file) };
		} finally {
			fs.rmSync(base, { recursive: true, force: true });
		}
	}

	if (tool === "defaults") {
		const d = require(path.join(distDir, "core", "defaults.js"));
		const op = args.op;
		if (typeof op !== "string") throw new Error("missing op");
		if (op !== "constants") throw new Error("unknown defaults op: " + op);
		return { result: { defaultThinkingLevel: d.DEFAULT_THINKING_LEVEL, options: [...d.THINKING_LEVEL_OPTIONS] } };
	}

	if (tool === "output-guard") {
		// core/output-guard.js is process-global: patch process.stdout.write /
		// process.stderr.write with scripted recorders (per-call null or
		// {code, message} errors), neutralize process.exit (the TS kills the
		// process on unrecoverable raw-write failures), and restore in the
		// finally block. Raw-write failures surface at waitFor/flush, matching
		// the promise-tail error propagation the Rust entry emulates.
		const og = require(path.join(distDir, "core", "output-guard.js"));
		if (!Array.isArray(args.ops)) throw new Error("missing ops");
		const mkScript = (v) => (Array.isArray(v) ? v.map((x) => (x && typeof x === "object" ? { ...x } : null)) : []);
		const stdoutScript = mkScript(args.stdoutScript);
		const stderrScript = mkScript(args.stderrScript);
		const stdoutLog = [];
		const stderrLog = [];
		const mkWriter = (log, script) => (chunk, a, b) => {
			const cb = typeof a === "function" ? a : b;
			const text = String(chunk);
			log.push(text);
			const step = script.length > 0 ? script.shift() : null;
			if (step) {
				const err = new Error(step.message);
				err.code = step.code;
				if (cb) cb(err);
				return false;
			}
			if (cb) cb(null);
			return true;
		};
		const savedOut = process.stdout.write;
		const savedErr = process.stderr.write;
		const savedExit = process.exit;
		process.stdout.write = mkWriter(stdoutLog, stdoutScript);
		process.stderr.write = mkWriter(stderrLog, stderrScript);
		process.exit = () => {};
		const out = [];
		try {
			for (const op of args.ops) {
				try {
					if (typeof op.op !== "string") throw new Error("missing op");
					let v;
					switch (op.op) {
						case "takeOverStdout": og.takeOverStdout(); v = null; break;
						case "restoreStdout": og.restoreStdout(); v = null; break;
						case "isStdoutTakenOver": v = og.isStdoutTakenOver(); break;
						case "writeStdout":
							if (typeof op.text !== "string") throw new Error("missing text");
							await new Promise((resolve, reject) => {
								process.stdout.write(op.text, (e) => (e ? reject(e) : resolve()));
							});
							v = null;
							break;
						case "writeRawStdout":
							if (typeof op.text !== "string") throw new Error("missing text");
							og.writeRawStdout(op.text);
							v = null;
							break;
						case "flushRawStdout": await og.flushRawStdout(); v = null; break;
						case "waitForRawStdoutBackpressure": await og.waitForRawStdoutBackpressure(); v = null; break;
						default: throw new Error("unknown output-guard op: " + op.op);
					}
					out.push(v === undefined ? null : v);
				} catch (e) {
					out.push({ thrown: e && e.message !== undefined ? e.message : String(e) });
				}
			}
			await og.waitForRawStdoutBackpressure().catch(() => {});
			return { result: { out, stdoutWrites: stdoutLog, stderrWrites: stderrLog } };
		} finally {
			process.stdout.write = savedOut;
			process.stderr.write = savedErr;
			process.exit = savedExit;
		}
	}

	if (tool === "agent-queue") {
		// packages/agent/dist/agent.js (pi-agent-core) — the queue surface of
		// `class Agent`: steering/follow-up queues, queue modes, peek/clear.
		// Mirrors crates/pi-cli/src/agent_harness.rs (AgentQueues port). A
		// real Agent is constructed with a never-invoked dummy streamFn; the
		// initialState seed is not observable through the queue surface.
		// Messages carry literal numeric timestamps — no normalization.
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		const modeOf = (v, what) => {
			if (v === undefined || v === null) return undefined;
			if (v !== "all" && v !== "one-at-a-time") {
				throw new Error(what + ' must be "all" or "one-at-a-time"');
			}
			return v;
		};
		if (!Array.isArray(args.ops)) throw new Error("missing ops");
		const steeringMode = modeOf(args.steeringMode, "steeringMode");
		const followUpMode = modeOf(args.followUpMode, "followUpMode");
		let initialState;
		if (args.initialState !== undefined && args.initialState !== null) {
			if (typeof args.initialState !== "object" || Array.isArray(args.initialState)) {
				throw new Error("initialState must be an object");
			}
			if (args.initialState.messages !== undefined && !Array.isArray(args.initialState.messages)) {
				throw new Error("initialState.messages must be an array");
			}
			if (args.initialState.tools !== undefined && !Array.isArray(args.initialState.tools)) {
				throw new Error("initialState.tools must be an array");
			}
			// Tools have no JSON representation (AgentTool instances); only
			// the message seed reaches the Agent, and it is unobservable here.
			initialState = { messages: args.initialState.messages ?? [] };
		}
		const agent = new Agent({
			streamFn: () => {
				throw new Error("agent-queue harness streamFn must never be called");
			},
			initialState,
			...(steeringMode !== undefined ? { steeringMode } : {}),
			...(followUpMode !== undefined ? { followUpMode } : {}),
		});
		const results = [];
		for (const op of args.ops) {
			if (op === null || typeof op !== "object" || Array.isArray(op)) {
				throw new Error("ops entries must be objects");
			}
			if (typeof op.op !== "string") throw new Error("missing op");
			let v;
			switch (op.op) {
				case "steer":
				case "followUp": {
					if (op.message === undefined) throw new Error("missing message");
					if (op.op === "steer") agent.steer(op.message);
					else agent.followUp(op.message);
					v = null;
					break;
				}
				case "setSteeringMode":
				case "setFollowUpMode": {
					if (op.mode === undefined || op.mode === null) throw new Error("missing mode");
					const mode = modeOf(op.mode, "mode");
					if (op.op === "setSteeringMode") agent.steeringMode = mode;
					else agent.followUpMode = mode;
					v = null;
					break;
				}
				case "getSteeringMode":
					v = agent.steeringMode;
					break;
				case "getFollowUpMode":
					v = agent.followUpMode;
					break;
				case "clearSteering":
					agent.clearSteeringQueue();
					v = null;
					break;
				case "clearFollowUp":
					agent.clearFollowUpQueue();
					v = null;
					break;
				case "clearAll":
					agent.clearAllQueues();
					v = null;
					break;
				case "hasQueued":
					v = agent.hasQueuedMessages();
					break;
				case "peek":
					v = agent.peekQueuedMessages();
					break;
				default:
					throw new Error("unknown agent-queue op: " + op.op);
			}
			results.push(v === undefined ? null : v);
		}
		return { result: { results } };
	}

	if (tool === "declare-tool-changes") {
		// packages/agent/dist/agent.js — the reduced fixed pipeline of what
		// the provider sees on the first request: a REAL Agent is seeded with
		// contextMessages + contextTools and prompted with the pending batch.
		// The scripted streamFn records the TranscriptContext messages of the
		// single request (no tool calls, one turn) and finishes immediately
		// with a minimal assistant message. Date.now is pinned to nowMs for
		// the run (the inserted system message timestamp). Mirrors the Rust
		// harness (declare_tool_changes + default_convert_to_llm over
		// [...contextMessages, ...pending]). Messages carry literal numeric
		// timestamps — no normalization.
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		// Validation order mirrors the Rust entry exactly.
		if (!Array.isArray(args.contextMessages)) throw new Error("missing contextMessages");
		if (!Array.isArray(args.contextTools)) throw new Error("missing contextTools");
		if (!Array.isArray(args.pendingMessages)) throw new Error("missing pendingMessages");
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		const nowMs = args.nowMs;

		// Minimal final assistant message: empty content (no tool calls) and
		// stopReason "stop" so the loop ends after exactly one request.
		const finalMessage = {
			role: "assistant",
			content: [],
			api: "unknown",
			provider: "unknown",
			model: "unknown",
			usage: {
				input: 0,
				output: 0,
				cacheRead: 0,
				cacheWrite: 0,
				totalTokens: 0,
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
			},
			stopReason: "stop",
			timestamp: 0,
		};
		// Hand-rolled AssistantMessageEventStream stand-in: the dist loop
		// for-awaits the events and calls response.result() on the terminal
		// event. A single "done" event carrying the final message satisfies
		// both (no streamed partials, no timing side effects).
		const fakeResponse = {
			async *[Symbol.asyncIterator]() {
				yield { type: "done", message: finalMessage };
			},
			result() {
				return Promise.resolve(finalMessage);
			},
		};
		let llmMessages;
		const streamFn = (_model, context) => {
			if (llmMessages === undefined) llmMessages = context.messages;
			return fakeResponse;
		};
		// AgentTool-shaped context tools (label + dummy execute); the loop
		// only reads name/description/parameters/constrainedSampling through
		// toToolDeclaration.
		const contextTools = args.contextTools.map((t) => ({
			name: typeof t.name === "string" ? t.name : "",
			description: t.description,
			parameters: t.parameters === undefined ? null : t.parameters,
			label: typeof t.label === "string" ? t.label : typeof t.name === "string" ? t.name : "",
			...(t.constrainedSampling !== undefined ? { constrainedSampling: t.constrainedSampling } : {}),
			execute: async () => ({ content: [], details: {} }),
		}));
		const realDateNow = Date.now;
		Date.now = () => nowMs;
		try {
			const agent = new Agent({
				streamFn,
				initialState: { messages: args.contextMessages },
			});
			agent.state.tools = contextTools;
			await agent.prompt(args.pendingMessages);
		} finally {
			Date.now = realDateNow;
		}
		return { result: { llmMessages: llmMessages ?? [] } };
	}

	if (tool === "run-tool-call") {
		// packages/agent/dist/agent-loop.js — the exported runToolCall with
		// scripted tools, hooks, and a pinned performance.now clock. Mirrors
		// the Rust harness (ScriptedTool + SharedPerfClock): every execute
		// step may set perfNow (advancing the clock before its update),
		// emit a parsed partial result via onUpdate, and the final step
		// returns the parsed AgentToolResult or throws. No timestamps are
		// produced (runToolCall adds no messages); durationMs comes from the
		// scripted clock (Math.round of the difference).
		const { runToolCall } = require(path.join(distDir, "..", "..", "agent", "dist", "agent-loop.js"));
		// Validation order mirrors the Rust entry exactly: toolCall, tools,
		// assistantMessage; contextMessages/perfSeq degrade to defaults.
		if (args.toolCall === undefined) throw new Error("missing toolCall");
		if (!Array.isArray(args.tools)) throw new Error("missing tools");
		if (args.assistantMessage === undefined) throw new Error("missing assistantMessage");
		const contextMessages = Array.isArray(args.contextMessages) ? args.contextMessages : [];
		const perfSeq = (Array.isArray(args.perfSeq) ? args.perfSeq : []).filter((v) => typeof v === "number");

		// JSON deep equality (object key order insensitive), mirroring the
		// serde_json Value equality of the Rust ScriptedTool pair match.
		const canonJson = (v) => {
			if (v === null || typeof v !== "object") return v;
			if (Array.isArray(v)) return v.map(canonJson);
			const o = {};
			for (const k of Object.keys(v).sort()) o[k] = canonJson(v[k]);
			return o;
		};
		const jsonEq = (a, b) => JSON.stringify(canonJson(a)) === JSON.stringify(canonJson(b));

		// JSON spec → AgentToolResult, mirroring the Rust parse_tool_result:
		// content defaults to [], details is always present (null when
		// absent), structuredContent/usage drop null, isError/terminate keep
		// booleans only.
		const parseToolResult = (value) => {
			const v = value === undefined || value === null ? {} : value;
			const out = {
				content: Array.isArray(v.content) ? v.content : [],
				details: v.details === undefined || v.details === null ? null : v.details,
			};
			if (v.structuredContent !== undefined && v.structuredContent !== null) out.structuredContent = v.structuredContent;
			if (v.usage !== undefined && v.usage !== null) out.usage = v.usage;
			if (typeof v.isError === "boolean") out.isError = v.isError;
			if (typeof v.terminate === "boolean") out.terminate = v.terminate;
			return out;
		};

		// Pinned monotonic clock: starts at perfSeq[0] (0 when empty); each
		// scripted step's perfNow advances it. Patched before the call (the
		// dist reads the global at call time) and restored after.
		let perfValue = perfSeq.length > 0 ? perfSeq[0] : 0;
		const realPerfNow = performance.now.bind(performance);
		performance.now = () => perfValue;

		// Scripted tool from a JSON spec (name/description/label/parameters/
		// executionMode/prepareArguments/execute), mirroring the Rust
		// ScriptedTool: identity and unmatched-script prepareArguments return
		// the same reference; execute walks update/return/throw steps in
		// order, setting perfNow before each step's update.
		const makeTool = (spec) => {
			const name = typeof spec.name === "string" ? spec.name : "";
			const tool = {
				name,
				description: typeof spec.description === "string" ? spec.description : "",
				label: typeof spec.label === "string" ? spec.label : name,
				parameters: spec.parameters === undefined ? null : spec.parameters,
				execute: async (_toolCallId, _args, _signal, onUpdate) => {
					const script = spec.execute && Array.isArray(spec.execute.script) ? spec.execute.script : [];
					for (let i = 0; i < script.length; i++) {
						const step = script[i];
						const last = i === script.length - 1;
						if (typeof step.perfNow === "number") perfValue = step.perfNow;
						if (step.update !== undefined) onUpdate(parseToolResult(step.update));
						if (last) {
							if (typeof step.throw === "string") throw new Error(step.throw);
							if (step.return !== undefined) return parseToolResult(step.return);
						}
					}
					return parseToolResult(undefined);
				},
			};
			if (spec.executionMode !== undefined) tool.executionMode = spec.executionMode;
			if (spec.prepareArguments !== undefined) {
				tool.prepareArguments = (callArgs) => {
					const prep = spec.prepareArguments;
					if (prep && prep.mode === "script" && Array.isArray(prep.script)) {
						for (const pair of prep.script) {
							if (pair && jsonEq(pair.in, callArgs)) return pair.out;
						}
					}
					return callArgs;
				};
			}
			return tool;
		};

		// Hook shims mirroring the Rust parse_before/parse_after: a "throw"
		// string rejects; block/reason/terminate/content/details/
		// structuredContent/usage/isError keep only their well-typed values
		// (null is treated as absent, exactly like the Rust filters).
		const makeBefore = (rawSpec) => {
			const spec = rawSpec === undefined || rawSpec === null ? {} : rawSpec;
			return async () => {
				if (typeof spec.throw === "string") throw new Error(spec.throw);
				const out = {};
				if (typeof spec.block === "boolean") out.block = spec.block;
				if (typeof spec.reason === "string") out.reason = spec.reason;
				if (typeof spec.terminate === "boolean") out.terminate = spec.terminate;
				return out;
			};
		};
		const makeAfter = (rawSpec) => {
			const spec = rawSpec === undefined || rawSpec === null ? {} : rawSpec;
			return async () => {
				if (typeof spec.throw === "string") throw new Error(spec.throw);
				const out = {};
				if (spec.content !== undefined && spec.content !== null) out.content = Array.isArray(spec.content) ? spec.content : [];
				if (spec.details !== undefined && spec.details !== null) out.details = spec.details;
				if (spec.structuredContent !== undefined && spec.structuredContent !== null) out.structuredContent = spec.structuredContent;
				if (spec.usage !== undefined && spec.usage !== null) out.usage = spec.usage;
				if (typeof spec.isError === "boolean") out.isError = spec.isError;
				if (typeof spec.terminate === "boolean") out.terminate = spec.terminate;
				return out;
			};
		};

		const updates = [];
		let outcome;
		try {
			outcome = await runToolCall(args.toolCall, {
				tools: args.tools.map(makeTool),
				assistantMessage: args.assistantMessage,
				context: { messages: contextMessages, tools: args.contextTools ?? [] },
				...(args.beforeToolCall !== undefined ? { beforeToolCall: makeBefore(args.beforeToolCall) } : {}),
				...(args.afterToolCall !== undefined ? { afterToolCall: makeAfter(args.afterToolCall) } : {}),
				onUpdate: (partialResult) => {
					updates.push(partialResult);
				},
			});
		} finally {
			performance.now = realPerfNow;
		}
		return { result: { outcome, updates } };
	}

	if (tool === "stream-assistant") {
		// packages/agent/dist/agent-loop.js — runAgentLoop driving the
		// (private) streamAssistantResponse over a SINGLE scripted provider
		// turn: events yielded from `script`, final message from `result`
		// (result() is authoritative; the done/error payloads are ignored).
		// The fake streamFn records {apiKey, model, messages} per call —
		// messages is the normalized TranscriptContext after transformContext
		// + convertToLlm. Date.now is pinned to nowMs (declareToolChanges
		// system-message timestamps). Mirrors the Rust harness entry; the
		// shared wire protocol is documented in agent-t34-protocol.md.
		const { runAgentLoop } = require(path.join(distDir, "..", "..", "agent", "dist", "agent-loop.js"));
		// Validation order per the shared protocol spec (byte-exact errors).
		if (!Array.isArray(args.script)) throw new Error("missing script");
		if (args.result === undefined) throw new Error("missing result");
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		if (args.prompts !== undefined && !Array.isArray(args.prompts)) throw new Error("prompts must be an array");
		if (args.contextMessages !== undefined && !Array.isArray(args.contextMessages)) {
			throw new Error("contextMessages must be an array");
		}
		if (args.transformContext !== undefined && args.transformContext !== "identity" && args.transformContext !== "dropSystem") {
			throw new Error('transformContext must be "identity" or "dropSystem"');
		}
		if (args.convertToLlm !== undefined && args.convertToLlm !== "default" && args.convertToLlm !== "dropAssistant") {
			throw new Error('convertToLlm must be "default" or "dropAssistant"');
		}
		const prompts = args.prompts ?? [];
		const contextMessages = args.contextMessages ?? [];

		// Default model (protocol spec, exact JSON / key order).
		const model = {
			id: "unknown",
			name: "unknown",
			api: "unknown",
			provider: "unknown",
			baseUrl: "",
			reasoning: false,
			input: [],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			contextWindow: 0,
			maxTokens: 0,
		};

		// getApiKey pops the next scripted entry per call (exhausted → undefined).
		let getApiKeyIdx = 0;
		const getApiKey = Array.isArray(args.getApiKeyScript)
			? async () => args.getApiKeyScript[getApiKeyIdx++]
			: undefined;

		const config = {
			model,
			reasoning: args.reasoning,
			convertToLlm:
				args.convertToLlm === "dropAssistant"
					? (messages) => messages.filter((m) => m.role === "system" || m.role === "user" || m.role === "toolResult")
					: (messages) =>
							messages.filter((m) => m.role === "system" || m.role === "user" || m.role === "assistant" || m.role === "toolResult"),
			...(args.transformContext === "dropSystem"
				? { transformContext: (messages) => messages.filter((m) => m.role !== "system") }
				: {}),
			...(args.apiKey !== undefined ? { apiKey: args.apiKey } : {}),
			...(getApiKey !== undefined ? { getApiKey } : {}),
		};

		const events = [];
		const streamCalls = [];
		const fakeStreamFn = (m, context, options) => {
			streamCalls.push({ apiKey: options?.apiKey ?? null, model: m, messages: context.messages });
			return {
				async *[Symbol.asyncIterator]() {
					for (const event of args.script) yield event;
				},
				result() {
					return Promise.resolve(args.result);
				},
			};
		};

		const realDateNow = Date.now;
		Date.now = () => args.nowMs;
		let newMessages;
		try {
			newMessages = await runAgentLoop(
				prompts,
				{ messages: contextMessages, tools: [] },
				config,
				async (event) => {
					events.push(event);
				},
				undefined,
				fakeStreamFn,
			);
		} finally {
			Date.now = realDateNow;
		}
		return { result: { events, newMessages, streamCalls } };
	}

	if (tool === "agent-loop") {
		// packages/agent/dist/agent-loop.js — runAgentLoop over multiple
		// scripted provider turns (fake provider pops providerTurns[i] per
		// stream call; exhaustion throws "provider script exhausted", which
		// propagates out of runAgentLoop as the in-band error). Tools are the
		// run-tool-call ScriptedTool shape plus steer/followUp/abort execute
		// steps; hooks are scripted (steering/followUp/finishTurn/
		// prepareRequest/prepareNextTurn) and before/afterToolCall reuse the
		// run-tool-call shims. Date.now + performance.now are pinned.
		// Mirrors the Rust harness entry (agent_loop_harness.rs); shared wire
		// protocol documented in agent-t34-protocol.md.
		const { runAgentLoop } = require(path.join(distDir, "..", "..", "agent", "dist", "agent-loop.js"));
		// Validation order per the shared protocol spec (byte-exact errors).
		if (!Array.isArray(args.prompts)) throw new Error("missing prompts");
		if (!Array.isArray(args.providerTurns)) throw new Error("missing providerTurns");
		if (args.tools !== undefined && !Array.isArray(args.tools)) throw new Error("tools must be an array");
		if (args.toolExecution !== undefined && args.toolExecution !== "parallel" && args.toolExecution !== "sequential") {
			throw new Error('toolExecution must be "parallel" or "sequential"');
		}
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		if (args.contextMessages !== undefined && !Array.isArray(args.contextMessages)) {
			throw new Error("contextMessages must be an array");
		}
		if (args.transformContext !== undefined && args.transformContext !== "identity" && args.transformContext !== "dropSystem") {
			throw new Error('transformContext must be "identity" or "dropSystem"');
		}
		if (args.convertToLlm !== undefined && args.convertToLlm !== "default" && args.convertToLlm !== "dropAssistant") {
			throw new Error('convertToLlm must be "default" or "dropAssistant"');
		}
		// Tool/step shape errors (the complete list; the Rust side mirrors
		// exactly these two):
		// - "tools entries must be objects": a tools entry that is null, an
		//   array, or not an object
		// - "missing tool name": a tools entry whose `name` is not a string
		if (args.tools !== undefined) {
			for (const t of args.tools) {
				if (t === null || typeof t !== "object" || Array.isArray(t)) throw new Error("tools entries must be objects");
				if (typeof t.name !== "string") throw new Error("missing tool name");
			}
		}
		const prompts = args.prompts;
		const contextMessages = args.contextMessages ?? [];
		const perfSeq = (Array.isArray(args.perfSeq) ? args.perfSeq : []).filter((v) => typeof v === "number");

		// Default model (protocol spec, exact JSON / key order).
		const model = {
			id: "unknown",
			name: "unknown",
			api: "unknown",
			provider: "unknown",
			baseUrl: "",
			reasoning: false,
			input: [],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			contextWindow: 0,
			maxTokens: 0,
		};

		// Pinned clocks: Date.now → nowMs (system/toolResult message
		// timestamps), performance.now → the scripted monotonic clock
		// (starts at perfSeq[0] or 0; execute steps may set perfNow).
		let perfValue = perfSeq.length > 0 ? perfSeq[0] : 0;
		const realPerfNow = performance.now.bind(performance);
		const realDateNow = Date.now;
		performance.now = () => perfValue;
		Date.now = () => args.nowMs;

		// Run-owned abort controller: the `abort` execute step fires it.
		const controller = new AbortController();

		// Harness steering/follow-up buffers drained FIRST by the
		// getSteeringMessages/getFollowUpMessages hooks (mid-run injection
		// via tool execute steps), concatenated before the scripted entry.
		const steerBuffer = [];
		const followUpBuffer = [];
		let steeringIdx = 0;
		let followUpIdx = 0;
		let finishIdx = 0;
		let prepareRequestIdx = 0;
		let prepareNextTurnIdx = 0;
		let getApiKeyIdx = 0;

		// Scripted tool: the run-tool-call ScriptedTool shape plus the
		// steer/followUp/abort step kinds. Step processing order: perfNow,
		// then steer/followUp/abort, then update, then (last step)
		// throw/return.
		const makeTool = (spec) => {
			const name = typeof spec.name === "string" ? spec.name : "";
			const tool = {
				name,
				description: typeof spec.description === "string" ? spec.description : "",
				label: typeof spec.label === "string" ? spec.label : name,
				parameters: spec.parameters === undefined ? null : spec.parameters,
				execute: async (_toolCallId, _args, _signal, onUpdate) => {
					const script = spec.execute && Array.isArray(spec.execute.script) ? spec.execute.script : [];
					for (let i = 0; i < script.length; i++) {
						const step = script[i];
						const last = i === script.length - 1;
						if (typeof step.perfNow === "number") perfValue = step.perfNow;
						if (step.steer !== undefined) steerBuffer.push(step.steer);
						if (step.followUp !== undefined) followUpBuffer.push(step.followUp);
						if (step.abort === true) controller.abort();
						if (step.update !== undefined) onUpdate(parityParseToolResult(step.update));
						if (last) {
							if (typeof step.throw === "string") throw new Error(step.throw);
							if (step.return !== undefined) return parityParseToolResult(step.return);
						}
					}
					return parityParseToolResult(undefined);
				},
			};
			if (spec.executionMode !== undefined) tool.executionMode = spec.executionMode;
			if (spec.prepareArguments !== undefined) {
				tool.prepareArguments = (callArgs) => {
					const prep = spec.prepareArguments;
					if (prep && prep.mode === "script" && Array.isArray(prep.script)) {
						for (const pair of prep.script) {
							if (pair && parityJsonEq(pair.in, callArgs)) return pair.out;
						}
					}
					return callArgs;
				};
			}
			return tool;
		};

		// Update-script context entries: {messages?, tools?} → AgentContext
		// (messages defaults to [], tools are ScriptedToolJson specs mapped
		// through makeTool, defaulting to []). The Rust side mirrors this
		// mapping exactly.
		const mapUpdateContext = (c) => ({
			messages: c && Array.isArray(c.messages) ? c.messages : [],
			tools: (c && Array.isArray(c.tools) ? c.tools : []).map(makeTool),
		});

		const getApiKey = Array.isArray(args.getApiKeyScript)
			? async () => args.getApiKeyScript[getApiKeyIdx++]
			: undefined;
		// Absent field → hook undefined (the loop treats that as no poll);
		// present → each poll pops script[i] ?? [] after draining the
		// harness buffer.
		const getSteeringMessages =
			args.steeringScript !== undefined
				? async () => {
						const drained = steerBuffer.splice(0, steerBuffer.length);
						const scripted = args.steeringScript[steeringIdx++] ?? [];
						return [...drained, ...scripted];
					}
				: undefined;
		const getFollowUpMessages =
			args.followUpScript !== undefined
				? async () => {
						const drained = followUpBuffer.splice(0, followUpBuffer.length);
						const scripted = args.followUpScript[followUpIdx++] ?? [];
						return [...drained, ...scripted];
					}
				: undefined;
		// Each call pops decisions[i] ?? null; null → undefined (normal
		// scheduling).
		const finishTurn =
			args.finishTurnDecisions !== undefined
				? async () => {
						const decision = args.finishTurnDecisions[finishIdx++] ?? null;
						return decision === null ? undefined : { action: decision };
					}
				: undefined;
		// Each call pops script[i] ?? null; entry fields map 1:1 onto the
		// AgentRequestUpdate (undefined fields keep current values).
		const prepareRequest =
			args.prepareRequestScript !== undefined
				? async () => {
						const entry = args.prepareRequestScript[prepareRequestIdx++] ?? null;
						if (entry === null || entry === undefined) return undefined;
						const out = {};
						if (entry.context !== undefined) out.context = mapUpdateContext(entry.context);
						if (entry.model !== undefined) out.model = entry.model;
						if (entry.thinkingLevel !== undefined) out.thinkingLevel = entry.thinkingLevel;
						return out;
					}
				: undefined;
		// Each call pops script[i] ?? null; entry fields map 1:1 onto the
		// AgentLoopTurnUpdate (undefined fields keep current values).
		const prepareNextTurn =
			args.prepareNextTurnScript !== undefined
				? async () => {
						const entry = args.prepareNextTurnScript[prepareNextTurnIdx++] ?? null;
						if (entry === null || entry === undefined) return undefined;
						const out = {};
						if (entry.context !== undefined) out.context = mapUpdateContext(entry.context);
						if (entry.messages !== undefined) out.messages = entry.messages;
						if (entry.model !== undefined) out.model = entry.model;
						if (entry.thinkingLevel !== undefined) out.thinkingLevel = entry.thinkingLevel;
						return out;
					}
				: undefined;

		const config = {
			model,
			reasoning: args.reasoning,
			toolExecution: args.toolExecution ?? "parallel",
			convertToLlm:
				args.convertToLlm === "dropAssistant"
					? (messages) => messages.filter((m) => m.role === "system" || m.role === "user" || m.role === "toolResult")
					: (messages) =>
							messages.filter((m) => m.role === "system" || m.role === "user" || m.role === "assistant" || m.role === "toolResult"),
			...(args.transformContext === "dropSystem"
				? { transformContext: (messages) => messages.filter((m) => m.role !== "system") }
				: {}),
			...(args.apiKey !== undefined ? { apiKey: args.apiKey } : {}),
			...(getApiKey !== undefined ? { getApiKey } : {}),
			...(getSteeringMessages !== undefined ? { getSteeringMessages } : {}),
			...(getFollowUpMessages !== undefined ? { getFollowUpMessages } : {}),
			...(finishTurn !== undefined ? { finishTurn } : {}),
			...(prepareRequest !== undefined ? { prepareRequest } : {}),
			...(prepareNextTurn !== undefined ? { prepareNextTurn } : {}),
			...(args.beforeToolCall !== undefined ? { beforeToolCall: parityMakeBefore(args.beforeToolCall) } : {}),
			...(args.afterToolCall !== undefined ? { afterToolCall: parityMakeAfter(args.afterToolCall) } : {}),
		};

		const events = [];
		const streamCalls = [];
		let providerIdx = 0;
		const fakeStreamFn = (m, context, options) => {
			const turn = args.providerTurns[providerIdx];
			if (turn === undefined) throw new Error("provider script exhausted");
			providerIdx++;
			streamCalls.push({ apiKey: options?.apiKey ?? null, model: m, messages: context.messages });
			return {
				async *[Symbol.asyncIterator]() {
					for (const event of turn.events ?? []) yield event;
				},
				result() {
					return Promise.resolve(turn.result);
				},
			};
		};

		let newMessages;
		try {
			newMessages = await runAgentLoop(
				prompts,
				{ messages: contextMessages, tools: (args.tools ?? []).map(makeTool) },
				config,
				async (event) => {
					events.push(event);
				},
				controller.signal,
				fakeStreamFn,
			);
		} finally {
			performance.now = realPerfNow;
			Date.now = realDateNow;
		}
		return { result: { events, newMessages, streamCalls } };
	}

	if (tool === "agent") {
		// packages/agent/dist/agent.js — the REAL Agent class over scripted
		// provider turns (fake provider pops providerTurns[i] per stream
		// call; exhaustion throws "provider script exhausted", which the
		// Agent's runWithLifecycle converts into the synthetic assistant
		// error message). Tools are the agent-loop ScriptedTool shape plus
		// steer/followUp/abort/snapshot execute steps (snapshot records
		// {tag, state} mid-run — the tool has closure access to the agent).
		// The harness subscribes ONE event-recording listener at
		// construction and drives a script of ops (prompt/continue with
		// optional settle:false, steer/followUp/abort/waitForIdle/reset/
		// getState). Date.now + performance.now are pinned. Mirrors the
		// Rust harness entry (agent_harness.rs run_agent_harness); shared
		// wire protocol documented in agent-t5-protocol.md.
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		// Validation order per the shared protocol spec (byte-exact errors).
		if (!Array.isArray(args.script)) throw new Error("missing script");
		if (!Array.isArray(args.providerTurns)) throw new Error("missing providerTurns");
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		if (args.initialState !== undefined && args.initialState !== null) {
			if (typeof args.initialState !== "object" || Array.isArray(args.initialState)) {
				throw new Error("initialState must be an object");
			}
			if (args.initialState.messages !== undefined && !Array.isArray(args.initialState.messages)) {
				throw new Error("initialState.messages must be an array");
			}
			if (args.initialState.tools !== undefined && !Array.isArray(args.initialState.tools)) {
				throw new Error("initialState.tools must be an array");
			}
		}
		if (args.tools !== undefined && !Array.isArray(args.tools)) throw new Error("tools must be an array");
		const initialState = args.initialState ?? {};
		const toolSpecs = Array.isArray(args.tools)
			? args.tools
			: Array.isArray(initialState.tools)
				? initialState.tools
				: [];
		// Tool/step shape errors (same list as the agent-loop entry).
		for (const t of toolSpecs) {
			if (t === null || typeof t !== "object" || Array.isArray(t)) throw new Error("tools entries must be objects");
			if (typeof t.name !== "string") throw new Error("missing tool name");
		}
		const perfSeq = (Array.isArray(args.perfSeq) ? args.perfSeq : []).filter((v) => typeof v === "number");

		// Pinned clocks: Date.now → nowMs (message timestamps), performance.now
		// → the scripted monotonic clock (starts at perfSeq[0] or 0; execute
		// steps may set perfNow).
		let perfValue = perfSeq.length > 0 ? perfSeq[0] : 0;
		const realPerfNow = performance.now.bind(performance);
		const realDateNow = Date.now;
		performance.now = () => perfValue;
		Date.now = () => args.nowMs;

		const events = [];
		const snapshots = [];
		const results = [];
		// settle:false ops: started without awaiting; their rejections are
		// collected (as thrown entries) once awaited at script end.
		const pending = [];
		let agentRef = null;

		// State snapshot (getState op + the tool `snapshot` step): exact
		// shape from the protocol spec; arrays copied so mid-run snapshots
		// observe mid-run state.
		const snapshotState = () => {
			const a = agentRef;
			return {
				messages: a.state.messages.slice(),
				isStreaming: a.state.isStreaming,
				streamingMessage: a.state.streamingMessage ?? null,
				pendingToolCalls: [...a.state.pendingToolCalls],
				errorMessage: a.state.errorMessage ?? null,
				model: a.state.model,
				thinkingLevel: a.state.thinkingLevel,
				toolNames: a.state.tools.map((t) => t.name),
				steeringMode: a.steeringMode,
				followUpMode: a.followUpMode,
			};
		};

		// Scripted tool: the agent-loop ScriptedTool shape plus the
		// steer/followUp/abort/snapshot step kinds (steer/followUp/abort hit
		// the live agent; snapshot records {tag, state}). Step processing
		// order: perfNow, steer/followUp/abort/snapshot, update, then (last
		// step) throw/return.
		const makeTool = (spec) => {
			const name = typeof spec.name === "string" ? spec.name : "";
			const tool = {
				name,
				description: typeof spec.description === "string" ? spec.description : "",
				label: typeof spec.label === "string" ? spec.label : name,
				parameters: spec.parameters === undefined ? null : spec.parameters,
				execute: async (_toolCallId, _args, _signal, onUpdate) => {
					const script = spec.execute && Array.isArray(spec.execute.script) ? spec.execute.script : [];
					for (let i = 0; i < script.length; i++) {
						const step = script[i];
						const last = i === script.length - 1;
						if (typeof step.perfNow === "number") perfValue = step.perfNow;
						if (step.steer !== undefined) agentRef.steer(step.steer);
						if (step.followUp !== undefined) agentRef.followUp(step.followUp);
						if (step.abort === true) agentRef.abort();
						if (step.snapshot !== undefined) snapshots.push({ tag: step.snapshot, state: snapshotState() });
						if (step.update !== undefined) onUpdate(parityParseToolResult(step.update));
						if (last) {
							if (typeof step.throw === "string") throw new Error(step.throw);
							if (step.return !== undefined) return parityParseToolResult(step.return);
						}
					}
					return parityParseToolResult(undefined);
				},
			};
			if (spec.executionMode !== undefined) tool.executionMode = spec.executionMode;
			if (spec.prepareArguments !== undefined) {
				tool.prepareArguments = (callArgs) => {
					const prep = spec.prepareArguments;
					if (prep && prep.mode === "script" && Array.isArray(prep.script)) {
						for (const pair of prep.script) {
							if (pair && parityJsonEq(pair.in, callArgs)) return pair.out;
						}
					}
					return callArgs;
				};
			}
			return tool;
		};

		// Fake provider popping providerTurns[i] per stream call.
		let providerIdx = 0;
		const fakeStreamFn = (_m, _context, _options) => {
			const turn = args.providerTurns[providerIdx];
			if (turn === undefined) throw new Error("provider script exhausted");
			providerIdx++;
			return {
				async *[Symbol.asyncIterator]() {
					for (const event of turn.events ?? []) yield event;
				},
				result() {
					return Promise.resolve(turn.result);
				},
			};
		};

		const agent = new Agent({
			initialState: {
				messages: Array.isArray(initialState.messages) ? initialState.messages : [],
				...(initialState.systemPrompt !== undefined ? { systemPrompt: initialState.systemPrompt } : {}),
				tools: toolSpecs.map(makeTool),
				...(initialState.model !== undefined ? { model: initialState.model } : {}),
				...(initialState.thinkingLevel !== undefined ? { thinkingLevel: initialState.thinkingLevel } : {}),
				...(initialState.steeringMode !== undefined ? { steeringMode: initialState.steeringMode } : {}),
				...(initialState.followUpMode !== undefined ? { followUpMode: initialState.followUpMode } : {}),
			},
			streamFn: fakeStreamFn,
		});
		agentRef = agent;
		agent.subscribe((event) => {
			events.push(event);
		});

		// Start a settle:false op: track the promise without awaiting it;
		// its rejection (if any) becomes a thrown entry at script end.
		const startUnsettled = (promise) => {
			const record = { thrown: undefined };
			const tracked = promise.then(
				() => {},
				(e) => {
					record.thrown = e && e.message !== undefined ? e.message : String(e);
				},
			);
			pending.push({ tracked, record });
			results.push(null);
		};

		try {
			for (const op of args.script) {
				if (op === null || typeof op !== "object" || Array.isArray(op)) {
					throw new Error("ops entries must be objects");
				}
				if (typeof op.op !== "string") throw new Error("missing op");
				try {
					switch (op.op) {
						case "prompt": {
							const input = op.messages !== undefined ? op.messages : op.input;
							if (op.settle === false) startUnsettled(agent.prompt(input));
							else {
								await agent.prompt(input);
								results.push(null);
							}
							break;
						}
						case "continue": {
							if (op.settle === false) startUnsettled(agent.continue());
							else {
								await agent.continue();
								results.push(null);
							}
							break;
						}
						case "steer":
							if (op.message === undefined) throw new Error("missing message");
							agent.steer(op.message);
							results.push(null);
							break;
						case "followUp":
							if (op.message === undefined) throw new Error("missing message");
							agent.followUp(op.message);
							results.push(null);
							break;
						case "abort":
							agent.abort();
							results.push(null);
							break;
						case "waitForIdle":
							await agent.waitForIdle();
							results.push(null);
							break;
						case "reset":
							agent.reset();
							results.push(null);
							break;
						case "getState":
							results.push(snapshotState());
							break;
						default:
							throw new Error("unknown op: " + op.op);
					}
				} catch (e) {
					results.push({ thrown: e && e.message !== undefined ? e.message : String(e) });
				}
			}
			// Await the pending settle:false ops; rejected ones surface as
			// thrown entries (in pending order).
			for (const { tracked, record } of pending) {
				await tracked;
				if (record.thrown !== undefined) results.push({ thrown: record.thrown });
			}
		} finally {
			performance.now = realPerfNow;
			Date.now = realDateNow;
		}
		return { result: { events, stateMessages: agent.state.messages, results, snapshots } };
	}

	if (tool === "agent-session-events") {
		// dist/core/agent-session.js — the events slice of the REAL
		// AgentSession: _handleAgentEvent (persistence, queue bookkeeping,
		// extension + public emission, nested-call merge, turn_end flush,
		// retry classification), sendCustomMessage, _emitAgentSettled,
		// _findPersistedMessageEntryId, waitForIdle. Mirrors the Rust
		// harness (crates/pi-cli/src/session_harness.rs, pi-session port).
		//
		// Message identity: the pool objects are wired into events, agent
		// state, and seeded entries so the WeakMap/WeakSet/reference-compare
		// semantics the Rust arena port emulates stay observable. Collectors
		// deep-clone at collection time (the Rust side snapshots at emit).
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		const { AgentSession } = require(path.join(distDir, "core", "agent-session.js"));
		const { SessionManager } = require(path.join(distDir, "core", "session-manager.js"));
		const { SettingsManager } = require(path.join(distDir, "core", "settings-manager.js"));
		const { DefaultResourceLoader } = require(path.join(distDir, "core", "resource-loader.js"));
		// Validation order mirrors the Rust entry exactly.
		if (!Array.isArray(args.messages)) throw new Error("missing messages");
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		if (typeof args.stateCount !== "number" || !Number.isInteger(args.stateCount)) {
			throw new Error("missing stateCount");
		}
		if (args.stateCount < 0 || args.stateCount > args.messages.length) {
			throw new Error("invalid stateCount");
		}
		if (
			!Array.isArray(args.entries) ||
			args.entries.length === 0 ||
			args.entries[0].type !== "session"
		) {
			throw new Error("missing entries");
		}
		if (args.model !== undefined && args.model !== null) {
			if (typeof args.model !== "object" || Array.isArray(args.model)) {
				throw new Error("invalid model");
			}
		}
		if (!Array.isArray(args.ops)) throw new Error("missing ops");
		const pool = args.messages;
		const poolIndex = (v, err) => {
			if (typeof v !== "number" || !Number.isInteger(v) || v < 0 || v >= pool.length) {
				throw new Error(err);
			}
			return v;
		};
		const indexList = (v, err) => {
			if (!Array.isArray(v)) throw new Error(err);
			return v.map((i) => poolIndex(i, err));
		};
		// Wire entries: message entries carry messageIndex → the pool object
		// (so `entry.message === message` reference scans work).
		const wiredEntries = [];
		const entryMessageIndices = [];
		for (const entry of args.entries) {
			if (entry === null || typeof entry !== "object" || Array.isArray(entry)) {
				throw new Error("entries entries must be objects");
			}
			const wired = {};
			for (const [key, value] of Object.entries(entry)) {
				if (key === "messageIndex") {
					const index = poolIndex(value, "invalid messageIndex");
					wired.message = pool[index];
					if (typeof wired.id === "string") entryMessageIndices.push([wired.id, index]);
				} else {
					wired[key] = value;
				}
			}
			wiredEntries.push(wired);
		}
		const settingsIn = args.settings ?? {};
		if (typeof settingsIn !== "object" || settingsIn === null || Array.isArray(settingsIn)) {
			throw new Error("invalid settings");
		}
		const retryIn = settingsIn.retry ?? {};
		if (typeof retryIn !== "object" || retryIn === null || Array.isArray(retryIn)) {
			throw new Error("invalid settings");
		}
		const extensionConfig = args.extensionConfig ?? {};
		if (typeof extensionConfig !== "object" || extensionConfig === null || Array.isArray(extensionConfig)) {
			throw new Error("invalid extensionConfig");
		}
		const nestedRecordsIn = args.nestedRecords ?? {};
		if (typeof nestedRecordsIn !== "object" || nestedRecordsIn === null || Array.isArray(nestedRecordsIn)) {
			throw new Error("invalid nestedRecords");
		}
		const nestedRecords = { ...nestedRecordsIn };

		const realDateNow = Date.now;
		Date.now = () => args.nowMs;
		// Pin `new Date()` too: appendMessage stamps entries with
		// `new Date().toISOString()`, which Date.now pinning alone leaves at
		// wall-clock time. The Rust side's now generator returns nowMs.
		const RealDate = Date;
		class PinnedDate extends RealDate {
			constructor(...ctorArgs) {
				if (ctorArgs.length === 0) super(args.nowMs);
				else super(...ctorArgs);
			}
		}
		globalThis.Date = PinnedDate;
		let uuidI = 0;
		const nextUuid = () => "uid" + String(++uuidI).padStart(5, "0");
		try {
			const agent = new Agent({
				streamFn: () => {
					throw new Error("agent-session-events harness streamFn must never be called");
				},
				initialState: { messages: [], model: args.model ?? undefined },
			});
			const sessionManager = SessionManager.inMemory("/proj", undefined, wiredEntries);
			const settingsManager = SettingsManager.inMemory({ retry: retryIn });
			const resourceLoader = new DefaultResourceLoader({
				cwd: "/proj",
				agentDir: "/proj/.pi",
				settingsManager,
				noExtensions: true,
			});
			const modelRuntime = { getPhysicalModel: () => undefined };
			const session = new AgentSession({
				agent,
				sessionManager,
				settingsManager,
				cwd: "/proj",
				resourceLoader,
				modelRuntime,
				usesDefaultTools: false,
			});
			// Swap the extension runner with a recording fake (mirrors the
			// Rust RecordingExtensionHost): emit logs, emitMessageEnd logs and
			// returns the configured replacement, no boundary handlers.
			const extensionEvents = [];
			const fakeRunner = {
				hasHandlers: () => false,
				emit: async (event) => {
					extensionEvents.push(snapEvent(event));
				},
				emitMessageEnd: async (event) => {
					extensionEvents.push(snapEvent(event));
					return extensionConfig.messageEndReplacement ?? undefined;
				},
				emitError: () => {},
				invalidate: () => {},
			};
			session._extensionRunner = fakeRunner;
			// Preloaded nested-call records (mirrors PreloadedNestedCallsStore).
			session._nestedToolCalls = {
				takeRecord: (id) => {
					const record = nestedRecords[id];
					delete nestedRecords[id];
					return record;
				},
				clear: () => {
					for (const key of Object.keys(nestedRecords)) delete nestedRecords[key];
				},
			};
			// Seed state with the pool objects (identity!), like the Rust
			// arena seeding + seedStateMessages.
			agent.state.messages = pool.slice(0, args.stateCount);

			const sessionEvents = [];
			const snapEvent = (event) => JSON.parse(JSON.stringify(event));
			const deferredLog = [];
			const unsubscribe = session.subscribe((event) => {
				sessionEvents.push(snapEvent(event));
			});

			// uuid counter: pinned AFTER construction so constructor-time
			// randomness cannot shift it; Rust generates ids only on append.
			globalThis.__piParityUuidOverride = nextUuid;

			const buildEvent = (op) => {
				const event = op.event;
				if (event === null || typeof event !== "object" || Array.isArray(event)) {
					throw new Error("missing event");
				}
				if (typeof event.type !== "string") throw new Error("missing event type");
				switch (event.type) {
					case "agent_start":
						return { type: "agent_start" };
					case "agent_end":
						return { type: "agent_end", messages: indexList(event.messageIndices, "invalid messageIndices").map((i) => pool[i]) };
					case "turn_start":
						return { type: "turn_start" };
					case "turn_end":
						return {
							type: "turn_end",
							message: pool[poolIndex(event.messageIndex, "invalid messageIndex")],
							toolResults: indexList(event.toolResultIndices, "invalid toolResultIndices").map((i) => pool[i]),
						};
					case "message_start":
						return { type: "message_start", message: pool[poolIndex(event.messageIndex, "invalid messageIndex")] };
					case "message_update":
						if (event.assistantMessageEvent === undefined) {
							throw new Error("missing assistantMessageEvent");
						}
						return {
							type: "message_update",
							message: pool[poolIndex(event.messageIndex, "invalid messageIndex")],
							assistantMessageEvent: event.assistantMessageEvent,
						};
					case "message_end":
						return { type: "message_end", message: pool[poolIndex(event.messageIndex, "invalid messageIndex")] };
					case "tool_execution_start":
						if (typeof event.toolCallId !== "string") throw new Error("missing toolCallId");
						if (typeof event.toolName !== "string") throw new Error("missing toolName");
						if (event.args === undefined) throw new Error("missing args");
						return { type: "tool_execution_start", toolCallId: event.toolCallId, toolName: event.toolName, args: event.args };
					case "tool_execution_update":
						if (typeof event.toolCallId !== "string") throw new Error("missing toolCallId");
						if (typeof event.toolName !== "string") throw new Error("missing toolName");
						if (event.args === undefined) throw new Error("missing args");
						if (event.partialResult === undefined) throw new Error("missing partialResult");
						return {
							type: "tool_execution_update",
							toolCallId: event.toolCallId,
							toolName: event.toolName,
							args: event.args,
							partialResult: event.partialResult,
						};
					case "tool_execution_end":
						if (typeof event.toolCallId !== "string") throw new Error("missing toolCallId");
						if (typeof event.toolName !== "string") throw new Error("missing toolName");
						if (event.result === undefined) throw new Error("missing result");
						if (typeof event.isError !== "boolean") throw new Error("missing isError");
						if (event.durationMs !== undefined && typeof event.durationMs !== "number") {
							throw new Error("invalid durationMs");
						}
						return {
							type: "tool_execution_end",
							toolCallId: event.toolCallId,
							toolName: event.toolName,
							result: event.result,
							isError: event.isError,
							...(event.durationMs === undefined ? {} : { durationMs: event.durationMs }),
						};
					default:
						throw new Error("unknown agent event type: " + event.type);
				}
			};

			const strField = (op, key) => {
				if (typeof op[key] !== "string") throw new Error("missing " + key);
				return op[key];
			};
			const boolField = (op, key) => {
				if (typeof op[key] !== "boolean") throw new Error("missing " + key);
				return op[key];
			};

			const results = [];
			const idleWaits = [];
			for (const op of args.ops) {
				if (op === null || typeof op !== "object" || Array.isArray(op)) {
					throw new Error("ops entries must be objects");
				}
				if (typeof op.op !== "string") throw new Error("missing op");
				let v = null;
				switch (op.op) {
					case "agentEvent":
						await session._handleAgentEvent(buildEvent(op));
						break;
					case "queueSteer":
						await session._queueSteer(strField(op, "text"));
						break;
					case "queueFollowUp":
						await session._queueFollowUp(strField(op, "text"));
						break;
					case "setStreaming":
						session._isAgentRunActive = boolField(op, "value");
						break;
					case "setRetryAttempt":
						if (typeof op.value !== "number") throw new Error("missing value");
						session._retryAttempt = op.value;
						break;
					case "requestAbort":
						session._agentRunAbortRequested = true;
						break;
					case "sendCustomMessage": {
						const customType = strField(op, "customType");
						if (op.content === undefined) throw new Error("missing content");
						const display = boolField(op, "display");
						if (op.triggerTurn !== undefined && typeof op.triggerTurn !== "boolean") {
							throw new Error("invalid triggerTurn");
						}
						if (op.deliverAs !== undefined && op.deliverAs !== null && typeof op.deliverAs !== "string") {
							throw new Error("invalid deliverAs");
						}
						await session.sendCustomMessage(
							{ customType, content: op.content, display, details: op.details },
							{
								...(op.triggerTurn === undefined ? {} : { triggerTurn: op.triggerTurn }),
								...(op.deliverAs === undefined ? {} : { deliverAs: op.deliverAs }),
							},
						);
						break;
					}
					case "findPersisted":
						v = session._findPersistedMessageEntryId(pool[poolIndex(op.messageIndex, "invalid messageIndex")]) ?? null;
						break;
					case "emitAgentSettled":
						await session._emitAgentSettled();
						break;
					case "waitForIdle": {
						const tag = strField(op, "tag");
						const entry = { tag, resolved: false };
						idleWaits.push(entry);
						session.waitForIdle().then(() => {
							entry.resolved = true;
						});
						break;
					}
					case "deferAction": {
						const tag = strField(op, "tag");
						// Deferred-Log stand-in (mirrors the Rust
						// push_deferred_log_action seam): the deferred action
						// appends its tag when _emitAgentSettled runs it.
						session._deferredSettledActions.push(async () => {
							deferredLog.push(tag);
						});
						break;
					}
					case "unsubscribe":
						unsubscribe();
						break;
					default:
						throw new Error("unknown agent-session-events op: " + op.op);
				}
				results.push(v === undefined ? null : v);
			}
			// Drain the microtask queue (macrotask turn) so `.then` flags of
			// waits resolved during ops are visible — mirrors the Rust
			// single poll of each collected idle future.
			await new Promise((r) => setImmediate(r));
			const queueState = {
				steering: session._steeringMessages,
				followUp: session._followUpMessages,
				isIdle: session.isIdle,
				retryAttempt: session.retryAttempt,
				pendingCustomMessages: session._pendingCustomMessages,
				lastAssistantToolResults: session._lastAssistantToolResults,
				lastActivityOutcome: session._lastActivityOutcome,
				turnIndex: session._turnIndex,
			};
			const deferredLogOut = deferredLog.slice();
			return {
				result: {
					results,
					sessionEvents,
					extensionEvents,
					entries: sessionManager.getEntries(),
					queueState,
					deferredLog: deferredLogOut,
					idleWaits,
				},
			};
		} finally {
			globalThis.__piParityUuidOverride = null;
			Date.now = realDateNow;
			globalThis.Date = RealDate;
		}
	}


	if (tool === "agent-session-tools") {
		// dist/core/agent-session.js — the tools slice of the REAL
		// AgentSession: _refreshToolRegistry, _applyToolLoadout (incl.
		// prepareLoadout hooks), exposure/allowlist algebra,
		// _rebuildSystemPrompt glue, _restoreToolsFromTranscript, the
		// hidden-declarations + forced-prompt projections, and the
		// constructor's _buildRuntime. Mirrors the Rust harness
		// (crates/pi-cli/src/session_tools_harness.rs, pi-session port).
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		const { AgentSession } = require(path.join(distDir, "core", "agent-session.js"));
		const { SessionManager } = require(path.join(distDir, "core", "session-manager.js"));
		const { SettingsManager } = require(path.join(distDir, "core", "settings-manager.js"));
		const { DefaultResourceLoader } = require(path.join(distDir, "core", "resource-loader.js"));
		// Validation order mirrors the Rust entry exactly.
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		const config = args.config ?? {};
		if (typeof config !== "object" || config === null || Array.isArray(config)) {
			throw new Error("invalid config");
		}
		if (typeof args.baseTools !== "object" || args.baseTools === null || Array.isArray(args.baseTools)) {
			throw new Error("missing baseTools");
		}
		const customToolsIn = args.customTools ?? [];
		if (!Array.isArray(customToolsIn)) throw new Error("invalid customTools");
		const extensionToolsIn = args.extensionTools ?? [];
		if (!Array.isArray(extensionToolsIn)) throw new Error("invalid extensionTools");
		if (
			!Array.isArray(args.entries) ||
			args.entries.length === 0 ||
			args.entries[0].type !== "session"
		) {
			throw new Error("missing entries");
		}
		const messages = args.messages ?? [];
		if (!Array.isArray(messages)) throw new Error("invalid messages");
		if (!Array.isArray(args.ops)) throw new Error("missing ops");

		const EXPOSURES = ["direct", "model-only", "codemode", "deferred", "hidden"];
		const buildDefinition = (d) => {
			if (d === null || typeof d !== "object" || Array.isArray(d)) {
				throw new Error("customTools entries must be objects");
			}
			if (d.exposure !== undefined && !EXPOSURES.includes(d.exposure)) {
				throw new Error("invalid exposure");
			}
			if (
				d.prepareLoadout !== undefined &&
				d.prepareLoadout?.kind !== "changes" &&
				d.prepareLoadout?.kind !== "throw"
			) {
				throw new Error("invalid prepareLoadout");
			}
			return {
				name: d.name,
				label: d.label,
				description: d.description,
				parameters: d.parameters ?? {},
				...(d.promptSnippet !== undefined ? { promptSnippet: d.promptSnippet } : {}),
				...(d.promptGuidelines !== undefined ? { promptGuidelines: d.promptGuidelines } : {}),
				...(d.exposure !== undefined ? { exposure: d.exposure } : {}),
				...(d.namespace !== undefined ? { namespace: d.namespace } : {}),
				...(d.annotations !== undefined ? { annotations: d.annotations } : {}),
				...(d.reachesTools !== undefined ? { reachesTools: d.reachesTools } : {}),
				...(d.defaultActive !== undefined ? { defaultActive: d.defaultActive } : {}),
				...(d.prepareLoadout !== undefined
					? {
							prepareLoadout: (loadout) => {
								if (d.prepareLoadout.kind === "throw") {
									throw new Error(d.prepareLoadout.message);
								}
								return {
									...(d.prepareLoadout.descriptions !== undefined
										? { descriptions: d.prepareLoadout.descriptions }
										: {}),
									...(d.prepareLoadout.hiddenDeclarations !== undefined
										? { hiddenDeclarations: d.prepareLoadout.hiddenDeclarations }
										: {}),
								};
							},
						}
					: {}),
				execute: () => {
					throw new Error("agent-session-tools harness tool execute must never be called");
				},
			};
		};
		const customTools = customToolsIn.map(buildDefinition);
		const extensionToolList = extensionToolsIn.map((t) => {
			if (t === null || typeof t !== "object" || Array.isArray(t)) {
				throw new Error("extensionTools entries must be objects");
			}
			if (t.definition === undefined) throw new Error("missing definition");
			if (t.sourceInfo === undefined) throw new Error("missing sourceInfo");
			return { definition: buildDefinition(t.definition), sourceInfo: t.sourceInfo };
		});
		const baseToolsMap = {};
		for (const [key, spec] of Object.entries(args.baseTools)) {
			if (spec === null || typeof spec !== "object" || Array.isArray(spec)) {
				throw new Error("baseTools entries must be objects");
			}
			baseToolsMap[key] = {
				name: spec.name,
				label: spec.label,
				description: spec.description,
				parameters: spec.parameters ?? {},
				execute: () => {
					throw new Error("agent-session-tools harness tool execute must never be called");
				},
			};
		}

		const realDateNow = Date.now;
		Date.now = () => args.nowMs;
		const RealDate = Date;
		class PinnedDate extends RealDate {
			constructor(...ctorArgs) {
				if (ctorArgs.length === 0) super(args.nowMs);
				else super(...ctorArgs);
			}
		}
		globalThis.Date = PinnedDate;
		let uuidI = 0;
		const nextUuid = () => "uid" + String(++uuidI).padStart(5, "0");
		try {
			const agent = new Agent({
				streamFn: () => {
					throw new Error("agent-session-tools harness streamFn must never be called");
				},
				initialState: { messages: [] },
			});
			const sessionManager = SessionManager.inMemory("/proj", undefined, args.entries);
			const settingsManager = SettingsManager.inMemory({});
			const resourceLoader = new DefaultResourceLoader({
				cwd: "/proj",
				agentDir: "/proj/.pi",
				settingsManager,
				noExtensions: true,
			});
			const modelRuntime = { getPhysicalModel: () => undefined };
			const session = new AgentSession({
				agent,
				sessionManager,
				settingsManager,
				cwd: "/proj",
				resourceLoader,
				modelRuntime,
				customTools,
				...(config.usesDefaultTools !== undefined ? { usesDefaultTools: config.usesDefaultTools } : {}),
				...(config.defaultToolModifiers !== undefined
					? { defaultToolModifiers: config.defaultToolModifiers }
					: {}),
				...(config.initialActiveToolNames !== undefined
					? { initialActiveToolNames: config.initialActiveToolNames }
					: {}),
				...(config.allowedToolNames !== undefined ? { allowedToolNames: config.allowedToolNames } : {}),
				...(config.excludedToolNames !== undefined
					? { excludedToolNames: config.excludedToolNames }
					: {}),
				baseToolsOverride: baseToolsMap,
			});
			// Swap in the recording fake runner AFTER construction (the
			// constructor refreshes against a real, empty ExtensionRunner).
			const extensionErrors = [];
			let installed = false;
			const fakeRunner = {
				hasHandlers: () => false,
				emit: async () => {},
				emitMessageEnd: async () => undefined,
				emitError: (err) => {
					extensionErrors.push({
						extensionPath: err.extensionPath,
						event: err.event,
						error: err.error,
					});
				},
				invalidate: () => {},
				getAllRegisteredTools: () => (installed ? extensionToolList : []),
			};
			session._extensionRunner = fakeRunner;
			installed = true;
			globalThis.__piParityUuidOverride = nextUuid;

			const results = [];
			for (const op of args.ops) {
				if (op === null || typeof op !== "object" || Array.isArray(op)) {
					throw new Error("ops entries must be objects");
				}
				if (typeof op.op !== "string") throw new Error("missing op");
				let v = null;
				switch (op.op) {
					case "refreshRegistry": {
						const options = {};
						if (op.activeToolNames !== undefined) options.activeToolNames = op.activeToolNames;
						if (op.includeAllExtensionTools !== undefined) {
							options.includeAllExtensionTools = op.includeAllExtensionTools;
						}
						session._refreshToolRegistry(Object.keys(options).length > 0 ? options : undefined);
						break;
					}
					case "setActiveTools": {
						if (!Array.isArray(op.names)) throw new Error("missing names");
						session.setActiveToolsByName(op.names);
						break;
					}
					case "restoreFromTranscript": {
						session._restoreToolsFromTranscript(op.modifiers ?? []);
						break;
					}
					case "transformContext": {
						if (!Array.isArray(op.messageIndices)) throw new Error("missing messageIndices");
						const input = op.messageIndices.map((i) => {
							if (typeof i !== "number" || !Number.isInteger(i) || i < 0 || i >= messages.length) {
								throw new Error("invalid messageIndex");
							}
							return messages[i];
						});
						v = await session.agent.transformContext(input, undefined);
						break;
					}
					case "setForcedPrompt": {
						if (op.text === undefined) throw new Error("missing text");
						if (op.text !== null && typeof op.text !== "string") throw new Error("invalid text");
						session._runSystemPromptOptions = op.text === null ? undefined : { forceSystemPrompt: op.text };
						break;
					}
					case "getAllTools":
						v = session.getAllTools();
						break;
					case "getCallableToolNames":
						v = session.getCallableToolNames();
						break;
					default:
						throw new Error("unknown agent-session-tools op: " + op.op);
				}
				results.push(v === undefined ? null : v);
			}
			return {
				result: {
					activeTools: session.getActiveToolNames(),
					declaredTools: agent.state.tools.map((t) => ({
						name: t.name,
						description: t.description,
						parameters: t.parameters,
					})),
					hiddenDeclarations: [...session._hiddenDeclarations],
					systemPromptOptions: session._baseSystemPromptOptions,
					results,
					extensionErrors,
				},
			};
		} finally {
			globalThis.__piParityUuidOverride = null;
			Date.now = realDateNow;
			globalThis.Date = RealDate;
		}
	}
	if (tool === "agent-session-prompt") {
		// dist/core/agent-session.js — the prompt slice of the REAL
		// AgentSession: prompt, steer/followUp (_queueUserInput),
		// sendUserMessage, extension-command dispatch, skill/template
		// expansion, image normalization, and the _startRun driver, over a
		// real Agent with a scripted provider (the `agent` entry's
		// providerTurns scripting; each stream call's llm context is
		// recorded as promptsSentToAgent). Mirrors the Rust harness
		// (crates/pi-cli/src/session_prompt_harness.rs).
		//
		// Bridges (documented divergences of the Rust port): the fake
		// `emitBeforeAgentStart` returns { systemPromptOptions: clone,
		// messages: [] }; docs-path bearing auth errors are normalized to
		// "<docs>" on both sides. The real `_preparePromptAndToolLoadout`
		// runs (tranche 11) with the fake _resourceLoader's
		// `getSystemPrompt` returning `config.systemPrompt` (cases pin a
		// non-null customPrompt so sections are deterministic). The fake
		// _resourceLoader supplies the harness skills/templates (real
		// skill files are written for non-failing entries; the Rust side
		// injects the content directly).
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		const { AgentSession } = require(path.join(distDir, "core", "agent-session.js"));
		const { SessionManager } = require(path.join(distDir, "core", "session-manager.js"));
		const { SettingsManager } = require(path.join(distDir, "core", "settings-manager.js"));
		const { DefaultResourceLoader } = require(path.join(distDir, "core", "resource-loader.js"));
		const { getDocsPath } = require(path.join(distDir, "config.js"));
		const fs = require("fs");
		// Validation order mirrors the Rust entry exactly.
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		const config = args.config ?? {};
		if (typeof config !== "object" || config === null || Array.isArray(config)) {
			throw new Error("invalid config");
		}
		const settingsIn = config.settings ?? {};
		if (typeof settingsIn !== "object" || settingsIn === null || Array.isArray(settingsIn)) {
			throw new Error("invalid settings");
		}
		const retryIn = settingsIn.retry ?? {};
		if (typeof retryIn !== "object" || retryIn === null || Array.isArray(retryIn)) {
			throw new Error("invalid settings");
		}
		if (settingsIn.imageAutoResize !== undefined && typeof settingsIn.imageAutoResize !== "boolean") {
			throw new Error("invalid imageAutoResize");
		}
		const modelAuthIn = config.modelAuth ?? {};
		if (typeof modelAuthIn !== "object" || modelAuthIn === null || Array.isArray(modelAuthIn)) {
			throw new Error("invalid modelAuth");
		}
		const authMode = modelAuthIn.mode ?? "ok";
		if (authMode !== "ok" && authMode !== "noKey" && authMode !== "oauth") {
			throw new Error("invalid modelAuth");
		}
		if (config.systemPrompt !== undefined && config.systemPrompt !== null) {
			if (typeof config.systemPrompt !== "string") throw new Error("invalid systemPrompt");
		}
		if (
			!Array.isArray(config.entries) ||
			config.entries.length === 0 ||
			config.entries[0].type !== "session"
		) {
			throw new Error("missing entries");
		}
		if (config.model !== undefined && config.model !== null) {
			if (typeof config.model !== "object" || Array.isArray(config.model)) {
				throw new Error("invalid model");
			}
		}
		const modeField = (key) => {
			if (config[key] === undefined || config[key] === null) return undefined;
			if (typeof config[key] !== "string") throw new Error("invalid " + key);
			return config[key];
		};
		const steeringMode = modeField("steeringMode");
		if (steeringMode !== undefined && steeringMode !== "all" && steeringMode !== "one-at-a-time") {
			throw new Error("invalid steeringMode");
		}
		const followUpMode = modeField("followUpMode");
		if (followUpMode !== undefined && followUpMode !== "all" && followUpMode !== "one-at-a-time") {
			throw new Error("invalid followUpMode");
		}
		const commandsIn = args.commands ?? {};
		if (typeof commandsIn !== "object" || commandsIn === null || Array.isArray(commandsIn)) {
			throw new Error("invalid commands");
		}
		for (const spec of Object.values(commandsIn)) {
			if (spec === null || typeof spec !== "object" || Array.isArray(spec)) {
				throw new Error("invalid commands");
			}
			if (spec.mode !== undefined && spec.mode !== "ok" && spec.mode !== "throw") {
				throw new Error("invalid commands");
			}
		}
		let inputHandler;
		if (args.inputHandler !== undefined && args.inputHandler !== null) {
			if (typeof args.inputHandler !== "object" || Array.isArray(args.inputHandler)) {
				throw new Error("invalid inputHandler");
			}
			const mode = args.inputHandler.mode ?? "continue";
			if (mode !== "continue" && mode !== "handled" && mode !== "transform") {
				throw new Error("invalid inputHandler");
			}
			inputHandler = args.inputHandler;
		}
		const templates = args.templates ?? [];
		if (!Array.isArray(templates)) throw new Error("invalid templates");
		const skills = args.skills ?? [];
		if (!Array.isArray(skills)) throw new Error("invalid skills");
		if (!Array.isArray(args.script)) throw new Error("missing script");
		if (!Array.isArray(args.providerTurns)) throw new Error("missing providerTurns");

		const realDocsPath = getDocsPath();
		const realDateNow = Date.now;
		Date.now = () => args.nowMs;
		const RealDate = Date;
		class PinnedDate extends RealDate {
			constructor(...ctorArgs) {
				if (ctorArgs.length === 0) super(args.nowMs);
				else super(...ctorArgs);
			}
		}
		globalThis.Date = PinnedDate;
		// Real skill files for non-failing entries (the Rust side reads the
		// injected content; the file must exist for readFileSync here).
		for (const skill of skills) {
			if (skill && typeof skill === "object" && !skill.readFails && typeof skill.filePath === "string") {
				fs.mkdirSync(path.dirname(skill.filePath), { recursive: true });
				fs.writeFileSync(skill.filePath, skill.content ?? "");
			}
		}
		let uuidI = 0;
		const nextUuid = () => "uid" + String(++uuidI).padStart(5, "0");
		try {
			const sessionManager = SessionManager.inMemory("/proj", undefined, config.entries);
			const projected = sessionManager.buildSessionProjection().messages;
			const promptsSentToAgent = [];
			let providerIdx = 0;
			const fakeStreamFn = (_model, context, _options) => {
				const turn = args.providerTurns[providerIdx];
				if (turn === undefined) throw new Error("provider script exhausted");
				providerIdx++;
				promptsSentToAgent.push(JSON.parse(JSON.stringify(context.messages ?? [])));
				return {
					async *[Symbol.asyncIterator]() {
						for (const event of turn.events ?? []) yield event;
					},
					result() {
						return Promise.resolve(turn.result);
					},
				};
			};
			const agent = new Agent({
				streamFn: fakeStreamFn,
				initialState: {
					messages: projected,
					...(steeringMode !== undefined ? { steeringMode } : {}),
					...(followUpMode !== undefined ? { followUpMode } : {}),
				},
			});
			agent.state.model = config.model ?? undefined;
			const settingsManager = SettingsManager.inMemory({
				retry: retryIn,
				...(settingsIn.imageAutoResize !== undefined
					? { images: { autoResize: settingsIn.imageAutoResize } }
					: {}),
			});
			// The fake loader must be in place at construction:
			// _rebuildSystemPrompt reads getSystemPrompt/getSkills/etc.
			// while the ctor builds the initial tool loadout.
			const fakeLoader = {
				getSkills: () => ({ skills, diagnostics: [] }),
				getPrompts: () => ({ prompts: templates, diagnostics: [] }),
				getAgentsFiles: () => ({ agentsFiles: [] }),
				getSystemPrompt: () => config.systemPrompt ?? undefined,
				getAppendSystemPrompt: () => [],
				getExtensions: () => ({
					extensions: [],
					// The ctor builds a real ExtensionRunner over this
					// runtime before the fake runner replaces it; the
					// fields bindCore iterates must exist.
					runtime: {
						mcpServers: { setChangeListener: () => {} },
						pendingProviderRegistrations: [],
						pendingNativeProviderRegistrations: [],
						pendingVirtualModelRegistrations: [],
					},
					diagnostics: [],
				}),
				getThemes: () => ({ themes: [], diagnostics: [] }),
			};
			const modelRuntime = {
				getModel: () => undefined,
				getPhysicalModel: () => undefined,
				hasConfiguredAuth: () => authMode === "ok",
				checkAuth: async () => (authMode === "ok" ? {} : undefined),
				isUsingOAuth: () => authMode === "oauth",
			};
			const session = new AgentSession({
				agent,
				sessionManager,
				settingsManager,
				cwd: "/proj",
				resourceLoader: fakeLoader,
				modelRuntime,
				usesDefaultTools: false,
				// Empty base-tools override on both sides: the Rust port has
				// no createAllToolDefinitions, so without this the real dist
				// declares its builtin tools and _preparePromptAndToolLoadout
				// emits a system patch the Rust run driver skips.
				baseToolsOverride: {},
			});
			const snapEvent = (event) => JSON.parse(JSON.stringify(event));
			const extensionEvents = [];
			const extensionErrors = [];
			const commandCalls = [];
			const inputEvents = [];
			const commands = commandsIn;
			const fakeRunner = {
				hasHandlers: (event) => event === "input" && inputHandler !== undefined,
				emit: async (event) => {
					extensionEvents.push(snapEvent(event));
				},
				emitMessageEnd: async (event) => {
					extensionEvents.push(snapEvent(event));
					return undefined;
				},
				emitError: (error) => {
					extensionErrors.push(snapEvent(error));
				},
				invalidate: () => {},
				getAllRegisteredTools: () => [],
				getCommand: (name) =>
					commands[name] !== undefined
						? {
								handler: async (callArgs) => {
									commandCalls.push({ name, args: callArgs });
									if (commands[name].mode === "throw") {
										throw new Error(commands[name].errorText ?? "");
									}
								},
							}
						: undefined,
				createCommandContext: () => ({}),
				emitInput: async (text, images, source, streamingBehavior) => {
					inputEvents.push(
						snapEvent({
							type: "input",
							text,
							...(Array.isArray(images) ? { images } : {}),
							source,
							...(streamingBehavior !== undefined ? { streamingBehavior } : {}),
						}),
					);
					if (!inputHandler) return { action: "continue" };
					if (inputHandler.mode === "handled") return { action: "handled" };
					if (inputHandler.mode === "transform") {
						return {
							action: "transform",
							...(inputHandler.text !== undefined ? { text: inputHandler.text } : {}),
							...(inputHandler.images !== undefined ? { images: inputHandler.images } : {}),
						};
					}
					return { action: "continue" };
				},
				emitBeforeAgentStart: async (_trigger, options) => ({
					systemPromptOptions: structuredClone(options),
					messages: [],
				}),
			};
			session._extensionRunner = fakeRunner;

			const sessionEvents = [];
			const dispositions = [];
			session.subscribe((event) => {
				sessionEvents.push(snapEvent(event));
			});
			globalThis.__piParityUuidOverride = nextUuid;

			const normalize = (message) => String(message).split(realDocsPath).join("<docs>");
			const results = [];
			for (const op of args.script) {
				try {
					if (op === null || typeof op !== "object" || Array.isArray(op)) {
						throw new Error("ops entries must be objects");
					}
					if (typeof op.op !== "string") throw new Error("missing op");
					let v = null;
					switch (op.op) {
						case "prompt": {
							if (typeof op.text !== "string") throw new Error("missing text");
							const options = op.options ?? {};
							if (typeof options !== "object" || options === null || Array.isArray(options)) {
								throw new Error("invalid options");
							}
							if (
								options.expandPromptTemplates !== undefined &&
								typeof options.expandPromptTemplates !== "boolean"
							) {
								throw new Error("invalid expandPromptTemplates");
							}
							if (
								options.streamingBehavior !== undefined &&
								options.streamingBehavior !== null &&
								typeof options.streamingBehavior !== "string"
							) {
								throw new Error("invalid streamingBehavior");
							}
							if (options.source !== undefined && options.source !== null && typeof options.source !== "string") {
								throw new Error("invalid source");
							}
							if (options.images !== undefined && options.images !== null && !Array.isArray(options.images)) {
								throw new Error("invalid images");
							}
							const preflight =
								options.preflightTag !== undefined
									? (disposition) => {
											dispositions.push({ tag: options.preflightTag, disposition });
										}
									: undefined;
							await session.prompt(op.text, {
								...(options.expandPromptTemplates !== undefined
									? { expandPromptTemplates: options.expandPromptTemplates }
									: {}),
								...(options.images !== undefined ? { images: options.images } : {}),
								...(options.streamingBehavior !== undefined
									? { streamingBehavior: options.streamingBehavior }
									: {}),
								...(options.source !== undefined ? { source: options.source } : {}),
								...(preflight !== undefined ? { preflightResult: preflight } : {}),
							});
							break;
						}
						case "steer":
						case "followUp": {
							if (typeof op.text !== "string") throw new Error("missing text");
							if (op.images !== undefined && op.images !== null && !Array.isArray(op.images)) {
								throw new Error("invalid images");
							}
							if (op.source !== undefined && op.source !== null && typeof op.source !== "string") {
								throw new Error("invalid source");
							}
							v = await (op.op === "steer"
								? session.steer(op.text, op.images ?? undefined, op.source !== undefined ? { source: op.source } : undefined)
								: session.followUp(op.text, op.images ?? undefined, op.source !== undefined ? { source: op.source } : undefined));
							break;
						}
						case "sendCustomMessage": {
							if (typeof op.customType !== "string") throw new Error("missing customType");
							if (op.content === undefined) throw new Error("missing content");
							if (typeof op.display !== "boolean") throw new Error("missing display");
							if (op.triggerTurn !== undefined && typeof op.triggerTurn !== "boolean") {
								throw new Error("invalid triggerTurn");
							}
							if (op.deliverAs !== undefined && op.deliverAs !== null && typeof op.deliverAs !== "string") {
								throw new Error("invalid deliverAs");
							}
							await session.sendCustomMessage(
								{ customType: op.customType, content: op.content, display: op.display, details: op.details },
								{
									...(op.triggerTurn === undefined ? {} : { triggerTurn: op.triggerTurn }),
									...(op.deliverAs === undefined ? {} : { deliverAs: op.deliverAs }),
								},
							);
							break;
						}
						case "sendUserMessage": {
							if (op.content === undefined) throw new Error("missing content");
							if (typeof op.content !== "string" && !Array.isArray(op.content)) {
								throw new Error("invalid content");
							}
							if (op.deliverAs !== undefined && op.deliverAs !== null && typeof op.deliverAs !== "string") {
								throw new Error("invalid deliverAs");
							}
							if (
								op.expandPromptTemplates !== undefined &&
								typeof op.expandPromptTemplates !== "boolean"
							) {
								throw new Error("invalid expandPromptTemplates");
							}
							await session.sendUserMessage(op.content, {
								...(op.deliverAs === undefined ? {} : { deliverAs: op.deliverAs }),
								...(op.expandPromptTemplates === undefined
									? {}
									: { expandPromptTemplates: op.expandPromptTemplates }),
							});
							break;
						}
						case "setStreaming":
							if (typeof op.value !== "boolean") throw new Error("missing value");
							session._isAgentRunActive = op.value;
							break;
						default:
							throw new Error("unknown agent-session-prompt op: " + op.op);
					}
					results.push(v === undefined ? null : v);
				} catch (e) {
					results.push({ thrown: normalize(e && e.message !== undefined ? e.message : String(e)) });
				}
			}
			const queueState = {
				steering: session._steeringMessages,
				followUp: session._followUpMessages,
				isIdle: session.isIdle,
				pendingCustomMessages: session._pendingCustomMessages,
			};
			return {
				result: {
					results,
					dispositions,
					sessionEvents,
					extensionEvents,
					extensionErrors,
					commandCalls,
					inputEvents,
					queueState,
					promptsSentToAgent,
					entries: sessionManager.getEntries(),
				},
			};
		} finally {
			globalThis.__piParityUuidOverride = null;
			Date.now = realDateNow;
			globalThis.Date = RealDate;
		}
	}

	if (tool === "agent-session-run") {
		// dist/core/agent-session.js — the run orchestration slice of the
		// REAL AgentSession (_startRun/_prepareRun/_handlePostAgentRun/
		// _runBeforeSettleBoundary, boundary drafts, abort) over a real
		// Agent with a scripted provider. Mirrors the Rust harness
		// (crates/pi-cli/src/session_run_harness.rs). The fake
		// _extensionRunner's emitBoundary mirrors the real runner loop
		// (entries/continue threading, context rebuild, "Invalid boundary
		// entries:" via emitError with extensionPath "<boundary>" to match
		// the Rust library port), and emitBeforeAgentStart mirrors the real
		// loop (message collection + forceSystemPrompt on the options
		// clone) plus harness script fields (selectedTools edit,
		// setActiveTools, abort → void session.abort(), agent_settled →
		// void session.prompt).
		const { Agent } = require(path.join(distDir, "..", "..", "agent", "dist", "agent.js"));
		const { AgentSession } = require(path.join(distDir, "core", "agent-session.js"));
		const { SessionManager } = require(path.join(distDir, "core", "session-manager.js"));
		const { SettingsManager } = require(path.join(distDir, "core", "settings-manager.js"));
		if (typeof args.nowMs !== "number") throw new Error("missing nowMs");
		const config = args.config ?? {};
		if (typeof config !== "object" || config === null || Array.isArray(config)) {
			throw new Error("invalid config");
		}
		const settingsIn = config.settings ?? {};
		if (typeof settingsIn !== "object" || settingsIn === null || Array.isArray(settingsIn)) {
			throw new Error("invalid settings");
		}
		const retryIn = settingsIn.retry ?? {};
		if (typeof retryIn !== "object" || retryIn === null || Array.isArray(retryIn)) {
			throw new Error("invalid settings");
		}
		if (settingsIn.imageAutoResize !== undefined && typeof settingsIn.imageAutoResize !== "boolean") {
			throw new Error("invalid imageAutoResize");
		}
		const modelAuthIn = config.modelAuth ?? {};
		if (typeof modelAuthIn !== "object" || modelAuthIn === null || Array.isArray(modelAuthIn)) {
			throw new Error("invalid modelAuth");
		}
		const authMode = modelAuthIn.mode ?? "ok";
		if (authMode !== "ok" && authMode !== "noKey" && authMode !== "oauth") {
			throw new Error("invalid modelAuth");
		}
		if (typeof config.systemPrompt !== "string") throw new Error("invalid systemPrompt");
		if (
			!Array.isArray(config.entries) ||
			config.entries.length === 0 ||
			config.entries[0].type !== "session"
		) {
			throw new Error("missing entries");
		}
		if (config.model !== undefined && config.model !== null) {
			if (typeof config.model !== "object" || Array.isArray(config.model)) {
				throw new Error("invalid model");
			}
		}
		if (typeof args.tools !== "object" || args.tools === null || Array.isArray(args.tools)) {
			throw new Error("missing tools");
		}
		const customToolsIn = config.customTools ?? [];
		if (!Array.isArray(customToolsIn)) throw new Error("invalid customTools");
		const modelLookupIn = config.modelLookup ?? [];
		if (!Array.isArray(modelLookupIn)) throw new Error("invalid modelLookup");
		const handlersIn = args.extensionHandlers ?? {};
		if (typeof handlersIn !== "object" || handlersIn === null || Array.isArray(handlersIn)) {
			throw new Error("invalid extensionHandlers");
		}
		const handlerList = (key) => {
			const list = handlersIn[key] ?? [];
			if (!Array.isArray(list)) throw new Error("invalid " + key);
			for (const spec of list) {
				if (spec === null || typeof spec !== "object" || Array.isArray(spec)) {
					throw new Error("invalid " + key);
				}
			}
			return list;
		};
		const beforeAgentStart = handlerList("before_agent_start");
		const turnEndHandlers = handlerList("turn_end");
		const beforeSettleHandlers = handlerList("agent_before_settle");
		const agentSettled = handlersIn.agent_settled ?? null;
		if (agentSettled !== null && (typeof agentSettled !== "object" || Array.isArray(agentSettled))) {
			throw new Error("invalid agent_settled");
		}
		if (!Array.isArray(args.script)) throw new Error("missing script");
		if (!Array.isArray(args.providerTurns)) throw new Error("missing providerTurns");

		const realDateNow = Date.now;
		Date.now = () => args.nowMs;
		const RealDate = Date;
		class PinnedDate extends RealDate {
			constructor(...ctorArgs) {
				if (ctorArgs.length === 0) super(args.nowMs);
				else super(...ctorArgs);
			}
		}
		globalThis.Date = PinnedDate;
		let uuidI = 0;
		const nextUuid = () => "uid" + String(++uuidI).padStart(5, "0");
		try {
			const sessionManager = SessionManager.inMemory("/proj", undefined, config.entries);
			const projected = sessionManager.buildSessionProjection().messages;
			const promptsSentToAgent = [];
			let providerIdx = 0;
			const fakeStreamFn = (_model, context, _options) => {
				const turn = args.providerTurns[providerIdx];
				if (turn === undefined) throw new Error("provider script exhausted");
				providerIdx++;
				promptsSentToAgent.push(JSON.parse(JSON.stringify(context.messages ?? [])));
				return {
					async *[Symbol.asyncIterator]() {
						for (const event of turn.events ?? []) yield event;
					},
					result() {
						return Promise.resolve(turn.result);
					},
				};
			};
			const agent = new Agent({
				streamFn: fakeStreamFn,
				initialState: { messages: projected },
			});
			agent.state.model = config.model ?? undefined;
			const settingsManager = SettingsManager.inMemory({
				retry: retryIn,
				...(settingsIn.imageAutoResize !== undefined
					? { images: { autoResize: settingsIn.imageAutoResize } }
					: {}),
			});
			const fakeLoader = {
				getSkills: () => ({ skills: [], diagnostics: [] }),
				getPrompts: () => ({ prompts: [], diagnostics: [] }),
				getAgentsFiles: () => ({ agentsFiles: [] }),
				getSystemPrompt: () => config.systemPrompt,
				getAppendSystemPrompt: () => [],
				getExtensions: () => ({
					extensions: [],
					runtime: {
						mcpServers: { setChangeListener: () => {} },
						pendingProviderRegistrations: [],
						pendingNativeProviderRegistrations: [],
						pendingVirtualModelRegistrations: [],
					},
					diagnostics: [],
				}),
				getThemes: () => ({ themes: [], diagnostics: [] }),
			};
			const modelLookup = (provider, id) =>
				modelLookupIn.find((m) => m.provider === provider && m.id === id);
			const modelRuntime = {
				getModel: modelLookup,
				getPhysicalModel: () => undefined,
				hasConfiguredAuth: () => authMode === "ok",
				checkAuth: async () => (authMode === "ok" ? {} : undefined),
				isUsingOAuth: () => authMode === "oauth",
			};
			const baseToolsMap = {};
			for (const [key, spec] of Object.entries(args.tools)) {
				if (spec === null || typeof spec !== "object" || Array.isArray(spec)) {
					throw new Error("tools entries must be objects");
				}
				baseToolsMap[key] = {
					name: spec.name,
					label: spec.label,
					description: spec.description,
					parameters: spec.parameters ?? {},
					execute: () => {
						throw new Error("agent-session-run harness tool execute must never be called");
					},
				};
			}
			const customTools = customToolsIn.map((d) => ({ ...d, execute: () => {} }));
			const session = new AgentSession({
				agent,
				sessionManager,
				settingsManager,
				cwd: "/proj",
				resourceLoader: fakeLoader,
				modelRuntime,
				customTools,
				usesDefaultTools: false,
				baseToolsOverride: baseToolsMap,
			});
			const snapEvent = (event) => JSON.parse(JSON.stringify(event));
			const extensionEvents = [];
			const extensionErrors = [];
			let settledFired = false;
			const fakeRunner = {
				hasHandlers: (event) =>
					event === "turn_end"
						? turnEndHandlers.length > 0
						: event === "agent_before_settle"
							? beforeSettleHandlers.length > 0
							: false,
				emit: async (event) => {
					extensionEvents.push(snapEvent(event));
					if (event.type === "agent_settled" && !settledFired) {
						settledFired = true;
						if (agentSettled && typeof agentSettled.prompt === "string") {
							void session.prompt(agentSettled.prompt);
						}
					}
				},
				emitMessageEnd: async (event) => {
					extensionEvents.push(snapEvent(event));
					return undefined;
				},
				emitError: (error) => {
					extensionErrors.push({
						extensionPath: error.extensionPath,
						event: error.event,
						error: error.error,
					});
				},
				invalidate: () => {},
				getAllRegisteredTools: () => [],
				boundaryHandlerCount: (type) =>
					type === "turn_end" ? turnEndHandlers.length : beforeSettleHandlers.length,
				emitBoundary: async (baseEvent, buildContext) => {
					const script =
						baseEvent.type === "turn_end" ? turnEndHandlers : beforeSettleHandlers;
					// `once: true` specs model a stateful handler that only
					// acts on the first boundary emission (a real extension
					// can keep state; the static spec list cannot).
					const calls = (boundaryCalls[baseEvent.type] = (boundaryCalls[baseEvent.type] ?? 0) + 1);
					let entries = [];
					let shouldContinue = false;
					let context = await buildContext(entries);
					let valid = true;
					for (const spec of script) {
						if (spec.once === true && calls > 1) continue;
						const event = { ...baseEvent, entries, continue: shouldContinue, context };
						extensionEvents.push(snapEvent(event));
						if (spec.abort === true) void session.abort();
						if (spec.entries !== undefined) entries = spec.entries;
						if (spec.continue !== undefined) shouldContinue = spec.continue;
						try {
							context = await buildContext(entries);
							valid = true;
						} catch (err) {
							valid = false;
							fakeRunner.emitError({
								extensionPath: "<boundary>",
								event: baseEvent.type,
								error: `Invalid boundary entries: ${err instanceof Error ? err.message : String(err)}`,
							});
						}
					}
					return valid
						? { entries, continue: shouldContinue, context, valid: true }
						: { entries: [], continue: false, context, valid: false };
				},
				emitBeforeAgentStart: async (trigger, options) => {
					const current = JSON.parse(JSON.stringify(options));
					const messages = [];
					const requests = [];
					for (const spec of beforeAgentStart) {
						const event = { ...trigger, type: "before_agent_start", systemPromptOptions: current };
						extensionEvents.push(snapEvent(event));
						if (spec.abort === true) void session.abort();
						if (Array.isArray(spec.setActiveTools)) requests.push(spec.setActiveTools);
						if (spec.selectedTools !== undefined) current.selectedTools = spec.selectedTools;
						if (spec.systemPrompt !== undefined) current.forceSystemPrompt = spec.systemPrompt;
						if (spec.message !== undefined) messages.push(spec.message);
					}
					for (const names of requests) session.setActiveToolsByName(names);
					return { messages, systemPromptOptions: current };
				},
			};
			session._extensionRunner = fakeRunner;

			const boundaryCalls = {};
			const sessionEvents = [];
			const idleTimeline = [];
			session.subscribe((event) => {
				if (event.type === "agent_settled") {
					idleTimeline.push({ kind: "settled", aborted: event.aborted });
				}
				sessionEvents.push(snapEvent(event));
			});
			globalThis.__piParityUuidOverride = nextUuid;

			const results = [];
			for (const op of args.script) {
				try {
					if (op === null || typeof op !== "object" || Array.isArray(op)) {
						throw new Error("ops entries must be objects");
					}
					if (typeof op.op !== "string") throw new Error("missing op");
					let v = null;
					switch (op.op) {
						case "prompt": {
							if (typeof op.text !== "string") throw new Error("missing text");
							const options = op.options ?? {};
							if (typeof options !== "object" || options === null || Array.isArray(options)) {
								throw new Error("invalid options");
							}
							if (
								options.expandPromptTemplates !== undefined &&
								typeof options.expandPromptTemplates !== "boolean"
							) {
								throw new Error("invalid expandPromptTemplates");
							}
							if (options.source !== undefined && options.source !== null && typeof options.source !== "string") {
								throw new Error("invalid source");
							}
							if (options.images !== undefined && options.images !== null && !Array.isArray(options.images)) {
								throw new Error("invalid images");
							}
							await session.prompt(op.text, {
								...(options.expandPromptTemplates !== undefined
									? { expandPromptTemplates: options.expandPromptTemplates }
									: {}),
								...(options.images !== undefined ? { images: options.images } : {}),
								...(options.streamingBehavior !== undefined
									? { streamingBehavior: options.streamingBehavior }
									: {}),
								...(options.source !== undefined ? { source: options.source } : {}),
							});
							break;
						}
						case "abort":
							await session.abort();
							break;
						case "waitForIdle":
							await session.waitForIdle();
							idleTimeline.push({ kind: "waitIdle" });
							break;
						default:
							throw new Error("unknown agent-session-run op: " + op.op);
					}
					results.push(v === undefined ? null : v);
				} catch (e) {
					results.push({ thrown: e && e.message !== undefined ? e.message : String(e) });
				}
			}
			return {
				result: {
					results,
					sessionEvents,
					entries: sessionManager.getEntries(),
					idleTimeline,
					extensionEvents,
					extensionErrors,
					promptsSentToAgent,
				},
			};
		} finally {
			globalThis.__piParityUuidOverride = null;
			Date.now = realDateNow;
			globalThis.Date = RealDate;
		}
	}

	if (tool === "file-mutation-queue") {
		// dist/core/tools/file-mutation-queue.js — withFileMutationQueue over
		// scripted fns. Ops run concurrently; each fn marks itself started,
		// then walks its script (label → log, awaitGate → spin on microtasks
		// until the release rule fires, throw → fn rejection). Release rules
		// are applied in order once all their afterStarted ops have begun.
		// Mirrors the Rust harness entry (run_file_mutation_queue_harness).
		const { withFileMutationQueue } = require(path.join(distDir, "core", "tools", "file-mutation-queue.js"));
		const fs = require("fs");
		const setup = args.setup ?? {};
		for (const file of setup.realFiles ?? []) {
			const target = path.resolve(cwd, file.path);
			fs.mkdirSync(path.dirname(target), { recursive: true });
			fs.writeFileSync(target, file.content ?? "");
		}
		for (const link of setup.symlinks ?? []) {
			{
				const target = path.resolve(cwd, link.link);
				fs.rmSync(target, { force: true, recursive: true });
				fs.symlinkSync(link.target, target);
			}
		}
		if (!Array.isArray(args.ops)) throw new Error("missing ops");
		const log = [];
		const gates = {};
		const startedResolvers = [];
		const startedPromises = args.ops.map((_, i) => new Promise((r) => (startedResolvers[i] = r)));
		const outcomes = new Array(args.ops.length).fill("pending");
		const runOp = async (i) => {
			const op = args.ops[i];
			try {
				await withFileMutationQueue(path.resolve(cwd, op.path), async () => {
					startedResolvers[i]();
					for (const step of op.script ?? []) {
						if (typeof step === "string") {
							log.push(step);
						} else if (typeof step.awaitGate === "string") {
							// setImmediate (not Promise.resolve): the spin must
							// yield through the macrotask queue so pending
							// realpath I/O in later registrations can complete;
							// a microtask spin would starve the event loop and
							// deadlock the release rule.
							while (!gates[step.awaitGate]) await new Promise((r) => setImmediate(r));
						} else if (typeof step.throw === "string") {
							throw new Error(step.throw);
						}
					}
				});
				outcomes[i] = "ok";
			} catch (e) {
				outcomes[i] = "error:" + (e && e.message !== undefined ? e.message : String(e));
			}
		};
		const master = (async () => {
			for (const rule of args.release ?? []) {
				await Promise.all((rule.afterStarted ?? []).map((i) => startedPromises[i]));
				gates[rule.gate] = true;
			}
		})();
		await Promise.all([...args.ops.map((_, i) => runOp(i)), master]);
		return { result: { log, outcomes } };
	}

	return undefined;
}

/** sess-manager normalization: <base>, uuids, ISO timestamps (plain and
 * filename form), exact-8-hex ids, and epoch-ms numbers under `timestamp`. */
function sessNorm(value, base) {
	if (typeof value === "string") {
		// Ambient cwd: `open` without cwdOverride falls back to process.cwd()
		// here and AMBIENT_CWD ("/") on the Rust side; each side normalizes
		// its own ambient value to <cwd>.
		let s = value.split(process.cwd()).join("<cwd>");
		s = s.split(base).join("<base>");
		// getDefaultSessionDir encodes the cwd into a safe directory name
		// (slashes → dashes), which escapes the plain <base> rewrite.
		s = s.split(base.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")).join("<base>");
		s = s.replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, "<uuid>");
		s = s.replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z/g, "<ts>");
		s = s.replace(/\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z/g, "<tsf>");
		// Quoted 8-hex ids embedded in raw JSONL strings (readFile): quote
		// delimiters cannot occur inside JSON string content, so this only
		// matches structural values. Mirrors the Rust sess_rewrite_string arm.
		s = s.replace(/"([0-9a-f]{8})"/g, '"<id8>"');
		if (/^[0-9a-f]{8}$/.test(s)) s = "<id8>";
		return s;
	}
	if (Array.isArray(value)) return value.map((v) => sessNorm(v, base));
	if (value !== null && typeof value === "object") {
		const out = {};
		for (const [k, v] of Object.entries(value)) {
			out[k] = k === "timestamp" && typeof v === "number" && v >= 1e12 && v < 1e13 ? "<ms>" : sessNorm(v, base);
		}
		return out;
	}
	return value;
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

/** Fixture helpers shared by the prompt-templates / skills entries. */
function parityValidateFiles(files) {
	if (files === undefined || files === null) return {};
	if (typeof files !== "object" || Array.isArray(files)) throw new Error("files must be an object");
	for (const v of Object.values(files)) {
		const ok = typeof v === "string" ||
			(v !== null && typeof v === "object" && !Array.isArray(v) &&
				Object.keys(v).length === 1 && typeof v.symlink === "string");
		if (!ok) throw new Error("files values must be a string or {symlink}");
	}
	return files;
}

function parityMaterialize(files, base) {
	const fs = require("fs");
	const path = require("path");
	for (const [rel, v] of Object.entries(files)) {
		const target = path.join(base, rel);
		fs.mkdirSync(path.dirname(target), { recursive: true });
		if (typeof v === "string") fs.writeFileSync(target, v);
		else fs.symlinkSync(v.symlink, target);
	}
}

function parityJoinRel(base, rel) {
	const trimmed = String(rel).replace(/^\/+/, "");
	if (trimmed === "" || trimmed === ".") return base;
	return base.replace(/\/+$/, "") + "/" + trimmed;
}

function parityNormalizeBase(value, base) {
	if (typeof value === "string") return value.split(base).join("<base>");
	if (Array.isArray(value)) return value.map((v) => parityNormalizeBase(v, base));
	if (value !== null && typeof value === "object") {
		const out = {};
		for (const [k, v] of Object.entries(value)) out[k] = parityNormalizeBase(v, base);
		return out;
	}
	return value;
}

/** Array.sort comparator over two string keys (missing → ""). */
function parityCmp2(a, b) {
	const cmp = (x, y) => (x < y ? -1 : x > y ? 1 : 0);
	return (x, y) => cmp(x[a] ?? "", y[a] ?? "") || cmp(x[b] ?? "", y[b] ?? "");
}

function parityOptString(obj, key, err) {
	const v = obj[key];
	if (v === undefined || v === null) return undefined;
	if (typeof v !== "string") throw new Error(err);
	return v;
}

function parityOptStrings(obj, key, err) {
	const v = obj[key];
	if (v === undefined || v === null) return undefined;
	if (!Array.isArray(v) || v.some((x) => typeof x !== "string")) throw new Error(err);
	return v;
}

function parityOptBool(obj, key, err) {
	const v = obj[key];
	if (v === undefined || v === null) return undefined;
	if (typeof v !== "boolean") throw new Error(err);
	return v;
}

/** JSON deep equality (object key order insensitive), mirroring the
 * serde_json Value equality of the Rust ScriptedTool pair match. Shared by
 * the run-tool-call and agent-loop scripted-tool entries. */
function parityCanonJson(v) {
	if (v === null || typeof v !== "object") return v;
	if (Array.isArray(v)) return v.map(parityCanonJson);
	const o = {};
	for (const k of Object.keys(v).sort()) o[k] = parityCanonJson(v[k]);
	return o;
}

function parityJsonEq(a, b) {
	return JSON.stringify(parityCanonJson(a)) === JSON.stringify(parityCanonJson(b));
}

/** JSON spec → AgentToolResult, mirroring the Rust parse_tool_result:
 * content defaults to [], details is always present (null when absent),
 * structuredContent/usage drop null, isError/terminate keep booleans only.
 * Shared by the run-tool-call and agent-loop entries. */
function parityParseToolResult(value) {
	const v = value === undefined || value === null ? {} : value;
	const out = {
		content: Array.isArray(v.content) ? v.content : [],
		details: v.details === undefined || v.details === null ? null : v.details,
	};
	if (v.structuredContent !== undefined && v.structuredContent !== null) out.structuredContent = v.structuredContent;
	if (v.usage !== undefined && v.usage !== null) out.usage = v.usage;
	if (typeof v.isError === "boolean") out.isError = v.isError;
	if (typeof v.terminate === "boolean") out.terminate = v.terminate;
	return out;
}

/** beforeToolCall hook shim mirroring the Rust parse_before: a "throw"
 * string rejects; block/reason/terminate keep only their well-typed values
 * (null is treated as absent, exactly like the Rust filters). */
function parityMakeBefore(rawSpec) {
	const spec = rawSpec === undefined || rawSpec === null ? {} : rawSpec;
	return async () => {
		if (typeof spec.throw === "string") throw new Error(spec.throw);
		const out = {};
		if (typeof spec.block === "boolean") out.block = spec.block;
		if (typeof spec.reason === "string") out.reason = spec.reason;
		if (typeof spec.terminate === "boolean") out.terminate = spec.terminate;
		return out;
	};
}

/** afterToolCall hook shim mirroring the Rust parse_after (same value
 * filters as parityMakeBefore plus the result-override fields). */
function parityMakeAfter(rawSpec) {
	const spec = rawSpec === undefined || rawSpec === null ? {} : rawSpec;
	return async () => {
		if (typeof spec.throw === "string") throw new Error(spec.throw);
		const out = {};
		if (spec.content !== undefined && spec.content !== null) out.content = Array.isArray(spec.content) ? spec.content : [];
		if (spec.details !== undefined && spec.details !== null) out.details = spec.details;
		if (spec.structuredContent !== undefined && spec.structuredContent !== null) out.structuredContent = spec.structuredContent;
		if (spec.usage !== undefined && spec.usage !== null) out.usage = spec.usage;
		if (typeof spec.isError === "boolean") out.isError = spec.isError;
		if (typeof spec.terminate === "boolean") out.terminate = spec.terminate;
		return out;
	};
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
