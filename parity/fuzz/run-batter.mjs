#!/usr/bin/env node
/**
 * T7 batter runner — deterministic differential fuzzing over the parity
 * harness boundary (see lib.mjs for the framework description).
 *
 * Usage:
 *   node parity/fuzz/run-batter.mjs
 *     [--target all|unicode,keys,stdin,latex,markdown]
 *     [--count N]        (per-target case count; overrides defaults)
 *     [--seed S]         (default 1)
 *     [--budget SECS]    (wall-clock budget, default 900)
 *     [--no-shrink]      (skip ddmin minimization of divergences)
 *     [--list]           (print target defaults and exit)
 *
 * Targets:
 *   unicode  tui-utils      visibleWidth / slice* / truncate / extract /
 *                           wrap / grapheme-cell / osc8 / strip-ansi over
 *                           unicode-soup + ANSI-dense lines
 *   keys     tui-keys       parseKey / decode* / matches over escape-ish
 *                           input strings (valid + malformed fragments)
 *   stdin    tui-stdin-buf  chunk scripts (string / raw bytes / u16 units
 *                           — units may include lone surrogates) with
 *                           timeout firing points
 *   latex    tui-latex      command/symbol/brace soup incl. astral chars
 *   markdown tui-markdown-lex  block/inline token soup incl. astral chars
 *
 * On divergence: the input is delta-debugged (ddmin) to a minimal repro and
 * written to parity/regressions/ as a permanent parity case; the batter
 * keeps going (capped at 20 recorded regressions per run).
 *
 * Exit code 0 = no divergences (or all recorded); 1 = divergences found.
 */
import {
	rng,
	pick,
	int,
	bool,
	sampleUnicode,
	sampleAnsiLine,
	sampleBytes,
	sampleKeyString,
	runBatch,
	checkCase,
	ddmin,
	stringToChunks,
	writeRegression,
	canonical,
} from "./lib.mjs";

const args = process.argv.slice(2);
const opt = (name, dflt) => {
	const i = args.indexOf(`--${name}`);
	return i >= 0 && args[i + 1] !== undefined ? args[i + 1] : dflt;
};
const targets = (opt("target", "all") === "all"
	? ["unicode", "keys", "stdin", "latex", "markdown"]
	: opt("target", "all").split(",")
).map((s) => s.trim());
const countOverride = parseInt(opt("count", "0"), 10);
const seed = parseInt(opt("seed", "1"), 10);
const budgetSecs = parseInt(opt("budget", "900"), 10);
const shrink = !args.includes("--no-shrink");

// ---------------------------------------------------------------- targets

const ELLIPSES = ["...", "…", ">", "!?", "😀"];
const KEY_IDS = ["up", "down", "left", "right", "enter", "escape", "tab", "backspace", "delete", "home", "end", "pageup", "space", "ctrl+a", "ctrl+c", "shift+tab", "f1", "a"];

const LATEX_TOKENS = [
	"\\alpha", "\\beta", "\\gamma", "\\pi", "\\infty", "\\sum", "\\int",
	"\\frac", "\\sqrt", "\\text", "\\mathbb", "\\hat", "\\vec", "\\bar",
	"\\overline", "\\underline", "\\pm", "\\times", "\\cdot", "\\leq",
	"\\geq", "\\neq", "\\approx", "\\rightarrow", "\\left(", "\\right)",
	"\\begin{matrix}", "\\end{matrix}", "\\begin{cases}", "\\end{cases}",
	"&", "\\\\", "^", "_", "{", "}", "(", ")", "[", "]", "+", "-", "=",
	"x", "y", "n", "2", "12", "abc", "\\sin", "\\cos", "\\lim", "\\quad",
	"\\,", "\\;", "\\!", "\\pmod", "\\not", "\\displaystyle", "\\left",
	"\\right", "\\prod", "\\partial", "\\nabla", "\\emptyset",
];

const MD_TOKENS = [
	"# ", "## ", "### ", "```", "```js\n", "`", "**", "__", "*", "_",
	"- ", "* ", "+ ", "1. ", "2. ", "> ", "| a | b |\n", "|--|--|\n",
	"[", "](", ")", "![", "http://x.com", "<https://x.com>", "<b>", "</b>",
	"---", "***", "\n", "\n\n", "  \n", "\\\n", "    indented", "\t",
	"~~", "^", "++", ":", "hello", "world", "foo_bar", "a.b.c",
];

