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
// case (stays in sync with the TS tree automatically).
import { SETTINGS_DEFAULTS } from "file:///tmp/pi-repo/packages/coding-agent/dist/core/settings-defaults.js";
import { latexCases } from "./waved/latex-cases.mjs";
import { markdownRenderCases } from "./waved/markdown-render-cases.mjs";
import { regressionCases } from "./regressions/index.mjs";

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

	// ------------------------------------------- session-manager (module)
	{
		name: "session-manager/parse-basic",
		tool: "session-manager",
		args: {
			fn: "parseSessionEntries",
			content: [
				JSON.stringify(smHeader(3)),
				JSON.stringify(smUser("e1", null, "hello", 1)),
				"",
				"   ",
				"not json at all",
				JSON.stringify(smAssistant("e2", "e1", "hi", 2)),
			].join("\n"),
		},
	},
	{
		name: "session-manager/parse-empty",
		tool: "session-manager",
		args: { fn: "parseSessionEntries", content: "" },
	},
	{
		name: "session-manager/parse-only-blank",
		tool: "session-manager",
		args: { fn: "parseSessionEntries", content: "\n  \n\t\n" },
	},
	{
		name: "session-manager/parse-no-trailing-newline",
		tool: "session-manager",
		args: {
			fn: "parseSessionEntries",
			content: JSON.stringify(smUser("e1", null, "hello", 1)) + "\n" + JSON.stringify(smUser("e2", "e1", "again", 2)),
		},
	},
	{
		name: "session-manager/assert-id-valid",
		tool: "session-manager",
		args: { fn: "assertValidSessionId", id: "abc-DEF_123.456" },
	},
	{ name: "session-manager/assert-id-space", tool: "session-manager", args: { fn: "assertValidSessionId", id: "has space" } },
	{ name: "session-manager/assert-id-leading-dash", tool: "session-manager", args: { fn: "assertValidSessionId", id: "-abc" } },
	{ name: "session-manager/assert-id-trailing-dot", tool: "session-manager", args: { fn: "assertValidSessionId", id: "abc." } },
	{ name: "session-manager/assert-id-empty", tool: "session-manager", args: { fn: "assertValidSessionId", id: "" } },
	{
		name: "session-manager/migrate-v1",
		tool: "session-manager",
		normalizeSessionIds: true,
		args: {
			fn: "migrateSessionEntries",
			entries: [
				smHeader(1),
				{ type: "message", timestamp: smTs(1), message: { role: "user", content: [{ type: "text", text: "hello" }], timestamp: smTs(1) } },
				{ type: "message", timestamp: smTs(2), message: { role: "hookMessage", content: "hm", timestamp: smTs(2) } },
				{ type: "compaction", timestamp: smTs(3), summary: "summarized", tokensBefore: 42, firstKeptEntryIndex: 1 },
				{ type: "message", timestamp: smTs(4), message: { role: "assistant", content: [{ type: "text", text: "post" }], provider: "anthropic", model: "claude-sonnet-4", timestamp: smTs(4) } },
			],
		},
	},
	{
		name: "session-manager/migrate-v1-compaction-index-out-of-range",
		tool: "session-manager",
		normalizeSessionIds: true,
		args: {
			fn: "migrateSessionEntries",
			entries: [
				smHeader(1),
				{ type: "message", timestamp: smTs(1), message: { role: "user", content: "hi", timestamp: smTs(1) } },
				{ type: "compaction", timestamp: smTs(2), summary: "s", tokensBefore: 1, firstKeptEntryIndex: 99 },
				{ type: "compaction", timestamp: smTs(3), summary: "s2", tokensBefore: 2, firstKeptEntryIndex: 0 },
			],
		},
	},
	{
		name: "session-manager/migrate-v2-hook-message",
		tool: "session-manager",
		args: {
			fn: "migrateSessionEntries",
			entries: [
				smHeader(2),
				smMsg("e1", null, { role: "hookMessage", content: "hm", timestamp: smTs(1) }, 1),
				smUser("e2", "e1", "hello", 2),
				smMsg("e3", "e2", { role: "hookMessage", content: [{ type: "text", text: "blocks" }], timestamp: smTs(3) }, 3),
			],
		},
	},
	{
		name: "session-manager/migrate-v3-noop",
		tool: "session-manager",
		args: {
			fn: "migrateSessionEntries",
			entries: [
				smHeader(3),
				smUser("e1", null, "hello", 1),
				smMsg("e2", "e1", { role: "custom", content: "already custom", timestamp: smTs(2) }, 2),
			],
		},
	},
	{
		name: "session-manager/migrate-no-header",
		tool: "session-manager",
		normalizeSessionIds: true,
		args: {
			fn: "migrateSessionEntries",
			entries: [
				{ type: "message", timestamp: smTs(1), message: { role: "user", content: "hi", timestamp: smTs(1) } },
				{ type: "message", timestamp: smTs(2), message: { role: "assistant", content: "yo", timestamp: smTs(2) } },
			],
		},
	},
	{
		name: "session-manager/migrate-string-version",
		tool: "session-manager",
		args: {
			fn: "migrateSessionEntries",
			entries: [
				{ ...smHeader(2), version: "2" },
				smMsg("e1", null, { role: "hookMessage", content: "hm", timestamp: smTs(1) }, 1),
			],
		},
	},
	{
		name: "session-manager/latest-compaction",
		tool: "session-manager",
		args: {
			fn: "getLatestCompactionEntry",
			entries: [
				smUser("e1", null, "q1", 1),
				{ type: "compaction", id: "c1", parentId: "e1", timestamp: smTs(2), summary: "first", tokensBefore: 10, firstKeptEntryId: "e1" },
				smUser("e2", "c1", "q2", 3),
				{ type: "compaction", id: "c2", parentId: "e2", timestamp: smTs(4), summary: "second", tokensBefore: 20, firstKeptEntryId: "e2" },
			],
		},
	},
	{
		name: "session-manager/latest-compaction-none",
		tool: "session-manager",
		args: { fn: "getLatestCompactionEntry", entries: [smUser("e1", null, "q1", 1)] },
	},
	{
		name: "session-manager/entry-user",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: smUser("e1", null, "hello", 1) },
	},
	{
		name: "session-manager/entry-assistant",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: smAssistant("e1", null, "hi!", 2) },
	},
	{
		name: "session-manager/entry-system",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: smSystem("e1", null, "You are pi.", 1) },
	},
	{
		name: "session-manager/entry-system-null-content",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: smMsg("e1", null, { role: "system", content: null, timestamp: smTs(1) }, 1) },
	},
	{
		name: "session-manager/entry-user-null-content",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: smMsg("e1", null, { role: "user", content: null, timestamp: smTs(1) }, 1) },
	},
	{
		name: "session-manager/entry-assistant-missing-content",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: smMsg("e1", null, { role: "assistant", timestamp: smTs(1) }, 1) },
	},
	{
		name: "session-manager/entry-toolresult",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: smMsg("e1", null, { role: "toolResult", content: [{ type: "text", text: "out" }], toolCallId: "call-1", toolName: "bash", timestamp: smTs(1) }, 1),
		},
	},
	{
		name: "session-manager/entry-custom-message",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "custom_message", id: "e1", parentId: null, timestamp: smTs(1), customType: "hook", content: [{ type: "text", text: "x" }], display: true, details: { k: 1 } },
		},
	},
	{
		name: "session-manager/entry-custom-message-null-content",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "custom_message", id: "e1", parentId: null, timestamp: smTs(1), customType: "hook" },
		},
	},
	{
		name: "session-manager/entry-custom-message-bad-timestamp",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "custom_message", id: "e1", parentId: null, timestamp: "bogus", customType: "hook", content: "txt", display: false },
		},
	},
	{
		name: "session-manager/entry-branch-summary",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "branch_summary", id: "e1", parentId: null, timestamp: smTs(5), fromId: "b2", summary: "abandoned path did X" },
		},
	},
	{
		name: "session-manager/entry-branch-summary-empty",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "branch_summary", id: "e1", parentId: null, timestamp: smTs(5), fromId: "b2", summary: "" },
		},
	},
	{
		name: "session-manager/entry-compaction-with-system",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: {
				type: "compaction",
				id: "c1",
				parentId: null,
				timestamp: smTs(6),
				summary: "summarized",
				tokensBefore: 500,
				firstKeptEntryId: "e3",
				systemMessage: { role: "system", content: "You are pi.", timestamp: smTs(0) },
			},
		},
	},
	{
		name: "session-manager/entry-compaction-no-system",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "compaction", id: "c1", parentId: null, timestamp: smTs(6), summary: "summarized", tokensBefore: 500, firstKeptEntryId: "e3" },
		},
	},
	{
		name: "session-manager/entry-label",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: { type: "label", id: "l1", parentId: null, timestamp: smTs(7), targetId: "e1", label: "good" } },
	},
	{
		name: "session-manager/entry-session-info",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: { type: "session_info", id: "s1", parentId: null, timestamp: smTs(7), name: "my session" } },
	},
	{
		name: "session-manager/entry-thinking-level-change",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: { type: "thinking_level_change", id: "t1", parentId: null, timestamp: smTs(7), thinkingLevel: "high" } },
	},
	{
		name: "session-manager/entry-custom",
		tool: "session-manager",
		args: { fn: "sessionEntryToContextMessages", entry: { type: "custom", id: "x1", parentId: null, timestamp: smTs(7), customType: "state", data: { a: 1 } } },
	},
	{
		name: "session-manager/entry-usage",
		tool: "session-manager",
		args: {
			fn: "sessionEntryToContextMessages",
			entry: { type: "usage", id: "u1", parentId: null, timestamp: smTs(7), usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, cost: { input: 0.001, output: 0.002, total: 0.003 } } },
		},
	},
	{
		name: "session-manager/context-entries-linear",
		tool: "session-manager",
		args: {
			fn: "buildContextEntries",
			entries: [smUser("e1", null, "q1", 1), smAssistant("e2", "e1", "a1", 2), smUser("e3", "e2", "q2", 3), smAssistant("e4", "e3", "a2", 4)],
		},
	},
	{
		name: "session-manager/context-entries-compaction",
		tool: "session-manager",
		args: {
			fn: "buildContextEntries",
			entries: [
				smSystem("s1", null, "You are pi.", 0),
				smUser("e1", "s1", "q1", 1),
				smAssistant("e2", "e1", "a1", 2),
				smUser("e3", "e2", "q2", 3),
				smAssistant("e4", "e3", "a2", 4),
				{ type: "compaction", id: "c1", parentId: "e4", timestamp: smTs(5), summary: "summarized", tokensBefore: 400, firstKeptEntryId: "e3" },
				smUser("e5", "c1", "q3", 6),
				smAssistant("e6", "e5", "a3", 7),
			],
		},
	},
	{
		name: "session-manager/context-entries-leaf",
		tool: "session-manager",
		args: {
			fn: "buildContextEntries",
			leafId: "b2",
			entries: [
				smUser("e1", null, "q1", 1),
				smAssistant("e2", "e1", "a1", 2),
				smUser("e3", "e2", "q2", 3),
				smAssistant("e4", "e3", "a2", 4),
				smUser("b1", "e2", "other branch", 5),
				smAssistant("b2", "b1", "other answer", 6),
			],
		},
	},
	{
		name: "session-manager/context-entries-leaf-null",
		tool: "session-manager",
		args: {
			fn: "buildContextEntries",
			leafId: null,
			entries: [smUser("e1", null, "q1", 1), smAssistant("e2", "e1", "a1", 2)],
		},
	},
	{
		name: "session-manager/context-entries-leaf-missing",
		tool: "session-manager",
		args: {
			fn: "buildContextEntries",
			leafId: "nope",
			entries: [smUser("e1", null, "q1", 1), smAssistant("e2", "e1", "a1", 2)],
		},
	},
	{
		name: "session-manager/context-entries-empty",
		tool: "session-manager",
		args: { fn: "buildContextEntries", entries: [] },
	},
	{
		name: "session-manager/projection-basic",
		tool: "session-manager",
		args: {
			fn: "buildSessionProjection",
			entries: [
				smUser("e1", null, "q1", 1),
				{ type: "thinking_level_change", id: "t1", parentId: "e1", timestamp: smTs(2), thinkingLevel: "high" },
				{ type: "model_change", id: "m1", parentId: "t1", timestamp: smTs(3), provider: "openai", modelId: "gpt-5" },
				smAssistant("e2", "m1", "a1", 4),
			],
		},
	},
	{
		name: "session-manager/projection-assistant-model",
		tool: "session-manager",
		args: {
			fn: "buildSessionProjection",
			entries: [smUser("e1", null, "q1", 1), smAssistant("e2", "e1", "a1", 2)],
		},
	},
	{
		name: "session-manager/projection-context-edit",
		tool: "session-manager",
		args: {
			fn: "buildSessionProjection",
			entries: [
				smUser("e1", null, "q1", 1),
				smAssistant("e2", "e1", "a1", 2),
				{ type: "context_edit", id: "x1", parentId: "e2", timestamp: smTs(3), targetId: "e1", replacement: { role: "user", content: "edited question" } },
				{ type: "context_edit", id: "x2", parentId: "x1", timestamp: smTs(4), targetId: "e2", replacement: { role: "assistant", content: "edited answer string" } },
			],
		},
	},
	{
		name: "session-manager/projection-context-edit-delete",
		tool: "session-manager",
		args: {
			fn: "buildSessionProjection",
			entries: [
				smUser("e1", null, "q1", 1),
				{ type: "context_edit", id: "x1", parentId: "e1", timestamp: smTs(2), targetId: "e1", replacement: null },
			],
		},
	},
	{
		name: "session-manager/projection-context-edit-blocks",
		tool: "session-manager",
		args: {
			fn: "buildSessionProjection",
			entries: [
				smUser("e1", null, "q1", 1),
				{ type: "context_edit", id: "x1", parentId: "e1", timestamp: smTs(2), targetId: "e1", replacement: { role: "user", content: [{ type: "text", text: "block form" }] } },
			],
		},
	},
	{
		name: "session-manager/context-basic",
		tool: "session-manager",
		args: {
			fn: "buildSessionContext",
			entries: [
				smUser("e1", null, "q1", 1),
				{ type: "branch_summary", id: "bs1", parentId: "e1", timestamp: smTs(2), fromId: "e1", summary: "took a different path" },
				smUser("e2", "bs1", "q2", 3),
			],
		},
	},
	{
		name: "session-manager/default-session-dir",
		tool: "session-manager",
		args: { fn: "getDefaultSessionDir", cwd: "/home/user/my proj", agentDir: "agent" },
	},
	{
		name: "session-manager/default-session-dir-special-chars",
		tool: "session-manager",
		args: { fn: "getDefaultSessionDir", cwd: "/a/b:c\\d", agentDir: "agent" },
	},
	{
		name: "session-manager/default-session-dir-relative-cwd",
		tool: "session-manager",
		args: { fn: "getDefaultSessionDir", cwd: "relative/proj", agentDir: "agent" },
	},
	{
		name: "session-manager/load-basic",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "sess.jsonl" },
		setup: {
			"sess.jsonl": [JSON.stringify(smHeader(3)), JSON.stringify(smUser("e1", null, "q1", 1)), JSON.stringify(smAssistant("e2", "e1", "a1", 2))].join("\n") + "\n",
		},
	},
	{
		name: "session-manager/load-repairs-missing-newline",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "sess.jsonl" },
		setup: {
			"sess.jsonl": [JSON.stringify(smHeader(3)), JSON.stringify(smUser("e1", null, "q1", 1))].join("\n"),
		},
	},
	{
		name: "session-manager/load-no-header",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "sess.jsonl" },
		setup: {
			"sess.jsonl": JSON.stringify(smUser("e1", null, "q1", 1)) + "\n",
		},
	},
	{
		name: "session-manager/load-header-no-id",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "sess.jsonl" },
		setup: {
			"sess.jsonl": JSON.stringify({ type: "session", version: 3, timestamp: smTs(0), cwd: "/x" }) + "\n",
		},
	},
	{
		name: "session-manager/load-missing",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "nope.jsonl" },
	},
	{
		name: "session-manager/load-empty",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "sess.jsonl" },
		setup: { "sess.jsonl": "" },
	},
	{
		name: "session-manager/load-malformed-lines",
		tool: "session-manager",
		args: { fn: "loadEntriesFromFile", filePath: "sess.jsonl" },
		setup: {
			"sess.jsonl": ["", "garbage", JSON.stringify(smHeader(3)), "", JSON.stringify(smUser("e1", null, "q1", 1)), "{broken"].join("\n") + "\n",
		},
	},
	{
		name: "session-manager/find-recent-match",
		tool: "session-manager",
		args: { fn: "findMostRecentSession", sessionDir: "sessions", cwd: "/tmp/pi-find-test/proj" },
		setup: {
			"sessions/a.jsonl": JSON.stringify({ ...smHeader(3), cwd: "/tmp/pi-find-test/proj" }) + "\n",
		},
	},
	{
		name: "session-manager/find-recent-cwd-mismatch",
		tool: "session-manager",
		args: { fn: "findMostRecentSession", sessionDir: "sessions", cwd: "/tmp/pi-find-test/other" },
		setup: {
			"sessions/a.jsonl": JSON.stringify({ ...smHeader(3), cwd: "/tmp/pi-find-test/proj" }) + "\n",
		},
	},
	{
		name: "session-manager/find-recent-no-dir",
		tool: "session-manager",
		args: { fn: "findMostRecentSession", sessionDir: "nope", cwd: "/tmp/pi-find-test/proj" },
	},
	{
		name: "session-manager/find-recent-no-cwd-filter",
		tool: "session-manager",
		args: { fn: "findMostRecentSession", sessionDir: "sessions" },
		setup: {
			"sessions/a.jsonl": JSON.stringify({ ...smHeader(3), cwd: "/somewhere/else" }) + "\n",
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
	// --------------------------------------------------------- nested-calls
	{
		name: "nested-calls/limits",
		tool: "nested-calls",
		args: { fn: "limits" },
	},
	{
		name: "nested-calls/recorder-basic-ok",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{ op: "start", toolCall: { id: "t1", name: "read", arguments: { path: "/a" } } },
				{ op: "finish", index: 0, isError: false, error: "" },
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-error-truncation",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{ op: "start", toolCall: { id: "t1", name: "read", arguments: { path: "/a" } } },
				{ op: "start", toolCall: { id: "t2", name: "edit", arguments: { path: "/b" } } },
				{ op: "finish", index: 1, isError: true, error: "boom ".repeat(120) },
				{ op: "addUsage", usage: { input: 10, output: 5 } },
				{ op: "totalUsage" },
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-null-arguments",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{ op: "start", toolCall: { id: "t1", name: "read" } },
				{ op: "start", toolCall: { id: "t2", name: "read", arguments: null } },
				{ op: "finish", index: 0, isError: false, error: "" },
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-drop-max-calls",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				...Array.from({ length: 257 }, (_, i) => ({
					op: "start",
					toolCall: { id: `t${i}`, name: "read", arguments: { path: "/a" } },
				})),
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-args-too-big-per-call",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{ op: "start", toolCall: { id: "t1", name: "write", arguments: { path: "/".padEnd(9000, "x") } } },
				{ op: "finish", index: 0, isError: false, error: "" },
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-args-total-budget",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				...Array.from({ length: 5 }, (_, i) => ({
					op: "start",
					toolCall: { id: `t${i}`, name: "write", arguments: { path: "/".padEnd(7000, "y") } },
				})),
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-snapshot-empty",
		tool: "nested-calls",
		args: { fn: "recorder", ops: [{ op: "snapshot" }] },
	},
	{
		name: "nested-calls/recorder-snapshot-unfinished",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{ op: "start", toolCall: { id: "t1", name: "read", arguments: { path: "/a" } } },
				{ op: "snapshot" },
			],
		},
	},
	{
		name: "nested-calls/recorder-add-usage-combine",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{
					op: "addUsage",
					usage: {
						input: 10,
						output: 5,
						cacheRead: 1,
						cacheWrite: 2,
						totalTokens: 18,
						cost: { input: 0.001, output: 0.002, cacheRead: 0, cacheWrite: 0, total: 0.003 },
					},
				},
				{
					op: "addUsage",
					usage: {
						input: 1,
						output: 2,
						cacheRead: 0,
						cacheWrite: 1,
						cacheWrite1h: 3,
						totalTokens: 4,
						cost: { input: 0.0001, output: 0.0002, cacheRead: 0, cacheWrite: 0, total: 0.0003 },
					},
				},
				{ op: "totalUsage" },
			],
		},
	},
	{
		name: "nested-calls/recorder-total-usage-none",
		tool: "nested-calls",
		args: { fn: "recorder", ops: [{ op: "totalUsage" }] },
	},
	{
		name: "nested-calls/recorder-finish-null-record",
		tool: "nested-calls",
		args: {
			fn: "recorder",
			ops: [
				{ op: "start", toolCall: { id: "t1", name: "read", arguments: { path: "/a" } } },
				{ op: "finish", index: null, isError: true, error: "nope" },
				{ op: "snapshot" },
			],
		},
	},

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
