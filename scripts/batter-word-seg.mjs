// Large-scale differential batter: Rust pi-cli (offline ICU port) vs live
// Intl.Segmenter, driven through the tui-word-navigation parity harness.
//
// ICU's language-break-engine factory is PROCESS-GLOBAL and warms lazily:
// results depend on what was segmented before (e.g. a U+FF70-led dictionary
// run merges while the factory is cold but splits once Cjk is cached). The
// batter therefore compares MATCHED trajectories on both sides:
//
//   phase 1 (warm-seg):   segmentWords over the whole fuzz corpus — one
//                         pi-cli process vs one shared warm Intl.Segmenter
//                         (both warm over the same input sequence).
//   phase 2 (cold-seg):   segmentWords per text — fresh pi-cli process vs a
//                         fresh node process whose very first segmentation
//                         is the text under test (both cold).
//   phase 3 (cold-nav):   findWordBackward/Forward cursor sweeps per text —
//                         fresh processes on both sides (both cold).
//   phase 4 (warm-nav):   nav cursor sweeps over all texts in ONE pi-cli
//                         process (batchNav), warmed by the same corpus
//                         sequence as the shared Intl.Segmenter.
//
// Usage: node scripts/batter-word-seg.mjs [SEG_N=20000] [NAV_N=1500]
//                                       [SEED=777] [COLD_SEG_N=1500]
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const PI_CLI = join(HERE, "..", "target", "release", "pi-cli");
const SEG_N = parseInt(process.argv[2] || "20000", 10);
const NAV_N = parseInt(process.argv[3] || "1500", 10);
const SEED = parseInt(process.argv[4] || "777", 10);
const COLD_SEG_N = parseInt(process.argv[5] || "1500", 10);