/** unicode: one tui-utils invocation per case. */
function genUnicode(r) {
	const fn = pick(r, [
		"visibleWidth",
		"stripTerminalSequences",
		"normalizeTerminalOutput",
		"getActiveBackgroundAnsi",
		"getGraphemeCellRange",
		"getOsc8LinkAtColumn",
		"truncateToWidth",
		"sliceByColumn",
		"sliceWithWidth",
		"extractSegments",
		"wrapTextWithAnsi",
		"isWhitespaceChar",
		"isPunctuationChar",
	]);
	const soup = bool(r) ? sampleAnsiLine(r) : sampleUnicode(r, { maxLen: 40 });
	switch (fn) {
		case "getGraphemeCellRange":
			return { fn, line: soup, column: int(r, 0, 30) };
		case "getOsc8LinkAtColumn":
			return { fn, line: soup, column: int(r, 0, 30) };
		case "truncateToWidth": {
			const a = { fn, text: soup, maxWidth: int(r, 0, 20) };
			if (bool(r)) a.ellipsis = pick(r, ELLIPSES);
			if (bool(r)) a.pad = bool(r);
			return a;
		}
		case "sliceByColumn":
		case "sliceWithWidth": {
			const a = { fn, line: soup, startCol: int(r, 0, 20), length: int(r, 0, 20) };
			if (bool(r)) a.strict = bool(r);
			return a;
		}
		case "extractSegments": {
			const a = {
				fn,
				line: soup,
				beforeEnd: int(r, 0, 15),
				afterStart: int(r, 0, 15),
				afterLen: int(r, 0, 15),
			};
			if (bool(r)) a.strictAfter = bool(r);
			return a;
		}
		case "wrapTextWithAnsi":
			return { fn, text: soup + (bool(r) ? "\n" + sampleUnicode(r, { maxLen: 20 }) : ""), width: int(r, 1, 24) };
		case "isWhitespaceChar":
		case "isPunctuationChar":
			return { fn, char: sampleUnicode(r, { maxLen: 1 }) };
		default:
			return { fn, text: soup };
	}
}

/** keys: one payload = ~40 sub-cases through the native batch entry. */
const KEYS_PER_PAYLOAD = 40;
function genKeysPayload(r) {
	const cases = [];
	for (let i = 0; i < KEYS_PER_PAYLOAD; i++) {
		const c = { input: sampleKeyString(r), kitty: r() < 0.3 };
		if (bool(r)) c.keyIds = pick(r, [KEY_IDS.slice(0, 4), KEY_IDS.slice(4, 8), ["up", "enter", "ctrl+a"]]);
		cases.push(c);
	}
	return { cases };
}

/** stdin: one payload = ~25 chunk-script sub-cases. */
const STDIN_PER_PAYLOAD = 25;
function genStdinPayload(r) {
	const cases = [];
	for (let i = 0; i < STDIN_PER_PAYLOAD; i++) {
		const chunks = [];
		const n = int(r, 1, 5);
		for (let j = 0; j < n; j++) {
			const kind = int(r, 0, 2);
			if (kind === 0) chunks.push(sampleKeyString(r, { maxLen: 8 }));
			else if (kind === 1) chunks.push({ bytes: sampleBytes(r, { maxLen: 10 }) });
			else {
				// u16 units — may include lone surrogates (Rust takes Vec<u16>)
				const units = [];
				const m = int(r, 1, 8);
				for (let k = 0; k < m; k++) {
					const roll = r();
					if (roll < 0.15) units.push(int(r, 0xd800, 0xdfff)); // surrogate
					else if (roll < 0.3) units.push(int(r, 0, 0x7f)); // control
					else units.push(int(r, 0x20, 0x7e)); // printable
				}
				chunks.push({ units });
			}
		}
		const script = { chunks };
		if (bool(r)) script.fireTimeoutAfterEachChunk = true;
		else if (bool(r)) script.fireTimeoutAt = [int(r, 0, n - 1)];
		cases.push({
			name: `b${cases.length}`,
			kitty: r() < 0.3,
			timeoutMs: pick(r, [50, 10, 0]),
			escapeTimeoutMs: pick(r, [10, 5, 0]),
			script,
		});
	}
	return { cases };
}

