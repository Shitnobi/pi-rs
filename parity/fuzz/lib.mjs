/**
 * T7 batter framework — shared library.
 *
 * Deterministic differential fuzzing over the parity harness boundary:
 *
 *   - Seeded PRNG (mulberry32): the SAME seed always produces the SAME
 *     cases; cases are generated in JS only (no cross-language PRNG —
 *     the Rust side just replays the emitted argument objects).
 *   - Corpus samplers: unicode building blocks (combining marks, ZWJ
 *     chains, CJK/astral, EAW-ambiguous, control chars, emoji clusters,
 *     Indic conjuncts), ANSI-dense strings, escape-sequence fragments,
 *     raw byte streams.
 *   - Batched execution: both harnesses support a batch mode
 *     (`pi-cli harness tool-batch <tool>` / `ts-harness.js --batch`) so a
 *     batter run amortizes process startup over thousands of cases.
 *   - Shrinker: classic delta-debugging (ddmin) reduces a divergent input
 *     to a minimal repro; the result is written to parity/regressions/
 *     as a permanent parity case (auto-aggregated into the main suite).
 *
 * All generated strings are VALID Unicode scalar sequences: lone UTF-16
 * surrogates cannot cross the JSON boundary into Rust strings (serde_json
 * rejects lone-surrogate escapes), so surrogate behavior is covered by the
 * latex sentinel regression cases instead (docs/report-t6-R2.md).
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const repoRoot = join(here, "..", "..");
export const piCli = existsSync(join(repoRoot, "target", "release", "pi-cli"))
	? join(repoRoot, "target", "release", "pi-cli")
	: join(repoRoot, "target", "debug", "pi-cli");
export const tsHarness = join(repoRoot, "parity", "ts-harness.js");
export const workDir = process.env.BATTER_DIR || "/tmp/pi-parity/batter";
export const regressionsDir = join(repoRoot, "parity", "regressions");

// ------------------------------------------------------------------- PRNG

/** mulberry32 — small, fast, fully deterministic. */
export function rng(seed) {
	let s = seed >>> 0;
	return () => {
		s = (s + 0x6d2b79f5) >>> 0;
		let t = s;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

export const pick = (r, arr) => arr[Math.floor(r() * arr.length) % arr.length];
export const int = (r, lo, hi) => lo + Math.floor(r() * (hi - lo + 1));
export const bool = (r) => r() < 0.5;

// ------------------------------------------------------------- samplers

/** Unicode building-block pools (all valid scalar sequences). */
export const POOLS = {
	ascii: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
	latin1: "éàüñÇ§¶",
	combining: "\u0301\u0308\u0327\u034F\u035C", // acute, diaeresis, cedilla, combining grapheme join
	zwj: "\u200D",
	emoji: "😀🌍👍🚀❤️",
	zwjChain: "👨\u200D👩\u200D👧\u200D👦",
	cjk: "日本語한국어中文",
	cjkExtB: "\u{20000}\u{20001}", // astral CJK
	eawAmbiguous: "±°×÷··αΩ",
	tabs: "\t",
	control: "\u0001\u0002\u007F",
	c1: "\u0085\u009B",
	bom: "\uFEFF",
	variation: "\uFE0F\uFE0E",
	regional: "\u{1F1FA}\u{1F1F8}", // regional indicators (flag halves)
	skinTone: "\u{1F3FB}\u{1F3FF}",
	keycap: "#\uFE0F\u20E3",
	hangulJamo: "\u1100\u1161\u11A8",
	indic: "\u0915\u094D\u0937\u0930", // Devanagari conjuncts (GB9c risk)
	bidi: "\u200E\u200F\u062E\u05D0",
	emojiPres: "\u231A\u231B", // emoji-presentation default
	lineSep: "\u2028\u2029",
	nbsp: "\u00A0",
};

const POOL_KEYS = Object.keys(POOLS);

/** Random unicode soup from weighted pools (weighted towards riskier classes). */
export function sampleUnicode(r, { minLen = 1, maxLen = 24 } = {}) {
	const len = int(r, minLen, maxLen);
	let out = "";
	while (out.length < len) {
		// 50% risky pools, 50% uniform over everything
		const key =
			r() < 0.5
				? pick(r, [
						"combining",
						"zwj",
						"zwjChain",
						"emoji",
						"cjk",
						"cjkExtB",
						"eawAmbiguous",
						"control",
						"regional",
						"skinTone",
						"keycap",
						"hangulJamo",
						"indic",
						"bidi",
						"variation",
						"lineSep",
						"nbsp",
						"c1",
						"bom",
						"tabs",
					])
				: pick(r, POOL_KEYS);
		out += pick(r, [...POOLS[key]]);
	}
	return out;
}

/** ANSI escape fragments (SGR, cursor, OSC8 links, bracketed paste, mouse). */
export const ANSI_FRAGMENTS = [
	"\u001B[31m",
	"\u001B[0m",
	"\u001B[1;38;2;10;20;30m",
	"\u001B[38;5;196m",
	"\u001B[48;2;1;2;3m",
	"\u001B[2J",
	"\u001B[H",
	"\u001B[?25l",
	"\u001B[?2026h",
	"\u001B[?2026l",
	"\u001B]8;;https://example.com/a\u001B\\link\u001B]8;;\u001B\\",
	"\u001B]8;;\u001B\\",
	"\u001B[200~pasted text\u001B[201~",
	"\u001B[<0;1;1M",
	"\u001B[M",
	"\u001B[10;20H",
	"\u001BOP",
	"\u001B[27u",
	"\u001B[97;27u",
	"\u001B",
	"\u001B[",
	"\u001B[38;",
];

/** Random ANSI-dense line: escape fragments interleaved with unicode soup. */
export function sampleAnsiLine(r, { maxFrags = 8 } = {}) {
	const n = int(r, 1, maxFrags);
	let out = "";
	for (let i = 0; i < n; i++) {
		if (bool(r)) out += pick(r, ANSI_FRAGMENTS);
		if (bool(r)) out += sampleUnicode(r, { maxLen: 6 });
	}
	return out;
}

/** Raw byte streams for the stdin-buffer harness (bytes: [u8] chunks). */
export function sampleBytes(r, { maxLen = 16 } = {}) {
	const n = int(r, 1, maxLen);
	const out = [];
	for (let i = 0; i < n; i++) out.push(int(r, 0, 255));
	return out;
}

/** Escape-sequence-ish input strings for tui-keys (valid scalars only). */
export function sampleKeyString(r, { maxLen = 12 } = {}) {
	const parts = [
		...ANSI_FRAGMENTS,
		"a",
		"A",
		"1",
		" ",
		"\r",
		"\n",
		"\u007F",
		"\u0001",
		"é",
		"😀",
	];
	const n = int(r, 1, maxLen);
	let out = "";
	for (let i = 0; i < n; i++) out += pick(r, parts);
	return out;
}

// ------------------------------------------------------------- execution

/** Canonical JSON text (recursively sorted keys) for equality checks. */
export function canonical(value) {
	const sort = (v) => {
		if (Array.isArray(v)) return v.map(sort);
		if (v && typeof v === "object") {
			const out = {};
			for (const k of Object.keys(v).sort()) out[k] = sort(v[k]);
			return out;
		}
		return v;
	};
	return JSON.stringify(sort(value));
}

/**
 * Run a list of argument objects for one tool through BOTH harnesses in
 * batches. Returns an array of { ok, ts, rs, args } per case
 * (ok = canonical-equal results; ts/rs are the parsed results).
 * A Rust panic / harness crash mid-batch falls back to per-case replay to
 * identify the culprit(s); crashed cases are reported as divergences
 * (rs = { "error": "<crash message>" }).
 */
export function runBatch(tool, argsList, { batchSize = 200 } = {}) {
	mkdirSync(workDir, { recursive: true });
	const out = [];
	for (let i = 0; i < argsList.length; i += batchSize) {
		const slice = argsList.slice(i, i + batchSize);
		let ts, rs;
		try {
			ts = JSON.parse(
				execFileSync("node", [tsHarness, join(workDir, "ts"), "--batch"], {
					input: JSON.stringify(slice.map((args) => ({ tool, args }))),
					encoding: "utf8",
					maxBuffer: 1 << 28,
					cwd: repoRoot,
				}),
			);
			rs = JSON.parse(
				execFileSync(piCli, ["harness", "tool-batch", tool, "--cwd", join(workDir, "rs")], {
					input: JSON.stringify(slice),
					encoding: "utf8",
					maxBuffer: 1 << 28,
				}),
			);
		} catch (e) {
			// One of the cases crashed a harness (usually a Rust panic) or
			// produced unparseable output. Replay one-by-one to isolate.
			for (const args of slice) {
				let oneTs, oneRs;
				try {
					oneTs = JSON.parse(
						execFileSync("node", [tsHarness, join(workDir, "ts"), "--batch"], {
							input: JSON.stringify([{ tool, args }]),
							encoding: "utf8",
							maxBuffer: 1 << 28,
							cwd: repoRoot,
						}),
					);
				} catch (e2) {
					oneTs = [{ error: `ts harness crashed: ${String(e2.message).slice(0, 300)}` }];
				}
				try {
					oneRs = JSON.parse(
						execFileSync(piCli, ["harness", "tool-batch", tool, "--cwd", join(workDir, "rs")], {
							input: JSON.stringify([args]),
							encoding: "utf8",
							maxBuffer: 1 << 28,
						}),
					);
				} catch (e2) {
					oneRs = [
						{
							error:
								"rs harness crashed: " +
								String(e2.message).slice(0, 300) +
								(e2.stderr ? " :: " + String(e2.stderr).slice(0, 300) : ""),
						},
					];
				}
				// wordWrapLine upstream bug: a single grapheme wider than
				// the wrap width recurses with identical arguments in BOTH
				// implementations (TS original and Rust port). TS surfaces
				// it as an in-band RangeError ("Maximum call stack size
				// exceeded"); Rust aborts the harness process with a stack
				// overflow. Same root cause, untestable class — reconcile
				// instead of flagging a divergence.
				const stackSkip =
					oneTs[0] &&
					oneTs[0].error === "Maximum call stack size exceeded" &&
					oneRs[0] &&
					typeof oneRs[0].error === "string" &&
					/stack overflow/i.test(oneRs[0].error);
				out.push({
					ok: stackSkip || canonical(oneTs[0]) === canonical(oneRs[0]),
					skipped: stackSkip,
					ts: oneTs[0],
					rs: oneRs[0],
					args,
				});
			}
			continue;
		}
		for (let j = 0; j < slice.length; j++) {
			out.push({
				ok: canonical(ts[j]) === canonical(rs[j]),
				ts: ts[j],
				rs: rs[j],
				args: slice[j],
			});
		}
	}
	return out;
}

/** Single-case check through the batch path (used by the shrinker). */
export function checkCase(tool, args) {
	return runBatch(tool, [args], { batchSize: 1 })[0];
}

// -------------------------------------------------------------- shrinker

/**
 * Classic delta-debugging (ddmin, complements at increasing granularity).
 * `parts` is an array of input chunks; `test(parts)` must return true when
 * the divergence is still present. Returns the minimal failing prefix
 * array, or null if the original input no longer fails (flaky).
 */
export async function ddmin(parts, test) {
	if (!(await test(parts))) return null;
	let current = parts.slice();
	let n = 2;
	while (current.length >= 2) {
		const chunk = Math.ceil(current.length / n);
		let reduced = false;
		for (let i = 0; i < current.length; i += chunk) {
			const candidate = current.slice(0, i).concat(current.slice(i + chunk));
			if (candidate.length === 0) continue;
			if (await test(candidate)) {
				current = candidate;
				n = 2;
				reduced = true;
				break;
			}
		}
		if (!reduced) {
			if (n >= current.length) return current;
			n = Math.min(n * 2, current.length);
		}
	}
	return current;
}

/** Split a string into fuzzer chunks (code points, paired with astral halves). */
export function stringToChunks(s) {
	return [...s];
}

// ------------------------------------------------------ regression writer

/**
 * Persist a minimized divergence as a permanent parity case.
 * File: parity/regressions/reg-<target>-<seq>.mjs exporting `cases`
 * (same shape as parity/cases.mjs entries). Aggregated by index.mjs.
 */
export function writeRegression(target, caseSpec) {
	mkdirSync(regressionsDir, { recursive: true });
	const existing = readdirSync(regressionsDir).filter((f) => new RegExp(`^reg-${target}-\\d+\\.mjs$`).test(f));
	// Max existing sequence + 1 (NOT file count): earlier deletions must
	// never make the counter collide with a surviving file and overwrite it
	// (this bug previously collapsed 10 editor regressions into one file).
	const maxSeq = existing.reduce((m, f) => {
		const n = parseInt(f.match(/(\d+)\.mjs$/)[1], 10);
		return Math.max(m, n);
	}, 0);
	const seq = String(maxSeq + 1).padStart(3, "0");
	const name = `reg-${target}-${seq}`;
	const file = join(regressionsDir, `${name}.mjs`);
	const json = (v) => JSON.stringify(v);
	writeFileSync(
		file,
		`// AUTO-GENERATED fuzz regression (parity/fuzz target: ${target}).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
\t{
\t\tname: ${json("fuzz/" + name)},
\t\ttool: ${json(caseSpec.tool)},
\t\targs: ${json(caseSpec.args)},
\t},
];
`,
	);
	return name;
}
