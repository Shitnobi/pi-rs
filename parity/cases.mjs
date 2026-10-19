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
];

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