/** latex: one render invocation per case. */
function genLatex(r) {
	const n = int(r, 1, 14);
	let source = "";
	for (let i = 0; i < n; i++) {
		const roll = r();
		if (roll < 0.7) source += pick(r, LATEX_TOKENS);
		else if (roll < 0.85) source += sampleUnicode(r, { maxLen: 3 });
		else source += pick(r, [" ", "  ", "\n"]);
	}
	return { fn: "render", source, display: bool(r) };
}

/** markdown: one lex invocation per case. */
function genMarkdown(r) {
	const n = int(r, 1, 24);
	let source = "";
	for (let i = 0; i < n; i++) {
		const roll = r();
		if (roll < 0.75) source += pick(r, MD_TOKENS);
		else if (roll < 0.9) source += sampleUnicode(r, { maxLen: 4 });
		else source += pick(r, [" ", "  ", "\t"]);
	}
	return { fn: "lex", source };
}

// ------------------------------------------------- per-target shrinkers

/** Build a ddmin test + regression case for a string-field divergence. */
async function shrinkStringField(target, tool, args, field, rebuildArgs) {
	const original = args[field];
	const chunks = stringToChunks(original);
	const minimal = await ddmin(chunks, async (parts) => {
		const candidate = rebuildArgs(parts.join(""));
		const res = checkCase(tool, candidate);
		return res && !res.ok;
	});
	const shrunk = minimal ? minimal.join("") : original;
	const caseArgs = rebuildArgs(shrunk);
	return { shrunk, caseArgs };
}

// ------------------------------------------------------------ the runner

const TARGETS = {
	unicode: {
		tool: "tui-utils",
		count: countOverride || 3000,
		batchSize: 200,
		gen: genUnicode,
		shrink: async (args) => {
			const f = args.text !== undefined ? "text" : args.line !== undefined ? "line" : null;
			if (!f) return null;
			return shrinkStringField("unicode", "tui-utils", args, f, (s) => ({ ...args, [f]: s }));
		},
	},
	keys: {
		tool: "tui-keys",
		count: countOverride || 1200,
		batchSize: 1, // each payload is a native ~40-case batch
		gen: genKeysPayload,
		/** Find the failing sub-case, shrink its input string. */
		shrink: async (args) => {
			// Bisect: re-run each sub-case as its own payload.
			for (const sub of args.cases) {
				const res = checkCase("tui-keys", { cases: [sub] });
				if (res && !res.ok) {
					const minimal = await ddmin(stringToChunks(sub.input), async (parts) => {
						const r2 = checkCase("tui-keys", { cases: [{ ...sub, input: parts.join("") }] });
						return r2 && !r2.ok;
					});
					const shrunk = minimal ? minimal.join("") : sub.input;
					const caseArgs = { cases: [{ ...sub, input: shrunk }] };
					return { shrunk, caseArgs };
				}
			}
			return null;
		},
	},
	stdin: {
		tool: "tui-stdin-buffer",
		count: countOverride || 800,
		batchSize: 1, // each payload is a native ~25-case batch
		gen: genStdinPayload,
		shrink: async (args) => {
			for (const sub of args.cases) {
				const res = checkCase("tui-stdin-buffer", { cases: [sub] });
				if (res && !res.ok) {
					// Shrink string chunks; drop non-string chunks wholesale.
					const chunks = sub.script.chunks;
					const parts = chunks.flatMap((c) => (typeof c === "string" ? [...c] : [c]));
					const minimal = await ddmin(parts, async (parts2) => {
						const script = { ...sub.script, chunks: reassemble(parts2) };
						const r2 = checkCase("tui-stdin-buffer", { cases: [{ ...sub, script }] });
						return r2 && !r2.ok;
					});
					const script = { ...sub.script, chunks: reassemble(minimal ?? parts) };
					return { shrunk: JSON.stringify(script), caseArgs: { cases: [{ ...sub, script }] } };
				}
			}
			return null;
		},
	},
	latex: {
		tool: "tui-latex",
		count: countOverride || 2000,
		batchSize: 200,
		gen: genLatex,
		shrink: (args) => shrinkStringField("latex", "tui-latex", args, "source", (s) => ({ fn: "render", source: s, display: args.display })),
	},
	markdown: {
		tool: "tui-markdown-lex",
		count: countOverride || 2000,
		batchSize: 200,
		gen: genMarkdown,
		shrink: (args) => shrinkStringField("markdown", "tui-markdown-lex", args, "source", (s) => ({ fn: "lex", source: s })),
	},
};