function mulberry32(a) {
	return function () {
		a |= 0;
		a = (a + 0x6d2b79f5) | 0;
		let t = Math.imul(a ^ (a >>> 15), 1 | a);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

const pools = [
	{ ranges: [[0x20, 0x7e]], w: 6 },
	{ ranges: [[0x21, 0x2f], [0x3a, 0x40], [0x5b, 0x60], [0x7b, 0x7e]], w: 3 },
	{ ranges: [[0x09, 0x0d], [0x20], [0xa0], [0x2000, 0x200a], [0x2028, 0x2029], [0x3000]], w: 2 },
	{ ranges: [[0x30, 0x39]], w: 2 },
	{ ranges: [[0xc0, 0x24f]], w: 2 },
	{ ranges: [[0x370, 0x3ff], [0x400, 0x4ff]], w: 2 },
	{ ranges: [[0x590, 0x5ff], [0x600, 0x6ff]], w: 2 },
	{ ranges: [[0x900, 0x97f]], w: 2 },
	{ ranges: [[0xe00, 0xe5b]], w: 8 },
	{ ranges: [[0xe80, 0xedd]], w: 6 },
	{ ranges: [[0x1780, 0x17f0]], w: 6 },
	{ ranges: [[0x1000, 0x103f], [0x1040, 0x109f]], w: 6 },
	{ ranges: [[0x1100, 0x11ff]], w: 2 },
	{ ranges: [[0xac00, 0xd7a3]], w: 5 },
	{ ranges: [[0x3040, 0x30ff]], w: 5 },
	{ ranges: [[0x31f0, 0x31ff], [0xff66, 0xff9f]], w: 3 },
	{ ranges: [[0x4e00, 0x9fff]], w: 8 },
	{ ranges: [[0x3400, 0x4dbf]], w: 2 },
	{ ranges: [[0xf900, 0xfaff], [0x2e80, 0x2eff], [0x2f00, 0x2fdf]], w: 2 },
	{ ranges: [[0x3000, 0x303f], [0xff01, 0xff60]], w: 2 },
	{ ranges: [[0x2000, 0x206f]], w: 2 },
	{ ranges: [[0x0300, 0x036f]], w: 2 },
	{ ranges: [[0x1f300, 0x1f5ff], [0x1f600, 0x1f64f], [0x1f680, 0x1f6ff]], w: 3 },
	{ ranges: [[0x2600, 0x27bf]], w: 2 },
	{ ranges: [[0x10000, 0x1007f], [0x10300, 0x1032f], [0x13000, 0x1342f]], w: 1 },
	{ ranges: [[0x20000, 0x200ff], [0x2a700, 0x2a7ff]], w: 2 },
	{ ranges: [[0x11700, 0x1174f]], w: 1 },
	{ ranges: [[0x1a00, 0x1a1f]], w: 1 },
];
const totalW = pools.reduce((s, p) => s + p.w, 0);
function pickPool(rand) {
	let x = rand() * totalW;
	for (const p of pools) {
		x -= p.w;
		if (x < 0) return p;
	}
	return pools[0];
}
function pickChar(rand) {
	const p = pickPool(rand);
	const r = p.ranges[(rand() * p.ranges.length) | 0];
	return r[0] + ((rand() * (r[1] - r[0] + 1)) | 0);
}
function fuzzString(rand) {
	const len = 1 + ((rand() * 24) | 0);
	let s = "";
	for (let i = 0; i < len; i++) s += String.fromCodePoint(pickChar(rand));
	return s;
}
// JSON transport cannot carry lone surrogates — skip those (covered by the
// in-crate binary fixture instead).
const hasLoneSurrogate = (s) =>
	/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(s);

const rand = mulberry32(SEED);
const texts = [];
while (texts.length < SEG_N) {
	const s = fuzzString(rand);
	if (!hasLoneSurrogate(s)) texts.push(s);
}

const pi = (payload) =>
	JSON.parse(
		execFileSync(PI_CLI, ["harness", "tool", "tui-word-navigation"], {
			input: JSON.stringify(payload),
			maxBuffer: 1 << 28,
		}),
	);

// fresh-process (cold) live reference for one op
const tmp = mkdtempSync(join(tmpdir(), "batter-"));
const coldIn = join(tmp, "in.json");
const coldRef = (payload) => {
	writeFileSync(coldIn, JSON.stringify(payload));
	return JSON.parse(
		execFileSync(process.execPath, [join(HERE, "navref-cold.mjs"), coldIn], {
			maxBuffer: 1 << 24,
		}),
	);
};

const refSeg = new Intl.Segmenter(undefined, { granularity: "word" });
let exitCode = 0;

// ---- phase 1: warm-matched segmentWords ---------------------------------
const expected = texts.map((t) =>
	[...refSeg.segment(t)].map(({ segment, isWordLike }) => [segment, isWordLike]),
);
const out = pi({ fn: "segmentWords", texts });
let segFails = 0;
outer: for (let i = 0; i < texts.length; i++) {
	const got = out.result[i];
	if (got.length !== expected[i].length) {
		segFails++;
		if (segFails <= 5) console.log(`WARM-SEG FAIL #${i} len: ${JSON.stringify(texts[i])}`);
		continue;
	}
	for (let j = 0; j < got.length; j++) {
		if (got[j].segment !== expected[i][j][0] || got[j].isWordLike !== expected[i][j][1]) {
			segFails++;
			if (segFails <= 5)
				console.log(
					`WARM-SEG FAIL #${i} seg ${j}: ${JSON.stringify(texts[i])}\n  rs: ${JSON.stringify(got[j])}\n  ts: ${JSON.stringify(expected[i][j])}`,
				);
			continue outer;
		}
	}
}
console.log(`phase 1 warm segmentWords: ${texts.length - segFails}/${texts.length} pass`);
exitCode += segFails > 0 ? 1 : 0;

// ---- phase 2: cold-matched segmentWords ---------------------------------
const navTexts = texts.slice(0, NAV_N);
const coldSegTexts = navTexts.slice(0, COLD_SEG_N);
let coldSegFails = 0;
for (let i = 0; i < coldSegTexts.length; i++) {
	const t = coldSegTexts[i];
	const rs = pi({ fn: "segmentWords", texts: [t] }).result[0];
	const exp = coldRef({ fn: "segmentWords", texts: [t] })[0];
	let ok = rs.length === exp.length;
	if (ok)
		for (let j = 0; j < rs.length; j++)
			if (rs[j].segment !== exp[j][0] || rs[j].isWordLike !== exp[j][1]) { ok = false; break; }
	if (!ok) {
		coldSegFails++;
		if (coldSegFails <= 5)
			console.log(
				`COLD-SEG FAIL #${i}: ${JSON.stringify(t)}\n  rs: ${JSON.stringify(rs)}\n  ts: ${JSON.stringify(exp)}`,
			);
	}
}
console.log(`phase 2 cold segmentWords: ${coldSegTexts.length - coldSegFails}/${coldSegTexts.length} pass`);
exitCode += coldSegFails > 0 ? 1 : 0;

// ---- phase 3: cold-matched findWordBackward/Forward sweeps --------------
const { findWordBackwardRef, findWordForwardRef } = await import(join(HERE, "lib", "nav-ref.mjs"));
let coldNavFails = 0;
for (const t of navTexts) {
	const cursors = Array.from({ length: t.length + 1 }, (_, i) => i);
	for (const fn of ["findWordBackward", "findWordForward"]) {
		const payload = { fn, text: t, cursors };
		const rs = pi(payload).result;
		const exp = coldRef(payload);
		for (let i = 0; i < cursors.length; i++) {
			if (rs[i] !== exp[i]) {
				coldNavFails++;
				if (coldNavFails <= 5)
					console.log(
						`COLD-NAV FAIL ${fn} ${JSON.stringify(t)} cursor=${cursors[i]}: rs=${rs[i]} ts=${exp[i]}`,
					);
				break;
			}
		}
	}
}
console.log(`phase 3 cold navigation: ${coldNavFails} failures over ${navTexts.length} texts x full cursor sweeps`);
exitCode += coldNavFails > 0 ? 1 : 0;

// ---- phase 4: warm-matched findWordBackward/Forward sweeps --------------
// One pi-cli process (batchNav), warmed by the SAME corpus sequence that
// warmed the shared Intl.Segmenter (phase 1's expected computation).
const ops = [{ fn: "segmentWords", texts }];
for (const t of navTexts) {
	const cursors = Array.from({ length: t.length + 1 }, (_, i) => i);
	ops.push({ fn: "findWordBackward", text: t, cursors });
	ops.push({ fn: "findWordForward", text: t, cursors });
}
const batch = pi({ fn: "batchNav", ops });
let warmNavFails = 0;
let opIdx = 1; // skip the segmentWords warm-up op
for (const t of navTexts) {
	const cursors = Array.from({ length: t.length + 1 }, (_, i) => i);
	for (const [fn, exp] of [
		["findWordBackward", cursors.map((c) => findWordBackwardRef(refSeg, t, c))],
		["findWordForward", cursors.map((c) => findWordForwardRef(refSeg, t, c))],
	]) {
		const rs = batch.result[opIdx++];
		for (let i = 0; i < cursors.length; i++) {
			if (rs[i] !== exp[i]) {
				warmNavFails++;
				if (warmNavFails <= 5)
					console.log(
						`WARM-NAV FAIL ${fn} ${JSON.stringify(t)} cursor=${cursors[i]}: rs=${rs[i]} ts=${exp[i]}`,
					);
				break;
			}
		}
	}
}
console.log(`phase 4 warm navigation: ${warmNavFails} failures over ${navTexts.length} texts x full cursor sweeps`);
exitCode += warmNavFails > 0 ? 1 : 0;

process.exit(exitCode);
