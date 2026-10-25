/**
 * Parity test cases.
 *
 * Each case runs against a fresh fixture directory that is set up identically
 * for both the original TypeScript implementation and the Rust port. The tool
 * result JSON and the resulting directory contents are compared.
 *
 * `setup` maps relative file paths to string contents.
 */

const nLines = (n, prefix = "line") =>
	Array.from({ length: n }, (_, i) => `${prefix} ${i + 1}`).join("\n") + "\n";

/** Base64 of a string, so chunk fixtures stay text-only. */
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

/** Base64 of a byte range of a string's UTF-8 encoding, for splitting
 * multi-byte characters across chunks. */
const b64range = (s, from, to) => Buffer.from(s, "utf8").subarray(from, to).toString("base64");

/** Session-manager fixtures: stable ISO timestamps and entry builders.
 * Entry ids deliberately avoid the 8-hex-char shape that the v1→v2
 * migration generates, so id normalization never rewrites hand-written
 * fixture ids. */
const smTs = (n) => `2026-01-01T00:00:${String(n).padStart(2, "0")}.000Z`;
const smMsg = (id, parentId, message, n) => ({ type: "message", id, parentId, timestamp: smTs(n), message });
const smUser = (id, parentId, text, n) =>
	smMsg(id, parentId, { role: "user", content: [{ type: "text", text }], timestamp: smTs(n) }, n);
const smAssistant = (id, parentId, text, n) =>
	smMsg(id, parentId, { role: "assistant", content: [{ type: "text", text }], provider: "anthropic", model: "claude-sonnet-4", timestamp: smTs(n) }, n);
const smSystem = (id, parentId, text, n) =>
	smMsg(id, parentId, { role: "system", content: text, timestamp: smTs(n) }, n);
const smHeader = (version) => ({
	type: "session",
	version,
	id: "01920000-0000-7000-8000-000000000001",
	timestamp: smTs(0),
	cwd: "/home/user/proj",
});

	const kc = (input, kitty = false, keyIds) => ({
		input,
		kitty,
		...(keyIds ? { keyIds } : {}),
	});

	/** tui-stdin-buffer sub-case builder (see the tui-stdin-buffer section). */
	const sb = (name, chunks, opts = {}) => ({
		name,
		kitty: opts.kitty ?? false,
		...(opts.timeoutMs !== undefined ? { timeoutMs: opts.timeoutMs } : {}),
		...(opts.escapeTimeoutMs !== undefined ? { escapeTimeoutMs: opts.escapeTimeoutMs } : {}),
		script: {
			chunks,
			...(opts.fireAfterEach ? { fireTimeoutAfterEachChunk: true } : {}),
			...(opts.fireAt ? { fireTimeoutAt: opts.fireAt } : {}),
		},
	});
	/** Every 2-chunk split of s: [s.slice(0,i), s.slice(i)] for i in 1..len-1. */
	const sbSplits = (s, opts, prefix) =>
		Array.from({ length: s.length - 1 }, (_, i) =>
			sb(`${prefix}-split-${i + 1}`, [s.slice(0, i + 1), s.slice(i + 1)], opts),
		);

import { mainCases } from "./wavec/main-cases.mjs";
import { altCases } from "./wavec/alt-cases.mjs";
import { interactionCases } from "./wavec/interaction-cases.mjs";
import { terminalImageCases } from "./waved/terminal-image-cases.mjs";
import { markdownLexCases } from "./waved/markdown-lex-cases.mjs";
// Ground-truth TS defaults, reused by the settings-schema valid-defaults
// case (stays in sync with the TS tree automatically). PI_REPO defaults to
// the persistent reference checkout (the /tmp default predates the bwrap
// sandbox, which gives every bash call a fresh /tmp).
import { pathToFileURL } from "node:url";
const PI_REPO = process.env.PI_REPO || "/home/shinobi/AI/pi-repo";
const { SETTINGS_DEFAULTS } = await import(
	pathToFileURL(PI_REPO + "/packages/coding-agent/dist/core/settings-defaults.js").href
);
import { latexCases } from "./waved/latex-cases.mjs";
import { markdownRenderCases } from "./waved/markdown-render-cases.mjs";
import { regressionCases } from "./regressions/index.mjs";

const astBase = (extra = {}) => ({
	nowMs: 1747000000000,
	baseTools: {
		read: { name: "read", description: "Read files", parameters: { type: "object" } },
		bash: { name: "bash", description: "Run commands", parameters: { type: "object" } },
	},
	entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
	...extra,
});

const asp = (name, args) => ({
	name: "agent-session-prompt/" + name,
	tool: "agent-session-prompt",
	args: {
		...args,
		// Every asp case pins a custom system prompt: the real
		// _preparePromptAndToolLoadout runs on both sides (tranche 11),
		// and a non-null customPrompt makes the system-patch sections
		// deterministic (the default preamble/docs sections embed host
		// paths).
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASP_NOW = 1747000000000;
const aspHeader = () => [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }];
const aspModel = (provider = "unknown") => ({
	id: "m1",
	name: "Model One",
	api: "unknown",
	provider,
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 0,
	maxTokens: 0,
});
const aspMsg = (o = {}) => ({
	role: "assistant",
	content: o.content ?? [{ type: "text", text: "done" }],
	api: "unknown",
	provider: o.provider ?? "unknown",
	model: o.model ?? "m1",
	usage: {
		input: 3,
		output: 5,
		cacheRead: 0,
		cacheWrite: 0,
		totalTokens: 8,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	},
	stopReason: o.stopReason ?? "stop",
	...(o.timestamp !== undefined ? { timestamp: o.timestamp } : {}),
	...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
});
const aspTurn = (result, events = []) => ({ events, result });
// agent-session-run: forces the required systemPrompt (and defaults the
// required tools object) so every case is a valid harness invocation.
const asr = (name, args) => ({
	name: "agent-session-run/" + name,
	tool: "agent-session-run",
	args: {
		tools: {},
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});

export const cases = [
	// ---------------------------------------------------------------- read
	{ name: "read/missing", tool: "read", args: { path: "nope.txt" } },
	{
		name: "read/basic",
		tool: "read",
		args: { path: "ten.txt" },
		setup: { "ten.txt": nLines(10) },
	},
	{
		name: "read/offset-limit",
		tool: "read",
		args: { path: "ten.txt", offset: 3, limit: 4 },
		setup: { "ten.txt": nLines(10) },
	},
	{
		name: "read/limit",
		tool: "read",
		args: { path: "ten.txt", limit: 3 },
		setup: { "ten.txt": nLines(10) },
	},
	{
		name: "read/offset-past-eof",
		tool: "read",
		args: { path: "ten.txt", offset: 100 },
		setup: { "ten.txt": nLines(10) },
	},
	{
		name: "read/truncated-lines",
		tool: "read",
		args: { path: "big.txt" },
		setup: { "big.txt": nLines(3000) },
	},
	{
		name: "read/crlf",
		tool: "read",
		args: { path: "crlf.txt" },
		setup: { "crlf.txt": "one\r\ntwo\r\nthree\r\n" },
	},
	{
		name: "read/bom",
		tool: "read",
		args: { path: "bom.txt" },
		setup: { "bom.txt": "﻿hello\nworld\n" },
	},
	{
		name: "read/unicode",
		tool: "read",
		args: { path: "uni.txt" },
		setup: { "uni.txt": "héllo wörld 🌍\n日本語テキスト\nemoji 👨‍👩‍👧‍👦 family\n" },
	},
	{
		name: "read/empty",
		tool: "read",
		args: { path: "empty.txt" },
		setup: { "empty.txt": "" },
	},
	{
		name: "read/no-trailing-newline",
		tool: "read",
		args: { path: "nonl.txt" },
		setup: { "nonl.txt": "alpha\nbeta\ngamma" },
	},
	{
		name: "read/long-line",
		tool: "read",
		args: { path: "long.txt" },
		setup: { "long.txt": "short\n" + "x".repeat(600) + "\nshort\n" },
	},
	{
		name: "read/byte-truncation",
		tool: "read",
		args: { path: "wide.txt" },
		setup: { "wide.txt": ("𝕏".repeat(50) + "\n").repeat(100) },
	},

	// --------------------------------------------------------------- write
	{
		name: "write/new-nested",
		tool: "write",
		args: { path: "a/b/c.txt", content: "created\n" },
	},
	{
		name: "write/overwrite",
		tool: "write",
		args: { path: "exists.txt", content: "replaced content\n" },
		setup: { "exists.txt": "original\n" },
	},
	{
		name: "write/empty",
		tool: "write",
		args: { path: "empty.txt", content: "" },
	},
	{
		name: "write/crlf-content",
		tool: "write",
		args: { path: "crlf.txt", content: "one\r\ntwo\r\n" },
	},
	{
		name: "write/unicode",
		tool: "write",
		args: { path: "uni.txt", content: "héllo 🌍\n" },
	},

	// ---------------------------------------------------------------- edit
	{
		name: "edit/basic",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "function foo() {\n\treturn 1;\n}", newText: "function foo() {\n\treturn 2;\n}" }],
		},
		setup: { "file.txt": "header\nfunction foo() {\n\treturn 1;\n}\nfooter\n" },
	},
	{
		name: "edit/not-found",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "absent", newText: "present" }],
		},
		setup: { "file.txt": "content\n" },
	},
	{
		name: "edit/multiple-disjoint",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [
				{ oldText: "alpha", newText: "ALPHA" },
				{ oldText: "omega", newText: "OMEGA" },
			],
		},
		setup: { "file.txt": "alpha\nmiddle\nomega\n" },
	},
	{
		name: "edit/multiple-same-block",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [
				{ oldText: "same", newText: "first" },
				{ oldText: "same", newText: "second" },
			],
		},
		setup: { "file.txt": "same\nsame\n" },
	},
	{
		name: "edit/fuzzy-trailing-whitespace",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "change me \n", newText: "changed\n" }],
		},
		setup: { "file.txt": "keep\nchange me  \nkeep\n" },
	},
	{
		name: "edit/fuzzy-smart-quotes",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "it's a “test” — really", newText: "it is a \"test\" - really" }],
		},
		setup: { "file.txt": "start\nit’s a “test” — really\nend\n" },
	},
	{
		name: "edit/crlf",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "two", newText: "TWO" }],
		},
		setup: { "file.txt": "one\r\ntwo\r\nthree\r\n" },
	},
	{
		name: "edit/bom",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "hello", newText: "goodbye" }],
		},
		setup: { "file.txt": "﻿hello world\n" },
	},
	{
		name: "edit/empty-old-text",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "", newText: "x" }],
		},
		setup: { "file.txt": "content\n" },
	},
	{
		name: "edit/empty-new-text-delete",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "middle\n", newText: "" }],
		},
		setup: { "file.txt": "top\nmiddle\nbottom\n" },
	},
	{
		name: "edit/duplicate-error",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [
				{ oldText: "dup", newText: "a" },
				{ oldText: "dup", newText: "b" },
			],
		},
		setup: { "file.txt": "dup\n" },
	},
	{
		name: "edit/overlap-error",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [
				{ oldText: "abc", newText: "x" },
				{ oldText: "bcd", newText: "y" },
			],
		},
		setup: { "file.txt": "abcd\n" },
	},
	{
		name: "edit/no-change",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: [{ oldText: "same", newText: "same" }],
		},
		setup: { "file.txt": "same\n" },
	},
	{
		name: "edit/missing-file",
		tool: "edit",
		args: {
			path: "nope.txt",
			edits: [{ oldText: "a", newText: "b" }],
		},
	},
	{
		name: "edit/edits-as-json-string",
		tool: "edit",
		args: {
			path: "file.txt",
			edits: JSON.stringify([{ oldText: "alpha", newText: "beta" }]),
		},
		setup: { "file.txt": "alpha\n" },
	},
	{
		name: "edit/legacy-old-text-new-text",
		tool: "edit",
		args: { path: "file.txt", oldText: "alpha", newText: "beta" },
		setup: { "file.txt": "alpha\n" },
	},

	// ------------------------------------------------------------------ ls
	{
		name: "ls/basic",
		tool: "ls",
		args: {},
		setup: {
			"zebra.txt": "",
			"apple.md": "",
			"Banana.ts": "",
			"sub-dir/inner.txt": "",
			".hidden": "",
		},
	},
	{
		name: "ls/empty",
		tool: "ls",
		args: {},
		setup: { "placeholder/.keep": "" },
	},
	{
		name: "ls/missing",
		tool: "ls",
		args: { path: "nope" },
	},
	{
		name: "ls/nested-path",
		tool: "ls",
		args: { path: "src" },
		setup: { "src/main.rs": "", "src/lib.rs": "", "src/bin/tool.rs": "" },
	},
	{
		name: "ls/limit",
		tool: "ls",
		args: { limit: 10 },
		setup: Object.fromEntries(
			Array.from({ length: 25 }, (_, i) => [`f${String(i).padStart(2, "0")}.txt`, ""]),
		),
	},

	// --------------------------------------------------------------- grep
	{
		name: "grep/basic",
		tool: "grep",
		sortLines: true, // rg's multi-file output order is nondeterministic
		args: { pattern: "needle" },
		setup: {
			"a.txt": "haystack\nneedle here\nmore hay\n",
			"b.md": "nothing\n",
			"sub/c.txt": "another needle\n",
		},
	},
	{
		name: "grep/ignore-case",
		tool: "grep",
		args: { pattern: "NEEDLE", ignoreCase: true },
		setup: { "a.txt": "needle\nNeedle\nNEEDLE\nplain\n" },
	},
	{
		name: "grep/literal-regex-chars",
		tool: "grep",
		args: { pattern: "a.c", literal: true },
		setup: { "a.txt": "a.c\nabc\naxc\n" },
	},
	{
		name: "grep/regex",
		tool: "grep",
		args: { pattern: "colou?r" },
		setup: { "a.txt": "color\ncolour\ncolouur\n" },
	},
	{
		name: "grep/glob-filter",
		tool: "grep",
		args: { pattern: "needle", glob: "*.ts" },
		setup: { "a.ts": "needle\n", "b.txt": "needle\n" },
	},
	{
		name: "grep/context",
		tool: "grep",
		args: { pattern: "needle", context: 1 },
		setup: { "a.txt": "one\ntwo\nneedle\nthree\nfour\nneedle\nfive\nsix\n" },
	},
	{
		name: "grep/no-match",
		tool: "grep",
		args: { pattern: "zzzz" },
		setup: { "a.txt": "content\n" },
	},
	{
		name: "grep/limit",
		tool: "grep",
		args: { pattern: "match", limit: 3 },
		setup: { "a.txt": Array.from({ length: 20 }, (_, i) => `match ${i}`).join("\n") + "\n" },
	},
	{
		name: "grep/single-file-path",
		tool: "grep",
		args: { pattern: "needle", path: "a.txt" },
		setup: { "a.txt": "needle\n", "b.txt": "needle\n" },
	},
	{
		name: "grep/missing-path",
		tool: "grep",
		args: { pattern: "x", path: "nope" },
	},

	// --------------------------------------------------------------- find
	{
		name: "find/basic",
		tool: "find",
		args: { pattern: "*.txt" },
		setup: { "a.txt": "", "sub/b.txt": "", "c.md": "" },
	},
	{
		name: "find/path-pattern",
		tool: "find",
		args: { pattern: "src/**/*.rs" },
		setup: { "src/main.rs": "", "src/core/lib.rs": "", "tests/x.rs": "" },
	},
	{
		name: "find/no-results",
		tool: "find",
		args: { pattern: "*.xyz" },
		setup: { "a.txt": "" },
	},
	{
		name: "find/limit",
		tool: "find",
		// fd --max-results returns an arbitrary subset of the matches (its
		// parallel walk stops wherever it happens to be), so the case uses
		// exactly `limit` matching files to stay deterministic.
		args: { pattern: "hit*.txt", limit: 3 },
		setup: {
			"hit0.txt": "",
			"hit1.txt": "",
			"hit2.txt": "",
			"other0.txt": "",
			"other1.txt": "",
		},
	},
	{
		name: "find/subdir-path",
		tool: "find",
		args: { pattern: "*.txt", path: "sub" },
		setup: { "sub/a.txt": "", "top.txt": "" },
	},

	// --------------------------------------------------------------- bash
	{
		name: "bash/echo",
		tool: "bash",
		args: { command: "echo hello" },
	},
	{
		name: "bash/stderr",
		tool: "bash",
		args: { command: "echo oops 1>&2" },
	},
	{
		name: "bash/exit-code",
		tool: "bash",
		args: { command: "echo before; exit 3" },
	},
	{
		name: "bash/no-output",
		tool: "bash",
		args: { command: "true" },
	},
	{
		name: "bash/combined-order",
		tool: "bash",
		// stdout and stderr are two pipes merged in arrival order on both
		// sides; the relative order of writes to different pipes is not
		// guaranteed, so compare as a set.
		sortLines: true,
		args: { command: "echo out1; echo err1 1>&2; echo out2; echo err2 1>&2" },
	},
	{
		name: "bash/exit-no-output",
		tool: "bash",
		args: { command: "exit 1" },
	},
	{
		name: "bash/writes-file",
		tool: "bash",
		args: { command: "printf 'written\\n' > out.txt" },
	},
	{
		name: "bash/large-output-truncation",
		tool: "bash",
		args: { command: "seq 1 3000" },
	},
	{
		name: "bash/timeout",
		tool: "bash",
		args: { command: "echo started; sleep 30; echo done", timeout: 1 },
	},
	{
		name: "bash/unicode-output",
		tool: "bash",
		args: { command: "printf 'h\\xc3\\xa9llo \\xf0\\x9f\\x8c\\x8d\\n'" },
	},

	// ------------------------------------------------------- read (images)
	// Image fixtures live in parity/fixtures/; processing runs through the
	// photon WASM pipeline on both sides (see PARITY.md).
	{
		name: "read/image-png-passthrough",
		tool: "read",
		args: { path: "small.png" },
		setup: { "small.png": { fixture: "small.png" } },
	},
	{
		name: "read/image-png-resize",
		tool: "read",
		args: { path: "resize.png" },
		setup: { "resize.png": { fixture: "resize.png" } },
	},
	{
		name: "read/image-jpeg-resize",
		tool: "read",
		args: { path: "tall.jpg" },
		setup: { "tall.jpg": { fixture: "tall.jpg" } },
	},
	{
		name: "read/image-bmp-convert",
		tool: "read",
		args: { path: "img.bmp" },
		setup: { "img.bmp": { fixture: "img.bmp" } },
	},
	{
		name: "read/image-gif-passthrough",
		tool: "read",
		args: { path: "small.gif" },
		setup: { "small.gif": { fixture: "small.gif" } },
	},
	{
		name: "read/image-webp-passthrough",
		tool: "read",
		args: { path: "small.webp" },
		setup: { "small.webp": { fixture: "small.webp" } },
	},
	{
		name: "read/image-corrupt-png",
		tool: "read",
		args: { path: "corrupt.png" },
		setup: { "corrupt.png": { fixture: "corrupt.png" } },
	},
	{
		name: "read/image-exif-orientation",
		tool: "read",
		args: { path: "exif6.jpg" },
		setup: { "exif6.jpg": { fixture: "exif6.jpg" } },
	},
	{
		name: "read/image-animated-png-text-path",
		tool: "read",
		args: { path: "animated.png" },
		setup: { "animated.png": { fixture: "animated.png" } },
	},
	// ------------------------------------------------- bash-executor (module)
	{ name: "bash-executor/echo", tool: "bash-executor", args: { command: "echo hello" } },
	{
		name: "bash-executor/exit-code",
		tool: "bash-executor",
		args: { command: "echo out; exit 3" },
	},
	{
		name: "bash-executor/stderr",
		tool: "bash-executor",
		args: { command: "echo err >&2" },
	},
	{
		name: "bash-executor/ansi-strip",
		tool: "bash-executor",
		args: { command: "printf '\\033[31mred\\033[0m plain\\n'" },
	},
	{
		name: "bash-executor/carriage-return",
		tool: "bash-executor",
		args: { command: "printf 'a\\r\\nb\\r\\n'" },
	},
	{
		name: "bash-executor/utf8",
		tool: "bash-executor",
		args: { command: "printf 'h\\xc3\\xa9llo w\\xc3\\xb6rld\\n'" },
	},
	{
		name: "bash-executor/cwd",
		tool: "bash-executor",
		args: { command: "echo written > wrote.txt && cat wrote.txt" },
	},
	{
		name: "bash-executor/big-output",
		tool: "bash-executor",
		args: { command: "seq 1 20000" },
	},

	// --------------------------------------------- output-accumulator (module)
	{
		name: "output-accumulator/basic",
		tool: "output-accumulator",
		args: { chunks: [b64("hello\nworld\n")] },
	},
	{
		name: "output-accumulator/max-lines",
		tool: "output-accumulator",
		args: { chunks: [b64(nLines(100))], maxLines: 10 },
	},
	{
		name: "output-accumulator/max-bytes-temp-file",
		tool: "output-accumulator",
		args: {
			chunks: [b64(nLines(2000, "0123456789"))],
			maxBytes: 1000,
			tempFilePrefix: "pi-bash",
			readFullOutputMaxBytes: 500,
		},
	},
	{
		name: "output-accumulator/persist-false",
		tool: "output-accumulator",
		args: {
			chunks: [b64(nLines(2000, "0123456789"))],
			maxBytes: 1000,
			tempFilePrefix: "pi-bash",
			persistIfTruncated: false,
		},
	},
	{
		name: "output-accumulator/utf8-split",
		tool: "output-accumulator",
		args: { chunks: [b64range("h\u00e9llo w\u00f6rld", 0, 2), b64range("h\u00e9llo w\u00f6rld", 2, 13)] },
	},
	{
		name: "output-accumulator/last-line-bytes",
		tool: "output-accumulator",
		args: { chunks: [b64("first\nsecond\npartial li")] },
	},
	{
		name: "output-accumulator/first-line-exceeds",
		tool: "output-accumulator",
		args: {
			chunks: [b64("x".repeat(5000) + "\n" + "tail")],
			maxBytes: 1000,
			tempFilePrefix: "pi-bash",
		},
	},
	{
		name: "output-accumulator/no-finish",
		tool: "output-accumulator",
		args: { chunks: [b64("unterminated")], finish: false },
	},
	{
		name: "output-accumulator/read-full-small",
		tool: "output-accumulator",
		args: { chunks: [b64(nLines(50))], readFullOutputMaxBytes: 100000 },
	},

	// ------------------------------------------------------ messages (module)
	{
		name: "messages/to-text-basic",
		tool: "messages",
		args: {
			fn: "bashExecutionToText",
			message: {
				role: "bashExecution",
				command: "ls -la",
				output: "total 0\ndrwxr-xr-x",
				exitCode: 0,
				cancelled: false,
				truncated: false,
				timestamp: 1700000000000,
			},
		},
	},
	{
		name: "messages/to-text-no-output",
		tool: "messages",
		args: {
			fn: "bashExecutionToText",
			message: {
				role: "bashExecution",
				command: "true",
				output: "",
				exitCode: 0,
				cancelled: false,
				truncated: false,
				timestamp: 1,
			},
		},
	},
	{
		name: "messages/to-text-cancelled",
		tool: "messages",
		args: {
			fn: "bashExecutionToText",
			message: {
				role: "bashExecution",
				command: "sleep 100",
				output: "",
				exitCode: 1,
				cancelled: true,
				truncated: false,
				timestamp: 1,
			},
		},
	},
	{
		name: "messages/to-text-nonzero-exit",
		tool: "messages",
		args: {
			fn: "bashExecutionToText",
			message: {
				role: "bashExecution",
				command: "make",
				output: "error: no target",
				exitCode: 2,
				cancelled: false,
				truncated: false,
				timestamp: 1,
			},
		},
	},
	{
		name: "messages/to-text-null-exit",
		tool: "messages",
		args: {
			fn: "bashExecutionToText",
			message: {
				role: "bashExecution",
				command: "kill -9 $$",
				output: "gone",
				exitCode: null,
				cancelled: false,
				truncated: false,
				timestamp: 1,
			},
		},
	},
	{
		name: "messages/to-text-truncated",
		tool: "messages",
		args: {
			fn: "bashExecutionToText",
			message: {
				role: "bashExecution",
				command: "seq 1 100000",
				output: "1\n2\n3",
				exitCode: 0,
				cancelled: false,
				truncated: true,
				fullOutputPath: "/tmp/pi-bash-0123456789abcdef.log",
				timestamp: 1,
			},
		},
	},
	{
		name: "messages/convert-to-llm",
		tool: "messages",
		args: {
			fn: "convertToLlm",
			messages: [
				{ role: "user", content: "plain question", timestamp: 1 },
				{
					role: "bashExecution",
					command: "echo hi",
					output: "hi\n",
					exitCode: 0,
					cancelled: false,
					truncated: false,
					timestamp: 2,
				},
				{
					role: "bashExecution",
					command: "secret",
					output: "o",
					exitCode: 0,
					cancelled: false,
					truncated: false,
					timestamp: 3,
					excludeFromContext: true,
				},
				{
					role: "custom",
					customType: "myExtension",
					content: "hello from extension",
					display: true,
					timestamp: 4,
				},
				{
					role: "custom",
					customType: "img",
					content: [{ type: "text", text: "caption" }, { type: "image", data: "abcd", mimeType: "image/png" }],
					display: false,
					timestamp: 5,
				},
				{
					role: "custom",
					customType: "detailed",
					content: "with details",
					display: true,
					details: { nested: [1, 2, { deep: true }] },
					timestamp: 6,
				},
				{ role: "branchSummary", summary: "did things", fromId: "id-1", timestamp: 7 },
				{ role: "branchSummary", summary: "no id", fromId: null, timestamp: 8 },
				{ role: "compactionSummary", summary: "compacted", tokensBefore: 1234, timestamp: 9 },
				{ role: "assistant", content: [{ type: "text", text: "answer" }] },
			],
		},
	},
	{
		name: "messages/create-branch-summary",
		tool: "messages",
		args: { fn: "createBranchSummaryMessage", summary: "branched", fromId: "sess-42", timestamp: "2026-01-02T03:04:05Z" },
	},
	{
		name: "messages/create-branch-summary-null-and-invalid-date",
		tool: "messages",
		args: { fn: "createBranchSummaryMessage", summary: "s", fromId: null, timestamp: "bogus" },
	},
	{
		name: "messages/create-compaction-summary",
		tool: "messages",
		args: { fn: "createCompactionSummaryMessage", summary: "compacted", tokensBefore: 9999, timestamp: "2026-01-02T03:04:05.123Z" },
	},
	{
		name: "messages/create-compaction-summary-pre-epoch",
		tool: "messages",
		args: { fn: "createCompactionSummaryMessage", summary: "s", tokensBefore: 1, timestamp: "1969-12-31T23:59:59Z" },
	},
	{
		name: "messages/create-custom",
		tool: "messages",
		args: {
			fn: "createCustomMessage",
			customType: "ext",
			content: "hello",
			display: true,
			details: { a: 1 },
			timestamp: "2026-01-02T03:04:05.5Z",
		},
	},
	{
		name: "messages/create-custom-blocks",
		tool: "messages",
		args: {
			fn: "createCustomMessage",
			customType: "img",
			content: [{ type: "image", data: "eHg=", mimeType: "image/png" }],
			display: false,
			timestamp: "2026-01-02T03:04:05+02:00",
		},
	},

	// ------------------------------------------------------- compaction-utils
	{
		name: "compaction-utils/create-file-ops",
		tool: "compaction-utils",
		args: { fn: "createFileOps" },
	},
	{
		name: "compaction-utils/extract-toolresult-nested",
		tool: "compaction-utils",
		args: {
			fn: "extractFileOpsFromMessage",
			message: {
				role: "toolResult",
				content: [{ type: "text", text: "done" }],
				nestedCalls: {
					calls: [
						{ name: "read", arguments: { path: "/a" } },
						{ name: "write", arguments: { path: "/b" } },
						{ name: "edit", arguments: { path: "/c" } },
						{ name: "read", arguments: { path: "/d" } },
					],
				},
			},
		},
	},
	{
		name: "compaction-utils/extract-assistant-toolcalls",
		tool: "compaction-utils",
		args: {
			fn: "extractFileOpsFromMessage",
			message: {
				role: "assistant",
				content: [
					{ type: "text", text: "working" },
					{ type: "toolCall", name: "read", arguments: { path: "/x" } },
					{ type: "toolCall", name: "edit", arguments: { path: "/y" } },
					{ type: "toolCall", name: "other", arguments: { path: "/z" } },
					{ type: "toolCall", name: "read", arguments: {} },
					{ type: "toolCall", name: "read", arguments: { path: 123 } },
				],
			},
		},
	},
	{
		name: "compaction-utils/extract-existing-fileops-dedupe",
		tool: "compaction-utils",
		args: {
			fn: "extractFileOpsFromMessage",
			fileOps: { read: ["/pre", "/a"], written: [], edited: [] },
			message: {
				role: "assistant",
				content: [{ type: "toolCall", name: "read", arguments: { path: "/a" } }],
			},
		},
	},
	{
		name: "compaction-utils/extract-user-message-noop",
		tool: "compaction-utils",
		args: {
			fn: "extractFileOpsFromMessage",
			fileOps: { read: ["/pre"], written: [], edited: [] },
			message: { role: "user", content: [{ type: "text", text: "hi" }] },
		},
	},
	{
		name: "compaction-utils/extract-assistant-no-content",
		tool: "compaction-utils",
		args: {
			fn: "extractFileOpsFromMessage",
			message: { role: "assistant" },
		},
	},
	{
		name: "compaction-utils/extract-toolresult-no-nested",
		tool: "compaction-utils",
		args: {
			fn: "extractFileOpsFromMessage",
			message: { role: "toolResult", content: [{ type: "text", text: "ok" }] },
		},
	},
	{
		name: "compaction-utils/compute-lists-overlap",
		tool: "compaction-utils",
		args: {
			fn: "computeFileLists",
			fileOps: { read: ["/b", "/a"], written: ["/b"], edited: ["/c", "/c"] },
		},
	},
	{
		name: "compaction-utils/compute-lists-empty",
		tool: "compaction-utils",
		args: { fn: "computeFileLists", fileOps: { read: [], written: [], edited: [] } },
	},
	{
		name: "compaction-utils/format-both",
		tool: "compaction-utils",
		args: { fn: "formatFileOperations", readFiles: ["/a", "/b"], modifiedFiles: ["/c"] },
	},
	{
		name: "compaction-utils/format-read-only",
		tool: "compaction-utils",
		args: { fn: "formatFileOperations", readFiles: ["/a"], modifiedFiles: [] },
	},
	{
		name: "compaction-utils/format-empty",
		tool: "compaction-utils",
		args: { fn: "formatFileOperations", readFiles: [], modifiedFiles: [] },
	},
	{
		name: "compaction-utils/serialize-user-only",
		tool: "compaction-utils",
		args: {
			fn: "serializeConversation",
			messages: [{ role: "user", content: "hello\nworld" }],
		},
	},
	{
		name: "compaction-utils/serialize-assistant-text-and-calls",
		tool: "compaction-utils",
		args: {
			fn: "serializeConversation",
			messages: [
				{
					role: "assistant",
					content: [
						{ type: "text", text: "let me check" },
						{
							type: "toolCall",
							name: "read",
							arguments: { path: "/x", count: 3, flag: true, note: null, nested: { a: 1 } },
						},
						{ type: "toolCall", name: "edit", arguments: { path: "/y" } },
					],
				},
			],
		},
	},
	{
		name: "compaction-utils/serialize-assistant-calls-only",
		tool: "compaction-utils",
		args: {
			fn: "serializeConversation",
			messages: [
				{
					role: "assistant",
					content: [{ type: "toolCall", name: "bash", arguments: { command: "ls" } }],
				},
			],
		},
	},
	{
		name: "compaction-utils/serialize-toolresult-truncation",
		tool: "compaction-utils",
		args: {
			fn: "serializeConversation",
			messages: [{ role: "toolResult", content: "z".repeat(2505) }],
		},
	},
	{
		name: "compaction-utils/serialize-skips-empty-and-thinking",
		tool: "compaction-utils",
		args: {
			fn: "serializeConversation",
			messages: [
				{ role: "user", content: [] },
				{
					role: "assistant",
					content: [{ type: "thinking", thinking: "inner monologue" }],
				},
				{ role: "toolResult", content: [{ type: "text", text: "" }] },
			],
		},
	},
	{
		name: "compaction-utils/summarization-system-prompt",
		tool: "compaction-utils",
		args: { fn: "summarizationSystemPrompt" },
	},
	// ------------------------------------------------------------ compaction
	{
		name: "compaction/default-settings",
		tool: "compaction",
		args: { fn: "defaultSettings" },
	},
	{
		name: "compaction/calc-tokens-total",
		tool: "compaction",
		args: {
			fn: "calculateContextTokens",
			usage: { totalTokens: 100, input: 1, output: 2, cacheRead: 3, cacheWrite: 4 },
		},
	},
	{
		name: "compaction/calc-tokens-zero-total",
		tool: "compaction",
		args: {
			fn: "calculateContextTokens",
			usage: { totalTokens: 0, input: 10, output: 5, cacheRead: 1, cacheWrite: 2 },
		},
	},
	{
		name: "compaction/calc-tokens-missing-components",
		tool: "compaction",
		args: { fn: "calculateContextTokens", usage: { input: 10, output: 5 } },
	},
	{
		name: "compaction/calc-tokens-empty",
		tool: "compaction",
		args: { fn: "calculateContextTokens", usage: {} },
	},
	{
		name: "compaction/last-usage-found",
		tool: "compaction",
		args: {
			fn: "getLastAssistantUsage",
			entries: [
				smUser("e1", null, "q", 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						stopReason: "endTurn",
						content: [{ type: "text", text: "a" }],
						usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0 },
					},
					2,
				),
			],
		},
	},
	{
		name: "compaction/last-usage-skips-aborted",
		tool: "compaction",
		args: {
			fn: "getLastAssistantUsage",
			entries: [
				smMsg(
					"e1",
					null,
					{
						role: "assistant",
						stopReason: "aborted",
						content: [{ type: "text", text: "a" }],
						usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0 },
					},
					1,
				),
			],
		},
	},
	{
		name: "compaction/last-usage-skips-error",
		tool: "compaction",
		args: {
			fn: "getLastAssistantUsage",
			entries: [
				smMsg(
					"e1",
					null,
					{
						role: "assistant",
						stopReason: "error",
						content: [{ type: "text", text: "a" }],
						usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0 },
					},
					1,
				),
			],
		},
	},
	{
		name: "compaction/last-usage-skips-zero",
		tool: "compaction",
		args: {
			fn: "getLastAssistantUsage",
			entries: [
				smMsg(
					"e1",
					null,
					{
						role: "assistant",
						stopReason: "endTurn",
						content: [{ type: "text", text: "a" }],
						usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
					},
					1,
				),
			],
		},
	},
	{
		name: "compaction/last-usage-none",
		tool: "compaction",
		args: { fn: "getLastAssistantUsage", entries: [smUser("e1", null, "q", 1)] },
	},
	{
		name: "compaction/estimate-tokens-system",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: {
				role: "system",
				content: "12345678",
				sections: { a: "1234", b: null, c: "" },
				toolsAdded: [{ name: "read" }],
			},
		},
	},
	{
		name: "compaction/estimate-tokens-user-string",
		tool: "compaction",
		args: { fn: "estimateTokens", message: { role: "user", content: "12345" } },
	},
	{
		name: "compaction/estimate-tokens-user-array-image",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: { role: "user", content: [{ type: "text", text: "abcd" }, { type: "image" }] },
		},
	},
	{
		name: "compaction/estimate-tokens-assistant",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: {
				role: "assistant",
				content: [
					{ type: "text", text: "12345" },
					{ type: "thinking", thinking: "123" },
					{ type: "toolCall", name: "read", arguments: { path: "/x" } },
				],
			},
		},
	},
	{
		name: "compaction/estimate-tokens-assistant-string-content",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: { role: "assistant", content: "123456789" },
		},
	},
	{
		name: "compaction/estimate-tokens-toolresult-array",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: { role: "toolResult", content: [{ type: "text", text: "123456789" }] },
		},
	},
	{
		name: "compaction/estimate-tokens-custom",
		tool: "compaction",
		args: { fn: "estimateTokens", message: { role: "custom", content: "abcdefgh" } },
	},
	{
		name: "compaction/estimate-tokens-bash",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: { role: "bashExecution", command: "abcd", output: "efghij" },
		},
	},
	{
		name: "compaction/estimate-tokens-summaries",
		tool: "compaction",
		args: {
			fn: "estimateTokens",
			message: { role: "branchSummary", summary: "abcdefghij" },
		},
	},
	{
		name: "compaction/estimate-tokens-unknown-role",
		tool: "compaction",
		args: { fn: "estimateTokens", message: { role: "weird", content: "abcdefgh" } },
	},
	{
		name: "compaction/estimate-context-no-usage",
		tool: "compaction",
		args: {
			fn: "estimateContextTokens",
			messages: [
				{ role: "user", content: "12345678" },
				{ role: "assistant", content: [{ type: "text", text: "1234" }] },
			],
		},
	},
	{
		name: "compaction/estimate-context-with-usage",
		tool: "compaction",
		args: {
			fn: "estimateContextTokens",
			messages: [
				{ role: "user", content: "12345678" },
				{
					role: "assistant",
					stopReason: "endTurn",
					content: [{ type: "text", text: "1234" }],
					usage: { input: 400, output: 100, cacheRead: 0, cacheWrite: 0 },
				},
				{ role: "user", content: "12345678" },
			],
		},
	},
	{
		name: "compaction/estimate-projected-trusted-usage",
		tool: "compaction",
		args: {
			fn: "estimateProjectedContextTokens",
			entries: [
				smUser("e1", null, "12345678", 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						stopReason: "endTurn",
						content: [{ type: "text", text: "1234" }],
						usage: { input: 400, output: 100, cacheRead: 0, cacheWrite: 0 },
					},
					2,
				),
				smUser("e3", "e2", "12345678", 3),
			],
			branchEntries: [
				smUser("e1", null, "12345678", 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						stopReason: "endTurn",
						content: [{ type: "text", text: "1234" }],
						usage: { input: 400, output: 100, cacheRead: 0, cacheWrite: 0 },
					},
					2,
				),
				smUser("e3", "e2", "12345678", 3),
			],
		},
	},
	{
		name: "compaction/estimate-projected-invalidated-by-edit",
		tool: "compaction",
		args: {
			fn: "estimateProjectedContextTokens",
			entries: [
				smUser("e1", null, "12345678", 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						stopReason: "endTurn",
						content: [{ type: "text", text: "1234" }],
						usage: { input: 400, output: 100, cacheRead: 0, cacheWrite: 0 },
					},
					2,
				),
				{ type: "context_edit", id: "e3", parentId: "e2", timestamp: smTs(3), targetId: "e1", replacement: null },
				smUser("e4", "e3", "12345678", 4),
			],
			branchEntries: [
				smUser("e1", null, "12345678", 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						stopReason: "endTurn",
						content: [{ type: "text", text: "1234" }],
						usage: { input: 400, output: 100, cacheRead: 0, cacheWrite: 0 },
					},
					2,
				),
				{ type: "context_edit", id: "e3", parentId: "e2", timestamp: smTs(3), targetId: "e1", replacement: null },
				smUser("e4", "e3", "12345678", 4),
			],
		},
	},
	{
		name: "compaction/should-compact-disabled",
		tool: "compaction",
		args: {
			fn: "shouldCompact",
			contextTokens: 999999,
			contextWindow: 100000,
			settings: { enabled: false, reserveTokens: 16384, keepRecentTokens: 20000 },
		},
	},
	{
		name: "compaction/should-compact-over",
		tool: "compaction",
		args: {
			fn: "shouldCompact",
			contextTokens: 90000,
			contextWindow: 100000,
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 20000 },
		},
	},
	{
		name: "compaction/should-compact-under",
		tool: "compaction",
		args: {
			fn: "shouldCompact",
			contextTokens: 50000,
			contextWindow: 100000,
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 20000 },
		},
	},
	{
		name: "compaction/find-cut-no-points",
		tool: "compaction",
		args: {
			fn: "findCutPoint",
			entries: [
				smMsg("e1", null, { role: "toolResult", content: [{ type: "text", text: "ok" }] }, 1),
				{ type: "label_change", id: "e2", parentId: "e1", timestamp: smTs(2), label: "x" },
			],
			startIndex: 0,
			endIndex: 2,
			keepRecentTokens: 10,
		},
	},
	{
		name: "compaction/find-cut-budget-not-exceeded",
		tool: "compaction",
		args: {
			fn: "findCutPoint",
			entries: [
				smUser("e1", null, "12345678", 1),
				smAssistant("e2", "e1", "1234", 2),
				smUser("e3", "e2", "12345678", 3),
			],
			startIndex: 0,
			endIndex: 3,
			keepRecentTokens: 10000,
		},
	},
	{
		name: "compaction/find-cut-mid",
		tool: "compaction",
		args: {
			fn: "findCutPoint",
			entries: [
				smUser("e1", null, "a".repeat(400), 1),
				smAssistant("e2", "e1", "b".repeat(400), 2),
				smUser("e3", "e2", "c".repeat(400), 3),
				smAssistant("e4", "e3", "d".repeat(400), 4),
			],
			startIndex: 0,
			endIndex: 4,
			keepRecentTokens: 150,
		},
	},
	{
		name: "compaction/find-cut-split-turn",
		tool: "compaction",
		args: {
			fn: "findCutPoint",
			entries: [
				smUser("e1", null, "123456789", 1),
				smAssistant("e2", "e1", "b".repeat(1200), 2),
				smUser("e3", "e2", "c".repeat(100), 3),
				smUser("e4", "e3", "d".repeat(100), 4),
			],
			startIndex: 0,
			endIndex: 4,
			keepRecentTokens: 150,
		},
	},
	{
		name: "compaction/find-cut-trailing-toolresults-overflow",
		tool: "compaction",
		args: {
			fn: "findCutPoint",
			entries: [
				smUser("e1", null, "a".repeat(40), 1),
				smMsg(
					"e2",
					"e1",
					{ role: "assistant", content: [{ type: "toolCall", name: "read", arguments: { path: "/x" } }] },
					2,
				),
				smMsg("e3", "e2", { role: "toolResult", content: [{ type: "text", text: "y".repeat(400) }] }, 3),
			],
			startIndex: 0,
			endIndex: 3,
			keepRecentTokens: 50,
		},
	},
	{
		name: "compaction/find-cut-metadata-backtrack",
		tool: "compaction",
		args: {
			fn: "findCutPoint",
			entries: [
				smUser("e1", null, "a".repeat(40), 1),
				{ type: "label_change", id: "e2", parentId: "e1", timestamp: smTs(2), label: "mid" },
				smUser("e3", "e2", "b".repeat(40), 3),
			],
			startIndex: 0,
			endIndex: 3,
			keepRecentTokens: 5,
		},
	},
	{
		name: "compaction/find-turn-start-basic",
		tool: "compaction",
		args: {
			fn: "findTurnStartIndex",
			entries: [
				smUser("e1", null, "q", 1),
				smMsg(
					"e2",
					"e1",
					{ role: "assistant", content: [{ type: "text", text: "a" }] },
					2,
				),
				smMsg("e3", "e2", { role: "toolResult", content: [{ type: "text", text: "r" }] }, 3),
			],
			entryIndex: 2,
			startIndex: 0,
		},
	},
	{
		name: "compaction/find-turn-start-none",
		tool: "compaction",
		args: {
			fn: "findTurnStartIndex",
			entries: [
				smMsg("e1", null, { role: "toolResult", content: [{ type: "text", text: "r" }] }, 1),
			],
			entryIndex: 0,
			startIndex: 0,
		},
	},
	{
		name: "compaction/failure-error",
		tool: "compaction",
		args: {
			fn: "getSummarizationFailure",
			response: { stopReason: "error", errorMessage: "boom" },
			label: "Summarization",
		},
	},
	{
		name: "compaction/failure-error-empty-message",
		tool: "compaction",
		args: {
			fn: "getSummarizationFailure",
			response: { stopReason: "error" },
			label: "Turn prefix summarization",
		},
	},
	{
		name: "compaction/failure-length",
		tool: "compaction",
		args: {
			fn: "getSummarizationFailure",
			response: { stopReason: "length" },
			label: "Summarization",
		},
	},
	{
		name: "compaction/failure-ok",
		tool: "compaction",
		args: {
			fn: "getSummarizationFailure",
			response: { stopReason: "endTurn" },
			label: "Summarization",
		},
	},
	{
		name: "compaction/prepare-basic",
		tool: "compaction",
		args: {
			fn: "prepareCompaction",
			entries: [
				smUser("e1", null, "a".repeat(400), 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						content: [{ type: "toolCall", name: "read", arguments: { path: "/x" } }],
						usage: { input: 100, output: 50 },
					},
					2,
				),
				smUser("e3", "e2", "c".repeat(400), 3),
				smUser("e4", "e3", "d".repeat(400), 4),
			],
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 120 },
		},
	},
	{
		name: "compaction/prepare-last-is-compaction",
		tool: "compaction",
		args: {
			fn: "prepareCompaction",
			entries: [
				smUser("e1", null, "q", 1),
				{
					type: "compaction",
					id: "e2",
					parentId: "e1",
					timestamp: smTs(2),
					summary: "done so far",
					firstKeptEntryId: "e1",
					tokensBefore: 100,
				},
			],
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 20000 },
		},
	},
	{
		name: "compaction/prepare-split-turn",
		tool: "compaction",
		args: {
			fn: "prepareCompaction",
			entries: [
				smUser("e1", null, "123456789", 1),
				smAssistant("e2", "e1", "b".repeat(1200), 2),
				smUser("e3", "e2", "c".repeat(100), 3),
				smUser("e4", "e3", "d".repeat(100), 4),
			],
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 150 },
		},
	},
	{
		name: "compaction/prepare-prev-compaction",
		tool: "compaction",
		args: {
			fn: "prepareCompaction",
			entries: [
				{
					type: "compaction",
					id: "e0",
					parentId: null,
					timestamp: smTs(0),
					summary: "earlier work",
					firstKeptEntryId: null,
					tokensBefore: 900,
					details: { readFiles: ["/old-r"], modifiedFiles: ["/old-m"] },
				},
				smUser("e1", "e0", "a".repeat(400), 1),
				smMsg(
					"e2",
					"e1",
					{
						role: "assistant",
						content: [{ type: "toolCall", name: "edit", arguments: { path: "/new" } }],
						usage: { input: 100, output: 50, cacheRead: 0, cacheWrite: 0 },
					},
					2,
				),
				smUser("e3", "e2", "c".repeat(400), 3),
				smUser("e4", "e3", "d".repeat(400), 4),
			],
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 120 },
		},
	},
	{
		name: "compaction/prepare-nothing-to-summarize",
		tool: "compaction",
		args: {
			fn: "prepareCompaction",
			entries: [smUser("e1", null, "a".repeat(400), 1)],
			settings: { enabled: true, reserveTokens: 16384, keepRecentTokens: 10 },
		},
	},
	// ------------------------------------------------ file-mutation-queue
	// Differential parity for withFileMutationQueue: per-file serialization,
	// cross-file parallelism, symlink/realpath key sharing, missing-path
	// fallback, and fn-error propagation with queue release. Ops run
	// concurrently with scripted fns; gates make the interleaving
	// deterministic on both sides.
	{
		name: "file-mutation-queue/same-file-serializes",
		tool: "file-mutation-queue",
		args: {
			ops: [
				{ path: "a.txt", script: ["a1", { awaitGate: "g" }, "a2"] },
				{ path: "a.txt", script: ["b1"] },
			],
			release: [{ gate: "g", afterStarted: [0] }],
		},
	},
	{
		name: "file-mutation-queue/different-files-parallel",
		tool: "file-mutation-queue",
		args: {
			ops: [
				{ path: "a.txt", script: ["a1", { awaitGate: "g" }, "a2"] },
				{ path: "b.txt", script: ["b1"] },
			],
			release: [{ gate: "g", afterStarted: [0, 1] }],
		},
	},
	{
		name: "file-mutation-queue/symlink-shares-queue",
		tool: "file-mutation-queue",
		args: {
			setup: {
				realFiles: [{ path: "f.txt", content: "x" }],
				symlinks: [{ link: "link.txt", target: "f.txt" }],
			},
			ops: [
				{ path: "f.txt", script: ["a1", { awaitGate: "g" }, "a2"] },
				{ path: "link.txt", script: ["b1"] },
			],
			release: [{ gate: "g", afterStarted: [0] }],
		},
	},
	{
		name: "file-mutation-queue/missing-path-fallback",
		tool: "file-mutation-queue",
		args: {
			ops: [
				{ path: "missing/x.txt", script: ["a1", { awaitGate: "g" }, "a2"] },
				{ path: "missing/x.txt", script: ["b1"] },
			],
			release: [{ gate: "g", afterStarted: [0] }],
		},
	},
	{
		name: "file-mutation-queue/fn-error-releases-queue",
		tool: "file-mutation-queue",
		args: {
			ops: [
				{ path: "a.txt", script: ["a1", { throw: "boom" }] },
				{ path: "a.txt", script: ["b1"] },
			],
		},
	},
	{
		name: "file-mutation-queue/three-ops-fifo",
		tool: "file-mutation-queue",
		args: {
			ops: [
				{ path: "a.txt", script: ["a1", { awaitGate: "g" }, "a2"] },
				{ path: "a.txt", script: ["b1"] },
				{ path: "a.txt", script: ["c1"] },
			],
			release: [{ gate: "g", afterStarted: [0] }],
		},
	},
	{
		name: "file-mutation-queue/two-gates",
		tool: "file-mutation-queue",
		args: {
			ops: [
				{ path: "a.txt", script: ["a1", { awaitGate: "g1" }, "a2", { awaitGate: "g2" }, "a3"] },
				{ path: "b.txt", script: ["b1", { awaitGate: "g2" }, "b2"] },
			],
			release: [
				{ gate: "g1", afterStarted: [0, 1] },
				{ gate: "g2", afterStarted: [0, 1] },
			],
		},
	},


	// ---------------------------------------------------- agent-session-events
	// Differential parity for the events slice of AgentSession (TS
	// dist/core/agent-session.js ↔ crates/pi-session). The case declares a
	// message POOL (referenced by index in ops/entries) that both harnesses
	// wire as the same objects; entries[0] must be the session header.
	// Conventions: entry ids never look like "uuid-N" (appended entries get
	// those); messageEndReplacement/nestedRecords targets are never
	// entry-linked (in-place mutation is only visible through live TS
	// references); sendCustomMessage passes boolean display, object-or-absent
	// details, and never triggerTurn:true (run pipeline, tranche 11).
	{
		name: "agent-session-events/missing-args",
		tool: "agent-session-events",
		args: {},
	},
	{
		name: "agent-session-events/per-role-persistence",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{ role: "system", content: "be brief", timestamp: 1 },
				{ role: "user", content: [{ type: "text", text: "hi" }], timestamp: 2 },
				{
					role: "assistant",
					content: [{ type: "text", text: "hello" }],
					stopReason: "stop",
					provider: "anthropic",
					model: "claude-x",
					timestamp: 3,
				},
				{ role: "toolResult", toolCallId: "c0", content: "out", timestamp: 4 },
			],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			ops: [
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 0 } },
				{ op: "findPersisted", messageIndex: 0 },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 1 } },
				{ op: "findPersisted", messageIndex: 1 },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 2 } },
				{ op: "findPersisted", messageIndex: 2 },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 3 } },
				{ op: "findPersisted", messageIndex: 3 },
			],
		},
	},
	{
		name: "agent-session-events/custom-entry-shape",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{
					role: "custom",
					customType: "note",
					content: [{ type: "text", text: "cached" }],
					display: true,
					details: { kind: "memo" },
					timestamp: 5,
				},
			],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			ops: [
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 0 } },
				{
					op: "sendCustomMessage",
					customType: "todo",
					content: [{ type: "text", text: "later" }],
					display: false,
				},
			],
		},
	},
	{
		name: "agent-session-events/queue-removal",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{ role: "user", content: [{ type: "text", text: "go left" }], timestamp: 1 },
				{ role: "user", content: [{ type: "text", text: "go right" }], timestamp: 2 },
				{ role: "user", content: [{ type: "text", text: "unrelated" }], timestamp: 3 },
			],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			ops: [
				{ op: "queueSteer", text: "go left" },
				{ op: "queueFollowUp", text: "go right" },
				{ op: "agentEvent", event: { type: "message_start", messageIndex: 0 } },
				{ op: "agentEvent", event: { type: "message_start", messageIndex: 2 } },
				{ op: "agentEvent", event: { type: "message_start", messageIndex: 1 } },
			],
		},
	},
	{
		name: "agent-session-events/nested-merge",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{
					role: "toolResult",
					toolCallId: "c1",
					content: "out",
					usage: {
						input: 2,
						output: 3,
						cacheRead: 0,
						cacheWrite: 0,
						totalTokens: 5,
						cost: { input: 0.001, output: 0.002, cacheRead: 0, cacheWrite: 0, total: 0.003 },
					},
					timestamp: 1,
				},
				{ role: "toolResult", toolCallId: "c2", content: "plain", timestamp: 2 },
			],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			nestedRecords: {
				c1: {
					calls: [{ type: "toolCall", id: "n1", name: "grep", arguments: { q: "x" } }],
					usage: {
						input: 5,
						output: 7,
						cacheRead: 0,
						cacheWrite: 0,
						totalTokens: 12,
						cost: { input: 0.005, output: 0.007, cacheRead: 0, cacheWrite: 0, total: 0.012 },
					},
				},
			},
			ops: [
				{ op: "agentEvent", event: { type: "message_start", messageIndex: 0 } },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 0 } },
				{ op: "agentEvent", event: { type: "message_start", messageIndex: 1 } },
				{ op: "agentEvent", event: { type: "agent_end", messageIndices: [0, 1] } },
			],
		},
	},
	{
		name: "agent-session-events/auto-retry-end",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{ role: "assistant", content: [], stopReason: "stop", timestamp: 1 },
				{ role: "assistant", content: [], stopReason: "error", errorMessage: "boom", timestamp: 2 },
			],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			ops: [
				{ op: "setRetryAttempt", value: 2 },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 0 } },
				{ op: "setRetryAttempt", value: 1 },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 1 } },
			],
		},
	},
	{
		name: "agent-session-events/will-retry",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			model: { provider: "anthropic", id: "claude-x", contextWindow: 1000 },
			messages: [
				{
					role: "assistant",
					content: [],
					stopReason: "error",
					errorMessage: "overloaded",
					provider: "anthropic",
					model: "claude-x",
					timestamp: 1,
				},
				{
					role: "assistant",
					content: [],
					stopReason: "error",
					errorMessage: "prompt is too long",
					provider: "anthropic",
					model: "claude-x",
					timestamp: 2,
				},
			],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			ops: [
				{ op: "agentEvent", event: { type: "agent_end", messageIndices: [0] } },
				{ op: "setRetryAttempt", value: 3 },
				{ op: "agentEvent", event: { type: "agent_end", messageIndices: [0] } },
				{ op: "setRetryAttempt", value: 0 },
				{ op: "agentEvent", event: { type: "agent_end", messageIndices: [1] } },
				{ op: "requestAbort" },
				{ op: "agentEvent", event: { type: "agent_end", messageIndices: [0] } },
			],
		},
	},
	{
		name: "agent-session-events/turn-end-flush",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{ role: "user", content: [{ type: "text", text: "run it" }], timestamp: 1 },
				{
					role: "assistant",
					content: [{ type: "text", text: "doing" }],
					stopReason: "error",
					errorMessage: "boom",
					timestamp: 2,
				},
				{ role: "toolResult", toolCallId: "c9", content: "res", timestamp: 3 },
			],
			stateCount: 3,
			entries: [
				{ type: "session", id: "s-root", version: 3, cwd: "/proj" },
				{ type: "message", id: "e1", parentId: null, timestamp: "2025-05-01T00:00:00.000Z", messageIndex: 0 },
			],
			ops: [
				{ op: "setStreaming", value: true },
				{
					op: "sendCustomMessage",
					customType: "aside",
					content: [{ type: "text", text: "quiet note" }],
					display: true,
					details: { silent: true },
					triggerTurn: false,
				},
				{
					op: "sendCustomMessage",
					customType: "queued",
					content: [{ type: "text", text: "next" }],
					display: true,
					deliverAs: "nextTurn",
				},
				{ op: "agentEvent", event: { type: "turn_end", messageIndex: 1, toolResultIndices: [2] } },
				{ op: "agentEvent", event: { type: "turn_end", messageIndex: 1, toolResultIndices: [2, 0] } },
			],
		},
	},
	{
		name: "agent-session-events/settled-deferred-idle",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [],
			stateCount: 0,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			ops: [
				{ op: "setStreaming", value: true },
				{ op: "waitForIdle", tag: "w1" },
				{ op: "deferAction", tag: "d1" },
				{ op: "deferAction", tag: "d2" },
				{ op: "requestAbort" },
				{ op: "emitAgentSettled" },
				{ op: "waitForIdle", tag: "w2" },
			],
		},
	},
	{
		name: "agent-session-events/find-persisted-scans",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{ role: "user", content: [{ type: "text", text: "seed" }], timestamp: 1 },
				{ role: "assistant", content: [], stopReason: "stop", timestamp: 2 },
				{ role: "system", content: "extra", timestamp: 3 },
			],
			stateCount: 2,
			entries: [
				{ type: "session", id: "s-root", version: 3, cwd: "/proj" },
				{ type: "message", id: "e1", parentId: null, timestamp: "2025-05-01T00:00:00.000Z", messageIndex: 0 },
				{
					type: "custom_message",
					id: "c1",
					parentId: "e1",
					timestamp: "2025-05-01T00:00:01.000Z",
					customType: "note",
					content: [{ type: "text", text: "hi" }],
					display: true,
				},
			],
			ops: [
				{ op: "findPersisted", messageIndex: 0 },
				{ op: "findPersisted", messageIndex: 1 },
				{ op: "findPersisted", messageIndex: 2 },
				{ op: "findPersisted", messageIndex: 1 },
			],
		},
	},
	{
		name: "agent-session-events/message-end-replacement",
		tool: "agent-session-events",
		args: {
			nowMs: 1747000000000,
			messages: [
				{
					role: "assistant",
					content: [{ type: "text", text: "original" }],
					stopReason: "stop",
					timestamp: 1,
				},
			],
			stateCount: 1,
			entries: [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }],
			extensionConfig: {
				messageEndReplacement: { role: "assistant", content: null, stopReason: "stop" },
			},
			ops: [
				{ op: "agentEvent", event: { type: "message_start", messageIndex: 0 } },
				{ op: "agentEvent", event: { type: "message_end", messageIndex: 0 } },
			],
		},
	},


	// --------------------------------------------------- agent-session-tools
	// Differential parity for the tools slice of AgentSession (TS
	// dist/core/agent-session.js ↔ crates/pi-session). Conventions:
	// `baseTools` is required (the builtin tool definitions are Phase 4+);
	// extensionTools are invisible to the constructor-time refresh (the TS
	// constructor builds a real, empty ExtensionRunner, the fake is swapped
	// after); tool execution and streamFn are never reached.
	{
		name: "agent-session-tools/missing-args",
		tool: "agent-session-tools",
		args: {},
	},
	{
		name: "agent-session-tools/default-loadout",
		tool: "agent-session-tools",
		args: astBase({
			ops: [{ op: "getAllTools" }, { op: "getCallableToolNames" }],
		}),
	},
	{
		name: "agent-session-tools/custom-tools-and-snippets",
		tool: "agent-session-tools",
		args: astBase({
			customTools: [
				{
					name: "snip",
					description: "Snippet tool",
					parameters: { type: "object", properties: { a: { type: "string" } } },
					promptSnippet: "  snip:   does\n\nstuff  ",
					promptGuidelines: [" keep snippy ", "", "keep snippy", "keep snippy"],
				},
				{
					name: "quiet",
					description: "No snippet",
					parameters: {},
					promptSnippet: "   ",
				},
			],
			ops: [{ op: "getAllTools" }, { op: "setActiveTools", names: ["read", "snip", "quiet", "nope"] }],
		}),
	},
	{
		name: "agent-session-tools/allowlist-filtering",
		tool: "agent-session-tools",
		args: astBase({
			config: { allowedToolNames: ["read"] },
			extensionTools: [
				{
					definition: { name: "mcp__docs__search", description: "docs", parameters: {}, exposure: "deferred" },
					sourceInfo: { path: "<ext:docs>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
				{
					definition: { name: "tool_search", description: "search tools", parameters: {}, exposure: "codemode", defaultActive: false },
					sourceInfo: { path: "<ext:tools>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
			],
			ops: [
				{ op: "refreshRegistry" },
				{ op: "getAllTools" },
				{ op: "setActiveTools", names: ["mcp__docs__search"] },
				{ op: "getCallableToolNames" },
			],
		}),
	},
	{
		name: "agent-session-tools/empty-allowlist-filters-mcp",
		tool: "agent-session-tools",
		args: astBase({
			config: { allowedToolNames: [] },
			extensionTools: [
				{
					definition: { name: "mcp__docs__search", description: "docs", parameters: {}, exposure: "deferred" },
					sourceInfo: { path: "<ext:docs>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
			],
			ops: [{ op: "refreshRegistry", includeAllExtensionTools: true }, { op: "getAllTools" }],
		}),
	},
	{
		name: "agent-session-tools/excluded-tools",
		tool: "agent-session-tools",
		args: astBase({
			config: { excludedToolNames: ["bash"] },
			ops: [{ op: "getAllTools" }, { op: "refreshRegistry" }],
		}),
	},
	{
		name: "agent-session-tools/extension-merge-and-exposures",
		tool: "agent-session-tools",
		args: astBase({
			extensionTools: [
				{
					definition: { name: "auto_ext", description: "auto", parameters: {} },
					sourceInfo: { path: "<ext:a>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
				{
					definition: { name: "off_ext", description: "off", parameters: {}, defaultActive: false },
					sourceInfo: { path: "<ext:a>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
				{
					definition: { name: "model_only", description: "mo", parameters: {}, exposure: "model-only" },
					sourceInfo: { path: "<ext:a>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
				{
					definition: { name: "code_ext", description: "code", parameters: {}, exposure: "codemode" },
					sourceInfo: { path: "<ext:a>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
			],
			ops: [
				{ op: "refreshRegistry", includeAllExtensionTools: true },
				{ op: "getAllTools" },
				{ op: "getCallableToolNames" },
				{ op: "setActiveTools", names: ["off_ext", "code_ext"] },
				{ op: "refreshRegistry", activeToolNames: ["read"] },
			],
		}),
	},
	{
		name: "agent-session-tools/restore-from-transcript",
		tool: "agent-session-tools",
		args: astBase({
			config: { usesDefaultTools: true, defaultToolModifiers: ["-bash", "+write"] },
			baseTools: {
				read: { name: "read", description: "Read files", parameters: {} },
				bash: { name: "bash", description: "Run commands", parameters: {} },
				write: { name: "write", description: "Write files", parameters: {} },
			},
			entries: [
				{ type: "session", id: "s-root", version: 3, cwd: "/proj" },
				{
					type: "message",
					id: "m-sys",
					message: {
						role: "system",
						content: "base prompt",
						toolsAdded: [{ name: "read" }, { name: "bash" }],
						timestamp: 100,
					},
				},
			],
			extensionTools: [
				{
					definition: { name: "late_ext", description: "late", parameters: {} },
					sourceInfo: { path: "<ext:late>", source: "temporary", scope: "temporary", origin: "top-level" },
				},
			],
			ops: [
				{ op: "restoreFromTranscript", modifiers: [] },
				{ op: "refreshRegistry", includeAllExtensionTools: true },
			],
		}),
	},
	{
		name: "agent-session-tools/hidden-declarations-projection",
		tool: "agent-session-tools",
		args: astBase({
			customTools: [
				{
					name: "steerer",
					description: "Steers",
					parameters: {},
					prepareLoadout: {
						kind: "changes",
						descriptions: { read: "Read files (steered)" },
						hiddenDeclarations: ["grep"],
					},
				},
			],
			messages: [
				{ role: "system", content: "base", toolsAdded: [{ name: "read" }, { name: "grep" }], timestamp: 1 },
				{ role: "user", content: [{ type: "text", text: "hi" }], timestamp: 2 },
				{ role: "system", content: "", toolsRemoved: [{ name: "grep" }], timestamp: 3 },
			],
			ops: [
				{ op: "setActiveTools", names: ["read", "grep", "steerer"] },
				{ op: "transformContext", messageIndices: [0, 1, 2] },
				{ op: "transformContext", messageIndices: [1] },
			],
		}),
	},
	{
		name: "agent-session-tools/forced-prompt-projection",
		tool: "agent-session-tools",
		args: astBase({
			messages: [
				{ role: "system", content: "base", toolsAdded: [{ name: "read" }], timestamp: 7 },
				{ role: "user", content: [{ type: "text", text: "hi" }], timestamp: 8 },
			],
			ops: [
				{ op: "setForcedPrompt", text: "FORCED PROMPT" },
				{ op: "transformContext", messageIndices: [0, 1] },
				{ op: "setForcedPrompt", text: null },
				{ op: "transformContext", messageIndices: [0, 1] },
			],
		}),
	},
	{
		name: "agent-session-tools/prepare-loadout-throw",
		tool: "agent-session-tools",
		args: astBase({
			customTools: [
				{
					name: "thrower",
					description: "Throws",
					parameters: {},
					prepareLoadout: { kind: "throw", message: "boom" },
				},
			],
			ops: [{ op: "setActiveTools", names: ["read", "thrower"] }],
		}),
	},

	// ------------------------------------------------- agent-session-prompt
	// Differential parity for the prompt slice of AgentSession (TS
	// dist/core/agent-session.js ↔ crates/pi-session
	// agent_session/prompt.rs): prompt / steer / followUp /
	// sendUserMessage / sendCustomMessage over a real Agent with a
	// scripted provider, extension-command dispatch, skill & template
	// expansion, image normalization, and queueing while a run is marked
	// active. entries[0] is the session header; provider turns are popped
	// per LLM call with each call's llm context recorded as
	// promptsSentToAgent; docs-path-bearing auth errors are normalized to
	// "<docs>" on both sides; skill files are written for non-failing
	// entries on the TS side (the Rust side reads injected content).
	asp("missing-args", {}),
	// Happy path: a prompt runs a full turn over the scripted provider and
	// settles; the starting user message (timestamp from the pinned clock)
	// and the assistant reply are persisted as new entries.
	asp("prompt-happy-path", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		script: [
			{ op: "prompt", text: "hello there", options: { preflightTag: "p1", source: "test" } },
		],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// Model/auth validation: no model → no-model-selected; a provider with
	// no configured key → no-API-key; OAuth mode → authentication failed.
	// All embed the docs path (normalized to "<docs>").
	asp("prompt-no-model", {
		nowMs: ASP_NOW,
		config: { entries: aspHeader() },
		script: [{ op: "prompt", text: "hi", options: { preflightTag: "p1" } }],
		providerTurns: [],
	}),
	asp("prompt-no-api-key", {
		nowMs: ASP_NOW,
		config: { modelAuth: { mode: "noKey" }, model: aspModel("anthropic"), entries: aspHeader() },
		script: [{ op: "prompt", text: "hi" }],
		providerTurns: [],
	}),
	asp("prompt-oauth-error", {
		nowMs: ASP_NOW,
		config: { modelAuth: { mode: "oauth" }, model: aspModel("anthropic"), entries: aspHeader() },
		script: [{ op: "prompt", text: "hi" }],
		providerTurns: [],
	}),
	// Queueing while a run is active (setStreaming drives
	// _isAgentRunActive): prompt without behavior throws; with behavior
	// (and direct steer/followUp) the message is queued; extension
	// commands still execute immediately.
	asp("streaming-queue", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		commands: { note: { mode: "ok" } },
		script: [
			{ op: "setStreaming", value: true },
			{ op: "prompt", text: "no behavior" },
			{ op: "prompt", text: "queued steer", options: { streamingBehavior: "steer", preflightTag: "q1" } },
			{ op: "prompt", text: "queued follow", options: { streamingBehavior: "followUp" } },
			{ op: "steer", text: "direct steer" },
			{ op: "followUp", text: "direct follow" },
			{ op: "prompt", text: "/note during stream" },
			{ op: "prompt", text: "/unknowncmd arg" },
		],
		providerTurns: [],
	}),
	// A steer queued while "active" is re-dispatched by the next run: the
	// first provider turn sees both the queued steering message and the
	// fresh prompt.
	asp("steer-requeue-on-next-run", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		script: [
			{ op: "setStreaming", value: true },
			{ op: "steer", text: "mid steer" },
			{ op: "setStreaming", value: false },
			{ op: "prompt", text: "fresh prompt" },
		],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),
	// Extension commands: dispatch (args string after the name), error
	// capture via emitError, unknown commands falling through to a normal
	// run, and the cannot-queue guard in steer.
	asp("extension-commands", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		commands: {
			note: { mode: "ok" },
			boom: { mode: "throw", errorText: "boom failed" },
		},
		script: [
			{ op: "prompt", text: "/note hi there", options: { preflightTag: "c1" } },
			{ op: "prompt", text: "/boom now", options: { preflightTag: "c2" } },
			{ op: "prompt", text: "/plaincmd arg" },
			{ op: "steer", text: "/note queued" },
		],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// Skill and template expansion: /skill:name blocks (frontmatter
	// stripped, args appended), read failures surface as emitError with
	// the ENOENT message, unknown skills pass through, and /name template
	// substitution runs the positional and ARGUMENTS placeholders.
	asp("skill-template-expansion", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		templates: [{
			name: "greet",
			description: "Greets",
			argumentHint: "name",
			content: "hello $1 and all: $ARGUMENTS",
			sourceInfo: { path: "/t/greet.md", source: "file", scope: "user", origin: "package" },
			filePath: "/t/greet.md",
		}],
		skills: [
			{
				name: "deploy",
				filePath: "/tmp/asp-skills/deploy.md",
				baseDir: "/tmp/asp-skills",
				content: "---\nname: deploy\n---\nRun the deploy steps.",
			},
			{
				name: "ghost",
				filePath: "/tmp/asp-skills/ghost.md",
				baseDir: "/tmp/asp-skills",
				content: "",
				readFails: true,
			},
		],
		script: [
			{ op: "prompt", text: "/skill:deploy prod", options: { preflightTag: "s1" } },
			{ op: "prompt", text: "/skill:ghost x" },
			{ op: "prompt", text: "/skill:missing y" },
			{ op: "prompt", text: "/greet world again" },
		],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg()), aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),
	// Image normalization (autoResize pinned false): a supported mime
	// passes through; an unsupported mime becomes an omission hint on the
	// prompt text.
	asp("image-normalization", {
		nowMs: ASP_NOW,
		config: { settings: { imageAutoResize: false }, model: aspModel(), entries: aspHeader() },
		script: [
			{
				op: "prompt",
				text: "look",
				options: {
					images: [
						{ type: "image", mimeType: "image/png", data: "aGVsbG8=" },
						{ type: "image", mimeType: "image/bmp", data: "aGVsbG8=" },
					],
				},
			},
		],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// Input-handler interception: "handled" swallows the prompt
	// (disposition handled, no run); "transform" rewrites the text and
	// images before the run.
	asp("input-handler-handled", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		inputHandler: { mode: "handled" },
		script: [
			{ op: "prompt", text: "eaten", options: { preflightTag: "h1" } },
			{ op: "steer", text: "also eaten" },
		],
		providerTurns: [],
	}),
	asp("input-handler-transform", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		inputHandler: { mode: "transform", text: "rewritten" },
		script: [{ op: "prompt", text: "original" }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// sendUserMessage (source "extension", no template expansion): a plain
	// string runs a turn; array content is folded to text + images.
	asp("send-user-message", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		script: [
			{ op: "sendUserMessage", content: "plain note" },
			{
				op: "sendUserMessage",
				content: [{ type: "text", text: "array note" }, { type: "text", text: "second line" }],
			},
		],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),
	// sendCustomMessage with triggerTurn: the custom message itself is the
	// run's starting message.
	asp("send-custom-message-trigger-turn", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		script: [
			{
				op: "sendCustomMessage",
				customType: "note",
				content: [{ type: "text", text: "ctx" }],
				display: true,
				details: { k: 1 },
				triggerTurn: true,
			},
		],
		providerTurns: [aspTurn(aspMsg())],
	}),

	// ------------------------------------------------------ agent-session-run
	// Tranche 11 part 2: the run orchestration slice (_startRun/_prepareRun/
	// _handlePostAgentRun/_runBeforeSettleBoundary, boundary drafts, abort)
	// over a real Agent with a scripted provider. extensionHandlers script
	// fake before_agent_start / turn_end / agent_before_settle / agent_settled
	// handlers; the script drives prompt/abort/waitForIdle ops; the result
	// records sessionEvents, entries, idleTimeline (settled vs waitIdle),
	// extensionEvents/Errors, and promptsSentToAgent.
	asr("missing-args", {}),
	// Happy path: one prompt runs a full turn; the system patch (preamble
	// from config.systemPrompt + cwd section) is sent to the provider, the
	// user + assistant messages are persisted, and the session settles.
	asr("run-happy-path", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		script: [{ op: "prompt", text: "hello there", options: { source: "test" } }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// A second prompt reuses the same system context: no new system patch
	// is emitted (cwd and loadout unchanged), so both provider prompts
	// carry the same system messages.
	asr("second-prompt-no-patch", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		script: [
			{ op: "prompt", text: "first prompt" },
			{ op: "prompt", text: "second prompt" },
		],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),
	// before_agent_start: an injected handler message becomes a custom
	// message alongside the prompt, and a systemPrompt force replaces the
	// preamble in the run's system patch.
	asr("before-agent-start-message-system-prompt", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: {
			before_agent_start: [
				{
					message: { customType: "note", content: [{ type: "text", text: "handler note" }], display: true },
				},
				{ systemPrompt: "Forced system preamble." },
			],
		},
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// Tool loadout edits: setActiveTools updates the live loadout (kept on
	// the next run when no explicit edit wins), while a selectedTools edit
	// on the options clone wins for that run only.
	asr("before-agent-start-tools", {
		nowMs: ASP_NOW,
		tools: {
			read: { name: "read", description: "Read files", parameters: { type: "object" } },
			bash: { name: "bash", description: "Run commands", parameters: { type: "object" } },
		},
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: {
			before_agent_start: [{ setActiveTools: ["read"] }, { selectedTools: ["bash"] }],
		},
		script: [
			{ op: "prompt", text: "hi" },
			{ op: "prompt", text: "again" },
		],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),
	// turn_end drafts: custom + custom_message entries are committed
	// (entry_appended events fire before agent_settled) and the boundary
	// context seen by handlers reflects the drafted entries.
	asr("turn-end-drafts", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: {
			turn_end: [
				{
					entries: [
						{ type: "custom", customType: "note", data: { k: 1 } },
						{
							type: "custom_message",
							customType: "ctx",
							content: [{ type: "text", text: "ctx note" }],
							display: true,
							details: { d: 2 },
						},
					],
				},
			],
		},
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// An invalid turn_end draft (context_edit targeting a missing entry)
	// invalidates the boundary: "Invalid boundary entries:" is reported via
	// emitError with extensionPath "<boundary>" and nothing is committed.
	asr("turn-end-invalid-draft", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: {
			turn_end: [
				{ entries: [{ type: "context_edit", targetId: "nope", replacement: { content: "x" } }] },
			],
		},
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// turn_end continue: a committed custom_message keeps the context
	// runnable, so the continue request drives an extra provider turn; the
	// abort in the same handler stops the loop after the boundary.
	asr("turn-end-continue-abort", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: {
			turn_end: [
				{
					entries: [{ type: "custom_message", customType: "ctx", content: [{ type: "text", text: "keep going" }] }],
					continue: true,
					abort: true,
				},
			],
		},
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// agent_before_settle: drafts commit before settling and a continue
	// request drives an extra provider turn when the context stays
	// runnable (retry disabled so the scripted exhaustion settles).
	asr("before-settle-continue-extra-turn", {
		nowMs: ASP_NOW,
		config: { settings: { retry: { enabled: false } }, model: aspModel(), entries: aspHeader() },
		extensionHandlers: {
			agent_before_settle: [
				{
					entries: [{ type: "custom_message", customType: "ctx", content: [{ type: "text", text: "more" }] }],
					continue: true,
					// A real handler is stateful and can continue only once;
					// the static spec needs `once` to model that (an
					// always-continue handler loops forever in TS too).
					once: true,
				},
			],
		},
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),
	// An abort requested from an agent_before_settle handler marks the
	// settle as aborted without another run.
	asr("before-settle-abort", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: { agent_before_settle: [{ abort: true }] },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg())],
	}),
	// An abort during before_agent_start cancels the run before the agent
	// is prompted (no provider turn consumed).
	asr("abort-during-before-agent-start", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: { before_agent_start: [{ abort: true }] },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [],
	}),
	// waitForIdle resolves immediately while idle; a prompt fired from an
	// agent_settled listener is deferred until the settle finishes and then
	// runs as a fresh turn (a later waitForIdle observes it).
	asr("wait-for-idle-deferred-prompt", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader() },
		extensionHandlers: { agent_settled: { prompt: "deferred follow up" } },
		script: [
			{ op: "waitForIdle" },
			{ op: "prompt", text: "first" },
			{ op: "waitForIdle" },
		],
		providerTurns: [aspTurn(aspMsg()), aspTurn(aspMsg())],
	}),

	// ------------------------------------------------------------- tui-utils
	// Differential parity for the pure width-engine functions of the TUI
	// package (layer 1 of the TUI parity plan; see docs/TUI_PORT_PLAN.md).
	{
		name: "tui-utils/visible-width-styled-ascii",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "\x1b[38;5;4mhello\x1b[39m world" },
	},
	{
		name: "tui-utils/visible-width-osc8-link",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "\x1b]8;;https://example.com\x07link\x1b]8;;\x07" },
	},
	{
		name: "tui-utils/visible-width-osc133-st",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "\x1b]133;A\x1b\\prompt\x1b]133;B\x1b\\" },
	},
	{
		name: "tui-utils/visible-width-apc-cursor",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "\x1b_pi:c\x07cursor" },
	},
	{
		name: "tui-utils/visible-width-tab-styled",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "\x1b[1ma\tb\x1b[22m" },
	},
	{
		name: "tui-utils/visible-width-cjk",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "\x1b[31m日本\x1b[39m ok" },
	},
	{
		name: "tui-utils/visible-width-unterminated-escapes",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "a\x1b[31" },
	},
	{
		name: "tui-utils/visible-width-emoji-clusters",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "a😀b👨‍👩‍👧‍👦🇺🇸🇺🇸한국어" },
	},
	{
		name: "tui-utils/visible-width-indic-conjuncts",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "र्क नेटवर्क র্ক ર્ક ର୍କ ర్క ര്‍ക सर्वाधिकार सुरक्षित।" },
	},
	{
		name: "tui-utils/visible-width-combining-marks",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "é čřžůú שָׁ بّ རྐ ᜠ᜴ 가〮 가〯 が" },
	},
	{
		name: "tui-utils/visible-width-myanmar",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "ကာ ကေ ကျ ကြ ကဳ ကဴ ကဵ ကး ကို က္" },
	},
	{
		name: "tui-utils/visible-width-thai-lao-am",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "ำ ຳ กำ ກຳ" },
	},
	{
		name: "tui-utils/visible-width-regional-indicators",
		tool: "tui-utils",
		args: { fn: "visibleWidth", text: "🇨🇳🇨 🇺🇸🇬🇧" },
	},
	{
		name: "tui-utils/normalize-terminal-output-thai-lao-am",
		tool: "tui-utils",
		args: { fn: "normalizeTerminalOutput", text: "ำabc ຳxyz กำ" },
	},
	{
		name: "tui-utils/normalize-terminal-output-tabs",
		tool: "tui-utils",
		args: {
			fn: "normalizeTerminalOutput",
			text: "\x1b]8;;https://example.test/a\tb\x07label\ttext\x1b]0;win\t dow\x1b\\\x1b_pay\tload\x1b\\",
		},
	},
	{
		name: "tui-utils/strip-terminal-sequences",
		tool: "tui-utils",
		args: {
			fn: "stripTerminalSequences",
			text: "\x1b[1mhi\x1b[22m there \x1b]8;;https://x\x07link\x1b]8;;\x07 \x1b_pi:c\x07cur",
		},
	},
	{
		name: "tui-utils/get-active-background-ansi",
		tool: "tui-utils",
		args: {
			fn: "getActiveBackgroundAnsi",
			text: "\x1b[1m\x1b[44mhi\x1b[39m there \x1b[48;5;240mcolored\x1b[0mtail",
		},
	},
	{
		name: "tui-utils/get-grapheme-cell-range",
		tool: "tui-utils",
		args: { fn: "getGraphemeCellRange", line: "a🙂b界", column: 1 },
	},
	{
		name: "tui-utils/get-grapheme-cell-range-boundary",
		tool: "tui-utils",
		args: { fn: "getGraphemeCellRange", line: "\x1b[31ma🙂b\x1b[0m", column: 2 },
	},
	{
		name: "tui-utils/get-grapheme-cell-range-past-end",
		tool: "tui-utils",
		args: { fn: "getGraphemeCellRange", line: "ab", column: 5 },
	},
	{
		name: "tui-utils/get-osc8-link-at-column",
		tool: "tui-utils",
		args: {
			fn: "getOsc8LinkAtColumn",
			line: "see \x1b]8;;https://example.com\x07the link\x1b]8;;\x07 and more",
			column: 10,
		},
	},
	{
		name: "tui-utils/get-osc8-link-at-column-outside",
		tool: "tui-utils",
		args: {
			fn: "getOsc8LinkAtColumn",
			line: "see \x1b]8;;https://example.com\x07the link\x1b]8;;\x07 and more",
			column: 1,
		},
	},
	{
		name: "tui-utils/truncate-basic",
		tool: "tui-utils",
		args: { fn: "truncateToWidth", text: "hello world this is long", maxWidth: 15 },
	},
	{
		name: "tui-utils/truncate-osc8-bel",
		tool: "tui-utils",
		args: {
			fn: "truncateToWidth",
			text: "\x1b]8;;https://example.com\x07some-longer-label-here\x1b]8;;\x07",
			maxWidth: 15,
		},
	},
	{
		name: "tui-utils/truncate-osc8-st",
		tool: "tui-utils",
		args: {
			fn: "truncateToWidth",
			text: "\x1b]8;;https://example.com\x1b\\some-longer-label-here\x1b]8;;\x1b\\",
			maxWidth: 15,
		},
	},
	{
		name: "tui-utils/truncate-ansi-reset-ellipsis",
		tool: "tui-utils",
		args: {
			fn: "truncateToWidth",
			text: `\x1b[31m${"hello ".repeat(100)}\x1b[0m`,
			maxWidth: 20,
			ellipsis: "…",
		},
	},
	{
		name: "tui-utils/truncate-wide-ellipsis",
		tool: "tui-utils",
		args: { fn: "truncateToWidth", text: "abcdef", maxWidth: 2, ellipsis: "🙂" },
	},
	{
		name: "tui-utils/truncate-pad-unicode",
		tool: "tui-utils",
		args: { fn: "truncateToWidth", text: "🙂界🙂界🙂界", maxWidth: 8, ellipsis: "…", pad: true },
	},
	{
		name: "tui-utils/truncate-contiguous-prefix-tab-apc",
		tool: "tui-utils",
		args: { fn: "truncateToWidth", text: "🙂\t界 \x1b_abc\x07", maxWidth: 7, ellipsis: "…", pad: true },
	},
	{
		name: "tui-utils/truncate-malformed-escape",
		tool: "tui-utils",
		args: {
			fn: "truncateToWidth",
			text: `abc\x1bnot-ansi ${"🙂".repeat(50)}`,
			maxWidth: 20,
			ellipsis: "…",
		},
	},
	{
		name: "tui-utils/truncate-empty-ellipsis",
		tool: "tui-utils",
		args: {
			fn: "truncateToWidth",
			text: `\x1b[31m${"hello".repeat(50)}`,
			maxWidth: 10,
			ellipsis: "",
		},
	},
	{
		name: "tui-utils/truncate-fits-with-wide-ellipsis",
		tool: "tui-utils",
		args: { fn: "truncateToWidth", text: "界", maxWidth: 2, ellipsis: "🙂" },
	},
	{
		name: "tui-utils/slice-by-column-ansi-order",
		tool: "tui-utils",
		args: { fn: "sliceByColumn", line: "\x1b[32mfoo\x1b[39m bar", startCol: 3, length: 4, strict: true },
	},
	{
		name: "tui-utils/slice-by-column-no-leak",
		tool: "tui-utils",
		args: {
			fn: "sliceByColumn",
			line: "Another \x1b[35malpha\x1b[39m line with \x1b[35mbeta\x1b[39m later.",
			startCol: 13,
			length: 100,
			strict: true,
		},
	},
	{
		name: "tui-utils/slice-with-width-tab",
		tool: "tui-utils",
		args: {
			fn: "sliceWithWidth",
			line: "out 192M\t.pi/skill-tests/results-ha",
			startCol: 0,
			length: 10,
			strict: true,
		},
	},
	{
		name: "tui-utils/slice-with-width-wide-strict",
		tool: "tui-utils",
		args: { fn: "sliceWithWidth", line: "ab🙂cd界ef", startCol: 2, length: 3, strict: true },
	},
	{
		name: "tui-utils/slice-with-width-wide-loose",
		tool: "tui-utils",
		args: { fn: "sliceWithWidth", line: "ab🙂cd界ef", startCol: 2, length: 3, strict: false },
	},
	{
		name: "tui-utils/extract-segments-tab",
		tool: "tui-utils",
		args: {
			fn: "extractSegments",
			line: "out 192M\t.pi/skill-tests/results-ha",
			beforeEnd: 10,
			afterStart: 13,
			afterLen: 10,
			strictAfter: true,
		},
	},
	{
		name: "tui-utils/extract-segments-tab-fits",
		tool: "tui-utils",
		args: {
			fn: "extractSegments",
			line: "out 192M\t.pi/skill-tests/results-ha",
			beforeEnd: 11,
			afterStart: 13,
			afterLen: 10,
			strictAfter: true,
		},
	},
	{
		name: "tui-utils/extract-segments-style-inherit",
		tool: "tui-utils",
		args: {
			fn: "extractSegments",
			line: "\x1b[31mred \x1b[44mbg tail\x1b[0m more",
			beforeEnd: 4,
			afterStart: 8,
			afterLen: 5,
			strictAfter: false,
		},
	},
	{
		name: "tui-utils/extract-segments-after-len-zero",
		tool: "tui-utils",
		args: {
			fn: "extractSegments",
			line: "abcdefghijk",
			beforeEnd: 3,
			afterStart: 5,
			afterLen: 0,
			strictAfter: false,
		},
	},
	{
		name: "tui-utils/wrap-cjk-breaks",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: "This is an example 中文汉字测试段落内容中文汉字测试段落内容.",
			width: 40,
		},
	},
	{
		name: "tui-utils/wrap-cjk-color",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: "\x1b[31mThis is an example 中文汉字测试段落内容中文汉字测试段落内容.\x1b[0m",
			width: 40,
		},
	},
	{
		name: "tui-utils/wrap-background-preserved",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: "\x1b[44mhello world this is blue background text\x1b[0m",
			width: 15,
		},
	},
	{
		name: "tui-utils/wrap-underline-bg",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: "\x1b[41mprefix \x1b[4mUNDERLINED_CONTENT_THAT_WRAPS\x1b[24m suffix\x1b[0m",
			width: 20,
		},
	},
	{
		name: "tui-utils/wrap-line-endings",
		tool: "tui-utils",
		args: { fn: "wrapTextWithAnsi", text: "\x1b[31mfirst\r\nsecond\rthird\x1b[0m", width: 80 },
	},
	{
		name: "tui-utils/wrap-osc8-bel-oauth",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: `\x1b]8;;https://example.com/oauth/${"a".repeat(32)}\x07https://example.com/oauth/${"a".repeat(32)}\x1b]8;;\x07`,
			width: 20,
		},
	},
	{
		name: "tui-utils/wrap-osc8-st",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: "\x1b]8;;https://example.com\x1b\\0123456789\x1b]8;;\x1b\\",
			width: 6,
		},
	},
	{
		name: "tui-utils/wrap-long-word-break",
		tool: "tui-utils",
		args: {
			fn: "wrapTextWithAnsi",
			text: "short \x1b[32maveryveryverylongwordthatmustbebroken\x1b[0m end",
			width: 12,
		},
	},
	{
		name: "tui-utils/wrap-trailing-whitespace",
		tool: "tui-utils",
		args: { fn: "wrapTextWithAnsi", text: "  ", width: 1 },
	},
	{
		name: "tui-utils/is-whitespace-char",
		tool: "tui-utils",
		args: { fn: "isWhitespaceChar", char: " " },
	},
	{
		name: "tui-utils/is-whitespace-char-nbsp",
		tool: "tui-utils",
		args: { fn: "isWhitespaceChar", char: " " },
	},
	{
		name: "tui-utils/is-punctuation-char",
		tool: "tui-utils",
		args: { fn: "isPunctuationChar", char: "~" },
	},

	// ------------------------------------------------------------- tui-fuzzy
	{
		name: "tui-fuzzy/match-empty-query",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "", text: "anything" },
	},
	{
		name: "tui-fuzzy/match-exact",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "test", text: "test" },
	},
	{
		name: "tui-fuzzy/match-scattered-vs-consecutive",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "foo", text: "f_o_o_bar" },
	},
	{
		name: "tui-fuzzy/match-out-of-order",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "abc", text: "cba" },
	},
	{
		name: "tui-fuzzy/match-word-boundary",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "fb", text: "foo-bar" },
	},
	{
		name: "tui-fuzzy/match-swapped-alphanumeric",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "codex52", text: "gpt-5.2-codex" },
	},
	{
		name: "tui-fuzzy/match-swapped-numeric-alpha",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "52codex", text: "codex-52" },
	},
	{
		name: "tui-fuzzy/match-case-insensitive",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "ABC", text: "aXbXc" },
	},
	{
		name: "tui-fuzzy/match-unicode-lowercase",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "ÉCOLE", text: "école" },
	},
	{
		name: "tui-fuzzy/match-greek-final-sigma",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyMatch", query: "ΑΣ", text: "δας" },
	},
	{
		name: "tui-fuzzy/filter-sorts-by-quality",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyFilter", items: ["a_p_p", "app", "application"], query: "app" },
	},
	{
		name: "tui-fuzzy/filter-empty-query",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyFilter", items: ["apple", "banana", "cherry"], query: "" },
	},
	{
		name: "tui-fuzzy/filter-slash-tokens",
		tool: "tui-fuzzy",
		args: {
			fn: "fuzzyFilter",
			items: [{ id: "gpt-5.5", provider: "openai-codex", text: "gpt-5.5 openai-codex" }],
			query: "openai-codex/gpt-5.5",
			textField: "text",
		},
	},
	{
		name: "tui-fuzzy/filter-text-field",
		tool: "tui-fuzzy",
		args: {
			fn: "fuzzyFilter",
			items: [
				{ name: "foo", id: 1, text: "foo" },
				{ name: "bar", id: 2, text: "bar" },
			],
			query: "foo",
			textField: "text",
		},
	},
	{
		name: "tui-fuzzy/filter-no-match",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyFilter", items: ["apple", "banana"], query: "zzz" },
	},
	{
		name: "tui-fuzzy/filter-all-tokens-must-match",
		tool: "tui-fuzzy",
		args: { fn: "fuzzyFilter", items: ["alpha beta", "alpha gamma"], query: "alpha beta" },
	},

	// --------------------------------------------------------- tui-kill-ring
	{
		name: "tui-kill-ring/push-peek-rotate",
		tool: "tui-kill-ring",
		args: {
			ops: [
				{ op: "push", text: "hello", prepend: false },
				{ op: "push", text: "world", prepend: false },
				{ op: "peek" },
				{ op: "rotate" },
				{ op: "peek" },
				{ op: "length" },
			],
		},
	},
	{
		name: "tui-kill-ring/accumulate-append",
		tool: "tui-kill-ring",
		args: {
			ops: [
				{ op: "push", text: "hello", prepend: false },
				{ op: "push", text: " world", prepend: false, accumulate: true },
				{ op: "peek" },
				{ op: "length" },
			],
		},
	},
	{
		name: "tui-kill-ring/accumulate-prepend",
		tool: "tui-kill-ring",
		args: {
			ops: [
				{ op: "push", text: "world", prepend: false },
				{ op: "push", text: "hello ", prepend: true, accumulate: true },
				{ op: "peek" },
			],
		},
	},
	{
		name: "tui-kill-ring/empty-push-and-peek",
		tool: "tui-kill-ring",
		args: { ops: [{ op: "push", text: "", prepend: false }, { op: "peek" }, { op: "length" }] },
	},

	// ---------------------------------------------------- tui-program-status
	{
		name: "tui-program-status/format-blocked",
		tool: "tui-program-status",
		args: {
			fn: "formatProgramStatus",
			state: "blocked",
			app: "pi",
			kind: "permission",
			message: "Allow bash?",
		},
	},
	{
		name: "tui-program-status/format-clear",
		tool: "tui-program-status",
		args: { fn: "formatProgramStatus", state: "clear" },
	},
	{
		name: "tui-program-status/format-omits-invalid",
		tool: "tui-program-status",
		args: { fn: "formatProgramStatus", state: "working", app: "my app", kind: "auth", message: " \n " },
	},
	{
		name: "tui-program-status/format-app-boundary",
		tool: "tui-program-status",
		args: { fn: "formatProgramStatus", state: "idle", app: "a".repeat(32) },
	},
	{
		name: "tui-program-status/format-app-too-long",
		tool: "tui-program-status",
		args: { fn: "formatProgramStatus", state: "idle", app: "a".repeat(33) },
	},
	{
		name: "tui-program-status/format-control-chars",
		tool: "tui-program-status",
		args: {
			fn: "formatProgramStatus",
			state: "error",
			message: "first\nsecond\x1b[31mthird\t",
		},
	},
	{
		name: "tui-program-status/format-long-utf8-message",
		tool: "tui-program-status",
		args: { fn: "formatProgramStatus", state: "working", app: "pi", message: "é".repeat(2000) },
	},
	{
		name: "tui-program-status/reply-query-st",
		tool: "tui-program-status",
		args: { fn: "isProgramStatusReply", sequence: "\x1b]7501;?\x1b\\" },
	},
	{
		name: "tui-program-status/reply-query-bel",
		tool: "tui-program-status",
		args: { fn: "isProgramStatusReply", sequence: "\x1b]7501;?\x07" },
	},
	{
		name: "tui-program-status/reply-future-pairs",
		tool: "tui-program-status",
		args: { fn: "isProgramStatusReply", sequence: "\x1b]7501;?version=2\x1b\\" },
	},
	{
		name: "tui-program-status/reply-state-report",
		tool: "tui-program-status",
		args: { fn: "isProgramStatusReply", sequence: "\x1b]7501;state=idle\x1b\\" },
	},
	{
		name: "tui-program-status/reply-osc11",
		tool: "tui-program-status",
		args: { fn: "isProgramStatusReply", sequence: "\x1b]11;rgb:0000/0000/0000\x07" },
	},

	// ------------------------------------------------------------ tui-colors
	{
		name: "tui-colors/parse-hex-3",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "#abc" },
	},
	{
		name: "tui-colors/parse-hex-6",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "#4f8eb3" },
	},
	{
		name: "tui-colors/parse-oklch-percent",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "oklch(62% 0.1 200)" },
	},
	{
		name: "tui-colors/parse-oklch-plain",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "oklch(0.5 .1 +30)" },
	},
	{
		name: "tui-colors/parse-oklch-deg-exp",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "OKLCH( +.5e0  1E-1  30DEG )" },
	},
	{
		name: "tui-colors/parse-okhsl",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "okhsl(29.23 100% 56.8%)" },
	},
	{
		name: "tui-colors/parse-okhsl-deg",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "OKHSL(250deg 60% 55%)" },
	},
	{
		name: "tui-colors/parse-number-indexed",
		tool: "tui-colors",
		args: { fn: "parseColor", value: 9 },
	},
	{
		name: "tui-colors/parse-invalid",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "red" },
	},
	{
		name: "tui-colors/parse-empty",
		tool: "tui-colors",
		args: { fn: "parseColor", value: "" },
	},
	{
		name: "tui-colors/indexed-valid",
		tool: "tui-colors",
		args: { fn: "indexedColor", index: 240 },
	},
	{
		name: "tui-colors/indexed-out-of-range",
		tool: "tui-colors",
		args: { fn: "indexedColor", index: 300 },
	},
	{
		name: "tui-colors/indexed-fractional",
		tool: "tui-colors",
		args: { fn: "indexedColor", index: 1.5 },
	},
	{
		name: "tui-colors/rgb-valid",
		tool: "tui-colors",
		args: { fn: "rgbColor", r: 18, g: 52, b: 86 },
	},
	{
		name: "tui-colors/rgb-out-of-range",
		tool: "tui-colors",
		args: { fn: "rgbColor", r: 0, g: 0, b: 256 },
	},
	{
		name: "tui-colors/oklch-invalid-lightness",
		tool: "tui-colors",
		args: { fn: "oklchColor", l: 1.5, c: 0.1, h: 30 },
	},
	{
		name: "tui-colors/oklch-negative-chroma",
		tool: "tui-colors",
		args: { fn: "oklchColor", l: 0.5, c: -0.1, h: 30 },
	},
	{
		name: "tui-colors/oklch-normalizes-negative-hue",
		tool: "tui-colors",
		args: { fn: "oklchColor", l: 0.5, c: 0.1, h: -30 },
	},
	{
		name: "tui-colors/okhsl-invalid-saturation",
		tool: "tui-colors",
		args: { fn: "okhslColor", h: 250, s: 1.6, l: 0.55 },
	},
	{
		name: "tui-colors/okhsl-red-cusp",
		tool: "tui-colors",
		args: { fn: "okhslColor", h: 29.23, s: 1, l: 0.568 },
	},
	{
		name: "tui-colors/rgb-from-oklch-red",
		tool: "tui-colors",
		args: { fn: "colorToRgb", color: { kind: "oklch", l: 0.627955, c: 0.257683, h: 29.2339 } },
	},
	{
		name: "tui-colors/rgb-from-oklch-white-limit",
		tool: "tui-colors",
		args: { fn: "colorToRgb", color: { kind: "oklch", l: 1, c: 0.3, h: 150 } },
	},
	{
		name: "tui-colors/rgb-from-oklch-black-limit",
		tool: "tui-colors",
		args: { fn: "colorToRgb", color: { kind: "oklch", l: 0, c: 0.3, h: 150 } },
	},
	{
		name: "tui-colors/rgb-from-indexed-cube",
		tool: "tui-colors",
		args: { fn: "colorToRgb", color: { kind: "indexed", index: 200 } },
	},
	{
		name: "tui-colors/rgb-from-indexed-gray",
		tool: "tui-colors",
		args: { fn: "colorToRgb", color: { kind: "indexed", index: 240 } },
	},
	{
		name: "tui-colors/rgb-from-indexed-basic",
		tool: "tui-colors",
		args: { fn: "colorToRgb", color: { kind: "indexed", index: 3 } },
	},
	{
		name: "tui-colors/oklch-roundtrip-blue",
		tool: "tui-colors",
		args: { fn: "colorToOklch", color: "#00ff00" },
	},
	{
		name: "tui-colors/oklch-of-indexed",
		tool: "tui-colors",
		args: { fn: "colorToOklch", color: { kind: "indexed", index: 1 } },
	},
	{
		name: "tui-colors/okhsl-of-hex",
		tool: "tui-colors",
		args: { fn: "colorToOkhsl", color: "#4f8eb3" },
	},
	{
		name: "tui-colors/okhsl-of-dark-hex",
		tool: "tui-colors",
		args: { fn: "colorToOkhsl", color: "#20242a" },
	},
	{
		name: "tui-colors/okhsl-of-gray",
		tool: "tui-colors",
		args: { fn: "colorToOkhsl", color: "#808080" },
	},
	{
		name: "tui-colors/hex-of-oklch",
		tool: "tui-colors",
		args: { fn: "colorToHex", color: { kind: "oklch", l: 0.7, c: 0.15, h: 200 } },
	},
	{
		name: "tui-colors/hex-of-indexed",
		tool: "tui-colors",
		args: { fn: "colorToHex", color: { kind: "indexed", index: 9 } },
	},
	{
		name: "tui-colors/hex-of-hex",
		tool: "tui-colors",
		args: { fn: "colorToHex", color: "#4f8eb3" },
	},
	{
		name: "tui-colors/mix-oklch-half",
		tool: "tui-colors",
		args: {
			fn: "mixColors",
			first: "#000000",
			second: "#ffffff",
			amount: 0.5,
			space: "oklch",
		},
	},
	{
		name: "tui-colors/mix-srgb-half",
		tool: "tui-colors",
		args: {
			fn: "mixColors",
			first: "#000000",
			second: "#ffffff",
			amount: 0.5,
			space: "srgb",
		},
	},
	{
		name: "tui-colors/mix-oklch-default-space",
		tool: "tui-colors",
		args: { fn: "mixColors", first: "#ff0000", second: "#0000ff", amount: 0.25 },
	},
	{
		name: "tui-colors/mix-oklch-complement-hue",
		tool: "tui-colors",
		args: { fn: "mixColors", first: "#ff0000", second: "#00ff00", amount: 0.5 },
	},
	{
		name: "tui-colors/mix-amount-out-of-range",
		tool: "tui-colors",
		args: { fn: "mixColors", first: "#000000", second: "#ffffff", amount: 1.5 },
	},
	{
		name: "tui-colors/foreground-truecolor",
		tool: "tui-colors",
		args: { fn: "foregroundAnsi", color: { kind: "rgb", r: 18, g: 52, b: 86 }, mode: "truecolor" },
	},
	{
		name: "tui-colors/foreground-256-rgb",
		tool: "tui-colors",
		args: { fn: "foregroundAnsi", color: { kind: "rgb", r: 18, g: 52, b: 86 }, mode: "256color" },
	},
	{
		name: "tui-colors/foreground-indexed",
		tool: "tui-colors",
		args: { fn: "foregroundAnsi", color: { kind: "indexed", index: 9 }, mode: "256color" },
	},
	{
		name: "tui-colors/background-truecolor",
		tool: "tui-colors",
		args: { fn: "backgroundAnsi", color: "#4f8eb3", mode: "truecolor" },
	},
	{
		name: "tui-colors/background-256-gray",
		tool: "tui-colors",
		args: { fn: "backgroundAnsi", color: "#808080", mode: "256color" },
	},
	{
		name: "tui-colors/background-256-oklch",
		tool: "tui-colors",
		args: { fn: "backgroundAnsi", color: { kind: "oklch", l: 0.4, c: 0.1, h: 250 }, mode: "256color" },
	},
	{
		name: "tui-colors/style-text-truecolor",
		tool: "tui-colors",
		args: {
			fn: "styleText",
			text: "Ready",
			style: {
				fg: { kind: "rgb", r: 18, g: 52, b: 86 },
				bg: { kind: "indexed", index: 9 },
				bold: true,
				italic: true,
			},
			mode: "truecolor",
		},
	},
	{
		name: "tui-colors/style-text-256",
		tool: "tui-colors",
		args: {
			fn: "styleText",
			text: "Ready",
			style: { fg: { kind: "rgb", r: 18, g: 52, b: 86 } },
			mode: "256color",
		},
	},
	{
		name: "tui-colors/style-text-all-attributes",
		tool: "tui-colors",
		args: {
			fn: "styleText",
			text: "x",
			style: {
				bold: true,
				dim: true,
				italic: true,
				underline: true,
				inverse: true,
				strikethrough: true,
			},
			mode: "truecolor",
		},
	},
	{
		name: "tui-colors/style-text-with-ansi",
		tool: "tui-colors",
		args: {
			fn: "styleTextWithAnsi",
			text: "hi",
			fgAnsi: "\x1b[38;5;240m",
			bgAnsi: "\x1b[48;5;17m",
			options: { underline: true },
		},
	},
	{
		name: "tui-colors/style-text-invalid-fg",
		tool: "tui-colors",
		args: { fn: "styleText", text: "x", style: { fg: "red" }, mode: "truecolor" },
	},

	// --------------------------------------------------- tui-terminal-colors
	{
		name: "tui-terminal-colors/osc10-rgb",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]10;rgb:ffff/ffff/ffff\x07" },
	},
	{
		name: "tui-terminal-colors/osc4-hex-st",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]4;13;#ff0080\x1b\\" },
	},
	{
		name: "tui-terminal-colors/osc4-bogus",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]4;1;bogus\x07" },
	},
	{
		name: "tui-terminal-colors/osc12-not-color",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]12;#ffffff\x07" },
	},
	{
		name: "tui-terminal-colors/osc11-12bit",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]11;rgb:eaea/6c6c/6d6d\x07" },
	},
	{
		name: "tui-terminal-colors/osc10-hex12",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]10;#aabbccddeeff\x1b\\" },
	},
	{
		name: "tui-terminal-colors/osc10-bad-hex",
		tool: "tui-terminal-colors",
		args: { fn: "parseOscColorResponse", data: "\x1b]10;#aabbccdd\x07" },
	},
	{
		name: "tui-terminal-colors/scheme-dark",
		tool: "tui-terminal-colors",
		args: { fn: "parseTerminalColorSchemeReport", data: "\x1b[?997;1n" },
	},
	{
		name: "tui-terminal-colors/scheme-light",
		tool: "tui-terminal-colors",
		args: { fn: "parseTerminalColorSchemeReport", data: "\x1b[?997;2n" },
	},
	{
		name: "tui-terminal-colors/scheme-last-wins-dark",
		tool: "tui-terminal-colors",
		args: { fn: "parseTerminalColorSchemeReport", data: "\x1b[?997;2n\x1b[?997;1n\x1b[?997;1n" },
	},
	{
		name: "tui-terminal-colors/scheme-last-wins-light",
		tool: "tui-terminal-colors",
		args: { fn: "parseTerminalColorSchemeReport", data: "\x1b[?997;1n\x1b[?997;2n\x1b[?997;2n" },
	},
	{
		name: "tui-terminal-colors/scheme-unknown-value",
		tool: "tui-terminal-colors",
		args: { fn: "parseTerminalColorSchemeReport", data: "\x1b[?997;3n" },
	},
	{
		name: "tui-terminal-colors/scheme-prefixed-junk",
		tool: "tui-terminal-colors",
		args: { fn: "parseTerminalColorSchemeReport", data: "x\x1b[?997;1n" },
	},

	// math: bit-level parity of the V8 (fdlibm-derived) Math.* ports used by
	// the color pipeline. Results are hex f64 bit patterns on both sides.
	{
		name: "math/basic",
		tool: "math",
		args: {
			ops: [
				{ fn: "atan2", args: [1, 2] },
				{ fn: "atan2", args: [-1, 2] },
				{ fn: "atan2", args: [1, -2] },
				{ fn: "atan2", args: [0.4809345314400326, 0.02897262682471269] },
				{ fn: "atan2", args: [0.6141707790052229, -0.5202806468753018] },
				{ fn: "atan2", args: [1.9750481795831567e-306, 4.2100188968685646e-297] },
				{ fn: "atan2", args: [9.609330007667106e-303, 4.2100188968685646e-297] },
				{ fn: "sin", args: [1.2957409547783927] },
				{ fn: "sin", args: [2.3861895747392154e-307] },
				{ fn: "sin", args: [-5.420249662451331] },
				{ fn: "cos", args: [0.9648986460818719] },
				{ fn: "cos", args: [3.9980273879569133] },
				{ fn: "cos", args: [5.420249662451331] },
				{ fn: "cbrt", args: [-0.7372079239367317] },
				{ fn: "cbrt", args: [1.5563728631690855e-298] },
				{ fn: "cbrt", args: [8] },
				{ fn: "cbrt", args: [0.001] },
				{ fn: "hypot", args: [0.13336238251905452, 0.42350188175877623] },
				{ fn: "hypot", args: [-0.12249456161475236, -0.38605417966635047] },
				{ fn: "hypot", args: [0.40228411685041243, 0.10167298157183269] },
				{ fn: "sqrt", args: [2] },
				{ fn: "pow", args: [0.5, 1 / 2.4] },
				{ fn: "pow", args: [0.5, 2.4] },
				// Math.log: V8's fdlibm port differs from the system libm's
				// ln for ~0.7% of inputs by exactly 1 ulp — these values are
				// known divergence vectors (js_math::js_math_log).
				{ fn: "log", args: [1.6] },
				{ fn: "log", args: [1 + (3 - 1 + 0.5) / (1 + 0.5)] },
				{ fn: "log", args: [3] },
				{ fn: "log", args: [10] },
				{ fn: "log", args: [0.5] },
				{ fn: "log", args: [5e-324] },
				{ fn: "log", args: [2.2250738585072014e-308] },
				{ fn: "log", args: [Math.pow(2, -1023) * 1.5] },
				{ fn: "log", args: [1.0000000000000002] },
				{ fn: "log", args: [0.9999999999999999] },
			],
		},
	},
	// --------------------------------------------------- tui-word-navigation
	// Word-granularity segmentation (Intl.Segmenter) + cursor motion
	// (word-navigation.ts). The Rust side executes the offline ICU 78.3 port;
	// the TS side runs against Node's live Intl.Segmenter.
	{
		name: "tui-word-navigation/segment-words-ascii",
		tool: "tui-word-navigation",
		args: {
			fn: "segmentWords",
			texts: [
				"",
				" ",
				"hello world",
				"foo.bar baz",
				"don't stop",
				"num 123 45.67 -8 +9 1e10 0x1F",
				"a-b_c d.e f,g",
				"(parens) [brackets] {braces} <angles>",
				"path/to/file.txt",
				"!!!???...,,,;;;:::",
				"---===+++***###",
				"MIXEDcase MiXeD ALLCAPS lowercase",
				"\t\ttabs\nnewlines\r\r\n",
				"nbsp\u00a0and\u00a0en\u2013dash\u2014em",
			],
		},
	},
	{
		name: "tui-word-navigation/segment-words-cjk",
		tool: "tui-word-navigation",
		args: {
			fn: "segmentWords",
			texts: [
				"你好世界 test",
				"漢字かな交じり文",
				"日本語のテキストです",
				"カタカナ ひらがな 漢字",
				"ァィゥェォ ガギグゲゴ パピプペポ",
				"ｱｲｳｴｵ ﾊﾞｲﾄ ﾊﾟｿｺﾝ",
				"〆切 々惜 〻",
				"韓国語 한글 텍스트",
				"한국어와 English 혼용",
				"令和3年5月15日",
				"第1234章",
				"𠮷野家",
				"𩸽 ほっけ",
				"𐌰𐌱𐌲 Gothic",
			],
		},
	},
	{
		name: "tui-word-navigation/segment-words-sea",
		tool: "tui-word-navigation",
		args: {
			fn: "segmentWords",
			texts: [
				"สวัสดีครับ",
				"ทดสอบการตัดคำภาษาไทย",
				"กรุงเทพมหานคร",
				"ภาษาไทยเป็นภาษาที่ไม่มีช่องว่างระหว่างคำ",
				"ไม่มีข้อมูล ๆ ฯลฯ",
				"ครับๆ ไปๆมาๆ",
				"สวัสดี, ครับ!",
				"สระอำ นำ้ ก้ำกเี๋ยว",
				"ສະບາຍດີ",
				"ພາສາລາວບໍ່ມີຊ່ອງຫວ່າງ",
				"ວຽງຈັນ",
				"ລາວດີ",
				"សួស្តី",
				"ភាសាខ្មែរ",
				"ក្រុងភ្នំពេញ",
				"ការប្រើប្រាស់ពាក្យ",
				"មិនទាន់ប្រាកដ",
				"မင်္ဂလာပါ",
				"မြန်မာဘာသာစကား",
				"ရန်ကုန်မြို့",
				"ကွန်ပျူတာ",
			],
		},
	},
	{
		name: "tui-word-navigation/segment-words-mixed",
		tool: "tui-word-navigation",
		args: {
			fn: "segmentWords",
			texts: [
				"ไทย English 漢字",
				"กรุงเทพ Bangkok 東京",
				"Thai123 ไทย123",
				"漢字123 かな456",
				"emoji 👍 test",
				"👨‍👩‍👧‍👦 family ZWJ",
				"👍🏽 skin tone",
				"🇺🇸 flag",
				"1️⃣2️⃣3️⃣ keycaps",
				"e\u0301 é e\u0301\u0302",
				"a\u0300\u0301b",
				"각 jamo กำ กึ กี",
				"a\u200Db",
				"「日本語」『カタカナ』",
				"한국,어.",
			],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-ascii",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "hello world foo.bar",
			cursors: [0, 1, 2, 5, 6, 10, 11, 12, 15, 16, 19, 20, 21],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-ascii",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "hello world foo.bar",
			cursors: [0, 1, 2, 5, 6, 10, 11, 12, 15, 16, 19, 20, 21],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-punct",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "foo...bar, baz!",
			cursors: [0, 1, 3, 4, 5, 6, 7, 9, 10, 11, 13, 14, 15],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-punct",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "foo...bar, baz!",
			cursors: [0, 1, 3, 4, 5, 6, 7, 9, 10, 11, 13, 14, 15],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-whitespace",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "   spaced   out   ",
			cursors: [0, 1, 2, 3, 4, 9, 10, 11, 12, 13, 15, 17, 18],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-whitespace",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "   spaced   out   ",
			cursors: [0, 1, 2, 3, 4, 9, 10, 11, 12, 13, 15, 17, 18],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-cjk",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "你好世界 test 漢字",
			cursors: [0, 1, 2, 3, 4, 5, 8, 9, 10, 12, 13, 14, 15],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-cjk",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "你好世界 test 漢字",
			cursors: [0, 1, 2, 3, 4, 5, 8, 9, 10, 12, 13, 14, 15],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-thai",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "สวัสดีครับ foo",
			cursors: [0, 1, 4, 8, 11, 12, 13, 15, 16],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-thai",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "สวัสดีครับ foo",
			cursors: [0, 1, 4, 8, 11, 12, 13, 15, 16],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-katakana",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "カタカナテスト",
			cursors: [0, 1, 2, 3, 4, 6, 8, 10, 12, 14],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-katakana",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "カタカナテスト",
			cursors: [0, 1, 2, 3, 4, 6, 8, 10, 12, 14],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-atomic",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "hello <paste> world",
			cursors: [0, 1, 5, 6, 7, 12, 13, 14, 19, 20, 21],
			atomic: ["<paste>"],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-atomic",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "hello <paste> world",
			cursors: [0, 1, 5, 6, 7, 12, 13, 14, 19, 20, 21],
			atomic: ["<paste>"],
		},
	},
	{
		name: "tui-word-navigation/find-word-backward-emoji",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordBackward",
			text: "👍🏽 ok 👨‍👩‍👧‍👦",
			cursors: [0, 1, 2, 4, 5, 7, 8, 9, 14, 15, 16, 17, 18],
		},
	},
	{
		name: "tui-word-navigation/find-word-forward-emoji",
		tool: "tui-word-navigation",
		args: {
			fn: "findWordForward",
			text: "👍🏽 ok 👨‍👩‍👧‍👦",
			cursors: [0, 1, 2, 4, 5, 7, 8, 9, 14, 15, 16, 17, 18],
		},
	},
	{
		name: "math/large-argument-reduction",
		tool: "math",
		args: {
			ops: [
				// Values that exercise __ieee754_rem_pio2's medium and large
				// branches, where fdlibm and glibc diverge the most.
				{ fn: "sin", args: [1000003.141592653589793] },
				{ fn: "cos", args: [1000003.141592653589793] },
				{ fn: "sin", args: [1e15] },
				{ fn: "cos", args: [1e15] },
				{ fn: "sin", args: [1e17] },
				{ fn: "cos", args: [1e17] },
				{ fn: "sin", args: [1e100] },
				{ fn: "cos", args: [1e300] },
				{ fn: "sin", args: [-2.5e20] },
				{ fn: "cos", args: [-2.5e20] },
				{ fn: "sin", args: [3.141592653589793] },
				{ fn: "cos", args: [1.5707963267948966] },
				{ fn: "atan2", args: [1e-300, 1e-310] },
				{ fn: "cbrt", args: [1e-300] },
				{ fn: "cbrt", args: [4.9e-324] },
			],
		},
	},
	// --------------------------------------------------- tui-keys
	// Differential parity for the pure key parser: TS packages/tui/src/keys.ts
	// vs Rust pi_tui::keys. Each sub-case = { input, kitty, keyIds? }; both
	// harnesses evaluate parseKey / matchesKey (per keyIds) /
	// decodePrintableKey / decodeKittyPrintable / isKeyRelease / isKeyRepeat
	// with the global kitty-protocol flag set from `kitty` (reset between
	// sub-cases). undefined serializes as null on both sides.
	{
		name: "tui-keys/kitty-csi-u",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b[107;9u", true, ["super+k"]),
			kc("\u001b[13;9u", true, ["super+enter"]),
			kc("\u001b[107;13u", true, ["ctrl+super+k","super+k"]),
			kc("\u001b[107;14u", true, ["ctrl+shift+super+k"]),
			kc("\u001b[49u", true, ["1"]),
			kc("\u001b[49;5u", true, ["ctrl+1","ctrl+2"]),
			kc("\u001b[57400u", true, ["1"]),
			kc("\u001b[57410u", true, ["/"]),
			kc("\u001b[57417u", true, ["left"]),
			kc("\u001b[57426u", true, ["delete"]),
			kc("\u001b[57399u", true),
			kc("\u001b[57409u", true),
			kc("\u001b[57413u", true),
			kc("\u001b[57416u", true),
			kc("\u001b[57418u", true),
			kc("\u001b[57419u", true),
			kc("\u001b[57420u", true),
			kc("\u001b[57421u", true),
			kc("\u001b[57422u", true),
			kc("\u001b[57423u", true),
			kc("\u001b[57424u", true),
			kc("\u001b[57425u", true),
			kc("\u001b[61:43;2u", true, ["+","shift+="]),
			kc("\u001b[61:43;6u", true, ["ctrl++"]),
			kc("\n", true, ["shift+enter","enter"]),
			kc("\u001b ", true, ["alt+space"]),
			kc("\u001b\b", true, ["alt+backspace"]),
			kc("\u001b\u0003", true, ["ctrl+alt+c"]),
			kc("\u001bB", true, ["alt+left"]),
			kc("\u001bF", true, ["alt+right"]),
			kc("\u001ba", true, ["alt+a"]),
			kc("\u001b1", true, ["alt+1"]),
			kc("\u001b,", true, ["alt+,"]),
			kc("\u001b.", true, ["alt+."]),
			kc("\u001by", true, ["alt+y"]),
			kc("\u001b[69;2u", true, ["shift+e"]),
			kc("\u001b[99;17u", true),
			kc("\u001b[97u", true, ["a","shift+a"]),
			kc("\u001b[97;5u", true, ["ctrl+a","a"]),
			kc("\u001b[99;5u", true, ["ctrl+c","ctrl+d"]),
			kc("\u001b[69;2u", true, ["shift+e","e"]),
			kc("\u001b[49u", true, ["1"]),
			kc("\u001b[49;5u", true, ["ctrl+1","ctrl+2"]),
			kc("\u001b[107;9u", true, ["super+k"]),
			kc("\u001b[107;13u", true, ["ctrl+super+k","super+k"]),
			kc("\u001b[13;9u", true, ["super+enter"]),
			kc("\u001b[57400u", true, ["1"]),
			kc("\u001b[57417u", true, ["left"]),
			],
		},
	},
	{
		name: "tui-keys/modify-other-keys",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b[27;5;99~", false, ["ctrl+c"]),
			kc("\u001b[27;5;100~", false, ["ctrl+d"]),
			kc("\u001b[27;5;122~", false, ["ctrl+z"]),
			kc("\u001b[27;5;13~", false, ["ctrl+enter"]),
			kc("\u001b[27;2;13~", false, ["shift+enter"]),
			kc("\u001b[27;3;13~", false, ["alt+enter"]),
			kc("\u001b[27;2;9~", false, ["shift+tab"]),
			kc("\u001b[27;5;9~", false, ["ctrl+tab"]),
			kc("\u001b[27;3;9~", false, ["alt+tab"]),
			kc("\u001b[27;1;127~", false, ["backspace"]),
			kc("\u001b[27;5;127~", false, ["ctrl+backspace"]),
			kc("\u001b[27;3;127~", false, ["alt+backspace"]),
			kc("\u001b[27;1;27~", false, ["escape"]),
			kc("\u001b[27;1;32~", false, ["space"]),
			kc("\u001b[27;5;32~", false, ["ctrl+space"]),
			kc("\u001b[27;5;47~", false, ["ctrl+/"]),
			kc("\u001b[27;6;43~", false, ["ctrl++"]),
			kc("\u001b[27;5;49~", false, ["ctrl+1"]),
			kc("\u001b[27;2;49~", false, ["shift+1"]),
			kc("\u001b[27;2;69~", false, ["shift+e"]),
			kc("\u001b[27;6;69~", false, ["ctrl+shift+e"]),
			kc("\u001b[27;7;104~", false, ["ctrl+alt+h"]),
			kc("\u001b[27;2;196~"),
			kc("\u001b[27;2;32~"),
			],
		},
	},
	{
		name: "tui-keys/legacy-chars",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u0003", false, ["ctrl+c"]),
			kc("\u0004", false, ["ctrl+d"]),
			kc("\u001b", false, ["escape"]),
			kc("\n", false, ["enter"]),
			kc("\u0000", false, ["ctrl+space"]),
			kc("\u001c", false, ["ctrl+\\"]),
			kc("\u001d", false, ["ctrl+]"]),
			kc("\u001f", false, ["ctrl+_","ctrl+-"]),
			kc("", false, ["backspace","ctrl+backspace"]),
			kc("\b", false, ["backspace","ctrl+backspace","ctrl+h"]),
			kc("\t"),
			kc("\r"),
			kc(" "),
			kc("1", false, ["1"]),
			kc("+", false, ["+"]),
			],
		},
	},
	{
		name: "tui-keys/legacy-alt-prefixed",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b\u001b", false, ["ctrl+alt+["]),
			kc("\u001b\u001c", false, ["ctrl+alt+\\"]),
			kc("\u001b\u001d", false, ["ctrl+alt+]"]),
			kc("\u001b\u001f", false, ["ctrl+alt+_","ctrl+alt+-"]),
			kc("\u001b ", false, ["alt+space"]),
			kc("\u001b\b", false, ["alt+backspace"]),
			kc("\u001b\u0003", false, ["ctrl+alt+c"]),
			kc("\u001bB", false, ["alt+left"]),
			kc("\u001bF", false, ["alt+right"]),
			kc("\u001ba", false, ["alt+a"]),
			kc("\u001b1", false, ["alt+1"]),
			kc("\u001b,", false, ["alt+,"]),
			kc("\u001b.", false, ["alt+."]),
			kc("\u001by", false, ["alt+y"]),
			kc("\u001bz", false, ["alt+z"]),
			kc("\u001bp", false, ["alt+up","up"]),
			],
		},
	},
	{
		name: "tui-keys/legacy-alt-prefixed-kitty-on",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b\u001b", true, ["ctrl+alt+["]),
			kc("\u001b\u001c", true, ["ctrl+alt+\\"]),
			kc("\u001b\u001d", true, ["ctrl+alt+]"]),
			kc("\u001b\u001f", true, ["ctrl+alt+_","ctrl+alt+-"]),
			kc("\u001b ", true, ["alt+space"]),
			kc("\u001b\b", true, ["alt+backspace"]),
			kc("\u001b\u0003", true, ["ctrl+alt+c"]),
			kc("\u001bB", true, ["alt+left"]),
			kc("\u001bF", true, ["alt+right"]),
			kc("\u001ba", true, ["alt+a"]),
			kc("\u001b1", true, ["alt+1"]),
			kc("\u001b,", true, ["alt+,"]),
			kc("\u001b.", true, ["alt+."]),
			kc("\u001by", true, ["alt+y"]),
			kc("\u001bz", true, ["alt+z"]),
			kc("\u001bp", true, ["alt+up","up"]),
			],
		},
	},
	{
		name: "tui-keys/legacy-sequences",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b[104;7u", false, ["ctrl+alt+h"]),
			kc("\u001b[A", false, ["up"]),
			kc("\u001b[B", false, ["down"]),
			kc("\u001b[C", false, ["right"]),
			kc("\u001b[D", false, ["left"]),
			kc("\u001bOA", false, ["up"]),
			kc("\u001bOB", false, ["down"]),
			kc("\u001bOC", false, ["right"]),
			kc("\u001bOD", false, ["left"]),
			kc("\u001bOH", false, ["home"]),
			kc("\u001bOF", false, ["end"]),
			kc("\u001b[1;5H", false, ["ctrl+home"]),
			kc("\u001b[1;5F", false, ["ctrl+end"]),
			kc("\u001b[5;5~", false, ["ctrl+pageUp"]),
			kc("\u001b[6;5~", false, ["ctrl+pageDown"]),
			kc("\u001bOP", false, ["f1"]),
			kc("\u001b[24~", false, ["f12"]),
			kc("\u001b[E", false, ["clear"]),
			kc("\u001b[a", false, ["shift+up"]),
			kc("\u001bOa", false, ["ctrl+up"]),
			kc("\u001b[2$", false, ["shift+insert"]),
			kc("\u001b[2^", false, ["ctrl+insert"]),
			kc("\u001b[7$", false, ["shift+home"]),
			kc("\u001b[57399u"),
			kc("\u001b[57400u"),
			kc("\u001b[57409u"),
			kc("\u001b[57410u"),
			kc("\u001b[57411u"),
			kc("\u001b[57412u"),
			kc("\u001b[57413u"),
			kc("\u001b[57415u"),
			kc("\u001b[57416u"),
			kc("\u001b[57417u"),
			kc("\u001b[[5~"),
			],
		},
	},
	{
		name: "tui-keys/legacy-tables",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b[A", false, ["up"]),
			kc("\u001bOA", false, ["up"]),
			kc("\u001b[B", false, ["down"]),
			kc("\u001bOB", false, ["down"]),
			kc("\u001b[C", false, ["right"]),
			kc("\u001bOC", false, ["right"]),
			kc("\u001b[D", false, ["left"]),
			kc("\u001bOD", false, ["left"]),
			kc("\u001b[H", false, ["home"]),
			kc("\u001bOH", false, ["home"]),
			kc("\u001b[1~", false, ["home"]),
			kc("\u001b[7~", false, ["home"]),
			kc("\u001b[F", false, ["end"]),
			kc("\u001bOF", false, ["end"]),
			kc("\u001b[4~", false, ["end"]),
			kc("\u001b[8~", false, ["end"]),
			kc("\u001b[2~", false, ["insert"]),
			kc("\u001b[3~", false, ["delete"]),
			kc("\u001b[5~", false, ["pageUp"]),
			kc("\u001b[[5~", false, ["pageUp"]),
			kc("\u001b[6~", false, ["pageDown"]),
			kc("\u001b[[6~", false, ["pageDown"]),
			kc("\u001b[E", false, ["clear"]),
			kc("\u001bOE", false, ["clear"]),
			kc("\u001bOP", false, ["f1"]),
			kc("\u001b[11~", false, ["f1"]),
			kc("\u001b[[A", false, ["f1"]),
			kc("\u001bOQ", false, ["f2"]),
			kc("\u001b[12~", false, ["f2"]),
			kc("\u001b[[B", false, ["f2"]),
			kc("\u001bOR", false, ["f3"]),
			kc("\u001b[13~", false, ["f3"]),
			kc("\u001b[[C", false, ["f3"]),
			kc("\u001bOS", false, ["f4"]),
			kc("\u001b[14~", false, ["f4"]),
			kc("\u001b[[D", false, ["f4"]),
			kc("\u001b[15~", false, ["f5"]),
			kc("\u001b[[E", false, ["f5"]),
			kc("\u001b[17~", false, ["f6"]),
			kc("\u001b[18~", false, ["f7"]),
			kc("\u001b[19~", false, ["f8"]),
			kc("\u001b[20~", false, ["f9"]),
			kc("\u001b[21~", false, ["f10"]),
			kc("\u001b[23~", false, ["f11"]),
			kc("\u001b[24~", false, ["f12"]),
			kc("\u001b[a", false, ["shift+up"]),
			kc("\u001b[b", false, ["shift+down"]),
			kc("\u001b[c", false, ["shift+right"]),
			kc("\u001b[d", false, ["shift+left"]),
			kc("\u001b[e", false, ["shift+clear"]),
			kc("\u001b[2$", false, ["shift+insert"]),
			kc("\u001b[3$", false, ["shift+delete"]),
			kc("\u001b[5$", false, ["shift+pageUp"]),
			kc("\u001b[6$", false, ["shift+pageDown"]),
			kc("\u001b[7$", false, ["shift+home"]),
			kc("\u001b[8$", false, ["shift+end"]),
			kc("\u001bOa", false, ["ctrl+up"]),
			kc("\u001bOb", false, ["ctrl+down"]),
			kc("\u001bOc", false, ["ctrl+right"]),
			kc("\u001bOd", false, ["ctrl+left"]),
			kc("\u001bOe", false, ["ctrl+clear"]),
			kc("\u001b[2^", false, ["ctrl+insert"]),
			kc("\u001b[3^", false, ["ctrl+delete"]),
			kc("\u001b[5^", false, ["ctrl+pageUp"]),
			kc("\u001b[6^", false, ["ctrl+pageDown"]),
			kc("\u001b[7^", false, ["ctrl+home"]),
			kc("\u001b[8^", false, ["ctrl+end"]),
			kc("\u001bb", false, ["alt+left"]),
			kc("\u001bf", false, ["alt+right"]),
			kc("\u001bp", false, ["alt+up"]),
			kc("\u001bn", false, ["alt+down"]),
			],
		},
	},
	{
		name: "tui-keys/specials-with-modified-ids",
		tool: "tui-keys",
		args: {
			cases: [

			],
		},
	},
	{
		name: "tui-keys/printable-basic",
		tool: "tui-keys",
		args: {
			cases: [
			kc("a", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","a"]),
			kc("b", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","b"]),
			kc("c", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","c"]),
			kc("x", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","x"]),
			kc("z", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","z"]),
			kc("A", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","A"]),
			kc("Z", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","Z"]),
			kc("0", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","0"]),
			kc("1", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","1"]),
			kc("9", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","9"]),
			kc("+", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","+"]),
			kc("-", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","-"]),
			kc("=", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","="]),
			kc("[", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","["]),
			kc("]", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","]"]),
			kc("\\", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","\\"]),
			kc(";", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t",";"]),
			kc("'", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","'"]),
			kc(",", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t",","]),
			kc(".", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","."]),
			kc("/", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","/"]),
			kc("`", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","`"]),
			kc("!", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","!"]),
			kc("@", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","@"]),
			kc("#", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","#"]),
			kc("$", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","$"]),
			kc("%", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","%"]),
			kc("^", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","^"]),
			kc("&", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","&"]),
			kc("*", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","*"]),
			kc("(", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","("]),
			kc(")", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t",")"]),
			kc("_", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","_"]),
			kc("|", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","|"]),
			kc("~", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","~"]),
			kc("{", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","{"]),
			kc("}", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","}"]),
			kc(":", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t",":"]),
			kc("<", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","<"]),
			kc(">", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t",">"]),
			kc("?", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","?"]),
			kc(" ", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t"," "]),
			kc("\t", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","\t"]),
			kc("\r", false, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","\r"]),
			kc("a", true, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","a"]),
			kc("c", true, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","c"]),
			kc("x", true, ["ctrl+c","shift+enter","alt+x","super+k","ctrl+shift+t","x"]),
			],
		},
	},
	{
		name: "tui-keys/alt-prefixed-sweep-kitty-off",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001ba", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bb", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bc", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bd", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001be", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bf", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bn", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bp", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bq", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001by", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bz", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bB", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bF", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b0", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b1", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b3", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b9", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b,", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b.", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b ", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b\b", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b\u0003", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b", false, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			],
		},
	},
	{
		name: "tui-keys/alt-prefixed-sweep-kitty-on",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001ba", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bb", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bc", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bd", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001be", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bf", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bn", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bp", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bq", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001by", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bz", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bB", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001bF", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b0", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b1", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b3", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b9", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b,", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b.", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b ", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b\b", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b\u0003", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			kc("\u001b", true, ["alt+a","alt+space","alt+backspace","ctrl+alt+c","alt+left","alt+right","alt+up","alt+down","alt+1","alt+,","alt+.","alt+y","alt+z","escape"]),
			],
		},
	},
	{
		name: "tui-keys/edge-cases",
		tool: "tui-keys",
		args: {
			cases: [
			kc("", false, ["escape","enter"]),
			kc("", true, ["escape"]),
			kc("\u001b", false, ["escape","alt+a"]),
			kc("\u001b[", false, ["up","escape"]),
			kc("\u001b[27", false, ["ctrl+c"]),
			kc("\u001b[27;", false, ["ctrl+c"]),
			kc("\u001b[27;5", false, ["ctrl+c"]),
			kc("\u001b[27;5;", false, ["ctrl+c"]),
			kc("\u001bu", false, ["u"]),
			kc("\u001b[27;5;99x", false, ["ctrl+c"]),
			kc("\u001b[27;0;99~", false, ["ctrl+c"]),
			kc("\u001b[27;99;99~", false, ["ctrl+c"]),
			kc("\u001b[27;5;999999999~", false, ["ctrl+c"]),
			kc(""),
			kc("abc", false, ["a"]),
			kc("😀"),
			kc("👍🏽"),
			kc("é"),
			kc("𠮷"),
			kc("\u001b[128512u", true),
			kc("\u001b[128512;5u", true, ["ctrl+c"]),
			kc("\u001b[999999999u", true),
			kc("\u001b[1114112u", true),
			kc("\u001b[0u", true),
			kc("\u001b[57398u", true),
			kc("\u001b[57427u", true),
			kc("\u001b[65;0u", true),
			kc("\u001b[65;4294967296u", true),
			kc("\u001b[65;99u", true),
			kc("\u001b[97;17u", true),
			kc("\u001b[97:;u", true),
			kc("\u001b[97::u", true),
			kc("\u001b[:5u", true),
			kc("\u001b[;;u", true),
			kc("\u001b[1;;A"),
			kc("\u001b[5;;~"),
			kc("", false, ["backspace","ctrl+backspace"]),
			],
		},
	},
	{
		name: "tui-keys/event-types",
		tool: "tui-keys",
		args: {
			cases: [
			kc("\u001b[97;1:3u", true),
			kc("\u001b[97;1:2u", true),
			kc("\u001b[97;1:1u", true),
			kc("\u001b[97;5:3u", true),
			kc("\u001b[97;5:2u", true),
			kc("\u001b[1089::99;5:3u", true),
			kc("\u001b[1089:1057:99;6:2u", true),
			kc("\u001b[1;5:3H", true),
			kc("\u001b[1;2:2B", true),
			kc("\u001b[5;3:3~", true),
			kc("\u001b[1;3:2F", true),
			kc("\u001b[1;5:3A"),
			kc("\u001b[5;5:2~"),
			kc("\u001b[27;5;99~"),
			kc("\u001b[200~"),
			kc("\u001b[201~"),
			kc("\u001b[200~90:62:3F:A5\u001b[201~"),
			kc("\u001b[200~a:2u b:3u\u001b[201~"),
			kc("90:62:3F:A5"),
			kc("plain text :3u inside"),
			kc("ends with :2H"),
			kc(":3A"),
			kc(":3D"),
			kc("x:3Fx"),
			],
		},
	},
	// --------------------------------------------------- tui-stdin-buffer
	// Differential parity for the stdin buffer: TS packages/tui/src/stdin-buffer.ts
	// vs Rust pi_tui::stdin_buffer. Each sub-case = { name, kitty?, timeoutMs?,
	// escapeTimeoutMs?, script: { chunks, fireTimeoutAfterEachChunk?,
	// fireTimeoutAt? } }; chunks are strings, {units:[u16]} (raw UTF-16, for
	// lone-surrogate inputs that JSON->serde_json would mangle), or {bytes:[u8]}
	// (the TS Buffer input path). Both harnesses construct StdinBuffer with the
	// explicit options, feed chunks in order, fire the pending timeout only
	// where scripted, then serialize the ordered event log (data/paste with
	// UTF-16 unit arrays), the final buffer units, and the pending-timeout
	// kind. The TS harness patches setTimeout so the real timer callback runs
	// synchronously at exactly the scripted fire points (deterministic).
	{
		name: "tui-stdin-buffer/regular-chars",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("single-a", ["a"]),
			sb("abc", ["abc"]),
			sb("hello-world-unicode", ["hello \u4e16\u754c"]),
			sb("unicode-chunked", ["hello", " \u4e16\u754c"]),
			sb("unicode-per-char", ["\u4e16", "\u754c"]),
			sb("astral-emoji", ["\ud83c\udf89"]),
			sb("astral-split-surrogate-halves", [{ units: [0xd83c] }, { units: [0xdf89] }]),
			sb("empty-input", [""]),
			sb("empty-twice", ["", ""]),
			sb("empty-after-text", ["a", ""]),
			sb("separate-chunks", ["a", "b", "c"]),
			sb("fire-after-each-plain-text", ["hello \u4e16\u754c"], { fireAfterEach: true }),
			sb("control-chars", ["x\r\n\t\u0003\u0004"]),
			sb("long-text", ["abcdefghij".repeat(10)]),
			sb("mixed-chunks", ["\u4e16\u754c", "\ud83c\udf89", "a"]),
			],
		},
	},
	{
		name: "tui-stdin-buffer/complete-sequences",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("mouse-sgr-press", ["\u001b[<0;10;5M"]),
			sb("mouse-sgr-release", ["\u001b[<0;10;5m"]),
			sb("mouse-sgr-move", ["\u001b[<35;20;5m"]),
			sb("mouse-multi", ["\u001b[<35;1;1m\u001b[<35;2;2m\u001b[<35;3;3m"]),
			sb("arrow-up", ["\u001b[A"]),
			sb("f1", ["\u001b[11~"]),
			sb("csi-home-ctrl", ["\u001b[1;5H"]),
			sb("meta-a", ["\u001ba"]),
			sb("ss3-a", ["\u001bOA"]),
			sb("ss3-pair", ["\u001bOP\u001bOQ"]),
			sb("unknown-esc-final", ["\u001bZ"]),
			sb("kitty-csiu-press", ["\u001b[97u"], { kitty: true }),
			sb("kitty-csiu-release", ["\u001b[97;1:3u"], { kitty: true }),
			sb("kitty-batched-press-release", ["\u001b[97u\u001b[97;1:3u"], { kitty: true }),
			sb("kitty-batched-multi", ["\u001b[97u\u001b[97;1:3u\u001b[98u\u001b[98;1:3u"], { kitty: true }),
			sb("kitty-arrow-event", ["\u001b[1;1:1A"], { kitty: true }),
			sb("kitty-delete-release", ["\u001b[3;1:3~"], { kitty: true }),
			sb("plain-then-kitty-release", ["a\u001b[97;1:3u"], { kitty: true }),
			sb("rapid-typing", ["\u001b[104u\u001b[104;1:3u\u001b[105u\u001b[105;1:3u"], { kitty: true }),
			sb("old-style-mouse", ["\u001b[M abc"]),
			sb("three-arrows", ["\u001b[A\u001b[B\u001b[C"]),
			sb("chars-then-esc", ["abc\u001b[A"]),
			sb("esc-then-chars", ["\u001b[Aabc"]),
			sb("long-csi", ["\u001b[" + "1;".repeat(50) + "H"]),
			sb("esc-o-incomplete-then-fire", ["\u001bO"], { fireAt: [0] }),
			sb("partial-then-complete", ["abc\u001b[<35", ";20;5m"]),
			sb("partial-then-complete-fire", ["abc\u001b[<35", ";20;5m"], { fireAt: [0] }),
			],
		},
	},
	{
		name: "tui-stdin-buffer/sgr-mouse-splits",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			...sbSplits("\u001b[<35;20;5m", {}, "sgr-move"),
			sb("sgr-move-char-by-char", ["\u001b", "[", "<", "3", "5", ";", "2", "0", ";", "5", "m"]),
			sb("sgr-move-split-fire", ["\u001b[", "<65;48;39M"], { fireAt: [0] }),
			...sbSplits("\u001b[<65;48;39M", {}, "sgr-m2"),
			sb("sgr-partial-4chunks", ["\u001b[<3", "5;1", "5;", "10m"]),
			],
		},
	},
	{
		name: "tui-stdin-buffer/csi-ss3-meta-splits",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			...sbSplits("\u001b[A", {}, "arrow"),
			...sbSplits("\u001b[1;5H", {}, "csi-h"),
			...sbSplits("\u001b[11~", {}, "f1"),
			...sbSplits("\u001b[3;5~", {}, "csi-del"),
			...sbSplits("\u001bOA", {}, "ss3"),
			...sbSplits("\u001ba", {}, "meta"),
			...sbSplits("\u001b[1;1:1A", { kitty: true }, "kitty-arrow"),
			...sbSplits("\u001b[97u", { kitty: true }, "kitty-u"),
			...sbSplits("\u001b[97;1:3u", { kitty: true }, "kitty-rel"),
			],
		},
	},
	{
		name: "tui-stdin-buffer/wezterm-esc-esc",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("wezterm-esc-esc-csi-mod", ["\u001b\u001b[27;129:3u"]),
			sb("wezterm-esc-esc-csi-nomod", ["\u001b\u001b[27;1:3u"]),
			sb("esc-esc-alone", ["\u001b\u001b"]),
			sb("esc-esc-then-plain", ["\u001b\u001bx"]),
			sb("esc-esc-split-1", ["\u001b", "\u001b[27;1:3u"]),
			sb("esc-esc-split-2", ["\u001b\u001b", "[27;1:3u"]),
			sb("esc-esc-split-3-fire", ["\u001b\u001b[", "27;1:3u"], { fireAt: [0] }),
			sb("esc-esc-osc-next", ["\u001b\u001b]"]),
			sb("esc-esc-ss3-next", ["\u001b\u001bO"]),
			sb("esc-esc-dcs-next", ["\u001b\u001bP"]),
			sb("esc-esc-apc-next", ["\u001b\u001b_"]),
			sb("esc-esc-unknown-next", ["\u001b\u001bZ"]),
			sb("esc-esc-esc-csi", ["\u001b\u001b\u001b[A"]),
			sb("esc-esc-fire", ["\u001b\u001b"], { fireAt: [0] }),
			],
		},
	},
	{
		name: "tui-stdin-buffer/kitty-printable-dedup",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("dedup-a", ["\u001b[97ua"], { kitty: true }),
			sb("dedup-astral", ["\u001b[128512u\ud83c\udf89"], { kitty: true }),
			sb("dedup-a-chunks", ["\u001b[97u", "a"], { kitty: true }),
			sb("dedup-nonmatching", ["\u001b[97ub"], { kitty: true }),
			sb("dedup-modified-kept", ["\u001b[64;3u@"], { kitty: true }),
			sb("dedup-at-sign", ["\u001b[64u@"], { kitty: true }),
			sb("dedup-then-paste", ["\u001b[97ua", "\u001b[200~p\u001b[201~"], { kitty: true }),
			sb("dedup-only-first", ["\u001b[97uaa"], { kitty: true }),
			sb("dedup-below-printable", ["\u001b[57376u"], { kitty: true }),
			sb("dedup-colon-form", ["\u001b[97:1ua"], { kitty: true }),
			sb("dedup-space", ["\u001b[32u "], { kitty: true }),
			sb("dedup-no-kitty", ["\u001b[97ua"]),
			],
		},
	},
	{
		name: "tui-stdin-buffer/terminal-replies-splits",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			...[
				["osc4-ghostty", "\u001b]4;1;rgb:eaea/6c6c/6d6d\u0007", "\u001b]4;1;"],
				["osc10", "\u001b]10;rgb:ffff/ffff/ffff\u001b\\", "\u001b]10;"],
				["osc11", "\u001b]11;rgb:0000/0000/0000\u0007", "\u001b]11;"],
				["osc4-alt", "\u001b]4;1;rgb:6464/6464/4d4d\u0007", "\u001b]4;1;"],
				["dcs-xtversion", "\u001bP>|Ghostty 1.3.1\u001b\\", "\u001bP>|"],
				["apc-kitty-graphics", "\u001b_Gi=1;OK\u001b\\", "\u001b_Gi=1;"],
				["osc7501", "\u001b]7501;?\u001b\\", "\u001b]7501;?"],
			].flatMap(([label, reply, header]) =>
				Array.from({ length: reply.length - header.length }, (_, i) => {
					const split = header.length + i;
					return sb(`${label}-split-${split}`, [reply.slice(0, split), reply.slice(split) + "x"], {
						fireAt: [0],
					});
				}),
			),
			],
		},
	},
	{
		name: "tui-stdin-buffer/terminal-replies-behavior",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("osc-multi-fire", ["\u001b]4;1;rgb:eaea/6c", "6c/6d6d", "\u0007"], { fireAfterEach: true }),
			sb("osc-header-assembled-no-fire", ["\u001b]1", "0;", "rgb:ffff/ffff/ffff\u001b\\"]),
			sb("osc-header-fire-at-boundary", ["\u001b]1", "0;"], { fireAt: [0] }),
			sb("unterminated-reply-new-escape", ["\u001b]4;1;rgb:eaea/6c", "\u001b", "[Ahello"], { fireAt: [1] }),
			sb("can-abort", ["\u001b]11;rgb:0000\u0018x"]),
			sb("sub-abort", ["\u001b]11;rgb:0000\u001ax"]),
			sb("can-abort-chunked", ["\u001b]11;rgb:0000", "\u0018x"]),
			sb("osc-introducer-alt-recovery", ["\u001b]", "x\r\u0003\u0004"], { timeoutMs: 10, fireAt: [0] }),
			sb("dcs-introducer-alt-recovery", ["\u001bP", "x\r\u0003\u0004"], { timeoutMs: 10, fireAt: [0] }),
			sb("apc-introducer-alt-recovery", ["\u001b_", "x\r\u0003\u0004"], { timeoutMs: 10, fireAt: [0] }),
			sb("reply-then-partial-fire", ["\u001b]10;", "rgb:ff"], { fireAt: [1] }),
			...[
				"\u001b]a", "\u001b]1", "\u001b]10", "\u001b]0;title", "\u001b]4;", "\u001b]4;a;",
				"\u001bPa", "\u001bP>", "\u001b_a", "\u001b_G", "\u001b_Gi=1",
			].map((ambiguous, i) =>
				sb(`ambiguous-${i}`, [ambiguous, "x\r\u0003\u0004"], { fireAt: [1] }),
			),
			],
		},
	},
	{
		name: "tui-stdin-buffer/bracketed-paste",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("paste-complete", ["\u001b[200~hello world\u001b[201~"]),
			sb("paste-chunked", ["\u001b[200~", "hello ", "world\u001b[201~"]),
			sb("paste-before-after", ["a", "\u001b[200~pasted\u001b[201~", "b"]),
			sb("paste-newlines", ["\u001b[200~line1\nline2\nline3\u001b[201~"]),
			sb("paste-unicode", ["\u001b[200~Hello \u4e16\u754c \ud83c\udf89\u001b[201~"]),
			sb("paste-empty", ["\u001b[200~\u001b[201~"]),
			sb("paste-contains-escape", ["\u001b[200~\u001b[A\u001b[201~"]),
			sb("paste-start-split", ["\u001b[2", "00~abc\u001b[201~"]),
			sb("paste-end-split", ["\u001b[200~abc\u001b[2", "01~"]),
			sb("paste-start-only", ["\u001b[200~"]),
			sb("paste-end-only", ["\u001b[201~"]),
			sb("paste-two", ["\u001b[200~a\u001b[201~", "\u001b[200~b\u001b[201~"]),
			sb("paste-then-escape", ["\u001b[200~x\u001b[201~", "\u001b[A"]),
			sb("escape-then-paste", ["\u001b[A", "\u001b[200~x\u001b[201~"]),
			sb("paste-after-partial-escape", ["\u001b[", "\u001b[200~x\u001b[201~"]),
			sb("interleaved-paste-escape", ["\u001b[200~", "\u001b[A", "\u001b[201~"]),
			sb("paste-fire-inside", ["\u001b[200~", "abc"], { fireAfterEach: true }),
			],
		},
	},
	{
		name: "tui-stdin-buffer/timeout-behavior",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("lone-esc-fire", ["\u001b"], { fireAt: [0] }),
			sb("lone-esc-fire-custom-timeouts", ["\u001b"], { fireAt: [0], escapeTimeoutMs: 33, timeoutMs: 77 }),
			sb("lone-esc-then-cr-no-fire", ["\u001b", "\r"]),
			sb("lone-esc-fire-then-cr", ["\u001b", "\r"], { fireAt: [0] }),
			sb("esc-cr-merge-custom-escape-timeout", ["\u001b", "\r"], { escapeTimeoutMs: 100 }),
			sb("partial-fire-then-more", ["\u001b[<35", "\u001b[A"], { fireAt: [0] }),
			sb("partial-then-complete-with-fires", ["\u001b[<35", ";20;5m"], { fireAt: [0, 1] }),
			sb("fire-empty-buffer", ["a"], { fireAt: [0] }),
			sb("fire-only-no-chunks", [], { fireAfterEach: true }),
			sb("empty-chunks-fire", ["", ""], { fireAfterEach: true }),
			sb("lone-esc-pending-state", ["\u001b"]),
			sb("partial-csi-pending-state", ["\u001b[<35"]),
			sb("reply-header-no-timer", ["\u001b]10;rgb:ff"]),
			sb("fire-reply-header-noop", ["\u001b]10;rgb:ff"], { fireAt: [0] }),
			sb("esc-esc-fire-then-input", ["\u001b\u001b", "x"], { fireAt: [0] }),
			sb("mixed-fire-no-op", ["hello", "\u4e16\u754c"], { fireAfterEach: true }),
			sb("esc-then-text-no-fire", ["\u001b", "abc"]),
			],
		},
	},
	{
		name: "tui-stdin-buffer/buffer-bytes",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("bytes-csi", [{ bytes: [0x1b, 0x5b, 0x41] }]),
			sb("bytes-high-e5", [{ bytes: [0xe5] }]),
			sb("bytes-high-94", [{ bytes: [0x94] }]),
			sb("bytes-high-ff", [{ bytes: [0xff] }]),
			sb("bytes-utf8-world", [{ bytes: [0xe4, 0xb8, 0x96] }]),
			sb("bytes-utf8-multi", [{ bytes: [0x61, 0xe4, 0xb8, 0x96, 0x62] }]),
			sb("bytes-invalid-utf8", [{ bytes: [0xe4, 0xb8] }]),
			sb("bytes-empty", [{ bytes: [] }]),
			sb("bytes-split-csi", [{ bytes: [0x1b, 0x5b] }, { bytes: [0x41] }]),
			sb("bytes-high-then-text", [{ bytes: [0xe5] }, "llo"]),
			],
		},
	},
	{
		name: "tui-stdin-buffer/adversarial",
		tool: "tui-stdin-buffer",
		args: {
			cases: [
			sb("osc-partial-across-fires", ["\u001b]4;1;", "rgb:ea", "ea/6c6c", "\u0007"], { fireAfterEach: true }),
			sb("reply-then-paste", ["\u001b]10;rgb:ffff/ffff/ffff\u001b\\", "\u001b[200~p\u001b[201~"]),
			sb("paste-start-inside-csi", ["\u001b[1", "\u001b[200~z\u001b[201~"]),
			sb("esc-fire-between-paste-chunks", ["\u001b[200~a", "\u001b[201~"], { fireAt: [0] }),
			sb("nested-osc-in-paste", ["\u001b[200~\u001b]10;x\u0007\u001b[201~"]),
			sb("kitty-then-abort", ["\u001b[97u", "\u001b]11;rgb:0\u0018"], { kitty: true }),
			sb("text-fire-text", ["ab", "cd"], { fireAfterEach: true }),
			sb("wezterm-fire-mid", ["\u001b", "\u001b[27;1:3u"], { fireAt: [0] }),
			sb("osc-split-inside-header-fire", ["\u001b]4", ";1;"], { fireAt: [0] }),
			sb("paste-unterminated-final", ["\u001b[200~abc"]),
			],
		},
	},
	// ------------------------------------------------------ tui-components
	// Differential render-line testing of TUI components: the same
	// JSON-serializable component spec tree is built on the TS side (ground
	// truth) and the Rust side, rendered at the given widths, and optionally
	// driven by a script (handle_input sequences, loader ticks, scroll ops,
	// mouse dispatch) with re-renders. Function-valued params are named
	// presets ("identity"/"bracket" styles, "record" callbacks whose event
	// log is part of the compared output). See ts-harness.js.
	{
		name: "tui-components/text",
		tool: "tui-components",
		args: {
			cases: [
				tc("basic", txt("hello"), { widths: [20] }),
				tc("empty", txt(""), { widths: [10] }),
				tc("whitespace-only", txt("   "), { widths: [10] }),
				tc("multiline", txt("one\ntwo\nthree"), { widths: [20] }),
				tc("wrap-long", txt("the quick brown fox jumps over the lazy dog"), { widths: [20, 12] }),
				tc("wrap-cjk", txt("世界你好世界你好世界"), { widths: [10, 7] }),
				tc("wrap-emoji-astral", txt("👍🏽👍🏽👍🏽👍🏽 family 👨‍👩‍👧‍👦 test"), { widths: [10] }),
				tc("padding-x-0", txt("hi", 0), { widths: [10] }),
				tc("padding-x-3", txt("hi", 3), { widths: [10] }),
				tc("padding-y-0", txt("hi", 1, 0), { widths: [10] }),
				tc("padding-y-2", txt("hi", 1, 2), { widths: [10] }),
				tc("narrow-3", txt("hello world", 2), { widths: [3] }),
				tc("tabs", txt("a\tb\tc"), { widths: [20] }),
				tc("bg-bracket", txt("hi", 1, 1, "bracket"), { widths: [12] }),
				tc("bg-identity", txt("hi", 1, 1, "identity"), { widths: [12] }),
			],
		},
	},
	{
		name: "tui-components/truncated-text",
		tool: "tui-components",
		args: {
			cases: [
				tc("basic", ttxt("hello world this is long"), { widths: [20] }),
				tc("exact-fit", ttxt("hello"), { widths: [10] }),
				tc("short", ttxt("hi"), { widths: [10] }),
				tc("multiline-first-line", ttxt("first\nsecond"), { widths: [20] }),
				tc("padding", ttxt("hello world", 2, 1), { widths: [15] }),
				tc("cjk", ttxt("世界世界世界"), { widths: [7] }),
				tc("empty", ttxt(""), { widths: [10] }),
			],
		},
	},
	{
		name: "tui-components/spacer",
		tool: "tui-components",
		args: {
			cases: [
				tc("default", spc(), { widths: [10] }),
				tc("zero", spc(0), { widths: [10] }),
				tc("three", spc(3), { widths: [10] }),
				tc("in-box", boxc([txt("a"), spc(2), txt("b")], 0, 0), { widths: [10] }),
			],
		},
	},
	{
		name: "tui-components/box",
		tool: "tui-components",
		args: {
			cases: [
				tc("empty", boxc([], 1, 1), { widths: [10] }),
				tc("single-child", boxc([txt("hello")], 1, 1), { widths: [20] }),
				tc("no-padding", boxc([txt("hello")], 0, 0), { widths: [12] }),
				tc("padding-2-0", boxc([txt("hello")], 2, 0), { widths: [14] }),
				tc("padding-0-2", boxc([txt("hello")], 0, 2), { widths: [12] }),
				tc("multiple-children", boxc([txt("one"), txt("two"), txt("three")], 1, 1), { widths: [16] }),
				tc("nested", boxc([boxc([txt("inner")], 1, 1)], 2, 2), { widths: [20] }),
				tc("bg-bracket", boxc([txt("hi")], 1, 1, "bracket"), { widths: [12] }),
				tc("child-wrap", boxc([txt("a long line that wraps around")], 1, 1), { widths: [14] }),
				tc("empty-text-child", boxc([txt("")], 1, 1), { widths: [10] }),
				tc("spacer-child", boxc([spc(1), txt("x")], 0, 0), { widths: [10] }),
			],
		},
	},
	{
		name: "tui-components/stacks",
		tool: "tui-components",
		args: {
			cases: [
				tc("vstack-basic", stk("v-stack", [ent(txt("a")), ent(txt("b"))]), { widths: [10] }),
				tc("vstack-gap", stk("v-stack", [ent(txt("a")), ent(txt("b"))], { gap: 2 }), { widths: [10] }),
				tc("vstack-empty", stk("v-stack", []), { widths: [10] }),
				tc("hstack-basic", stk("h-stack", [ent(txt("aa")), ent(txt("bb"))]), { widths: [20] }),
				tc("hstack-gap", stk("h-stack", [ent(txt("aa")), ent(txt("bb"))], { gap: 1 }), { widths: [20] }),
				tc("hstack-grow", stk("h-stack", [ent(txt("a"), { grow: 1 }), ent(txt("b"), { grow: 3 })]), { widths: [20] }),
				tc("hstack-shrink", stk("h-stack", [ent(txt("aaaaaaaaaa"), { shrink: 1 }), ent(txt("bbbbbbbbbb"), { shrink: 3 })]), { widths: [10] }),
				tc("hstack-basis-points", stk("h-stack", [ent(txt("a"), { basis: 6 }), ent(txt("b"), { basis: 4 })]), { widths: [20] }),
				tc("hstack-basis-auto", stk("h-stack", [ent(txt("aaa"), { basis: "auto" }), ent(txt("b"), { basis: "auto", grow: 1 })]), { widths: [20] }),
				tc("hstack-min-size", stk("h-stack", [ent(txt("a"), { basis: 10, min: 4 }), ent(txt("b"))]), { widths: [8] }),
				tc("hstack-max-size", stk("h-stack", [ent(txt("a"), { grow: 1, max: 5 }), ent(txt("b"), { grow: 1 })]), { widths: [20] }),
				tc("hidden-entry", stk("v-stack", [ent(txt("a")), ent(txt("hidden"), { visible: "never" }), ent(txt("b"))]), { widths: [10] }),
				tc("align-center", stk("h-stack", [ent(txt("a")), ent(txt("b\nb2"))], { align: "center" }), { widths: [20] }),
				tc("align-end", stk("h-stack", [ent(txt("a")), ent(txt("b\nb2"))], { align: "end" }), { widths: [20] }),
				tc("align-start", stk("h-stack", [ent(txt("a\na2")), ent(txt("b"))], { align: "start" }), { widths: [20] }),
				tc("nested-v-in-h", boxc([stk("v-stack", [ent(txt("x")), ent(stk("h-stack", [ent(txt("y")), ent(txt("z"))]))])], 1, 1), { widths: [24] }),
			],
		},
	},
	{
		name: "tui-components/loader",
		tool: "tui-components",
		args: {
			cases: [
				tc("initial", ld({}), { widths: [20] }),
				tc("tick-1", ld({}), { script: [{ ticks: 1, render: true }] }),
				tc("tick-3", ld({}), { script: [{ ticks: 3, render: true }] }),
				tc("tick-12-wrap", ld({}), { script: [{ ticks: 12, render: true }] }),
				tc("custom-message", ld({ message: "Working hard" }), { widths: [20] }),
				tc("custom-frames", ld({ frames: ["-", "\\", "|", "/"], intervalMs: 10 }), { script: [{ render: true }, { ticks: 2, render: true }] }),
				tc("empty-frames", ld({ frames: [] }), { widths: [20] }),
				tc("single-frame", ld({ frames: ["*"] }), { script: [{ ticks: 2, render: true }] }),
				tc("interval-ms-zero", ld({ intervalMs: 0 }), { script: [{ ticks: 1, render: true }] }),
			],
		},
	},
	{
		name: "tui-components/cancellable-loader",
		tool: "tui-components",
		args: {
			cases: [
				tc("initial", { type: "cancellable-loader", message: "Working..." }, { widths: [20] }),
				tc("escape-abort", { type: "cancellable-loader", onAbort: "record" }, { script: [{ inputs: ["\u001b"], render: true }] }),
				tc("escape-twice", { type: "cancellable-loader", onAbort: "record" }, { script: [{ inputs: ["\u001b"] }, { inputs: ["\u001b"], render: true }] }),
				tc("plain-input-ignored", { type: "cancellable-loader", onAbort: "record" }, { script: [{ inputs: ["x", "\r"], render: true }] }),
				tc("tick", { type: "cancellable-loader" }, { script: [{ ticks: 2, render: true }] }),
			],
		},
	},
	{
		name: "tui-components/mouse-region",
		tool: "tui-components",
		args: {
			cases: [
				tc("render-passthrough", mr(txt("hello")), { widths: [12] }),
				tc("render-box-child", mr(boxc([txt("a"), txt("b")], 1, 1)), { widths: [12] }),
				tc("press-record", mr(txt("hello")), {
					widths: [12],
					script: [{ mouse: { type: "press", button: "left", x: 2, y: 0, width: 12, height: 3 }, render: true }],
				}),
				tc("click-record", mr(txt("hello")), {
					script: [{ mouse: { type: "click", button: "left", x: 4, y: 1, width: 12, height: 3, clickCount: 2 } }],
				}),
				tc("wheel-record", mr(txt("hello")), {
					script: [{ mouse: { type: "wheel", button: "none", x: 0, y: 0, width: 12, height: 3, wheelDelta: -3 } }],
				}),
				tc("modifiers-record", mr(txt("hello")), {
					script: [{ mouse: { type: "move", button: "none", x: 1, y: 2, width: 12, height: 3, shift: true, alt: true, ctrl: true } }],
				}),
				tc("box-dispatch", boxc([txt("hello")], 1, 1), {
					widths: [12],
					script: [{ mouse: { type: "press", button: "left", x: 2, y: 1, width: 12, height: 5 } }],
				}),
			],
		},
	},
	{
		name: "tui-components/scroll-view",
		tool: "tui-components",
		args: {
			cases: [
				tc("render-passthrough", svw(txt("hello")), { widths: [12] }),
				tc("initial-state", svw(txt("a")), { widths: [10] }),
				tc("scroll-state", svw(boxc([txt("one"), txt("two"), txt("three"), txt("four"), txt("five")], 0, 0)), {
					script: [
						{ scroll: [{ updateLayout: [5, 2] }] },
						{ scroll: [{ scrollBy: 1 }] },
						{ scroll: [{ scrollBy: 10 }] },
						{ scroll: [{ scrollToStart: true }] },
						{ scroll: [{ scrollToEnd: true }] },
						{ render: true, width: 10 },
					],
				}),
				tc("follow-end", svw(boxc([txt("one"), txt("two"), txt("three")], 0, 0), { follow: "end" }), {
					script: [
						{ scroll: [{ updateLayout: [3, 2] }, { scrollBy: 5 }] },
						{ render: true, width: 10 },
					],
				}),
				tc("scroll-to-negative", svw(txt("x")), { script: [{ scroll: [{ updateLayout: [4, 2] }, { scrollTo: -5 }] }] }),
				tc("nested-in-box", boxc([svw(txt("deep"))], 1, 1), { widths: [12] }),
			],
		},
	},
	{
		name: "tui-components/input",
		tool: "tui-components",
		args: {
			cases: [
				tc("initial", inp({}), { widths: [20] }),
				tc("prompt-placeholder", inp({ prompt: "Name: ", placeholder: "enter name" }), { widths: [30] }),
				tc("placeholder-focused", inp({ placeholder: "type here", focused: true }), { widths: [30] }),
				tc("type-plain", inp({}), { script: [press("h", "e", "l", "l", "o"), { render: true, width: 20 }] }),
				tc("backspace", inp({}), { script: [press("a", "b", "c", "\u007f"), { render: true, width: 20 }] }),
				tc("arrows", inp({}), { script: [press("a", "b", "c", "\u001b[D", "\u001b[D", "\u001b[C"), { render: true, width: 20 }] }),
				tc("home-end", inp({ focused: true }), { script: [press("a", "b", "\u0001", "x", "\u0005", "z"), { render: true, width: 20 }] }),
				tc("word-left-right", inp({}), { script: [press("one two three", "\u001bb", "\u001bb", "\u001bf", "\u001bf"), { render: true, width: 30 }] }),
				tc("delete-word-backward", inp({}), { script: [press("one two", "\u001b\u007f"), { render: true, width: 20 }] }),
				tc("enter-submit", inp({ onSubmit: "record" }), { script: [press("hi", "\r", "\r")] }),
				tc("escape", inp({ onEscape: "record" }), { script: [press("a", "\u001b")] }),
				tc("cjk-input", inp({ focused: true }), { script: [press("你", "好", "世", "界"), { render: true, width: 20 }] }),
				tc("astral-input", inp({ focused: true }), { script: [press("\ud835\udc00", "b", "\ud83d\ude00"), { render: true, width: 20 }] }),
				tc("narrow-scroll", inp({ focused: true }), {
					script: [press("abcdefghijklmnopqrstuvwxyz"), { render: true, width: 10 }, { inputs: ["\u001b[D", "\u001b[D"], render: true, width: 10 }],
				}),
				tc("narrow-cjk", inp({}), { script: [press("世界世界世界世界"), { render: true, width: 9 }] }),
				tc("paste", inp({}), { script: [press("\u001b[200~ab\ncd\u001b[201~"), { render: true, width: 20 }] }),
				tc("kitty-csi-u", inp({}), { script: [press("\u001b[97u", "\u001b[98u"), { render: true, width: 20 }] }),
				tc("control-ignored", inp({}), { script: [press("\u0000", "\u0003"), { render: true, width: 20 }] }),
				tc("wide-render", inp({ prompt: ">>> " }), { script: [press("hello"), { render: true, width: 60 }] }),
			],
		},
	},
	{
		name: "tui-components/select-list",
		tool: "tui-components",
		args: {
			cases: [
				tc("empty", slist([]), { widths: [40] }),
				tc("single", slist([sitem("a", "Alpha")]), { widths: [40] }),
				tc("three", slist([sitem("a", "Alpha"), sitem("b", "Beta"), sitem("c", "Gamma")]), { widths: [40] }),
				tc("many-scroll", slist(Array.from({ length: 8 }, (_, i) => sitem("v" + i, "Item " + i)), { maxVisible: 3 }), { widths: [40] }),
				tc("descriptions", slist([sitem("a", "Alpha", "the first letter"), sitem("b", "Beta", "the second letter")]), { widths: [60] }),
				tc("description-narrow", slist([sitem("a", "Alpha", "desc here")]), { widths: [30] }),
				tc("move-down-wrap", slist([sitem("a", "A"), sitem("b", "B"), sitem("c", "C")], { onSelectionChange: "record" }), {
					script: [press("\u001b[A"), { render: true, width: 30 }, press("\u001b[B"), press("\u001b[B"), { render: true, width: 30 }],
				}),
				tc("enter-select", slist([sitem("a", "A"), sitem("b", "B")], { onSelect: "record", onSelectionChange: "record" }), {
					script: [press("\u001b[B"), press("\r")],
				}),
				tc("escape-cancel", slist([sitem("a", "A")], { onCancel: "record" }), { script: [press("\u001b")] }),
				tc("filter-match", slist([sitem("alpha", "Alpha"), sitem("beta", "Beta"), sitem("alphabet", "Alphabet")]), {
					script: [{ setFilter: "al" }, { render: true, width: 40 }],
				}),
				tc("filter-no-match", slist([sitem("a", "A")]), { script: [{ setFilter: "zzz" }, { render: true, width: 40 }] }),
				tc("min-max-primary", slist([sitem("a", "A"), sitem("b", "Bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")], { layoutOptions: { minPrimaryColumnWidth: 10, maxPrimaryColumnWidth: 12 } }), { widths: [60] }),
				tc("max-primary-only", slist([sitem("a", "A"), sitem("b", "B")], { layoutOptions: { maxPrimaryColumnWidth: 8 } }), { widths: [60] }),
				tc("label-fallback", slist([sitem("value-only", "")]), { widths: [40] }),
				tc("truncation-narrow", slist([sitem("a", "AAAAAAAAAAAAAAAAAAAA"), sitem("b", "B")]), { widths: [12] }),
			],
		},
	},
	{
		name: "tui-components/settings-list",
		tool: "tui-components",
		args: {
			cases: [
				tc("basic", stlist([sit("theme", "Theme", "dark"), sit("lang", "Language", "en")]), { widths: [50] }),
				tc("selected-cursor", stlist([sit("a", "Aaa", "1"), sit("b", "Bbb", "2")]), { widths: [50] }),
				tc("description", stlist([sit("a", "Alpha", "off", { description: "The alpha setting controls things" }), sit("b", "Beta", "on")]), { widths: [50] }),
				tc("values-cycle", stlist([sit("mode", "Mode", "auto", { values: ["auto", "manual", "off"] })], { onChange: "record" }), {
					script: [press("\r"), { render: true, width: 50 }, press("\r"), { render: true, width: 50 }],
				}),
				tc("move-up-down", stlist([sit("a", "A", "1"), sit("b", "B", "2"), sit("c", "C", "3")]), {
					script: [press("\u001b[B"), { render: true, width: 40 }, press("\u001b[A"), { render: true, width: 40 }],
				}),
				tc("escape-cancel", stlist([sit("a", "A", "1")], { onCancel: "record" }), { script: [press("\u001b")] }),
				tc("empty", stlist([], { onCancel: "record" }), { widths: [40] }),
				tc("search-filter", stlist([sit("alpha", "Alpha", "1"), sit("beta", "Beta", "2"), sit("gamma", "Gamma", "3")], { enableSearch: true }), {
					script: [press("al"), { render: true, width: 40 }, press("z"), { render: true, width: 40 }],
				}),
				tc("max-visible-scroll", stlist(Array.from({ length: 6 }, (_, i) => sit("s" + i, "Setting " + i, String(i))), { maxVisible: 2 }), {
					script: [press("\u001b[B"), press("\u001b[B"), press("\u001b[B"), { render: true, width: 40 }],
				}),
			],
		},
	},
	{
		name: "tui-components/composite",
		tool: "tui-components",
		args: {
			cases: [
				tc("box-vstack", boxc([stk("v-stack", [ent(txt("Title")), ent(spc(1)), ent(txt("Body text here"))])], 1, 1), { widths: [24] }),
				tc("box-hstack", boxc([stk("h-stack", [ent(txt("left")), ent(txt("right"))], { gap: 1 })], 1, 1), { widths: [24] }),
				tc("box-input-static", boxc([stk("v-stack", [ent(txt("Name:")), ent(inp({ placeholder: "type...", focused: true }))])], 1, 1), {
					script: [press("bob"), { render: true, width: 30 }],
				}),
				tc("box-selectlist", boxc([slist([sitem("a", "Alpha"), sitem("b", "Beta")], { onSelect: "record" })], 1, 1), {
					script: [press("\u001b[B"), press("\r"), { render: true, width: 40 }],
				}),
				tc("scroll-vstack", svw(stk("v-stack", [ent(txt("one")), ent(txt("two")), ent(txt("three"))])), {
					script: [{ scroll: [{ updateLayout: [3, 2] }] }, { render: true, width: 12 }],
				}),
			],
		},
	},
	// --------------------------------------------------------- tui-editor
	// Event-sequence differential testing of the Editor component
	// (editor.ts): a scripted input stream drives a fresh Editor built on a
	// fake TUI (rows=24, cols=80) with the identity theme preset
	// ("default": borderColor and all selectList members are (t) => t —
	// the Rust harness mirrors this exactly, like the tui-components
	// "identity" presets). The compared output is the final editor state
	// (text/expandedText/cursor/lines), one frame per {render} op, the
	// fake TUI's render-request counter, and the onChange/onSubmit event
	// log. Autocomplete (provider presets) is out of scope for this wave.
	// Case shape + output follow docs/T4_PLAN.md §6; see the "tui-editor"
	// handler in ts-harness.js for the documented extensions (case-level
	// `focused` / `disableSubmit`, and `{render: true}` falling back to
	// case `width ?? 80`).
	te("typing-ascii", [...ky("hello"), { render: 20 }]),
	te("typing-umlauts", ky("h", "ä", "l", "l", "ö"), { render: 20 }),
	te("typing-emoji-astral", ky("a", "😀", "b", "👨‍👩‍👧‍👦", "c")),
	te("typing-cjk", ky("你", "好", "世", "界"), { render: 20 }),
	te("typing-cjk-emoji-mixed", ky("日本", "😀", "語", "👨‍👩‍👧‍👦"), { render: 20 }),
	te("typing-single-chunks", [{ input: "hello" }, { input: " world" }, { input: "!" }]),
	te("typing-into-settext", [{ input: "!" }], { setText: "hello" }),
	te("cursor-left-right", ky("hello", "\x1b[D", "\x1b[D", "\x1b[D", "\x1b[C")),
	te("cursor-clamp-edges", ky("ab", "\x01", "\x1b[D", "\x05", "\x1b[C")),
	te("cursor-home-end-keys", ky("one two", "\x1b[H", "x", "\x1b[F", "y")),
	te("cursor-ctrl-a-e", ky("one two", "\x01", "x", "\x05", "y")),
	te("cursor-word-left", ky("one two three", "\x1b[1;5D", "\x1b[1;5D", "\x1b[1;5D")),
	te("cursor-word-right", ky("one two three", "\x01", "\x1b[1;5C", "\x1b[1;5C")),
	te("cursor-word-alt-b-f", ky("one two three", "\x1bb", "\x1bb", "\x1bf")),
	te("cursor-word-cjk", ky("你好世界 hello 世界", "\x1b[1;5D", "\x1b[1;5D", "\x1b[1;5C", "\x1bb")),
	te("cursor-word-emoji", ky("a 😀😀 b c", "\x1bb", "\x1bb", "\x1bf")),
	te("cursor-multiline-up-down", ky("one", "\n", "two", "\n", "three", "\x1b[A", "\x1b[A", "\x1b[B")),
	te("newline-alt-enter", ky("one", "\x1b\r", "two")),
	te("newline-ctrl-j", ky("one", "\n", "two")),
	te("newline-csi-shift-enter", ky("one", "\x1b[13;2~", "two")),
	te("backslash-enter-newline", ky("foo\\", "\r", "bar")),
	te("enter-submit", ky("hi there", "\r")),
	te("enter-submit-clears", ky("hi", "\r", "next")),
	te("enter-submit-empty", [{ input: "\r" }]),
	te("enter-submit-multiline", ky("one", "\n", "two", "\r")),
	te("submit-disabled", ky("hi", "\r", "more"), { disableSubmit: true }),
	te("backslash-enter-disabled", ky("a\\", "\r"), { disableSubmit: true }),
	te("backspace-ascii", ky("hello", "\x7f", "\x7f")),
	te("backspace-emoji", ky("a😀b", "\x7f", "\x7f", "\x7f")),
	te("backspace-zwj-family", ky("x👨‍👩‍👧‍👦y", "\x7f", "\x7f")),
	te("backspace-join-lines", ky("ab", "\n", "cd", "\x7f")),
	te("backspace-at-start-empty", [{ input: "\x7f" }]),
	te("forward-delete", ky("a😀b", "\x01", "\x1b[3~", "\x1b[3~")),
	te("forward-delete-end", ky("ab", "\x05", "\x1b[3~")),
	te("ctrl-w-delete-word", ky("hello world", "\x17")),
	te("alt-backspace-delete-word", ky("hello world", "\x1b\x7f")),
	te("alt-d-delete-word-forward", ky("hello world", "\x01", "\x1bd")),
	te("ctrl-w-punctuation", ky("foo.bar baz", "\x17", "\x17")),
	te("kill-yank", ky("hello world", "\x17", "\x01", "\x19")),
	te("kill-yank-midline", ky("one two three", "\x01", "\x1b[1;5C", "\x17", "\x01", "\x19")),
	te("kill-accumulate", ky("one two three", "\x17", "\x17", "\x01", "\x19")),
	te("kill-line-start-yank", ky("one two", "\x15", "\x19")),
	te("yank-pop-cycle", ky("one", "\x15", "two", "\x15", "\x19", "\x1by", "\x1by", "\x1by")),
	te("yank-empty-ring", ky("abc", "\x19")),
	te("yank-pop-without-yank", ky("one", "\x15", "two", "\x15", "\x1by")),
	te("undo-typing-coalesce", ky("h", "e", "l", "l", "o", "\x1f")),
	te("undo-space-word", ky("one", " ", "two", "\x1f", "\x1f")),
	te("undo-boundary", ky("ab", "\x1f", "\x1f", "\x1f")),
	te("undo-newline", ky("a", "\n", "b", "\x1f")),
	te("undo-delete", ky("abc", "\x7f", "\x1f")),
	te("undo-yank", ky("one two", "\x17", "\x01", "\x19", "\x1f")),
	te("undo-paste-small", [{ input: "ab" }, { input: pasteSeq("xy\nz") }, { input: "\x1f" }]),
	te("undo-settext", [{ setText: "replaced" }, { input: "\x1f" }]),
	te("undo-after-submit", ky("hi", "\r", "\x1f")),
	te("undo-retyped-boundary", ky("one", "\x1f", "two", "\x1f", "\x1f")),
	te("history-up-down", ky("draft", "\x1b[A", "\x1b[A", "\x1b[B", "\x1b[B"), { history: ["first", "second"] }),
	te("history-draft-restore", ky("draft", "\x1b[A", "\x1b[B"), { history: ["first"] }),
	te("history-up-from-text", ky("abc", "\x1b[A", "\x1b[A"), { history: ["first"] }),
	te("history-empty-up-down", ky("\x1b[A", "\x1b[A", "\x1b[B"), { history: ["only"] }),
	te("history-down-not-browsing", ky("abc", "\x1b[B")),
	te("history-add-mid-script", [{ addToHistory: "later" }, { input: "x" }, { input: "\x1b[A" }]),
	te("history-duplicate-skipped", [
		{ addToHistory: "same" },
		{ addToHistory: "same" },
		{ addToHistory: "other" },
		{ input: "\x1b[A" },
	]),
	te("history-blank-skipped", [{ addToHistory: "  " }, { addToHistory: "x" }, { input: "\x1b[A" }]),
	te("history-multiline-entry", [{ input: "\x1b[A" }], { history: ["one\ntwo"] }),
	te("history-submit-reads-marker", [{ input: pasteSeq(manyLines(12)) }, { input: "\r" }]),
	te("jump-forward", ky("foo bar baz", "\x01", "\x1d", { input: "b" }, { input: "\x1d" }, { input: "r" })),
	te("jump-backward", ky("foo bar baz", "\x05", "\x1b\x1d", { input: "r" })),
	te("jump-forward-crosses-lines", ky("abc", "\n", "xyz", "\x01", "\x1d", { input: "x" })),
	te("jump-cancel-repeat", ky("foo", "\x1d", "\x1d", { input: "b" })),
	te("jump-no-match", ky("foo", "\x05", "\x1d", { input: "z" })),
	te("jump-cancel-control", ky("foo", "\x01", "\x1d", "\x05", { input: "b" })),
	te("sticky-column-basic", ky("\x05", "\x1b[A", "\x1b[B"), { setText: "aaaa\nbb\ncccccc" }),
	te("sticky-column-clamp", ky("\x05", "\x1b[A"), { setText: "aaaaaa\nbb" }),
	te("sticky-column-reset-typing", ky("\x05", "\x1b[A", { input: "x" }, "\x1b[B"), { setText: "aaaa\nbb" }),
	te("sticky-column-reset-horizontal", ky("\x05", "\x1b[A", "\x1b[D", "\x1b[B"), { setText: "aaaa\nbb" }),
	te("wrap-widths", [{ render: 20 }, { render: 40 }, { render: 80 }], {
		setText: "the quick brown fox jumps over the lazy dog and keeps running far away",
	}),
	te("wrap-cjk", [{ render: 10 }, { render: 7 }], { setText: "世界你好世界你好世界こんにちは" }),
	te("wrap-emoji", [{ render: 9 }, { render: 11 }], { setText: "😀😀😀😀😀😀 family 👨‍👩‍👧‍👦" }),
	te("wrap-url", [{ render: 30 }, { render: 15 }], {
		setText: "see https://example.com/a/very/long/path/with/segments?query=1&x=2 for details",
	}),
	te("wrap-multiple-spaces", [{ render: 10 }], { setText: "a    b    ccccccccccc    dd" }),
	te("wrap-long-word", [{ render: 10 }, { render: 6 }], { setText: "aaaaaaaaaa bbbbbbbbbbbbbbbb" }),
	te("wrap-padding-x", [{ render: 20 }], { setText: "hello world this wraps", options: { paddingX: 2 } }),
	te("wrap-cursor-middle", ky("\x1b[1;5D", "\x1b[1;5D", { render: 20 }), {
		setText: "one two three four five six",
	}),
	te("wrap-empty", [{ render: 10 }], { setText: "" }),
	te("wrap-multiline", [{ render: 12 }], {
		setText: "first line here\nsecond line also long enough to wrap\nthird",
	}),
	te("paste-large-lines", [{ input: pasteSeq(manyLines(12)) }, { render: 40 }]),
	te("paste-large-chars", [{ input: pasteSeq("x".repeat(1001)) }, { render: 40 }]),
	te("paste-small-multiline", [{ input: pasteSeq("ab\ncd\nef") }]),
	te("paste-single-line", [{ input: pasteSeq("hello pasted") }]),
	te("paste-split-inputs", [{ input: "\x1b[200~abc" }, { input: "def\x1b[201~" }, { input: "z" }]),
	te("paste-trailing-input", [{ input: pasteSeq("xy") + "z" }]),
	te("paste-marker-navigation", [
		{ input: pasteSeq(manyLines(12)) },
		{ input: " tail" },
		{ input: "\x1b[D" },
		{ input: "\x1b[D" },
		{ input: "\x1b[1;5D" },
		{ input: "\x1b[1;5D" },
		{ render: 40 },
	]),
	te("paste-marker-backspace", [{ input: pasteSeq(manyLines(12)) }, { input: "\x7f" }]),
	te("paste-marker-forward-delete", [
		{ input: "pre " },
		{ input: pasteSeq(manyLines(12)) },
		{ input: "\x1b[1;5D" },
		{ input: "\x1b[3~" },
	]),
	te("paste-marker-undo", [{ input: pasteSeq(manyLines(12)) }, { input: "\x1f" }]),
	te("paste-two-markers-renumber", [
		{ input: pasteSeq(manyLines(12)) },
		{ input: pasteSeq(manyLines(15)) },
		{ input: "\x1b[1;5D" },
		{ input: "\x7f" },
	]),
	te("paste-crlf", [{ input: pasteSeq("a\r\nb\r\nc") }]),
	te("paste-control-filtered", [{ input: pasteSeq("ab\x07c\n\x1b[107;5u d") }]),
	te("paste-submit-expands", [{ input: pasteSeq(manyLines(12)) }, { input: "\r" }]),
	te("paste-unterminated", [{ input: "\x1b[200~abc" }]),
	te("paste-path-space-prepend", [{ input: "hello" }, { input: pasteSeq("/tmp/file") }]),
	te(
		"scroll-down-border",
		[...Array.from({ length: 8 }, () => ({ input: "\x1b[B" })), { render: 40 }],
		{ setText: manyLines(12) },
	),
	te("scroll-top-then-up", ky("\x1b[A", "\x1b[A"), { setText: "one\ntwo" }),
	te("scroll-page-down", [{ input: "\x1b[6~" }, { input: "\x1b[6~" }, { render: 40 }], { setText: manyLines(15) }),
	te("scroll-page-up", [{ input: "\x1b[6~" }, { input: "\x1b[5~" }, { render: 40 }], { setText: manyLines(15) }),
	te(
		"scroll-border-narrow",
		[{ render: 12 }, ...Array.from({ length: 8 }, () => ({ input: "\x1b[B" })), { render: 12 }],
		{ setText: manyLines(12) },
	),
	te("settext-crlf-cr", [], { setText: "a\r\nb\rc" }),
	te("settext-tabs", [], { setText: "a\tb\tc" }),
	te("insert-at-cursor", ky("hello", "\x1b[D", "\x1b[D", { insertTextAtCursor: "XY" })),
	te("insert-multiline-crlf", ky("abcd", "\x1b[D", "\x1b[D", { insertTextAtCursor: "1\r\n2" })),
	te("insert-empty-noop", [{ insertTextAtCursor: "" }, { input: "a" }]),
	te("csi-u-plain", ky("\x1b[97u", "\x1b[98u")),
	te("csi-u-shifted", ky("\x1b[97:65;2u", "\x1b[98:66;2u")),
	te("csi-u-modify-other-keys", ky("\x1b[27;2;98~", "\x1b[27;1;99~")),
	te("csi-u-ctrl-rejected", ky("x", "\x1b[97;5u", "y")),
	te("csi-u-emoji-codepoint", ky("\x1b[128512u")),
	te("tick-no-op", [{ input: "hi" }, { tickMs: 100 }, { render: 20 }]),
	te("render-focused-cursor-marker", [{ render: 20 }], { setText: "hi", focused: true }),

	// T5 Wave C: tui-main/tui-alt screen-level differential cases
	// (ported from upstream tui-render/tui-shrink/tui-cell-size-input and
	// tui-alt-screen tests — see wavec/*.mjs for provenance).
	...mainCases,
	...altCases,
	...interactionCases,
	...terminalImageCases,
	...markdownLexCases,
	...latexCases,
	...markdownRenderCases,
	...regressionCases,

	// ------------------------------------------------- mcp-servers (module)
	// core/mcp-servers.ts: tool name matching, config validation, exposure
	// resolution, loopback redirect URIs, and the WHATWG URL subset.
	{
		name: "mcp-servers/tool-name-matcher",
		tool: "mcp-servers",
		args: {
			fn: "createToolNameMatcher",
			entries: ["read_file", "b*", "*a", "x*y*z", "a*b", "mcp__srv__*"],
			names: [
				"read_file",
				"read_files",
				"bcd",
				"abc",
				"ba",
				"xb",
				"xbycz",
				"xcyz",
				"ab",
				"axb",
				"a\nb",
				"mcp__srv__tool",
				"",
			],
		},
	},
	{
		name: "mcp-servers/namespace-and-name-checks",
		tool: "mcp-servers",
		args: {
			fn: "mcpNamespace",
			servers: ["github", "my-server", "server.with.dots", "SERVER", "a b"],
		},
	},
	{
		name: "mcp-servers/validate-stdio",
		tool: "mcp-servers",
		args: {
			fn: "validateMcpServerConfig",
			name: "std",
			raw: {
				command: "node",
				args: ["server.js"],
				env: { A: "1" },
				exposure: "deferred",
				toolExposure: { "read_*": "direct", write_tool: "hidden" },
			},
		},
	},
	{
		name: "mcp-servers/validate-http",
		tool: "mcp-servers",
		args: {
			fn: "validateMcpServerConfig",
			name: "web",
			raw: {
				url: "https://example.com/mcp",
				exposure: "mcp",
				oauth: { clientId: "cid", scopes: "read write", redirectUri: "http://localhost:8931/callback" },
			},
		},
	},
	{
		name: "mcp-servers/validate-errors",
		tool: "mcp-servers",
		args: {
			fn: "validateMcpServerConfig",
			name: "bad",
			raw: { command: "node", exposure: "mcp" },
		},
	},
	{
		name: "mcp-servers/validate-missing-transport",
		tool: "mcp-servers",
		args: {
			fn: "validateMcpServerConfig",
			name: "bad",
			raw: { type: "stdio" },
		},
	},
	{
		name: "mcp-servers/validate-bad-tool-exposure",
		tool: "mcp-servers",
		args: {
			fn: "validateMcpServerConfig",
			name: "bad",
			raw: { command: "node", toolExposure: { "read_*": "direct", "(": "hidden" } },
		},
	},
	{
		name: "mcp-servers/tool-exposure",
		tool: "mcp-servers",
		args: {
			fn: "getMcpToolExposure",
			name: "srv",
			raw: {
				command: "node",
				exposure: "codemode",
				toolExposure: { "read_*": "direct", write_tool: "hidden", "*_file": "deferred", bad: "direct" },
			},
			toolNames: ["read_file", "write_tool", "some_file", "other", "readme"],
		},
	},
	{
		name: "mcp-servers/loopback-redirect-uri",
		tool: "mcp-servers",
		args: {
			fn: "isLoopbackRedirectUri",
			values: [
				"http://localhost:8931/callback",
				"http://127.0.0.1:80/cb",
				"http://[::1]:3000/cb",
				"https://localhost:8931/cb",
				"http://example.com/cb",
				"ui://localhost/cb",
				"http://localhost:8931",
				"not a url",
				"",
			],
		},
	},
	{
		name: "mcp-servers/parse-url",
		tool: "mcp-servers",
		args: {
			fn: "parseUrl",
			inputs: [
				"http://x:8080/p?q=1#f",
				"https://EXAMPLE.com/a/b",
				"ui://a/b.txt",
				"http://[::1]:3000/x",
				"http:example.com",
				"http:///path",
				"HTTP://x",
				"http://user:pw@host:99/p",
				"http://x:99999/p",
				"http://x:nope/p",
				"noscheme",
				"  http://x  ",
				"http://x\t/y",
			],
		},
	},

	// ---------------------------------------------------- mcp-tools (module)
	// extensions/mcp/tools.ts: tool names, result schemas, content conversion.
	{
		name: "mcp-tools/tool-names",
		tool: "mcp-tools",
		args: {
			fn: "createMcpToolName",
			calls: [
				{ server: "srv", tool: "read_file" },
				{ server: "my-server", tool: "read file" },
				{ server: "srv", tool: "a-b" },
				{ server: "srv", tool: "a_b" },
				{ server: "srv", tool: "ünïcödé" },
				{ server: "srv", tool: "𝄞-sharp" },
				{ server: "srv", tool: "x".repeat(80) },
				{ server: "s", tool: "t", taken: ["mcp__s__t"] },
				{ server: "srv", tool: "y".repeat(80), taken: [] },
			],
		},
	},
	{
		name: "mcp-tools/result-schema",
		tool: "mcp-tools",
		args: {
			fn: "createMcpResultSchema",
			structuredContentSchema: {
				type: "object",
				properties: { a: { $ref: "#/$defs/X" }, b: { enum: ["#/$defs/X"] } },
				$defs: { X: { type: "string" } },
				anyOf: [{ $ref: "#" }],
			},
		},
	},
	{
		name: "mcp-tools/result-schema-undefined",
		tool: "mcp-tools",
		args: { fn: "createMcpResultSchema" },
	},
	{
		name: "mcp-tools/to-tool-exposure",
		tool: "mcp-tools",
		args: { fn: "toToolExposure", exposures: ["codemode", "deferred", "direct", "hidden"] },
	},
	{
		name: "mcp-tools/to-model-content",
		tool: "mcp-tools",
		args: {
			fn: "toModelContent",
			server: "srv",
			readableResources: true,
			blocks: [
				{ type: "text", text: "hello" },
				{ type: "image", data: "AAAA", mimeType: "image/png" },
				{ type: "audio", data: "AAAA", mimeType: "audio/wav" },
				{
					type: "resource_link",
					uri: "ui://x/y.png",
					name: "res",
					title: "T",
					description: "d",
					mimeType: "image/png",
					size: 2048,
				},
				{ type: "resource_link", uri: "u", name: "n", description: "" },
				{ type: "resource", resource: { uri: "ui://t.txt", mimeType: "text/plain", blob: "aGVsbG8=" } },
				{ type: "resource", resource: { uri: "ui://b.bin", mimeType: "application/octet-stream", blob: "AAAA" } },
				{ type: "resource", resource: { uri: "ui://j.json", mimeType: "application/json", blob: "eyJ4IjoxfQ==" } },
			],
		},
	},
	{
		name: "mcp-tools/to-model-content-not-readable",
		tool: "mcp-tools",
		args: {
			fn: "toModelContent",
			server: "srv",
			readableResources: false,
			blocks: [{ type: "resource_link", uri: "u", name: "n", title: undefined }],
		},
	},
	{
		name: "mcp-tools/convert-result",
		tool: "mcp-tools",
		args: {
			fn: "convertMcpResult",
			server: "s",
			tool: "t",
			result: {
				content: [{ type: "text", text: "hi" }],
				structuredContent: { x: 1 },
				_meta: { progress: 0.5 },
			},
		},
	},
	{
		name: "mcp-tools/convert-result-error-no-text",
		tool: "mcp-tools",
		args: {
			fn: "convertMcpResult",
			server: "s",
			tool: "t",
			result: { isError: true, structuredContent: { code: 5 } },
		},
	},
	{
		name: "mcp-tools/convert-result-error-with-text",
		tool: "mcp-tools",
		args: {
			fn: "convertMcpResult",
			server: "s",
			tool: "t",
			result: { isError: true, content: [{ type: "text", text: "boom" }] },
		},
	},
	{
		name: "mcp-tools/limit-content",
		tool: "mcp-tools",
		args: {
			fn: "limitMcpContent",
			content: [
				{ type: "text", text: "short" },
				{ type: "image", data: "AAAA", mimeType: "image/png" },
			],
		},
	},
	{
		name: "mcp-tools/limit-content-truncated",
		tool: "mcp-tools",
		args: {
			fn: "limitMcpContent",
			content: [
				{ type: "text", text: "x".repeat(25000) },
				{ type: "image", data: "AAAA", mimeType: "image/png" },
			],
		},
	},

	// ------------------------------------------------- mcp-section (module)
	// extensions/mcp/index.ts renderServersSection: the `mcp_servers`
	// system-prompt section (filtering, sorting, description budget,
	// omission lines, UTF-16 truncation).
	...(() => {
		const ms = (name, servers) => ({ tool: "mcp-section", name: "mcs/" + name, args: { servers } });
		const cmd = (extra = {}) => ({ command: "node", ...extra });
		const long = (n) => "z".repeat(n);
		const manyDescriptions = Array.from({ length: 20 }, (_, i) => ({
			name: String.fromCharCode(97 + i),
			config: cmd({ description: "d".repeat(240) + i }),
		}));
		return [
			ms("empty", []),
			ms("direct-only", [{ name: "a", config: cmd({ exposure: "direct" }) }]),
			ms("disabled-only", [{ name: "a", config: cmd({ enabled: false }) }]),
			ms("hidden-only", [{ name: "a", config: cmd({ exposure: "hidden" }) }]),
			ms("sorted-mixed", [
				{ name: "beta", config: cmd({ description: "  Second\nmore " }) },
				{ name: "alpha", config: cmd({ exposure: "deferred", description: "First" }) },
				{ name: "gamma", config: { url: "https://example.test/mcp" }, instructions: "Remote" },
			]),
			ms("instructions-fallback", [
				{ name: "a", config: cmd({ description: "   " }), instructions: "Instr\nsecond" },
				{ name: "b", config: cmd(), instructions: "\nleading newline" },
			]),
			ms("tool-exposure-indirect", [
				{ name: "a", config: cmd({ exposure: "direct", toolExposure: { t: "codemode" } }) },
				{ name: "b", config: cmd({ exposure: "hidden", toolExposure: { t: "deferred" } }) },
			]),
			ms("multiline-description", [
				{ name: "a", config: cmd({ description: "First line\nsecond line" }) },
			]),
			ms("crlf-description", [{ name: "a", config: cmd({ description: "First\r\nsecond" }) }]),
			ms("long-description", [{ name: "a", config: cmd({ description: "d".repeat(400) }) }]),
			ms("emoji-description", [
				{ name: "a", config: cmd({ description: "d".repeat(249) + "🔥tail" }) },
			]),
			ms("one-omitted", [
				{ name: "a", config: cmd() },
				{ name: long(4000), config: cmd() },
			]),
			ms("all-omitted", [
				{ name: "0" + long(4000), config: cmd() },
				{ name: "1" + long(4000), config: cmd() },
				{ name: "2" + long(4000), config: cmd() },
			]),
			ms("description-budget-shared", manyDescriptions),
			ms("scope-and-override", [
				{
					name: "a",
					config: cmd({ description: "Project" }),
					source: "/proj/.pi/mcp.json",
					scope: "global",
					override: "/proj/.pi/mcp.json",
				},
			]),
		];
	})(),
	// ----------------------------------------------- mcp-extension (module)
	// extensions/mcp/index.ts createMcpExtension: session lifecycle, config
	// vs registered-server merge, discovery activation, mcp_servers section,
	// /mcp status/usage/picker messages, argument completions, shutdown.
	// Cases avoid enabled servers where a later op would await `pending`
	// (connections arrive with the next slice).
	...(() => {
		const mce = (name, args) => ({ tool: "mcp-extension", name: "mce/" + name, args });
		const cmd = (extra = {}) => ({ command: "node", ...extra });
		const disabled = (name, config = cmd()) => ({
			name,
			config: { ...cmd(), ...config, enabled: false },
			source: "/g/mcp.json",
			scope: "global",
		});
		const host = (extra = {}) => ({
			cwd: "/proj",
			agentDir: "/home/u/.pi",
			projectTrusted: true,
			activeTools: ["read"],
			...extra,
		});
		const sectionEvent = (sections) => ({
			type: "before_agent_start",
			systemPromptOptions: { cwd: "/proj", sections },
		});
		// Scripted transports (same schema as mcc/) for the servers an
		// enabled config connects.
		const rpc = (id, result) => ({ message: { jsonrpc: "2.0", id, result } });
		const rpcErr = (id, code, message) => ({
			message: { jsonrpc: "2.0", id, error: { code, message } },
		});
		const initResult = (capabilities, extra) => ({
			protocolVersion: "2025-11-25",
			capabilities,
			serverInfo: { name: "srv", version: "1.0" },
			...(extra || {}),
		});
		const connectSends = (initRes, tools) => [
			[rpc(1, initRes)],
			[],
			...(tools === null ? [] : [[rpc(2, { tools })]]),
		];
		const failConnect = () => ({ sendSteps: [[rpcErr(1, -32603, "boom")]] });
		const configured = (name, config) => ({
			name,
			config,
			source: "/g/mcp.json",
			scope: "global",
		});
		const tool = (name, inputSchema, extra) => ({ name, inputSchema, ...(extra || {}) });
		return [
			mce("status-and-usage", {
				host: host(),
				loadConfig: {
					servers: [
						disabled("off"),
						disabled("gone", { url: "https://example.test/mcp" }),
					],
					errors: ["bad file"],
					projectConfig: "/proj/.pi/mcp.json",
				},
				ops: [
					{ op: "sessionStart" },
					{ op: "command", args: "" },
					{ op: "command", args: "bogus" },
					{ op: "command", args: "login" },
					{ op: "command", args: "logout" },
					{ op: "command", args: "reconnect" },
					{ op: "command", args: "login missing" },
					{ op: "command", args: "reconnect x" },
					{ op: "command", args: "login a b" },
					{ op: "command", args: "   " },
					{ op: "turnStart" },
					{ op: "toolCall", event: { type: "tool_call", toolName: "read", input: {} } },
					{ op: "shutdown" },
					{ op: "command", args: "" },
				],
			}),
			mce("discovery-codemode", {
				host: host({
					tools: [
						{ name: "codemode", reachesTools: true },
						{ name: "tool_search", reachesTools: true },
					],
				}),
				loadConfig: { servers: [{ name: "d", config: cmd({ exposure: "codemode" }) }] },
				transports: [failConnect()],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "beforeAgentStart", event: sectionEvent({ custom: "C" }) },
					{ op: "shutdown" },
				],
			}),
			mce("discovery-tool-search", {
				host: host({
					tools: [
						{ name: "codemode", reachesTools: true },
						{ name: "tool_search", reachesTools: true },
					],
				}),
				loadConfig: { servers: [{ name: "d", config: cmd({ exposure: "deferred" }) }] },
				ops: [{ op: "sessionStart" }, { op: "shutdown" }],
			}),
			mce("discovery-both", {
				host: host({
					tools: [
						{ name: "codemode", reachesTools: true },
						{ name: "tool_search", reachesTools: true },
					],
				}),
				loadConfig: {
					servers: [
						{ name: "a", config: cmd({ exposure: "deferred" }) },
						{ name: "b", config: cmd({ exposure: "codemode" }) },
					],
				},
				ops: [{ op: "sessionStart" }, { op: "shutdown" }],
			}),
			mce("discovery-already-active", {
				host: host({
					activeTools: ["codemode", "read"],
					tools: [{ name: "codemode", reachesTools: true }],
				}),
				loadConfig: { servers: [{ name: "d", config: cmd() }] },
				ops: [{ op: "sessionStart" }, { op: "shutdown" }],
			}),
			mce("discovery-warning-autoenable-off", {
				host: host({
					activeTools: [],
					tools: [{ name: "codemode", reachesTools: true }],
				}),
				loadConfig: {
					autoEnableCodemode: false,
					servers: [{ name: "d", config: cmd() }],
				},
				ops: [{ op: "sessionStart" }, { op: "shutdown" }],
			}),
			mce("discovery-warning-no-reaching-tool", {
				host: host({
					activeTools: [],
					tools: [
						{ name: "codemode" },
						{ name: "tool_search", reachesTools: true },
					],
				}),
				loadConfig: { servers: [{ name: "d", config: cmd() }] },
				ops: [{ op: "sessionStart" }, { op: "shutdown" }],
			}),
			mce("discovery-warning-once", {
				host: host({ activeTools: [], tools: [{ name: "codemode" }] }),
				loadConfig: {
					servers: [
						{ name: "a", config: cmd() },
						{ name: "b", config: cmd() },
					],
				},
				ops: [{ op: "sessionStart" }, { op: "serversChange" }, { op: "shutdown" }],
			}),
			mce("registered-merge-and-override", {
				host: host({
					registeredServers: [
						{ name: "reg", config: { ...cmd(), enabled: false }, extensionPath: "/ext/a.js" },
						{ name: "cfg", config: { ...cmd(), enabled: false }, extensionPath: "/ext/b.js" },
						{ name: "my-srv", config: { ...cmd(), enabled: false }, extensionPath: "/ext/c.js" },
					],
				}),
				loadConfig: {
					servers: [
						{ name: "cfg", config: { ...cmd(), enabled: false }, source: "/g/mcp.json", scope: "global" },
						{ name: "my_srv", config: { ...cmd(), enabled: false }, source: "/g/mcp.json", scope: "global" },
					],
				},
				ops: [{ op: "sessionStart" }, { op: "command", args: "" }, { op: "shutdown" }],
			}),
			mce("servers-change-lifecycle", {
				host: host({
					registeredServers: [
						{ name: "a", config: { ...cmd({ exposure: "direct" }), enabled: false }, extensionPath: "/ext/a.js" },
					],
				}),
				loadConfig: { servers: [] },
				ops: [
					{ op: "sessionStart" },
					{ op: "command", args: "" },
					{ op: "setRegistered", servers: [
						{ name: "a", config: { ...cmd({ exposure: "deferred" }), enabled: false }, extensionPath: "/ext/a.js" },
					] },
					{ op: "serversChange" },
					{ op: "command", args: "" },
					{ op: "setRegistered", servers: [] },
					{ op: "serversChange" },
					{ op: "command", args: "" },
					{ op: "shutdown" },
				],
			}),
			mce("servers-change-ignores-configured", {
				host: host({
					registeredServers: [
						{ name: "cfg", config: { ...cmd(), enabled: false }, extensionPath: "/ext/b.js" },
					],
				}),
				loadConfig: { servers: [disabled("cfg")] },
				ops: [{ op: "sessionStart" }, { op: "serversChange" }, { op: "command", args: "" }, { op: "shutdown" }],
			}),
			mce("section-remove-when-none", {
				host: host(),
				loadConfig: { servers: [disabled("direct", { exposure: "direct" })] },
				ops: [
					{ op: "sessionStart" },
					{ op: "beforeAgentStart", event: sectionEvent({ mcp_servers: "stale", custom: "C" }) },
					{ op: "shutdown" },
				],
			}),
			mce("section-insert-and-order", {
				host: host(),
				loadConfig: {
					servers: [
						{ name: "b", config: cmd({ description: "Bee" }) },
						{ name: "a", config: cmd({ exposure: "deferred" }) },
					],
				},
				transports: [failConnect(), failConnect()],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "beforeAgentStart", event: sectionEvent({ custom: "C" }) },
					{ op: "beforeAgentStart", event: sectionEvent({ mcp_servers: "old", custom: "C" }) },
					{ op: "shutdown" },
				],
			}),
			mce("completions", {
				host: host(),
				loadConfig: { servers: [] },
				ops: [
					{ op: "sessionStart" },
					{ op: "completions", prefix: "" },
					{ op: "completions", prefix: "lo" },
					{ op: "completions", prefix: "  login" },
					{ op: "completions", prefix: "logi n" },
					{ op: "completions", prefix: "login " },
					{ op: "completions", prefix: "reconnect " },
					{ op: "completions", prefix: "login a b" },
					{ op: "shutdown" },
				],
			}),
			mce("connect-status", {
				host: host(),
				loadConfig: {
					servers: [configured("s", cmd({ description: "Srv" }))],
				},
				transports: [
					{
						sendSteps: connectSends(
							initResult({ tools: {}, resources: {} }, { instructions: "Be nice" }),
							[tool("greet", { type: "object" })],
						),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "command", args: "" },
					{ op: "shutdown" },
				],
			}),
			mce("connect-failed-report", {
				host: host(),
				loadConfig: { servers: [configured("s", cmd())] },
				transports: [failConnect()],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "command", args: "" },
					{ op: "shutdown" },
				],
			}),
			mce("connect-instructions-section", {
				host: host(),
				loadConfig: {
					servers: [configured("s", cmd({ exposure: "codemode" }))],
				},
				transports: [
					{
						sendSteps: connectSends(
							initResult({ tools: {} }, { instructions: "  Use me first  \nsecond" }),
							[tool("t", { type: "object" })],
						),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "beforeAgentStart", event: sectionEvent({ custom: "C" }) },
					{ op: "shutdown" },
				],
			}),
			mce("servers-change-connect", {
				host: host(),
				loadConfig: { servers: [] },
				transports: [
					{
						sendSteps: connectSends(initResult({ tools: {} }), [
							tool("added", { type: "object" }),
						]),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "setRegistered", servers: [
						{ name: "r", config: cmd(), extensionPath: "/ext/r.js" },
					] },
					{ op: "serversChange" },
					{ op: "command", args: "" },
					{ op: "shutdown" },
				],
			}),
			mce("servers-change-remove-closes", {
				host: host({
					registeredServers: [
						{ name: "r", config: cmd(), extensionPath: "/ext/r.js" },
					],
				}),
				loadConfig: { servers: [] },
				transports: [
					{
						sendSteps: connectSends(initResult({ tools: {} }), [
							tool("t", { type: "object" }),
						]),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "command", args: "" },
					{ op: "setRegistered", servers: [] },
					{ op: "serversChange" },
					{ op: "command", args: "" },
					{ op: "shutdown" },
				],
			}),
			mce("shutdown-after-connect", {
				host: host(),
				loadConfig: { servers: [configured("s", cmd())] },
				transports: [
					{ sendSteps: connectSends(initResult({ tools: {} }), [tool("t", { type: "object" })]) },
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "shutdown" },
				],
			}),
			mce("connect-registers-tools", {
				host: host({ activeTools: ["read"] }),
				loadConfig: {
					servers: [
						configured(
							"s",
							cmd({ toolExposure: { plain: "direct", hiddenTool: "hidden" } }),
						),
					],
				},
				transports: [
					{
						sendSteps: connectSends(initResult({ tools: {} }), [
							{
								name: "greet",
								description: "  Say hi  ",
								inputSchema: {
									type: "object",
									properties: { who: { type: "string" } },
									required: ["who"],
								},
								annotations: { readOnlyHint: true, openWorldHint: false },
							},
							{ name: "plain", title: "Plain title" },
							{ name: "hiddenTool", description: "Hidden" },
							{ name: "bare" },
						]),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "command", args: "" },
					{ op: "shutdown" },
				],
			}),
			mce("connect-name-collisions", {
				host: host(),
				loadConfig: { servers: [configured("s", cmd())] },
				transports: [
					{
						sendSteps: connectSends(initResult({ tools: {} }), [
							{ name: "a-b" },
							{ name: "a_b" },
							{ name: "a b" },
							{ name: "a-b", description: "again" },
						]),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "shutdown" },
				],
			}),
			mce("connect-resources-exposure", {
				host: host(),
				loadConfig: {
					servers: [configured("s", cmd({ exposure: "deferred" }))],
				},
				transports: [
					{
						sendSteps: connectSends(initResult({ tools: {}, resources: {} }), [
							tool("t", { type: "object" }),
						]),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "shutdown" },
				],
			}),
			mce("servers-change-remove-hides-tools", {
				host: host({
					registeredServers: [{ name: "r", config: cmd(), extensionPath: "/ext/r.js" }],
				}),
				loadConfig: { servers: [] },
				transports: [
					{
						sendSteps: connectSends(initResult({ tools: {} }), [
							tool("one", { type: "object" }),
							tool("two", { type: "object" }),
						]),
					},
				],
				ops: [
					{ op: "sessionStart" },
					{ op: "settle" },
					{ op: "setRegistered", servers: [] },
					{ op: "serversChange" },
					{ op: "shutdown" },
				],
			}),
			mce("shutdown-clears-status", {
				host: host(),
				loadConfig: { servers: [disabled("off")] },
				ops: [
					{ op: "sessionStart" },
					{ op: "shutdown" },
					{ op: "command", args: "" },
				],
			}),
		];
	})(),
	// ------------------------------------------------ mcp-resources (module)
	// extensions/mcp/resources.ts: list_mcp_resources,
	// list_mcp_resource_templates, read_mcp_resource over scripted fake
	// servers (see the ts-harness "mcp-resources" entry for the scripting
	// protocol: pages consumed per call, all*/readResource outcome specs,
	// shared call log).
	{
		name: "mcr/definitions",
		tool: "mcp-resources",
		args: { fn: "definitions", exposure: "direct" },
	},
	{
		name: "mcr/definitions-exposures",
		tool: "mcp-resources",
		args: { fn: "definitions", exposure: "codemode" },
	},
	{
		name: "mcr/definitions-deferred",
		tool: "mcp-resources",
		args: { fn: "definitions", exposure: "deferred" },
	},
	{
		name: "mcr/definitions-hidden",
		tool: "mcp-resources",
		args: { fn: "definitions", exposure: "hidden" },
	},
	{
		name: "mcr/list-page",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [
				{
					name: "alpha",
					timeoutMs: 1500,
					resourcesPages: [
						{
							resources: [
								{ uri: "file:///a.txt", name: "a", title: "A", mimeType: "text/plain", size: 10, _meta: { progress: 1 } },
								{ uri: "file:///b.json", name: "b", description: "config", size: 2048.5 },
							],
							nextCursor: "c2",
						},
						{ resources: [{ uri: "file:///c.txt", name: "c" }] },
					],
				},
			],
		},
	},
	{
		name: "mcr/list-page-no-next-cursor",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [
				{ name: "alpha", resourcesPages: [{ resources: [{ uri: "u", name: "n" }] }] },
			],
		},
	},
	{
		name: "mcr/list-page-null-next-cursor",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [
				{ name: "alpha", resourcesPages: [{ resources: [], nextCursor: null }] },
			],
		},
	},
	{
		name: "mcr/list-page-cursor-continuation",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha", cursor: "c2" },
			servers: [
				{
					name: "alpha",
					resourcesPages: [
						{ resources: [{ uri: "u1", name: "n1" }], nextCursor: "c2" },
						{ resources: [{ uri: "u2", name: "n2" }], nextCursor: "c3" },
					],
				},
			],
		},
	},
	{
		name: "mcr/list-page-empty",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [{ name: "alpha", resourcesPages: [{ resources: [] }] }],
		},
	},
	{
		name: "mcr/list-page-error",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [{ name: "alpha", resourcesPages: [{ error: "page fetch failed" }] }],
		},
	},
	{
		name: "mcr/list-page-reject-value",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [{ name: "alpha", resourcesPages: [{ reject: 42 }] }],
		},
	},
	{
		name: "mcr/list-page-unscripted",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha" },
			servers: [{ name: "alpha", allResources: { items: [] } }],
		},
	},
	{
		name: "mcr/list-all-multi",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: {},
			servers: [
				{
					name: "zeta",
					timeoutMs: 2000,
					allResources: { items: [{ uri: "z://1", name: "z1" }, { uri: "z://2", name: "z2" }] },
				},
				{ name: "alpha", allResources: { items: [{ uri: "a://1", name: "a1", _meta: { x: 1 }, icons: [{ src: "i" }] }] } },
				{ name: "mid", allResources: { items: [] } },
			],
		},
	},
	{
		name: "mcr/list-all-sort-order",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: {},
			servers: [
				{ name: "zebra", allResources: { items: [{ uri: "z://1", name: "z" }] } },
				{ name: "apple", allResources: { items: [{ uri: "a://1", name: "a" }] } },
				{ name: "banana2", allResources: { items: [{ uri: "b2://1", name: "b2" }] } },
				{ name: "banana10", allResources: { items: [{ uri: "b10://1", name: "b10" }] } },
				{ name: "mcp-docs", allResources: { items: [{ uri: "d://1", name: "d1" }] } },
				{ name: "mcpdocs", allResources: { items: [{ uri: "d://2", name: "d2" }] } },
				{ name: "docs", allResources: { items: [{ uri: "d://3", name: "d3" }] } },
				{ name: "docs", allResources: { items: [{ uri: "d://4", name: "d4" }] } },
			],
		},
	},
	{
		name: "mcr/list-all-empty-servers",
		tool: "mcp-resources",
		args: { fn: "execute", tool: "list_mcp_resources", params: {}, servers: [] },
	},
	{
		name: "mcr/list-all-mcp-app-filter",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: {},
			servers: [
				{
					name: "apps",
					allResources: {
						items: [
							{ uri: "ui://host/panel", name: "panel" },
							{ uri: "file:///keep.txt", name: "keep" },
							{ uri: "mem://x", name: "html", mimeType: "text/html; profile=mcp-app" },
							{ uri: "mem://y", name: "html2", mimeType: "text/html;\tPROFILE = \"mcp-app\"" },
							{ uri: "mem://z", name: "html3", mimeType: 'text/html;profile="mcp-app' },
							{ uri: "mem://w", name: "web", mimeType: "text/html; profile=web-app" },
							{ uri: "mem://v", name: "near", mimeType: "text/html;profileX=mcp-app" },
							{ uri: "mem://u", name: "nosemi", mimeType: "profile=mcp-app" },
							{ uri: "file:///icon.png", name: "icon", icons: [{ src: "x" }] },
						],
					},
				},
			],
		},
	},
	{
		name: "mcr/list-all-per-server-error",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: {},
			servers: [
				{ name: "a", allResources: { items: [{ uri: "a://1", name: "a1" }] } },
				{ name: "b", allResources: { error: "connection lost" } },
				{ name: "c", allResources: { items: [{ uri: "c://1", name: "c1" }] } },
			],
		},
	},
	{
		name: "mcr/list-all-reject-non-error",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: {},
			servers: [
				{ name: "a", allResources: { reject: 42 } },
				{ name: "b", allResources: { reject: { x: 1 } } },
				{ name: "c", allResources: { reject: true } },
				{ name: "d", allResources: { reject: "plain string" } },
			],
		},
	},
	{
		name: "mcr/list-all-mixed",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: {},
			servers: [
				{ name: "broken", timeoutMs: 3000, allResources: { reject: 7 } },
				{ name: "works", allResources: { items: [{ uri: "w://1", name: "w1", _meta: null }] } },
				{ name: "empty", allResources: { items: [] } },
			],
		},
	},
	{
		name: "mcr/list-all-blank-server-param",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "   " },
			servers: [{ name: "alpha", allResources: { items: [{ uri: "a://1", name: "a1" }] } }],
		},
	},
	{
		name: "mcr/list-server-not-found",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "nope" },
			servers: [
				{ name: "zeta", allResources: { items: [] } },
				{ name: "alpha", allResources: { items: [] } },
			],
		},
	},
	{
		name: "mcr/list-server-not-found-no-servers",
		tool: "mcp-resources",
		args: { fn: "execute", tool: "list_mcp_resources", params: { server: "nope" }, servers: [] },
	},
	{
		name: "mcr/list-server-whitespace",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "  alpha\t" },
			servers: [{ name: "alpha", resourcesPages: [{ resources: [{ uri: "u", name: "n" }] }] }],
		},
	},
	{
		name: "mcr/list-cursor-without-server",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { cursor: "x" },
			servers: [{ name: "alpha", allResources: { items: [] } }],
		},
	},
	{
		name: "mcr/list-blank-cursor-without-server",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { cursor: "   " },
			servers: [{ name: "alpha", allResources: { items: [{ uri: "a://1", name: "a1" }] } }],
		},
	},
	{
		name: "mcr/list-non-string-server",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: 42 },
			servers: [],
		},
	},
	{
		name: "mcr/list-non-string-cursor",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resources",
			params: { server: "alpha", cursor: 42 },
			servers: [{ name: "alpha", resourcesPages: [] }],
		},
	},
	{
		name: "mcr/templates-page",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resource_templates",
			params: { server: "alpha" },
			servers: [
				{
					name: "alpha",
					templatesPages: [
						{
							resourceTemplates: [
								{ uriTemplate: "mem://{key}", name: "mem", description: "by key", mimeType: "text/plain" },
								{ uriTemplate: "ui://host/{panel}", name: "panel", _meta: { a: 1 } },
							],
							nextCursor: "t2",
						},
					],
				},
			],
		},
	},
	{
		name: "mcr/templates-all",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resource_templates",
			params: {},
			servers: [
				{
					name: "zeta",
					allResourceTemplates: { items: [{ uriTemplate: "z://{id}", name: "z", mimeType: "text/html; profile=mcp-app" }] },
				},
				{
					name: "alpha",
					allResourceTemplates: { items: [{ uriTemplate: "a://{x}/{y}", name: "a", title: "A" }] },
				},
			],
		},
	},
	{
		name: "mcr/templates-all-errors",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resource_templates",
			params: {},
			servers: [
				{ name: "a", allResourceTemplates: { items: [{ uriTemplate: "a://{x}", name: "a" }] } },
				{ name: "b", allResourceTemplates: { error: "templates unavailable" } },
				{ name: "c", allResourceTemplates: { reject: "raw" } },
			],
		},
	},
	{
		name: "mcr/templates-page-error",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resource_templates",
			params: { server: "alpha" },
			servers: [{ name: "alpha", templatesPages: [{ reject: "no templates here" }] }],
		},
	},
	{
		name: "mcr/templates-page-unscripted",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "list_mcp_resource_templates",
			params: { server: "alpha" },
			servers: [{ name: "alpha", allResourceTemplates: { items: [] } }],
		},
	},
	{
		name: "mcr/read-single-text",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "file:///a.txt" },
			servers: [
				{
					name: "alpha",
					readResource: {
						contents: [{ uri: "file:///a.txt", mimeType: "text/plain", text: "hello world" }],
					},
				},
			],
		},
	},
	{
		name: "mcr/read-multi",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "file:///dir" },
			servers: [
				{
					name: "alpha",
					readResource: {
						contents: [
							{ uri: "file:///dir/x.txt", mimeType: "text/plain", text: "xx" },
							{ mimeType: "text/plain", text: "no uri" },
							{ uri: null, text: "null uri" },
						],
					},
				},
			],
		},
	},
	{
		name: "mcr/read-image",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "file:///img.png" },
			servers: [
				{
					name: "alpha",
					readResource: {
						contents: [{ uri: "file:///img.png", mimeType: "image/png", blob: "aGVsbG8=" }],
					},
				},
			],
		},
	},
	{
		name: "mcr/read-empty",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "mem://empty" },
			servers: [{ name: "alpha", readResource: { contents: [] } }],
		},
	},
	{
		name: "mcr/read-null-contents",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "mem://x" },
			servers: [{ name: "alpha", readResource: { contents: null } }],
		},
	},
	{
		name: "mcr/read-non-object-contents",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "mem://x" },
			servers: [
				{
					name: "alpha",
					readResource: { contents: [42, "str", { uri: "mem://real", text: "real" }] },
				},
			],
		},
	},
	{
		name: "mcr/read-meta-strip",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "file:///a.txt" },
			servers: [
				{
					name: "alpha",
					readResource: {
						contents: [{ uri: "file:///a.txt", mimeType: "text/plain", text: "data", _meta: { p: 1 } }],
					},
				},
			],
		},
	},
	{
		name: "mcr/read-error",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "mem://x" },
			servers: [{ name: "alpha", readResource: { error: "read failed" } }],
		},
	},
	{
		name: "mcr/read-reject-value",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "mem://x" },
			servers: [{ name: "alpha", readResource: { reject: 42 } }],
		},
	},
	{
		name: "mcr/read-missing-server",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { uri: "mem://x" },
			servers: [{ name: "alpha", readResource: { contents: [] } }],
		},
	},
	{
		name: "mcr/read-missing-uri",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "   " },
			servers: [{ name: "alpha", readResource: { contents: [] } }],
		},
	},
	{
		name: "mcr/read-non-string-uri",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: 42 },
			servers: [{ name: "alpha", readResource: { contents: [] } }],
		},
	},
	{
		name: "mcr/read-server-not-found",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "nope", uri: "mem://x" },
			servers: [{ name: "alpha", readResource: { contents: [] } }],
		},
	},
	{
		name: "mcr/read-unscripted-defaults-empty",
		tool: "mcp-resources",
		args: {
			fn: "execute",
			tool: "read_mcp_resource",
			params: { server: "alpha", uri: "mem://x" },
			servers: [{ name: "alpha" }],
		},
	},

	// -------------------------------------------------- tool-search (module)
	// extensions/tool-search/tool.ts: tokenizer, documents, BM25 ranker.
	{
		name: "tool-search/tokenize",
		tool: "tool-search",
		args: {
			fn: "tokenize",
			texts: [
				"FooBar_baz HTTPServer2 x-y",
				"a1b2 camelCase snake_case kebab-case dots.periods",
				"get_issues searches classes analyses the AND with",
				"buses uses classes",
				"process address",
				"is ties",
				"",
				"...",
				"héllo wörld",
			],
		},
	},
	{
		name: "tool-search/document",
		tool: "tool-search",
		args: {
			fn: "createToolSearchDocument",
			tool: {
				name: "list_issues",
				description: "Lists GitHub issues",
				parameters: {
					type: "object",
					description: "params",
					properties: {
						state: { type: "string", description: "open or closed" },
						labels: { type: "array", items: { type: "string", description: "label names" } },
					},
					anyOf: [{ type: "string", description: "alt" }],
				},
			},
			namespace: { name: "github", description: "GitHub tools", instructions: "Use for repo work" },
		},
	},
	{
		name: "tool-search/document-minimal",
		tool: "tool-search",
		args: {
			fn: "createToolSearchDocument",
			tool: { name: "t", description: "", parameters: "not an object" },
		},
	},
	{
		name: "tool-search/rank",
		tool: "tool-search",
		args: {
			fn: "rank",
			query: "file read",
			documents: [
				{ name: "read_file", text: "read file contents filesystem" },
				{ name: "write_file", text: "write file save disk" },
				{ name: "search_web", text: "search the web internet" },
			],
			limit: 8,
		},
	},
	{
		name: "tool-search/rank-params-and-ties",
		tool: "tool-search",
		args: {
			fn: "rank",
			query: "file",
			documents: [
				{ name: "read_file", text: "read file contents filesystem" },
				{ name: "write_file", text: "write file save disk" },
				{ name: "search_web", text: "search the web internet" },
				{ name: "a", text: "file" },
				{ name: "b", text: "file" },
			],
			limit: 3,
			k1: 2.0,
			b: 0.0,
		},
	},

	// ------------------------------------------------- config-value (module)
	// core/resolve-config-value.ts: "$VAR"/"${VAR}" templates, "!cmd" shell
	// values, and header resolution. Env is always passed explicitly and only
	// uses the reserved PI_PARITY_ prefix, which never exists in either
	// harness process's environment (the TS side falls back to process.env).
	// Commands run for real via the shell: only deterministic ones are used.
	cv("resolve-single-env-ref", "resolveConfigValue", "$PI_PARITY_KEY", { PI_PARITY_KEY: "sk-secret" }),
	cv("resolve-braced-env-ref", "resolveConfigValue", "${PI_PARITY_KEY}", { PI_PARITY_KEY: "sk-secret" }),
	cv("resolve-mixed-literal-and-ref", "resolveConfigValue", "Bearer $PI_PARITY_KEY", {
		PI_PARITY_KEY: "sk-secret",
	}),
	cv("resolve-multiple-refs", "resolveConfigValue", "$PI_PARITY_A/${PI_PARITY_B}/$PI_PARITY_A", {
		PI_PARITY_A: "one",
		PI_PARITY_B: "two",
	}),
	cv("resolve-missing-var-null", "resolveConfigValue", "$PI_PARITY_MISSING", {}),
	cv("resolve-empty-env-value-unset", "resolveConfigValue", "$PI_PARITY_KEY", { PI_PARITY_KEY: "" }),
	cv("resolve-dollar-alone", "resolveConfigValue", "$", {}),
	cv("resolve-dollar-dollar-escape", "resolveConfigValue", "a$$b", {}),
	cv("resolve-empty-braces-literal", "resolveConfigValue", "${}", {}),
	cv("resolve-unterminated-brace-literal", "resolveConfigValue", "pre${PI_PARITY_KEY", { PI_PARITY_KEY: "x" }),
	cv("resolve-no-default-operator", "resolveConfigValue", "$PI_PARITY_KEY:-fallback", { PI_PARITY_KEY: "x" }),
	cv("resolve-literal", "resolveConfigValue", "plain-literal-value", {}),
	cv("resolve-command-printf", "resolveConfigValue", "!printf pi-parity-key", {}),
	cv("resolve-command-echo-trims-newline", "resolveConfigValue", "!echo hello", {}),
	cv("resolve-command-empty-stdout-null", "resolveConfigValue", "!true", {}),
	cv("resolve-command-nonzero-exit-null", "resolveConfigValue", "!exit 7", {}),
	cv("is-command-config-value-true", "isCommandConfigValue", "!echo hi", {}),
	cv("is-command-config-value-literal-false", "isCommandConfigValue", "not-a-command", {}),
	cv("is-command-config-value-bare-bang", "isCommandConfigValue", "!", {}),
	cv("is-configured-present", "isConfigValueConfigured", "$PI_PARITY_KEY", { PI_PARITY_KEY: "sk" }),
	cv("is-configured-missing", "isConfigValueConfigured", "$PI_PARITY_KEY", {}),
	cv("is-configured-failing-command", "isConfigValueConfigured", "!exit 7", {}),
	cv("env-var-name-single-ref", "getConfigValueEnvVarName", "$PI_PARITY_KEY", {}),
	cv("env-var-name-mixed-null", "getConfigValueEnvVarName", "prefix-$PI_PARITY_KEY", {}),
	cv("env-var-names-multi-dedup", "getConfigValueEnvVarNames", "$PI_PARITY_A ${PI_PARITY_B} $PI_PARITY_A", {}),
	cv("env-var-names-command-empty", "getConfigValueEnvVarNames", "!printf x", {}),
	cv("missing-env-var-names", "getMissingConfigValueEnvVarNames", "$PI_PARITY_A/$PI_PARITY_B/$PI_PARITY_C", {
		PI_PARITY_B: "set",
	}),
	cv("or-throw-missing-error", "resolveConfigValueOrThrow", "$PI_PARITY_MISSING", {}, { description: "api key" }),
	cv("or-throw-command-error", "resolveConfigValueOrThrow", "!exit 7", {}, { description: "api key" }),
	cv(
		"headers-resolve",
		"resolveHeaders",
		"",
		{ PI_PARITY_KEY: "sk-secret" },
		{
			headers: {
				Authorization: "Bearer $PI_PARITY_KEY",
				"X-Static": "fixed",
				"X-Tool": "!printf tool-value",
			},
		},
	),
	cv(
		"headers-partial-unresolvable",
		"resolveHeaders",
		"",
		{},
		{ headers: { "X-Missing": "$PI_PARITY_MISSING", "X-Static": "fixed" } },
	),
	cv("headers-empty-null", "resolveHeaders", "", {}, { headers: {} }),
	cv(
		"headers-or-throw-success",
		"resolveHeadersOrThrow",
		"",
		{ PI_PARITY_KEY: "sk-secret" },
		{ description: "api key", headers: { Authorization: "Bearer $PI_PARITY_KEY" } },
	),
	cv(
		"headers-or-throw-missing-error",
		"resolveHeadersOrThrow",
		"",
		{},
		{ description: "api key", headers: { Authorization: "Bearer $PI_PARITY_MISSING" } },
	),

	// ------------------------------------------------ http-dispatcher (module)
	// core/http-dispatcher.ts: parse/format of the idle-timeout choices.
	hdp("choice-number", 300000),
	hdp("zero", 0),
	hdp("string-whitespace", " 100 "),
	hdp("string-hex", "0x1E"),
	hdp("string-exponent", "1e3"),
	hdp("string-plus", "+5"),
	hdp("string-disabled", "disabled"),
	hdp("string-invalid-null", "abc"),
	hdp("string-empty-null", ""),
	hdp("null-value-null", null),
	hdp("negative-null", -5),
	hdp("float-floor", 2.9),
	hdp("object-null", {}),
	hdf("non-choice-millis", 1500),
	hdf("non-choice-sub-second", 100),
	hdf("choice-five-min", 300000),
	hdf("choice-disabled", 0),
	hdf("non-choice-odd", 1501),

	// -------------------------------------------------- auth-guidance (module)
	// core/auth-guidance.ts: login help and no-model messages. The harness
	// substitutes the docs path with the literal "<docs>" in the output.
	{
		name: "auth-guidance/provider-login-help",
		tool: "auth-guidance",
		args: { fn: "getProviderLoginHelp", docsPath: "/DOCS" },
	},
	{
		name: "auth-guidance/no-models-available",
		tool: "auth-guidance",
		args: { fn: "formatNoModelsAvailableMessage", docsPath: "/DOCS" },
	},
	{
		name: "auth-guidance/no-model-selected",
		tool: "auth-guidance",
		args: { fn: "formatNoModelSelectedMessage", docsPath: "/DOCS" },
	},
	{
		name: "auth-guidance/no-api-key-found",
		tool: "auth-guidance",
		args: { fn: "formatNoApiKeyFoundMessage", docsPath: "/DOCS", provider: "anthropic" },
	},
	{
		name: "auth-guidance/no-api-key-found-unknown-provider",
		tool: "auth-guidance",
		args: { fn: "formatNoApiKeyFoundMessage", docsPath: "/DOCS", provider: "unknown" },
	},

	// --------------------------------------------------- models-store (module)
	// core/models-store.ts: InMemoryCodingAgentModelsStore and FileModelsStore
	// (pi-ai ModelsStoreEntry: {models, lastModified?, checkedAt?, etag?}).
	// File cases use a unique path per case; the harness namespaces by pid.
	msInMemory("read-empty-null", [{ op: "read", providerId: "anthropic" }]),
	msInMemory("write-read-roundtrip", [
		{ op: "write", providerId: "anthropic", entry: msEntry() },
		{ op: "read", providerId: "anthropic" },
	]),
	msInMemory("overwrite", [
		{ op: "write", providerId: "anthropic", entry: msEntry({ etag: '"v1"' }) },
		{
			op: "write",
			providerId: "anthropic",
			entry: msEntry({ etag: '"v2"', lastModified: 1700000000 }),
		},
		{ op: "read", providerId: "anthropic" },
	]),
	msInMemory("delete", [
		{ op: "write", providerId: "anthropic", entry: msEntry() },
		{ op: "delete", providerId: "anthropic" },
		{ op: "read", providerId: "anthropic" },
	]),
	msInMemory("multi-provider", [
		{ op: "write", providerId: "anthropic", entry: msEntry() },
		{
			op: "write",
			providerId: "openai",
			entry: msEntry({
				models: [msModel({ id: "gpt-x", name: "GPT X", api: "openai", provider: "openai", baseUrl: "https://api.openai.com/v1" })],
			}),
		},
		{ op: "read", providerId: "anthropic" },
		{ op: "read", providerId: "openai" },
		{ op: "read", providerId: "unknown-provider" },
	]),
	msFile("absent-initial-null", "case-a", null, [{ op: "read", providerId: "anthropic" }]),
	msFile("initial-content-read", "case-b", JSON.stringify({ anthropic: msEntry({ etag: '"etag-1"' }) }), [
		{ op: "read", providerId: "anthropic" },
		{ op: "read", providerId: "openai" },
	]),
	msFile("initial-content-bom", "case-c", "﻿" + JSON.stringify({ anthropic: msEntry() }), [
		{ op: "read", providerId: "anthropic" },
	]),
	msFile("invalid-json-error", "case-d", "{", [{ op: "read", providerId: "anthropic" }]),
	// NOTE: non-object JSON roots ("null", "[1,2]") are a documented port
	// divergence — the TS parse is an unchecked cast (property access on
	// null throws "Cannot read properties of null"; on arrays string keys
	// read undefined), while the Rust parse_stored rejects non-objects with
	// "Invalid models-store.json: expected a JSON object". Not parity-
	// tested; see models_store.rs parse_stored.
	msFile("entry-null-value-read-null", "case-e", JSON.stringify({ anthropic: null }), [
		{ op: "read", providerId: "anthropic" },
	]),
	msFile("unrelated-provider-read-null", "case-f", JSON.stringify({ other: msEntry() }), [
		{ op: "read", providerId: "anthropic" },
	]),
	msFile("write-content-pretty", "case-g", null, [
		{ op: "write", providerId: "anthropic", entry: msEntry({ etag: '"v1"' }) },
	]),
	msFile("delete-leaves-empty-object", "case-h", JSON.stringify({ anthropic: msEntry() }), [
		{ op: "delete", providerId: "anthropic" },
	]),

	// --------------------------------------------- runtime-credentials (module)
	// core/runtime-credentials.ts: RuntimeCredentials overlay over a backing
	// CredentialStore (pi-ai Credential: {type:"api_key", key?} | oauth).
	rc("read-unknown-null", [], [{ op: "read", providerId: "anthropic" }]),
	rc("override-shadows-backing", [rcKey("anthropic", "backing-key")], [
		{ op: "set", providerId: "anthropic", key: "runtime-key" },
		{ op: "read", providerId: "anthropic" },
		{ op: "has", providerId: "anthropic" },
	]),
	rc("remove-restores-backing", [rcKey("anthropic", "backing-key")], [
		{ op: "set", providerId: "anthropic", key: "runtime-key" },
		{ op: "remove", providerId: "anthropic" },
		{ op: "read", providerId: "anthropic" },
		{ op: "has", providerId: "anthropic" },
	]),
	rc("has-false-without-override", [rcKey("anthropic", "backing-key")], [
		{ op: "has", providerId: "anthropic" },
	]),
	rc("set-then-remove-has", [], [
		{ op: "set", providerId: "anthropic", key: "k" },
		{ op: "has", providerId: "anthropic" },
		{ op: "remove", providerId: "anthropic" },
		{ op: "has", providerId: "anthropic" },
	]),
	rc(
		"list-backing-order-override-merge",
		[rcKey("zeta", "z"), rcKey("alpha", "a"), rcKey("mid", "m")],
		[
			{ op: "set", providerId: "alpha", key: "a2" },
			{ op: "set", providerId: "beta", key: "b" },
			{ op: "list" },
		],
	),
	rc("list-empty", [], [{ op: "list" }]),
	rc("list-append-insertion-order", [rcKey("first", "1")], [
		{ op: "set", providerId: "zz", key: "z" },
		{ op: "set", providerId: "aa", key: "a" },
		{ op: "list" },
	]),
	rc("delete-removes-override-and-backing", [rcKey("anthropic", "backing-key")], [
		{ op: "set", providerId: "anthropic", key: "runtime-key" },
		{ op: "delete", providerId: "anthropic" },
		{ op: "read", providerId: "anthropic" },
		{ op: "has", providerId: "anthropic" },
		{ op: "list" },
	]),
	rc("modify-set-new-credential", [rcKey("anthropic", "old-key")], [
		{
			op: "modify",
			providerId: "anthropic",
			set: { type: "oauth", refresh: "r1", access: "a1", expires: 1700000000 },
		},
	]),
	rc("modify-no-set-unchanged", [rcKey("anthropic", "current-key")], [
		{ op: "modify", providerId: "anthropic" },
	]),
	rc("modify-unknown-null", [], [{ op: "modify", providerId: "anthropic" }]),
	rc(
		"read-oauth-backing",
		[{ providerId: "github", credential: { type: "oauth", refresh: "r1", access: "a1", expires: 1700000000 } }],
		[
			{ op: "read", providerId: "github" },
			{ op: "list" },
		],
	),

	// ------------------------------------------------- virtual-models (module)
	// core/virtual-models.ts: catalog entries that route to physical models.
	vmCase("is-virtual-model-true", {
		fn: "isVirtualModel",
		model: vmModel("vm-1", "anthropic", "pi-virtual"),
	}),
	vmCase("is-virtual-model-false", {
		fn: "isVirtualModel",
		model: vmModel("claude-sonnet-4", "anthropic", "anthropic"),
	}),
	vmCase("find-latest-no-assistant-null", {
		fn: "findLatestResponse",
		messages: [{ role: "user", content: "hi", timestamp: 1780000000000 }],
	}),
	vmCase("find-latest-all-error-null", {
		fn: "findLatestResponse",
		messages: [vmAssistant({ stopReason: "error" }), vmAssistant({ stopReason: "aborted", model: "m2" })],
	}),
	vmCase("find-latest-error-after-good", {
		fn: "findLatestResponse",
		messages: [vmAssistant({ model: "good-model" }), vmAssistant({ stopReason: "error", model: "bad-model" })],
	}),
	vmCase("find-latest-latest-good-wins", {
		fn: "findLatestResponse",
		messages: [vmAssistant({ model: "first-model" }), vmAssistant({ model: "second-model" })],
	}),
	vmCase("branch-model-change", {
		fn: "getBranchSelection",
		branch: [vmModelChange(1, "anthropic", "claude-sonnet-4")],
		models: [],
	}),
	vmCase("branch-model-change-wins-over-response", {
		fn: "getBranchSelection",
		branch: [vmMsgEntry(1, vmAssistant()), vmModelChange(2, "openai", "gpt-x")],
		models: [],
	}),
	vmCase("branch-physical-response-only", {
		fn: "getBranchSelection",
		branch: [vmMsgEntry(1, vmAssistant({ provider: "anthropic", model: "claude-sonnet-4" }))],
		models: [],
	}),
	vmCase("branch-virtual-change-holds", {
		fn: "getBranchSelection",
		branch: [vmModelChange(1, "anthropic", "vm-1"), vmMsgEntry(2, vmAssistant({ model: "claude-sonnet-4" }))],
		models: [vmModel("vm-1", "anthropic", "pi-virtual")],
	}),
	vmCase("branch-virtual-change-unregistered-fallback", {
		fn: "getBranchSelection",
		branch: [vmModelChange(1, "anthropic", "vm-1"), vmMsgEntry(2, vmAssistant({ model: "claude-sonnet-4" }))],
		models: [],
	}),
	vmCase("branch-last-model-change-wins", {
		fn: "getBranchSelection",
		branch: [
			vmModelChange(1, "anthropic", "vm-1"),
			vmMsgEntry(2, vmAssistant()),
			vmModelChange(3, "openai", "gpt-x"),
		],
		models: [vmModel("vm-1", "anthropic", "pi-virtual")],
	}),
	vmCase("branch-no-selection-null", {
		fn: "getBranchSelection",
		branch: [vmMsgEntry(1, { role: "user", content: "hi", timestamp: 1780000000000 })],
		models: [],
	}),
	vmCase("state-found", {
		fn: "getVirtualModelState",
		branch: [vmStateEntry(1, "anthropic", "vm-1", { turn: 3 })],
		provider: "anthropic",
		modelId: "vm-1",
	}),
	vmCase("state-missing-null", {
		fn: "getVirtualModelState",
		branch: [vmStateEntry(1, "anthropic", "vm-1", { turn: 3 })],
		provider: "openai",
		modelId: "vm-9",
	}),
	vmCase("state-latest-wins", {
		fn: "getVirtualModelState",
		branch: [vmStateEntry(1, "anthropic", "vm-1", { turn: 1 }), vmStateEntry(2, "anthropic", "vm-1", { turn: 2 })],
		provider: "anthropic",
		modelId: "vm-1",
	}),
	vmCase("create-defaults", {
		fn: "createVirtualModel",
		definition: { provider: "anthropic", id: "vm-1", name: "Router One" },
	}),
	vmCase("create-thinking-levels", {
		fn: "createVirtualModel",
		definition: { provider: "openai", id: "vm-2", name: "Router Two", thinkingLevels: ["low", "high"] },
	}),
	vmCase("create-full", {
		fn: "createVirtualModel",
		definition: {
			provider: "anthropic",
			id: "vm-3",
			name: "Router Three",
			thinkingLevels: ["off", "medium"],
			input: ["text"],
			contextWindow: 128000,
			maxTokens: 8192,
		},
	}),
	// ------------------------------------------ provider-attribution (module)
	// core/provider-attribution.ts: mergeProviderAttributionHeaders. Only
	// provider+baseUrl of the model matter; telemetry gates the default
	// attribution headers; session headers apply to opencode providers/hosts.
	paCase("telemetry-off-no-headers", paModel(), false),
	paCase("telemetry-on-anthropic-null", paModel(), true),
	paCase("openrouter-provider", paModel({ provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1" }), true),
	paCase("openrouter-provider-telemetry-off", paModel({ provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1" }), false),
	paCase("openrouter-baseurl-substring", paModel({ provider: "myprov", baseUrl: "https://proxy.example/openrouter.ai/v1" }), true),
	paCase("openrouter-baseurl-substring-lookalike-host", paModel({ provider: "myprov", baseUrl: "https://openrouter.ai.example.com/v1" }), true),
	paCase("openrouter-baseurl-mixed-case-no-substring", paModel({ provider: "myprov", baseUrl: "https://OpenRouter.AI/v1" }), true),
	paCase("nvidia-provider", paModel({ provider: "nvidia", baseUrl: "https://integrate.api.nvidia.com/v1" }), true),
	paCase("nvidia-host", paModel({ provider: "myprov", baseUrl: "https://integrate.api.nvidia.com/v1" }), true),
	paCase("nvidia-host-mixed-case", paModel({ provider: "myprov", baseUrl: "https://Integrate.API.NVIDIA.COM/v1" }), true),
	paCase("nvidia-substring-not-hostname", paModel({ provider: "myprov", baseUrl: "https://evil.example/integrate.api.nvidia.com" }), true),
	paCase("cloudflare-workers-ai-provider", paModel({ provider: "cloudflare-workers-ai", baseUrl: "https://api.cloudflare.com/client/v4" }), true),
	paCase("cloudflare-ai-gateway-provider", paModel({ provider: "cloudflare-ai-gateway", baseUrl: "https://gateway.ai.cloudflare.com/v1" }), true),
	paCase("cloudflare-api-host", paModel({ provider: "myprov", baseUrl: "https://api.cloudflare.com/client/v4/accounts/abc" }), true),
	paCase("cloudflare-gateway-host", paModel({ provider: "myprov", baseUrl: "https://gateway.ai.cloudflare.com/v1/abc/anthropic" }), true),
	paCase("opencode-provider-session", paModel({ provider: "opencode", baseUrl: "https://opencode.ai/api" }), false, { sessionId: "s-1" }),
	paCase("opencode-provider-session-telemetry-on", paModel({ provider: "opencode", baseUrl: "https://opencode.ai/api" }), true, { sessionId: "s-1" }),
	paCase("opencode-go-provider-session", paModel({ provider: "opencode-go" }), false, { sessionId: "s-1" }),
	paCase("opencode-host-session", paModel({ provider: "myprov", baseUrl: "https://opencode.ai/api" }), false, { sessionId: "s-1" }),
	paCase("opencode-session-id-undefined", paModel({ provider: "opencode" }), true),
	paCase("session-id-non-opencode-ignored", paModel(), false, { sessionId: "s-1" }),
	paCase("opencode-session-plus-nvidia-host", paModel({ provider: "opencode", baseUrl: "https://integrate.api.nvidia.com/v1" }), true, { sessionId: "s-1" }),
	paCase("garbage-url-nvidia-provider", paModel({ provider: "nvidia", baseUrl: "not a url" }), true),
	paCase("garbage-url-cloudflare-provider", paModel({ provider: "cloudflare-ai-gateway", baseUrl: "not a url" }), true),
	paCase("garbage-url-opencode-provider-session", paModel({ provider: "opencode", baseUrl: "not a url" }), false, { sessionId: "s-1" }),
	paCase("garbage-url-plain-null", paModel({ provider: "myprov", baseUrl: "not a url" }), true),
	paCase("header-sources-merge", paModel(), true, { headerSources: [{ "x-a": "1" }, null] }),
	paCase("header-sources-override-default", paModel({ provider: "openrouter", baseUrl: "https://openrouter.ai/api/v1" }), true, { headerSources: [{ "X-OpenRouter-Title": "custom" }] }),
	paCase("header-sources-later-wins", paModel(), false, { headerSources: [{ a: "1" }, { a: "2", b: "3" }] }),
	paCase("header-sources-only-telemetry-off", paModel(), false, { headerSources: [{ "x-only": "src" }] }),
	paCase("empty-merge-null", paModel(), false),

	// -------------------------------------------------- model-config (module)
	// core/model-config.ts: ModelConfig.load. `path` is a label passed through
	// verbatim into error strings; `content` is the raw file text (comments,
	// trailing commas and BOM are handled by the loader); `ioError` simulates
	// a non-ENOENT read failure. Expected error strings follow the typebox
	// ground truth captured in the wave-2 spec.
	mcCase("path-null", null, null, null),
	mcCase("enoent-empty", "/models.json", null, null),
	mcCase("io-error", "/models.json", null, "EACCES: permission denied, open '/models.json'"),
	mcCase("io-error-path-label-verbatim", "/deep/dir/models.json", null, "EISDIR: illegal operation on a directory, read"),
	mcCase("parse-not-json", "/m.json", "not json", null),
	mcCase("parse-open-brace", "/m.json", "{", null),
	mcCase("parse-trailing-garbage", "/m.json", "{} garbage", null),
	mcCase("parse-block-comment-not-stripped", "/m.json", '{"providers": /* NOT stripped - only // per the regex */ {}}', null),
	mcCase("strip-line-comment", "/m.json", '{\n  // top-level comment\n  "providers": {} // trailing\n}', null),
	mcCase("strip-trailing-comma", "/m.json", '{"providers": {},}', null),
	mcCase("strip-trailing-comma-in-array", "/m.json", '{"providers":{"a":{"models":[{"id":"m",},],},},}', null),
	mcCase("comma-inside-string", "/m.json", '{"providers":{"a":{"name":"x, y"}}}', null),
	mcCase("double-slash-inside-string", "/m.json", '{"providers":{"a":{"name":"http://x"}}}', null),
	mcCase("bom", "/m.json", "\uFEFF" + '{"providers":{}}', null),
	mcCase("bom-plus-comment", "/m.json", "\uFEFF" + '{ // c\n"providers": {"a": {}},}', null),
	mcCase("valid-empty-providers", "/m.json", '{"providers":{}}', null),
	mcCase("valid-one-provider", "/m.json", '{"providers":{"anthropic":{"name":"Anthropic","baseUrl":"https://api.anthropic.com","apiKey":"sk-test"}}}', null),
	mcCase("valid-provider-order", "/m.json", '{"providers":{"b":{},"a":{},"c":{}}}', null),
	mcCase("valid-model-full", "/m.json", '{"providers":{"p":{"models":[{"id":"full","name":"Full","api":"anthropic","baseUrl":"https://x.example","reasoning":true,"thinkingLevelMap":{"high":"max","low":null},"input":["text","image"],"inputLimits":{"maxRequestBytes":1000,"images":{"resize":{"maxWidth":1024,"maxHeight":768,"maxBytes":100000,"jpegQuality":80},"maxPerMessage":5,"maxPerRequest":20}},"cost":{"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75,"tiers":[{"inputTokensAbove":0,"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75},{"inputTokensAbove":1000,"input":1.5,"output":7.5,"cacheRead":0.15,"cacheWrite":1.875}]},"promptCache":{"short":0.5,"long":1},"contextWindow":200000,"maxTokens":64000,"samplingParams":{"temperature":0.6,"any":{"nested":[1,2]},"nul":null},"samplingParamsByThinkingLevel":{"high":{"temperature":1},"off":{}},"headers":{"x-custom":"v"},"compat":{"maxTokensField":"max_completion_tokens","thinkingFormat":"openai","thinkingTokenBudgetField":"thinking_budget","cacheControlFormat":"anthropic","sessionAffinityFormat":"openai","supportsTemperature":true,"vllmPriority":3}}]}}}', null),
	mcCase("valid-model-minimal", "/m.json", '{"providers":{"a":{"models":[{"id":"m"}]}}}', null),
	mcCase("valid-model-overrides", "/m.json", '{"providers":{"p":{"modelOverrides":{"m":{"name":"Renamed","reasoning":false,"contextWindow":1000,"cost":{"input":1}}}}}}', null),
	mcCase("valid-schema-key", "/m.json", '{"$schema":"https://example.com/schema.json","providers":{}}', null),
	mcCase("valid-unicode-strings", "/m.json", '{"providers":{"ünïcode":{"name":"Ünïcode ✓","models":[{"id":"mødel","name":"名前"}]}}}', null),
	mcCase("valid-oauth-radius", "/m.json", '{"providers":{"r":{"oauth":"radius","name":"Radius"}}}', null),
	mcCase("valid-cost-zero-negative", "/m.json", '{"providers":{"a":{"models":[{"id":"m","cost":{"input":0,"output":-1,"cacheRead":0,"cacheWrite":0}}]}}}', null),
	mcCase("valid-sampling-params-arbitrary", "/m.json", '{"providers":{"a":{"models":[{"id":"m","samplingParams":{"temperature":0.7,"obj":{"nested":[1,2]},"nul":null,"str":"s","arr":[{"x":1}]}}]}}}', null),
	mcCase("valid-sampling-params-by-thinking-level", "/m.json", '{"providers":{"a":{"models":[{"id":"m","samplingParamsByThinkingLevel":{"high":{"temperature":1},"off":{},"max":{"x":null}}}]}}}', null),
	mcCase("valid-compat-chat-template-kwargs", "/m.json", '{"providers":{"p":{"compat":{"chatTemplateKwargs":{"budget":{"$var":"thinking.budget","omitWhenOff":true},"enabled":"thinking.enabled"}}}}}', null),
	mcCase("valid-compat-openrouter-routing", "/m.json", '{"providers":{"p":{"compat":{"openRouterRouting":{"allow_fallbacks":false,"data_collection":"deny","only":["a","b"],"sort":{"by":"throughput","partition":null},"max_price":{"prompt":0.5,"completion":"auto"}}}}}}', null),
	mcCase("valid-compat-allowed-fallback-models", "/m.json", '{"providers":{"p":{"compat":{"allowedFallbackModels":[{"provider":"openai","model":"gpt-5.5","cost":{"input":1,"output":2,"cacheRead":0.5,"cacheWrite":1}}]}}}}', null),
	mcCase("error-missing-providers", "/m.json", "{}", null),
	mcCase("error-root-not-object", "/m.json", "[]", null),
	mcCase("error-providers-array", "/m.json", '{"providers":[]}', null),
	mcCase("error-provider-value-number", "/m.json", '{"providers":{"a":1}}', null),
	mcCase("error-model-missing-id", "/m.json", '{"providers":{"a":{"models":[{}]}}}', null),
	mcCase("error-model-empty-id", "/m.json", '{"providers":{"a":{"models":[{"id":""}]}}}', null),
	mcCase("error-model-id-number", "/m.json", '{"providers":{"a":{"models":[{"id":5}]}}}', null),
	mcCase("error-context-window-zero", "/m.json", '{"providers":{"a":{"models":[{"id":"m","contextWindow":0}]}}}', null),
	mcCase("error-max-tokens-negative", "/m.json", '{"providers":{"a":{"models":[{"id":"m","maxTokens":-1}]}}}', null),
	mcCase("error-context-window-string", "/m.json", '{"providers":{"a":{"models":[{"id":"m","contextWindow":"1000"}]}}}', null),
	mcCase("error-reasoning-string", "/m.json", '{"providers":{"a":{"models":[{"id":"m","reasoning":"yes"}]}}}', null),
	mcCase("error-oauth-wrong-literal", "/m.json", '{"providers":{"a":{"oauth":"github"}}}', null),
	mcCase("error-input-bogus-modality", "/m.json", '{"providers":{"a":{"models":[{"id":"m","input":["text","audio"]}]}}}', null),
	mcCase("error-thinking-level-map-number", "/m.json", '{"providers":{"a":{"models":[{"id":"m","thinkingLevelMap":{"high":5}}]}}}', null),
	mcCase("error-headers-non-string", "/m.json", '{"providers":{"a":{"headers":{"x":1}}}}', null),
	mcCase("error-cost-missing-required", "/m.json", '{"providers":{"a":{"models":[{"id":"m","cost":{"input":1}}]}}}', null),
	mcCase("error-cost-tier-missing-required", "/m.json", '{"providers":{"a":{"models":[{"id":"m","cost":{"input":1,"output":1,"cacheRead":1,"cacheWrite":1,"tiers":[{"input":2}]}}]}}}', null),
	mcCase("error-prompt-cache-zero", "/m.json", '{"providers":{"a":{"models":[{"id":"m","promptCache":{"short":0}}]}}}', null),
	mcCase("error-max-request-bytes-zero", "/m.json", '{"providers":{"a":{"models":[{"id":"m","inputLimits":{"maxRequestBytes":0}}]}}}', null),
	mcCase("error-max-request-bytes-string", "/m.json", '{"providers":{"a":{"models":[{"id":"m","inputLimits":{"maxRequestBytes":"x"}}]}}}', null),
	mcCase("error-jpeg-quality-101", "/m.json", '{"providers":{"a":{"models":[{"id":"m","inputLimits":{"images":{"resize":{"jpegQuality":101}}}}]}}}', null),
	mcCase("error-compat-max-tokens-field", "/m.json", '{"providers":{"a":{"compat":{"maxTokensField":"bogus"}}}}', null),
	mcCase("error-two-bad-props-reverse-instance-order", "/m.json", '{"providers":{"a":{"models":[{"id":"m","contextWindow":0,"reasoning":"x"}]}}}', null),
	mcCase("error-two-providers-insertion-order", "/m.json", '{"providers":{"z":{"models":[{"id":"m","contextWindow":0}]},"a":{"models":[{"id":"m","maxTokens":-1}]}}}', null),

	// ------------------------------------------------- model-resolver (module)
	// core/model-resolver.ts: reference/scope/CLI/initial/session model
	// resolution over a synthetic catalog (mrCatalog at the bottom of this
	// file). All inputs are literal; `usable`/`configuredAuth` stand in for
	// credential state.
	mrRef("exact-bare-id", "gpt-5.5"),
	mrRef("exact-provider-slash-id", "anthropic/claude-opus-4-8"),
	mrRef("provider-slash-case-insensitive", "ANTHROPIC/claude-opus-4-8"),
	mrRef("ambiguous-same-id-two-providers", "claude-sonnet-4-5", { usable: [] }),
	mrRef("ambiguous-usable-one", "claude-sonnet-4-5", { usable: ["openai"] }),
	mrRef("ambiguous-usable-both", "claude-sonnet-4-5", { usable: ["anthropic", "openai"] }),
	mrRef("gateway-id-provider-prefix", "gateway/zai/glm-5"),
	mrRef("gateway-id-bare", "zai/glm-5"),
	mrRef("tie-named-usable-wins", "myprov/aab", { usable: ["myprov"] }),
	mrRef("tie-sole-usable-raw-wins", "myprov/aab", { usable: ["other"] }),
	mrRef("tie-unusable-named-preferred", "myprov/aab", { usable: [] }),
	mrRef("prefix-target-gateway", "gateway/claude-sonnet-4-5", { usable: [] }),
	mrRef("thinking-none-suffix-stays", "gpt-5.5:high", { thinkingSuffix: "none" }),
	mrRef("thinking-strict-high", "gpt-5.5:high", { thinkingSuffix: "strict" }),
	mrRef("thinking-strict-bogus", "gpt-5.5:bogus", { thinkingSuffix: "strict" }),
	mrRef("thinking-lenient-bogus-warning", "gpt-5.5:bogus", { thinkingSuffix: "lenient" }),
	mrRef("colon-id-strict-suffix-part-of-id", "qwen/qwen3-coder:exacto", { thinkingSuffix: "strict" }),
	mrRef("colon-id-plus-thinking", "qwen/qwen3-coder:exacto:high", { thinkingSuffix: "strict" }),
	mrRef("fuzzy-substring-id", "glm", { fuzzy: true, usable: ["anthropic", "openai", "openrouter", "gateway", "myprov", "other", "moonshotai"] }),
	mrRef("fuzzy-substring-name", "zenith", { fuzzy: true, usable: ["anthropic", "openai", "openrouter", "gateway", "myprov", "other", "moonshotai"] }),
	mrRef("fuzzy-off-name-miss", "zenith", { fuzzy: false }),
	mrRef("fuzzy-sonnet-ambiguous", "sonnet", { fuzzy: true, usable: [] }),
	mrRef("fuzzy-sonnet-usable-one-alias-wins", "sonnet", { fuzzy: true, usable: ["anthropic"] }),
	mrRef("fuzzy-kimi-best-match-sort", "kimi", { fuzzy: true, usable: [] }),
	mrRef("fuzzy-dated-substring", "claude-sonnet-4-5-2025092", { fuzzy: true, usable: [] }),
	mrRef("provider-constraint", "claude-sonnet-4-5", { provider: "openai" }),
	mrRef("provider-constraint-case-insensitive", "claude-opus-4-8", { provider: "Anthropic" }),
	mrRef("provider-constraint-not-found-target", "nope-model", { provider: "anthropic" }),
	mrRef("not-found-provider-prefix", "anthropic/nope-model"),
	mrRef("not-found-prefix-with-thinking", "anthropic/nope-model:high"),
	mrRef("not-found-unknown-prefix", "unknownprov/x"),
	mrRef("empty-text", ""),
	mrRef("text-trimmed", "  gpt-5.5  "),
	mrExact("hit-bare-id", "gpt-5.5"),
	mrExact("miss-fuzzy-only", "glm"),
	mrExact("hit-provider-slash", "anthropic/claude-opus-4-8"),
	mrExact("ambiguous-null", "claude-sonnet-4-5"),
	mrExact("hit-gateway-id", "gateway/zai/glm-5"),
	mrExact("thinking-suffix-null", "gpt-5.5:high"),
	mrScope("exact-bare-id", ["gpt-5.5"]),
	mrScope("exact-provider-id", ["anthropic/claude-opus-4-8"]),
	mrScope("exact-thinking-high", ["gpt-5.5:high"]),
	mrScope("invalid-suffix-warning", ["gpt-5.5:bogus"]),
	mrScope("provider-id-invalid-suffix", ["anthropic/claude-opus-4-8:bogus"]),
	mrScope("ambiguous-bare-id", ["claude-sonnet-4-5"]),
	mrScope("thinking-suffix-ambiguous", ["claude-sonnet-4-5:high"]),
	mrScope("glob-star-sonnet-bare-id-only", ["*sonnet*"]),
	mrScope("glob-provider-star", ["anthropic/*"]),
	mrScope("glob-brace-providers", ["{anthropic,openai}/*"]),
	mrScope("glob-doublestar-kimi", ["**/kimi*"]),
	mrScope("glob-question-single-char", ["myprov/a?b"]),
	mrScope("glob-bracketed-exact-id-first", ["bracketed-model[1m]"]),
	mrScope("glob-no-match", ["*nomatch*"]),
	mrScope("no-match-non-glob", ["does-not-exist"]),
	mrScope("duplicate-adds-deduped", ["gpt-5.5", "openai/gpt-5.5", "*gpt*"]),
	mrScope("multiple-patterns-accumulate", ["anthropic/claude-opus-4-8", "myprov/aab", "nope"]),
	mrScope("empty-patterns", []),
	mrCli("no-model", {}),
	mrCli("empty-models-error", { cliModel: "x", models: [] }),
	mrCli("unknown-provider", { cliProvider: "nope", cliModel: "x" }),
	mrCli("resolved-bare", { cliModel: "gpt-5.5" }),
	mrCli("resolved-provider-flag", { cliProvider: "anthropic", cliModel: "claude-opus-4-8" }),
	mrCli("resolved-provider-case-insensitive", { cliProvider: "OpenAI", cliModel: "gpt-5.5" }),
	mrCli("resolved-with-thinking", { cliModel: "gpt-5.5:high" }),
	mrCli("resolved-gateway-id", { cliModel: "gateway/zai/glm-5" }),
	mrCli("ambiguous-no-auth", { cliModel: "claude-sonnet-4-5", configuredAuth: [] }),
	mrCli("ambiguous-both-auth", { cliModel: "claude-sonnet-4-5", configuredAuth: ["anthropic", "openai"] }),
	mrCli("ambiguous-inline-key", { cliModel: "claude-sonnet-4-5", inlineApiKey: true }),
	mrCli("strict-bogus-suffix-not-found", { cliModel: "gpt-5.5:bogus" }),
	mrCli("fallback-custom-model-default-present", { cliProvider: "anthropic", cliModel: "claude-future-9" }),
	mrCli("fallback-custom-model-thinking", { cliModel: "anthropic/claude-future-9:high" }),
	mrCli("fallback-no-default-first-model", { cliProvider: "myprov", cliModel: "brand-new-model" }),
	mrCli("fallback-cli-thinking-suppresses-suffix", { cliProvider: "anthropic", cliModel: "claude-future-9:high", cliThinking: "low" }),
	mrInitial("scoped-wins", { scopedModels: [mrScoped(mrFind("gpt-5.5", "openai"), "high")] }),
	mrInitial("scoped-per-model-thinking", { scopedModels: [mrScoped(mrFind("gpt-5.5", "openai"))], modelThinkingLevels: { "openai/gpt-5.5": "low" } }),
	mrInitial("scoped-default-thinking", { scopedModels: [mrScoped(mrFind("claude-opus-4-8", "anthropic"))], defaultThinkingLevel: "max" }),
	mrInitial("continuing-skips-scoped", {
		scopedModels: [mrScoped(mrFind("gpt-5.5", "openai"), "high")],
		isContinuing: true,
		defaultProvider: "anthropic",
		defaultModelId: "claude-opus-4-8",
		defaultThinkingLevel: "low",
		configuredAuth: ["anthropic"],
	}),
	mrInitial("saved-default-with-auth", {
		defaultProvider: "anthropic",
		defaultModelId: "claude-opus-4-8",
		configuredAuth: ["anthropic"],
	}),
	mrInitial("saved-default-per-model-thinking", {
		defaultProvider: "anthropic",
		defaultModelId: "claude-opus-4-8",
		modelThinkingLevels: { "anthropic/claude-opus-4-8": "xhigh" },
		configuredAuth: ["anthropic"],
	}),
	mrInitial("saved-default-no-auth-falls-through", {
		defaultProvider: "anthropic",
		defaultModelId: "claude-opus-4-8",
		configuredAuth: [],
		availableModels: [mrFind("gpt-5.5", "openai")],
	}),
	mrInitial("available-default-model-match", {
		availableModels: [mrFind("gpt-5.5", "openai"), mrFind("claude-opus-4-8", "anthropic")],
	}),
	mrInitial("available-first-fallback", {
		availableModels: [mrFind("aab", "myprov"), mrFind("abc", "myprov")],
	}),
	mrInitial("no-models", {}),
	mrRestore("ok", { savedProvider: "anthropic", savedModelId: "claude-opus-4-8", configuredAuth: ["anthropic"] }),
	mrRestore("model-no-longer-exists", {
		savedProvider: "anthropic",
		savedModelId: "gone-model",
		availableModels: [mrFind("gpt-5.5", "openai")],
	}),
	mrRestore("no-auth-configured", {
		savedProvider: "anthropic",
		savedModelId: "claude-opus-4-8",
		configuredAuth: [],
		availableModels: [mrFind("aab", "myprov")],
	}),
	mrRestore("current-model-fallback", {
		savedProvider: "anthropic",
		savedModelId: "gone-model",
		currentModel: mrFind("abc", "myprov"),
		availableModels: [mrFind("gpt-5.5", "openai")],
	}),
	mrRestore("available-default-model-fallback", {
		savedProvider: "anthropic",
		savedModelId: "gone-model",
		availableModels: [mrFind("aab", "myprov"), mrFind("claude-opus-4-8", "anthropic")],
	}),

	...(function () {
		const __cases = [];
		// =====================================================================
		// auth-storage
		// =====================================================================

		const asCase = (name, fn, extra = {}) => ({ tool: "auth-storage", name: "as-" + name, args: { fn, ...extra } });
		const asContent = (name, content, fn = "readOnlyLoad") =>
			asCase(name, fn, { content: content === null ? undefined : content });
		const oauthCred = { type: "oauth", access: "a", refresh: "r", expires: 1234567890 };

		__cases.push(asCase("load-valid", "readOnlyLoad", { content: JSON.stringify({ p1: { type: "api_key", key: "k" }, p2: oauthCred }) }));
		__cases.push(asCase("load-missing-file", "readOnlyLoad", {}));
		__cases.push(asContent("load-invalid-json", "{\"a\":}"));
		__cases.push(asContent("load-array", "[1,2]"));
		__cases.push(asContent("load-string", "\"hello\""));
		__cases.push(asContent("load-null", "null"));
		__cases.push(asContent("load-bad-credential", JSON.stringify({ p: { type: "nope" } })));
		__cases.push(asContent("load-bad-api-key-env", JSON.stringify({ p: { type: "api_key", key: "k", env: { a: 1 } } })));
		__cases.push(asContent("load-oauth-missing-refresh", JSON.stringify({ p: { type: "oauth", access: "a", expires: 1 } })));
		__cases.push(asContent("load-okey-api-key-no-key", JSON.stringify({ p: { type: "api_key" } })));
		__cases.push(asCase("read-plain-key", "readOnlyRead", { content: JSON.stringify({ p: { type: "api_key", key: "k" } }), providerId: "p" }));
		__cases.push(asCase("read-missing-provider", "readOnlyRead", { content: JSON.stringify({ p: { type: "api_key", key: "k" } }), providerId: "q" }));
		__cases.push(asCase("read-missing-file", "readOnlyRead", { providerId: "p" }));
		__cases.push(asCase("read-command-key", "readOnlyRead", { content: JSON.stringify({ p: { type: "api_key", key: "!echo hi" } }), providerId: "p" }));
		__cases.push(asCase("read-env-key-cred-env", "readOnlyRead", { content: JSON.stringify({ p: { type: "api_key", key: "$MY_KEY", env: { MY_KEY: "from-cred" } } }), providerId: "p" }));
		__cases.push(asCase("read-env-key-ambient", "readOnlyRead", { content: JSON.stringify({ p: { type: "api_key", key: "$MY_KEY" } }), providerId: "p", env: { MY_KEY: "ambient" } }));
		__cases.push(asCase("read-env-key-missing", "readOnlyRead", { content: JSON.stringify({ p: { type: "api_key", key: "$MY_KEY" } }), providerId: "p" }));
		__cases.push(asCase("read-oauth", "readOnlyRead", { content: JSON.stringify({ p: oauthCred }), providerId: "p" }));
		__cases.push(asCase("list-order", "readOnlyList", { content: JSON.stringify({ zz: { type: "api_key", key: "k" }, a: oauthCred, m: { type: "api_key" } }) }));
		__cases.push(asCase("list-empty-file", "readOnlyList", { content: "{}" }));
		__cases.push(asCase("list-missing", "readOnlyList", {}));
		__cases.push(asCase("modify-read-only", "readOnlyModify", { content: "{}", providerId: "p" }));
		__cases.push(asCase("delete-read-only", "readOnlyDelete", { content: "{}", providerId: "p" }));
		asCase("backend-initial-and-locks", "inMemoryBackend", {
			initial: JSON.stringify({ a: 1 }),
			locks: [{ result: "x" }, { result: "y", next: "{\"b\":2}" }, { result: null }],
		});
		__cases.push(asCase("backend-no-initial", "inMemoryBackend", { locks: [{ result: 1 }, { result: 2, next: "{\"a\":true}" }] }));
		__cases.push(asCase("backend-empty-locks", "inMemoryBackend", { locks: [] }));
		asCase("seq-read-modify-delete", "authStorageSequence", {
			data: { p: { type: "api_key", key: "k" } },
			steps: [
				{ op: "read", providerId: "p" },
				{ op: "modify", providerId: "q", set: { type: "api_key", key: "k2" } },
				{ op: "list" },
				{ op: "delete", providerId: "p" },
				{ op: "read", providerId: "p" },
				{ op: "list" },
			],
		});
		asCase("seq-modify-undefined", "authStorageSequence", {
			data: { p: { type: "api_key", key: "k" } },
			steps: [{ op: "modify", providerId: "p" }, { op: "read", providerId: "p" }],
		});
		asCase("seq-modify-null-set", "authStorageSequence", {
			data: { p: { type: "api_key", key: "k" } },
			steps: [{ op: "modify", providerId: "p", set: null }, { op: "read", providerId: "p" }],
		});
		asCase("seq-merge-order", "authStorageSequence", {
			data: { a: { type: "api_key", key: "1" }, zz: { type: "api_key", key: "2" } },
			steps: [
				{ op: "modify", providerId: "m", set: { type: "api_key", key: "3" } },
				{ op: "delete", providerId: "a" },
				{ op: "list" },
			],
		});
		asCase("seq-reload-and-read", "authStorageSequence", {
			data: { p: { type: "api_key", key: "$K", env: { K: "v" } } },
			steps: [
				{ op: "read", providerId: "p" },
				{ op: "reload" },
				{ op: "read", providerId: "p" },
				{ op: "read", providerId: "nope" },
			],
		});
		__cases.push(asCase("seq-empty", "authStorageSequence", { steps: [] }));
		__cases.push(asCase("stored-credential-present", "readStoredCredential", { providerId: "p", content: JSON.stringify({ p: oauthCred }) }));
		__cases.push(asCase("stored-credential-missing", "readStoredCredential", { providerId: "p" }));
		__cases.push(asCase("stored-credential-invalid-json", "readStoredCredential", { providerId: "p", content: "{" }));

		// =====================================================================
		// remote-catalog
		// =====================================================================

		const rcModel = (id, provider = "prov", extra = {}) => ({
			id,
			name: id,
			api: "openai-completions",
			provider,
			baseUrl: "https://api.example",
			input: ["text"],
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			reasoning: false,
			contextWindow: 1000,
			maxTokens: 100,
			...extra,
		});
		const rcCase = (name, fn, extra = {}) => ({ tool: "remote-catalog", name: "rc-" + name, args: { fn, providerId: "prov", models: [rcModel("b1"), rcModel("b2")], now: 1000000000000, ...extra } });
		const rc200 = (body) => ({ status: 200, body });
		const rcStored = (models, extra = {}) => ({ models, checkedAt: 999999000000, lastModified: 1000000000000, etag: "W/e1", ...extra });

		__cases.push(rcCase("fresh-200", "withRemoteCatalog", { response: rc200([rcModel("r1"), rcModel("b1", "prov", { name: "remote b1" })]) }));
		__cases.push(rcCase("fresh-200-image-models", "withRemoteCatalog", { allModels: [rcModel("b1"), { ...rcModel("im1"), type: "image", output: ["image"] }], response: rc200([{ ...rcModel("r1"), type: "image", output: ["image"] }]) }));
		__cases.push(rcCase("fresh-200-other-provider", "withRemoteCatalog", { response: rc200([rcModel("x1", "other")]) }));
		__cases.push(rcCase("fresh-200-invalid-body", "withRemoteCatalog", { response: rc200(42) }));
		__cases.push(rcCase("fresh-200-body-object-shape", "withRemoteCatalog", { response: rc200({ models: [rcModel("r1")] }) }));
		__cases.push(rcCase("no-response-scripted", "withRemoteCatalog", { allowNetwork: false }));
		__cases.push(rcCase("allow-network-false", "withRemoteCatalog", { response: rc200([rcModel("r1")]), allowNetwork: false }));
		__cases.push(rcCase("force-true", "withRemoteCatalog", { stored: rcStored([rcModel("s1")]), force: true, response: rc200([rcModel("r1")]) }));
		__cases.push(rcCase("stored-304-with-models", "withRemoteCatalog", { stored: rcStored([rcModel("s1")]), response: { status: 304, etag: "e" } }));
		__cases.push(rcCase("stored-404", "withRemoteCatalog", { stored: rcStored([rcModel("s1")]), response: { status: 404 } }));
		__cases.push(rcCase("stored-501", "withRemoteCatalog", { stored: rcStored([rcModel("s1")]), response: { status: 501 } }));
		__cases.push(rcCase("no-stored-404", "withRemoteCatalog", { response: { status: 404 } }));
		__cases.push(rcCase("not-ok-400", "withRemoteCatalog", { response: { status: 400 } }));
		__cases.push(rcCase("freshness-skip", "withRemoteCatalog", { stored: rcStored([rcModel("s1")]), response: rc200([rcModel("r1")]) }));
		__cases.push(rcCase("freshness-expired", "withRemoteCatalog", { stored: rcStored([rcModel("s1")], { checkedAt: 999999000000 - 5 * 60 * 60 * 1000 }), response: rc200([rcModel("r1")]) }));
		__cases.push(rcCase("stored-no-last-modified", "withRemoteCatalog", { stored: { models: [rcModel("s1")], checkedAt: 1000000000000 }, response: rc200([rcModel("r1")]) }));
		__cases.push(rcCase("stored-older-local", "withRemoteCatalog", { localGeneratedAt: 2000000000000, stored: rcStored([rcModel("s1")]) }));
		__cases.push(rcCase("stored-newer-local", "withRemoteCatalog", { localGeneratedAt: 500000000000, stored: rcStored([rcModel("s1")]) }));
		__cases.push(rcCase("local-undefined-restore", "withRemoteCatalog", { stored: rcStored([rcModel("s1")]), allowNetwork: false }));
		__cases.push(rcCase("etag-empty-stored", "withRemoteCatalog", { stored: { models: [], checkedAt: 999999000000, lastModified: 1000000000000, etag: "e" }, response: rc200([rcModel("r1")]) }));
		__cases.push(rcCase("pers-identity", "withPersonalizedCatalog", { response: rc200([rcModel("r1")]), personalize: { op: "identity" } }));
		__cases.push(rcCase("pers-empty", "withPersonalizedCatalog", { response: rc200([rcModel("r1")]), personalize: { op: "empty" } }));
		__cases.push(rcCase("pers-reverse", "withPersonalizedCatalog", { response: rc200([rcModel("r1")]), personalize: { op: "reverse" } }));
		__cases.push(rcCase("pers-ids", "withPersonalizedCatalog", { response: rc200([rcModel("r1")]), personalize: { op: "ids", ids: ["b2"] } }));
		__cases.push(rcCase("pers-404", "withPersonalizedCatalog", { response: { status: 404 }, personalize: { op: "identity" } }));
		__cases.push(rcCase("pers-not-ok", "withPersonalizedCatalog", { response: { status: 400 }, personalize: { op: "identity" } }));
		__cases.push(rcCase("pers-stored", "withPersonalizedCatalog", { stored: rcStored([rcModel("s1")]), allowNetwork: false, personalize: { op: "identity" } }));
		__cases.push(rcCase("pers-allow-network-false", "withPersonalizedCatalog", { response: rc200([rcModel("r1")]), allowNetwork: false, personalize: { op: "identity" } }));
		__cases.push(rcCase("pers-local-newer", "withPersonalizedCatalog", { localGeneratedAt: 2000000000000, response: rc200([rcModel("r1")]), personalize: { op: "identity" } }));

		// =====================================================================
		// provider-composer
		// =====================================================================

		const pcModel = (id, extra = {}) => ({
			id,
			name: id,
			api: "openai-completions",
			provider: "prov",
			baseUrl: "https://api.example",
			input: ["text"],
			cost: { input: 1, output: 2, cacheRead: 0.5, cacheWrite: 0.25 },
			reasoning: false,
			contextWindow: 10000,
			maxTokens: 1000,
			...extra,
		});
		const pcBase = (extra = {}) => ({
			id: "prov",
			name: "Base Provider",
			baseUrl: "https://base.example",
			headers: { "X-Base": "1" },
			models: [pcModel("b1"), pcModel("b2", { reasoning: true })],
			auth: { apiKey: { name: "Base key", resolve: { auth: { apiKey: "resolved-base" } } } },
			...extra,
		});
		const pcCase = (name, fn, extra = {}) => ({
			tool: "provider-composer",
			name: "pc-" + name,
			args: { fn, providerId: "prov", ...extra },
		});
		const pcExt = (extra = {}) => ({
			name: "Ext Provider",
			baseUrl: "https://ext.example",
			apiKey: "ext-key",
			...extra,
		});

		__cases.push(pcCase("validate-ok", "validateExtensionProvider", { base: pcBase(), extension: pcExt({ models: [pcModel("e1")] }) }));
		__cases.push(pcCase("validate-no-models", "validateExtensionProvider", { base: pcBase(), extension: pcExt() }));
		__cases.push(pcCase("validate-missing-base-url", "validateExtensionProvider", { base: pcBase(), extension: pcExt({ models: [pcModel("e1", { baseUrl: undefined })] }) }));
		__cases.push(pcCase("validate-config-oauth-no-base-url", "validateExtensionProvider", { base: pcBase(), modelConfig: { providers: { prov: { oauth: "radius" } } }, extension: pcExt() }));
		__cases.push(pcCase("compose-layers", "composeModelProvider", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://config.example" } } }, extension: pcExt({ models: [pcModel("e1")] }) }));
		__cases.push(pcCase("compose-base-only", "composeModelProvider", { base: pcBase() }));
		__cases.push(pcCase("compose-no-auth", "composeModelProvider", { base: { ...pcBase(), auth: {} }, modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } } }));
		__cases.push(pcCase("compose-oauth-only", "composeModelProvider", { base: { ...pcBase(), auth: {} }, extension: pcExt({ apiKey: undefined, oauth: { name: "Ext OAuth", getApiKey: "static-key" } }), modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } } }));
		__cases.push(pcCase("compose-name-precedence", "composeModelProvider", { base: pcBase(), modelConfig: { providers: { prov: { name: "Config Name" } } }, extension: pcExt() }));
		__cases.push(pcCase("compose-models-json-models", "composeModelProvider", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://config.example", models: [{ id: "c1", name: "C1", input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 5000, maxTokens: 500 }] } } } }));
		__cases.push(pcCase("compose-models-json-override", "composeModelProvider", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://config.example", modelOverrides: { b1: { name: "Renamed", maxTokens: 777 } } } } } }));
		__cases.push(pcCase("compose-models-json-empty-error", "composeModelProvider", { base: pcBase(), modelConfig: { providers: { prov: {} } } }));
		__cases.push(pcCase("compose-mixed-types", "composeModelProvider", { base: { ...pcBase(), models: [pcModel("b1"), { ...pcModel("im1"), type: "image", output: ["image"] }, { ...pcModel("cl1"), type: "classifier", contextWindow: 100 }] } }));
		__cases.push(pcCase("compose-refresh-from-base", "composeModelProvider", { base: pcBase({ refreshModels: { models: [] } }) }));
		__cases.push(pcCase("refresh-extension", "refreshModels", { base: pcBase(), extension: pcExt({ models: [pcModel("e1")], refreshModels: { models: [pcModel("e1"), pcModel("e2")] } }) }));
		__cases.push(pcCase("refresh-publish-rejected", "refreshModels", { base: pcBase(), extension: pcExt({ models: [pcModel("e1")], refreshModels: { models: [pcModel("e2")] } }), publishAccept: false }));
		__cases.push(pcCase("refresh-aborted", "refreshModels", { base: pcBase(), extension: pcExt({ models: [pcModel("e1")], refreshModels: { models: [pcModel("e2")] } }), aborted: true }));
		__cases.push(pcCase("refresh-extension-error", "refreshModels", { base: pcBase(), extension: pcExt({ refreshModels: { error: "boom" } }) }));
		__cases.push(pcCase("refresh-base-error", "refreshModels", { base: pcBase({ refreshModels: { error: "base boom" } }), extension: pcExt({ refreshModels: { models: [pcModel("e2")] } }) }));
		__cases.push(pcCase("refresh-oauth-modify", "refreshModels", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } }, extension: pcExt({ apiKey: undefined, oauth: { name: "O", getApiKey: "k", modifyModels: { op: "prefixIds" } } }), credential: { type: "oauth", access: "a", refresh: "r", expires: 1 } }));
		__cases.push(pcCase("refresh-oauth-no-credential", "refreshModels", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } }, extension: pcExt({ apiKey: undefined, oauth: { name: "O", getApiKey: "k", modifyModels: { op: "reverse" } } }) }));
		__cases.push(pcCase("headers-config-model", "resolveConfiguredModelHeaders", { model: pcModel("b1"), modelConfig: { providers: { prov: { baseUrl: "https://x.example", models: [{ id: "b1", headers: { "X-M": "$HDR" } }] } } }, env: { HDR: "mv" } }));
		__cases.push(pcCase("headers-override", "resolveConfiguredModelHeaders", { model: pcModel("b1"), modelConfig: { providers: { prov: { baseUrl: "https://x.example", modelOverrides: { b1: { headers: { "X-O": "1" } } } } } } }));
		__cases.push(pcCase("headers-extension-model", "resolveConfiguredModelHeaders", { model: pcModel("b1"), extension: pcExt({ models: [pcModel("b1", { headers: { "X-E": "2" } })] }) }));
		__cases.push(pcCase("headers-none", "resolveConfiguredModelHeaders", { model: pcModel("b1") }));
		__cases.push(pcCase("headers-missing-env", "resolveConfiguredModelHeaders", { model: pcModel("b1"), modelConfig: { providers: { prov: { baseUrl: "https://x.example", models: [{ id: "b1", headers: { "X-M": "$NOPE" } }] } } } }));
		__cases.push(pcCase("compat-config", "resolveCompatibilityRequestConfig", { model: pcModel("b1", { headers: { "X-Model": "m" } }), modelConfig: { providers: { prov: { baseUrl: "https://x.example", headers: { "X-C": "c" }, authHeader: true } } } }));
		__cases.push(pcCase("compat-extension-auth-header", "resolveCompatibilityRequestConfig", { model: pcModel("b1"), extension: pcExt({ authHeader: true }) }));
		__cases.push(pcCase("compat-none", "resolveCompatibilityRequestConfig", { model: pcModel("b1") }));
		__cases.push(pcCase("status-models-json-key", "configuredRequestAuthStatus", { modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "literal" } } } }));
		__cases.push(pcCase("status-fallback-extension", "configuredRequestAuthStatus", { extension: pcExt(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "literal" } } } }));
		__cases.push(pcCase("status-env-configured", "configuredRequestAuthStatus", { modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "$MY_KEY" } } }, env: { MY_KEY: "v" } }));
		__cases.push(pcCase("status-env-missing", "configuredRequestAuthStatus", { modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "$MY_KEY" } } } }));
		__cases.push(pcCase("status-command", "configuredRequestAuthStatus", { modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "!echo k" } } } }));
		__cases.push(pcCase("status-none", "configuredRequestAuthStatus", { modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } } }));
		__cases.push(pcCase("check-credential-key", "apiKeyCheck", { base: pcBase(), credential: { type: "api_key", key: "stored" } }));
		__cases.push(pcCase("check-credential-no-key", "apiKeyCheck", { base: pcBase(), credential: { type: "api_key" } }));
		__cases.push(pcCase("check-inherited", "apiKeyCheck", { base: pcBase({ auth: { apiKey: { name: "B", check: { source: "inherited-check" }, resolve: { auth: { apiKey: "r" } } } } }) }));
		__cases.push(pcCase("check-raw-env-present", "apiKeyCheck", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "$MY_KEY" } } }, env: { MY_KEY: "v" } }));
		__cases.push(pcCase("check-raw-env-missing", "apiKeyCheck", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "$MY_KEY" } } } }));
		__cases.push(pcCase("check-command-key", "apiKeyCheck", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "!echo k" } } } }));
		__cases.push(pcCase("resolve-credential", "apiKeyResolve", { base: pcBase(), credential: { type: "api_key", key: "stored" } }));
		__cases.push(pcCase("resolve-raw-env", "apiKeyResolve", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "$MY_KEY" } } }, env: { MY_KEY: "v" } }));
		__cases.push(pcCase("resolve-raw-missing-env", "apiKeyResolve", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "$MY_KEY" } } } }));
		__cases.push(pcCase("resolve-inherited", "apiKeyResolve", { base: pcBase({ auth: { apiKey: { name: "B", resolve: { auth: { apiKey: "inherited" } } } } }), credential: { type: "api_key", key: "stored" } }));
		__cases.push(pcCase("resolve-headers-auth-header", "apiKeyResolve", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", apiKey: "k", headers: { "X-H": "$HDR" }, authHeader: true } } }, env: { HDR: "hv" }, credential: { type: "api_key", key: "stored" } }));
		__cases.push(pcCase("resolve-credential-env", "apiKeyResolve", { base: pcBase(), credential: { type: "api_key", key: "stored", env: { A: "1" } } }));
		__cases.push(pcCase("oauth-static", "oauthToAuth", { base: pcBase(), extension: pcExt({ apiKey: undefined, oauth: { name: "O", getApiKey: "static-key" } }), modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } }, credential: { type: "oauth", access: "a", refresh: "r", expires: 1 } }));
		__cases.push(pcCase("oauth-field", "oauthToAuth", { base: pcBase(), extension: pcExt({ apiKey: undefined, oauth: { name: "O", getApiKey: { field: "access" } } }), modelConfig: { providers: { prov: { baseUrl: "https://x.example" } } }, credential: { type: "oauth", access: "the-access", refresh: "r", expires: 1 } }));
		__cases.push(pcCase("oauth-headers", "oauthToAuth", { base: pcBase(), modelConfig: { providers: { prov: { baseUrl: "https://x.example", headers: { Authorization: "nope", "X-O": "$HDR" } } } }, extension: pcExt({ apiKey: undefined, oauth: { name: "O", getApiKey: "k" } }), credential: { type: "oauth", access: "a", refresh: "r", expires: 1, env: { HDR: "hv" } } }));
		__cases.push(pcCase("images-extension", "generateImages", { base: pcBase(), extension: pcExt({ images: { "openai-images": { marker: "ext-images" } } }), model: { ...pcModel("im1"), type: "image", output: ["image"] } }));
		__cases.push(pcCase("images-error", "generateImages", { base: pcBase(), model: { ...pcModel("im1"), type: "image", output: ["image"] } }));
		__cases.push(pcCase("classify-extension", "classify", { base: pcBase(), extension: pcExt({ classifiers: { "openai-classifiers": { marker: "ext-classify" } } }), model: { ...pcModel("cl1"), type: "classifier", contextWindow: 100 } }));
		__cases.push(pcCase("classify-error", "classify", { base: pcBase(), model: { ...pcModel("cl1"), type: "classifier", contextWindow: 100 } }));

		return __cases;
	})(),

// model-runtime / model-registry (facade wave). Steps-based protocol: one
// runtime per case, `steps` run in order; `settle: true` on register steps
// runs an explicit refresh({allowNetwork:false}) on both sides (the TS
// register* fns fire a discarded void refresh the sync port skips).
...(() => {
	const mr = (name, args) => ({ tool: "model-runtime", name: "mr-" + name, args });
	const reg = (name, args) => ({ tool: "model-registry", name: "reg-" + name, args });
	const __cases = [];
	const mrModel = (id, provider = "ext1", extra = {}) => ({
		id,
		name: id,
		api: "openai-completions",
		provider,
		baseUrl: "https://ext.example",
		input: ["text"],
		cost: { input: 1, output: 2, cacheRead: 0.5, cacheWrite: 0.25 },
		reasoning: false,
		contextWindow: 10000,
		maxTokens: 1000,
		...extra,
	});
	const mrImageModel = (id, provider) => ({ ...mrModel(id, provider), type: "image", output: ["image"], api: "openai-images" });
	const mrClassifierModel = (id, provider) => ({ ...mrModel(id, provider), type: "classifier", contextWindow: 100, api: "openai-classifiers" });
	const mrVirtual = (extra = {}) => ({
		provider: "anthropic",
		id: "vm1",
		name: "VM One",
		route: { provider: "anthropic", model: "claude-fable-5", thinkingLevel: "low", echo: true },
		...extra,
	});
	const mrSteps = (steps, extra = {}) => ({ steps, offline: true, ...extra });

	// ---- catalog basics ----
	__cases.push(mr("providers-offline", mrSteps([{ fn: "getProviders" }])));
	__cases.push(mr("providers-env", mrSteps(
		[
			{ fn: "checkAuth", providerId: "openai" },
			{ fn: "getProviderAuthStatus", providerId: "openai" },
			{ fn: "hasConfiguredAuth", providerId: "openai" },
			{ fn: "isUsingOAuth", providerId: "openai" },
			{ fn: "isUsingSubscription", providerId: "openai" },
		],
		{ env: { OPENAI_API_KEY: "sk-test" } },
	)));
	__cases.push(mr("models-all", mrSteps([{ fn: "getModels" }])));
	__cases.push(mr("models-provider", mrSteps([{ fn: "getModels", providerId: "anthropic" }])));
	__cases.push(mr("model-found", mrSteps([{ fn: "getModel", providerId: "anthropic", modelId: "claude-fable-5" }])));
	__cases.push(mr("model-not-found", mrSteps([{ fn: "getModel", providerId: "anthropic", modelId: "nope" }])));
	__cases.push(mr("models-of-type-image", mrSteps([{ fn: "getModelsOfType", type: "image" }])));
	__cases.push(mr("models-of-type-classifier-scoped", mrSteps([{ fn: "getModelsOfType", type: "classifier", providerId: "cloudflare-workers-ai" }])));
	__cases.push(mr("model-of-type", mrSteps([
		{ fn: "getModelOfType", type: "image", providerId: "openrouter", modelId: "black-forest-labs/flux-3-image" },
		{ fn: "getModelOfType", type: "image", providerId: "openrouter", modelId: "nope" },
	])));
	__cases.push(mr("all-models-provider", mrSteps([{ fn: "getAllModels", providerId: "openai" }])));
	__cases.push(mr("error-clean", mrSteps([{ fn: "getError" }])));
	__cases.push(mr("snapshot-empty", mrSteps([{ fn: "getAvailableSnapshot" }])));

	// ---- availability & auth status ----
	__cases.push(mr("available-env", mrSteps([{ fn: "getAvailable" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(mr("available-provider-env", mrSteps([{ fn: "getAvailable", providerId: "anthropic" }], { env: { ANTHROPIC_API_KEY: "sk-ant" } })));
	__cases.push(mr("available-none", mrSteps([{ fn: "getAvailable" }])));
	__cases.push(mr("check-auth-env", mrSteps([{ fn: "checkAuth", providerId: "anthropic" }], { env: { ANTHROPIC_API_KEY: "sk-ant" } })));
	__cases.push(mr("check-auth-none", mrSteps([{ fn: "checkAuth", providerId: "openai" }])));
	__cases.push(mr("status-env", mrSteps([{ fn: "getProviderAuthStatus", providerId: "openai" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(mr("status-none", mrSteps([{ fn: "getProviderAuthStatus", providerId: "anthropic" }])));
	__cases.push(mr("runtime-key-status", mrSteps([
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "getProviderAuthStatus", providerId: "anthropic" },
		{ fn: "listCredentials" },
		{ fn: "hasConfiguredAuth", providerId: "anthropic" },
		{ fn: "getAvailableSnapshot" },
	])));
	__cases.push(mr("remove-runtime-key", mrSteps([
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "removeRuntimeApiKey", providerId: "anthropic" },
		{ fn: "getProviderAuthStatus", providerId: "anthropic" },
		{ fn: "listCredentials" },
	])));
	__cases.push(mr("stored-credential", mrSteps(
		[
			{ fn: "getAvailable" },
			{ fn: "getProviderAuthStatus", providerId: "anthropic" },
			{ fn: "listCredentials" },
			{ fn: "checkAuth", providerId: "anthropic" },
		],
		{ authJson: JSON.stringify({ anthropic: { type: "api_key", key: "stored-key" } }) },
	)));

	// ---- getAuth ----
	__cases.push(mr("getauth-env", mrSteps([{ fn: "getAuth", providerId: "openai" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(mr("getauth-model", mrSteps([{ fn: "getAuthModel", model: { provider: "openai", modelId: "gpt-4" } }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(mr("getauth-none", mrSteps([{ fn: "getAuth", providerId: "anthropic" }])));

	// ---- register / unregister ----
	__cases.push(mr("register-provider", mrSteps([
		{ fn: "registerProvider", providerId: "ext1", config: { name: "Ext One", baseUrl: "https://ext.example", apiKey: "ext-key", models: [mrModel("m1")] }, settle: true },
		{ fn: "getModels", providerId: "ext1" },
		{ fn: "getRegisteredProviderIds" },
		{ fn: "getRegisteredProviderConfig", providerId: "ext1" },
		{ fn: "getProviderAuthStatus", providerId: "ext1" },
		{ fn: "hasConfiguredAuth", providerId: "ext1" },
	])));
	__cases.push(mr("register-provider-merge", mrSteps([
		{ fn: "registerProvider", providerId: "ext1", config: { name: "Ext One", baseUrl: "https://ext.example", models: [mrModel("m1")] }, settle: true },
		{ fn: "registerProvider", providerId: "ext1", config: { name: "Ext Two" }, settle: true },
		{ fn: "getRegisteredProviderConfig", providerId: "ext1" },
		{ fn: "getModels", providerId: "ext1" },
	])));
	__cases.push(mr("register-provider-invalid", mrSteps([
		{ fn: "registerProvider", providerId: "ext1", config: { name: "No Base" } },
		{ fn: "getRegisteredProviderIds" },
	])));
	__cases.push(mr("register-native", mrSteps([
		{ fn: "registerNativeProvider", provider: { id: "nat1", name: "Native One", baseUrl: "https://nat.example", models: [mrModel("n1", "nat1")] }, settle: true },
		{ fn: "getModels", providerId: "nat1" },
		{ fn: "getRegisteredNativeProvider", providerId: "nat1" },
		{ fn: "getRegisteredProviderIds" },
	])));
	__cases.push(mr("register-native-empty-id", mrSteps([
		{ fn: "registerNativeProvider", provider: { id: "", models: [] } },
	])));
	__cases.push(mr("unregister-provider", mrSteps([
		{ fn: "registerProvider", providerId: "ext1", config: { name: "Ext One", baseUrl: "https://ext.example", models: [mrModel("m1")] }, settle: true },
		{ fn: "unregisterProvider", providerId: "ext1", settle: true },
		{ fn: "getModels", providerId: "ext1" },
		{ fn: "getRegisteredProviderIds" },
	])));

	// ---- virtual models ----
	__cases.push(mr("virtual-register", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual(), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "getModels", providerId: "anthropic" },
		{ fn: "getModel", providerId: "anthropic", modelId: "vm1" },
	])));
	__cases.push(mr("virtual-resolve", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual(), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "user", thinkingLevel: "high", messages: [{ role: "assistant", provider: "openai", model: "gpt-4", thinkingLevel: "low", stopReason: "stop" }] },
	])));
	__cases.push(mr("virtual-resolve-failed", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual(), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "retry", thinkingLevel: "low", failed: { role: "assistant", provider: "openai", model: "gpt-4", thinkingLevel: "low", stopReason: "error", errorMessage: "boom" } },
	])));
	__cases.push(mr("virtual-resolve-state", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual({ route: { provider: "anthropic", model: "claude-fable-5", thinkingLevel: "low", echo: true, state: { step: 2 } } }), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "continuation", thinkingLevel: "low", state: { step: 1 } },
	])));
	__cases.push(mr("virtual-no-credentials", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual(), settle: true },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "user", thinkingLevel: "low" },
	])));
	__cases.push(mr("virtual-not-physical", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual({ route: { provider: "anthropic", model: "nope" } }), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "user", thinkingLevel: "low" },
	])));
	__cases.push(mr("virtual-conflict", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual({ id: "claude-fable-5" }) },
	])));
	__cases.push(mr("virtual-empty", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual({ provider: "" }) },
	])));
	__cases.push(mr("virtual-not-registered", mrSteps([
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "claude-fable-5" }, reason: "user", thinkingLevel: "low" },
	])));
	__cases.push(mr("virtual-unregister", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual(), settle: true },
		{ fn: "unregisterVirtualModel", providerId: "anthropic", modelId: "vm1", settle: true },
		{ fn: "getModel", providerId: "anthropic", modelId: "vm1" },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "user", thinkingLevel: "low" },
	])));

	// ---- login / logout ----
	__cases.push(mr("login-error", mrSteps([{ fn: "login", providerId: "openai", type: "api_key" }])));
	__cases.push(mr("logout", mrSteps(
		[
			{ fn: "getAvailable" },
			{ fn: "logout", providerId: "anthropic" },
			{ fn: "listCredentials" },
			{ fn: "getProviderAuthStatus", providerId: "anthropic" },
		],
		{ authJson: JSON.stringify({ anthropic: { type: "api_key", key: "stored-key" } }) },
	)));

	// ---- refresh ----
	__cases.push(mr("refresh-offline", mrSteps([{ fn: "refresh", allowNetwork: false }])));
	__cases.push(mr("refresh-network-gated-offline", mrSteps([{ fn: "refresh", allowNetwork: true }])));
	__cases.push(mr("refresh-fetch", mrSteps(
		[
			{ fn: "refresh", allowNetwork: true, providers: ["openai"], force: true },
			{ fn: "getModels", providerId: "openai" },
		],
		{
			fetch: { status: 200, lastModified: 1792457207915, body: { models: [mrModel("dyn-1", "openai")] } },
			stored: { openai: { models: [], lastModified: 1792457207915, checkedAt: 0, etag: "\"e1\"" } },
		},
	)));

	// ---- models.json overlays ----
	__cases.push(mr("overlay-status", mrSteps(
		[{ fn: "getProviderAuthStatus", providerId: "openai" }, { fn: "getAuth", providerId: "openai" }],
		{ modelsJson: JSON.stringify({ providers: { openai: { baseUrl: "https://x.example", apiKey: "cfg-key" } } }) },
	)));
	__cases.push(mr("overlay-model-override", mrSteps(
		[{ fn: "getModel", providerId: "openai", modelId: "gpt-4" }],
		{ modelsJson: JSON.stringify({ providers: { openai: { baseUrl: "https://x.example", modelOverrides: { "gpt-4": { name: "Renamed GPT", maxTokens: 777 } } } } }) },
	)));
	__cases.push(mr("compat-config", mrSteps(
		[{ fn: "getCompatibilityRequestConfig", model: { provider: "openai", modelId: "gpt-4" } }],
		{ modelsJson: JSON.stringify({ providers: { openai: { baseUrl: "https://x.example", headers: { "X-Cfg": "c" }, authHeader: true, modelOverrides: { "gpt-4": { headers: { "X-Model": "m" } } } } } }) },
	)));
	__cases.push(mr("composition-error", mrSteps(
		[{ fn: "getError" }],
		{ modelsJson: JSON.stringify({ providers: { prov: { baseUrl: "https://x.example", models: [{ id: "m1", api: "no-such-api" }] } } }) },
	)));

	// ---- images / classifiers ----
	__cases.push(mr("generate-images-error", mrSteps([
		{ fn: "generateImages", model: { provider: "openrouter", modelId: "black-forest-labs/flux-3-image" }, context: { input: [] } },
	])));
	__cases.push(mr("generate-images-marker", mrSteps([
		{ fn: "registerNativeProvider", provider: { id: "nat1", name: "Native One", baseUrl: "https://nat.example", models: [mrImageModel("im1", "nat1")], images: { "openai-images": { marker: "nat-images" } } }, settle: true },
		{ fn: "setRuntimeApiKey", providerId: "nat1", apiKey: "rk" },
		{ fn: "generateImages", model: { provider: "nat1", modelId: "im1" }, context: { input: [] } },
	])));
	__cases.push(mr("classify-error", mrSteps([
		{ fn: "classify", model: { provider: "cloudflare-workers-ai", modelId: "@cf/cloudflare/clef" }, context: { state: {}, questions: {} } },
	])));
	__cases.push(mr("classify-marker", mrSteps([
		{ fn: "registerNativeProvider", provider: { id: "nat1", name: "Native One", baseUrl: "https://nat.example", models: [mrClassifierModel("cl1", "nat1")], classifiers: { "openai-classifiers": { marker: "nat-classify" } } }, settle: true },
		{ fn: "setRuntimeApiKey", providerId: "nat1", apiKey: "rk" },
		{ fn: "classify", model: { provider: "nat1", modelId: "cl1" }, context: { state: {}, questions: {} } },
	])));

	// ---- registry facade ----
	__cases.push(reg("getall", mrSteps([{ fn: "getAll" }])));
	__cases.push(reg("getavailable-env", mrSteps([{ fn: "getAvailable" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(reg("find", mrSteps([
		{ fn: "find", providerId: "anthropic", modelId: "claude-fable-5" },
		{ fn: "find", providerId: "anthropic", modelId: "nope" },
	])));
	__cases.push(reg("find-of-type", mrSteps([
		{ fn: "findOfType", type: "image", providerId: "openrouter", modelId: "black-forest-labs/flux-3-image" },
		{ fn: "findOfType", type: "image", providerId: "openrouter", modelId: "nope" },
	])));
	__cases.push(reg("has-configured-auth", mrSteps(
		[
			{ fn: "hasConfiguredAuth", model: { provider: "openai", modelId: "gpt-4" } },
			{ fn: "hasConfiguredAuth", model: { provider: "anthropic", modelId: "claude-fable-5" } },
		],
		{ env: { OPENAI_API_KEY: "sk-test" } },
	)));
	__cases.push(reg("apikey-headers-ok", mrSteps(
		[{ fn: "getApiKeyAndHeaders", model: { provider: "openai", modelId: "gpt-4" } }],
		{ env: { OPENAI_API_KEY: "sk-test" } },
	)));
	__cases.push(reg("apikey-headers-nokey", mrSteps(
		[{ fn: "getApiKeyAndHeaders", model: { provider: "openai", modelId: "gpt-4" } }],
	)));
	__cases.push(reg("apikey-headers-authheader", mrSteps(
		[{ fn: "getApiKeyAndHeaders", model: { provider: "openai", modelId: "gpt-4" } }],
		{ modelsJson: JSON.stringify({ providers: { openai: { baseUrl: "https://x.example", authHeader: true } } }) },
	)));
	__cases.push(reg("display-name", mrSteps([
		{ fn: "getProviderDisplayName", providerId: "anthropic" },
		{ fn: "getProviderDisplayName", providerId: "nope" },
	])));
	__cases.push(reg("provider-auth", mrSteps([{ fn: "getProviderAuth", providerId: "openai" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(reg("apikey-for-provider", mrSteps([
		{ fn: "getApiKeyForProvider", providerId: "openai" },
		{ fn: "getApiKeyForProvider", providerId: "anthropic" },
	], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(reg("is-using-oauth", mrSteps([{ fn: "isUsingOAuth", model: { provider: "openai", modelId: "gpt-4" } }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(reg("models-of-type", mrSteps([{ fn: "getModelsOfType", type: "image" }])));
	__cases.push(reg("available-of-type", mrSteps([{ fn: "getAvailableOfType", type: "image" }], { env: { OPENROUTER_API_KEY: "or-test" } })));
	__cases.push(reg("model-of-type", mrSteps([{ fn: "getModelOfType", type: "image", providerId: "openrouter", modelId: "black-forest-labs/flux-3-image" }])));
	__cases.push(reg("get-provider", mrSteps([
		{ fn: "getProvider", providerId: "anthropic" },
		{ fn: "getProvider", providerId: "nope" },
	])));
	__cases.push(reg("get-error", mrSteps([{ fn: "getError" }])));
	__cases.push(reg("check-auth", mrSteps([{ fn: "checkAuth", providerId: "openai" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(reg("status", mrSteps([{ fn: "getProviderAuthStatus", providerId: "openai" }], { env: { OPENAI_API_KEY: "sk-test" } })));
	__cases.push(reg("registered-ids", mrSteps([{ fn: "getRegisteredProviderIds" }])));
	__cases.push(reg("get-models", mrSteps([{ fn: "getModels", providerId: "anthropic" }])));
	__cases.push(reg("get-model", mrSteps([{ fn: "getModel", providerId: "anthropic", modelId: "claude-fable-5" }])));
	__cases.push(reg("register-by-name", mrSteps([
		{ fn: "registerProvider", providerId: "ext1", config: { name: "Ext One", baseUrl: "https://ext.example", apiKey: "ext-key", models: [mrModel("m1")] }, settle: true },
		{ fn: "getModels", providerId: "ext1" },
		{ fn: "find", providerId: "ext1", modelId: "m1" },
		{ fn: "getRegisteredProviderConfig", providerId: "ext1" },
	])));
	__cases.push(reg("register-by-name-no-config", mrSteps([
		{ fn: "registerProvider", providerId: "ext1" },
	])));
	__cases.push(reg("register-native", mrSteps([
		{ fn: "registerNativeProvider", provider: { id: "nat1", name: "Native One", baseUrl: "https://nat.example", models: [mrModel("n1", "nat1")] }, settle: true },
		{ fn: "getModels", providerId: "nat1" },
		{ fn: "getRegisteredNativeProvider", providerId: "nat1" },
	])));
	__cases.push(reg("unregister", mrSteps([
		{ fn: "registerProvider", providerId: "ext1", config: { name: "Ext One", baseUrl: "https://ext.example", models: [mrModel("m1")] }, settle: true },
		{ fn: "unregisterProvider", providerId: "ext1", settle: true },
		{ fn: "find", providerId: "ext1", modelId: "m1" },
	])));
	__cases.push(reg("register-virtual", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual({ route: { provider: "anthropic", model: "claude-fable-5" } }), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "getAll" },
	])));
	__cases.push(reg("resolve-model", mrSteps([
		{ fn: "registerVirtualModel", definition: mrVirtual(), settle: true },
		{ fn: "setRuntimeApiKey", providerId: "anthropic", apiKey: "rk" },
		{ fn: "resolveModel", model: { provider: "anthropic", modelId: "vm1" }, reason: "user", thinkingLevel: "low" },
	])));
	__cases.push(reg("refresh", mrSteps([{ fn: "refresh", allowNetwork: false }])));
	__cases.push(reg("generate-images-error", mrSteps([
		{ fn: "generateImages", model: { provider: "openrouter", modelId: "black-forest-labs/flux-3-image" }, context: { input: [] } },
	])));
	__cases.push(reg("classify-error", mrSteps([
		{ fn: "classify", model: { provider: "cloudflare-workers-ai", modelId: "@cf/cloudflare/clef" }, context: { state: {}, questions: {} } },
	])));

	return __cases;
})(),

// ---- landed core-module wave: event-bus, timings, slash-commands, ----------
// settings-defaults, settings-schema (see pi-cli core_harness.rs and the
// matching arms in ts-harness.js for the wire protocol).

// event-bus: script-driven bus with numbered handlers.
...(() => {
	const eb = (name, ops) => ({ tool: "event-bus", name: "eb-" + name, args: { ops } });
	const __cases = [];

	// basic delivery
	__cases.push(eb("single-handler", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: { n: 1 } },
	]));
	__cases.push(eb("two-handlers-order", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "emit", channel: "evt", data: "x" },
	]));
	__cases.push(eb("emit-no-listeners", [
		{ op: "emit", channel: "nobody", data: 42 },
	]));
	__cases.push(eb("channels-isolated", [
		{ op: "on", channel: "a", handler: 1 },
		{ op: "on", channel: "b", handler: 2 },
		{ op: "emit", channel: "a", data: 1 },
		{ op: "emit", channel: "b", data: 2 },
		{ op: "emit", channel: "c", data: 3 },
	]));

	// data shapes (the bus signature is single-data emit)
	__cases.push(eb("data-null", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt" },
	]));
	__cases.push(eb("data-array", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: [1, "two", { three: 3 }] },
	]));
	__cases.push(eb("data-bool-int", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: false },
		{ op: "emit", channel: "evt", data: 0 },
	]));
	__cases.push(eb("data-nested-object", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: { a: { b: [null, true] }, c: "d" } },
	]));

	// unsubscribe
	__cases.push(eb("off-removes-handler", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "off", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: 2 },
	]));
	__cases.push(eb("off-idempotent", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "off", channel: "evt", handler: 1 },
		{ op: "off", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: 1 },
	]));
	__cases.push(eb("off-unknown-id-noop", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "off", channel: "evt", handler: 9 },
		{ op: "emit", channel: "evt", data: 1 },
	]));
	__cases.push(eb("off-all-channel", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "on", channel: "other", handler: 3 },
		{ op: "off", channel: "evt" },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "emit", channel: "other", data: 2 },
	]));
	__cases.push(eb("off-all-unused-channel", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "off", channel: "nope" },
		{ op: "emit", channel: "evt", data: 1 },
	]));
	__cases.push(eb("re-register-after-off-errors", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "off", channel: "evt", handler: 1 },
		{ op: "on", channel: "evt", handler: 1 },
	]));
	__cases.push(eb("handler-reuse-errors", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "on", channel: "other", handler: 1 },
	]));

	// clear
	__cases.push(eb("clear-removes-all", [
		{ op: "on", channel: "a", handler: 1 },
		{ op: "on", channel: "b", handler: 2 },
		{ op: "clear" },
		{ op: "emit", channel: "a", data: 1 },
		{ op: "emit", channel: "b", data: 2 },
	]));
	__cases.push(eb("clear-then-re-register", [
		{ op: "on", channel: "a", handler: 1 },
		{ op: "clear" },
		{ op: "on", channel: "a", handler: 2 },
		{ op: "emit", channel: "a", data: 3 },
	]));

	// once (harness-level emulation: self-unsubscribe, Node once semantics)
	__cases.push(eb("once-fires-once", [
		{ op: "once", channel: "evt", handler: 1 },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "emit", channel: "evt", data: 2 },
	]));
	__cases.push(eb("once-among-regular", [
		{ op: "once", channel: "evt", handler: 1 },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "emit", channel: "evt", data: "a" },
		{ op: "emit", channel: "evt", data: "b" },
	]));
	__cases.push(eb("once-throws-still-removed", [
		{ op: "once", channel: "evt", handler: 1, throws: "boom" },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "emit", channel: "evt", data: 2 },
	]));

	// error paths (handler throws a string; report captured on both sides)
	__cases.push(eb("throws-captured", [
		{ op: "on", channel: "boom", handler: 1, throws: "kaput" },
		{ op: "emit", channel: "boom", data: 42 },
	]));
	__cases.push(eb("throws-later-handlers-run", [
		{ op: "on", channel: "boom", handler: 1, throws: "kaput" },
		{ op: "on", channel: "boom", handler: 2 },
		{ op: "emit", channel: "boom", data: 42 },
	]));
	__cases.push(eb("throws-multiple-order", [
		{ op: "on", channel: "boom", handler: 1, throws: "one" },
		{ op: "on", channel: "boom", handler: 2 },
		{ op: "on", channel: "boom", handler: 3, throws: "two" },
		{ op: "emit", channel: "boom", data: null },
	]));
	__cases.push(eb("throws-non-first", [
		{ op: "on", channel: "boom", handler: 1 },
		{ op: "on", channel: "boom", handler: 2, throws: "late" },
		{ op: "emit", channel: "boom", data: "z" },
	]));

	// emit over the listener snapshot: off/clear during an emit
	__cases.push(eb("unsub-during-emit-still-runs", [
		{ op: "on", channel: "evt", handler: 1 },
		{ op: "on", channel: "evt", handler: 2, unsub: [2] },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "emit", channel: "evt", data: 2 },
	]));
	__cases.push(eb("unsub-other-during-emit", [
		{ op: "on", channel: "evt", handler: 1, unsub: [3] },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "on", channel: "evt", handler: 3 },
		{ op: "emit", channel: "evt", data: "a" },
		{ op: "emit", channel: "evt", data: "b" },
	]));
	__cases.push(eb("unsub-self-during-emit", [
		{ op: "on", channel: "evt", handler: 1, unsub: [1] },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "emit", channel: "evt", data: "a" },
		{ op: "emit", channel: "evt", data: "b" },
	]));
	__cases.push(eb("unsub-multiple-during-emit", [
		{ op: "on", channel: "evt", handler: 1, unsub: [2, 3] },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "on", channel: "evt", handler: 3 },
		{ op: "on", channel: "evt", handler: 4 },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "emit", channel: "evt", data: 2 },
	]));
	__cases.push(eb("clear-during-emit-still-runs", [
		{ op: "on", channel: "evt", handler: 1, clear: true },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "emit", channel: "evt", data: "a" },
		{ op: "emit", channel: "evt", data: "b" },
	]));
	__cases.push(eb("unsub-and-throw-during-emit", [
		{ op: "on", channel: "evt", handler: 1, unsub: [2], throws: "after" },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "emit", channel: "evt", data: 1 },
		{ op: "emit", channel: "evt", data: 2 },
	]));
	__cases.push(eb("once-unsub-other-during-emit", [
		{ op: "once", channel: "evt", handler: 1, unsub: [2] },
		{ op: "on", channel: "evt", handler: 2 },
		{ op: "emit", channel: "evt", data: "a" },
		{ op: "emit", channel: "evt", data: "b" },
	]));

	// protocol misuse (in-band errors, identical messages on both sides)
	__cases.push(eb("unknown-op", [{ op: "nope", channel: "evt" }]));
	__cases.push(eb("missing-op", [{ channel: "evt" }]));
	__cases.push({ tool: "event-bus", name: "eb-missing-ops", args: {} });

	return __cases;
})(),

// timings: script-driven timings with an injected clock.
...(() => {
	const tm = (name, steps, extra = {}) => ({
		tool: "timings", name: "tm-" + name, args: { steps, ...extra },
	});
	const __cases = [];

	__cases.push(tm("disabled", [
		{ step: "reset" },
		{ step: "time", label: "a" },
		{ step: "print" },
	], { enabled: false }));
	__cases.push(tm("enabled-explicit", [
		{ step: "time", label: "a" },
		{ step: "print" },
	], { enabled: true }));
	__cases.push(tm("basic-elapsed", [
		{ step: "reset" },
		{ step: "advance", ms: 10 },
		{ step: "time", label: "a" },
		{ step: "advance", ms: 20 },
		{ step: "time", label: "b" },
		{ step: "print" },
	]));
	__cases.push(tm("no-print", [
		{ step: "reset" },
		{ step: "advance", ms: 5 },
		{ step: "time", label: "a" },
	]));
	__cases.push(tm("implicit-reset-constant-clock", [
		{ step: "time", label: "only" },
		{ step: "print" },
	]));
	__cases.push(tm("implicit-reset-then-more", [
		{ step: "time", label: "first" },
		{ step: "advance", ms: 7 },
		{ step: "time", label: "second" },
		{ step: "print" },
	]));
	__cases.push(tm("negative-filtered", [
		{ step: "reset" },
		{ step: "time", label: "a" },
		{ step: "set", ms: 500 },
		{ step: "time", label: "bad" },
		{ step: "set", ms: 600 },
		{ step: "time", label: "c" },
		{ step: "print" },
	]));
	__cases.push(tm("all-negative-omits-group", [
		{ step: "reset" },
		{ step: "set", ms: 2000 },
		{ step: "time", label: "bad" },
		{ step: "print" },
	]));
	__cases.push(tm("namespaces-first-touch-order", [
		{ step: "time", label: "m1" },
		{ step: "time", label: "e1", namespace: "extensions" },
		{ step: "time", label: "m2" },
		{ step: "print" },
	]));
	__cases.push(tm("extensions-first", [
		{ step: "reset", namespace: "extensions" },
		{ step: "time", label: "e1", namespace: "extensions" },
		{ step: "time", label: "m1" },
		{ step: "print" },
	]));
	__cases.push(tm("reset-preserves-position", [
		{ step: "time", label: "m1" },
		{ step: "time", label: "e1", namespace: "extensions" },
		{ step: "reset" },
		{ step: "time", label: "m2" },
		{ step: "print" },
	]));
	__cases.push(tm("reset-clears-entries", [
		{ step: "reset" },
		{ step: "advance", ms: 3 },
		{ step: "time", label: "a" },
		{ step: "reset" },
		{ step: "time", label: "b" },
		{ step: "print" },
	]));
	__cases.push(tm("empty-group-omitted", [
		{ step: "reset" },
		{ step: "print" },
	]));
	__cases.push(tm("print-twice-identical", [
		{ step: "reset" },
		{ step: "advance", ms: 4 },
		{ step: "time", label: "a" },
		{ step: "print" },
		{ step: "print" },
	]));
	__cases.push(tm("time-after-print", [
		{ step: "reset" },
		{ step: "advance", ms: 1 },
		{ step: "time", label: "a" },
		{ step: "print" },
		{ step: "advance", ms: 2 },
		{ step: "time", label: "b" },
		{ step: "print" },
	]));
	__cases.push(tm("advance-negative", [
		{ step: "reset" },
		{ step: "advance", ms: 50 },
		{ step: "advance", ms: -20 },
		{ step: "time", label: "a" },
		{ step: "print" },
	]));
	__cases.push(tm("zero-elapsed-prints", [
		{ step: "reset" },
		{ step: "time", label: "a" },
		{ step: "time", label: "b" },
		{ step: "print" },
	]));
	__cases.push(tm("many-entries", [
		{ step: "reset" },
		{ step: "advance", ms: 1 },
		{ step: "time", label: "a" },
		{ step: "advance", ms: 2 },
		{ step: "time", label: "b" },
		{ step: "advance", ms: 3 },
		{ step: "time", label: "c" },
		{ step: "advance", ms: 4 },
		{ step: "time", label: "d" },
		{ step: "print" },
	]));
	__cases.push(tm("unknown-step", [{ step: "bogus" }]));
	__cases.push(tm("unknown-namespace", [{ step: "reset", namespace: "bogus" }]));
	__cases.push({ tool: "timings", name: "tm-missing-steps", args: {} });

	return __cases;
})(),

// slash-commands: builtin list + type round-trip.
...(() => {
	const sc = (name, args) => ({ tool: "slash-commands", name: "sc-" + name, args });
	const __cases = [];

	__cases.push(sc("list", { op: "list" }));
	__cases.push(sc("list-app-ignored", { op: "list", app: "pi" }));
	const info = (extra = {}) => ({
		name: "review",
		source: "skill",
		sourceInfo: {
			path: "/skills/review/skill.json",
			source: "skills",
			scope: "project",
			origin: "top-level",
		},
		...extra,
	});
	__cases.push(sc("info-minimal", { op: "roundtrip", kind: "info", value: info() }));
	__cases.push(sc("info-with-description", {
		op: "roundtrip", kind: "info", value: info({ description: "Run a review" }),
	}));
	__cases.push(sc("info-with-base-dir", {
		op: "roundtrip",
		kind: "info",
		value: info({ sourceInfo: { path: "p", source: "builtin", scope: "user", origin: "package", baseDir: "/x" } }),
	}));
	for (const source of ["extension", "prompt", "skill"]) {
		__cases.push(sc("info-source-" + source, {
			op: "roundtrip", kind: "info", value: info({ source }),
		}));
	}
	for (const scope of ["user", "project", "temporary"]) {
		__cases.push(sc("info-scope-" + scope, {
			op: "roundtrip",
			kind: "info",
			value: info({ sourceInfo: { path: "p", source: "s", scope, origin: "package" } }),
		}));
	}
	for (const origin of ["package", "top-level"]) {
		__cases.push(sc("info-origin-" + origin, {
			op: "roundtrip",
			kind: "info",
			value: info({ sourceInfo: { path: "p", source: "s", scope: "user", origin } }),
		}));
	}
	__cases.push(sc("builtin-minimal", {
		op: "roundtrip", kind: "builtin", value: { name: "settings", description: "Open settings menu" },
	}));
	__cases.push(sc("builtin-with-hint", {
		op: "roundtrip",
		kind: "builtin",
		value: { name: "model", description: "Select model", argumentHint: "<provider/model>" },
	}));
	__cases.push(sc("unknown-op", { op: "bogus" }));
	__cases.push(sc("unknown-kind", { op: "roundtrip", kind: "bogus", value: {} }));
	__cases.push({ tool: "slash-commands", name: "sc-missing-op", args: {} });

	return __cases;
})(),

// settings-defaults: fresh SETTINGS_DEFAULTS per call.
...(() => {
	const sd = (name, args) => ({ tool: "settings-defaults", name: "sd-" + name, args });
	return [
		sd("default", {}),
		sd("calls-1", { calls: 1 }),
		sd("calls-3", { calls: 3 }),
		sd("calls-0", { calls: 0 }),
		sd("calls-negative", { calls: -1 }),
	];
})(),

// settings-schema: validate_settings vs Compile(SettingsSchema).Errors.
...(() => {
	const ss = (name, settings) => ({ tool: "settings-schema", name: "ss-" + name, args: { settings } });
	const __cases = [];

	// valid
	__cases.push(ss("valid-empty", {}));
	__cases.push(ss("valid-assorted", {
		$schema: "https://example.com/schema.json",
		lastChangelogVersion: "1.2.3",
		defaultProvider: "anthropic",
		defaultModel: "claude-fable-5",
		defaultThinkingLevel: "high",
		modelThinkingLevels: { "anthropic/claude-fable-5": "low", "openai/gpt-5.2": "off" },
		transport: "websocket",
		sessionShareDestination: "gist",
		steeringMode: "all",
		followUpMode: "one-at-a-time",
		theme: "dark",
		compaction: {
			enabled: false,
			reserveTokens: 1000,
			keepRecentTokens: 2000,
			modelOverrides: { "anthropic/claude-fable-5": { reserveTokens: 500, keepRecentTokens: 700 } },
		},
		branchSummary: { reserveTokens: 2048, skipPrompt: true },
		retry: {
			enabled: false,
			maxRetries: 5,
			baseDelayMs: 500,
			maxAgentDelayMs: 10000,
			provider: { timeoutMs: 30000, maxRetries: 2, maxRetryDelayMs: 30000 },
			maxDelayMs: 9000,
		},
		hideThinkingBlock: true,
		showCacheMissNotices: true,
		externalEditor: "vim",
		shellPath: "/bin/bash",
		quietStartup: "header",
		defaultProjectTrust: "always",
		shellCommandPrefix: "docker exec",
		npmCommand: ["mise", "exec", "--", "npm"],
		collapseChangelog: true,
		enableInstallTelemetry: false,
		enableAnalytics: true,
		trackingId: "abc",
		packages: ["some-pkg", { source: "other-pkg", autoload: false, extensions: ["a"], skills: ["b"], prompts: ["c"], themes: ["d"] }],
		extensions: ["/path/to/ext"],
		deviceId: "uuid",
		skills: ["/skills/dir"],
		prompts: ["/prompts"],
		themes: ["/themes"],
		enableSkillCommands: false,
		terminal: {
			showImages: false,
			imageWidthCells: 80,
			clearOnShrink: true,
			showTerminalProgress: true,
			hyperlinks: "auto",
			images: "kitty",
			trueColor: false,
		},
		images: { autoResize: false, blockImages: true },
		enabledModels: ["anthropic/*"],
		defaultTools: ["+read", "-bash"],
		doubleEscapeAction: "fork",
		treeFilterMode: "no-tools",
		thinkingBudgets: { minimal: 1024, low: 2048, medium: 4096, high: 8192 },
		editorPaddingX: 2,
		outputPad: 1,
		autocompleteMaxVisible: 7,
		showHardwareCursor: true,
		markdown: { codeBlockIndent: "    ", mermaid: "off" },
		warnings: { anthropicExtraUsage: false },
		codemode: { mode: "only", inlineBudget: 1000 },
		sessionDir: "/sessions",
		httpProxy: "http://proxy.example",
		httpIdleTimeoutMs: "disabled",
		websocketConnectTimeoutMs: 0,
		tuiMode: "regular",
		fullscreenExitOutput: "resume-hint",
		fullscreenScrollbar: "always",
		fullscreenCopyOnSelect: false,
		fullscreenWheelScrollLines: 3,
		queueMode: "all",
		websockets: true,
	}));
	__cases.push(ss("valid-defaults-object", SETTINGS_DEFAULTS));

	// root type violations
	__cases.push(ss("root-array", []));
	__cases.push(ss("root-string", "nope"));
	__cases.push(ss("root-number", 42));
	__cases.push(ss("root-null", null));
	__cases.push(ss("root-boolean", true));

	// unknown keys are permissive (additionalProperties: true)
	__cases.push(ss("unknown-keys-ok", { totallyUnknown: 123, nested: { deep: true } }));
	__cases.push(ss("unknown-in-compaction", { compaction: { bogus: "x", enabled: true } }));

	// top-level type/enum violations
	__cases.push(ss("transport-wrong-type", { transport: 42 }));
	__cases.push(ss("transport-bad-literal", { transport: "nope" }));
	__cases.push(ss("session-share-bad", { sessionShareDestination: "github" }));
	__cases.push(ss("steering-mode-bad", { steeringMode: "one" }));
	__cases.push(ss("follow-up-mode-bad", { followUpMode: [] }));
	__cases.push(ss("default-project-trust-bad", { defaultProjectTrust: "maybe" }));
	__cases.push(ss("quiet-startup-bad", { quietStartup: "loud" }));
	__cases.push(ss("double-escape-bad", { doubleEscapeAction: "jump" }));
	__cases.push(ss("tree-filter-bad", { treeFilterMode: "some" }));
	__cases.push(ss("tui-mode-bad", { tuiMode: "windowed" }));
	__cases.push(ss("fullscreen-exit-bad", { fullscreenExitOutput: "nothing" }));
	__cases.push(ss("fullscreen-scrollbar-bad", { fullscreenScrollbar: "sometimes" }));
	__cases.push(ss("fullscreen-wheel-bad", { fullscreenWheelScrollLines: "fast" }));
	__cases.push(ss("fullscreen-wheel-null", { fullscreenWheelScrollLines: null }));
	__cases.push(ss("queue-mode-bad", { queueMode: "sometimes" }));
	__cases.push(ss("default-thinking-bad", { defaultThinkingLevel: "ultra" }));
	__cases.push(ss("model-thinking-levels-bad-value", { modelThinkingLevels: { "a/b": "ultra" } }));
	__cases.push(ss("model-thinking-levels-not-object", { modelThinkingLevels: "nope" }));
	__cases.push(ss("schema-not-string", { $schema: 5 }));
	__cases.push(ss("last-changelog-not-string", { lastChangelogVersion: 1.2 }));
	__cases.push(ss("npm-command-not-array", { npmCommand: "npm" }));
	__cases.push(ss("npm-command-bad-entry", { npmCommand: ["mise", 5] }));
	__cases.push(ss("enabled-models-not-array", { enabledModels: "anthropic/*" }));
	__cases.push(ss("default-tools-bad-entry", { defaultTools: ["+read", 7] }));
	__cases.push(ss("http-idle-timeout-bad", { httpIdleTimeoutMs: "nope" }));
	__cases.push(ss("http-idle-timeout-negative", { httpIdleTimeoutMs: -1 }));
	__cases.push(ss("websocket-connect-timeout-bad", { websocketConnectTimeoutMs: true }));

	// nested: compaction
	__cases.push(ss("compaction-not-object", { compaction: "nope" }));
	__cases.push(ss("compaction-enabled-bad", { compaction: { enabled: "yes" } }));
	__cases.push(ss("compaction-reserve-negative", { compaction: { reserveTokens: -1 } }));
	__cases.push(ss("compaction-reserve-fractional", { compaction: { reserveTokens: 1.5 } }));
	__cases.push(ss("compaction-reserve-string", { compaction: { reserveTokens: "5" } }));
	__cases.push(ss("compaction-keep-recent-null", { compaction: { keepRecentTokens: null } }));
	__cases.push(ss("compaction-overrides-bad-value", {
		compaction: { modelOverrides: { "prov/model": { reserveTokens: -5 } } },
	}));
	__cases.push(ss("compaction-overrides-not-object", {
		compaction: { modelOverrides: { "prov/model": "nope" } },
	}));
	__cases.push(ss("compaction-overrides-not-record", { compaction: { modelOverrides: [1, 2] } }));

	// nested: branchSummary, retry
	__cases.push(ss("branch-summary-skip-prompt-bad", { branchSummary: { skipPrompt: 1 } }));
	__cases.push(ss("branch-summary-reserve-bad", { branchSummary: { reserveTokens: "2048" } }));
	__cases.push(ss("retry-max-retries-bad", { retry: { maxRetries: "3" } }));
	__cases.push(ss("retry-provider-not-object", { retry: { provider: [] } }));
	__cases.push(ss("retry-provider-max-delay-negative", { retry: { provider: { maxRetryDelayMs: -5 } } }));
	__cases.push(ss("retry-max-delay-bad", { retry: { maxDelayMs: true } }));

	// nested: terminal, images, markdown, warnings, codemode
	__cases.push(ss("terminal-show-images-bad", { terminal: { showImages: "true" } }));
	__cases.push(ss("terminal-image-width-bad", { terminal: { imageWidthCells: "wide" } }));
	__cases.push(ss("terminal-images-bad-literal", { terminal: { images: "foo" } }));
	__cases.push(ss("terminal-images-number", { terminal: { images: 0 } }));
	__cases.push(ss("terminal-hyperlinks-bad", { terminal: { hyperlinks: "nope" } }));
	__cases.push(ss("terminal-true-color-bad", { terminal: { trueColor: 1 } }));
	__cases.push(ss("images-auto-resize-bad", { images: { autoResize: "yes" } }));
	__cases.push(ss("images-not-object", { images: ["kitty"] }));
	__cases.push(ss("markdown-mermaid-bad", { markdown: { mermaid: "sometimes" } }));
	__cases.push(ss("markdown-indent-bad", { markdown: { codeBlockIndent: 4 } }));
	__cases.push(ss("warnings-usage-bad", { warnings: { anthropicExtraUsage: "no" } }));
	__cases.push(ss("codemode-mode-bad", { codemode: { mode: "off" } }));
	__cases.push(ss("codemode-inline-budget-negative", { codemode: { inlineBudget: -1 } }));

	// nested: thinkingBudgets, outputPad, packages, skills
	__cases.push(ss("thinking-budgets-bad", { thinkingBudgets: { low: "5" } }));
	__cases.push(ss("thinking-budgets-null-entry", { thinkingBudgets: { minimal: null } }));
	__cases.push(ss("output-pad-bad-literal", { outputPad: 2 }));
	__cases.push(ss("output-pad-bad-type", { outputPad: "1" }));
	__cases.push(ss("packages-not-array", { packages: "pkg" }));
	__cases.push(ss("packages-bad-object", { packages: [{ source: 5 }] }));
	__cases.push(ss("packages-bad-autoload", { packages: [{ source: "p", autoload: "yes" }] }));
	__cases.push(ss("packages-bad-extensions-entry", { packages: [{ source: "p", extensions: ["a", 5] }] }));
	__cases.push(ss("skills-object-bad", { skills: { enableSkillCommands: "yes" } }));
	__cases.push(ss("skills-array-bad-entry", { skills: ["/dir", 5] }));
	__cases.push(ss("prompts-bad-entry", { prompts: ["/p", true] }));

	// error volume / maxErrors cap
	__cases.push(ss("cap-many-errors", {
		transport: 42,
		sessionShareDestination: 42,
		steeringMode: 42,
		followUpMode: 42,
		defaultProjectTrust: 42,
		doubleEscapeAction: 42,
		treeFilterMode: 42,
		tuiMode: 42,
	}));

	return __cases;
})(),

// settings-manager: script-driven SettingsManager parity (memory / files /
// inMemory modes; migrated loads, persisted bytes, trust, errors, uuids).
...(() => {
	const smc = (name, args) => ({ tool: "settings-manager", name: "sm-" + name, args });
	const J = JSON.stringify;
	const mem = (name, o) => smc(name, {
		mode: "memory",
		storage: { global: o.g === undefined ? null : o.g, project: o.p === undefined ? null : o.p },
		options: o.opt ?? {},
		ops: o.ops,
	});
	const files = (name, o) => smc(name, {
		mode: "files",
		storage: { global: o.g === undefined ? null : o.g, project: o.p === undefined ? null : o.p },
		options: o.opt ?? {},
		ops: o.ops,
	});
	const get = (m) => ({ op: "get", method: m });
	const set = (m, ...a) => ({ op: "set", method: m, args: a });
	const c = [];

	// --- defaults / basic getters ---
	c.push(mem("empty", { ops: [get("getSettings"), get("getGlobalSettings"), get("getProjectSettings"), get("isProjectTrusted"), { op: "storage" }] }));
	c.push(mem("defaults", { ops: [
		get("getSteeringMode"), get("getFollowUpMode"), get("getTransport"), get("getTheme"),
		get("getThemeSetting"), get("getDefaultProvider"), get("getDefaultModel"), get("getSessionDir"),
		get("getDefaultThinkingLevel"), get("getAllModelThinkingLevels"), get("getQuietStartup"),
		get("getDefaultProjectTrust"), get("getShellCommandPrefix"), get("getNpmCommand"),
		get("getEditorPaddingX"), get("getOutputPad"), get("getAutocompleteMaxVisible"),
		get("getCodeBlockIndent"), get("getMermaidRenderingMode"), get("getWarnings"),
		get("getDoubleEscapeAction"), get("getTreeFilterMode"), get("getTuiMode"),
		get("getFullscreenExitOutput"), get("getFullscreenScrollbar"), get("getFullscreenCopyOnSelect"),
		get("getFullscreenWheelScrollLines"), get("getCompactionEnabled"), get("getRetrySettings"),
		get("getProviderRetrySettings"), get("getHttpIdleTimeoutMs"), get("getWebSocketConnectTimeoutMs"),
		get("getBranchSummarySettings"), get("getCacheWarmingMode"), get("getDefaultTools"),
		get("getTerminalCapabilityOverrides"), get("getThinkingBudgets"), get("getEnabledModels"),
	]}));

	// --- deep merge ---
	c.push(mem("merge-basic", { g: J({ a: 1, theme: "dark", nested: { x: 1, y: 2 } }), p: J({ b: 2, theme: "light", nested: { y: 3 } }),
		ops: [get("getSettings"), get("getGlobalSettings"), get("getProjectSettings")] }));
	c.push(mem("merge-arrays-replace", { g: J({ packages: ["a", "b"], terminal: { images: "kitty" } }), p: J({ packages: ["c"] }),
		ops: [get("getPackages"), get("getTerminalCapabilityOverrides")] }));
	c.push(mem("merge-null-override", { g: J({ theme: "dark" }), p: J({ theme: null }), ops: [get("getThemeSetting")] }));
	c.push(mem("merge-default-tools", {
		g: J({ defaultTools: ["read", "bash"] }), p: J({ defaultTools: ["+edit", "-bash"] }),
		ops: [get("getDefaultTools"), get("getSettings")] }));
	c.push(mem("default-tools-plain-replace", { g: J({ defaultTools: ["read"] }), p: J({ defaultTools: ["grep", "find"] }), ops: [get("getDefaultTools")] }));
	c.push(mem("default-tools-modifiers-only", { g: J({ defaultTools: ["+web", "-edit"] }), ops: [get("getDefaultTools")] }));
	c.push(mem("default-tools-empty", { g: J({ defaultTools: [] }), ops: [get("getDefaultTools")] }));
	c.push(mem("default-tools-nonstring-filtered", { g: J({ defaultTools: [5, "grep"] }), ops: [get("getDefaultTools")] }));
	c.push(mem("default-tools-dedupe", { g: J({ defaultTools: ["+read", "+read", "-bash", "-bash"] }), ops: [get("getDefaultTools")] }));

	// --- migrations ---
	c.push(mem("migrate-queue-mode", { g: J({ queueMode: "all", keep: 1 }), ops: [get("getSettings"), get("getSteeringMode")] }));
	c.push(mem("migrate-queue-mode-both", { g: J({ queueMode: "all", steeringMode: "top" }), ops: [get("getSettings")] }));
	c.push(mem("migrate-websockets-true", { g: J({ websockets: true }), ops: [get("getTransport")] }));
	c.push(mem("migrate-websockets-false", { g: J({ websockets: false, x: 1 }), ops: [get("getTransport"), get("getSettings")] }));
	c.push(mem("migrate-websockets-transport-set", { g: J({ websockets: true, transport: "sse" }), ops: [get("getSettings")] }));
	c.push(mem("migrate-websockets-nonbool", { g: J({ websockets: "yes" }), ops: [get("getSettings")] }));
	c.push(mem("migrate-skills-hoist", { g: J({ skills: { enableSkillCommands: true } }), ops: [get("getSettings"), get("getEnableSkillCommands"), get("getSkillPaths")] }));
	c.push(mem("migrate-skills-custom-dirs", { g: J({ skills: { enableSkillCommands: false, customDirectories: ["/a", "/b"] } }), ops: [get("getSkillPaths"), get("getSettings")] }));
	c.push(mem("migrate-skills-empty-dirs", { g: J({ skills: { customDirectories: [] }, enableSkillCommands: true }), ops: [get("getSettings")] }));
	c.push(mem("migrate-retry-max-delay", { g: J({ retry: { maxDelayMs: 9000, enabled: false } }), ops: [get("getProviderRetrySettings"), get("getSettings")] }));
	c.push(mem("migrate-retry-provider-exists", { g: J({ retry: { maxDelayMs: 9000, provider: { timeoutMs: 5, maxRetryDelayMs: 7 } } }), ops: [get("getProviderRetrySettings")] }));
	c.push(mem("migrate-retry-non-number", { g: J({ retry: { maxDelayMs: "x" } }), ops: [get("getSettings")] }));

	// --- load errors (V8 JSON.parse messages; avoid the two port gaps) ---
	for (const [n, content] of [["truncated", "{"], ["trailing", "[1]x"], ["bad-token", "nope"], ["whitespace", " "], ["empty-string", ""], ["missing-colon", '{"a" 1}'], ["array-comma", "[1,"], ["non-object-number", "42"], ["non-object-null", "null"], ["non-object-bool", "true"], ["non-object-string", '"hello"']]) {
		c.push(mem("load-error-global-" + n, { g: content, ops: [{ op: "drainErrors" }, get("getSettings"), get("getGlobalSettings")] }));
	}
	c.push(mem("load-error-project", { g: J({ a: 1 }), p: "{oops", ops: [{ op: "drainErrors" }, get("getSettings")] }));
	c.push(mem("load-error-both", { g: "{", p: "[", ops: [{ op: "drainErrors" }] }));
	c.push(mem("load-error-then-drain-empty", { g: "{", ops: [{ op: "drainErrors" }, { op: "drainErrors" }] }));
	c.push(mem("load-error-save-short-circuit", { g: "{", ops: [set("setTheme", "red"), { op: "flush" }, { op: "storage" }, { op: "drainErrors" }] }));
	c.push(files("load-error-paths", { g: "{", p: "{", ops: [{ op: "drainErrors" }] }));
	c.push(mem("bom-content", { g: "﻿" + J({ theme: "dark" }), ops: [get("getThemeSetting")] }));

	// --- persist semantics (byte-level strings) ---
	c.push(mem("persist-key-order", { g: J({ b: 1, a: 2 }), ops: [set("setTheme", "x"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("persist-set-existing-keeps-position", { g: J({ theme: "a", first: 1 }), ops: [set("setTheme", "b"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("persist-no-flush-unchanged", { g: J({ theme: "a" }), ops: [set("setTheme", "b"), { op: "storage" }] }));
	c.push(mem("persist-batched-sets", { ops: [set("setTheme", "a"), set("setTransport", "websocket"), set("setSteeringMode", "top"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("persist-nested-partial-merge", { g: J({ terminal: { showImages: false, imageWidthCells: 80 } }), ops: [set("setShowImages", true), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("persist-migrates-current-file", { g: J({ queueMode: "all", keep: 1 }), ops: [set("setTheme", "x"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("persist-clear-field", { g: J({ shellPath: "/bin/zsh", theme: "a" }), ops: [set("setShellPath", null), { op: "flush" }, { op: "storage" }, get("getShellPath")] }));
	c.push(mem("flush-empty-queue", { g: J({ a: 1 }), ops: [{ op: "flush" }, { op: "storage" }] }));
	c.push(mem("reload-pending-write-first", { ops: [set("setTheme", "z"), { op: "reload" }, get("getThemeSetting"), { op: "storage" }] }));
	c.push(mem("reload-external-edit", { g: J({ theme: "a" }), ops: [get("getThemeSetting"), { op: "external", args: [{ scope: "global", content: J({ theme: "b", extra: 2 }) }] }, { op: "reload" }, get("getThemeSetting"), get("getSettings")] }));
	c.push(mem("reload-external-invalid", { g: J({ theme: "a" }), ops: [{ op: "external", args: [{ scope: "global", content: "{" }] }, { op: "reload" }, get("getThemeSetting"), { op: "drainErrors" }] }));
	c.push(mem("reload-external-delete", { g: J({ theme: "a" }), ops: [{ op: "external", args: [{ scope: "global", content: null }] }, { op: "reload" }, get("getThemeSetting"), { op: "storage" }] }));
	c.push(mem("external-edit-then-set", { g: J({ theme: "dark" }), ops: [
		{ op: "external", args: [{ scope: "global", content: J({ theme: "blue", extra: 1 }) }] },
		set("setTheme", "red"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("external-invalid-then-flush", { g: J({ theme: "a" }), ops: [
		{ op: "external", args: [{ scope: "global", content: '{"a":}' }] },
		set("setTheme", "b"), { op: "flush" }, { op: "storage" }, { op: "drainErrors" }] }));

	// --- files mode: FileSettingsStorage ---
	c.push(files("create-dirs-on-write", { ops: [set("setTheme", "x"), { op: "flush" }, { op: "storage" }] }));
	c.push(files("project-write", { ops: [set("setProjectPackages", ["pkg-a"]), { op: "flush" }, { op: "storage" }, get("getPackages")] }));
	c.push(files("project-load", { g: J({ theme: "g" }), p: J({ theme: "p" }), ops: [get("getSettings"), get("getProjectSettings")] }));
	c.push(files("project-partial-merge", { p: J({ packages: ["one"], theme: "p" }), ops: [set("setProjectPackages", ["two"]), { op: "flush" }, { op: "storage" }] }));
	c.push(files("bom-file", { g: "﻿" + J({ theme: "bom" }), ops: [get("getThemeSetting")] }));

	// --- project trust ---
	c.push(mem("untrusted-skip-load", { p: J({ theme: "p" }), opt: { projectTrusted: false }, ops: [get("isProjectTrusted"), get("getSettings"), { op: "drainErrors" }] }));
	c.push(mem("untrusted-write-throws", { opt: { projectTrusted: false }, ops: [set("setProjectPackages", ["x"]), { op: "drainErrors" }] }));
	c.push(mem("untrusted-queued-write-error", { p: J({ theme: "p" }), ops: [
		set("setProjectPackages", ["x"]), { op: "setProjectTrusted", args: [false] }, { op: "flush" },
		{ op: "storage" }, { op: "drainErrors" }] }));
	c.push(mem("trust-transitions", { p: J({ theme: "p" }), opt: { projectTrusted: false }, ops: [
		get("getSettings"), { op: "setProjectTrusted", args: [true] }, get("getSettings"),
		{ op: "setProjectTrusted", args: [true] }, { op: "setProjectTrusted", args: [false] }, get("getSettings")] }));
	c.push(mem("trust-load-error-surface", { p: "{", opt: { projectTrusted: false }, ops: [
		{ op: "drainErrors" }, { op: "setProjectTrusted", args: [true] }, { op: "drainErrors" }] }));

	// --- clamps / env / editor ---
	c.push(mem("clamp-editor-padding", { ops: [set("setEditorPaddingX", 2.9), get("getEditorPaddingX"), set("setEditorPaddingX", -5), get("getEditorPaddingX")] }));
	c.push(mem("clamp-autocomplete", { ops: [set("setAutocompleteMaxVisible", 25), get("getAutocompleteMaxVisible"), set("setAutocompleteMaxVisible", 1), get("getAutocompleteMaxVisible")] }));
	c.push(mem("output-pad-values", { ops: [set("setOutputPad", 0), get("getOutputPad"), set("setOutputPad", 5), get("getOutputPad"), set("setOutputPad", 0.5), get("getOutputPad"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("image-width-clamp", { g: J({ terminal: { imageWidthCells: 0.9 } }), ops: [get("getImageWidthCells"), set("setImageWidthCells", 12.7), get("getImageWidthCells")] }));
	c.push(mem("wheel-scroll-lines", { ops: [set("setFullscreenWheelScrollLines", "auto"), get("getFullscreenWheelScrollLines"), set("setFullscreenWheelScrollLines", 500), get("getFullscreenWheelScrollLines"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("editor-env-visual", { opt: { env: { VISUAL: "emacs", EDITOR: "vi" } }, ops: [get("getExternalEditorCommand")] }));
	c.push(mem("editor-env-editor", { opt: { env: { EDITOR: "vi" } }, ops: [get("getExternalEditorCommand")] }));
	c.push(mem("editor-env-none", { opt: { env: {} }, ops: [get("getExternalEditorCommand")] }));
	c.push(mem("editor-configured", { g: J({ externalEditor: "  " }), opt: { env: { EDITOR: "vi" } }, ops: [get("getExternalEditorCommand")] }));
	c.push(mem("editor-win32", { opt: { platform: "win32", env: {} }, ops: [get("getExternalEditorCommand")] }));
	c.push(mem("env-clear-on-shrink", { opt: { env: { PI_CLEAR_ON_SHRINK: "1" } }, ops: [get("getClearOnShrink")] }));
	c.push(mem("env-hardware-cursor", { opt: { env: { PI_HARDWARE_CURSOR: "1" } }, ops: [get("getShowHardwareCursor")] }));
	c.push(mem("session-dir-tilde", { g: J({ sessionDir: "~/sessions" }), opt: { env: { HOME: "/home/parity" } }, ops: [get("getSessionDir")] }));

	// --- httpIdleTimeoutMs ---
	for (const [n, v] of [["object", {}], ["string", "abc"], ["bool", true], ["array", [1, 2]], ["negative", -1]]) {
		c.push(mem("http-idle-invalid-" + n, { g: J({ httpIdleTimeoutMs: v }), ops: [get("getHttpIdleTimeoutMs")] }));
	}
	c.push(mem("http-idle-disabled", { g: J({ httpIdleTimeoutMs: "disabled" }), ops: [get("getHttpIdleTimeoutMs")] }));
	c.push(mem("http-idle-float", { g: J({ httpIdleTimeoutMs: 5000.7 }), ops: [get("getHttpIdleTimeoutMs")] }));
	c.push(mem("http-idle-set-invalid", { ops: [set("setHttpIdleTimeoutMs", -1), { op: "storage" }] }));
	c.push(mem("http-idle-set-float", { ops: [set("setHttpIdleTimeoutMs", 100.9), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("ws-connect-timeout", { g: J({ websocketConnectTimeoutMs: 2500 }), ops: [get("getWebSocketConnectTimeoutMs")] }));

	// --- analytics uuids ---
	c.push(mem("analytics-uuids", { opt: { uuids: ["uuid-a", "uuid-b"] }, ops: [
		set("setEnableAnalytics", true), get("getTrackingId"), set("setEnableAnalytics", false),
		get("getTrackingId"), set("setEnableAnalytics", true), get("getTrackingId"),
		get("getOrCreateDeviceId"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("uuid-exhaustion", { opt: { uuids: ["only"] }, ops: [get("getOrCreateDeviceId"), set("setEnableAnalytics", true), get("getTrackingId")] }));
	c.push(mem("device-id-persists", { opt: { uuids: ["dev-1"] }, ops: [get("getOrCreateDeviceId"), get("getOrCreateDeviceId"), { op: "flush" }, { op: "storage" }] }));

	// --- compaction ---
	c.push(mem("compaction-overrides", { g: J({ compaction: { reserveTokens: 100, modelOverrides: { "a/b": { reserveTokens: 50 } } } }), ops: [
		get("getCompactionReserveTokens"), get("getCompactionReserveTokens", { provider: "a", id: "b" }),
		get("getCompactionKeepRecentTokens", { provider: "a", id: "b" }),
		get("getCompactionSettings", { provider: "a", id: "b" })] }));
	for (const [n, v] of [["negative", -1], ["fraction", 1.5], ["string", "x"]]) {
		c.push(mem("compaction-invalid-" + n, { g: J({ compaction: { reserveTokens: v } }), ops: [get("getCompactionReserveTokens")] }));
	}
	c.push(mem("compaction-invalid-override-entry", { g: J({ compaction: { modelOverrides: { "a/b": 5 } } }), ops: [get("getCompactionReserveTokens", { provider: "a", id: "b" })] }));
	c.push(mem("compaction-invalid-override-field", { g: J({ compaction: { modelOverrides: { "a/b": { keepRecentTokens: -3 } } } }), ops: [get("getCompactionKeepRecentTokens", { provider: "a", id: "b" })] }));

	// --- misc setters/getters ---
	c.push(mem("thinking-levels", { ops: [
		set("setModelThinkingLevel", "anthropic", "m1", "high"), get("getModelThinkingLevel", "anthropic", "m1"),
		get("getAllModelThinkingLevels"), set("removeModelThinkingLevel", "anthropic", "m1"),
		get("getAllModelThinkingLevels"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("quiet-startup-values", { g: J({ quietStartup: "header" }), ops: [get("getQuietStartup"), set("setQuietStartup", true), get("getQuietStartup"), set("setQuietStartup", "loud"), get("getQuietStartup")] }));
	c.push(mem("npm-command", { ops: [set("setNpmCommand", ["mise", "exec"]), get("getNpmCommand"), set("setNpmCommand", null), get("getNpmCommand"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("enabled-models", { ops: [set("setEnabledModels", ["a/*"]), get("getEnabledModels"), set("setEnabledModels", null), get("getEnabledModels"), { op: "flush" }, { op: "storage" }] }));
	c.push(mem("warnings-copy", { g: J({ warnings: { a: true } }), ops: [get("getWarnings"), set("setWarnings", { b: false }), get("getWarnings"), get("getWarnings"), set("setWarnings", "not-an-object"), get("getWarnings")] }));
	c.push(mem("cache-warming-global-only", { g: J({ cacheWarming: "idle" }), p: J({ cacheWarming: "streaming" }), ops: [get("getCacheWarmingMode")] }));
	c.push(mem("theme-slash-filtered", { g: J({ theme: "custom/dark" }), ops: [get("getThemeSetting"), get("getTheme")] }));
	c.push(mem("terminal-capabilities", { g: J({ terminal: { images: false, trueColor: true, hyperlinks: "x" } }), ops: [get("getTerminalCapabilityOverrides")] }));

	// --- inMemory factory + overrides ---
	c.push(smc("in-memory-factory", { mode: "inMemory", settings: { queueMode: "all", theme: "x" }, ops: [get("getSettings"), get("getSteeringMode"), { op: "storage" }] }));
	c.push(smc("in-memory-empty", { mode: "inMemory", settings: {}, ops: [get("getSettings"), set("setTheme", "y"), { op: "flush" }, get("getThemeSetting")] }));
	c.push(mem("apply-overrides", { g: J({ theme: "a", nested: { x: 1 } }), ops: [
		{ op: "applyOverrides", args: [{ theme: "b", nested: { y: 2 }, defaultTools: ["+edit"] }] },
		get("getSettings"), get("getDefaultTools")] }));

	// --- pure functions ---
	c.push(smc("pure-apply-modifiers", { mode: "memory", ops: [{ op: "pure", fn: "applyToolModifiers", args: [["read", "bash"], ["+edit", "-bash", "+read"]] }] }));
	c.push(smc("pure-tool-list-error-mixed", { mode: "memory", ops: [{ op: "pure", fn: "getToolListError", args: [["read", "+edit"]] }] }));
	c.push(smc("pure-tool-list-error-pattern", { mode: "memory", ops: [{ op: "pure", fn: "getToolListError", args: [["+rea*", "-bash"]] }] }));
	c.push(smc("pure-tool-list-error-ok", { mode: "memory", ops: [{ op: "pure", fn: "getToolListError", args: [["read", "bash"]] }] }));
	c.push(smc("pure-is-modifier", { mode: "memory", ops: [{ op: "pure", fn: "isToolModifier", args: ["+x"] }, { op: "pure", fn: "isToolModifier", args: ["x"] }, { op: "pure", fn: "isToolModifier", args: [5] }] }));
	c.push(smc("pure-default-tool-names", { mode: "memory", ops: [{ op: "pure", fn: "DEFAULT_TOOL_NAMES" }] }));

	// --- protocol misuse (identical in-band errors) ---
	c.push(mem("unknown-method", { ops: [{ op: "get", method: "bogus" }] }));
	c.push(mem("unknown-op", { ops: [{ op: "bogus" }] }));
	c.push(mem("unknown-pure-fn", { ops: [{ op: "pure", fn: "bogus" }] }));
	c.push(mem("arg-type-error", { ops: [{ op: "set", method: "setTheme", args: [5] }] }));
	c.push(mem("arg-count-error", { ops: [{ op: "get", method: "getTheme", args: ["a"] }] }));
	c.push(smc("missing-mode", { ops: [] }));
	c.push(smc("unknown-mode", { mode: "bogus", ops: [] }));
	c.push(smc("missing-ops", { mode: "memory" }));

	return c.filter(Boolean);
})(),

// prompt-templates (core-module wave). Pure string ops plus `load` cases
// with per-case temp fixtures (`files` maps materialized under `<base>`;
// option paths are relative roots joined by the harness, `promptPaths`
// entries resolve against cwd like the module itself).
...(() => {
	const pt = (name, args) => ({ tool: "prompt-templates", name: "pt-" + name, args });
	const c = [];
	const sub = (name, content, args) => c.push(pt("sub-" + name, { op: "substituteArgs", content, args: args ?? [] }));
	const load = (name, files, options) => c.push(pt("load-" + name, { op: "load", files, options }));

	// --- parseCommandArgs ---
	c.push(pt("parse-empty", { op: "parseCommandArgs", argsString: "" }));
	c.push(pt("parse-simple", { op: "parseCommandArgs", argsString: "a b c" }));
	c.push(pt("parse-dquote", { op: "parseCommandArgs", argsString: 'a "b c" d' }));
	c.push(pt("parse-squote", { op: "parseCommandArgs", argsString: "a 'b c' d" }));
	c.push(pt("parse-empty-quotes", { op: "parseCommandArgs", argsString: "'' \"\"" }));
	c.push(pt("parse-unterminated", { op: "parseCommandArgs", argsString: 'a "bc' }));
	c.push(pt("parse-quote-in-word", { op: "parseCommandArgs", argsString: 'a"b c"d' }));
	c.push(pt("parse-mixed-quotes", { op: "parseCommandArgs", argsString: `"it's" 'say "hi"'` }));
	c.push(pt("parse-nbsp", { op: "parseCommandArgs", argsString: "a\u00a0b" }));
	c.push(pt("parse-bom-lsep", { op: "parseCommandArgs", argsString: "a\ufeffb\u2028c" }));

	// --- substituteArgs ---
	sub("positional", "a $1 b $2 c", ["x", "y"]);
	sub("missing", "[$1][$5]", ["x"]);
	sub("all-args", "[$@][$ARGUMENTS]", ["a", "b", "c"]);
	sub("default-missing", "${1:-fallback}", []);
	sub("default-present", "${1:-fallback}", ["value"]);
	sub("default-empty", "${1:-fallback}", [""]);
	sub("default-all", "[${@:-none}][${ARGUMENTS:-none}]", []);
	sub("slice", "[${@:2}]", ["a", "b", "c"]);
	sub("slice-len", "[${@:2:1}]", ["a", "b", "c"]);
	sub("slice-zero", "[${@:0}]", ["a", "b"]);
	sub("slice-beyond", "[${@:99}]", ["a"]);
	sub("slice-bad", "[${@:1:2:3}][${@:x}]", ["a", "b"]);
	sub("adjacent", "$1$2 $$1 x$1y", ["a", "b"]);
	sub("no-resubstitute", "$1", ["$2"]);
	sub("huge-index", "[$99999999999999999999]", ["a"]);
	sub("ten", "${10:-d}", ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);

	// --- expandPromptTemplate ---
	c.push(pt("expand-plain", { op: "expandPromptTemplate", text: "hello world", templates: [] }));
	c.push(pt("expand-exact", { op: "expandPromptTemplate", text: "/greet", templates: [{ name: "greet", content: "Hello $1" }] }));
	c.push(pt("expand-args", { op: "expandPromptTemplate", text: "/greet world and moon", templates: [{ name: "greet", content: "Hello $1 and $2" }] }));
	c.push(pt("expand-unknown", { op: "expandPromptTemplate", text: "/nope args here", templates: [{ name: "greet", content: "x" }] }));
	c.push(pt("expand-slash-only", { op: "expandPromptTemplate", text: "/", templates: [{ name: "greet", content: "x" }] }));
	c.push(pt("expand-space-after-slash", { op: "expandPromptTemplate", text: "/ greet", templates: [{ name: "greet", content: "x" }] }));
	c.push(pt("expand-tabs", { op: "expandPromptTemplate", text: "/greet\ta\tb", templates: [{ name: "greet", content: "[$1][$2][$@]" }] }));
	c.push(pt("expand-newline-args", { op: "expandPromptTemplate", text: "/greet a\nb", templates: [{ name: "greet", content: "[$1][$2]" }] }));
	c.push(pt("expand-first-match", { op: "expandPromptTemplate", text: "/dup", templates: [{ name: "dup", content: "first" }, { name: "dup", content: "second" }] }));
	c.push(pt("expand-quoted-args", { op: "expandPromptTemplate", text: '/greet "a b" c', templates: [{ name: "greet", content: "[$1][$2][$@]" }] }));
	c.push(pt("expand-unicode-name", { op: "expandPromptTemplate", text: "/café x", templates: [{ name: "café", content: "Hola $1" }] }));

	// --- load: defaults ---
	load("defaults-global", { "agent/prompts/greet.md": "---\ndescription: Greets\n---\nHello $1\n" }, { agentDir: "agent", cwd: "proj", includeDefaults: true });
	load("defaults-project", { "proj/.pi/prompts/note.md": "---\ndescription: Note\n---\nBody\n" }, { agentDir: "agent", cwd: "proj", includeDefaults: true });
	load("defaults-both", {
		"agent/prompts/global-tpl.md": "---\ndescription: G\n---\nG body\n",
		"proj/.pi/prompts/proj-tpl.md": "---\ndescription: P\n---\nP body\n",
	}, { agentDir: "agent", cwd: "proj", includeDefaults: true });
	load("defaults-and-path", {
		"agent/prompts/a.md": "---\ndescription: A\n---\nA\n",
		"extra/b.md": "---\ndescription: B\n---\nB\n",
	}, { agentDir: "agent", cwd: "proj", includeDefaults: true, promptPaths: ["../extra"] });

	// --- load: explicit prompt paths ---
	load("path-dir", { "tpls/a.md": "---\ndescription: A\n---\nA body\n" }, { cwd: "proj", promptPaths: ["../tpls"] });
	load("path-dir-cwd-root", { "proj/a.md": "---\ndescription: A\n---\nA\n" }, { cwd: "proj", promptPaths: ["."] });
	load("path-file", { "one.md": "---\ndescription: One\n---\nOne body\n" }, { cwd: "", promptPaths: ["one.md"] });
	load("path-nonmd", { "x.txt": "hi" }, { cwd: "", promptPaths: ["x.txt"] });
	load("path-missing", {}, { cwd: "", promptPaths: ["nope"] });
	load("two-paths", {
		"d1/a.md": "---\ndescription: A\n---\nA\n",
		"d2/b.md": "---\ndescription: B\n---\nB\n",
	}, { cwd: "", promptPaths: ["d1", "d2"] });
	load("same-name-two-paths", {
		"d1/greet.md": "---\ndescription: First\n---\n1\n",
		"d2/greet.md": "---\ndescription: Second\n---\n2\n",
	}, { cwd: "", promptPaths: ["d1", "d2"] });
	load("tilde", { "home/tpls/t.md": "---\ndescription: T\n---\nT\n" }, { cwd: "", promptPaths: ["~/tpls"] });
	load("path-dir-only-subdirs", { "tpls/sub/inner.md": "---\ndescription: I\n---\nI\n" }, { cwd: "", promptPaths: ["tpls"] });

	// --- load: frontmatter variants ---
	load("fm-quoted-dq", { "t/a.md": '---\ndescription: "say \\"hi\\" now"\n---\nB\n' }, { cwd: "", promptPaths: ["t"] });
	load("fm-quoted-sq", { "t/a.md": "---\ndescription: 'it''s here'\n---\nB\n" }, { cwd: "", promptPaths: ["t"] });
	load("fm-comment", { "t/a.md": "---\ndescription: value # comment\n---\nB\n" }, { cwd: "", promptPaths: ["t"] });
	load("fm-number-desc", { "t/a.md": "---\ndescription: 42\n---\nFirst line body\n---\n" }, { cwd: "", promptPaths: ["t"] });
	load("fm-bool-desc", { "t/a.md": "---\ndescription: true\n---\nBody line\n" }, { cwd: "", promptPaths: ["t"] });
	load("fm-null-desc", { "t/a.md": "---\ndescription:\n---\nFallback line\n" }, { cwd: "", promptPaths: ["t"] });
	load("fm-empty-desc", { "t/a.md": "---\ndescription: ''\n---\nFallback line\n" }, { cwd: "", promptPaths: ["t"] });
	load("fm-arghint", { "t/a.md": '---\ndescription: D\nargument-hint: "[names...]"\n---\nB\n' }, { cwd: "", promptPaths: ["t"] });
	// Unquoted flow sequence: eemeli/yaml parses [names...] as an ARRAY, so
	// argumentHint is omitted (non-string frontmatter value) on both sides.
	load("fm-arghint-flow", { "t/a.md": '---\ndescription: D\nargument-hint: [names...]\n---\nB\n' }, { cwd: "", promptPaths: ["t"] });
	load("fm-arghint-empty", { "t/a.md": "---\ndescription: D\nargument-hint: ''\n---\nB\n" }, { cwd: "", promptPaths: ["t"] });

	// --- load: frontmatter error shapes (yaml messages, byte-exact) ---
	load("err-unterminated-dq", { "t/a.md": '---\ndescription: "abc\n---\nbody\n' }, { cwd: "", promptPaths: ["t"] });
	load("err-unterminated-sq", { "t/a.md": "---\ndescription: 'abc\n---\nbody\n" }, { cwd: "", promptPaths: ["t"] });
	load("err-nested-mapping", { "t/a.md": "---\ndescription: a: b\n---\nbody\n" }, { cwd: "", promptPaths: ["t"] });
	load("err-duplicate-keys", { "t/a.md": "---\nname: a\nname: b\ndescription: D\n---\nbody\n" }, { cwd: "", promptPaths: ["t"] });

	// --- load: description fallback from body ---
	load("no-fm-first-line", { "t/a.md": "Hello world this is a template body\nsecond line\n" }, { cwd: "", promptPaths: ["t"] });
	load("long-first-line", { "t/a.md": "x".repeat(70) + "\nrest\n" }, { cwd: "", promptPaths: ["t"] });
	load("first-line-61", { "t/a.md": "y".repeat(61) + "\nrest\n" }, { cwd: "", promptPaths: ["t"] });
	load("body-trim", { "t/a.md": "---\ndescription: D\n---\n\n\n  Body  \n\n" }, { cwd: "", promptPaths: ["t"] });
	load("bom-crlf", { "t/a.md": "\uFEFF---\r\ndescription: D\r\n---\r\nBody\r\n" }, { cwd: "", promptPaths: ["t"] });
	load("unicode", { "t/a.md": "---\ndescription: café ünïcode\n---\nCörps ✓\n" }, { cwd: "", promptPaths: ["t"] });

	// --- load: directory scan rules ---
	load("nonmd-ignored", {
		"t/a.txt": "x",
		"t/b.markdown": "y",
		"t/c.MD": "z",
	}, { cwd: "", promptPaths: ["t"] });
	load("subdir-not-scanned", { "t/sub/inner.md": "---\ndescription: I\n---\nI\n" }, { cwd: "", promptPaths: ["t"] });
	load("dotfile-md-loaded", { "t/.hidden.md": "---\ndescription: H\n---\nH\n" }, { cwd: "", promptPaths: ["t"] });
	load("symlink-broken", { "t/broken.md": { symlink: "missing.md" } }, { cwd: "", promptPaths: ["t"] });
	load("symlink-file", {
		"real.md": "---\ndescription: Real\n---\nR\n",
		"t/link.md": { symlink: "../real.md" },
	}, { cwd: "", promptPaths: ["t"] });

	// --- protocol misuse (identical in-band errors) ---
	c.push(pt("err-missing-op", {}));
	c.push(pt("err-unknown-op", { op: "bogus" }));
	c.push(pt("err-missing-args-string", { op: "parseCommandArgs" }));
	c.push(pt("err-bad-files", { op: "load", files: 5 }));
	c.push(pt("err-bad-prompt-paths", { op: "load", options: { promptPaths: "x" } }));
	c.push(pt("err-bad-include-defaults", { op: "load", options: { includeDefaults: "yes" } }));

	return c.filter(Boolean);
})(),

// skills (core-module wave). Same fixture style: `loadFromDir` scans a named
// subdir of `<base>` (never the base itself — the name fallback for root
// files is the parent dir name, which would leak the per-side basename);
// `load` exercises multi-path collisions and canonical-path dedupe;
// `formatForPrompt` drives the XML prompt formatter.
...(() => {
	const sk = (name, args) => ({ tool: "skills", name: "sk-" + name, args });
	const c = [];
	const fromDir = (name, files, source, dir) =>
		c.push(sk("dir-" + name, { op: "loadFromDir", files, source, ...(dir !== undefined ? { dir } : {}) }));
	const load = (name, files, options) => c.push(sk("load-" + name, { op: "load", files, options }));
	const SKILL = (name, desc, extra = "") => `---\n${name ? "name: " + name + "\n" : ""}description: ${desc}\n${extra}---\nbody\n`;

	// --- loadFromDir: discovery ---
	fromDir("root-skill", { "my-skill/SKILL.md": SKILL("my-skill", "Does things") }, "user", "my-skill");
	fromDir("name-fallback", { "my-skill/SKILL.md": SKILL("", "Falls back") }, "user", "my-skill");
	fromDir("nested", {
		"root/a/SKILL.md": SKILL("alpha", "A"),
		"root/b/inner/SKILL.md": SKILL("beta", "B"),
	}, "path", "root");
	fromDir("root-md-with-desc", { "root/note.md": "---\ndescription: has one\n---\nb\n" }, "path", "root");
	fromDir("root-md-no-desc", { "root/note.md": "# nope\n" }, "path", "root");
	fromDir("root-skillmd-wins", {
		"root/SKILL.md": SKILL("rooty", "root"),
		"root/sub/SKILL.md": SKILL("subby", "sub"),
		"root/plain.md": "---\ndescription: plain\n---\nb\n",
	}, "path", "root");
	fromDir("subdir-plain-md-not-loaded", { "root/sub/note.md": "---\ndescription: d\n---\nb\n" }, "path", "root");
	fromDir("missing-dir", {}, "user", "nope");
	fromDir("empty-dir", {}, "user", "root");
	fromDir("node-modules-skipped", { "root/node_modules/dep/SKILL.md": SKILL("dep", "d") }, "path", "root");
	fromDir("dot-dirs-skipped", {
		"root/.config/SKILL.md": SKILL("hidden", "h"),
		"root/.hidden.md": "---\ndescription: hm\n---\nb\n",
	}, "path", "root");

	// --- loadFromDir: ignore files ---
	fromDir("gitignore-dir", {
		"root/.gitignore": "excluded/\n",
		"root/excluded/SKILL.md": SKILL("ex", "no"),
		"root/kept/SKILL.md": SKILL("keep", "yes"),
	}, "path", "root");
	fromDir("gitignore-negation-in-ignored-dir", {
		"root/.gitignore": "excluded/\n!excluded/SKILL.md\n",
		"root/excluded/SKILL.md": SKILL("ex", "no"),
		"root/kept/SKILL.md": SKILL("keep", "yes"),
	}, "path", "root");
	fromDir("gitignore-negation", {
		"root/.gitignore": "*.tmp\n!keep.md\n",
		"root/keep.md": "---\ndescription: kept\n---\nb\n",
		"root/skip.tmp.md": "---\ndescription: skipped\n---\nb\n",
	}, "path", "root");
	fromDir("gitignore-anchored", {
		"root/.gitignore": "/*.md\n",
		"root/plain.md": "---\ndescription: p\n---\nb\n",
		"root/kept/SKILL.md": SKILL("keep", "yes"),
	}, "path", "root");
	fromDir("gitignore-comments", {
		"root/.gitignore": "# a comment\n\nskip.md\n",
		"root/skip.md": "---\ndescription: s\n---\nb\n",
		"root/keep.md": "---\ndescription: k\n---\nb\n",
	}, "path", "root");
	fromDir("nested-gitignore", {
		"root/sub/.gitignore": "ex/\n",
		"root/sub/ex/SKILL.md": SKILL("ex", "no"),
		"root/sub/ok/SKILL.md": SKILL("ok", "yes"),
	}, "path", "root");
	fromDir("ignore-and-fdignore", {
		"root/.ignore": "a/\n",
		"root/.fdignore": "b/\n",
		"root/a/SKILL.md": SKILL("a", "no"),
		"root/b/SKILL.md": SKILL("b", "no"),
		"root/c/SKILL.md": SKILL("c", "yes"),
	}, "path", "root");

	// --- loadFromDir: validation + frontmatter ---
	fromDir("name-invalid", { "root/Bad_Name/SKILL.md": SKILL("Bad_Name", "d") }, "user", "root");
	fromDir("name-too-long", { "root/long/SKILL.md": SKILL("a".repeat(65), "d") }, "user", "root");
	fromDir("name-hyphens", {
		"root/lead/SKILL.md": SKILL("-lead", "d"),
		"root/trail/SKILL.md": SKILL("trail-", "d"),
		"root/dbl/SKILL.md": SKILL("dou--ble", "d"),
	}, "user", "root");
	fromDir("desc-missing", { "root/s/SKILL.md": "---\nname: ok\n---\nb\n" }, "user", "root");
	fromDir("desc-too-long", { "root/s/SKILL.md": SKILL("ok", "d".repeat(1025)) }, "user", "root");
	fromDir("disable-true", { "root/s/SKILL.md": SKILL("s", "d", "disable-model-invocation: true\n") }, "user", "root");
	fromDir("disable-nonbool", { "root/s/SKILL.md": SKILL("s", "d", "disable-model-invocation: yes\n") }, "user", "root");
	fromDir("fm-error-skillmd", { "root/s/SKILL.md": "---\nname: a\nname: b\ndescription: D\n---\nb\n" }, "user", "root");
	fromDir("fm-error-plainmd", { "root/note.md": "---\nname: a\nname: b\n---\nb\n" }, "user", "root");

	// --- loadFromDir: sources + symlinks ---
	fromDir("source-project", { "root/s/SKILL.md": SKILL("s", "d") }, "project", "root");
	fromDir("source-path", { "root/s/SKILL.md": SKILL("s", "d") }, "path", "root");
	fromDir("source-custom", { "root/s/SKILL.md": SKILL("s", "d") }, "custom-source", "root");
	fromDir("symlink-broken", { "root/broken/SKILL.md": { symlink: "missing.md" } }, "path", "root");
	fromDir("symlink-skillmd", {
		"real/SKILL.md": SKILL("linked", "via symlink"),
		"root/link/SKILL.md": { symlink: "../../real/SKILL.md" },
	}, "path", "root");

	// --- load: explicit paths ---
	load("single-path", { "skills/a/SKILL.md": SKILL("alpha", "A") }, { cwd: "proj", skillPaths: ["../skills"] });
	load("file-direct", { "one.md": "---\nname: direct-skill\ndescription: D\n---\nb\n" }, { cwd: "proj", skillPaths: ["../one.md"] });
	load("missing-path", {}, { cwd: "proj", skillPaths: ["nope"] });
	load("nonmd", { "x.txt": "hi" }, { cwd: "proj", skillPaths: ["../x.txt"] });
	load("collision", {
		"a/SKILL.md": SKILL("dup", "first"),
		"b/SKILL.md": SKILL("dup", "second"),
	}, { cwd: "proj", skillPaths: ["../a", "../b"] });
	load("same-file-twice", { "a/SKILL.md": SKILL("once", "only") }, { cwd: "proj", skillPaths: ["../a/SKILL.md", "../a/SKILL.md"] });
	load("symlink-alias", {
		"real/SKILL.md": SKILL("aliased", "real"),
		"link.md": { symlink: "real/SKILL.md" },
	}, { cwd: "proj", skillPaths: ["../real", "../link.md"] });
	load("multiple-skills-one-dir", {
		"root/a/SKILL.md": SKILL("aa", "A"),
		"root/b/SKILL.md": SKILL("bb", "B"),
	}, { cwd: "proj", skillPaths: ["../root"] });
	load("empty", {}, { cwd: "proj", skillPaths: [] });
	load("tilde", { "home/sk/SKILL.md": SKILL("homed", "H") }, { cwd: "proj", skillPaths: ["~/sk"] });

	// --- load: defaults + source classification ---
	load("defaults-user-project", {
		"agent/skills/user-skill/SKILL.md": SKILL("user-skill", "user skill"),
		"proj/.pi/skills/proj-skill/SKILL.md": SKILL("proj-skill", "project skill"),
	}, { cwd: "proj", agentDir: "agent", includeDefaults: true });
	load("path-under-user-dir", { "agent/skills/user-skill/SKILL.md": SKILL("user-skill", "u") },
		{ cwd: "proj", agentDir: "agent", skillPaths: ["../agent/skills"] });
	load("path-under-project-dir", { "proj/.pi/skills/proj-skill/SKILL.md": SKILL("proj-skill", "p") },
		{ cwd: "proj", agentDir: "agent", skillPaths: [".pi/skills"] });
	load("defaults-path-under-user", { "agent/skills/user-skill/SKILL.md": SKILL("user-skill", "u") },
		{ cwd: "proj", agentDir: "agent", includeDefaults: true, skillPaths: ["../agent/skills"] });
	load("collision-default-vs-path", {
		"agent/skills/dup/SKILL.md": SKILL("dup", "winner"),
		"other/SKILL.md": SKILL("dup", "loser"),
	}, { cwd: "proj", agentDir: "agent", includeDefaults: true, skillPaths: ["../other"] });

	// --- formatForPrompt ---
	const fsSkill = (name, description, extra = {}) => ({
		name,
		description,
		filePath: "/x/" + name + "/SKILL.md",
		...extra,
	});
	c.push(sk("fmt-empty", { op: "formatForPrompt", skills: [] }));
	c.push(sk("fmt-all-disabled", { op: "formatForPrompt", skills: [fsSkill("a", "d", { disableModelInvocation: true })] }));
	c.push(sk("fmt-read", { op: "formatForPrompt", skills: [fsSkill("a", "d")], fileReadTool: "read" }));
	c.push(sk("fmt-default-tool", { op: "formatForPrompt", skills: [fsSkill("a", "d")] }));
	c.push(sk("fmt-bash", { op: "formatForPrompt", skills: [fsSkill("a", "d")], fileReadTool: "bash" }));
	c.push(sk("fmt-indirect", { op: "formatForPrompt", skills: [fsSkill("a", "d")], fileReadTool: "indirect" }));
	c.push(sk("fmt-unknown-tool", { op: "formatForPrompt", skills: [fsSkill("a", "d")], fileReadTool: "weird" }));
	c.push(sk("fmt-escaping", { op: "formatForPrompt", skills: [fsSkill("a<b", "d&<e>\"f'g", { filePath: "/x&p/Q.md" })] }));
	c.push(sk("fmt-mixed", {
		op: "formatForPrompt",
		skills: [fsSkill("one", "first"), fsSkill("two", "second", { disableModelInvocation: true }), fsSkill("three", "third")],
	}));
	c.push(sk("fmt-unicode", { op: "formatForPrompt", skills: [fsSkill("café", "Ünïcode ✓")] }));
	c.push(sk("fmt-truthy-disable", { op: "formatForPrompt", skills: [fsSkill("a", "d", { disableModelInvocation: "yes" })] }));

	// --- protocol misuse ---
	c.push(sk("err-missing-op", {}));
	c.push(sk("err-unknown-op", { op: "bogus" }));
	c.push(sk("err-missing-source", { op: "loadFromDir" }));
	c.push(sk("err-bad-skills", { op: "formatForPrompt", skills: "x" }));
	c.push(sk("err-bad-skill-paths", { op: "load", options: { skillPaths: 5 } }));
	c.push(sk("err-bad-files", { op: "load", files: [] }));

	return c.filter(Boolean);
})(),

...(() => {
	// ========================================================================
	// sess-manager / nested-tool-calls / pi-manifest / output-guard /
	// defaults — the session-manager + small-modules wave. Protocol mirrors
	// crates/pi-cli/src/core_harness.rs and the matching ts-harness.js
	// entries: {op, ...} for sess-manager/pi-manifest/defaults, {ops: [...]}
	// scripts for nested-tool-calls/output-guard. Fixture ids avoid the
	// 8-hex / uuid shapes the normalizers rewrite; both sides rewrite
	// uuids, ISO timestamps and epoch-ms `timestamp` fields.
	// ========================================================================

	// --- defaults (core/defaults.js constants) ---
	const def = (name, args) => ({ tool: "defaults", name: "def-" + name, args });
	const defCases = [
		def("constants", { op: "constants" }),
		def("err-missing-op", {}),
		def("err-unknown-op", { op: "bogus" }),
	];

	// --- pi-manifest (core/pi-manifest.js readPiManifest) ---
	const pm = (name, args) => ({ tool: "pi-manifest", name: "pm-" + name, args });
	const pmJson = (o) => JSON.stringify(o);
	const pmCases = [
		pm("all-fields", { op: "read", content: pmJson({ name: "pkg", pi: { extensions: ["a"], skills: ["s1", "s2"], prompts: [], themes: ["t"] } }) }),
		pm("subset", { op: "read", content: pmJson({ pi: { skills: ["only"] } }) }),
		pm("empty-pi", { op: "read", content: pmJson({ pi: {} }) }),
		pm("missing-pi", { op: "read", content: pmJson({ name: "pkg" }) }),
		pm("pi-null", { op: "read", content: pmJson({ pi: null }) }),
		pm("pi-array", { op: "read", content: pmJson({ pi: [] }) }),
		pm("pi-string", { op: "read", content: pmJson({ pi: "x" }) }),
		pm("root-array", { op: "read", content: "[1,2]" }),
		pm("root-string", { op: "read", content: '"str"' }),
		pm("root-number", { op: "read", content: "42" }),
		pm("invalid-json", { op: "read", content: "{oops" }),
		pm("empty-content", { op: "read", content: "" }),
		pm("whitespace", { op: "read", content: "   \n\t " }),
		pm("bom", { op: "read", content: "\uFEFF" + pmJson({ pi: { skills: ["a"] } }) }),
		pm("non-string-entries", { op: "read", content: pmJson({ pi: { skills: ["a", 1], prompts: ["p"], themes: "x" } }) }),
		pm("nested-objects", { op: "read", content: pmJson({ pi: { skills: [{ a: 1 }] } }) }),
		pm("extra-pi-fields", { op: "read", content: pmJson({ pi: { other: true, skills: ["s"] } }) }),
		pm("unicode-entries", { op: "read", content: pmJson({ pi: { skills: ["café", "日本語"] } }) }),
		pm("duplicate-entries", { op: "read", content: pmJson({ pi: { extensions: ["a", "a"] } }) }),
		pm("err-missing-op", {}),
		pm("err-unknown-op", { op: "bogus" }),
		pm("err-missing-content", { op: "read" }),
	];

	// --- nested-tool-calls (core/nested-tool-calls.js recorder) ---
	const ntc = (name, args) => ({ tool: "nested-tool-calls", name: "ntc-" + name, args });
	const usage = (o = {}) => ({
		input: o.input ?? 10,
		output: o.output ?? 20,
		cacheRead: o.cacheRead ?? 0,
		cacheWrite: o.cacheWrite ?? 0,
		...(o.cacheWrite1h !== undefined ? { cacheWrite1h: o.cacheWrite1h } : {}),
		...(o.reasoning !== undefined ? { reasoning: o.reasoning } : {}),
		totalTokens: o.totalTokens ?? 30,
		cost: o.cost ?? { input: 0.1, output: 0.2, cacheRead: 0, cacheWrite: 0, total: 0.3 },
	});
	const ntcCases = [
		ntc("limits", { ops: [{ op: "limits" }] }),
		ntc("start-finish-ok", { ops: [
			{ op: "start", id: "call-1", name: "read", arguments: { path: "/x" } },
			{ op: "finish", record: 0, isError: false },
			{ op: "snapshot" },
		] }),
		ntc("start-finish-error", { ops: [
			{ op: "start", id: "call-1", name: "bash", arguments: {} },
			{ op: "finish", record: 0, isError: true, errorText: "boom" },
			{ op: "snapshot" },
		] }),
		ntc("error-empty-text", { ops: [
			{ op: "start", id: "c", name: "t", arguments: {} },
			{ op: "finish", record: 0, isError: true, errorText: "" },
			{ op: "snapshot" },
		] }),
		ntc("error-truncated", { ops: [
			{ op: "start", id: "c", name: "t", arguments: {} },
			{ op: "finish", record: 0, isError: true, errorText: "x".repeat(600) },
			{ op: "snapshot" },
		] }),
		ntc("error-truncated-multibyte", { ops: [
			{ op: "start", id: "c", name: "t", arguments: {} },
			{ op: "finish", record: 0, isError: true, errorText: "é".repeat(600) },
			{ op: "snapshot" },
		] }),
		ntc("unfinished-snapshot", { ops: [
			{ op: "start", id: "c1", name: "t", arguments: { a: 1 } },
			{ op: "start", id: "c2", name: "t", arguments: { a: 2 } },
			{ op: "finish", record: 0, isError: false },
			{ op: "snapshot" },
		] }),
		ntc("empty-snapshot-null", { ops: [{ op: "snapshot" }] }),
		ntc("null-arguments", { ops: [
			{ op: "start", id: "c", name: "t", arguments: null },
			{ op: "snapshot" },
		] }),
		ntc("missing-arguments", { ops: [
			{ op: "start", id: "c", name: "t" },
			{ op: "snapshot" },
		] }),
		ntc("scalar-arguments", { ops: [
			{ op: "start", id: "c1", name: "t", arguments: "text" },
			{ op: "start", id: "c2", name: "t", arguments: 42 },
			{ op: "start", id: "c3", name: "t", arguments: 0.5 },
			{ op: "snapshot" },
		] }),
		ntc("float-arguments-roundtrip", { ops: [
			{ op: "start", id: "c", name: "t", arguments: { a: 0.1, b: 1e21, c: 1.5e-7, d: 123456789012345678901 } },
			{ op: "snapshot" },
		] }),
		ntc("unicode-arguments", { ops: [
			{ op: "start", id: "c", name: "t", arguments: { text: "café ✓ 日本語", nested: { arr: [1, "two", null, true] } } },
			{ op: "snapshot" },
		] }),
		ntc("arguments-over-per-call", { ops: [
			{ op: "start", id: "c", name: "t", arguments: { big: "y".repeat(9000) } },
			{ op: "snapshot" },
		] }),
		ntc("arguments-over-total", { ops: [
			{ op: "start", id: "c1", name: "t", arguments: { big: "y".repeat(16000) } },
			{ op: "start", id: "c2", name: "t", arguments: { big: "y".repeat(16000) } },
			{ op: "start", id: "c3", name: "t", arguments: { small: "ok" } },
			{ op: "snapshot" },
		] }),
		ntc("max-calls-drop", { ops: [
			...Array.from({ length: 257 }, (_, i) => ({ op: "start", id: "c" + i, name: "t", arguments: { i } })),
			{ op: "snapshot" },
		] }),
		ntc("finish-null-record", { ops: [
			{ op: "start", id: "c", name: "t", arguments: {} },
			{ op: "finish", record: null, isError: true, errorText: "x" },
			{ op: "snapshot" },
		] }),
		ntc("finish-twice", { ops: [
			{ op: "start", id: "c", name: "t", arguments: {} },
			{ op: "finish", record: 0, isError: false },
			{ op: "finish", record: 0, isError: true, errorText: "second" },
			{ op: "snapshot" },
		] }),
		ntc("total-usage-null", { ops: [{ op: "totalUsage" }] }),
		ntc("add-usage-single", { ops: [
			{ op: "addUsage", usage: usage() },
			{ op: "totalUsage" },
		] }),
		ntc("add-usage-combine", { ops: [
			{ op: "addUsage", usage: usage({ input: 1, output: 2, totalTokens: 3 }) },
			{ op: "addUsage", usage: usage({ input: 10, output: 20, totalTokens: 30, cacheRead: 5 }) },
			{ op: "totalUsage" },
		] }),
		ntc("add-usage-optional-splits", { ops: [
			{ op: "addUsage", usage: usage({ cacheWrite1h: 7 }) },
			{ op: "addUsage", usage: usage({ reasoning: 4 }) },
			{ op: "totalUsage" },
		] }),
		ntc("add-usage-then-snapshot", { ops: [
			{ op: "start", id: "c", name: "t", arguments: {} },
			{ op: "addUsage", usage: usage() },
			{ op: "snapshot" },
			{ op: "totalUsage" },
		] }),
		ntc("err-missing-ops", {}),
		ntc("err-unknown-op", { ops: [{ op: "bogus" }] }),
		ntc("err-start-missing-id", { ops: [{ op: "start", name: "t" }] }),
		ntc("err-start-missing-name", { ops: [{ op: "start", id: "c" }] }),
		ntc("err-addusage-missing", { ops: [{ op: "addUsage" }] }),
	];

	// --- output-guard (core/output-guard.js, scripted stream writers) ---
	const og = (name, args) => ({ tool: "output-guard", name: "og-" + name, args });
	const ogCases = [
		og("write-stdout", { ops: [
			{ op: "writeStdout", text: "hello" },
			{ op: "isStdoutTakenOver" },
		] }),
		og("write-raw-and-flush", { ops: [
			{ op: "writeRawStdout", text: "raw" },
			{ op: "flushRawStdout" },
		] }),
		og("raw-empty-skipped", { ops: [
			{ op: "writeRawStdout", text: "" },
			{ op: "flushRawStdout" },
		] }),
		og("takeover-redirects", { ops: [
			{ op: "writeStdout", text: "before" },
			{ op: "takeOverStdout" },
			{ op: "isStdoutTakenOver" },
			{ op: "writeStdout", text: "during" },
			{ op: "writeRawStdout", text: "raw-during" },
			{ op: "restoreStdout" },
			{ op: "isStdoutTakenOver" },
			{ op: "writeStdout", text: "after" },
		] }),
		og("takeover-idempotent", { ops: [
			{ op: "takeOverStdout" },
			{ op: "takeOverStdout" },
			{ op: "writeStdout", text: "x" },
			{ op: "restoreStdout" },
			{ op: "writeStdout", text: "y" },
		] }),
		og("restore-without-takeover", { ops: [
			{ op: "restoreStdout" },
			{ op: "isStdoutTakenOver" },
			{ op: "writeStdout", text: "ok" },
		] }),
		og("write-stdout-error", { ops: [
			{ op: "writeStdout", text: "a" },
			{ op: "writeStdout", text: "b" },
		], stdoutScript: [null, { code: "EPIPE", message: "broken pipe" }] }),
		og("raw-failure-sticky", { ops: [
			{ op: "writeRawStdout", text: "a" },
			{ op: "writeRawStdout", text: "b" },
			{ op: "waitForRawStdoutBackpressure" },
			{ op: "waitForRawStdoutBackpressure" },
			{ op: "flushRawStdout" },
		], stdoutScript: [{ code: "EPIPE", message: "gone" }] }),
		og("raw-failure-at-flush", { ops: [
			{ op: "writeRawStdout", text: "a" },
			{ op: "flushRawStdout" },
		], stdoutScript: [{ code: "EBADF", message: "bad descriptor" }] }),
		og("raw-retry-enobufs", { ops: [
			{ op: "writeRawStdout", text: "a" },
			{ op: "flushRawStdout" },
		], stdoutScript: [{ code: "ENOBUFS", message: "backpressure" }] }),
		og("stderr-error-while-taken-over", { ops: [
			{ op: "takeOverStdout" },
			{ op: "writeStdout", text: "redirected" },
		], stderrScript: [{ code: "EPIPE", message: "stderr broke" }] }),
		og("flush-while-taken-over", { ops: [
			{ op: "takeOverStdout" },
			{ op: "flushRawStdout" },
			{ op: "writeRawStdout", text: "raw" },
		] }),
		og("wait-no-writes", { ops: [{ op: "waitForRawStdoutBackpressure" }] }),
		og("err-missing-ops", {}),
		og("err-unknown-op", { ops: [{ op: "bogus" }] }),
		og("err-write-missing-text", { ops: [{ op: "writeStdout" }] }),
		og("err-raw-missing-text", { ops: [{ op: "writeRawStdout" }] }),
	];

	// --- sess-manager (core/session-manager.js) ---
	const se = (name, args) => ({ tool: "sess-manager", name: "sess-" + name, args });
	// Fixture builders: stable ISO timestamps, ids that never look like the
	// 8-hex / uuid shapes the normalizers rewrite.
	const seHeader = (version, cwd = "/home/user/proj", extra = {}) => ({
		type: "session", version, id: "sess-header-id", timestamp: smTs(0), cwd, ...extra,
	});
	const seEntry = (type, id, parentId, n, extra = {}) => ({
		type, id, parentId, timestamp: smTs(n), ...extra,
	});
	const seMsgEntry = (id, parentId, message, n) => seEntry("message", id, parentId, n, { message });
	const seUserE = (id, parentId, text, n) =>
		seMsgEntry(id, parentId, { role: "user", content: [{ type: "text", text }], timestamp: smTs(n) }, n);
	const seAssistantE = (id, parentId, text, n, extra = {}) =>
		seMsgEntry(id, parentId, { role: "assistant", content: [{ type: "text", text }], provider: "anthropic", model: "claude-sonnet-4", timestamp: smTs(n), ...extra }, n);
	const seJsonl = (...entries) => entries.map((e) => JSON.stringify(e)).join("\n") + "\n";
	const seM = (role, content, extra = {}) => ({ role, content, ...extra });
	const mOps = (factory, extra, ops) => ({ op: "manager", factory, ...extra, ops });

	const sessCases = [];
	const addSess = (name, args) => sessCases.push(se(name, args));

	// ---- pure ops ----
	const hdr3 = seHeader(3);
	const chain = [
		hdr3,
		seUserE("e1", null, "hello", 1),
		seAssistantE("e2", "e1", "hi", 2),
		seUserE("e3", "e2", "again", 3),
	];
	addSess("parse-basic", { op: "parseSessionEntries", content: seJsonl(...chain) });
	addSess("parse-crlf", { op: "parseSessionEntries", content: chain.map((e) => JSON.stringify(e)).join("\r\n") + "\r\n" });
	addSess("parse-lone-cr", { op: "parseSessionEntries", content: JSON.stringify(hdr3) + "\r" + JSON.stringify(seUserE("e1", null, "x", 1)) });
	addSess("parse-no-trailing-newline", { op: "parseSessionEntries", content: seJsonl(...chain).trimEnd() });
	addSess("parse-blank-lines", { op: "parseSessionEntries", content: "\n" + seJsonl(hdr3) + "\n\n" + seJsonl(seUserE("e1", null, "x", 1)) + "\n\n" });
	addSess("parse-invalid-lines", { op: "parseSessionEntries", content: "{oops\n" + seJsonl(hdr3) + "not json\n" + seJsonl(seUserE("e1", null, "x", 1)) });
	addSess("parse-empty", { op: "parseSessionEntries", content: "" });
	addSess("parse-whitespace-only", { op: "parseSessionEntries", content: "  \n\t\n " });
	addSess("parse-bom-first-line", { op: "parseSessionEntries", content: "\uFEFF" + seJsonl(hdr3, seUserE("e1", null, "x", 1)) });
	addSess("parse-leading-space-line", { op: "parseSessionEntries", content: "  " + seJsonl(hdr3) + "\t" + seJsonl(seUserE("e1", null, "x", 1)) });

	addSess("migrate-v1", { op: "migrateSessionEntries", entries: [
		seHeader(1),
		{ type: "message", timestamp: smTs(1), message: { role: "user", content: "hi", timestamp: smTs(1) } },
		{ type: "message", timestamp: smTs(2), message: { role: "assistant", content: "yo", timestamp: smTs(2) } },
	] });
	addSess("migrate-v1-compaction", { op: "migrateSessionEntries", entries: [
		seHeader(1),
		{ type: "message", timestamp: smTs(1), message: { role: "user", content: "a", timestamp: smTs(1) } },
		{ type: "message", timestamp: smTs(2), message: { role: "user", content: "b", timestamp: smTs(2) } },
		{ type: "compaction", timestamp: smTs(3), summary: "s", tokensBefore: 9, firstKeptEntryIndex: 2 },
	] });
	addSess("migrate-v1-compaction-header-index", { op: "migrateSessionEntries", entries: [
		seHeader(1),
		{ type: "message", timestamp: smTs(1), message: { role: "user", content: "a", timestamp: smTs(1) } },
		{ type: "compaction", timestamp: smTs(2), summary: "s", tokensBefore: 9, firstKeptEntryIndex: 0 },
	] });
	addSess("migrate-v2-hook", { op: "migrateSessionEntries", entries: [
		seHeader(2),
		seMsgEntry("e1", null, { role: "hookMessage", content: "hm", timestamp: smTs(1) }, 1),
		seUserE("e2", "e1", "u", 2),
	] });
	addSess("migrate-v3-noop", { op: "migrateSessionEntries", entries: chain });
	addSess("migrate-no-header", { op: "migrateSessionEntries", entries: [
		{ type: "message", timestamp: smTs(1), message: { role: "user", content: "a", timestamp: smTs(1) } },
	] });
	addSess("migrate-version-missing", { op: "migrateSessionEntries", entries: [
		{ type: "session", id: "sess-header-id", timestamp: smTs(0), cwd: "/home/user/proj" },
		{ type: "message", timestamp: smTs(1), message: { role: "user", content: "a", timestamp: smTs(1) } },
	] });

	addSess("latest-compaction-none", { op: "getLatestCompactionEntry", entries: chain });
	addSess("latest-compaction-one", { op: "getLatestCompactionEntry", entries: [
		hdr3, seUserE("e1", null, "a", 1),
		seEntry("compaction", "c1", "e1", 2, { summary: "s", tokensBefore: 5, firstKeptEntryId: "e1" }),
	] });
	addSess("latest-compaction-last", { op: "getLatestCompactionEntry", entries: [
		hdr3, seUserE("e1", null, "a", 1),
		seEntry("compaction", "c1", "e1", 2, { summary: "s1", tokensBefore: 5, firstKeptEntryId: "e1" }),
		seUserE("e2", "c1", "b", 3),
		seEntry("compaction", "c2", "e2", 4, { summary: "s2", tokensBefore: 6, firstKeptEntryId: "e2" }),
	] });

	addSess("entry-to-messages-user", { op: "sessionEntryToContextMessages", entry: seUserE("e1", null, "hello", 1) });
	addSess("entry-to-messages-system-null-content", { op: "sessionEntryToContextMessages", entry: seMsgEntry("e1", null, { role: "system", content: null, timestamp: smTs(1) }, 1) });
	addSess("entry-to-messages-user-null-content", { op: "sessionEntryToContextMessages", entry: seMsgEntry("e1", null, { role: "user", content: null, timestamp: smTs(1) }, 1) });
	addSess("entry-to-messages-assistant-null-content", { op: "sessionEntryToContextMessages", entry: seMsgEntry("e1", null, { role: "assistant", content: null, timestamp: smTs(1) }, 1) });
	addSess("entry-to-messages-toolresult-null-content", { op: "sessionEntryToContextMessages", entry: seMsgEntry("e1", null, { role: "toolResult", content: null, timestamp: smTs(1) }, 1) });
	addSess("entry-to-messages-custom", { op: "sessionEntryToContextMessages", entry: seEntry("custom_message", "e1", null, 1, { customType: "ext", content: [{ type: "text", text: "c" }], display: true, details: { x: 1 } }) });
	addSess("entry-to-messages-custom-null-content", { op: "sessionEntryToContextMessages", entry: seEntry("custom_message", "e1", null, 1, { customType: "ext", content: null, display: false }) });
	addSess("entry-to-messages-branch-summary", { op: "sessionEntryToContextMessages", entry: seEntry("branch_summary", "e1", null, 1, { fromId: "e0", summary: "abandoned" }) });
	addSess("entry-to-messages-branch-summary-empty", { op: "sessionEntryToContextMessages", entry: seEntry("branch_summary", "e1", null, 1, { fromId: "e0" }) });
	addSess("entry-to-messages-compaction", { op: "sessionEntryToContextMessages", entry: seEntry("compaction", "c1", null, 1, { summary: "s", tokensBefore: 7, firstKeptEntryId: "c1" }) });
	addSess("entry-to-messages-compaction-system", { op: "sessionEntryToContextMessages", entry: seEntry("compaction", "c1", null, 1, { summary: "s", tokensBefore: 7, firstKeptEntryId: "c1", systemMessage: { role: "system", content: "sys", timestamp: smTs(0) } }) });
	addSess("entry-to-messages-unknown-type", { op: "sessionEntryToContextMessages", entry: seEntry("custom", "e1", null, 1, { customType: "x" }) });
	addSess("entry-to-messages-thinking", { op: "sessionEntryToContextMessages", entry: seEntry("thinking_level_change", "e1", null, 1, { thinkingLevel: "high" }) });

	// build* over a branched tree with compaction, edits and settings
	const branched = [
		hdr3,
		seMsgEntry("s1", null, seM("system", "sys prompt"), 1),
		seUserE("e1", "s1", "hello", 2),
		seEntry("thinking_level_change", "t1", "e1", 3, { thinkingLevel: "high" }),
		seEntry("model_change", "m1", "t1", 4, { provider: "anthropic", modelId: "claude-op-4" }),
		seAssistantE("e2", "m1", "hi", 5),
		seEntry("compaction", "c1", "e2", 6, { summary: "compacted", tokensBefore: 100, firstKeptEntryId: "e2" }),
		seUserE("e3", "c1", "after compaction", 7),
		seUserE("b1", "e2", "branched from e2", 8),
		seAssistantE("b2", "b1", "branch reply", 9),
	];
	addSess("build-context-last", { op: "buildContextEntries", entries: branched });
	addSess("build-context-root", { op: "buildContextEntries", entries: branched, leafId: null });
	addSess("build-context-explicit-leaf", { op: "buildContextEntries", entries: branched, leafId: "e2" });
	addSess("build-context-branch-leaf", { op: "buildContextEntries", entries: branched, leafId: "b1" });
	addSess("build-context-missing-leaf", { op: "buildContextEntries", entries: branched, leafId: "nope" });
	addSess("build-context-empty", { op: "buildContextEntries", entries: [] });
	addSess("build-context-orphan", { op: "buildContextEntries", entries: [
		hdr3, seUserE("e1", "missing-parent", "orphan", 1), seUserE("e2", null, "root", 2),
	] });
	addSess("build-projection-last", { op: "buildSessionProjection", entries: branched });
	addSess("build-projection-branch", { op: "buildSessionProjection", entries: branched, leafId: "b2" });
	addSess("build-context-settings", { op: "buildSessionContext", entries: [
		hdr3,
		seUserE("e1", null, "a", 1),
		seEntry("thinking_level_change", "t1", "e1", 2, { thinkingLevel: "low" }),
		seEntry("model_change", "m1", "t1", 3, { provider: "openai", modelId: "gpt-x" }),
		seAssistantE("e2", "m1", "b", 4, { provider: "other", model: "m-2" }),
	] });
	addSess("build-context-defaults", { op: "buildSessionContext", entries: [hdr3, seUserE("e1", null, "a", 1)] });
	const edited = [
		hdr3,
		seUserE("e1", null, "original", 1),
		seEntry("context_edit", "x1", "e1", 2, { targetId: "e1", replacement: { content: "edited text" } }),
	];
	addSess("build-projection-edit", { op: "buildSessionProjection", entries: edited });
	addSess("build-projection-edit-null", { op: "buildSessionProjection", entries: [
		hdr3,
		seUserE("e1", null, "original", 1),
		seEntry("context_edit", "x1", "e1", 2, { targetId: "e1", replacement: null }),
	] });
	addSess("build-projection-edit-assistant", { op: "buildSessionProjection", entries: [
		hdr3,
		seAssistantE("e1", null, "orig", 1),
		seEntry("context_edit", "x1", "e1", 2, { targetId: "e1", replacement: { content: "replaced" } }),
	] });
	const twoCompactions = [
		hdr3,
		seUserE("e1", null, "a", 1),
		seEntry("compaction", "c1", "e1", 2, { summary: "first", tokensBefore: 10, firstKeptEntryId: "e1" }),
		seUserE("e2", "c1", "b", 3),
		seEntry("compaction", "c2", "e2", 4, { summary: "second", tokensBefore: 20, firstKeptEntryId: "e2" }),
		seUserE("e3", "c2", "c", 5),
	];
	addSess("build-projection-two-compactions", { op: "buildSessionProjection", entries: twoCompactions });
	addSess("build-context-compaction-keepsystem", { op: "buildSessionProjection", entries: [
		hdr3,
		seMsgEntry("s1", null, seM("system", "keep me"), 1),
		seUserE("e1", "s1", "a", 2),
		seEntry("compaction", "c1", "e1", 3, { summary: "s", tokensBefore: 1, firstKeptEntryId: "s1" }),
		seUserE("e2", "c1", "b", 4),
	] });

	addSess("assert-id-valid", { op: "assertValidSessionId", id: "abc-DEF_123.456" });
	addSess("assert-id-single-char", { op: "assertValidSessionId", id: "a" });
	addSess("assert-id-space", { op: "assertValidSessionId", id: "has space" });
	addSess("assert-id-empty", { op: "assertValidSessionId", id: "" });

	addSess("default-session-dir", { op: "getDefaultSessionDir", cwd: "proj", agentDir: "agent" });
	addSess("default-session-dir-nested", { op: "getDefaultSessionDir", cwd: "a/b/c", agentDir: "x/y" });
	addSess("default-session-dir-dot", { op: "getDefaultSessionDir", cwd: ".", agentDir: "." });

	// ---- fs ops ----
	addSess("load-entries-file", { op: "loadEntriesFromFile", files: { "s.jsonl": seJsonl(...chain) }, filePath: "s.jsonl" });
	addSess("load-entries-missing", { op: "loadEntriesFromFile", filePath: "nope.jsonl" });
	addSess("load-entries-crlf", { op: "loadEntriesFromFile", files: { "s.jsonl": chain.map((e) => JSON.stringify(e)).join("\r\n") + "\r\n" }, filePath: "s.jsonl" });
	addSess("load-entries-no-final-newline", { op: "loadEntriesFromFile", files: { "s.jsonl": seJsonl(...chain).trimEnd() }, filePath: "s.jsonl" });
	addSess("load-entries-bom", { op: "loadEntriesFromFile", files: { "s.jsonl": "\uFEFF" + seJsonl(...chain) }, filePath: "s.jsonl" });
	addSess("load-entries-garbage", { op: "loadEntriesFromFile", files: { "s.jsonl": "not a session\nat all\n" }, filePath: "s.jsonl" });
	addSess("load-entries-empty", { op: "loadEntriesFromFile", files: { "s.jsonl": "" }, filePath: "s.jsonl" });
	addSess("load-entries-header-no-id", { op: "loadEntriesFromFile", files: { "s.jsonl": seJsonl({ type: "session", version: 3, timestamp: smTs(0) }, seUserE("e1", null, "x", 1)) }, filePath: "s.jsonl" });
	addSess("load-entries-first-not-header", { op: "loadEntriesFromFile", files: { "s.jsonl": seJsonl(seUserE("e1", null, "x", 1), hdr3) }, filePath: "s.jsonl" });
	addSess("load-entries-blank-and-invalid", { op: "loadEntriesFromFile", files: { "s.jsonl": "\n{bad\n" + seJsonl(hdr3) + "\n\n" + seJsonl(seUserE("e1", null, "x", 1)) + "nope\n" }, filePath: "s.jsonl" });

	const hdrCwd = (cwd) => seHeader(3, cwd);
	const fmrFile = (name, cwd, n) => ({ [name + ".jsonl"]: seJsonl(hdrCwd(cwd), seUserE("e1", null, "m" + n, n)) });
	addSess("find-recent-newest", {
		op: "findMostRecentSession",
		files: { ...fmrFile("a", "/home/user/proj", 1), ...fmrFile("b", "/home/user/proj", 2) },
		mtimes: { "a.jsonl": 1700000000000, "b.jsonl": 1700000001000 },
		sessionDir: ".",
	});
	addSess("find-recent-cwd-match", {
		op: "findMostRecentSession",
		files: { ...fmrFile("a", "/home/user/proj", 1), ...fmrFile("b", "/elsewhere", 2) },
		mtimes: { "a.jsonl": 1700000000000, "b.jsonl": 1700000001000 },
		sessionDir: ".", cwd: "/home/user/proj",
	});
	addSess("find-recent-no-cwd", {
		op: "findMostRecentSession",
		files: { ...fmrFile("a", "/home/user/proj", 1), ...fmrFile("b", "/elsewhere", 2) },
		mtimes: { "a.jsonl": 1700000000000, "b.jsonl": 1700000001000 },
		sessionDir: ".",
	});
	addSess("find-recent-skips-non-session", {
		op: "findMostRecentSession",
		files: { "new.jsonl": "garbage\n", ...fmrFile("old", "/home/user/proj", 1) },
		mtimes: { "new.jsonl": 1700000002000, "old.jsonl": 1700000000000 },
		sessionDir: ".", cwd: "/home/user/proj",
	});
	addSess("find-recent-ignores-non-jsonl", {
		op: "findMostRecentSession",
		files: { "a.txt": "x", ...fmrFile("b", "/home/user/proj", 1) },
		mtimes: { "a.txt": 1700000009000, "b.jsonl": 1700000000000 },
		sessionDir: ".",
	});
	addSess("find-recent-leading-blank", {
		op: "findMostRecentSession",
		files: { "a.jsonl": "\n" + seJsonl(hdrCwd("/home/user/proj"), seUserE("e1", null, "x", 1)) },
		sessionDir: ".", cwd: "/home/user/proj",
	});
	addSess("find-recent-header-no-cwd", {
		op: "findMostRecentSession",
		files: { "a.jsonl": seJsonl({ type: "session", version: 3, id: "sess-header-id", timestamp: smTs(0) }, seUserE("e1", null, "x", 1)) },
		sessionDir: ".", cwd: "/home/user/proj",
	});
	addSess("find-recent-empty-dir", { op: "findMostRecentSession", files: { "keep/keep.txt": "x" }, sessionDir: "keep" });
	addSess("find-recent-missing-dir", { op: "findMostRecentSession", sessionDir: "nope" });

	// ---- manager flows ----
	addSess("mgr-create-basic", mOps("create", {}, [
		{ op: "getCwd" }, { op: "getSessionDir" }, { op: "getSessionId" }, { op: "getSessionFile" },
		{ op: "isPersisted" }, { op: "getEntryCount" }, { op: "getLeafId" }, { op: "getHeader" },
		{ op: "getEntries" }, { op: "listDir" }, { op: "buildSessionContext" },
	]));
	addSess("mgr-create-explicit-id", mOps("create", { id: "my-session" }, [
		{ op: "getSessionId" }, { op: "getHeader" }, { op: "newSession" }, { op: "getSessionId" },
	]));
	addSess("mgr-create-parent-session", mOps("create", { parentSession: "/old/session.jsonl" }, [
		{ op: "getHeader" },
	]));
	addSess("mgr-create-invalid-id", mOps("create", { id: "has space" }, []));
	addSess("mgr-append-user-writes-file", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "hello" }]) },
		{ op: "getSessionFile" }, { op: "readFile" }, { op: "listDir" }, { op: "getEntryCount" },
		{ op: "getLeafId" }, { op: "getEntries" },
	]));
	addSess("mgr-setup-only-no-file", mOps("create", {}, [
		{ op: "appendThinkingLevelChange", thinkingLevel: "high" },
		{ op: "appendModelChange", provider: "anthropic", modelId: "claude-sonnet-4" },
		{ op: "appendSessionInfo", name: "my session" },
		{ op: "appendCustomEntry", customType: "ext", data: { a: 1 } },
		{ op: "getSessionFile" }, { op: "readFile" }, { op: "listDir" }, { op: "getEntryCount" },
	]));
	addSess("mgr-append-order", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "one" }]) },
		{ op: "appendMessage", message: seM("assistant", [{ type: "text", text: "two" }]) },
		{ op: "appendThinkingLevelChange", thinkingLevel: "low" },
		{ op: "appendModelChange", provider: "p", modelId: "m" },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "three" }]) },
		{ op: "readFile" }, { op: "getEntryCount" }, { op: "buildSessionContext" },
	]));
	addSess("mgr-append-usage", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "u" }]) },
		{ op: "appendUsage", kind: "turn", provider: "anthropic", model: "claude-sonnet-4", usage: usage() },
		{ op: "appendUsage", kind: "turn", provider: "anthropic", model: "claude-sonnet-4", usage: usage({ input: 1 }), note: "cached turn" },
		{ op: "getEntries" }, { op: "readFile" },
	]));
	addSess("mgr-append-compaction", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "u" }]) },
		{ op: "appendMessage", message: seM("assistant", [{ type: "text", text: "a" }]) },
		{ op: "appendCompaction", summary: "summarized", tokensBefore: 500 },
		{ op: "getEntries" }, { op: "buildContextEntries" },
	]));
	addSess("mgr-append-compaction-system", mOps("create", {}, [
		{ op: "appendMessage", message: seM("system", "be helpful") },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "u" }]) },
		{ op: "appendCompaction", summary: "summarized", tokensBefore: 500, details: { reason: "auto" }, fromHook: true, usage: usage() },
		{ op: "getEntries" }, { op: "buildSessionProjection" },
	]));
	addSess("mgr-append-compaction-firstkept", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "one" }]) },
		{ op: "appendMessage", message: seM("assistant", [{ type: "text", text: "two" }]) },
		{ op: "appendCompaction", summary: "s", firstKeptEntryId: "FIRST", tokensBefore: 5 },
		{ op: "getEntries" },
	]));
	addSess("mgr-session-info", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "u" }]) },
		{ op: "appendSessionInfo", name: "  spaced\r\nname  " },
		{ op: "getSessionName" }, { op: "getEntries" },
		{ op: "appendSessionInfo", name: "  " },
		{ op: "getSessionName" },
	]));
	addSess("mgr-custom-message", mOps("create", {}, [
		{ op: "appendCustomMessageEntry", customType: "ext", content: [{ type: "text", text: "custom" }], display: true, details: { k: 1 } },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "u" }]) },
		{ op: "getEntries" }, { op: "buildSessionContext" }, { op: "readFile" },
	]));
	// Entry ids are generated at runtime (8-hex, normalized to <id8>), so
	// context-edit cases target fixed fixture ids via in-memory sessions.
	addSess("mgr-context-edit", mOps("inMemory", { entries: [seHeader(3), seUserE("e1", null, "original", 1)] }, [
		{ op: "appendContextEdit", targetId: "e1", replacement: { content: "edited" } },
		{ op: "getEntries" }, { op: "buildSessionContext" },
	]));
	addSess("mgr-context-edit-null", mOps("inMemory", { entries: [seHeader(3), seUserE("e1", null, "original", 1)] }, [
		{ op: "appendContextEdit", targetId: "e1", replacement: null },
		{ op: "buildSessionContext" },
	]));
	addSess("mgr-context-edit-assistant", mOps("inMemory", { entries: [seHeader(3), seAssistantE("e1", null, "orig", 1)] }, [
		{ op: "appendContextEdit", targetId: "e1", replacement: { content: "replaced" } },
		{ op: "buildSessionContext" },
	]));
	addSess("mgr-context-edit-errors", mOps("inMemory", { entries: [seHeader(3), seUserE("e1", null, "u", 1)] }, [
		{ op: "appendContextEdit", targetId: "nope", replacement: { content: "x" } },
		{ op: "appendContextEdit", targetId: "e1", replacement: { content: 5 } },
		{ op: "appendContextEdit", targetId: "e1", replacement: "not an object" },
	]));
	addSess("mgr-context-edit-fixed-ids", mOps("inMemory", { entries: [
		seHeader(3), seUserE("e1", null, "original", 1), seEntry("thinking_level_change", "t1", "e1", 2, { thinkingLevel: "high" }),
	] }, [
		{ op: "appendContextEdit", targetId: "e1", replacement: { content: "edited" } },
		{ op: "buildSessionContext" },
		{ op: "appendContextEdit", targetId: "t1", replacement: { content: "x" } },
		{ op: "appendContextEdit", targetId: "nope", replacement: null },
		{ op: "branch", id: "e1" },
		{ op: "appendContextEdit", targetId: "e1", replacement: null },
	]));
	addSess("mgr-labels", mOps("inMemory", { entries: [seHeader(3), seUserE("e1", null, "u", 1)] }, [
		{ op: "appendLabelChange", targetId: "e1", label: "bookmark" },
		{ op: "getLabel", id: "e1" },
		{ op: "getTree" },
		{ op: "appendLabelChange", targetId: "e1" },
		{ op: "getLabel", id: "e1" },
		{ op: "appendLabelChange", targetId: "nope", label: "x" },
	]));
	addSess("mgr-tree", mOps("inMemory", { entries: [
		seHeader(3),
		seUserE("e1", null, "root", 1),
		seUserE("e2", "e1", "left", 2),
		seUserE("e3", "e1", "right", 3),
		seUserE("e4", "e4", "self-parent", 4),
		seUserE("e5", "ghost", "orphan", 5),
	] }, [
		{ op: "getTree" }, { op: "getChildren", id: "e1" }, { op: "getChildren", id: "e2" },
		{ op: "getEntry", id: "e3" }, { op: "getEntry", id: "ghost" }, { op: "getBranch" },
		{ op: "getBranch", fromId: "e2" }, { op: "getBranch", fromId: "nope" },
	]));
	addSess("mgr-branch", mOps("inMemory", { entries: [
		seHeader(3), seUserE("e1", null, "one", 1), seUserE("e2", "e1", "two", 2), seUserE("e3", "e2", "three", 3),
	] }, [
		{ op: "getLeafId" },
		{ op: "branch", id: "e1" },
		{ op: "getLeafId" }, { op: "getBranch" },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "new branch" }]) },
		{ op: "getChildren", id: "e1" }, { op: "buildContextEntries" },
		{ op: "branch", id: "nope" },
	]));
	addSess("mgr-reset-leaf", mOps("inMemory", { entries: [seHeader(3), seUserE("e1", null, "one", 1)] }, [
		{ op: "resetLeaf" }, { op: "getLeafId" },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "fresh root" }]) },
		{ op: "getBranch" }, { op: "getTree" }, { op: "buildContextEntries" },
	]));
	addSess("mgr-branch-with-summary", mOps("inMemory", { entries: [
		seHeader(3), seUserE("e1", null, "one", 1), seUserE("e2", "e1", "two", 2), seUserE("e3", "e2", "three", 3),
	] }, [
		{ op: "branchWithSummary", summary: "abandoned path", details: { why: "user" } },
		{ op: "getEntries" }, { op: "buildSessionContext" }, { op: "getLeafId" },
	]));
	addSess("mgr-branch-with-summary-from", mOps("inMemory", { entries: [
		seHeader(3), seUserE("e1", null, "one", 1), seUserE("e2", "e1", "two", 2),
	] }, [
		{ op: "branchWithSummary", branchFromId: "e1", summary: "s" },
		{ op: "buildSessionContext" },
		{ op: "branchWithSummary", branchFromId: "nope", summary: "s" },
	]));
	addSess("mgr-create-branched-session", mOps("inMemory", { entries: [
		seHeader(3), seUserE("e1", null, "one", 1), seUserE("e2", "e1", "two", 2), seUserE("e3", "e2", "three", 3),
	] }, [
		{ op: "appendLabelChange", targetId: "e2", label: "mark" },
		{ op: "createBranchedSession", leafId: "e2" },
		{ op: "getSessionId" }, { op: "getEntries" }, { op: "getLabel", id: "e2" },
		{ op: "createBranchedSession", leafId: "nope" },
	]));
	addSess("mgr-create-branched-persisted", {
		op: "manager", factory: "open", path: "src.jsonl",
		files: { "src.jsonl": seJsonl(...chain) },
		ops: [
			{ op: "createBranchedSession", leafId: "e2" },
			{ op: "getSessionFile" }, { op: "listDir", dir: "." }, { op: "readFile" },
		],
	});
	addSess("mgr-new-session", mOps("create", { id: "first-id" }, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "u" }]) },
		{ op: "newSession", id: "second-id", parentSession: "PARENT_FILE" },
		{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "getHeader" }, { op: "listDir" },
		{ op: "newSession", id: "bad id" },
	]));
	addSess("mgr-set-session-file", {
		op: "manager", factory: "create",
		files: {
			"existing.jsonl": seJsonl(...chain),
			"empty.jsonl": "",
			"invalid.jsonl": "not a session\n",
		},
		ops: [
			{ op: "setSessionFile", path: "existing.jsonl" },
			{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "getCwd" },
			{ op: "setSessionFile", path: "empty.jsonl" },
			{ op: "getEntryCount" }, { op: "readFile", path: "empty.jsonl" },
			{ op: "setSessionFile", path: "invalid.jsonl" },
		],
	});
	addSess("mgr-open-existing", {
		op: "manager", factory: "open", path: "s.jsonl",
		files: { "s.jsonl": seJsonl(...chain) },
		ops: [
			{ op: "getSessionId" }, { op: "getCwd" }, { op: "getEntryCount" }, { op: "getLeafId" },
			{ op: "getEntries" }, { op: "buildSessionContext" },
			{ op: "appendMessage", message: seM("user", [{ type: "text", text: "resumed" }]) },
			{ op: "readFile" },
		],
	});
	addSess("mgr-open-missing", {
		op: "manager", factory: "open", path: "fresh.jsonl",
		ops: [
			{ op: "getSessionFile" }, { op: "readFile", path: "fresh.jsonl" },
			{ op: "appendMessage", message: seM("user", [{ type: "text", text: "first" }]) },
			{ op: "readFile", path: "fresh.jsonl" },
		],
	});
	addSess("mgr-open-empty-file", {
		op: "manager", factory: "open", path: "e.jsonl",
		files: { "e.jsonl": "" },
		ops: [{ op: "getEntryCount" }, { op: "readFile", path: "e.jsonl" }, { op: "getSessionId" }],
	});
	addSess("mgr-open-invalid", {
		op: "manager", factory: "open", path: "bad.jsonl",
		files: { "bad.jsonl": "not a session file\n" },
		ops: [],
	});
	addSess("mgr-open-v1-migrates", {
		op: "manager", factory: "open", path: "v1.jsonl",
		files: { "v1.jsonl": seJsonl(
			seHeader(1),
			{ type: "message", timestamp: smTs(1), message: { role: "user", content: "hi", timestamp: smTs(1) } },
			{ type: "message", timestamp: smTs(2), message: { role: "hookMessage", content: "hm", timestamp: smTs(2) } },
		) },
		ops: [{ op: "getEntries" }, { op: "readFile" }],
	});
	addSess("mgr-open-cwd-override", {
		op: "manager", factory: "open", path: "s.jsonl", cwdOverride: "proj",
		files: { "s.jsonl": seJsonl(...chain) },
		ops: [{ op: "getCwd" }, { op: "getEntries" }],
	});
	addSess("mgr-open-no-trailing-newline", {
		op: "manager", factory: "open", path: "s.jsonl",
		files: { "s.jsonl": seJsonl(...chain).trimEnd() },
		ops: [{ op: "getEntryCount" }, { op: "readFile" }],
	});
	addSess("mgr-continue-recent-empty", mOps("continueRecent", {}, [
		{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "getSessionFile" }, { op: "listDir" },
	]));
	addSess("mgr-continue-recent-match", {
		op: "manager", factory: "continueRecent", cwd: "../proj",
		files: { "recent.jsonl": seJsonl(seHeader(3, "/tmp/proj"), seUserE("e1", null, "recent", 1)) },
		ops: [{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "getEntries" }, { op: "getCwd" }],
	});
	addSess("mgr-continue-recent-cwd-mismatch", {
		op: "manager", factory: "continueRecent", cwd: "proj",
		files: { "recent.jsonl": seJsonl(seHeader(3, "/elsewhere"), seUserE("e1", null, "recent", 1)) },
		ops: [{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "listDir" }],
	});
	addSess("mgr-in-memory-header", mOps("inMemory", { entries: chain }, [
		{ op: "isPersisted" }, { op: "getSessionId" }, { op: "getSessionFile" }, { op: "getEntryCount" },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "more" }]) },
		{ op: "getEntries" }, { op: "listDir" },
	]));
	addSess("mgr-in-memory-no-header", mOps("inMemory", { entries: [seUserE("e1", null, "orphan entry", 1)] }, [
		{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "getEntries" }, { op: "getHeader" },
	]));
	addSess("mgr-in-memory-empty", mOps("inMemory", {}, [
		{ op: "getEntryCount" }, { op: "getEntries" }, { op: "getHeader" }, { op: "buildSessionContext" },
	]));
	addSess("mgr-fork-from", {
		op: "manager", factory: "forkFrom", sourcePath: "src.jsonl", sessionDir: "sess",
		files: { "src.jsonl": seJsonl(...chain) },
		ops: [
			{ op: "getSessionId" }, { op: "getEntryCount" }, { op: "getCwd" },
			{ op: "readFile" }, { op: "listDir", dir: "sess" }, { op: "getHeader" },
		],
	});
	addSess("mgr-fork-from-explicit-id", {
		op: "manager", factory: "forkFrom", sourcePath: "src.jsonl", sessionDir: "sess", id: "forked-id",
		files: { "src.jsonl": seJsonl(...chain) },
		ops: [{ op: "getSessionId" }, { op: "readFile" }],
	});
	addSess("mgr-fork-from-empty", {
		op: "manager", factory: "forkFrom", sourcePath: "src.jsonl", sessionDir: "sess",
		files: { "src.jsonl": "" },
		ops: [],
	});
	addSess("mgr-fork-from-no-header", {
		op: "manager", factory: "forkFrom", sourcePath: "src.jsonl", sessionDir: "sess",
		files: { "src.jsonl": seJsonl(seUserE("e1", null, "x", 1)) },
		ops: [],
	});
	addSess("mgr-uses-default-dir", mOps("create", {}, [
		{ op: "usesDefaultSessionDir", agentDir: "agent" },
	]));
	addSess("mgr-projection-after-compaction", mOps("create", {}, [
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "one" }]) },
		{ op: "appendMessage", message: seM("assistant", [{ type: "text", text: "two" }]) },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "three" }]) },
		{ op: "appendCompaction", summary: "early stuff", tokensBefore: 100 },
		{ op: "appendMessage", message: seM("user", [{ type: "text", text: "after" }]) },
		{ op: "buildContextEntries" }, { op: "buildSessionProjection" }, { op: "readFile" },
	]));
	addSess("mgr-unknown-op", mOps("create", {}, [{ op: "bogus" }]));
	addSess("mgr-unknown-factory", mOps("bogus", {}, []));
	addSess("mgr-missing-ops", { op: "manager", factory: "create" });
	addSess("err-missing-op", {});
	addSess("err-unknown-op", { op: "bogus" });
	addSess("err-bad-files", { op: "loadEntriesFromFile", filePath: "x", files: [] });
	addSess("mtimes-invalid-ignored", { op: "findMostRecentSession", sessionDir: ".", mtimes: { "a.jsonl": "soon" } });

	return [...defCases, ...pmCases, ...ntcCases, ...ogCases, ...sessCases];
})(),

...(() => {
	// ========================================================================
	// agent-queue — pi-agent-core (packages/agent) `Agent` queue surface:
	// steer/followUp queues, queue modes, peek/clear/hasQueued. Protocol
	// mirrors crates/pi-cli/src/agent_harness.rs and the ts-harness.js
	// agent-queue entry: the TS side drives a real Agent (dummy streamFn,
	// never called), the Rust side the AgentQueues port. Messages carry
	// literal numeric timestamps; no normalization needed.
	// ========================================================================
	const aq = (name, args) => ({ tool: "agent-queue", name: "aq-" + name, args });
	const aqMsg = (text, ts) => ({ role: "user", content: [{ type: "text", text }], timestamp: ts });

	const c = [];
	c.push(aq("defaults", { ops: [
		{ op: "getSteeringMode" },
		{ op: "getFollowUpMode" },
		{ op: "hasQueued" },
		{ op: "peek" },
	]}));
	c.push(aq("steer-one-at-a-time-peek-first-only", { ops: [
		{ op: "steer", message: aqMsg("a", 1) },
		{ op: "steer", message: aqMsg("b", 2) },
		{ op: "hasQueued" },
		{ op: "peek" },
		{ op: "peek" },
	]}));
	c.push(aq("steer-all-mode-peek-all", {
		steeringMode: "all",
		ops: [
			{ op: "steer", message: aqMsg("a", 1) },
			{ op: "steer", message: aqMsg("b", 2) },
			{ op: "peek" },
			{ op: "getSteeringMode" },
		],
	}));
	c.push(aq("set-steering-mode-mid-run", { ops: [
		{ op: "steer", message: aqMsg("a", 1) },
		{ op: "steer", message: aqMsg("b", 2) },
		{ op: "peek" },
		{ op: "setSteeringMode", mode: "all" },
		{ op: "peek" },
		{ op: "getSteeringMode" },
		{ op: "setSteeringMode", mode: "one-at-a-time" },
		{ op: "peek" },
	]}));
	c.push(aq("follow-up-shadowed-by-steering", { ops: [
		{ op: "steer", message: aqMsg("s", 1) },
		{ op: "followUp", message: aqMsg("f", 2) },
		{ op: "peek" },
		{ op: "clearSteering" },
		{ op: "peek" },
	]}));
	c.push(aq("follow-up-mode-all", {
		followUpMode: "all",
		ops: [
			{ op: "followUp", message: aqMsg("f1", 1) },
			{ op: "followUp", message: aqMsg("f2", 2) },
			{ op: "peek" },
			{ op: "getFollowUpMode" },
		],
	}));
	c.push(aq("clear-variants", { ops: [
		{ op: "steer", message: aqMsg("s", 1) },
		{ op: "followUp", message: aqMsg("f", 2) },
		{ op: "clearSteering" },
		{ op: "hasQueued" },
		{ op: "peek" },
		{ op: "clearAll" },
		{ op: "hasQueued" },
		{ op: "peek" },
	]}));
	c.push(aq("mixed-with-initial-state", {
		initialState: {
			messages: [{ role: "system", content: [{ type: "text", text: "sys" }], timestamp: 0 }],
		},
		ops: [
			{ op: "getSteeringMode" },
			{ op: "steer", message: aqMsg("a", 1) },
			{ op: "followUp", message: aqMsg("f", 2) },
			{ op: "setFollowUpMode", mode: "all" },
			{ op: "peek" },
			{ op: "clearFollowUp" },
			{ op: "peek" },
			{ op: "hasQueued" },
		],
	}));
	// Protocol-misuse cases: in-band {error} with byte-identical messages.
	c.push(aq("err-unknown-op", { ops: [{ op: "bogus" }] }));
	c.push(aq("err-missing-ops", {}));
	c.push(aq("err-bad-steering-mode", { steeringMode: "sometimes", ops: [] }));
	c.push(aq("err-bad-set-mode", { ops: [{ op: "setSteeringMode", mode: "many" }] }));
	c.push(aq("err-missing-message", { ops: [{ op: "followUp" }] }));
	c.push(aq("err-missing-mode", { ops: [{ op: "setFollowUpMode" }] }));

	return c.filter(Boolean);
})(),

...(() => {
	// ========================================================================
	// declare-tool-changes — pi-agent-core first-request reduction: the real
	// Agent (scripted streamFn recording the TranscriptContext messages of
	// the single request, Date.now pinned to nowMs) prompted with the
	// pending batch after seeding contextMessages/contextTools. The result
	// is defaultConvertToLlm([...contextMessages, ...pending]) with
	// declareToolChanges applied to the pending batch. Mirrors
	// crates/pi-cli/src/agent_harness.rs run_declare_tool_changes_harness.
	// All timestamps are literal numbers; no dynamic values.
	// ========================================================================
	const dtc = (name, args) => ({ tool: "declare-tool-changes", name: "dtc-" + name, args });
	const decl = (name, description) => ({
		name,
		description: description ?? "Tool " + name,
		parameters: { type: "object", properties: {}, additionalProperties: false },
	});
	const ctxTool = (name, description) => ({
		name,
		description: description ?? "Tool " + name,
		parameters: { type: "object", properties: {}, additionalProperties: false },
	});
	const sys = (content, timestamp, extra = {}) => ({ role: "system", content, timestamp, ...extra });
	const usr = (text, timestamp) => ({ role: "user", content: [{ type: "text", text }], timestamp });

	const c = [];
	// No changes: the transcript already declares exactly the context tools,
	// so the pending batch passes through untouched.
	c.push(dtc("no-changes", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a"), decl("b")] })],
		contextTools: [ctxTool("a"), ctxTool("b")],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// No changes with nothing declared at all (no system message anywhere).
	c.push(dtc("no-changes-no-tools", {
		contextMessages: [usr("first", 50)],
		contextTools: [],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// Tools added: a new system message (timestamp = nowMs) is inserted
	// before the first non-system pending message.
	c.push(dtc("tools-added", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a")] })],
		contextTools: [ctxTool("a"), ctxTool("b")],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// Tools removed.
	c.push(dtc("tools-removed", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a"), decl("b")] })],
		contextTools: [ctxTool("a")],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// A changed definition is a removal plus an addition of the same name.
	c.push(dtc("tool-redefined", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a", "v1")] })],
		contextTools: [ctxTool("a", "v2")],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// Mixed add + remove in one delta message.
	c.push(dtc("mixed-add-remove", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a"), decl("b")] })],
		contextTools: [ctxTool("b"), ctxTool("c")],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// Pending system message with stale tool fields: replaced by the delta
	// between the committed transcript and the executable set.
	c.push(dtc("pending-system-stale-replaced", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a")] })],
		contextTools: [ctxTool("a"), ctxTool("b")],
		pendingMessages: [sys("note", 300, { toolsAdded: [decl("z"), decl("q")] }), usr("hi", 301)],
		nowMs: 1000,
	}));
	// Pending system message with a stale toolsRemoved only.
	c.push(dtc("pending-system-stale-removed", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a"), decl("b")] })],
		contextTools: [ctxTool("a")],
		pendingMessages: [sys("note", 300, { toolsRemoved: [{ name: "c" }] })],
		nowMs: 1000,
	}));
	// Pending system already clean and no changes: kept as-is (same objects).
	c.push(dtc("pending-system-clean-kept", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a")] })],
		contextTools: [ctxTool("a")],
		pendingMessages: [sys("note", 300), usr("hi", 301)],
		nowMs: 1000,
	}));
	// Pending system clean but changes exist: the delta is written into it.
	c.push(dtc("pending-system-clean-changed", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a")] })],
		contextTools: [ctxTool("a"), ctxTool("b")],
		pendingMessages: [sys("note", 300), usr("hi", 301)],
		nowMs: 1000,
	}));
	// Empty pending batch with changes: the delta message is the only
	// initial message (insert index falls off the end).
	c.push(dtc("empty-pending-changes", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a")] })],
		contextTools: [ctxTool("a"), ctxTool("b")],
		pendingMessages: [],
		nowMs: 1000,
	}));
	// Only context tools, no transcript tools: the delta message is inserted
	// before the first non-system pending message.
	c.push(dtc("context-tools-only", {
		contextMessages: [usr("first", 50)],
		contextTools: [ctxTool("a")],
		pendingMessages: [usr("hi", 200)],
		nowMs: 1000,
	}));
	// Pending messages are all system: the delta message goes last.
	c.push(dtc("all-system-pending", {
		contextMessages: [sys("base", 100, { toolsAdded: [decl("a")] })],
		contextTools: [ctxTool("a"), ctxTool("b")],
		pendingMessages: [sys("s1", 300), sys("s2", 301)],
		nowMs: 1000,
	}));
	// Protocol-misuse cases: in-band {error} with byte-identical messages.
	c.push(dtc("err-missing-nowms", {
		contextMessages: [],
		contextTools: [],
		pendingMessages: [],
	}));
	c.push(dtc("err-bad-context-tools", {
		contextMessages: [],
		contextTools: "nope",
		pendingMessages: [],
		nowMs: 1000,
	}));

	return c.filter(Boolean);
})(),

...(() => {
	// ========================================================================
	// run-tool-call — pi-agent-core agent-loop runToolCall with scripted
	// tools (update/return/throw execute steps), scripted prepareArguments,
	// before/after hooks, and a pinned performance.now clock (durationMs =
	// Math.round of the scripted difference; one case pins a .5 fraction).
	// Mirrors crates/pi-cli/src/agent_harness.rs run_run_tool_call_harness.
	// No timestamps or random values.
	// ========================================================================
	const rtc = (name, args) => ({ tool: "run-tool-call", name: "rtc-" + name, args });
	const rtcParams = () => ({
		type: "object",
		properties: { path: { type: "string" } },
		required: ["path"],
		additionalProperties: false,
	});
	const rtcTool = (name, extra = {}) => ({
		name,
		description: "Tool " + name,
		parameters: rtcParams(),
		label: name,
		...extra,
	});
	const rtcCall = (name, callArgs) => ({ id: "call-1", name, arguments: callArgs });
	const rtcAssistant = () => ({
		role: "assistant",
		content: [{ type: "toolCall", id: "call-1", name: "read", arguments: { path: "x" } }],
		timestamp: 5,
	});
	const rtcUser = (text, timestamp) => ({ role: "user", content: [{ type: "text", text }], timestamp });
	const rtcContext = () => [rtcUser("ctx", 1)];

	const c = [];
	// Happy path: two update steps then a full result (content, details,
	// structuredContent); durationMs from the scripted clock.
	c.push(rtc("happy-path-updates", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", {
			execute: { script: [
				{ perfNow: 1010, update: { content: [{ type: "text", text: "partial" }], details: { stage: 1 } } },
				{ perfNow: 1100, return: {
					content: [{ type: "text", text: "done" }],
					details: { ok: true },
					structuredContent: { path: "x" },
				} },
			] },
		})],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		contextTools: [{ name: "read", description: "Tool read", parameters: rtcParams(), label: "read" }],
		perfSeq: [1000],
	}));
	// Tool not found: immediate error outcome, no durationMs, no updates.
	c.push(rtc("tool-not-found", {
		toolCall: rtcCall("write", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1000, return: { content: [], details: {} } }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// Validation error: arguments miss the required "path" property.
	c.push(rtc("validation-error", {
		toolCall: rtcCall("read", {}),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1000, return: { content: [], details: {} } }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// beforeToolCall blocks without a reason: the default message is used.
	c.push(rtc("before-block-no-reason", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1000, return: { content: [], details: {} } }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		beforeToolCall: { block: true },
		perfSeq: [1000],
	}));
	// beforeToolCall blocks with a reason.
	c.push(rtc("before-block-reason", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1000, return: { content: [], details: {} } }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		beforeToolCall: { block: true, reason: "Not allowed" },
		perfSeq: [1000],
	}));
	// beforeToolCall blocks with terminate: the error result carries it.
	c.push(rtc("before-block-terminate", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1000, return: { content: [], details: {} } }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		beforeToolCall: { block: true, reason: "Stop here", terminate: true },
		perfSeq: [1000],
	}));
	// execute throws: the message becomes an error result, durationMs kept.
	c.push(rtc("execute-throw", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [
			{ perfNow: 1015, update: { content: [{ type: "text", text: "partial" }], details: null } },
			{ perfNow: 1025, throw: "boom" },
		] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// execute returns isError: true — passed through as the outcome flag.
	c.push(rtc("execute-is-error", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1050, return: {
			content: [{ type: "text", text: "soft failure" }],
			details: {},
			isError: true,
		} }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// afterToolCall overrides content: structuredContent is dropped (it was
	// not replaced along with the content).
	c.push(rtc("after-content-override-drops-structured", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1050, return: {
			content: [{ type: "text", text: "original" }],
			details: { d: 1 },
			structuredContent: { s: 1 },
		} }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		afterToolCall: { content: [{ type: "text", text: "replaced" }] },
		perfSeq: [1000],
	}));
	// afterToolCall returns structuredContent (and overrides details/usage):
	// content is kept, structuredContent is replaced.
	c.push(rtc("after-keeps-structured-content", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1050, return: {
			content: [{ type: "text", text: "original" }],
			details: { d: 1 },
			structuredContent: { s: 1 },
		} }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		afterToolCall: { details: { d: 2 }, structuredContent: { s: 2 }, usage: { totalTokens: 7 } },
		perfSeq: [1000],
	}));
	// afterToolCall overrides isError (and terminate) on a success.
	c.push(rtc("after-is-error-override", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1050, return: {
			content: [{ type: "text", text: "done" }],
			details: {},
		} }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		afterToolCall: { isError: true, terminate: true },
		perfSeq: [1000],
	}));
	// afterToolCall throws: the result becomes an error tool result.
	c.push(rtc("after-throw", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [{ perfNow: 1050, return: {
			content: [{ type: "text", text: "done" }],
			details: { d: 1 },
		} }] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		afterToolCall: { throw: "after hook failed" },
		perfSeq: [1000],
	}));
	// prepareArguments script rewrite: the outcome toolCall carries the
	// rewritten arguments (validation and execution see them too).
	c.push(rtc("prepare-arguments-script", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", {
			prepareArguments: { mode: "script", script: [{ in: { path: "x" }, out: { path: "rewritten" } }] },
			execute: { script: [{ perfNow: 1050, return: { content: [{ type: "text", text: "ok" }], details: {} } }] },
		})],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// prepareArguments identity: same reference — toolCall object unchanged.
	c.push(rtc("prepare-arguments-identity", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", {
			prepareArguments: { mode: "identity" },
			execute: { script: [{ perfNow: 1050, return: { content: [{ type: "text", text: "ok" }], details: {} } }] },
		})],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// durationMs Math.round with a .5 fraction (50.5 → 51).
	c.push(rtc("duration-half-fraction", {
		toolCall: rtcCall("read", { path: "x" }),
		tools: [rtcTool("read", { execute: { script: [
			{ perfNow: 1010.25, update: { content: [{ type: "text", text: "partial" }], details: null } },
			{ perfNow: 1050.5, return: { content: [{ type: "text", text: "done" }], details: {} } },
		] } })],
		assistantMessage: rtcAssistant(),
		contextMessages: rtcContext(),
		perfSeq: [1000],
	}));
	// Protocol-misuse cases: in-band {error} with byte-identical messages.
	c.push(rtc("err-missing-tool-call", { tools: [], assistantMessage: rtcAssistant() }));
	c.push(rtc("err-missing-tools", { toolCall: rtcCall("read", { path: "x" }), assistantMessage: rtcAssistant() }));

	return c.filter(Boolean);
})(),

...(() => {
	// ========================================================================
	// stream-assistant — pi-agent-core runAgentLoop driving
	// streamAssistantResponse over a single scripted provider turn (events
	// from `script`, final message from `result`; result() is authoritative,
	// so the done/error payload fields are junk on purpose). Date.now is
	// pinned to nowMs. Mirrors the Rust harness entry; the shared wire
	// protocol is documented in agent-t34-protocol.md. Messages carry
	// literal numeric timestamps.
	// ========================================================================
	const sa = (name, args) => ({ tool: "stream-assistant", name: "sa-" + name, args });
	const NOW = 1780000000000;
	const saUser = (text, timestamp) => ({ role: "user", content: [{ type: "text", text }], timestamp });
	const saSystem = (text, timestamp) => ({ role: "system", content: text, timestamp });
	// Structurally valid AssistantMessage with overridable fields.
	const saMsg = (o = {}) => ({
		role: "assistant",
		content: o.content ?? [{ type: "text", text: "final" }],
		api: o.api ?? "unknown",
		provider: o.provider ?? "unknown",
		model: o.model ?? "unknown",
		usage: o.usage ?? {
			input: 3,
			output: 5,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 8,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: o.stopReason ?? "stop",
		...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
		timestamp: o.timestamp ?? 4400,
	});
	const saText = (text) => [{ type: "text", text }];
	const saBase = (script, result, extra = {}) => ({ script, result, nowMs: NOW, ...extra });

	const c = [];
	// Plain done: no partial at all — message_start emitted at finalization
	// with a copy of the final message. Full event sequence pinned.
	c.push(sa("plain-done", saBase([{ type: "done", reason: "stop", message: { junk: "ignored" } }], saMsg())));
	// start + text_delta×2 + done: message_start with a copy of the first
	// partial, one message_update per delta (assistantMessageEvent = the
	// raw event JSON, message = {...partial}), message_end with the final.
	c.push(sa("start-deltas-done", saBase(
		[
			{ type: "start", partial: saMsg({ content: saText("He") }) },
			{ type: "text_delta", contentIndex: 0, delta: "ll", partial: saMsg({ content: saText("Hell") }) },
			{ type: "text_delta", contentIndex: 0, delta: "o", partial: saMsg({ content: saText("Hello") }) },
			{ type: "done", reason: "stop", message: { junk: true } },
		],
		saMsg({ content: saText("Hello") }),
		{ prompts: [saUser("hi", 1000)] },
	)));
	// start + text_end + done: text_end also replaces the last partial and
	// emits message_update.
	c.push(sa("start-text-end-done", saBase(
		[
			{ type: "start", partial: saMsg({ content: saText("Hel") }) },
			{ type: "text_end", contentIndex: 0, content: "Hello", partial: saMsg({ content: saText("Hello") }) },
			{ type: "done", reason: "stop", message: null },
		],
		saMsg({ content: saText("Hello") }),
	)));
	// thinking_start/thinking_delta path.
	c.push(sa("thinking-deltas", saBase(
		[
			{ type: "start", partial: saMsg({ content: [] }) },
			{ type: "thinking_start", contentIndex: 0, partial: saMsg({ content: [{ type: "thinking", text: "hmm" }] }) },
			{ type: "thinking_delta", contentIndex: 0, delta: "…", partial: saMsg({ content: [{ type: "thinking", text: "hmm…" }] }) },
			{ type: "done", reason: "stop", message: null },
		],
		saMsg({ content: [{ type: "thinking", text: "hmm…" }, { type: "text", text: "done" }] }),
	)));
	// toolcall_start/toolcall_end path with no tools in the context — the
	// events are still emitted; the final result has no tool calls so the
	// loop ends after the single turn.
	c.push(sa("toolcall-events", saBase(
		[
			{ type: "start", partial: saMsg({ content: [] }) },
			{ type: "toolcall_start", contentIndex: 0, partial: saMsg({ content: [{ type: "toolCall", id: "call-1", name: "read", arguments: { path: "x" } }] }) },
			{ type: "toolcall_end", contentIndex: 0, toolCall: { id: "call-1", name: "read", arguments: { path: "x" } }, partial: saMsg({ content: [{ type: "toolCall", id: "call-1", name: "read", arguments: { path: "x" } }] }) },
			{ type: "done", reason: "stop", message: null },
		],
		saMsg(),
	)));
	// Stream ends without a terminal event: the same finalization runs
	// after the for-await loop (message_start at finalization, then
	// message_end) — no done/error event needed at all.
	c.push(sa("empty-script-no-terminal", saBase([], saMsg())));
	// Error event path: result stopReason "error" — the loop hard-exits
	// right after message_end (turn_end, agent_end; no further polling
	// output). The event's own error payload is ignored junk.
	c.push(sa("error-event", saBase(
		[{ type: "error", reason: "error", error: { junk: "ignored" } }],
		saMsg({ stopReason: "error", errorMessage: "boom" }),
	)));
	// dropSystem transformContext: streamCalls.messages lacks the system
	// message (the user message survives).
	c.push(sa("drop-system", {
		script: [{ type: "done", reason: "stop", message: null }],
		result: saMsg(),
		nowMs: NOW,
		transformContext: "dropSystem",
		contextMessages: [saSystem("be gone", 100), saUser("hi", 200)],
	}));
	// dropAssistant convertToLlm: streamCalls.messages lacks the assistant
	// context message.
	c.push(sa("drop-assistant", {
		script: [{ type: "done", reason: "stop", message: null }],
		result: saMsg(),
		nowMs: NOW,
		convertToLlm: "dropAssistant",
		contextMessages: [saUser("hi", 200), saMsg({ content: saText("earlier"), timestamp: 300 })],
	}));
	// reasoning "medium": the final message carries thinkingLevel "medium"
	// as the LAST key (Object.assign after result()).
	c.push(sa("reasoning-medium", {
		script: [{ type: "done", reason: "stop", message: null }],
		result: saMsg(),
		nowMs: NOW,
		reasoning: "medium",
	}));
	// getApiKeyScript ["dynamic-key"]: the dynamic key wins.
	c.push(sa("api-key-dynamic", {
		script: [{ type: "done", reason: "stop", message: null }],
		result: saMsg(),
		nowMs: NOW,
		getApiKeyScript: ["dynamic-key"],
	}));
	// Static apiKey with no getApiKeyScript.
	c.push(sa("api-key-static", {
		script: [{ type: "done", reason: "stop", message: null }],
		result: saMsg(),
		nowMs: NOW,
		apiKey: "static-key",
	}));
	// Empty dynamic key falls through to the static one (|| semantics).
	c.push(sa("api-key-empty-falls-to-static", {
		script: [{ type: "done", reason: "stop", message: null }],
		result: saMsg(),
		nowMs: NOW,
		getApiKeyScript: [""],
		apiKey: "static-key",
	}));
	// Protocol-misuse cases: in-band {error} with byte-identical messages.
	c.push(sa("err-missing-script", { result: saMsg(), nowMs: NOW }));
	c.push(sa("err-missing-result", { script: [], nowMs: NOW }));
	c.push(sa("err-missing-now-ms", { script: [], result: saMsg() }));
	c.push(sa("err-bad-transform-context", {
		script: [],
		result: saMsg(),
		nowMs: NOW,
		transformContext: "dropEverything",
	}));

	return c.filter(Boolean);
})(),

...(() => {
	// ========================================================================
	// agent-loop — pi-agent-core runAgentLoop over multiple scripted
	// provider turns with scripted tools (steer/followUp/abort steps),
	// hook scripts (steering/followUp/finishTurn/prepareRequest/
	// prepareNextTurn), before/after hooks, and pinned Date.now +
	// performance.now clocks. Mirrors the Rust harness entry
	// (agent_loop_harness.rs). Messages carry literal numeric timestamps.
	// ========================================================================
	const al = (name, args) => ({ tool: "agent-loop", name: "al-" + name, args });
	const NOW = 1780000000000;
	const alUser = (text, timestamp) => ({ role: "user", content: [{ type: "text", text }], timestamp });
	const alMsg = (o = {}) => ({
		role: "assistant",
		content: o.content ?? [{ type: "text", text: "done" }],
		api: o.api ?? "unknown",
		provider: o.provider ?? "unknown",
		model: o.model ?? "unknown",
		usage: o.usage ?? {
			input: 3,
			output: 5,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 8,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: o.stopReason ?? "stop",
		...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
		timestamp: o.timestamp ?? 4400,
	});
	const alToolCall = (id, name, callArgs) => ({ type: "toolCall", id, name, arguments: callArgs });
	const alParams = () => ({
		type: "object",
		properties: { path: { type: "string" } },
		required: ["path"],
		additionalProperties: false,
	});
	const alTool = (name, extra = {}) => ({
		name,
		description: "Tool " + name,
		parameters: alParams(),
		label: name,
		...extra,
	});
	const alTurn = (result, events = []) => ({ events, result });
	const alModel = (id) => ({
		id,
		name: id,
		api: "unknown",
		provider: "unknown",
		baseUrl: "",
		reasoning: false,
		input: [],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 0,
		maxTokens: 0,
	});
	const alOk = (text = "ok") => ({ content: [{ type: "text", text }], details: {} });

	const c = [];
	// Baseline: single turn, no tools, stopReason "stop".
	c.push(al("single-turn-stop", {
		prompts: [alUser("hi", 1000)],
		providerTurns: [alTurn(alMsg())],
		nowMs: NOW,
	}));
	// Tool call happy path (parallel): the tool-loadout system message is
	// declared before the prompt; the tool executes (durationMs from the
	// scripted clock); a second provider turn ends the run.
	c.push(al("tool-call-happy-parallel", {
		prompts: [alUser("read x", 1000)],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// Sequential toolExecution with two tool calls in one message: the
	// observable order is start/end/message per tool, in source order.
	c.push(al("sequential-tool-execution", {
		prompts: [alUser("read both", 1000)],
		toolExecution: "sequential",
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk("one") }] } })],
		providerTurns: [
			alTurn(alMsg({
				content: [alToolCall("call-1", "read", { path: "a" }), alToolCall("call-2", "read", { path: "b" })],
				stopReason: "toolUse",
				timestamp: 2000,
			})),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// A tool with executionMode "sequential" forces sequential execution
	// even though config.toolExecution is parallel (the default).
	c.push(al("tool-mode-sequential", {
		prompts: [alUser("read both", 1000)],
		tools: [alTool("read", { executionMode: "sequential", execute: { script: [{ perfNow: 1050, return: alOk("one") }] } })],
		providerTurns: [
			alTurn(alMsg({
				content: [alToolCall("call-1", "read", { path: "a" }), alToolCall("call-2", "read", { path: "b" })],
				stopReason: "toolUse",
				timestamp: 2000,
			})),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// Tool not found: an error tool result ("Tool write not found", no
	// durationMs); the run continues with a second provider turn.
	c.push(al("tool-not-found", {
		prompts: [alUser("write x", 1000)],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "write", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// Validation error: the tool call misses the required "path" property —
	// the validation failure becomes an error tool result.
	c.push(al("validation-error", {
		prompts: [alUser("read", 1000)],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", {})], stopReason: "toolUse", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// stopReason "length" with tool calls: every call fails with the
	// truncated-arguments error (byte-exact text) instead of executing.
	c.push(al("length-truncated-tool-calls", {
		prompts: [alUser("read x", 1000)],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "length", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// terminate: every tool result sets terminate:true → the run ends after
	// the batch (streamCalls length 1, no second provider call).
	c.push(al("terminate-all", {
		prompts: [alUser("read x", 1000)],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: { content: [{ type: "text", text: "stop" }], details: {}, terminate: true } }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// One terminate, one not: the batch does not terminate → the run
	// continues with a second provider turn.
	c.push(al("terminate-partial", {
		prompts: [alUser("read both", 1000)],
		tools: [
			alTool("read", { execute: { script: [{ perfNow: 1050, return: { content: [{ type: "text", text: "stop" }], details: {}, terminate: true } }] } }),
		],
		providerTurns: [
			alTurn(alMsg({
				content: [alToolCall("call-1", "read", { path: "a" }), alToolCall("call-2", "read", { path: "b" })],
				stopReason: "toolUse",
				timestamp: 2000,
			})),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// Steering injected mid-run by a tool execute step (steer buffer drained
	// first by the getSteeringMessages hook): the message is emitted and
	// reaches the second provider request.
	c.push(al("steer-step", {
		prompts: [alUser("read x", 1000)],
		steeringScript: [],
		tools: [alTool("read", { execute: { script: [
			{ steer: alUser("steered mid-run", 1500) },
			{ perfNow: 1050, return: alOk() },
		] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// steeringScript [[a,b]]: the first poll returns both messages (the loop
	// drains all of them before the first request); the next polls return
	// [] (script exhausted).
	c.push(al("steering-script-one-shot", {
		prompts: [alUser("hi", 1000)],
		steeringScript: [[alUser("steer a", 1500), alUser("steer b", 1600)]],
		providerTurns: [alTurn(alMsg())],
		nowMs: NOW,
	}));
	// Follow-up: messages returned after the run would stop produce one
	// extra turn.
	c.push(al("follow-up-extra-turn", {
		prompts: [alUser("hi", 1000)],
		followUpScript: [[alUser("follow up", 1500)]],
		providerTurns: [alTurn(alMsg()), alTurn(alMsg({ timestamp: 5000 }))],
		nowMs: NOW,
	}));
	// finishTurnDecisions ["end"]: the run ends immediately after the first
	// turn (no second provider call).
	c.push(al("finish-turn-end", {
		prompts: [alUser("hi", 1000)],
		finishTurnDecisions: ["end"],
		providerTurns: [alTurn(alMsg())],
		nowMs: NOW,
	}));
	// finishTurnDecisions ["continue"] with nothing queued: one context-only
	// extra provider call (streamCalls length 2, identical messages).
	c.push(al("finish-turn-continue", {
		prompts: [alUser("hi", 1000)],
		finishTurnDecisions: ["continue"],
		providerTurns: [alTurn(alMsg()), alTurn(alMsg({ timestamp: 5000 }))],
		nowMs: NOW,
	}));
	// prepareRequestScript: the second request sees the swapped model.
	c.push(al("prepare-request-model", {
		prompts: [alUser("read x", 1000)],
		prepareRequestScript: [{}, { model: alModel("m2") }],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// prepareNextTurnScript: the entry's messages are appended before the
	// next request and thinkingLevel "high" reaches the final message.
	c.push(al("prepare-next-turn-messages", {
		prompts: [alUser("read x", 1000)],
		prepareNextTurnScript: [{ messages: [alUser("prepared", 1500)], thinkingLevel: "high" }],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// abort step mid-run: the first tool aborts the run's controller during
	// parallel execution; the remaining thunk finalizes with the
	// "Operation aborted" error result, and the loop still schedules the
	// next provider call (an aborted signal only stops tool scheduling).
	c.push(al("abort-step-mid-run", {
		prompts: [alUser("read both", 1000)],
		tools: [
			alTool("read", { execute: { script: [{ abort: true, perfNow: 1050, return: alOk("first") }] } }),
		],
		providerTurns: [
			alTurn(alMsg({
				content: [alToolCall("call-1", "read", { path: "a" }), alToolCall("call-2", "read", { path: "b" })],
				stopReason: "toolUse",
				timestamp: 2000,
			})),
			alTurn(alMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// stopReason "aborted": hard exit right after the assistant message.
	c.push(al("stop-reason-aborted", {
		prompts: [alUser("hi", 1000)],
		providerTurns: [alTurn(alMsg({ stopReason: "aborted", errorMessage: "user aborted" }))],
		nowMs: NOW,
	}));
	// Provider script exhausted: the second stream call throws, which
	// propagates out of runAgentLoop as the in-band error.
	c.push(al("provider-script-exhausted", {
		prompts: [alUser("read x", 1000)],
		tools: [alTool("read", { execute: { script: [{ perfNow: 1050, return: alOk() }] } })],
		providerTurns: [
			alTurn(alMsg({ content: [alToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// Protocol-misuse cases: in-band {error} with byte-identical messages.
	c.push(al("err-missing-prompts", { providerTurns: [], nowMs: NOW }));
	c.push(al("err-missing-provider-turns", { prompts: [], nowMs: NOW }));
	c.push(al("err-bad-tool-execution", { prompts: [], providerTurns: [], toolExecution: "concurrent", nowMs: NOW }));
	c.push(al("err-tools-not-array", { prompts: [], providerTurns: [], tools: "nope", nowMs: NOW }));

	return c.filter(Boolean);
})(),

...(() => {
	// ========================================================================
	// agent — the REAL Agent class (packages/agent) over scripted provider
	// turns with scripted tools (snapshot/steer/followUp/abort steps) and a
	// script of harness ops (prompt/continue with optional settle:false,
	// steer/followUp/abort/waitForIdle/reset/getState). Mirrors the Rust
	// harness entry (agent_harness.rs run_agent_harness). Messages carry
	// literal numeric timestamps.
	// ========================================================================
	const ag = (name, args) => ({ tool: "agent", name: "ag-" + name, args });
	const NOW = 1780000000000;
	const agUser = (text, timestamp) => ({ role: "user", content: [{ type: "text", text }], timestamp });
	const agMsg = (o = {}) => ({
		role: "assistant",
		content: o.content ?? [{ type: "text", text: "done" }],
		api: o.api ?? "unknown",
		provider: o.provider ?? "unknown",
		model: o.model ?? "unknown",
		usage: o.usage ?? {
			input: 3,
			output: 5,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 8,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: o.stopReason ?? "stop",
		...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
		timestamp: o.timestamp ?? 4400,
	});
	const agToolCall = (id, name, callArgs) => ({ type: "toolCall", id, name, arguments: callArgs });
	const agParams = () => ({
		type: "object",
		properties: { path: { type: "string" } },
		required: ["path"],
		additionalProperties: false,
	});
	const agTool = (name, extra = {}) => ({
		name,
		description: "Tool " + name,
		parameters: agParams(),
		label: name,
		...extra,
	});
	const agTurn = (result, events = []) => ({ events, result });
	const agOk = (text = "ok") => ({ content: [{ type: "text", text }], details: {} });

	const c = [];
	// Baseline: single awaited prompt turn, no tools, stopReason "stop".
	c.push(ag("prompt-happy-path", {
		script: [{ op: "prompt", messages: [agUser("hi", 1000)] }],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// prompt with an input string: normalizePromptInput produces a user
	// message with timestamp = pinned Date.now (nowMs).
	c.push(ag("prompt-input-string", {
		script: [{ op: "prompt", input: "hello there" }],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// initialState without a leading system message: createInitialSystemMessage
	// is unshifted (system content + toolsAdded from the scripted tools).
	c.push(ag("initial-state-unshifts-system", {
		initialState: { systemPrompt: "be helpful" },
		tools: [agTool("read", { execute: { script: [{ perfNow: 1050, return: agOk() }] } })],
		script: [{ op: "prompt", messages: [agUser("hi", 1000)] }],
		providerTurns: [agTurn(agMsg())],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// initialState with a leading system message: kept as-is (no unshift).
	c.push(ag("initial-state-keeps-leading-system", {
		initialState: {
			messages: [
				{ role: "system", content: "custom base", timestamp: 1 },
				agUser("hi", 1000),
			],
		},
		script: [{ op: "prompt", messages: [agUser("more", 1000)] }],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// Tool call turn then final turn: the tool `snapshot` step observes
	// mid-run state (isStreaming true, pendingToolCalls populated).
	c.push(ag("tool-call-snapshot-mid-run", {
		tools: [agTool("read", { execute: { script: [
			{ snapshot: "during-tool" },
			{ perfNow: 1050, return: agOk() },
		] } })],
		script: [{ op: "prompt", messages: [agUser("read x", 1000)] }],
		providerTurns: [
			agTurn(agMsg({ content: [agToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			agTurn(agMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// steer before the prompt: the steering queue is drained by the loop's
	// first steering poll, so the steered message joins the first turn.
	c.push(ag("steer-before-prompt", {
		script: [
			{ op: "steer", message: agUser("steered", 1500) },
			{ op: "prompt", messages: [agUser("hi", 1000)] },
		],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// followUp queued before the prompt: drained when the loop would stop,
	// driving an extra provider turn.
	c.push(ag("follow-up-pre-queued", {
		script: [
			{ op: "followUp", message: agUser("and then?", 1500) },
			{ op: "prompt", messages: [agUser("hi", 1000)] },
		],
		providerTurns: [agTurn(agMsg()), agTurn(agMsg({ timestamp: 5000 }))],
		nowMs: NOW,
	}));
	// continue from a toolResult tail (terminate ends the first run after
	// the tool batch) → runContinuation (agentLoopContinue).
	c.push(ag("continue-from-tool-result", {
		tools: [agTool("read", { execute: { script: [{ perfNow: 1050, return: { content: [{ type: "text", text: "stop" }], details: {}, terminate: true } }] } })],
		script: [
			{ op: "prompt", messages: [agUser("read x", 1000)] },
			{ op: "continue" },
		],
		providerTurns: [
			agTurn(agMsg({ content: [agToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			agTurn(agMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// continue from an assistant tail with steering queued: the steering is
	// drained and run as a prompt with skipInitialSteeringPoll.
	c.push(ag("continue-from-assistant-steering", {
		script: [
			{ op: "prompt", messages: [agUser("hi", 1000)] },
			{ op: "steer", message: agUser("steered", 1500) },
			{ op: "continue" },
		],
		providerTurns: [agTurn(agMsg()), agTurn(agMsg({ timestamp: 5000 }))],
		nowMs: NOW,
	}));
	// continue from an assistant tail with follow-ups queued: the follow-ups
	// are drained and run as a prompt.
	c.push(ag("continue-from-assistant-follow-up", {
		script: [
			{ op: "prompt", messages: [agUser("hi", 1000)] },
			{ op: "followUp", message: agUser("and then?", 1500) },
			{ op: "continue" },
		],
		providerTurns: [agTurn(agMsg()), agTurn(agMsg({ timestamp: 5000 }))],
		nowMs: NOW,
	}));
	// continue from an assistant tail with nothing queued → thrown.
	c.push(ag("continue-assistant-nothing-queued", {
		script: [
			{ op: "prompt", messages: [agUser("hi", 1000)] },
			{ op: "continue" },
			{ op: "getState" },
		],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// continue on an empty state → thrown "No messages to continue from".
	c.push(ag("continue-empty-state", {
		script: [{ op: "continue" }],
		providerTurns: [],
		nowMs: NOW,
	}));
	// continue when every message is a system message → thrown.
	c.push(ag("continue-all-system", {
		initialState: { messages: [{ role: "system", content: "only system", timestamp: 1 }] },
		script: [{ op: "continue" }, { op: "getState" }],
		providerTurns: [],
		nowMs: NOW,
	}));
	// Re-entrancy: prompt settle:false then prompt → the second throws the
	// byte-exact re-entrancy error; after waitForIdle the state is
	// consistent (only the first prompt ran).
	c.push(ag("reentrant-prompt", {
		script: [
			{ op: "prompt", messages: [agUser("first", 1000)], settle: false },
			{ op: "prompt", messages: [agUser("second", 1000)] },
			{ op: "waitForIdle" },
			{ op: "getState" },
		],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// Re-entrancy with both prompts unsettled: the second start rejects and
	// its error is collected as a thrown entry once awaited at script end.
	c.push(ag("prompt-settle-false-rejection", {
		script: [
			{ op: "prompt", messages: [agUser("first", 1000)], settle: false },
			{ op: "prompt", messages: [agUser("second", 1000)], settle: false },
			{ op: "waitForIdle" },
			{ op: "getState" },
		],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// reset mid-run → thrown "Agent is already processing. Wait for
	// completion before resetting.".
	c.push(ag("reset-mid-run", {
		script: [
			{ op: "prompt", messages: [agUser("hi", 1000)], settle: false },
			{ op: "reset" },
			{ op: "waitForIdle" },
			{ op: "getState" },
		],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// reset when idle: messages reduced to the system baseline, queues
	// cleared.
	c.push(ag("reset-idle", {
		initialState: { systemPrompt: "base prompt" },
		script: [
			{ op: "prompt", messages: [agUser("hi", 1000)] },
			{ op: "steer", message: agUser("queued", 1500) },
			{ op: "followUp", message: agUser("later", 1500) },
			{ op: "reset" },
			{ op: "getState" },
		],
		providerTurns: [agTurn(agMsg())],
		nowMs: NOW,
	}));
	// Provider exhausted during prompt: handleRunFailure emits the synthetic
	// assistant error message (stopReason "error", errorMessage "provider
	// script exhausted"); the prompt itself still resolves (results null).
	c.push(ag("provider-exhausted", {
		script: [
			{ op: "prompt", messages: [agUser("hi", 1000)] },
			{ op: "getState" },
		],
		providerTurns: [],
		nowMs: NOW,
	}));
	// abort mid-run (tool abort step) + provider exhaustion while aborted:
	// the synthetic message carries stopReason "aborted".
	c.push(ag("abort-mid-run", {
		tools: [agTool("read", { execute: { script: [
			{ abort: true },
			{ perfNow: 1050, return: agOk() },
		] } })],
		script: [
			{ op: "prompt", messages: [agUser("read x", 1000)] },
			{ op: "getState" },
		],
		providerTurns: [
			agTurn(agMsg({ content: [agToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// steer injected mid-run by a tool execute step: agent.steer feeds the
	// Agent's steering queue, drained by the next poll (extra turn).
	c.push(ag("steer-tool-step-mid-run", {
		tools: [agTool("read", { execute: { script: [
			{ steer: agUser("steered mid-run", 1500) },
			{ perfNow: 1050, return: agOk() },
		] } })],
		script: [{ op: "prompt", messages: [agUser("read x", 1000)] }],
		providerTurns: [
			agTurn(agMsg({ content: [agToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			agTurn(agMsg({ timestamp: 4000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// followUp injected mid-run by a tool execute step: drained when the
	// loop would stop, driving a third provider turn.
	c.push(ag("follow-up-tool-step-mid-run", {
		tools: [agTool("read", { execute: { script: [
			{ followUp: agUser("follow up mid-run", 1500) },
			{ perfNow: 1050, return: agOk() },
		] } })],
		script: [{ op: "prompt", messages: [agUser("read x", 1000)] }],
		providerTurns: [
			agTurn(agMsg({ content: [agToolCall("call-1", "read", { path: "x" })], stopReason: "toolUse", timestamp: 2000 })),
			agTurn(agMsg({ timestamp: 4000 })),
			agTurn(agMsg({ timestamp: 6000 })),
		],
		perfSeq: [1000],
		nowMs: NOW,
	}));
	// Protocol-misuse cases: in-band {error} with byte-identical messages.
	c.push(ag("err-missing-script", { providerTurns: [], nowMs: NOW }));
	c.push(ag("err-missing-provider-turns", { script: [], nowMs: NOW }));
	c.push(ag("err-missing-now-ms", { script: [], providerTurns: [] }));
	c.push(ag("err-initial-state-not-object", { script: [], providerTurns: [], nowMs: NOW, initialState: "nope" }));

	return c.filter(Boolean);
})(),
];

// config-value sub-case builder (hoisted; used by the cases above).
// `extra` merges optional description/headers fields into the args.
function cv(name, fn, config, env, extra = {}) {
	return {
		name: "config-value/" + name,
		tool: "config-value",
		args: { fn, config, env: env ?? {}, ...extra },
	};
}

// http-dispatcher sub-case builders (hoisted; used by the cases above).
function hdp(name, value) {
	return {
		name: "http-dispatcher/parse-" + name,
		tool: "http-dispatcher",
		args: { fn: "parseHttpIdleTimeoutMs", value },
	};
}
function hdf(name, timeoutMs) {
	return {
		name: "http-dispatcher/format-" + name,
		tool: "http-dispatcher",
		args: { fn: "formatHttpIdleTimeoutMs", timeoutMs },
	};
}

// models-store sub-case builders (hoisted; used by the cases above).
// Entry is a pi-ai ModelsStoreEntry: {models, lastModified?, checkedAt?, etag?}.
function msEntry(o = {}) {
	return {
		// Key order matches the ModelsStoreEntry interface (and the Rust
		// port's to_value emission) so file-content strings compare equal.
		models: o.models ?? [msModel()],
		...(o.lastModified !== undefined ? { lastModified: o.lastModified } : {}),
		...(o.checkedAt !== undefined ? { checkedAt: o.checkedAt } : {}),
		...(o.etag !== undefined ? { etag: o.etag } : {}),
	};
}

// Full chat-model shape (all fields the typed Rust port round-trips), in
// the Model interface's field order.
function msModel(o = {}) {
	return {
		id: o.id ?? "model-a",
		name: o.name ?? "Model A",
		api: o.api ?? "anthropic",
		provider: o.provider ?? "anthropic",
		baseUrl: o.baseUrl ?? "https://api.anthropic.com",
		input: o.input ?? ["text"],
		cost: o.cost ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		reasoning: o.reasoning ?? false,
		contextWindow: o.contextWindow ?? 128000,
		maxTokens: o.maxTokens ?? 16384,
	};
}
function msInMemory(name, steps) {
	return {
		name: "models-store/inMemory-" + name,
		tool: "models-store",
		args: { fn: "inMemory", steps },
	};
}
function msFile(name, path, initialContent, steps) {
	return {
		name: "models-store/file-" + name,
		tool: "models-store",
		args: { fn: "file", path, initialContent, steps },
	};
}

// runtime-credentials sub-case builders (hoisted; used by the cases above).
function rc(name, backing, steps) {
	return {
		name: "runtime-credentials/" + name,
		tool: "runtime-credentials",
		args: { fn: "overlay", backing, steps },
	};
}
function rcKey(providerId, key) {
	return { providerId, credential: { type: "api_key", key } };
}

// virtual-models sub-case builders (hoisted; used by the cases above).
function vmCase(name, args) {
	return {
		name: "virtual-models/" + name,
		tool: "virtual-models",
		args: { ...args },
	};
}
/** Structurally valid AssistantMessage with overridable fields. */
function vmAssistant(o = {}) {
	return {
		role: "assistant",
		content: [{ type: "text", text: o.text ?? "ok" }],
		api: o.api ?? "anthropic",
		provider: o.provider ?? "anthropic",
		model: o.model ?? "claude-sonnet-4",
		usage: {
			input: 10,
			output: 5,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 15,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: o.stopReason ?? "stop",
		timestamp: 1780000000000,
	};
}
function vmTs(n) {
	return `2026-02-01T00:00:${String(n).padStart(2, "0")}.000Z`;
}
function vmMsgEntry(n, message) {
	return { type: "message", id: "vm-e" + n, parentId: null, timestamp: vmTs(n), message };
}
function vmModelChange(n, provider, modelId) {
	return { type: "model_change", id: "vm-e" + n, parentId: null, timestamp: vmTs(n), provider, modelId };
}
function vmStateEntry(n, provider, modelId, state) {
	return {
		type: "custom",
		id: "vm-e" + n,
		parentId: null,
		timestamp: vmTs(n),
		customType: "pi.virtual-model-state",
		data: { provider, modelId, state },
	};
}
function vmModel(id, provider, api = "anthropic") {
	return { id, name: id, api, provider };
}

// tui-components sub-case builders (hoisted; used by the cases above).
function tc(name, component, opts = {}) {
	return {
		name,
		component,
		...(opts.widths ? { widths: opts.widths } : {}),
		...(opts.script ? { script: opts.script } : {}),
	};
}
function txt(text, px, py, bg) {
	return {
		type: "text",
		text,
		...(px !== undefined ? { paddingX: px } : {}),
		...(py !== undefined ? { paddingY: py } : {}),
		...(bg ? { bg } : {}),
	};
}
function ttxt(text, px, py) {
	return {
		type: "truncated-text",
		text,
		...(px !== undefined ? { paddingX: px } : {}),
		...(py !== undefined ? { paddingY: py } : {}),
	};
}
function spc(lines) {
	return { type: "spacer", ...(lines !== undefined ? { lines } : {}) };
}
function boxc(children, px, py, bg) {
	return {
		type: "box",
		children,
		...(px !== undefined ? { paddingX: px } : {}),
		...(py !== undefined ? { paddingY: py } : {}),
		...(bg ? { bg } : {}),
	};
}
function ent(component, o = {}) {
	return {
		component,
		...(o.basis !== undefined ? { basis: o.basis } : {}),
		...(o.grow !== undefined ? { grow: o.grow } : {}),
		...(o.shrink !== undefined ? { shrink: o.shrink } : {}),
		...(o.min !== undefined ? { minSize: o.min } : {}),
		...(o.max !== undefined ? { maxSize: o.max } : {}),
		...(o.visible !== undefined ? { visible: o.visible } : {}),
	};
}
function stk(type, entries, o = {}) {
	return { type, entries, ...(o.gap !== undefined ? { gap: o.gap } : {}), ...(o.align ? { align: o.align } : {}) };
}
function inp(o = {}) {
	return { type: "input", ...o };
}
function slist(items, o = {}) {
	return { type: "select-list", items, ...o };
}
function stlist(items, o = {}) {
	return { type: "settings-list", items, ...o };
}
function ld(o = {}) {
	return { type: "loader", ...o };
}
function svw(child, o = {}) {
	return { type: "scroll-view", child, ...o };
}
function mr(child) {
	return { type: "mouse-region", child, onMouse: "record" };
}
function press(...inputs) {
	return { inputs: [...inputs] };
}
function sitem(value, label, description) {
	return { value, label, ...(description !== undefined ? { description } : {}) };
}
function sit(id, label, currentValue, o = {}) {
	return { id, label, currentValue, ...o };
}

// tui-editor sub-case builders (hoisted; used by the tui-editor cases).
// `te` wraps one docs/T4_PLAN.md §6 case: the args object IS the case
// (passed to the harness as-is); `ky` expands key arguments into one
// {input} script op each; `pasteSeq` wraps content in bracketed-paste
// markers; `manyLines` builds multi-line paste/setText content.
function te(name, script, o = {}) {
	return {
		name: "tui-editor/" + name,
		tool: "tui-editor",
		args: {
			name,
			group: "tui-editor",
			theme: "default",
			...(o.width !== undefined ? { width: o.width } : {}),
			...(o.options ? { options: o.options } : {}),
			...(o.history ? { history: o.history } : {}),
			...(o.setText !== undefined ? { setText: o.setText } : {}),
			...(o.focused !== undefined ? { focused: o.focused } : {}),
			...(o.disableSubmit !== undefined ? { disableSubmit: o.disableSubmit } : {}),
			script,
		},
	};
}
function ky(...inputs) {
	const script = [];
	for (const input of inputs) {
		if (typeof input === "string") script.push({ input });
		else script.push(input);
	}
	return script;
}
function pasteSeq(content) {
	return "\x1b[200~" + content + "\x1b[201~";
}
function manyLines(n) {
	return Array.from({ length: n }, (_, i) => "line " + (i + 1)).join("\n");
}

// provider-attribution sub-case builders (hoisted; see the
// provider-attribution section). Only provider+baseUrl of the model matter,
// so paModel reuses the full msModel chat-model shape.
function paCase(name, model, telemetryEnabled, o = {}) {
	return {
		name: "provider-attribution/" + name,
		tool: "provider-attribution",
		args: {
			fn: "mergeProviderAttributionHeaders",
			model,
			telemetryEnabled,
			...(o.sessionId !== undefined ? { sessionId: o.sessionId } : {}),
			...(o.headerSources !== undefined ? { headerSources: o.headerSources } : {}),
		},
	};
}
function paModel(o = {}) {
	return msModel({ provider: o.provider ?? "anthropic", baseUrl: o.baseUrl ?? "https://api.anthropic.com" });
}

// model-config sub-case builder (hoisted; see the model-config section).
// path is a label passed through verbatim into error strings (null = no
// modelsJsonPath); content is the raw file text; ioError simulates a
// non-ENOENT read failure. Null content + null ioError = ENOENT.
function mcCase(name, path, content, ioError) {
	return { name: "model-config/" + name, tool: "model-config", args: { fn: "load", path, content, ioError } };
}

// model-resolver sub-case builders (hoisted; see the model-resolver section).
// mrCatalog builds the synthetic catalog shared by the resolver cases:
// same id on two providers (claude-sonnet-4-5), alias vs dated ids, ids with
// "/" and ":" (gateway/openrouter style), a bracketed id, and name-substring
// fuzzy targets. Models use the full msModel chat-model shape.
function mrCatalog() {
	const defs = [
		["claude-sonnet-4-5", "anthropic", "Claude Sonnet 4.5"],
		["claude-sonnet-4-5-20250929", "anthropic", "Claude Sonnet 4.5 (dated)"],
		["claude-opus-4-8", "anthropic", "Claude Opus 4.8"],
		["claude-sonnet-4-5", "openai", "Sonnet Custom"],
		["gpt-5.5", "openai", "GPT-5.5"],
		["moonshotai/kimi-k2.6", "openrouter", "Kimi K2.6"],
		["qwen/qwen3-coder:exacto", "openrouter", "Qwen3 Coder"],
		["zai/glm-5", "gateway", "GLM-5"],
		["bracketed-model[1m]", "myprov", "Bracketed"],
		["aab", "myprov", "AAB"],
		["abc", "myprov", "ABC"],
		["mystery-1", "myprov", "Zenith Special"],
		["myprov/aab", "other", "Prefixed aab"],
		["kimi-for-coding", "moonshotai", "Kimi for coding"],
	];
	return defs.map((d) => msModel({ id: d[0], provider: d[1], name: d[2] }));
}
function mrFind(id, provider) {
	for (const model of mrCatalog()) {
		if (model.id === id && model.provider === provider) return model;
	}
	throw new Error("mrFind: no catalog model " + provider + "/" + id);
}
function mrCase(name, args) {
	return { name: "model-resolver/" + name, tool: "model-resolver", args };
}
function mrRef(name, text, o = {}) {
	return mrCase("ref-" + name, {
		fn: "resolveModelReference",
		text,
		...(o.provider !== undefined ? { provider: o.provider } : {}),
		thinkingSuffix: o.thinkingSuffix ?? "none",
		fuzzy: o.fuzzy ?? false,
		models: o.models ?? mrCatalog(),
		usable: o.usable ?? [],
	});
}
function mrExact(name, modelReference) {
	return mrCase("exact-" + name, { fn: "findExactModelReferenceMatch", modelReference, models: mrCatalog() });
}
function mrScope(name, patterns) {
	return mrCase("scope-" + name, { fn: "resolveModelScopeFromModels", patterns, models: mrCatalog() });
}
function mrCli(name, o = {}) {
	return mrCase("cli-" + name, {
		fn: "resolveCliModel",
		...(o.cliProvider !== undefined ? { cliProvider: o.cliProvider } : {}),
		...(o.cliModel !== undefined ? { cliModel: o.cliModel } : {}),
		...(o.cliThinking !== undefined ? { cliThinking: o.cliThinking } : {}),
		...(o.inlineApiKey !== undefined ? { inlineApiKey: o.inlineApiKey } : {}),
		models: o.models ?? mrCatalog(),
		configuredAuth: o.configuredAuth ?? [],
	});
}
function mrInitial(name, o = {}) {
	return mrCase("initial-" + name, {
		fn: "findInitialModel",
		scopedModels: o.scopedModels ?? [],
		isContinuing: o.isContinuing ?? false,
		...(o.defaultProvider !== undefined ? { defaultProvider: o.defaultProvider } : {}),
		...(o.defaultModelId !== undefined ? { defaultModelId: o.defaultModelId } : {}),
		...(o.defaultThinkingLevel !== undefined ? { defaultThinkingLevel: o.defaultThinkingLevel } : {}),
		...(o.modelThinkingLevels !== undefined ? { modelThinkingLevels: o.modelThinkingLevels } : {}),
		models: o.models ?? mrCatalog(),
		availableModels: o.availableModels ?? [],
		configuredAuth: o.configuredAuth ?? [],
	});
}
function mrRestore(name, o = {}) {
	return mrCase("restore-" + name, {
		fn: "restoreModelFromSession",
		savedProvider: o.savedProvider,
		savedModelId: o.savedModelId,
		currentModel: o.currentModel ?? null,
		models: o.models ?? mrCatalog(),
		availableModels: o.availableModels ?? [],
		configuredAuth: o.configuredAuth ?? [],
	});
}
function mrScoped(model, thinkingLevel) {
	return { model, ...(thinkingLevel !== undefined ? { thinkingLevel } : {}) };
}

// =========================================================== agent-session-retry
const asrr = (name, args) => ({
	name: "agent-session-retry/" + name,
	tool: "agent-session-retry",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const asrRetry = (o = {}) => ({
	enabled: o.enabled ?? true,
	maxRetries: o.maxRetries ?? 3,
	baseDelayMs: o.baseDelayMs ?? 4,
	maxAgentDelayMs: o.maxAgentDelayMs ?? 100,
});
const asrErrTurn = (errorMessage) => aspTurn(aspMsg({ stopReason: "error", errorMessage }));
cases.push(
	asrr("missing-args", {}),
	asrr("retry-success", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry() } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asrErrTurn("overloaded_error"), aspTurn(aspMsg({ content: [{ type: "text", text: "recovered" }] }))],
	}),
	asrr("retry-multi-error-then-success", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry({ baseDelayMs: 4 }) } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asrErrTurn("overloaded_error"), asrErrTurn("500 Internal"), aspTurn(aspMsg())],
	}),
	asrr("retry-exhausted", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry({ maxRetries: 2, baseDelayMs: 2 }) } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asrErrTurn("overloaded_error"), asrErrTurn("overloaded_error"), asrErrTurn("overloaded_error")],
	}),
	asrr("retry-disabled", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry({ enabled: false }) } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asrErrTurn("overloaded_error")],
	}),
	asrr("non-retryable-error", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry() } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asrErrTurn("invalid_api_key")],
	}),
	// NOTE: context-overflow is covered by
	// agent-session-compaction/context-overflow-not-retried: TS routes it
	// through _checkCompaction's overflow-recovery path (_omitRecoveryAttempt
	// + _runAutoCompaction), ported in tranche 13.
	asrr("empty-error-message-not-retried", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry() } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [aspTurn(aspMsg({ stopReason: "error", errorMessage: "" }))],
	}),
	asrr("backoff-capped-at-max-agent-delay", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry({ maxRetries: 5, baseDelayMs: 1, maxAgentDelayMs: 5 }) } },
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [
			asrErrTurn("overloaded_error"), asrErrTurn("overloaded_error"), asrErrTurn("overloaded_error"),
			asrErrTurn("overloaded_error"), asrErrTurn("overloaded_error"), aspTurn(aspMsg()),
		],
	}),
	asrr("backoff-safe-integer-cap", {
		nowMs: ASP_NOW,
		config: {
			model: aspModel(),
			entries: aspHeader(),
			settings: { retry: asrRetry({ maxRetries: 1, baseDelayMs: 1e300, maxAgentDelayMs: 1e300 }) },
		},
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asrErrTurn("overloaded_error"), aspTurn(aspMsg())],
	}),
	asrr("abort-retry-mid-sleep", {
		nowMs: ASP_NOW,
		sleepMode: "pending",
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry({ baseDelayMs: 100 }) } },
		script: [
			{ op: "promptStart", text: "hello" },
			{ op: "waitEvent", eventType: "auto_retry_start" },
			{ op: "abortRetry" },
			{ op: "awaitPrompt" },
		],
		providerTurns: [asrErrTurn("overloaded_error")],
	}),
	asrr("abort-mid-sleep", {
		nowMs: ASP_NOW,
		sleepMode: "pending",
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry({ baseDelayMs: 100 }) } },
		script: [
			{ op: "promptStart", text: "hello" },
			{ op: "waitEvent", eventType: "auto_retry_start" },
			{ op: "abortTap" },
			{ op: "awaitPrompt" },
		],
		providerTurns: [asrErrTurn("overloaded_error")],
	}),
	asrr("unknown-op", {
		nowMs: ASP_NOW,
		config: { model: aspModel(), entries: aspHeader(), settings: { retry: asrRetry() } },
		script: [{ op: "bogus" }],
		providerTurns: [],
	}),
);

// ======================================================= agent-session-compaction
// The compaction slice of AgentSession (manual compact(), _checkCompaction
// overflow/threshold dispatch, _runAutoCompaction, abortCompaction) over a
// scripted provider. The same providerTurns script drives agent turns and
// the summarizer model call. Seeded entries mirror the TS suite's
// seedCompactableSession: a user/assistant pair with keepRecentTokens low
// enough that prepareCompaction finds a summarizable range.
const asc = (name, args) => ({
	name: "agent-session-compaction/" + name,
	tool: "agent-session-compaction",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASC_NOW = 1747000000000;
const ascIso = (ms) => new Date(ms).toISOString();
const ascHeader = () => [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }];
const ascModel = (o = {}) => ({
	id: "m-asc",
	name: "Asc Model",
	api: "anthropic",
	provider: "anthropic",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: o.contextWindow ?? 0,
	maxTokens: o.maxTokens ?? 0,
});
const ascUsage = (o = {}) => ({
	input: o.input ?? 0,
	output: o.output ?? 0,
	cacheRead: o.cacheRead ?? 0,
	cacheWrite: o.cacheWrite ?? 0,
	totalTokens: o.totalTokens ?? 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
});
const ascMsg = (o = {}) => ({
	role: "assistant",
	content: o.content ?? [{ type: "text", text: "done" }],
	api: "anthropic",
	provider: "anthropic",
	model: "m-asc",
	usage: o.usage ?? ascUsage(),
	stopReason: o.stopReason ?? "stop",
	...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
	...(o.timestamp !== undefined ? { timestamp: o.timestamp } : {}),
});
const ascTurn = (result, events = []) => ({ events, result });
const ascCompaction = (o = {}) => ({
	enabled: o.enabled ?? true,
	reserveTokens: o.reserveTokens ?? 16384,
	keepRecentTokens: o.keepRecentTokens ?? 20000,
});
const ascUserEntry = (id, parentId, text, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: ascIso(ms),
	message: { role: "user", content: [{ type: "text", text }], timestamp: ms },
});
const ascAsstEntry = (id, parentId, message, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: ascIso(ms),
	message,
});
// header + user + zero-usage assistant (compactable with keepRecentTokens 1).
const ascSeed = (o = {}) => [
	...ascHeader(),
	ascUserEntry("e-user", "s-root", o.userText ?? "message to compact", ASC_NOW - 2000),
	ascAsstEntry(
		"e-asst",
		"e-user",
		o.assistant ?? ascMsg({ stopReason: "stop", content: [{ type: "text", text: "assistant response to compact" }], timestamp: ASC_NOW - 1000 }),
		ASC_NOW - 1000,
	),
];
const ascSummarizerTurn = () =>
	ascTurn(ascMsg({ content: [{ type: "text", text: "Session summary." }], usage: ascUsage({ input: 7, output: 11, totalTokens: 18 }) }));
const ascOverflowErr = (ms) =>
	ascTurn(ascMsg({ stopReason: "error", errorMessage: "prompt is too long", usage: ascUsage(), timestamp: ms }));
cases.push(
	asc("missing-args", {}),
	asc("invalid-scenario", {
		nowMs: ASC_NOW,
		config: { model: ascModel(), entries: ascHeader() },
		scenario: "bogus",
		script: [],
		providerTurns: [],
	}),
	asc("unknown-op", {
		nowMs: ASC_NOW,
		config: { model: ascModel(), entries: ascSeed(), settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) } },
		scenario: "manual",
		script: [{ op: "bogus" }],
		providerTurns: [],
	}),
	// Manual happy path: compaction_start reason manual, the summarizer
	// consumes the first provider turn, the compaction entry is appended,
	// compactionResult carries summary/firstKeptEntryId/tokensBefore/
	// estimatedTokensAfter/usage/details, and compaction_end fires.
	asc("manual-happy", {
		nowMs: ASC_NOW,
		config: { model: ascModel(), entries: ascSeed(), settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) } },
		scenario: "manual",
		script: [{ op: "compact" }],
		providerTurns: [ascSummarizerTurn()],
	}),
	// customInstructions reach the summarizer prompt ("Additional focus:").
	asc("manual-custom-instructions", {
		nowMs: ASC_NOW,
		config: { model: ascModel(), entries: ascSeed(), settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) } },
		scenario: "manual",
		script: [{ op: "compact", customInstructions: "Focus on the migration." }],
		providerTurns: [ascSummarizerTurn()],
	}),
	asc("manual-already-compacted", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel(),
			entries: [
				...ascSeed(),
				{
					type: "compaction",
					id: "c-old",
					parentId: "e-asst",
					timestamp: ascIso(ASC_NOW - 500),
					summary: "old summary",
					firstKeptEntryId: "e-asst",
					tokensBefore: 10,
					fromHook: false,
				},
			],
			settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) },
		},
		scenario: "already-compacted",
		script: [{ op: "compact" }],
		providerTurns: [],
	}),
	asc("manual-too-small", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel(),
			entries: [...ascHeader(), ascUserEntry("e-user", "s-root", "only message", ASC_NOW - 1000)],
			settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) },
		},
		scenario: "too-small",
		script: [{ op: "compact" }],
		providerTurns: [],
	}),
	// Threshold compaction after a big-context response: usage-backed
	// context tokens cross window - reserve, compaction_start reason
	// "threshold", compaction_end willRetry false, no retry turn.
	asc("threshold-after-big-usage", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel({ contextWindow: 200 }),
			entries: ascSeed(),
			settings: { compaction: ascCompaction({ reserveTokens: 50, keepRecentTokens: 1 }) },
		},
		scenario: "threshold",
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [
			ascTurn(ascMsg({ content: [{ type: "text", text: "big response" }], usage: ascUsage({ input: 100, totalTokens: 175 }), timestamp: ASC_NOW })),
			ascSummarizerTurn(),
		],
	}),
	// Overflow with retry: the failed assistant is omitted via a context_edit
	// entry, compaction runs, and the retried turn's provider context
	// (promptsSentToAgent[2]) excludes the omitted failed message.
	asc("overflow-retry-recovers", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel({ contextWindow: 200 }),
			entries: ascSeed(),
			settings: {
				retry: { enabled: true, maxRetries: 2, baseDelayMs: 50, maxAgentDelayMs: 100 },
				compaction: ascCompaction({ reserveTokens: 50, keepRecentTokens: 1 }),
			},
		},
		scenario: "overflow-retry",
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [
			ascOverflowErr(ASC_NOW),
			ascSummarizerTurn(),
			ascTurn(ascMsg({ content: [{ type: "text", text: "recovered" }], usage: ascUsage({ input: 5, totalTokens: 9 }), timestamp: ASC_NOW + 5000 })),
		],
	}),
	// Second overflow after one recovery: byte-exact compaction_end
	// errorMessage and a session_compact_failed extension event, no second
	// compaction.
	asc("overflow-recovery-already-attempted", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel({ contextWindow: 200 }),
			entries: ascSeed(),
			settings: {
				retry: { enabled: true, maxRetries: 2, baseDelayMs: 50, maxAgentDelayMs: 100 },
				compaction: ascCompaction({ reserveTokens: 50, keepRecentTokens: 1 }),
			},
		},
		scenario: "overflow-retry",
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [ascOverflowErr(ASC_NOW), ascSummarizerTurn(), ascOverflowErr(ASC_NOW + 5000)],
		extensionHandlers: { session_compact_failed: [{}] },
	}),
	// Overflow without retry: a completed response whose usage exceeds the
	// context window compacts but preserves the response (no retry turn).
	asc("overflow-complete-no-retry", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel({ contextWindow: 200 }),
			entries: ascSeed(),
			settings: { compaction: ascCompaction({ reserveTokens: 50, keepRecentTokens: 1 }) },
		},
		scenario: "overflow-complete",
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [
			ascTurn(ascMsg({ content: [{ type: "text", text: "huge response" }], usage: ascUsage({ input: 250, totalTokens: 260 }), timestamp: ASC_NOW })),
			ascSummarizerTurn(),
		],
	}),
	// session_before_compact {cancel: true}: "Compaction cancelled",
	// compaction_end aborted true, no errorMessage.
	asc("extension-cancel", {
		nowMs: ASC_NOW,
		config: { model: ascModel(), entries: ascSeed(), settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) } },
		scenario: "manual",
		script: [{ op: "compact" }],
		providerTurns: [],
		extensionHandlers: { session_before_compact: [{ cancel: true }] },
	}),
	// session_before_compact {compaction: {...}}: the extension result is
	// used verbatim (fromExtension entry, no summarizer provider turn).
	asc("extension-custom-compaction", {
		nowMs: ASC_NOW,
		config: { model: ascModel(), entries: ascSeed(), settings: { compaction: ascCompaction({ keepRecentTokens: 1 }) } },
		scenario: "manual",
		script: [{ op: "compact" }],
		providerTurns: [],
		extensionHandlers: {
			session_before_compact: [
				{
					compaction: {
						summary: "summary from extension",
						firstKeptEntryId: "e-asst",
						tokensBefore: 42,
						usage: ascUsage({ input: 10, output: 20, totalTokens: 30 }),
						details: { source: "extension" },
					},
				},
			],
		},
	}),
	// Abort mid-compaction: the summarizer retry sleep parks (sleepMode
	// "pending"), abortCompaction cancels it, compaction_end is aborted
	// with no errorMessage, and no compaction entry is appended.
	asc("abort-mid-compaction", {
		nowMs: ASC_NOW,
		sleepMode: "pending",
		config: {
			model: ascModel({ contextWindow: 200 }),
			entries: ascSeed(),
			settings: {
				retry: { enabled: true, maxRetries: 2, baseDelayMs: 100, maxAgentDelayMs: 200 },
				compaction: ascCompaction({ reserveTokens: 50, keepRecentTokens: 1 }),
			},
		},
		scenario: "overflow-retry",
		script: [
			{ op: "promptStart", text: "hello" },
			{ op: "waitEvent", eventType: "compaction_start" },
			{ op: "abortCompaction" },
			{ op: "awaitPrompt" },
		],
		providerTurns: [
			ascOverflowErr(ASC_NOW),
			ascTurn(ascMsg({ stopReason: "error", errorMessage: "overloaded_error", usage: ascUsage() })),
		],
	}),
	// Stale pre-compaction usage must not retrigger compaction: the kept
	// assistant predates the compaction entry, so the usage-backed estimate
	// is skipped even though its input tokens exceed the threshold.
	asc("stale-usage-no-retrigger", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel({ contextWindow: 1000 }),
			entries: [
				ascUserEntry("e-user", "s-root", "message to compact", ASC_NOW - 3000),
				ascAsstEntry(
					"e-asst",
					"e-user",
					ascMsg({ stopReason: "stop", content: [{ type: "text", text: "kept response" }], usage: ascUsage({ input: 5000, totalTokens: 5000 }), timestamp: ASC_NOW - 2000 }),
					ASC_NOW - 2000,
				),
				{
					type: "compaction",
					id: "c-old",
					parentId: "e-asst",
					timestamp: ascIso(ASC_NOW - 1000),
					summary: "old summary",
					firstKeptEntryId: "e-asst",
					tokensBefore: 5000,
					fromHook: false,
				},
			],
			settings: { compaction: ascCompaction({ reserveTokens: 10, keepRecentTokens: 1 }) },
		},
		scenario: "threshold",
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [ascTurn(ascMsg({ content: [{ type: "text", text: "small response" }], usage: ascUsage(), timestamp: ASC_NOW }))],
	}),
	// Deferred from tranche 12: a context-overflow error is not retried via
	// the auto-retry path; it routes through _checkCompaction's
	// overflow-recovery (_omitRecoveryAttempt context_edit entry). With
	// keepRecentTokens too large to leave a summarizable range,
	// prepareCompaction returns null, so no compaction events fire.
	asc("context-overflow-not-retried", {
		nowMs: ASC_NOW,
		config: {
			model: ascModel({ contextWindow: 200 }),
			entries: ascSeed(),
			settings: {
				retry: { enabled: true, maxRetries: 3, baseDelayMs: 50, maxAgentDelayMs: 100 },
				compaction: ascCompaction({ reserveTokens: 50, keepRecentTokens: 1000000000 }),
			},
		},
		scenario: "overflow-retry",
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [ascOverflowErr(ASC_NOW)],
	}),
);

// ======================================================= agent-session-model
// The model & thinking-management slice of AgentSession (setModel /
// cycleModel / setThinkingLevel / cycleThinkingLevel) plus the
// prepareRequest projection with virtual-model routing (resolveModel),
// over a scripted provider. The models list feeds getAvailableSnapshot
// and setModel targets; resolveScript feeds the scripted router.
const asmo = (name, args) => ({
	name: "agent-session-model/" + name,
	tool: "agent-session-model",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASMO_NOW = 1748000000000;
const asmoIso = (ms) => new Date(ms).toISOString();
const asmoHeader = () => [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }];
const asmoModel = (o = {}) => ({
	id: o.id ?? "m-a",
	name: o.name ?? "Model A",
	api: o.api ?? "anthropic",
	provider: o.provider ?? "anthropic",
	baseUrl: "",
	reasoning: o.reasoning ?? false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: o.contextWindow ?? 100000,
	maxTokens: o.maxTokens ?? 8192,
	...(o.thinkingLevelMap !== undefined ? { thinkingLevelMap: o.thinkingLevelMap } : {}),
});
const asmoUsage = () => ({
	input: 3,
	output: 5,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 8,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
});
const asmoMsg = (o = {}) => ({
	role: "assistant",
	content: o.content ?? [{ type: "text", text: "ok" }],
	api: o.api ?? "anthropic",
	provider: o.provider ?? "anthropic",
	model: o.model ?? "m-a",
	usage: o.usage ?? asmoUsage(),
	stopReason: o.stopReason ?? "stop",
	...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
	timestamp: o.timestamp ?? ASMO_NOW,
});
const asmoTurn = (result, events = []) => ({ events, result });
cases.push(
	asmo("missing-args", {}),
	asmo("unknown-op", {
		nowMs: ASMO_NOW,
		config: { model: asmoModel(), entries: asmoHeader() },
		models: [asmoModel()],
		script: [{ op: "bogus" }],
	}),
	// setModel happy path: model_change entry, thinking clamp to the new
	// model's supported levels, thinking_level_changed session event, and
	// the model_select / thinking_level_select extension events.
	asmo("setmodel-happy", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel({ id: "m-a", name: "Model A" }),
			entries: asmoHeader(),
		},
		models: [
			asmoModel({ id: "m-a", name: "Model A" }),
			asmoModel({
				id: "m-b",
				name: "Model B",
				reasoning: true,
				thinkingLevelMap: { off: null, minimal: "b-min", low: "b-low", medium: "b-med" },
			}),
		],
		script: [{ op: "setModel", modelIndex: 1 }],
	}),
	// setModel throws before any mutation when auth is not configured.
	asmo("setmodel-no-api-key", {
		nowMs: ASMO_NOW,
		config: { model: asmoModel(), entries: asmoHeader() },
		models: [asmoModel(), asmoModel({ id: "m-b", name: "Model B" })],
		authDenied: ["anthropic"],
		script: [{ op: "setModel", modelIndex: 1 }],
	}),
	// setModel persist: global default model/provider recorded; the empty
	// scoped-models list leaves enabledModels untouched.
	asmo("setmodel-persist", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel(),
			entries: asmoHeader(),
			settings: { enabledModels: ["anthropic/m-a"] },
		},
		models: [asmoModel(), asmoModel({ id: "m-b", name: "Model B" })],
		script: [{ op: "setModel", modelIndex: 1, persist: true }],
	}),
	// setModel persist with a non-empty scope: the persisted model joins
	// scopedModels and enabledModels gains its reference.
	asmo("setmodel-persist-scoped", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel(),
			entries: asmoHeader(),
			scopedModels: [{ model: asmoModel() }],
			settings: { enabledModels: ["anthropic/m-a"] },
		},
		models: [asmoModel(), asmoModel({ id: "m-b", name: "Model B" })],
		script: [{ op: "setModel", modelIndex: 1, persist: true }],
	}),
	// cycleModel forward / wrap / backward over the available snapshot.
	asmo("cyclemodel-wrap", {
		nowMs: ASMO_NOW,
		config: { model: asmoModel(), entries: asmoHeader() },
		models: [
			asmoModel({ id: "m-a", name: "Model A" }),
			asmoModel({ id: "m-b", name: "Model B" }),
			asmoModel({ id: "m-c", name: "Model C" }),
		],
		script: [
			{ op: "cycleModel" },
			{ op: "cycleModel", direction: "forward" },
			{ op: "cycleModel", direction: "forward" },
			{ op: "cycleModel", direction: "backward" },
		],
	}),
	// A single available model cannot cycle.
	asmo("cyclemodel-single", {
		nowMs: ASMO_NOW,
		config: { model: asmoModel(), entries: asmoHeader() },
		models: [asmoModel()],
		script: [{ op: "cycleModel" }],
	}),
	// Scoped cycling filters to models present in the available snapshot.
	asmo("cyclemodel-scoped", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel(),
			entries: asmoHeader(),
			scopedModels: [
				{ model: asmoModel() },
				{ model: asmoModel({ id: "m-b", name: "Model B", reasoning: true }) },
				{ model: asmoModel({ id: "m-unavailable", name: "Unavailable" }) },
			],
		},
		models: [asmoModel(), asmoModel({ id: "m-b", name: "Model B", reasoning: true })],
		script: [
			{ op: "cycleModel" },
			{ op: "cycleModel", direction: "backward" },
		],
	}),
	// setThinkingLevel clamps unsupported levels to the nearest supported
	// one (forward scan from the requested level, then backward).
	asmo("setthinkinglevel-clamp", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel({
				id: "m-think",
				name: "Think",
				reasoning: true,
				thinkingLevelMap: { off: null, minimal: "t-min", low: "t-low", medium: "t-med" },
			}),
			entries: asmoHeader(),
		},
		models: [asmoModel({ id: "m-think", name: "Think", reasoning: true })],
		script: [
			{ op: "setThinkingLevel", level: "off" },
			{ op: "setThinkingLevel", level: "max" },
		],
	}),
	// A no-op setThinkingLevel persists nothing and records no entry.
	asmo("setthinkinglevel-no-change", {
		nowMs: ASMO_NOW,
		config: { model: asmoModel(), entries: asmoHeader() },
		models: [asmoModel()],
		script: [{ op: "setThinkingLevel", level: "off" }],
	}),
	// setThinkingLevel persist records the raw requested level.
	asmo("setthinkinglevel-persist", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel({ id: "m-think", name: "Think", reasoning: true }),
			entries: asmoHeader(),
		},
		models: [asmoModel({ id: "m-think", name: "Think", reasoning: true })],
		script: [{ op: "setThinkingLevel", level: "high", persist: true }],
	}),
	// cycleThinkingLevel walks the supported levels and wraps; models
	// without reasoning support return undefined.
	asmo("cyclethinkinglevel", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel({ id: "m-think", name: "Think", reasoning: true }),
			entries: asmoHeader(),
		},
		models: [
			asmoModel({ id: "m-think", name: "Think", reasoning: true }),
			asmoModel({ id: "m-plain", name: "Plain" }),
		],
		script: [
			{ op: "cycleThinkingLevel" },
			{ op: "cycleThinkingLevel" },
			{ op: "cycleThinkingLevel" },
			{ op: "cycleThinkingLevel" },
			{ op: "cycleThinkingLevel" },
			{ op: "setModel", modelIndex: 1 },
			{ op: "cycleThinkingLevel" },
		],
	}),
	// Virtual-model routing: a prompt through a virtual model resolves
	// with reason "user", appends the router state entry, and streams with
	// the routed physical model.
	asmo("virtual-model-route", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel({ id: "m-virt", name: "Virt", api: "pi-virtual", provider: "virtual" }),
			entries: asmoHeader(),
		},
		models: [asmoModel({ id: "m-virt", name: "Virt", api: "pi-virtual", provider: "virtual" })],
		resolveScript: [
			{
				model: asmoModel({ id: "m-phys", name: "Phys" }),
				thinkingLevel: "low",
				state: { turn: 1 },
			},
		],
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [asmoTurn(asmoMsg({ model: "m-phys", content: [{ type: "text", text: "routed" }] }))],
	}),
	// A failed routed response sets _failedResponse; the retried request
	// resolves with reason "retry" and the failed message attached.
	asmo("virtual-model-retry-reason", {
		nowMs: ASMO_NOW,
		config: {
			model: asmoModel({ id: "m-virt", name: "Virt", api: "pi-virtual", provider: "virtual" }),
			entries: asmoHeader(),
		},
		models: [asmoModel({ id: "m-virt", name: "Virt", api: "pi-virtual", provider: "virtual" })],
		resolveScript: [
			{ model: asmoModel({ id: "m-phys", name: "Phys" }), thinkingLevel: "off" },
			{ model: asmoModel({ id: "m-phys", name: "Phys" }), thinkingLevel: "off" },
		],
		script: [{ op: "prompt", text: "hello" }],
		providerTurns: [
			asmoTurn(asmoMsg({ model: "m-phys", stopReason: "error", errorMessage: "overloaded" })),
			asmoTurn(asmoMsg({ model: "m-phys", content: [{ type: "text", text: "recovered" }] })),
		],
	}),
);

// ======================================================= agent-session-bash
// The bash slice of AgentSession (executeBash / recordBashResult /
// abortBash / isBashRunning / hasPendingBashMessages /
// _flushPendingBashMessages) over a scripted provider and scripted
// BashOperations. bashScript feeds the operations fake (one entry per
// exec call; `delay` steps yield on the harness event loop; `abortAfter`
// steps fire session.abortBash()).
const asbh = (name, args) => ({
	name: "agent-session-bash/" + name,
	tool: "agent-session-bash",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASBH_NOW = 1748000000000;
const asbhHeader = () => [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }];
const asbhModel = {
	id: "m-a",
	name: "Model A",
	api: "anthropic",
	provider: "anthropic",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: 100000,
	maxTokens: 8192,
};
const asbhMsg = (o = {}) => ({
	role: "assistant",
	content: o.content ?? [{ type: "text", text: "ok" }],
	api: "anthropic",
	provider: "anthropic",
	model: o.model ?? "m-a",
	usage: {
		input: 3,
		output: 5,
		cacheRead: 0,
		cacheWrite: 0,
		totalTokens: 8,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	},
	stopReason: o.stopReason ?? "stop",
	...(o.errorMessage !== undefined ? { errorMessage: o.errorMessage } : {}),
	timestamp: ASBH_NOW,
});
cases.push(
	// Arg validation mirrors the Rust entry byte-for-byte.
	asbh("missing-args", {}),
	asbh("missing-bashscript", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		script: [],
	}),
	asbh("invalid-settings", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader(), settings: { shellCommandPrefix: 5 } },
		bashScript: [],
		script: [],
	}),
	asbh("invalid-bashscript", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ chunks: "nope" }],
		script: [],
	}),
	asbh("unknown-op", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [],
		script: [{ op: "bogus" }],
	}),
	asbh("awaitbash-no-pending", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [],
		script: [{ op: "awaitBash" }],
	}),
	asbh("invalid-result", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [],
		script: [{ op: "recordBashResult", command: "c", result: { output: 5 } }],
	}),
	// executeBash happy path: streamed bash_execution_update chunks (with
	// id), the user onChunk callback, the recorded bashExecution entry, and
	// controller cleanup (isBashRunning false afterwards).
	asbh("execute-happy", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ chunks: [{ data: "hello " }, { data: "world\n", delay: true }], exitCode: 0 }],
		script: [
			{ op: "executeBash", command: "echo hi", id: "b1", recordChunks: true },
			{ op: "isBashRunning" },
			{ op: "hasPendingBash" },
		],
	}),
	// excludeFromContext (!! prefix semantics): the recorded bashExecution
	// message carries excludeFromContext so convertToLlm skips it.
	asbh("exclude-from-context", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ output: "secret\n", exitCode: 0 }],
		script: [{ op: "executeBash", command: "c", excludeFromContext: true }],
	}),
	// Concurrent executions with ids: two bashStarts interleave on the
	// harness event loop (single-threaded FIFO both sides); awaitAllBash
	// settles both by input index with allSettled semantics.
	asbh("concurrent-ids", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [
			{ chunks: [{ data: "a1" }, { data: "a2", delay: true }], exitCode: 0 },
			{
				chunks: [{ data: "b1", delay: true }, { data: "b2", delay: true }, { data: "b3" }],
				exitCode: 1,
			},
		],
		script: [
			{ op: "bashStart", command: "one", id: "b1" },
			{ op: "bashStart", command: "two", id: "b2" },
			{ op: "awaitAllBash" },
			{ op: "isBashRunning" },
		],
	}),
	// abortBash mid-execution: the signal is aborted while the exec is
	// suspended at a delay; the remaining steps still stream, and the
	// final result is cancelled with exitCode dropped.
	asbh("abort-mid-execution", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [
			{ chunks: [{ data: "x", delay: true }, { data: "y" }, { data: "z", delay: true }], exitCode: 0 },
		],
		script: [
			{ op: "bashStart", command: "long", id: "b1" },
			{ op: "abortBash" },
			{ op: "awaitBash" },
			{ op: "isBashRunning" },
		],
	}),
	// abortAfter fires session.abortBash() from inside the exec; the
	// aborted exec still returns its partial output.
	asbh("abort-after-step", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ chunks: [{ data: "a" }, { data: "b", abortAfter: true }, { data: "c" }], exitCode: 0 }],
		script: [{ op: "executeBash", command: "c", id: "b1" }],
	}),
	// shellCommandPrefix: the operations fake sees the prefixed command
	// while the recorded message keeps the original command.
	asbh("prefix-application", {
		nowMs: ASBH_NOW,
		config: {
			model: asbhModel,
			entries: asbhHeader(),
			settings: { shellCommandPrefix: "shopt -s expand_aliases" },
		},
		bashScript: [{ output: "ok\n", exitCode: 0 }],
		script: [{ op: "executeBash", command: "myalias" }],
	}),
	// A throwing script: the error propagates out of executeBash, the
	// controller is cleaned up, and no bashExecution entry is recorded.
	asbh("script-throws", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ output: "partial", throws: "kaboom" }],
		script: [
			{ op: "executeBash", command: "c" },
			{ op: "isBashRunning" },
			{ op: "hasPendingBash" },
		],
	}),
	// Pending-flush ordering: while streaming, recordBashResult queues the
	// message instead of appending; flushPending drains it in order.
	asbh("pending-flush-ordering", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ output: "queued\n", exitCode: 0 }],
		script: [
			{ op: "setStreaming", value: true },
			{ op: "executeBash", command: "c1" },
			{ op: "hasPendingBash" },
			{ op: "flushPending" },
			{ op: "hasPendingBash" },
		],
	}),
	// The _startRun checkpoint flushes queued bash messages before the
	// agent turn appends its own messages.
	asbh("pending-flush-on-prompt", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [{ output: "queued\n", exitCode: 0 }],
		providerTurns: [{ events: [], result: asbhMsg() }],
		script: [
			{ op: "setStreaming", value: true },
			{ op: "executeBash", command: "c1" },
			{ op: "setStreaming", value: false },
			{ op: "prompt", text: "go" },
			{ op: "waitForIdle" },
			{ op: "hasPendingBash" },
		],
	}),
	// recordBashResult direct effects: message shape with exitCode omitted
	// when absent and fullOutputPath included when present.
	asbh("record-result-direct", {
		nowMs: ASBH_NOW,
		config: { model: asbhModel, entries: asbhHeader() },
		bashScript: [],
		script: [
			{ op: "recordBashResult", command: "c1", result: { output: "o1", cancelled: true } },
			{
				op: "recordBashResult",
				command: "c2",
				result: { output: "o2", exitCode: 2, fullOutputPath: "/tmp/pi-bash-x.log" },
			},
			{ op: "hasPendingBash" },
		],
	}),
);

// ======================================================= agent-session-tree
// The tree-navigation slice of AgentSession (navigateTree /
// abortBranchSummary / getUserMessagesForForking) over a scripted provider
// (the same providerTurns script drives the branch-summary summarizer
// model call). extensionHandlers configures the fake extension runner
// (session_before_tree handlers may cancel, provide a summary, override
// instructions/label, or abort the in-flight branch summary).
const asth = (name, args) => ({
	name: "agent-session-tree/" + name,
	tool: "agent-session-tree",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASTH_NOW = 1749000000000;
const asthIso = (ms) => new Date(ms).toISOString();
const asthHeader = () => [{ type: "session", id: "s-root", version: 3, cwd: "/proj" }];
const asthModel = {
	id: "m-asth",
	name: "Tree Model",
	api: "anthropic",
	provider: "anthropic",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: 100000,
	maxTokens: 8192,
};
const asthUsage = (o = {}) => ({
	input: o.input ?? 0,
	output: o.output ?? 0,
	cacheRead: o.cacheRead ?? 0,
	cacheWrite: o.cacheWrite ?? 0,
	totalTokens: o.totalTokens ?? 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
});
const asthUserEntry = (id, parentId, text, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: asthIso(ms),
	message: { role: "user", content: [{ type: "text", text }], timestamp: ms },
});
const asthAsstEntry = (id, parentId, text, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: asthIso(ms),
	message: { role: "assistant", content: [{ type: "text", text }], timestamp: ms },
});
// s-root -> u1 -> a1 -> u2 -> a2 (leaf) with sibling a1b under u1.
const asthSeed = () => [
	...asthHeader(),
	asthUserEntry("u1", "s-root", "first question", ASTH_NOW - 5000),
	asthAsstEntry("a1", "u1", "first answer", ASTH_NOW - 4000),
	asthAsstEntry("a1b", "u1", "alternate answer", ASTH_NOW - 3500),
	asthUserEntry("u2", "a1", "second question", ASTH_NOW - 3000),
	asthAsstEntry("a2", "u2", "second answer", ASTH_NOW - 2000),
];
const asthSummarizerTurn = () => ({
	events: [],
	result: {
		role: "assistant",
		content: [{ type: "text", text: "Branch summary." }],
		api: "anthropic",
		provider: "anthropic",
		model: "m-asth",
		usage: asthUsage({ input: 9, output: 13, totalTokens: 22 }),
		stopReason: "stop",
		timestamp: ASTH_NOW,
	},
});
cases.push(
	asth("missing-args", {}),
	asth("missing-script", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthHeader() },
	}),
	asth("unknown-op", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "bogus" }],
		providerTurns: [],
	}),
	asth("missing-targetid", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", options: {} }],
		providerTurns: [],
	}),
	// Navigating to the current leaf is a no-op: {cancelled:false} with no
	// extension events and no entry changes.
	asth("navigate-noop", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "a2" }],
		providerTurns: [],
	}),
	// Navigate to an ancestor user message: editorText is restored from the
	// message content, new leaf is the user entry's parent, session_tree
	// event fires.
	asth("navigate-to-ancestor-user", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "u2", options: { summarize: false } }],
		providerTurns: [],
	}),
	// Navigate to a sibling branch (assistant entry): leaf becomes the
	// target entry itself, no editorText.
	asth("navigate-to-sibling-assistant", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "a1b", options: { summarize: false } }],
		providerTurns: [],
	}),
	// Navigate to the root user with summarize:true: the branch summarizer
	// consumes one provider turn and a branch_summary entry is appended.
	asth("branch-summary-generation", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "u1", options: { summarize: true, label: "focus" } }],
		providerTurns: [asthSummarizerTurn()],
	}),
	// Custom instructions flow into the summarizer prompt.
	asth("branch-summary-custom-instructions", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [
			{
				op: "navigateTree",
				targetId: "u1",
				options: { summarize: true, customInstructions: "Emphasize decisions." },
			},
		],
		providerTurns: [asthSummarizerTurn()],
	}),
	// A session_before_tree handler aborts the in-flight branch summary.
	asth("branch-summary-abort", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "u1", options: { summarize: true } }],
		providerTurns: [asthSummarizerTurn()],
		extensionHandlers: { session_before_tree: [{ abortBranchSummary: true }] },
	}),
	// A session_before_tree handler cancels navigation outright.
	asth("extension-cancel", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "u1", options: { summarize: false } }],
		providerTurns: [],
		extensionHandlers: { session_before_tree: [{ cancel: true }] },
	}),
	// A session_before_tree handler supplies the summary (no provider call).
	asth("extension-summary", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "u1", options: { summarize: true } }],
		providerTurns: [],
		extensionHandlers: {
			session_before_tree: [
				{
					summary: {
						summary: "Hook summary.",
						details: { readFiles: ["/proj/a.ts"], modifiedFiles: ["/proj/b.ts"] },
						usage: asthUsage({ input: 4, output: 6, totalTokens: 10 }),
					},
					label: "hooked",
				},
			],
		},
	}),
	asth("invalid-target", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "nope" }],
		providerTurns: [],
	}),
	asth("navigate-while-streaming", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [
			{ op: "setStreaming", value: true },
			{ op: "navigateTree", targetId: "u1", options: { summarize: false } },
			{ op: "isCompacting" },
		],
		providerTurns: [],
	}),
	asth("no-model-summarize", {
		nowMs: ASTH_NOW,
		config: { model: null, entries: asthSeed() },
		script: [{ op: "navigateTree", targetId: "u1", options: { summarize: true } }],
		providerTurns: [],
	}),
	asth("user-messages-for-forking", {
		nowMs: ASTH_NOW,
		config: { model: asthModel, entries: asthSeed() },
		script: [
			{ op: "getUserMessagesForForking" },
			{ op: "isCompacting" },
			{ op: "waitForIdle" },
		],
		providerTurns: [],
	}),
);

// ======================================================= agent-session-stats
// The stats & exports slice of AgentSession (getSessionStats /
// getContextUsage / exportToJsonl / summarizeForBugReport) over a scripted
// provider. config.auth feeds the fake modelRuntime.getAuth (summarization
// request auth); config.physicalModels feeds getPhysicalModel (the
// routedModel seam under a virtual selection). exportToJsonl files are
// written under the harness cwd, read back, and removed (idempotent).
const assh = (name, args) => ({
	name: "agent-session-stats/" + name,
	tool: "agent-session-stats",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASSH_NOW = 1749000000000;
const asshIso = (ms) => new Date(ms).toISOString();
const asshHeader = () => [{ type: "session", id: "s-stats", version: 3, cwd: "/proj" }];
const asshModel = {
	id: "m-stats",
	name: "Stats Model",
	api: "anthropic",
	provider: "anthropic",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: 100000,
	maxTokens: 8192,
};
const asshVirtualModel = {
	id: "m-virtual",
	name: "Virtual Model",
	api: "pi-virtual",
	provider: "pi",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: 100000,
	maxTokens: 8192,
};
const asshUsage = (o = {}) => ({
	input: o.input ?? 0,
	output: o.output ?? 0,
	cacheRead: o.cacheRead ?? 0,
	cacheWrite: o.cacheWrite ?? 0,
	totalTokens: o.totalTokens ?? 0,
	cost: {
		input: o.costInput ?? 0,
		output: o.costOutput ?? 0,
		cacheRead: 0,
		cacheWrite: 0,
		total: (o.costInput ?? 0) + (o.costOutput ?? 0),
	},
});
const asshUserEntry = (id, parentId, text, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: asshIso(ms),
	message: { role: "user", content: [{ type: "text", text }], timestamp: ms },
});
const asshAsstEntry = (id, parentId, blocks, ms, usage) => ({
	type: "message",
	id,
	parentId,
	timestamp: asshIso(ms),
	message: {
		role: "assistant",
		content: Array.isArray(blocks) ? blocks : [{ type: "text", text: blocks }],
		api: "anthropic",
		provider: "anthropic",
		model: "m-stats",
		usage: usage ?? asshUsage(),
		stopReason: "stop",
		timestamp: ms,
	},
});
const asshToolResultEntry = (id, parentId, ms, usage) => ({
	type: "message",
	id,
	parentId,
	timestamp: asshIso(ms),
	message: {
		role: "toolResult",
		content: [{ type: "text", text: "tool output" }],
		toolCallId: "call-" + id,
		usage: usage,
		timestamp: ms,
	},
});
const asshUsageEntry = (id, parentId, ms, usage) => ({
	type: "usage",
	id,
	parentId,
	timestamp: asshIso(ms),
	provider: "anthropic",
	model: "m-stats",
	usage,
});
const asshTurn = (text, o = {}) => ({
	events: [],
	result: {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "anthropic",
		provider: "anthropic",
		model: "m-stats",
		usage: o.usage ?? asshUsage(),
		stopReason: o.stopReason ?? "stop",
		errorMessage: o.errorMessage,
		timestamp: ASSH_NOW,
	},
});
cases.push(
	assh("missing-args", {}),
	assh("missing-script", {
		nowMs: ASSH_NOW,
		config: { model: asshModel, entries: asshHeader() },
	}),
	assh("unknown-op", {
		nowMs: ASSH_NOW,
		config: { model: asshModel, entries: asshHeader() },
		script: [{ op: "bogus" }],
		providerTurns: [],
	}),
	// Empty session (header only): all-zero stats, no contextUsage (no
	// model), no sessionFile (in-memory).
	assh("stats-empty", {
		nowMs: ASSH_NOW,
		config: { entries: asshHeader() },
		script: [{ op: "getSessionStats" }, { op: "getContextUsage" }],
		providerTurns: [],
	}),
	// Mixed entries: user/assistant/toolResult/bashExecution/contextEdit/
	// compaction — only the three counted roles increment; toolCalls counts
	// toolCall blocks; bashExecution and context_edit are ignored.
	assh("stats-mixed-entries", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hello", ASSH_NOW - 9000),
				asshAsstEntry(
					"a1",
					"u1",
					[
						{ type: "text", text: "running" },
						{ type: "toolCall", id: "call-1", name: "read", arguments: {} },
						{ type: "toolCall", id: "call-2", name: "write", arguments: {} },
					],
					ASSH_NOW - 8000,
					asshUsage({ input: 10, output: 20, totalTokens: 30, costInput: 0.1, costOutput: 0.2 })
				),
				asshToolResultEntry("t1", "a1", ASSH_NOW - 7000, asshUsage({ input: 5, output: 0, totalTokens: 5, costInput: 0.05 })),
				{
					type: "message",
					id: "b1",
					parentId: "t1",
					timestamp: asshIso(ASSH_NOW - 6500),
					message: {
						role: "bashExecution",
						content: "ls -la",
						command: "ls -la",
						timestamp: ASSH_NOW - 6500,
					},
				},
				{
					type: "context_edit",
					id: "ce1",
					parentId: "b1",
					timestamp: asshIso(ASSH_NOW - 6000),
					targetId: "t1",
					replacement: { content: "edited tool output" },
				},
				asshUserEntry("u2", "ce1", "again", ASSH_NOW - 5000),
				asshAsstEntry("a2", "u2", "done", ASSH_NOW - 4000, asshUsage({ input: 7, output: 3, totalTokens: 10, costInput: 0.07, costOutput: 0.03 })),
			],
		},
		script: [{ op: "getSessionStats" }],
		providerTurns: [],
	}),
	// Token/cost aggregation across usage entries, toolResult usage, and a
	// branch_summary usage.
	assh("stats-token-aggregation", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "ok", ASSH_NOW - 8000, asshUsage({ input: 100, output: 50, cacheRead: 25, cacheWrite: 10, totalTokens: 185, costInput: 1, costOutput: 0.5 })),
				asshUsageEntry("us1", "a1", ASSH_NOW - 7500, asshUsage({ input: 40, output: 10, totalTokens: 50, costInput: 0.4, costOutput: 0.1 })),
				asshToolResultEntry("t1", "us1", ASSH_NOW - 7000, asshUsage({ input: 5, output: 5, totalTokens: 10, costInput: 0.05, costOutput: 0.05 })),
				{
					type: "branch_summary",
					id: "bs1",
					parentId: "t1",
					timestamp: asshIso(ASSH_NOW - 6000),
					summary: "prior turn",
					usage: asshUsage({ input: 8, output: 2, totalTokens: 10, costInput: 0.08, costOutput: 0.02 }),
				},
				asshUserEntry("u2", "bs1", "next", ASSH_NOW - 5000),
				asshAsstEntry("a2", "u2", "sure", ASSH_NOW - 4000, asshUsage({ input: 20, output: 10, totalTokens: 30, costInput: 0.2, costOutput: 0.1 })),
			],
		},
		script: [{ op: "getSessionStats" }],
		providerTurns: [],
	}),
	// An assistant message without usage throws exactly like the TS.
	assh("stats-missing-assistant-usage", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				{
					type: "message",
					id: "a1",
					parentId: "u1",
					timestamp: asshIso(ASSH_NOW - 8000),
					message: {
						role: "assistant",
						content: [{ type: "text", text: "no usage" }],
						api: "anthropic",
						provider: "anthropic",
						model: "m-stats",
						stopReason: "stop",
						timestamp: ASSH_NOW - 8000,
					},
				},
			],
		},
		script: [{ op: "getSessionStats" }],
		providerTurns: [],
	}),
	// Context usage from provider usage: tokens = the last assistant
	// totalTokens, percent = tokens/window*100.
	assh("context-usage-provider-usage", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "ok", ASSH_NOW - 8000, asshUsage({ input: 1000, output: 500, totalTokens: 1500 })),
				asshUserEntry("u2", "a1", "more", ASSH_NOW - 7000),
				asshAsstEntry("a2", "u2", "sure", ASSH_NOW - 6000, asshUsage({ input: 2000, output: 500, totalTokens: 2500 })),
			],
		},
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	// No model and zero context window both yield undefined (null).
	assh("context-usage-no-model", {
		nowMs: ASSH_NOW,
		config: { entries: asshHeader() },
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	assh("context-usage-zero-window", {
		nowMs: ASSH_NOW,
		config: {
			model: { ...asshModel, contextWindow: 0 },
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000)],
		},
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	// Right after a compaction with no post-compaction assistant usage:
	// tokens/percent null.
	assh("context-usage-after-compaction-unknown", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "ok", ASSH_NOW - 8000, asshUsage({ input: 1000, output: 500, totalTokens: 1500 })),
				{
					type: "compaction",
					id: "c1",
					parentId: "a1",
					timestamp: asshIso(ASSH_NOW - 7000),
					summary: "compacted",
				},
				asshUserEntry("u2", "c1", "next", ASSH_NOW - 6000),
			],
		},
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	// With a post-compaction assistant carrying usage, the estimate is
	// projected (compaction summary + post-compaction messages).
	assh("context-usage-after-compaction-projected", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "old context", ASSH_NOW - 8000, asshUsage({ input: 1000, output: 500, totalTokens: 1500 })),
				{
					type: "compaction",
					id: "c1",
					parentId: "a1",
					timestamp: asshIso(ASSH_NOW - 7000),
					summary: "compacted history summary text",
				},
				asshUserEntry("u2", "c1", "next", ASSH_NOW - 6000),
				asshAsstEntry("a2", "u2", "resumed", ASSH_NOW - 5000, asshUsage({ input: 300, output: 100, totalTokens: 400 })),
			],
		},
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	// Virtual selection: the limits model is the physical model of the
	// latest successful response (context window from the physical model).
	assh("context-usage-virtual-model", {
		nowMs: ASSH_NOW,
		config: {
			model: asshVirtualModel,
			physicalModels: [
				{
					provider: "anthropic",
					id: "m-stats",
					model: { ...asshModel, contextWindow: 50000 },
				},
			],
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "ok", ASSH_NOW - 8000, asshUsage({ input: 1000, output: 500, totalTokens: 1500 })),
			],
		},
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	// Virtual selection with no matching physical model: routedModel is
	// undefined, so the virtual model's own window applies.
	assh("context-usage-virtual-model-unresolved", {
		nowMs: ASSH_NOW,
		config: {
			model: asshVirtualModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "ok", ASSH_NOW - 8000, asshUsage({ input: 1000, output: 500, totalTokens: 1500 })),
			],
		},
		script: [{ op: "getContextUsage" }],
		providerTurns: [],
	}),
	// exportToJsonl with an explicit path: header + branch entries with
	// re-chained parentId, byte-exact.
	assh("export-jsonl-explicit-path", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "hello", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "world", ASSH_NOW - 8000, asshUsage({ input: 10, output: 5, totalTokens: 15 })),
				asshAsstEntry("a1b", "u1", "sibling", ASSH_NOW - 7500),
				asshUserEntry("u2", "a1", "more", ASSH_NOW - 7000),
			],
		},
		script: [{ op: "exportToJsonl", path: "stats-export-explicit.jsonl" }],
		providerTurns: [],
	}),
	// Default path: timestamped filename under the process cwd (pinned
	// date), resolved identically on both sides.
	assh("export-jsonl-default-path", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "only", ASSH_NOW - 9000)],
		},
		script: [{ op: "exportToJsonl" }],
		providerTurns: [],
	}),
	// summarizeForBugReport: full prompt (conversation + instructions) and
	// the request options (maxTokens, sessionId, cacheRetention) recorded.
	assh("bug-report-basic", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "it crashed when saving", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "I will save the file", ASSH_NOW - 8000, asshUsage({ input: 10, output: 5, totalTokens: 15 })),
			],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [asshTurn("# What the user was doing\nediting files")],
	}),
	// A user hint flows into <user-report>.
	assh("bug-report-hint", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [
				...asshHeader(),
				asshUserEntry("u1", "s-stats", "broken tool", ASSH_NOW - 9000),
				asshAsstEntry("a1", "u1", "running tool", ASSH_NOW - 8000, asshUsage({ totalTokens: 15 })),
			],
		},
		script: [{ op: "summarizeForBugReport", hint: "  the write tool hangs  " }],
		providerTurns: [asshTurn("report text")],
	}),
	// No model selected.
	assh("bug-report-no-model", {
		nowMs: ASSH_NOW,
		config: { entries: asshHeader() },
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [],
	}),
	// A tool-call response is rejected.
	assh("bug-report-tool-call", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000)],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [
			{
				events: [],
				result: {
					role: "assistant",
					content: [{ type: "toolCall", id: "call-9", name: "read", arguments: {} }],
					api: "anthropic",
					provider: "anthropic",
					model: "m-stats",
					usage: asshUsage(),
					stopReason: "stop",
					timestamp: ASSH_NOW,
				},
			},
		],
	}),
	// An empty summary is rejected.
	assh("bug-report-empty", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000)],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [asshTurn("   ")],
	}),
	// An aborted summarization is cancelled.
	assh("bug-report-aborted", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000)],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [asshTurn("partial", { stopReason: "aborted" })],
	}),
	// Retryable provider error: the first turn errors ("overloaded"), the
	// retry succeeds — two provider calls recorded.
	assh("bug-report-retry", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			settings: { retry: { enabled: true, maxRetries: 2, baseDelayMs: 100 } },
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000)],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [
			asshTurn("x", { stopReason: "error", errorMessage: "provider is overloaded" }),
			asshTurn("recovered report"),
		],
	}),
	// Auth (apiKey/headers/env/baseUrl): the request carries them and the
	// model's baseUrl is replaced.
	assh("bug-report-auth", {
		nowMs: ASSH_NOW,
		config: {
			model: asshModel,
			auth: {
				auth: { baseUrl: "https://proxy.example", apiKey: "sk-test", headers: { "x-custom": "1" } },
				env: { ANTHROPIC_API_KEY: "sk-test" },
			},
			entries: [...asshHeader(), asshUserEntry("u1", "s-stats", "hi", ASSH_NOW - 9000)],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [asshTurn("authed report")],
	}),
	// A small context window truncates the selection: the prompt carries
	// the "only the last N of M messages" note.
	assh("bug-report-truncated-selection", {
		nowMs: ASSH_NOW,
		config: {
			model: { ...asshModel, contextWindow: 2000 },
			entries: [
				...asshHeader(),
				...Array.from({ length: 8 }, (_, i) =>
					asshUserEntry("u" + (i + 1), i === 0 ? "s-stats" : "u" + i, "word ".repeat(200), ASSH_NOW - 10000 + i * 100)
				),
			],
		},
		script: [{ op: "summarizeForBugReport" }],
		providerTurns: [asshTurn("short report")],
	}),
);

// ======================================================= agent-session-runtime
// The session-replacement runtime (AgentSessionRuntime:
// switchSession / newSession / fork / importFromJsonl / dispose) over a
// factory building real AgentSessions. The initial session is
// <cwd>/pi-parity-asr/sessions/initial.jsonl (written from
// config.entries); `files` materializes further fixtures under the same
// base. The interleaved shutdownOrder trace records session events,
// extension emits, beforeInvalidate/invalidate, sessionStart:<reason>,
// rebind, and withSession, so replacement ordering is compared exactly.
const asrt = (name, args) => ({
	name: "agent-session-runtime/" + name,
	tool: "agent-session-runtime",
	args: {
		...args,
		config: { systemPrompt: "You are the parity assistant.", ...(args.config ?? {}) },
	},
});
const ASRT_NOW = 1760000000000;
const asrtIso = (ms) => new Date(ms).toISOString();
const asrtModel = {
	id: "m-asrt",
	name: "Runtime Model",
	api: "anthropic",
	provider: "anthropic",
	baseUrl: "",
	reasoning: false,
	input: [],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	contextWindow: 100000,
	maxTokens: 8192,
};
const asrtUsage = (o = {}) => ({
	input: o.input ?? 0,
	output: o.output ?? 0,
	cacheRead: o.cacheRead ?? 0,
	cacheWrite: o.cacheWrite ?? 0,
	totalTokens: o.totalTokens ?? 0,
	cost: {
		input: o.costInput ?? 0,
		output: o.costOutput ?? 0,
		cacheRead: 0,
		cacheWrite: 0,
		total: (o.costInput ?? 0) + (o.costOutput ?? 0),
	},
});
const asrtHeader = (o = {}) => ({
	type: "session",
	id: o.id ?? "s-asrt",
	version: 3,
	timestamp: asrtIso(o.ms ?? ASRT_NOW),
	cwd: o.cwd ?? "/tmp",
	...(o.parentSession !== undefined ? { parentSession: o.parentSession } : {}),
});
const asrtUser = (id, parentId, text, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: asrtIso(ms),
	message: { role: "user", content: [{ type: "text", text }], timestamp: ms },
});
const asrtAsst = (id, parentId, text, ms) => ({
	type: "message",
	id,
	parentId,
	timestamp: asrtIso(ms),
	message: {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "anthropic",
		provider: "anthropic",
		model: "m-asrt",
		usage: asrtUsage(),
		stopReason: "stop",
		timestamp: ms,
	},
});
const asrtSeed = () => [
	asrtHeader(),
	asrtUser("u1", "s-asrt", "hello there", ASRT_NOW - 9000),
	asrtAsst("a1", "u1", "hi back", ASRT_NOW - 8000),
	asrtUser("u2", "a1", "please fork", ASRT_NOW - 7000),
];
const asrtJsonl = (entries) => entries.map((e) => JSON.stringify(e)).join("\n") + "\n";
cases.push(
	// Harness validation.
	asrt("missing-args", {}),
	asrt("missing-script", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
	}),
	asrt("missing-entries", {
		nowMs: ASRT_NOW,
		config: { systemPrompt: "x", entries: [] },
		script: [],
		providerTurns: [],
	}),
	asrt("invalid-handlers", {
		nowMs: ASRT_NOW,
		config: { entries: asrtSeed() },
		script: [],
		providerTurns: [],
		extensionHandlers: { session_shutdown: "yes" },
	}),
	asrt("invalid-files", {
		nowMs: ASRT_NOW,
		config: { entries: asrtSeed() },
		script: [],
		providerTurns: [],
		files: { "sessions/x.jsonl": 5 },
	}),
	// Script-op validation (each throws into its results slot).
	asrt("op-validation", {
		nowMs: ASRT_NOW,
		config: { entries: asrtSeed() },
		script: [null, {}, ["x"], { op: "bogus" }, { op: "switchSession" }, { op: "fork" }, { op: "prompt" }, { op: "setRetryAttempt" }],
		providerTurns: [],
	}),
	// Switch to an existing session file: state loads, the old session is
	// torn down (shutdown emit -> beforeInvalidate -> invalidate), the new
	// runtime is created (sessionStart:resume) and rebound.
	asrt("switch-existing", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		files: {
			"sessions/other.jsonl": asrtJsonl([
				asrtHeader({ id: "s-other", ms: ASRT_NOW - 20000 }),
				asrtUser("ou1", "s-other", "other hello", ASRT_NOW - 19000),
				asrtAsst("oa1", "ou1", "other reply", ASRT_NOW - 18000),
			]),
		},
		script: [
			{ op: "switchSession", path: "pi-parity-asr/sessions/other.jsonl" },
			{ op: "getSessionId" },
		],
		providerTurns: [],
		extensionHandlers: { session_shutdown: true },
	}),
	// Switch with a before-switch handler present (non-cancelling) and a
	// withSession callback: emit precedes the shutdown trace, withSession
	// runs after rebind.
	asrt("switch-with-session", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		files: {
			"sessions/other.jsonl": asrtJsonl([
				asrtHeader({ id: "s-other", ms: ASRT_NOW - 20000 }),
				asrtUser("ou1", "s-other", "other hello", ASRT_NOW - 19000),
			]),
		},
		script: [{ op: "switchSession", path: "pi-parity-asr/sessions/other.jsonl", withSession: true }],
		providerTurns: [],
		extensionHandlers: { session_before_switch: true, session_shutdown: true },
	}),
	// A cancelling session_before_switch handler intercepts the switch:
	// no teardown, no new session, state unchanged.
	asrt("switch-cancelled", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		files: {
			"sessions/other.jsonl": asrtJsonl([asrtHeader({ id: "s-other", ms: ASRT_NOW - 20000 })]),
		},
		script: [{ op: "switchSession", path: "pi-parity-asr/sessions/other.jsonl" }],
		providerTurns: [],
		extensionHandlers: { session_before_switch: { cancel: true } },
	}),
	// A stored session cwd that does not exist throws the byte-exact
	// MissingSessionCwdError before teardown; a cwdOverride bypasses it.
	asrt("switch-missing-cwd", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		files: {
			"sessions/badcwd.jsonl": asrtJsonl([
				asrtHeader({ id: "s-bad", cwd: "/definitely-missing-asrt", ms: ASRT_NOW - 20000 }),
				asrtUser("bu1", "s-bad", "bad cwd", ASRT_NOW - 19000),
			]),
		},
		script: [
			{ op: "switchSession", path: "pi-parity-asr/sessions/badcwd.jsonl" },
			{ op: "switchSession", path: "pi-parity-asr/sessions/badcwd.jsonl", cwdOverride: "/tmp" },
		],
		providerTurns: [],
	}),
	// Switching while an auto-retry is pending: the teardown abort fires
	// auto_retry_end before session_shutdown.
	asrt("switch-retry-pending", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		files: {
			"sessions/other.jsonl": asrtJsonl([asrtHeader({ id: "s-other", ms: ASRT_NOW - 20000 })]),
		},
		script: [
			{ op: "setRetryAttempt", n: 2 },
			{ op: "switchSession", path: "pi-parity-asr/sessions/other.jsonl" },
		],
		providerTurns: [],
		extensionHandlers: { session_shutdown: true },
	}),
	// /new: a fresh persisted session (new uuid + pinned timestamp), the
	// withSession callback runs after rebind.
	asrt("new-session", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "newSession", withSession: true }, { op: "getSessionId" }],
		providerTurns: [],
		extensionHandlers: { session_shutdown: true },
	}),
	// /new with a parent session: the new header records parentSession.
	asrt("new-session-parent", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "newSession", parentSession: "/tmp/pi-parity-asr/sessions/initial.jsonl" }],
		providerTurns: [],
		extensionHandlers: { session_shutdown: true },
	}),
	// Fork before a user entry: selectedText is the user text, the branch
	// is re-chained to that entry's parent, and a before-fork handler is
	// emitted first.
	asrt("fork-before", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "fork", entryId: "u2" }],
		providerTurns: [],
		extensionHandlers: { session_before_fork: true, session_shutdown: true },
	}),
	// Fork at an entry: no selectedText, the branch keeps the entry.
	asrt("fork-at", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "fork", entryId: "u2", position: "at" }],
		providerTurns: [],
		extensionHandlers: { session_shutdown: true },
	}),
	// Forking an unknown id or a non-user entry (position before) throws
	// before any teardown.
	asrt("fork-invalid", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "fork", entryId: "nope" }, { op: "fork", entryId: "a1" }],
		providerTurns: [],
	}),
	// A cancelling session_before_fork handler intercepts the fork.
	asrt("fork-cancelled", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "fork", entryId: "u2" }],
		providerTurns: [],
		extensionHandlers: { session_before_fork: { cancel: true } },
	}),
	// importFromJsonl round-trip: the source is copied into the session
	// dir, switched to, and a second import of the same source collides
	// with the stored copy and takes a -1 suffix.
	asrt("import-round-trip", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		files: {
			"imports/remote.jsonl": asrtJsonl([
				asrtHeader({ id: "s-remote", ms: ASRT_NOW - 30000 }),
				asrtUser("ru1", "s-remote", "remote hello", ASRT_NOW - 29000),
			]),
		},
		script: [
			{ op: "importFromJsonl", path: "pi-parity-asr/imports/remote.jsonl" },
			{ op: "getSessionId" },
			{ op: "importFromJsonl", path: "pi-parity-asr/imports/remote.jsonl" },
		],
		providerTurns: [],
		extensionHandlers: { session_before_switch: true, session_shutdown: true },
	}),
	// Importing a missing file throws the byte-exact
	// SessionImportFileNotFoundError (resolved absolute path).
	asrt("import-missing-file", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "importFromJsonl", path: "pi-parity-asr/imports/nope.jsonl" }],
		providerTurns: [],
	}),
	// dispose: shutdown emit -> beforeInvalidate -> invalidate, session
	// state intact afterwards.
	asrt("dispose", {
		nowMs: ASRT_NOW,
		config: { model: asrtModel, entries: asrtSeed() },
		script: [{ op: "dispose" }, { op: "getSessionId" }],
		providerTurns: [],
		extensionHandlers: { session_shutdown: true },
	}),
);

// ======================================================= agent-session-services
// createAgentSessionServices: pending provider/native/virtual-model
// registration drain, refresh, and applyExtensionFlagValues over a
// recording fake ModelRuntime and inline extension factories. Diagnostics
// are compared byte-exact (unknown options, string-flag-requires-value,
// registration errors attributed to the inline extension path).
const assv = (name, args) => ({
	name: "agent-session-services/" + name,
	tool: "agent-session-services",
	args,
});
cases.push(
	// Harness validation.
	assv("missing-args", {}),
	assv("missing-cwd", { options: {} }),
	assv("invalid-flag-values", {
		options: { cwd: "/tmp/pi-parity-assv", extensionFlagValues: [["x", 5]] },
	}),
	assv("invalid-extension", {
		options: { cwd: "/tmp/pi-parity-assv" },
		extensions: [{ name: "ext-a", flags: [{ name: "f", type: "number" }] }],
	}),
	// Happy path: registrations drain in order (providers, native
	// providers, virtual models), refresh runs with allowNetwork false,
	// and flag values land in the runtime map (boolean flags coerce the
	// provided value to true).
	assv("happy-path", {
		options: {
			cwd: "/tmp/pi-parity-assv",
			agentDir: "/tmp/pi-parity-assv",
			extensionFlagValues: [
				["flagB", false],
				["flagS", "a value"],
			],
		},
		extensions: [
			{
				name: "ext-a",
				flags: [
					{ name: "flagB", type: "boolean" },
					{ name: "flagS", type: "string" },
				],
				providers: [{ name: "p1", config: { baseUrl: "https://p1.example" } }],
				nativeProviders: [{ name: "np1", type: "native" }],
				virtualModels: [{ id: "vm1", provider: "p1" }],
			},
		],
	}),
	// No extensions and no flags: only the refresh call is recorded.
	assv("empty", {
		options: { cwd: "/tmp/pi-parity-assv", agentDir: "/tmp/pi-parity-assv" },
	}),
	// Flag validation: a boolean value for a string flag requires a value;
	// unregistered names collect into the unknown-options diagnostic
	// (pluralized).
	assv("invalid-flags", {
		options: {
			cwd: "/tmp/pi-parity-assv",
			agentDir: "/tmp/pi-parity-assv",
			extensionFlagValues: [
				["x", true],
				["flagS", true],
				["also", false],
			],
		},
		extensions: [
			{
				name: "ext-a",
				flags: [
					{ name: "flagS", type: "string" },
					{ name: "flagB", type: "boolean" },
				],
			},
		],
	}),
	// A single unknown option stays singular.
	assv("unknown-flag-single", {
		options: {
			cwd: "/tmp/pi-parity-assv",
			agentDir: "/tmp/pi-parity-assv",
			extensionFlagValues: [["x", true]],
		},
	}),
	// Registration drain with every registration throwing: each error is
	// attributed to the inline extension path, refresh still runs, and the
	// pending lists are drained.
	assv("registration-errors", {
		options: { cwd: "/tmp/pi-parity-assv", agentDir: "/tmp/pi-parity-assv" },
		extensions: [
			{
				name: "ext-a",
				providers: [
					{ name: "p1", config: {} },
					{ name: "p2", config: { baseUrl: "https://p2.example" } },
				],
				nativeProviders: [{ name: "np1" }],
				virtualModels: [{ id: "vm1" }],
			},
			{
				name: "ext-b",
				providers: [{ name: "p3", config: {} }],
			},
		],
		modelRuntime: { throwOn: ["registerProvider", "registerNativeProvider", "registerVirtualModel"] },
	}),
);

// ---- extension-runner: loader + runner core slice (mirrors the ----------
// "extension-runner" entries in ts-harness.js and pi-cli
// extension_runner_harness.rs; factory scripts + op sequences).

// Helper: one extension-runner case from an op list.
function exr(name, ops) {
	return { tool: "extension-runner", name: "exr/" + name, args: { ops } };
}

cases.push(
	// -- load lifecycle ---------------------------------------------------
	exr("load-empty", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "hasHandlers", event: "session_start" },
	]),
	exr("load-factory-throw", [
		{ op: "load", path: "/ext/a.js", script: [{ step: "throw", message: "boom" }] },
		{ op: "createRunner" },
	]),
	exr("load-try-step", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "try", steps: [{ step: "throw", message: "inner failure" }] },
				{ step: "throw", message: "after try" },
			],
		},
	]),
	exr("load-pending-lifecycle", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerFlag", name: "verbose", options: { type: "boolean", default: true, description: "Verbose output" } },
				{ step: "getFlag", name: "verbose" },
				{ step: "getFlag", name: "nope" },
				{ step: "getMcpServers" },
				{ step: "try", steps: [{ step: "getActiveTools" }] },
				{ step: "registerMcpServer", name: "alpha", config: { command: "node" } },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{ op: "mcpList" },
		{ op: "getActiveTools" },
		{ op: "bindCore", actions: { activeTools: ["read", "bash"] } },
		{ op: "getFlagValues" },
		{ op: "getActiveTools" },
		{ op: "api", key: "a", steps: [{ step: "getFlag", name: "verbose" }, { step: "getActiveTools" }] },
	]),
	exr("load-throw-discards-pending", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "eventsOn", channel: "chan", handler: { id: 1 } },
				{ step: "registerFlag", name: "f", options: { type: "number", default: 1 } },
				{ step: "registerMcpServer", name: "s", config: { command: "x" } },
				{ step: "throw", message: "kaput" },
			],
		},
		{ op: "busEmit", channel: "chan", data: { n: 1 } },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "getFlagValues" },
		{ op: "mcpList" },
	]),

	// -- bindCore: model-registry forwarding --------------------------------
	exr("bindcore-flush-providers", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerProvider", name: "p1", config: { baseUrl: "https://p1.example", apiKey: "k" } },
				{ step: "registerProvider", name: "p2", config: { baseUrl: "https://p2.example" } },
				{ step: "unregisterProvider", name: "p2" },
				{ step: "registerProviderNative", provider: { id: "np1", name: "Native One" } },
				{ step: "registerVirtualModel", definition: { provider: "p1", id: "vm1", modelId: "m" } },
				{ step: "registerVirtualModel", definition: { provider: "p1", id: "vm2" } },
				{ step: "unregisterVirtualModel", provider: "p1", id: "vm2" },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
	]),
	exr("bindcore-flush-throwOn", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerProvider", name: "p1", config: { baseUrl: "https://p1.example" } },
				{ step: "registerVirtualModel", definition: { provider: "p1", id: "vm1" } },
			],
		},
		{ op: "createRunner", throwOn: ["registerProvider", "registerVirtualModel"], throwMsg: "registry down" },
		{ op: "onError" },
		{ op: "bindCore" },
	]),
	exr("bindcore-provider-hooks", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerProvider", name: "hp", config: { x: 1 } },
				{ step: "registerProviderNative", provider: { id: "np" } },
				{ step: "registerVirtualModel", definition: { provider: "hp", id: "v" } },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{
			op: "bindCore",
			providerActions: {
				registerProvider: "log",
				registerNativeProvider: "log",
				unregisterProvider: "log",
				registerVirtualModel: "log",
				unregisterVirtualModel: "log",
			},
		},
		{
			op: "api",
			key: "a",
			steps: [
				{ step: "unregisterProvider", name: "hp" },
				{ step: "registerProvider", name: "live", config: { y: 2 } },
				{ step: "registerVirtualModel", definition: { provider: "live", id: "lv" } },
				{ step: "unregisterVirtualModel", provider: "live", id: "lv" },
			],
		},
	]),
	exr("bindcore-provider-hooks-partial-fallback", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerProviderNative", provider: { id: "np" } },
				{ step: "registerVirtualModel", definition: { provider: "x", id: "v" } },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore", providerActions: { registerProvider: "log" } },
	]),
	exr("bindcore-hook-throw", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "registerProvider", name: "hp", config: { x: 1 } }],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore", providerActions: { registerProvider: "throw" } },
	]),
	exr("api-provider-immediate", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "captureApi", key: "a" }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{
			op: "api",
			key: "a",
			steps: [
				{ step: "registerProvider", name: "live", config: { y: 2 } },
				{ step: "registerProviderNative", provider: { id: "np2" } },
				{ step: "registerVirtualModel", definition: { provider: "live", id: "lv" } },
				{ step: "unregisterVirtualModel", provider: "live", id: "lv" },
				{ step: "unregisterProvider", name: "live" },
			],
		},
	]),
	exr("api-provider-immediate-throw", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "captureApi", key: "a" }],
		},
		{ op: "createRunner", throwOn: ["registerProvider", "unregisterProvider"] },
		{ op: "bindCore" },
		{
			op: "api",
			key: "a",
			steps: [
				{ step: "registerProvider", name: "live", config: {} },
				{ step: "unregisterProvider", name: "live" },
			],
		},
	]),

	// -- handlers / emit ----------------------------------------------------
	exr("has-handlers", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "evt", handler: { id: 1 } },
				{ step: "on", event: "evt", handler: { id: 2 } },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{ op: "hasHandlers", event: "evt" },
		{ op: "hasHandlers", event: "other" },
		{ op: "api", key: "a", steps: [{ step: "off", index: 0 }] },
		{ op: "api", key: "a", steps: [{ step: "off", index: 0 }] },
		{ op: "hasHandlers", event: "evt" },
		{ op: "api", key: "a", steps: [{ step: "off", index: 1 }] },
		{ op: "hasHandlers", event: "evt" },
	]),
	exr("emit-multi-handler-order", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "custom", handler: { id: 1, reads: ["cwd"] } },
				{ step: "on", event: "custom", handler: { id: 2 } },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "on", event: "custom", handler: { id: 3, reads: ["mode", "hasUI"] } }],
		},
		{ op: "createRunner" },
		{
			op: "bindCore",
			actions: { activeTools: [] },
			contextActions: { isIdle: true, isProjectTrusted: false, model: { providerId: "p", modelId: "m" } },
		},
		{ op: "emit", event: { type: "custom", payload: 1 } },
	]),
	exr("emit-pre-bind-context-defaults", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "custom", handler: { id: 1, reads: ["isIdle", "isProjectTrusted", "model", "thinkingLevel"] } },
			],
		},
		{ op: "createRunner" },
		{ op: "emit", event: { type: "custom" } },
	]),
	exr("emit-handler-throw", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "custom", handler: { id: 1, throws: "h1 blew up" } },
				{ step: "on", event: "custom", handler: { id: 2 } },
			],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emit", event: { type: "custom" } },
	]),
	exr("emit-no-handlers", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emit", event: { type: "nobody_listens" } },
	]),
	exr("emit-session-before-cancel", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "session_before_switch", handler: { id: 1, returns: { cancel: true } } },
				{ step: "on", event: "session_before_switch", handler: { id: 2 } },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "on", event: "session_before_fork", handler: { id: 3, returns: { entryId: "e1" } } }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emit", event: { type: "session_before_switch", sessionPath: "/x" } },
		{ op: "emit", event: { type: "session_before_fork", entryId: "e0" } },
		{ op: "emit", event: { type: "session_before_compact" } },
	]),
	exr("emit-message-end", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "message_end", handler: { id: 1, returns: { message: { role: "assistant", content: [{ type: "text", text: "changed" }] } } } },
				{ step: "on", event: "message_end", handler: { id: 2, returns: { message: { role: "user" } } } },
				{ step: "on", event: "message_end", handler: { id: 3 } },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "on", event: "message_end", handler: { id: 4, returns: { message: { role: "assistant", content: [{ type: "text", text: "final" }] } } } }],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitMessageEnd", event: { type: "message_end", message: { role: "assistant", content: [{ type: "text", text: "orig" }] } } },
	]),
	exr("emit-message-end-unmodified", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "message_end", handler: { id: 1 } }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitMessageEnd", event: { type: "message_end", message: { role: "assistant", content: [] } } },
	]),
	exr("error-listener-remove", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "custom", handler: { id: 1, throws: "x" } }],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "onError" },
		{ op: "removeErrorListener", id: 0 },
		{ op: "bindCore" },
		{ op: "emit", event: { type: "custom" } },
	]),

	// -- flags ---------------------------------------------------------------
	exr("flags", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerFlag", name: "alpha", options: { type: "string", default: "one", description: "First" } },
				{ step: "registerFlag", name: "beta", options: { type: "number" } },
				{ step: "captureApi", key: "a" },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [
				{ step: "registerFlag", name: "alpha", options: { type: "string", default: "two" } },
				{ step: "registerFlag", name: "gamma", options: { type: "boolean", default: false } },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "getFlags" },
		{ op: "setFlagValue", name: "beta", value: 7 },
		{ op: "getFlagValues" },
		{ op: "api", key: "a", steps: [{ step: "getFlag", name: "beta" }, { step: "getFlag", name: "gamma" }, { step: "getFlag", name: "alpha" }] },
		{ op: "setFlagValue", name: "delta", value: true },
		{
			op: "load",
			path: "/ext/c.js",
			script: [
				{ step: "registerFlag", name: "delta", options: { type: "boolean", default: false } },
				{ step: "getFlag", name: "delta" },
			],
		},
		{ op: "getFlagValues" },
	]),
	exr("flags-invalid-default", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "try", steps: [{ step: "registerFlag", name: "bad1", options: { type: "boolean", default: "nope" } }] },
				{ step: "try", steps: [{ step: "registerFlag", name: "bad2", options: { default: true } }] },
				{ step: "registerFlag", name: "ok", options: { type: "number", default: 5 } },
			],
		},
		{ op: "createRunner" },
		{ op: "getFlags" },
	]),

	// -- commands / tools -----------------------------------------------------
	exr("commands", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerCommand", name: "greet", options: { handler: true, description: "Say hi" } },
				{ step: "registerCommand", name: "run", options: { handler: true } },
				{ step: "try", steps: [{ step: "registerCommand", name: "", options: { handler: true } }] },
				{ step: "try", steps: [{ step: "registerCommand", name: "nohandler", options: {} }] },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "registerCommand", name: "greet", options: { handler: true, description: "Also hi" } }],
		},
		{ op: "createRunner" },
		{ op: "getRegisteredCommands" },
		{ op: "getCommand", name: "greet:2" },
		{ op: "getCommand", name: "greet" },
		{ op: "getCommand", name: "run" },
		{ op: "getCommand", name: "missing" },
	]),
	exr("tools", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerTool", tool: { name: "t1", description: "d", parameters: { type: "object", properties: {} } } },
				{ step: "try", steps: [{ step: "registerTool", tool: { name: "bad" } }] },
				{ step: "try", steps: [{ step: "registerTool", tool: { name: "bad2", parameters: [1] } }] },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore", actions: { activeTools: ["read"] } },
		{ op: "getActiveTools" },
		{ op: "api", key: "a", steps: [{ step: "registerTool", tool: { name: "t2", parameters: { type: "object" } } }] },
		{ op: "getActiveTools" },
	]),

	// -- shortcuts ------------------------------------------------------------
	exr("shortcuts", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "registerShortcut", shortcut: "ctrl+g", options: { description: "Go" } },
				{ step: "registerShortcut", shortcut: "ctrl+c", options: {} },
				{ step: "registerShortcut", shortcut: "ctrl+k", options: { description: "K" } },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "registerShortcut", shortcut: "ctrl+g", options: { description: "B go" } }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "getShortcuts", keybindings: { "app.interrupt": "ctrl+c", "app.custom.thing": "ctrl+k" } },
		{ op: "setUIContext", ui: true, mode: "interactive" },
		{ op: "getShortcuts", keybindings: {} },
	]),

	// -- MCP servers ------------------------------------------------------------
	exr("mcp-post-bind-register", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "captureApi", key: "a" }],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "captureApi", key: "b" }],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "api", key: "a", steps: [{ step: "registerMcpServer", name: "alpha", config: { command: "node", args: ["-e", ""] } }] },
		{ op: "mcpList" },
		{ op: "reportUnhandledMcpServers" },
		{ op: "api", key: "a", steps: [{ step: "registerMcpServer", name: "alpha", config: { command: "other" } }] },
		{ op: "mcpList" },
		{ op: "api", key: "b", steps: [{ step: "registerMcpServer", name: "beta", config: { command: "node" } }] },
		{ op: "api", key: "a", steps: [{ step: "unregisterMcpServer", name: "beta" }] },
		{ op: "mcpList" },
		{ op: "api", key: "a", steps: [{ step: "unregisterMcpServer", name: "alpha" }] },
		{ op: "mcpList" },
	]),
	exr("mcp-validation-errors", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "try", steps: [{ step: "registerMcpServer", name: "bad name!", config: { command: "x" } }] },
				{ step: "try", steps: [{ step: "registerMcpServer", name: "s1", config: { command: 123 } }] },
				{ step: "try", steps: [{ step: "registerMcpServer", name: "s2", config: { type: "sse", url: "https://x" } }] },
				{ step: "try", steps: [{ step: "registerMcpServer", name: "s3", config: { command: "x", args: [1] } }] },
				{ step: "try", steps: [{ step: "registerMcpServer", name: "s4", config: { url: "ftp://x" } }] },
				{ step: "captureApi", key: "a" },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "captureApi", key: "b" }],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "api", key: "a", steps: [{ step: "registerMcpServer", name: "my-srv", config: { command: "x" } }] },
		{ op: "api", key: "a", steps: [{ step: "registerMcpServer", name: "my_srv", config: { command: "x" } }] },
		{ op: "api", key: "b", steps: [{ step: "registerMcpServer", name: "my-srv", config: { command: "y" } }] },
		{ op: "mcpList" },
	]),
	exr("mcp-handled-change", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "mcp_servers_change", handler: { id: 1 } },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "api", key: "a", steps: [{ step: "registerMcpServer", name: "gamma", config: { command: "node" } }] },
		{ op: "mcpList" },
		{ op: "reportUnhandledMcpServers" },
	]),
	// Engine traces (Addendum B): two mutations in ONE op interleave
	// handler-by-handler (h1:A, h1:B, h2:A, h2:B — not h1,h2,h1,h2).
	exr("mcp-two-mutations-two-handlers", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "mcp_servers_change", handler: { id: 1 } },
				{ step: "on", event: "mcp_servers_change", handler: { id: 2 } },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{
			op: "api",
			key: "a",
			steps: [
				{ step: "registerMcpServer", name: "alpha", config: { command: "node" } },
				{ step: "registerMcpServer", name: "beta", config: { command: "node" } },
			],
		},
		{ op: "mcpList" },
	]),
	// Load-time mutation after bindCore: handler 1's body lands BEFORE the
	// load mark; handler 2 after it (h1, load:b:ok, h2).
	exr("mcp-late-load-interleave", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{
			op: "load",
			path: "/ext/b.js",
			script: [
				{ step: "on", event: "mcp_servers_change", handler: { id: 1 } },
				{ step: "on", event: "mcp_servers_change", handler: { id: 2 } },
				{ step: "registerMcpServer", name: "early", config: { command: "node" } },
			],
		},
	]),
	// ---- tranche 6: emitters -------------------------------------------
	exr("emit-context-rewrites", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "context", handler: { id: 1, returns: { messages: [{ role: "user", content: "new" }] } } },
			{ step: "on", event: "context_with_system", handler: { id: 2, eventReads: ["messages"] } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitContext", messages: [
			{ role: "system", content: "HEAD", timestamp: 5 },
			{ role: "user", content: "old" },
		] },
	]),
	exr("emit-context-lost-head", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "context_with_system", handler: { id: 1, returns: { messages: [{ role: "user", content: "only" }] } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitContext", messages: [{ role: "system", content: "S", timestamp: 1 }] },
	]),
	exr("emit-tool-call-block", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_call", handler: { id: 1, returns: { block: false, reason: "a" } } },
			{ step: "on", event: "tool_call", handler: { id: 2, returns: { block: true } } },
			{ step: "on", event: "tool_call", handler: { id: 3, returns: { block: false } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolCall", event: { type: "tool_call", toolName: "read" } },
	]),
	exr("emit-tool-call-throw", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_call", handler: { id: 1, throws: "tc boom" } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitToolCall", event: { type: "tool_call" } },
	]),
	exr("emit-tool-result-patch", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_result", handler: { id: 1, returns: { content: [{ type: "text", text: "new" }], isError: true } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolResult", event: {
			type: "tool_result", toolCallId: "c1",
			content: [{ type: "text", text: "old" }],
			details: { d: 1 }, structuredContent: { s: 1 }, isError: false, usage: { tokens: 1 },
		} },
	]),
	exr("emit-cache-warming-null", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "cache_warming_decision", handler: { id: 1, returns: { action: null } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitCacheWarmingDecision", event: { type: "cache_warming_decision", action: "warm" } },
	]),
	exr("emit-provider-hooks", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_provider_request", handler: { id: 1, returns: null } },
			{ step: "on", event: "before_provider_request", handler: { id: 2, eventReads: ["payload"], returns: { replaced: true } } },
			{ step: "on", event: "before_provider_headers", handler: { id: 3, mutate: [{ path: ["headers", "authorization"], value: "token" }], returns: { ignored: true } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeProviderRequest", payload: { p: 0 } },
		{ op: "emitBeforeProviderHeaders", headers: { accept: "a" } },
	]),
	exr("emit-resources-discover", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "resources_discover", handler: { id: 1, returns: { skillPaths: ["/s1"], themePaths: ["/t1"] } } },
			{ step: "on", event: "resources_discover", handler: { id: 2, returns: { skillPaths: ["/s2"] } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitResourcesDiscover", cwd: "/proj", reason: "startup" },
	]),
	exr("emit-input-transform", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "input", handler: { id: 1, eventReads: ["text", "images"], returns: { action: "transform", text: "hi!" } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitInput", text: "hi", source: "user" },
	]),
	exr("emit-user-bash-invalid", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "user_bash", handler: { id: 1, returns: { nope: 1 } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitUserBash", event: { command: "ls" } },
	]),
	exr("emit-user-bash-valid", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "user_bash", handler: { id: 1, returns: { result: { output: "o", exitCode: 0, cancelled: false, truncated: false } } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitUserBash", event: { command: "ls" } },
	]),
	// ---- tranche 6: before_agent_start / contexts / ui / actions --------
	exr("before-agent-start-merge", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_agent_start", handler: {
				id: 1,
				reads: ["cwd", "mode"],
				eventReads: ["systemPromptOptions.customPrompt", "systemPrompt"],
				returns: { message: { role: "user", content: "m" }, systemPrompt: "X" },
			} },
			{ step: "on", event: "before_agent_start", handler: {
				id: 2,
				calls: ["getSystemPrompt"],
				eventReads: ["systemPromptOptions.forceSystemPrompt"],
			} },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeAgentStart", trigger: { prompt: "p" }, systemPromptOptions: { cwd: "/proj", customPrompt: "CP" } },
	]),
	exr("before-agent-start-null-force", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_agent_start", handler: { id: 1, returns: { systemPrompt: null }, eventReads: ["systemPrompt"] } },
			// `event.systemPrompt` after a null force is the plan's documented
			// divergence (TS getter throws at read; the port omits the key).
			// The same V8 error is observable through ctx.getSystemPrompt.
			{ step: "on", event: "before_agent_start", handler: { id: 2, calls: ["getSystemPrompt"], eventReads: ["systemPromptOptions.forceSystemPrompt"] } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeAgentStart", trigger: {}, systemPromptOptions: { cwd: "/proj" } },
	]),
	exr("before-agent-start-normalize-throw", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeAgentStart", trigger: {}, systemPromptOptions: { selectedTools: 5 } },
	]),
	exr("before-agent-start-options-mutate", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_agent_start", handler: {
				id: 1,
				mutate: [{ path: ["systemPromptOptions", "customPrompt"], value: "MUT" }],
			} },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeAgentStart", trigger: {}, systemPromptOptions: { cwd: "/proj" } },
	]),
	exr("tool-context-fallback", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "toolContextTools", id: "call-1" },
		{ op: "toolContextExecuteTool", id: "call-1", name: "read", args: { p: 1 }, options: {} },
	]),
	exr("command-context-defaults", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "commandContext", method: "newSession", args: [{}] },
		{ op: "commandContext", method: "fork", args: ["e", {}] },
		{ op: "commandContext", method: "navigateTree", args: ["t", {}] },
		{ op: "commandContext", method: "switchSession", args: ["/s", {}] },
		{ op: "commandContext", method: "waitForIdle" },
		{ op: "commandContext", method: "reload" },
		{ op: "commandContext", method: "getSystemPromptOptions" },
	]),
	exr("ui-prompts-scripted", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "ui_prompt_start", handler: { id: 1 } },
			{ step: "on", event: "ui_prompt_end", handler: { id: 2 } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "hasUI" },
		{ op: "uiSelect", title: "Pick", options: ["a", "b"], opts: {} },
		{ op: "setUIContext", ui: true, mode: "tui", uiScript: [{ returns: "picked" }, { returns: true }, { throws: "ui boom" }] },
		{ op: "hasUI" },
		{ op: "uiSelect", title: "Pick", options: ["a", "b"], opts: {} },
		{ op: "uiConfirm", title: "Sure?", message: "m", opts: {} },
		{ op: "uiInput", title: "Name", placeholder: "p", opts: {} },
	]),
	exr("api-actions", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "captureApi", key: "a" },
		] },
		{ op: "createRunner" },
		{ op: "bindCore", actions: { activeTools: ["read"], thinkingLevel: "high" } },
		{ op: "apiCall", key: "a", method: "sendMessage", args: [{ customType: "t", content: "c" }, {}] },
		{ op: "apiCall", key: "a", method: "getSessionName" },
		{ op: "apiCall", key: "a", method: "getActiveTools" },
		{ op: "apiCall", key: "a", method: "setActiveTools", args: [["bash"]] },
		{ op: "apiCall", key: "a", method: "getThinkingLevel" },
		{ op: "apiCall", key: "a", method: "setModel", args: ["m"] },
		{ op: "apiCall", key: "a", method: "exec", args: ["echo", ["hi"], {}] },
	]),
	exr("ctx-reads-bound", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore", contextActions: { isIdle: false, isProjectTrusted: false } },
		{ op: "ctxRead", field: "isIdle" },
		{ op: "ctxRead", field: "isProjectTrusted" },
		{ op: "ctxRead", field: "hasPendingMessages" },
		{ op: "ctxRead", field: "getContextUsage" },
		{ op: "ctxRead", field: "getSystemPrompt" },
		{ op: "ctxRead", field: "cwd" },
		{ op: "ctxRead", field: "thinkingLevel" },
	]),
	// ---- tranche 6: emitter edge matrix (batch 2) ----------------------
	exr("emit-tool-result-null-replaces", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_result", handler: { id: 1, returns: { content: null, structuredContent: null, details: null, isError: null } } },
			{ step: "on", event: "tool_result", handler: { id: 2, eventReads: ["content", "structuredContent", "details", "isError"], returns: { usage: null } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolResult", event: { type: "tool_result", toolCallId: "c1", content: [{ type: "text", text: "old" }], details: { d: 1 }, structuredContent: { s: 1 }, isError: false, usage: { tokens: 1 } } },
	]),
	exr("emit-tool-result-in-place", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_result", handler: { id: 1, mutate: [{ path: ["content"], value: "MUT" }] } },
			{ step: "on", event: "tool_result", handler: { id: 2, eventReads: ["content"], returns: { details: { d: 2 } } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolResult", event: { type: "tool_result", content: [{ type: "text", text: "old" }], details: { d: 1 } } },
	]),
	exr("emit-tool-result-throw-continues", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_result", handler: { id: 1, throws: "tr boom" } },
			{ step: "on", event: "tool_result", handler: { id: 2, returns: { isError: true } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitToolResult", event: { type: "tool_result", content: [] } },
	]),
	exr("emit-tool-call-merge", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_call", handler: { id: 1, returns: { block: false, description: "d1" } } },
			{ step: "on", event: "tool_call", handler: { id: 2, eventReads: ["type", "toolName"], returns: { args: { a: 1 } } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolCall", event: { type: "tool_call", toolName: "read" } },
	]),
	exr("emit-tool-call-in-place", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_call", handler: { id: 1, mutate: [{ path: ["toolName"], value: "bash" }] } },
			{ step: "on", event: "tool_call", handler: { id: 2, eventReads: ["toolName"] } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolCall", event: { type: "tool_call", toolName: "read" } },
	]),
	exr("emit-message-end-role-mismatch", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "message_end", handler: { id: 1, returns: { message: { role: "assistant", content: "x" } } } },
			{ step: "on", event: "message_end", handler: { id: 2, eventReads: ["message.content"], returns: { message: { role: "user", content: "y" } } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitMessageEnd", event: { type: "message_end", message: { role: "user", content: "orig" } } },
	]),
	exr("emit-message-end-in-place", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "message_end", handler: { id: 1, mutate: [{ path: ["message", "content"], value: "MUT" }] } },
			{ step: "on", event: "message_end", handler: { id: 2, eventReads: ["message.content"], returns: { message: { role: "user", content: "final" } } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitMessageEnd", event: { type: "message_end", message: { role: "user", content: "orig" } } },
	]),
	exr("emit-provider-request-null-replace", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_provider_request", handler: { id: 1, returns: null } },
			{ step: "on", event: "before_provider_request", handler: { id: 2, eventReads: ["payload"], returns: { second: true } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeProviderRequest", payload: { p: 0 } },
	]),
	exr("emit-provider-request-in-place", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_provider_request", handler: { id: 1, mutate: [{ path: ["payload", "p"], value: 9 }] } },
			{ step: "on", event: "before_provider_request", handler: { id: 2, eventReads: ["payload.p"] } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeProviderRequest", payload: { p: 0 } },
	]),
	exr("emit-provider-headers-chain", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_provider_headers", handler: { id: 1, mutate: [{ path: ["headers", "authorization"], value: "a" }] } },
			{ step: "on", event: "before_provider_headers", handler: { id: 2, eventReads: ["headers.authorization"], mutate: [{ path: ["headers", "accept"], value: "b" }], returns: { ignored: true } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeProviderHeaders", headers: { accept: "old" } },
	]),
	exr("emit-boundary-continue", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "session_before_compact", handler: { id: 1, returns: { entries: [{ a: 1 }], continue: true } } },
			{ step: "on", event: "session_before_compact", handler: { id: 2, eventReads: ["entries", "continue", "context"], returns: { entries: [] } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBoundary", baseEvent: { type: "session_before_compact" }, script: [{ context: { c: 0 } }, { context: { c: 1 } }, { context: { c: 2 } }] },
	]),
	exr("emit-boundary-invalid", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "session_before_compact", handler: { id: 1, returns: { entries: [{ a: 1 }] } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitBoundary", baseEvent: { type: "session_before_compact" }, script: [{ context: { c: 0 } }, { throws: "bad entries" }] },
	]),
	exr("emit-input-handled", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "input", handler: { id: 1, eventReads: ["text", "images", "streamingBehavior"], returns: { action: "handled", text: "x" } } },
			{ step: "on", event: "input", handler: { id: 2, returns: { action: "transform", text: "ignored" } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitInput", text: "hi", images: [{ data: "old" }], source: "user", streamingBehavior: "queued" },
	]),
	exr("emit-input-unchanged", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "input", handler: { id: 1, returns: { action: "transform", text: "hi" } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitInput", text: "hi", source: "rpc" },
	]),
	exr("emit-input-images-transform", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "input", handler: { id: 1, eventReads: ["images"], returns: { action: "transform", images: [{ data: "x" }] } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitInput", text: "hi", images: [{ data: "old" }], source: "user" },
	]),
	exr("emit-user-bash-result-valid", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "user_bash", handler: { id: 1, returns: { result: { output: "o", exitCode: 0, cancelled: false, truncated: false } } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitUserBash", event: { command: "ls" } },
	]),
	// `operations.exec` must be a FUNCTION in TS (JSON cannot carry one), so
	// an empty object is the modelable invalid shape.
	exr("emit-user-bash-invalid-operations", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "user_bash", handler: { id: 1, returns: { operations: {} } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitUserBash", event: { command: "ls" } },
	]),
	exr("emit-context-in-place", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "context", handler: { id: 1, mutate: [{ path: ["messages"], op: "push", value: { role: "user", content: "m" } }] } },
			{ step: "on", event: "context_with_system", handler: { id: 2, eventReads: ["messages.0.role", "messages.1.content", "messages.2.content"] } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitContext", messages: [{ role: "system", content: "S", timestamp: 1 }, { role: "user", content: "old" }] },
	]),
	exr("api-actions-more", [
		{ op: "load", path: "/ext/a.js", script: [{ step: "captureApi", key: "a" }] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "apiCall", key: "a", method: "getCommands" },
		{ op: "apiCall", key: "a", method: "getAllTools" },
		{ op: "apiCall", key: "a", method: "getSettings" },
		{ op: "apiCall", key: "a", method: "setLabel", args: ["e1", "lbl"] },
		{ op: "apiCall", key: "a", method: "appendEntry", args: ["ct", { d: 1 }] },
		{ op: "apiCall", key: "a", method: "sendUserMessage", args: ["hi", {}] },
		{ op: "apiCall", key: "a", method: "setThinkingLevel", args: ["low"] },
		{ op: "apiCall", key: "a", method: "getSessionName" },
	]),
	exr("ctx-reads-model", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore", contextActions: { isIdle: true, isProjectTrusted: true, model: { provider: "p", id: "m" } } },
		{ op: "ctxRead", field: "getModel" },
		{ op: "ctxRead", field: "isIdle" },
		{ op: "commandContext", method: "getSystemPromptOptions" },
	]),
	exr("command-context-bound", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "bindCommandContext", actions: {} },
		{ op: "commandContext", method: "waitForIdle" },
		{ op: "commandContext", method: "newSession", args: [{}] },
		{ op: "commandContext", method: "fork", args: ["e", {}] },
		{ op: "commandContext", method: "navigateTree", args: ["t", {}] },
		{ op: "commandContext", method: "switchSession", args: ["/s", {}] },
		{ op: "commandContext", method: "reload" },
		{ op: "commandContext", method: "getSystemPromptOptions" },
	]),
	exr("command-context-reset", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "bindCommandContext", actions: {} },
		{ op: "bindCommandContext" },
		{ op: "commandContext", method: "newSession", args: [{}] },
		{ op: "commandContext", method: "reload" },
	]),
	exr("api-exec-enoent", [
		{ op: "load", path: "/ext/a.js", script: [{ step: "captureApi", key: "a" }] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "apiCall", key: "a", method: "exec", args: ["definitely-not-a-real-cmd-xyz", [], {}] },
	]),
	exr("api-exec-args-coerced", [
		{ op: "load", path: "/ext/a.js", script: [{ step: "captureApi", key: "a" }] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "apiCall", key: "a", method: "exec", args: ["echo", ["a", 1, true], {}] },
	]),
	exr("api-exec-bad-cwd", [
		{ op: "load", path: "/ext/a.js", script: [{ step: "captureApi", key: "a" }] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "apiCall", key: "a", method: "exec", args: ["echo", ["hi"], { cwd: "/definitely/not/here" }] },
	]),
	exr("emit-session-before-cancel", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "session_before_switch", handler: { id: 1, returns: { cancel: false, tag: 1 } } },
			{ step: "on", event: "session_before_switch", handler: { id: 2, returns: { cancel: true, tag: 2 } } },
			{ step: "on", event: "session_before_switch", handler: { id: 3, returns: { cancel: false, tag: 3 } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emit", event: { type: "session_before_switch" } },
	]),
	exr("emit-non-session-result-ignored", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "custom", handler: { id: 1, returns: { anything: true } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emit", event: { type: "custom" } },
	]),
	exr("emit-message-end-throw-continues", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "message_end", handler: { id: 1, throws: "me boom" } },
			{ step: "on", event: "message_end", handler: { id: 2, returns: { message: { role: "user", content: "z" } } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitMessageEnd", event: { type: "message_end", message: { role: "user", content: "orig" } } },
	]),
	exr("emit-tool-result-content-deletes-structured", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "tool_result", handler: { id: 1, returns: { content: [{ type: "text", text: "new" }] } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitToolResult", event: { type: "tool_result", content: [{ type: "text", text: "old" }], structuredContent: { s: 1 } } },
	]),
	exr("emit-cache-warming-last-wins", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "cache_warming_decision", handler: { id: 1, returns: { action: "off" } } },
			{ step: "on", event: "cache_warming_decision", handler: { id: 2, returns: { other: 1 } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitCacheWarmingDecision", event: { type: "cache_warming_decision", action: "warm" } },
	]),
	exr("emit-resources-prompt-paths", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "resources_discover", handler: { id: 1, returns: { promptPaths: ["/p1"], skillPaths: [] } } },
			{ step: "on", event: "resources_discover", handler: { id: 2, returns: { promptPaths: ["/p2"], themePaths: ["/t2"] } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitResourcesDiscover", cwd: "/proj", reason: "reload" },
	]),
	exr("emit-provider-request-throw", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_provider_request", handler: { id: 1, throws: "pp boom" } },
			{ step: "on", event: "before_provider_request", handler: { id: 2, eventReads: ["payload"], returns: { ok: true } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitBeforeProviderRequest", payload: { p: 0 } },
	]),
	exr("before-agent-start-handler-throw", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "before_agent_start", handler: { id: 1, throws: "bas boom" } },
			{ step: "on", event: "before_agent_start", handler: { id: 2, returns: { message: { role: "user", content: "m" } } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitBeforeAgentStart", trigger: {}, systemPromptOptions: { cwd: "/proj" } },
	]),
	exr("ctx-reads-unbound-defaults", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "ctxRead", field: "isIdle" },
		{ op: "ctxRead", field: "isProjectTrusted" },
		{ op: "ctxRead", field: "hasPendingMessages" },
		{ op: "ctxRead", field: "getContextUsage" },
		{ op: "ctxRead", field: "getSystemPrompt" },
		{ op: "ctxRead", field: "model" },
		{ op: "ctxRead", field: "scopedModels" },
	]),
	exr("invalidate-command-context", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "bindCommandContext", actions: {} },
		{ op: "invalidate" },
		{ op: "commandContext", method: "newSession", args: [{}] },
		{ op: "commandContext", method: "getSystemPromptOptions" },
	]),
	exr("emit-input-continue-action", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "input", handler: { id: 1, returns: { action: "continue", text: "ignored" } } },
		] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitInput", text: "hi", source: "user" },
	]),
	exr("emit-context-throw-continues", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "context", handler: { id: 1, throws: "ctx boom" } },
			{ step: "on", event: "context", handler: { id: 2, returns: { messages: [{ role: "user", content: "n" }] } } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitContext", messages: [{ role: "user", content: "old" }] },
	]),
	exr("before-agent-start-selected-tools-string", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitBeforeAgentStart", trigger: {}, systemPromptOptions: { cwd: "/proj", selectedTools: "ab" } },
	]),
	exr("emit-boundary-handler-throw", [
		{ op: "load", path: "/ext/a.js", script: [
			{ step: "on", event: "session_before_tree", handler: { id: 1, throws: "bd boom" } },
		] },
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "emitBoundary", baseEvent: { type: "session_before_tree" }, script: [{ context: { c: 0 } }, { context: { c: 1 } }] },
	]),
	exr("api-exec-rejects-nonstring-arg", [
		{ op: "load", path: "/ext/a.js", script: [{ step: "captureApi", key: "a" }] },
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "apiCall", key: "a", method: "exec", args: ["echo", [{ a: 1 }], {}] },
	]),
	exr("mcp-pre-bind-report", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "registerMcpServer", name: "early", config: { command: "node" } }],
		},
		{ op: "createRunner" },
		{ op: "onError" },
		{ op: "bindCore" },
		{ op: "reportUnhandledMcpServers" },
		{ op: "reportUnhandledMcpServers" },
		{ op: "mcpList" },
	]),

	// -- invalidate / stale ctx -----------------------------------------------
	exr("invalidate-stale", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "captureApi", key: "a" },
				{ step: "on", event: "custom", handler: { id: 1, reads: ["cwd"] } },
				{ step: "eventsOn", channel: "chan", handler: { id: 9 } },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore", actions: { activeTools: ["read"] } },
		{ op: "busEmit", channel: "chan", data: { n: 1 } },
		{ op: "emit", event: { type: "custom" } },
		{ op: "invalidate", message: "custom stale" },
		{ op: "busEmit", channel: "chan", data: { n: 2 } },
		{ op: "emit", event: { type: "custom" } },
		{
			op: "api",
			key: "a",
			steps: [{ step: "getFlag", name: "x" }, { step: "getActiveTools" }, { step: "eventsEmit", channel: "chan", data: { n: 3 } }],
		},
		{ op: "getActiveTools" },
		{ op: "invalidate", message: "second message" },
		{ op: "api", key: "a", steps: [{ step: "getFlag", name: "x" }] },
	]),
	exr("invalidate-default-message", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "captureApi", key: "a" }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "invalidate" },
		{ op: "api", key: "a", steps: [{ step: "getFlag", name: "x" }] },
		{ op: "getActiveTools" },
	]),

	// -- UI context / shutdown ---------------------------------------------------
	exr("ui-context", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "custom", handler: { id: 1, reads: ["mode", "hasUI"] } }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "hasUI" },
		{ op: "emit", event: { type: "custom" } },
		{ op: "setUIContext", ui: true, mode: "interactive" },
		{ op: "hasUI" },
		{ op: "emit", event: { type: "custom" } },
		{ op: "setUIContext", ui: false },
		{ op: "hasUI" },
		{ op: "emit", event: { type: "custom" } },
	]),
	exr("shutdown", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "custom", handler: { id: 1, calls: ["shutdown"] } }],
		},
		{ op: "createRunner" },
		{ op: "shutdown" },
		{ op: "bindCore", contextActions: { isIdle: true } },
		{ op: "emit", event: { type: "custom" } },
		{ op: "shutdown" },
	]),
	exr("session-shutdown", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "session_shutdown", handler: { id: 1 } }],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "emitSessionShutdown", event: { type: "session_shutdown" } },
	]),
	exr("session-shutdown-no-handlers", [
		{ op: "load", path: "/ext/a.js", script: [] },
		{ op: "createRunner" },
		{ op: "emitSessionShutdown", event: { type: "session_shutdown" } },
	]),

	// -- project trust ---------------------------------------------------------
	exr("project-trust-decided", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "on", event: "project_trust", handler: { id: 1, returns: { trusted: "undecided" }, reads: ["isProjectTrusted"] } },
				{ step: "on", event: "project_trust", handler: { id: 2, returns: { trusted: "yes", reason: "trusted by a" } } },
			],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [
				{ step: "on", event: "project_trust", handler: { id: 3 } },
				{ step: "on", event: "project_trust", handler: { id: 4, returns: { trusted: "no" } } },
			],
		},
		{ op: "createRunner" },
		{ op: "emitProjectTrust", event: { type: "project_trust", path: "/proj" }, ctx: { isProjectTrusted: true, mode: "print" } },
	]),
	exr("project-trust-error", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "project_trust", handler: { id: 1, returns: { trusted: "undecided" } } }],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "on", event: "project_trust", handler: { id: 2, throws: "trust error" } }],
		},
		{ op: "createRunner" },
		{ op: "emitProjectTrust", event: { type: "project_trust", path: "/proj" }, ctx: {} },
	]),
	exr("project-trust-all-undecided", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [{ step: "on", event: "project_trust", handler: { id: 1, returns: { trusted: "undecided" } } }],
		},
		{
			op: "load",
			path: "/ext/b.js",
			script: [{ step: "on", event: "project_trust", handler: { id: 2 } }],
		},
		{ op: "createRunner" },
		{ op: "emitProjectTrust", event: { type: "project_trust", path: "/proj" }, ctx: {} },
	]),

	// -- events bus --------------------------------------------------------------
	exr("events-bus", [
		{
			op: "load",
			path: "/ext/a.js",
			script: [
				{ step: "eventsOn", channel: "chan", handler: { id: 1 } },
				{ step: "eventsOn", channel: "chan", handler: { id: 9, throws: "bus boom" } },
				{ step: "eventsEmit", channel: "chan", data: { n: 0 } },
				{ step: "captureApi", key: "a" },
			],
		},
		{ op: "createRunner" },
		{ op: "bindCore" },
		{ op: "busEmit", channel: "chan", data: { n: 1 } },
		{ op: "busEmit", channel: "other", data: { n: 2 } },
		{ op: "api", key: "a", steps: [{ step: "eventsEmit", channel: "chan", data: { n: 3 } }] },
	]),

	// mcp-config (extensions/mcp/config.ts port): loadMcpConfig merge
	// semantics, override entries, collected validation errors, and the
	// update/add/remove edit-and-write helpers (default-deleting patches,
	// indent sniffing, pretty-write byte-exactness). Each case materializes
	// `files` under a per-side temp base; the final file map is compared.
	...(() => {
		const mcf = (name, files, steps) => ({ tool: "mcp-config", name: "mcf/" + name, args: { files, steps } });
		const c = [];
		const L = (name, files, options) => c.push(mcf(name, files, [{ op: "load", ...options }]));
		const G = "agent/mcp.json";
		const P = "cwd/.pi/mcp.json";
		const both = (g, p) => ({ [G]: g, [P]: p });

		// --- load: discovery and merging ---
		L("global-only", { [G]: '{"mcpServers":{"fs":{"command":"npx","args":["-y","pkg"]},"docs":{"url":"https://e/mcp","headers":{"Authorization":"Bearer t"}}}}' }, { agentDir: "agent", cwd: "cwd", projectTrusted: false });
		L("global-missing", {}, { agentDir: "agent", cwd: "cwd", projectTrusted: false });
		L("defaults", { [G]: '{"mcpServers":{}}' }, {});
		L("project-only-trusted", { [P]: '{"mcpServers":{"x":{"url":"https://e/mcp"}}}' }, { projectTrusted: true });
		L("project-untrusted", { [P]: '{"mcpServers":{"x":{"url":"https://e/mcp"}}}' }, { projectTrusted: false });
		L("project-replaces-global", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"command":"local","args":["--x"]}}}'), { projectTrusted: true });
		L("disabled-included", { [G]: '{"mcpServers":{"fs":{"command":"npx","enabled":false}}}' }, {});
		L("extra-top-level-keys", { [G]: '{"autoEnableCodemode":true,"other":[1,2.5],"mcpServers":{"x":{"command":"bin"}},"tail":null}' }, {});
		L("insertion-order", { [G]: '{"mcpServers":{"b":{"command":"b"},"a":{"command":"a"},"c":{"command":"c"}}}' }, { projectTrusted: true });

		// --- load: project override entries ---
		L("override-enabled", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"enabled":false}}}'), { projectTrusted: true });
		L("override-exposure", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"exposure":"deferred"}}}'), { projectTrusted: true });
		L("override-toolemposure", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"toolExposure":{"read":"direct","write*":"hidden"}}}}'), { projectTrusted: true });
		L("override-all-keys", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"enabled":true,"exposure":"hidden","toolExposure":{"t":"deferred"}}}}'), { projectTrusted: true });
		L("override-keeps-credentials", both('{"mcpServers":{"docs":{"url":"https://e/mcp","headers":{"Authorization":"Bearer t"},"auth":{"provider":"p"},"oauth":{"scope":"s"}}}}', '{"mcpServers":{"docs":{"enabled":false,"exposure":"direct"}}}'), { projectTrusted: true });
		L("override-updates-in-place", both('{"mcpServers":{"a":{"command":"a"},"fs":{"command":"npx"},"z":{"command":"z"}}}', '{"mcpServers":{"fs":{"enabled":false}}}'), { projectTrusted: true });
		L("override-alias-resolution", both('{"mcpServers":{"fs":{"command":"npx","exposure":"codemode-deferred","toolExposure":{"t":"codemode-deferred"}}}}', '{"mcpServers":{"fs":{"exposure":"direct"}}}'), { projectTrusted: true });
		L("override-no-base", { [P]: '{"mcpServers":{"x":{"enabled":false}}}' }, { projectTrusted: true });
		L("override-extra-key", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"enabled":false,"description":"no"}}}'), { projectTrusted: true });
		L("override-empty-object", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{}}}'), { projectTrusted: true });
		L("override-invalid-exposure", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"exposure":"bogus"}}}'), { projectTrusted: true });
		L("override-raw-entry-not-object", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":"str"}}'), { projectTrusted: true });
		L("override-type-only-is-definition", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"type":"stdio"}}}'), { projectTrusted: true });
		L("override-command-null-is-definition", both('{"mcpServers":{"fs":{"command":"npx"}}}', '{"mcpServers":{"fs":{"command":null,"enabled":false}}}'), { projectTrusted: true });

		// --- load: auth and namespace rules ---
		L("auth-in-project", { [P]: '{"mcpServers":{"x":{"url":"https://e/mcp","auth":{"provider":"p"}}}}' }, { projectTrusted: true });
		L("auth-null-in-project", { [P]: '{"mcpServers":{"x":{"url":"https://e/mcp","auth":null}}}' }, { projectTrusted: true });
		L("auth-zero-in-project", { [P]: '{"mcpServers":{"x":{"url":"https://e/mcp","auth":0}}}' }, { projectTrusted: true });
		L("auth-stdio-project-ok", { [P]: '{"mcpServers":{"x":{"command":"bin","auth":{"provider":"p"}}}}' }, { projectTrusted: true });
		L("auth-url-null-stdio-project", { [P]: '{"mcpServers":{"x":{"url":null,"command":"bin","auth":{"provider":"p"}}}}' }, { projectTrusted: true });
		L("namespace-clash-project", both('{"mcpServers":{"my-server":{"command":"npx"}}}', '{"mcpServers":{"my_server":{"command":"other"}}}'), { projectTrusted: true });
		L("namespace-clash-same-file", { [G]: '{"mcpServers":{"a-b":{"command":"npx"},"a_b":{"command":"other"}}}' }, {});
		L("namespace-no-clash", { [G]: '{"mcpServers":{"ab":{"command":"npx"},"a-b":{"command":"other"}}}' }, {});

		// --- load: parse and shape errors ---
		L("bad-json", { [G]: '{"a":}' }, {});
		L("bad-json-empty-file", { [G]: "" }, {});
		L("bad-json-bom", { [G]: "﻿{}" }, {});
		L("not-object", { [G]: "[1,2]" }, {});
		L("not-object-string", { [G]: '"x"' }, {});
		L("mcpservers-array", { [G]: '{"mcpServers":[]}' }, {});
		L("mcpservers-null", { [G]: '{"mcpServers":null}' }, {});
		L("mcpservers-absent", { [G]: '{"other":1}' }, {});
		L("duplicate-keys", { [G]: '{"mcpServers":{"a":{"command":"one"},"a":{"url":"https://x/mcp"}}}' }, {});
		L("auto-enable-invalid", { [G]: '{"autoEnableCodemode":"yes","mcpServers":{}}' }, {});
		L("auto-enable-global-false", { [G]: '{"autoEnableCodemode":false,"mcpServers":{}}' }, {});
		L("auto-enable-project-overrides", both('{"autoEnableCodemode":false,"mcpServers":{}}', '{"autoEnableCodemode":true,"mcpServers":{}}'), { projectTrusted: true });
		L("invalid-server-exposure", { [G]: '{"mcpServers":{"x":{"command":"bin","exposure":"nope"}}}' }, {});
		L("invalid-server-enabled", { [G]: '{"mcpServers":{"x":{"command":"bin","enabled":"yes"}}}' }, {});
		L("invalid-server-must-be-object", { [G]: '{"mcpServers":{"x":"str"}}' }, {});
		L("invalid-server-needs-command-or-url", { [G]: '{"mcpServers":{"x":{}}}' }, {});
		L("invalid-server-url", { [G]: '{"mcpServers":{"x":{"url":"ftp://e/mcp"}}}' }, {});
		L("invalid-server-args", { [G]: '{"mcpServers":{"x":{"command":"bin","args":[1]}}}' }, {});
		L("invalid-server-headers", { [G]: '{"mcpServers":{"x":{"url":"https://e/mcp","headers":{"a":1}}}}' }, {});
		L("invalid-server-timeout", { [G]: '{"mcpServers":{"x":{"command":"bin","timeout":0}}}' }, {});
		L("invalid-server-sse", { [G]: '{"mcpServers":{"x":{"type":"sse","url":"https://e/mcp"}}}' }, {});
		L("invalid-server-name", { [G]: '{"mcpServers":{"bad name":{"command":"bin"}}}' }, {});
		L("errors-from-both-files", both('{"mcpServers":{"a":{"command":"bin","enabled":"x"}}}', '{"mcpServers":{"a":{"command":"npx"},"b":{"enabled":false}}}'), { projectTrusted: true });

		// --- update ---
		c.push(mcf("update-deletes-defaults", { [G]: '{\n  "mcpServers": {\n    "x": {\n      "command": "bin",\n      "enabled": false,\n      "exposure": "deferred"\n    }\n  }\n}\n' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: true, exposure: "codemode" } },
		]));
		c.push(mcf("update-sets-values", { [G]: '{\n  "mcpServers": {\n    "x": {\n      "command": "bin",\n      "enabled": true\n    }\n  }\n}\n' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: false, exposure: "hidden" } },
		]));
		c.push(mcf("update-override-keeps-defaults", { [P]: '{\n  "mcpServers": {\n    "x": {\n      "enabled": false\n    }\n  }\n}\n' }, [
			{ op: "update", path: P, name: "x", patch: { enabled: true, exposure: "codemode" } },
		]));
		c.push(mcf("update-missing-throws", { [G]: '{"mcpServers":{"x":{"command":"bin"}}}' }, [
			{ op: "update", path: G, name: "y", patch: { enabled: true } },
		]));
		c.push(mcf("update-non-record-entry-throws", { [G]: '{"mcpServers":{"x":"str"}}' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: true } },
		]));
		c.push(mcf("update-missing-with-override", { [G]: '{"mcpServers":{"x":{"command":"bin"}}}' }, [
			{ op: "update", path: G, name: "y", patch: { enabled: false }, override: true },
		]));
		c.push(mcf("update-override-no-mcpservers-key", { [G]: '{"other":1}' }, [
			{ op: "update", path: G, name: "y", patch: { enabled: false }, override: true },
		]));
		c.push(mcf("update-indent-four-spaces", { [G]: '{\n    "mcpServers": {\n        "x": {\n            "command": "bin"\n        }\n    }\n}\n' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: false } },
		]));
		c.push(mcf("update-indent-tab", { [G]: '{\n\t"mcpServers": {\n\t\t"x": {"command": "bin"}\n\t}\n}\n' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: false } },
		]));
		c.push(mcf("update-indent-long-truncated", { [G]: '{\n               "mcpServers": {\n                    "x": {"command": "bin"}\n               }\n}\n' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: false } },
		]));
		c.push(mcf("update-indent-first-indented-line-wins", { [G]: '{"a":1,\n    "mcpServers": {"x": {"command": "bin"}}\n}\n' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: false } },
		]));
		c.push(mcf("update-bad-json-throws-raw", { [G]: "{" }, [
			{ op: "update", path: G, name: "x", patch: { enabled: true } },
		]));
		c.push(mcf("update-empty-file-throws", { [G]: "" }, [
			{ op: "update", path: G, name: "x", patch: { enabled: true } },
		]));
		c.push(mcf("update-mcpservers-not-object-throws", { [G]: '{"mcpServers":5}' }, [
			{ op: "update", path: G, name: "x", patch: { enabled: true } },
		]));
		c.push(mcf("update-missing-file-creates", {}, [
			{ op: "update", path: "new/dir/mcp.json", name: "x", patch: { enabled: false }, override: true },
		]));
		c.push(mcf("update-no-patch-is-noop-write", { [G]: '{"mcpServers":{"x":{"command":"bin"}}}\n' }, [
			{ op: "update", path: G, name: "x", patch: {} },
		]));

		// --- add / remove ---
		c.push(mcf("add-creates-file", {}, [
			{ op: "add", path: "new/dir/mcp.json", name: "x", config: { command: "bin", args: ["--a"] } },
		]));
		c.push(mcf("add-replaces-keeps-position", { [G]: '{"mcpServers":{"x":{"command":"old"},"y":{"command":"y"}}}' }, [
			{ op: "add", path: G, name: "x", config: { url: "https://e/mcp" } },
			{ op: "add", path: G, name: "z", config: { command: "z" } },
		]));
		c.push(mcf("add-keeps-other-content", { [G]: '{\n  "before": [1, 2.5, "s"],\n  "mcpServers": {},\n  "after": {"deep": [true, null]}\n}\n' }, [
			{ op: "add", path: G, name: "x", config: { command: "bin" } },
		]));
		c.push(mcf("add-bad-json-throws-raw", { [G]: "not json" }, [
			{ op: "add", path: G, name: "x", config: { command: "bin" } },
		]));
		c.push(mcf("remove-existing", { [G]: '{"mcpServers":{"x":{"command":"x"},"y":{"command":"y"}}}' }, [
			{ op: "remove", path: G, name: "x" },
		]));
		c.push(mcf("remove-missing-name-no-write", { [G]: '{"mcpServers":{"x":{"command":"x"}}}\n' }, [
			{ op: "remove", path: G, name: "y" },
		]));
		c.push(mcf("remove-missing-file", {}, [
			{ op: "remove", path: "nope/mcp.json", name: "x" },
		]));
		c.push(mcf("remove-empty-mcpservers", { [G]: '{"mcpServers":{"x":{"command":"x"}}}' }, [
			{ op: "remove", path: G, name: "x" },
		]));

		// --- round trip: writes are readable by the loader ---
		c.push(mcf("round-trip", { [G]: '{"mcpServers":{"fs":{"command":"npx","enabled":false}}}' }, [
			{ op: "load", agentDir: "agent", cwd: "cwd", projectTrusted: false },
			{ op: "update", path: G, name: "fs", patch: { enabled: true } },
			{ op: "load", agentDir: "agent", cwd: "cwd", projectTrusted: false },
			{ op: "add", path: P, name: "fs", config: { enabled: false } },
			{ op: "load", agentDir: "agent", cwd: "cwd", projectTrusted: true },
			{ op: "remove", path: P, name: "fs" },
			{ op: "load", agentDir: "agent", cwd: "cwd", projectTrusted: true },
		]));

		// --- harness argument errors ---
		c.push({ tool: "mcp-config", name: "mcf/err-missing-steps", args: { files: {} } });
		c.push({ tool: "mcp-config", name: "mcf/err-bad-files", args: { files: [1], steps: [] } });
		c.push({ tool: "mcp-config", name: "mcf/err-bad-patch", args: { files: {}, steps: [{ op: "update", path: "a", name: "x", patch: 5 }] } });
		c.push({ tool: "mcp-config", name: "mcf/err-unknown-op", args: { files: {}, steps: [{ op: "bogus" }] } });

		return c;
	})(),

	// mcp-log (extensions/mcp/log.ts port): formatMcpLogMessage byte
	// format (level/logger defaults, string vs JSON data, continuation
	// indent, ISO timestamps) and McpServerLog append/rotation semantics
	// (single-append lines, cached-size rotation with the external-rotation
	// guard, ignored write errors). `{ unit, count }` file seeds cross the
	// 5MB rotation threshold without megabyte payloads.
	...(() => {
		const mll = (name, files, steps) => ({ tool: "mcp-log", name: "mll/" + name, args: { files, steps } });
		const F = (name, steps) => mll(name, {}, steps);
		const fmt = (server, params, nowMs) => ({ op: "format", server, ...(params === undefined ? {} : { params }), nowMs });
		const wr = (path, server, params, nowMs, extra) => ({
			op: "write", path, server, ...(params === undefined ? {} : { params }), nowMs, ...(extra || {}),
		});
		const MB = 1024 * 1024;
		const c = [];

		// --- formatMcpLogMessage ---
		c.push(F("fmt-string-data", [
			fmt("fs", { level: "debug", logger: "mylog", data: "hello world" }, 0),
			fmt("context7", { level: "warning", data: "careful" }, 1767225623123),
		]));
		c.push(F("fmt-object-data", [
			fmt("fs", { data: { a: 1, b: "x", c: [1, 2], d: null } }, 0),
			fmt("fs", { data: { nested: { deep: true } } }, 0),
			fmt("fs", { data: {} }, 0),
			fmt("fs", { data: [] }, 0),
		]));
		c.push(F("fmt-defaults", [
			fmt("fs", {}, 0),
			fmt("fs", undefined, 0),
			fmt("fs", null, 0),
			fmt("fs", { data: null }, 0),
			fmt("fs", { level: null, logger: null, data: "x" }, 0),
		]));
		c.push(F("fmt-non-record-params", [
			fmt("fs", "plain text", 0),
			fmt("fs", 42, 0),
			fmt("fs", 1.5, 0),
			fmt("fs", true, 0),
			fmt("fs", [1, "a", { b: 2 }], 0),
		]));
		c.push(F("fmt-level-logger-edges", [
			fmt("fs", { level: 5, data: "x" }, 0),
			fmt("fs", { level: "", data: "x" }, 0),
			fmt("fs", { level: null, data: "x" }, 0),
			fmt("fs", { logger: "", data: "x" }, 0),
			fmt("fs", { logger: 7, data: "x" }, 0),
			fmt("fs", { logger: "server.logger", level: "error", data: "boom" }, 0),
		]));
		c.push(F("fmt-multiline", [
			fmt("fs", { data: "a\nb" }, 0),
			fmt("fs", { data: "a\r\nb" }, 0),
			fmt("fs", { data: "a\rb\nc" }, 0),
			fmt("fs", { data: "\n" }, 0),
			fmt("fs", { data: "line1\nline2\nline3" }, 0),
		]));
		c.push(F("fmt-iso-timestamps", [
			fmt("fs", { data: "x" }, -1),
			fmt("fs", { data: "x" }, 86400123),
			fmt("fs", { data: "x" }, 253402300799999),
		]));
		c.push(F("fmt-number-formatting", [
			fmt("fs", { data: 1e21 }, 0),
			fmt("fs", { data: 0.000001 }, 0),
			fmt("fs", { data: -0.5 }, 0),
			fmt("fs", { data: 100 }, 0),
		]));

		// --- McpServerLog appends ---
		c.push(F("write-creates-file", [wr("agent/mcp.log", "fs", { data: "first" }, 1000)]));
		c.push(F("write-multiple-lines", [
			wr("agent/mcp.log", "fs", { data: "one" }, 1000),
			wr("agent/mcp.log", "fs", { level: "warning", data: "two" }, 2000),
			wr("agent/mcp.log", "other", { logger: "l", data: "three" }, 3000),
		]));
		c.push(F("write-two-processes", [
			wr("agent/mcp.log", "fs", { data: "cached" }, 1000),
			wr("agent/mcp.log", "fs", { data: "fresh" }, 2000, { fresh: true }),
			wr("agent/mcp.log", "fs", { data: "cached-again" }, 3000),
		]));
		c.push(F("write-multiline-indent", [
			wr("agent/mcp.log", "fs", { data: "a\nb" }, 0),
			wr("agent/mcp.log", "fs", { data: "c" }, 0),
		]));
		// The log path's parent is a seeded file: mkdirSync fails and the
		// write is silently ignored.
		c.push(mll("write-error-ignored", { afile: "seed" }, [
			wr("afile/mcp.log", "fs", { data: "x" }, 0),
			wr("afile/mcp.log", "fs", { data: "y" }, 0),
		]));

		// --- rotation ---
		c.push(mll("rotate-over-threshold", { "logs/mcp.log": { unit: "x", count: 5 * MB + 1 } }, [
			wr("logs/mcp.log", "fs", { data: "first" }, 0),
			wr("logs/mcp.log", "fs", { data: "second" }, 0),
		]));
		c.push(mll("rotate-at-exact-threshold", { "logs/mcp.log": { unit: "x", count: 5 * MB } }, [
			wr("logs/mcp.log", "fs", { data: "first" }, 0),
			wr("logs/mcp.log", "fs", { data: "second" }, 1000),
			wr("logs/mcp.log", "fs", { data: "third" }, 2000),
		]));
		c.push(mll("rotate-external-guard", { "logs/mcp.log": { unit: "x", count: 5 * MB - 10 } }, [
			wr("logs/mcp.log", "fs", { data: "0123456789abcdef" }, 0),
			{ op: "renameFile", from: "logs/mcp.log", to: "logs/mcp.log.1" },
			wr("logs/mcp.log", "fs", { data: "after-external" }, 1000),
		]));
		c.push(mll("rotate-two-files-after", { "logs/mcp.log": { unit: "x", count: 5 * MB + 100 } }, [
			wr("logs/mcp.log", "fs", { data: "rotates" }, 0),
			wr("logs/mcp.log", "fs", { data: "stays" }, 1000),
			wr("logs/mcp.log", "fs", { data: "more" }, 2000),
		]));

		// --- harness argument errors ---
		c.push(F("err-missing-steps", undefined));
		c.push(mll("err-bad-files", [1], []));
		c.push(mll("err-bad-files-value", { a: { unit: "x", count: -1 } }, []));
		c.push(mll("err-bad-step", {}, [5]));
		c.push(mll("err-missing-op", {}, [{ server: "fs" }]));
		c.push(mll("err-unknown-op", {}, [{ op: "bogus" }]));
		c.push(mll("err-bad-server", {}, [{ op: "format", server: 5, nowMs: 0 }]));
		c.push(mll("err-missing-server", {}, [{ op: "format", nowMs: 0 }]));
		c.push(mll("err-bad-nowms", {}, [{ op: "format", server: "fs", nowMs: 1.5 }]));
		c.push(mll("err-bad-fresh", {}, [{ op: "write", path: "a", server: "fs", fresh: "x" }]));

		return c;
	})(),

	// mcp-connection (extensions/mcp/runtime.ts + pi-mcp client.ts port):
	// McpServerConnection over scripted fake transports — connect handshake
	// (initialize params, capability gating, instructions), request/reply
	// flows (callTool, resources, templates, pagination + validation),
	// retry semantics (read-only transient, session-expired, connect
	// retries), auth flows (needs-auth vs propagated errors), timeouts
	// (notifications/cancelled), transport drops (disconnected +
	// reconnect), refreshes (tools/list_changed), server notifications
	// (notifications/message logging), lifecycle ops (reconnect, signOut,
	// close), and harness argument errors. Harness constraints: replies
	// arrive in the same send's batch or the case tests timeouts with
	// small timeoutMs; incoming server requests ride in dedicated batches.
	...(() => {
		const httpConfig = (extra) => ({ url: "https://example.test/mcp", ...(extra || {}) });
		const stdioConfig = (extra) => ({ command: "run-server", ...(extra || {}) });
		const msg = (m) => ({ message: m });
		const resp = (id, result) => msg({ jsonrpc: "2.0", id, result });
		const rpcErr = (id, code, message) => msg({ jsonrpc: "2.0", id, error: { code, message } });
		const notif = (method, params) => msg({ jsonrpc: "2.0", method, ...(params === undefined ? {} : { params }) });
		const fail = (spec) => ({ fail: spec });
		const closeStep = { close: true };
		const httpFail = (status, message, extra) => ({ kind: "http", status, ...(message === undefined ? {} : { message }), ...(extra || {}) });
		const initResult = (capabilities, extra) => ({
			protocolVersion: "2025-11-25",
			capabilities,
			serverInfo: { name: "srv", version: "1.0" },
			...(extra || {}),
		});
		const caps = (tools, resources, extra) => ({
			...(tools ? { tools: {} } : {}),
			...(resources ? { resources: {} } : {}),
			...(extra || {}),
		});
		// The connect handshake: initialize, notifications/initialized,
		// then tools/list (when the server has the tools capability).
		const connectSends = (initRes, toolsResult, ...extra) => [
			[resp(1, initRes)],
			[],
			...(toolsResult === null ? [] : [[toolsResult]]),
			...extra,
		];
		const tool = (name, inputSchema, extra) => ({ name, inputSchema, ...(extra || {}) });
		const mcc = (name, config, steps, transports, extra) => ({
			tool: "mcp-connection",
			name: "mcc/" + name,
			args: { entry: { name: "srv", config }, steps, transports, ...(extra || {}) },
		});
		const call = (name, args, options) => ({ op: "callTool", name, ...(args === undefined ? {} : { args }), ...(options || {}) });
		const snap = { op: "snapshot" };
		const c = [];
		const push = (name, config, steps, transports, extra) => c.push(mcc(name, config, steps, transports, extra));

		// --- connect + callTool ---
		push("connect-calltool", httpConfig(), [call("greet", { who: "world" }), snap], [
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true), { instructions: "  be nice  " }), resp(2, { tools: [tool("greet", {})] }), resp(3, { content: [{ type: "text", text: "hi" }] })) },
		]);
		push("connect-no-tools-capability", httpConfig(), [call("greet"), snap], [
			{ startSteps: [], sendSteps: [...connectSends(initResult(caps(false, false)), null), resp(2, { content: [] })] },
		]);
		push("connect-no-resources", httpConfig(), [snap], [
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, false), { instructions: "   " }), resp(2, { tools: [] })) },
		]);
		push("connect-instructions-missing", httpConfig(), [snap], [
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [] })) },
		]);
		push("connect-invalid-initialize", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[resp(1, { protocolVersion: "2025-11-25", capabilities: {}, serverInfo: { name: "s" } })]] },
		]);
		push("connect-unsupported-protocol", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[resp(1, initResult(caps(true, true), { protocolVersion: "2024-01-01" }))]] },
		]);
		push("calltool-structured-only", httpConfig(), [call("t")], [
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [] }), resp(3, { structuredContent: { a: 1 } })) },
		]);
		push("calltool-invalid-content", httpConfig(), [call("t")], [
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [] }), resp(3, { content: "x" })) },
		]);
		push("calltool-rpc-error", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [] }), rpcErr(3, -32602, "bad args")) },
		]);

		// --- resources + templates ---
		const resourceSends = connectSends(initResult(caps(true, true)), resp(2, { tools: [] }));
		const res1 = { uri: "file:///a", name: "a", description: "first" };
		const res2 = { uri: "file:///b" };
		push("resources-page", httpConfig(), [{ op: "resourcesPage" }, { op: "resourcesPage", cursor: "c2" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resources: [res1, res2], nextCursor: "c2" }), resp(4, { resources: [] })] },
		]);
		push("resources-all-name-defaults", httpConfig(), [{ op: "allResources" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resources: [res1, res2], nextCursor: "x" }), resp(4, { resources: [res1], nextCursor: null }), resp(5, { resources: [] })] },
		]);
		push("resources-null-cursor-ends", httpConfig(), [{ op: "allResources" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resources: [res2], nextCursor: "" })] },
		]);
		push("resources-duplicate-cursor", httpConfig(), [{ op: "allResources" }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resources: [], nextCursor: "a" }), resp(4, { resources: [], nextCursor: "a" })] },
		]);
		push("resources-invalid-entry", httpConfig(), [{ op: "resourcesPage" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resources: [{ name: 5 }] })] },
		]);
		push("resources-invalid-cursor", httpConfig(), [{ op: "resourcesPage" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resources: [], nextCursor: 7 })] },
		]);
		push("resources-read", httpConfig(), [{ op: "readResource", uri: "file:///a" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { contents: [{ uri: "file:///a", text: "body" }] })] },
		]);
		push("resources-read-invalid", httpConfig(), [{ op: "readResource", uri: "file:///a" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { contents: [{ uri: "file:///a" }] })] },
		]);
		const tmpl = { uriTemplate: "file:///{x}", description: "t" };
		push("templates-page-method-not-found", httpConfig(), [{ op: "resourceTemplatesPage" }], [
			{ startSteps: [], sendSteps: [...resourceSends, rpcErr(3, -32601, "Method not found: resources/templates/list")] },
		]);
		push("templates-page", httpConfig(), [{ op: "resourceTemplatesPage" }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { resourceTemplates: [tmpl, { uriTemplate: "file:///y" }] })] },
		]);
		push("templates-all-method-not-found", httpConfig(), [{ op: "allResourceTemplates" }], [
			{ startSteps: [], sendSteps: [...resourceSends, rpcErr(3, -32601, "Method not found: resources/templates/list")] },
		]);
		push("templates-all-error", httpConfig(), [{ op: "allResourceTemplates" }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends, rpcErr(3, -32603, "boom")] },
		]);

		// --- retries ---
		push("read-retries-transient", httpConfig(), [{ op: "readResource", uri: "file:///a" }], [
			{ startSteps: [], sendSteps: [...resourceSends, [fail(httpFail(429, "rate limited"))], resp(4, { contents: [{ uri: "file:///a", text: "ok" }] })] },
		]);
		push("calltool-no-transient-retry", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [...resourceSends, [fail(httpFail(503, "overloaded"))]] },
		]);
		push("read-server-error-not-retried", httpConfig(), [{ op: "readResource", uri: "file:///a" }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends, [fail(httpFail(501, "not implemented"))]] },
		]);
		push("connect-retries-transient-http", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "typeError", message: "fetch failed" })]] },
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [] }), resp(3, { content: [] })) },
		]);
		push("connect-no-retry-stdio", stdioConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "typeError", message: "spawn failed" })]] },
		]);
		push("connect-nontransient-http-error", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail(httpFail(404, "no route"))]] },
		]);
		push("connect-exhausts-retries", httpConfig({ timeout: 1 }), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "typeError", message: "fetch failed 1" })]] },
			{ startSteps: [], sendSteps: [[fail({ kind: "typeError", message: "fetch failed 2" })]] },
			{ startSteps: [], sendSteps: [[fail({ kind: "typeError", message: "fetch failed 3" })]] },
		]);
		push("session-expired-retries", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [...resourceSends, [fail({ kind: "sessionExpired", body: "gone" })]] },
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [tool("t", {})] }), resp(3, { content: [] })) },
		]);

		// --- auth ---
		push("auth-required-with-provider", httpConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "authRequired" })]] },
		]);
		push("auth-required-login-provider", httpConfig({ auth: { provider: "anthropic" } }), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "authRequired" })]] },
		]);
		push("auth-required-without-provider", httpConfig({ headers: { Authorization: "Bearer x" } }), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "authRequired" })]] },
		]);
		push("oauth-required-stdio", stdioConfig(), [call("t"), snap], [
			{ startSteps: [], sendSteps: [[fail({ kind: "oauthRequired" })]] },
		]);

		// --- timeouts ---
		push("calltool-timeout", httpConfig(), [call("t", undefined, { options: { timeoutMs: 5 } }), snap], [
			{ startSteps: [], sendSteps: [...resourceSends, [], []] },
		]);

		// --- transport drops ---
		push("transport-close-mid-op", httpConfig(), [call("t"), snap, call("t"), snap], [
			{ startSteps: [], sendSteps: [...resourceSends, [closeStep]] },
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [] }), resp(3, { content: [] })) },
		]);
		push("emit-close-disconnects", httpConfig(), [call("t"), { op: "emit", close: true }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { content: [] })] },
		]);

		// --- incoming requests + notifications (emit) ---
		push("emit-ping", httpConfig(), [call("t"), { op: "emit", message: { jsonrpc: "2.0", id: 50, method: "ping" } }], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { content: [] })] },
		]);
		push("emit-roots-list", httpConfig(), [{ op: "emit", message: { jsonrpc: "2.0", id: 51, method: "roots/list" } }], [
			{ startSteps: [], sendSteps: [...resourceSends] },
		]);
		push("emit-unknown-method", httpConfig(), [{ op: "emit", message: { jsonrpc: "2.0", id: 52, method: "x/y" } }], [
			{ startSteps: [], sendSteps: [...resourceSends] },
		]);
		push("emit-error-only", httpConfig(), [{ op: "emit", error: "transport hiccup" }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends] },
		]);
		push("emit-invalid-message", httpConfig(), [{ op: "emit", message: { jsonrpc: "1.0", id: 1, method: "x" } }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends] },
		]);
		push("tools-list-changed-refresh", httpConfig(), [call("t"), snap], [
			{
				startSteps: [],
				sendSteps: [
					[resp(1, initResult(caps(true, true)))],
					[],
					[resp(2, { tools: [] }), notif("notifications/tools/list_changed")],
					[resp(3, { tools: [tool("new-tool", {})] })],
					[resp(4, { content: [] })],
				],
			},
		]);
		push("message-notification-logs", httpConfig(), [{ op: "callTool", name: "t", nowMs: 1000 }], [
			{
				startSteps: [],
				sendSteps: [
					[resp(1, initResult(caps(true, true))), notif("notifications/message", { level: "info", data: "hello" })],
					[],
					[resp(2, { tools: [] })],
					[resp(3, { content: [] }), notif("notifications/message", { data: "again" })],
				],
			},
		], { logPath: "logs/srv.log" });

		// --- lifecycle ---
		push("reconnect", httpConfig(), [call("t"), { op: "reconnect" }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { content: [] })] },
			{ startSteps: [], sendSteps: connectSends(initResult(caps(true, true)), resp(2, { tools: [tool("x", {})] })) },
		]);
		push("signout", httpConfig(), [call("t"), { op: "signOut" }, snap], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { content: [] })] },
		]);
		push("close-then-call", httpConfig(), [call("t"), { op: "close" }, call("t"), snap], [
			{ startSteps: [], sendSteps: [...resourceSends, resp(3, { content: [] })] },
		]);

		// --- harness argument errors ---
		c.push({ tool: "mcp-connection", name: "mcc/err-missing-steps", args: { entry: { name: "srv", config: httpConfig() }, transports: [] } });
		c.push({ tool: "mcp-connection", name: "mcc/err-missing-transports", args: { entry: { name: "srv", config: httpConfig() }, steps: [] } });
		c.push({ tool: "mcp-connection", name: "mcc/err-bad-entry", args: { steps: [], transports: [] } });
		c.push({ tool: "mcp-connection", name: "mcc/err-unknown-op", args: { entry: { name: "srv", config: httpConfig() }, steps: [{ op: "bogus" }], transports: [] } });
		c.push({ tool: "mcp-connection", name: "mcc/err-bad-fail", args: { entry: { name: "srv", config: httpConfig() }, steps: [], transports: [{ startSteps: [{}], sendSteps: [] }] } });
		c.push({ tool: "mcp-connection", name: "mcc/err-bad-timeout", args: { entry: { name: "srv", config: httpConfig() }, steps: [{ op: "callTool", name: "t", options: { timeoutMs: 1.5 } }], transports: [] } });

		return c;
	})(),

	// -- harness error paths --------------------------------------------------------
	{ tool: "extension-runner", name: "exr/missing-ops", args: {} },
);