/** Re-concatenate adjacent string chunks into chunk objects for stdin scripts. */
function reassemble(parts) {
	const chunks = [];
	for (const p of parts) {
		if (typeof p === "string" && typeof chunks[chunks.length - 1] === "string") chunks[chunks.length - 1] += p;
		else chunks.push(p);
	}
	return chunks.filter((c) => c !== "");
}

if (args.includes("--list")) {
	for (const [name, t] of Object.entries(TARGETS)) {
		console.log(`${name.padEnd(9)} tool=${t.tool.padEnd(18)} count=${t.count}`);
	}
	process.exit(0);
}

// ------------------------------------------------------------ main loop

const startMs = Date.now();
const budgetMs = budgetSecs * 1000;
let totalCases = 0;
let totalMismatches = 0;
let regressionsWritten = 0;
const MAX_REGRESSIONS = 20;
const summary = [];
let exitCode = 0;

for (const name of targets) {
	const t = TARGETS[name];
	if (!t) {
		console.error(`unknown target: ${name}`);
		process.exit(2);
	}
	if (Date.now() - startMs > budgetMs) {
		console.log(`[budget] skipping ${name} (out of time)`);
		continue;
	}
	const r = rng(seed * 7919 + name.length * 104729 + name.charCodeAt(0));
	const argsList = [];
	for (let i = 0; i < t.count; i++) argsList.push(t.gen(r));

	const t0 = Date.now();
	const results = runBatch(t.tool, argsList, { batchSize: t.batchSize });
	const secs = ((Date.now() - t0) / 1000).toFixed(1);
	// "cases" for native-batch targets = sub-cases inside payloads
	const caseCount = name === "keys" ? argsList.length * KEYS_PER_PAYLOAD : name === "stdin" ? argsList.length * STDIN_PER_PAYLOAD : argsList.length;

	let mismatches = 0;
	let firstIdx = -1;
	for (let i = 0; i < results.length; i++) {
		if (results[i].ok) continue;
		mismatches++;
		totalMismatches++;
		if (firstIdx < 0) firstIdx = i;
		console.log(`\n[${name}] DIVERGENCE case ${i}:`);
		console.log(`  args: ${JSON.stringify(results[i].args).slice(0, 400)}`);
		const tsC = canonical(results[i].ts);
		const rsC = canonical(results[i].rs);
		console.log(`  ts: ${tsC.slice(0, 400)}`);
		console.log(`  rs: ${rsC.slice(0, 400)}`);
		if (shrink && regressionsWritten < MAX_REGRESSIONS) {
			try {
				const shrunk = await t.shrink(results[i].args);
				if (shrunk) {
					// Confirm the minimized case still diverges.
					const confirm = checkCase(t.tool, shrunk.caseArgs);
					if (confirm && !confirm.ok) {
						const regName = writeRegression(name, { tool: t.tool, args: shrunk.caseArgs });
						regressionsWritten++;
						console.log(`  -> minimized (${shrunk.shrunk.length} chars) and recorded: parity/regressions/${regName}.mjs`);
					} else {
						console.log(`  -> minimized case no longer diverges (flaky?) — recorded raw case`);
						const regName = writeRegression(name, { tool: t.tool, args: results[i].args });
						regressionsWritten++;
					}
				}
			} catch (e) {
				console.log(`  -> shrink failed: ${e && e.message}`);
			}
		}
	}
	totalCases += caseCount;
	summary.push({ target: name, cases: caseCount, mismatches, firstDivergence: firstIdx, secs });
	console.log(
		`[${name}] ${caseCount} cases, ${mismatches} divergences${firstIdx >= 0 ? ` (first at ${firstIdx})` : ""}, ${secs}s`,
	);
	if (mismatches) exitCode = 1;
}

console.log(`\n=== batter summary (seed ${seed}, ${((Date.now() - startMs) / 1000).toFixed(0)}s) ===`);
for (const s of summary) {
	console.log(
		`${s.target.padEnd(9)} ${String(s.cases).padStart(6)} cases  ${String(s.mismatches).padStart(3)} divergences  ${s.secs}s${s.mismatches ? `  first@${s.firstDivergence}` : ""}`,
	);
}
console.log(`total: ${totalCases} cases, ${totalMismatches} divergences, ${regressionsWritten} regressions recorded`);
process.exit(exitCode);
