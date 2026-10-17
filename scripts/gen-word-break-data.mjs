#!/usr/bin/env node
// Generates crates/pi-tui/src/unicode/word_break_data.rs (+ binary trie
// payloads in crates/pi-tui/src/unicode/data/) — the ICU 78.3 word-break
// machinery data carved from Node 24.22.0's icudt78l.dat and UCD files,
// exactly as executed by scripts/lib/word-segmenter.mjs (the validated
// bit-exact prototype of Node's Intl.Segmenter word granularity).
//
// Emitted tables:
//   - RBBI forward state machine rows (word.brk) + rule status table
//   - UCPTrie category lookup, range-compressed over all code points
//   - Scripts.txt range table (u8 script ids)
//   - DictionaryBreakEngine character sets (Thai/Lao/Khmer/Burmese/CJ/Hangul)
//   - NFKC decomposition / composition / ccc / hasBoundaryBefore data
//   - Dictionary tries (cjdict u16 units + 4 SEA byte tries) as .bin files
//
// Usage: node scripts/gen-word-break-data.mjs
// Deterministic; no network access (all inputs vendored in scripts/data/icu78).

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
	parseWordBrk,
	makeTrieLookup,
	rangeCompress,
	parseDict,
	parseScripts,
	parseSaCodePoints,
	parseUnicodeData,
	dataPath,
} from "./lib/icu-data.mjs";
import { CpSet } from "./lib/word-segmenter.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT_RS = join(HERE, "..", "crates", "pi-tui", "src", "unicode", "word_break_data.rs");
const OUT_DATA = join(HERE, "..", "crates", "pi-tui", "src", "unicode", "data");
mkdirSync(OUT_DATA, { recursive: true });

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------
const hex = (n) => "0x" + n.toString(16);

function emitU8Array(name, arr, perLine) {
	const lines = [];
	for (let i = 0; i < arr.length; i += perLine) {
		lines.push("    " + Array.from(arr.slice(i, i + perLine)).join(", ") + ",");
	}
	return `pub static ${name}: [u8; ${arr.length}] = &[\n${lines.join("\n")}\n];\n`.replace(
		"= &[", "= [",
	);
}
function emitI32Array(name, arr, perLine) {
	const lines = [];
	for (let i = 0; i < arr.length; i += perLine) {
		lines.push("    " + arr.slice(i, i + perLine).join(", ") + ",");
	}
	return `pub static ${name}: [i32; ${arr.length}] = [\n${lines.join("\n")}\n];\n`;
}
function emitU32Array(name, arr, perLine) {
	const lines = [];
	for (let i = 0; i < arr.length; i += perLine) {
		lines.push("    " + arr.slice(i, i + perLine).join(", ") + ",");
	}
	return `pub static ${name}: [u32; ${arr.length}] = [\n${lines.join("\n")}\n];\n`;
}
function emitRanges2(name, ranges, perLine = 6) {
	const parts = [];
	for (const [a, b] of ranges) parts.push(`(${hex(a)}, ${hex(b)})`);
	const lines = [];
	for (let i = 0; i < parts.length; i += perLine) {
		lines.push("    " + parts.slice(i, i + perLine).join(", ") + ",");
	}
	return `pub static ${name}: &[(u32, u32)] = &[\n${lines.join("\n")}\n];\n`;
}
function emitRanges3(name, ranges, perLine = 6) {
	const parts = [];
	for (const [a, b, v] of ranges) parts.push(`(${hex(a)}, ${hex(b)}, ${v})`);
	const lines = [];
	for (let i = 0; i < parts.length; i += perLine) {
		lines.push("    " + parts.slice(i, i + perLine).join(", ") + ",");
	}
	return `pub static ${name}: &[(u32, u32, u8)] = &[\n${lines.join("\n")}\n];\n`;
}
function emitTriples(name, triples, perLine = 4) {
	const parts = [];
	for (const [x, y, v] of triples) parts.push(`(${hex(x)}, ${hex(y)}, ${hex(v)})`);
	const lines = [];
	for (let i = 0; i < parts.length; i += perLine) {
		lines.push("    " + parts.slice(i, i + perLine).join(", ") + ",");
	}
	return `pub static ${name}: &[(u32, u32, u32)] = &[\n${lines.join("\n")}\n];\n`;
}

// ---------------------------------------------------------------------------
// word.brk: RBBI state table + status table
// ---------------------------------------------------------------------------
const brk = parseWordBrk();
const fwd = brk.forward;
if (fwd.rowLen !== 34 || fwd.numStates !== 58 || fwd.dictCategoriesStart !== 24) {
	throw new Error(`unexpected word.brk shape: ${fwd.numStates}x${fwd.rowLen} dictStart=${fwd.dictCategoriesStart}`);
}
const rowsFlat = fwd.rows.subarray(0, fwd.numStates * fwd.rowLen);

// ---------------------------------------------------------------------------
// UCPTrie categories, range-compressed
// ---------------------------------------------------------------------------
const cat = makeTrieLookup(brk.trie);
const catRanges = rangeCompress((c) => cat.get(c));

// ---------------------------------------------------------------------------
// Scripts table
// ---------------------------------------------------------------------------
const scripts = parseScripts();
const scriptNames = [...new Set(scripts.map((r) => r.script))].sort();
const scriptId = new Map([["Unknown", 0]]);
for (const n of scriptNames) scriptId.set(n, scriptId.size);
const scriptRanges = scripts.map((r) => [r.start, r.end, scriptId.get(r.script)]);

// ---------------------------------------------------------------------------
// DictionaryBreakEngine character sets (replicating dictbe.cpp ctors exactly
// as done in word-segmenter.mjs getState())
// ---------------------------------------------------------------------------
const scriptSet = (name) =>
	new CpSet(scripts.filter((r) => r.script === name).map((r) => [r.start, r.end]));
const saSet = CpSet.fromCps(parseSaCodePoints());
const ud = parseUnicodeData();
const markSetFor = (ss) => {
	const m = [];
	for (const [lo, hi] of ss.ranges) {
		for (let c = lo; c <= hi; c++) {
			const r = ud.get(c);
			if (r && r.gc[0] === "M") m.push(c);
		}
	}
	return CpSet.fromCps(m);
};

const thaiWordSet = scriptSet("Thai").intersect(saSet);
const thaiEnd = thaiWordSet.clone().remove(CpSet.range(0x0e31, 0x0e31)).remove(CpSet.range(0x0e40, 0x0e44));
const thaiBegin = CpSet.range(0x0e01, 0x0e2e).clone().addRange(0x0e40, 0x0e44);
const thaiMark = markSetFor(scriptSet("Thai").intersect(saSet)).clone().addCp(0x20);
const thaiSuffix = CpSet.fromCps([0x0e2f, 0x0e46]);

const laoWordSet = scriptSet("Lao").intersect(saSet);
const laoEnd = laoWordSet.clone().remove(CpSet.range(0x0ec0, 0x0ec4));
const laoBegin = CpSet.range(0x0e81, 0x0eae).clone().addRange(0x0edc, 0x0edd).addRange(0x0ec0, 0x0ec4);
const laoMark = markSetFor(scriptSet("Lao").intersect(saSet)).clone().addCp(0x20);

const khmerWordSet = scriptSet("Khmer").intersect(saSet);
const khmerEnd = khmerWordSet.clone().remove(CpSet.range(0x17d2, 0x17d2));
const khmerBegin = CpSet.range(0x1780, 0x17b3);
const khmerMark = markSetFor(scriptSet("Khmer").intersect(saSet)).clone().addCp(0x20);

const burmeseWordSet = scriptSet("Myanmar").intersect(saSet);
const burmeseEnd = burmeseWordSet.clone();
const burmeseBegin = CpSet.range(0x1000, 0x102a);
const burmeseMark = markSetFor(scriptSet("Myanmar").intersect(saSet)).clone().addCp(0x20);

const cjSet = scriptSet("Han")
	.clone()
	.addRange(0x30fc, 0x30fc)
	.addRange(0xff70, 0xff70)
	.addRange(0xff9e, 0xff9e)
	.addRange(0xff9f, 0xff9f);
for (const name of ["Hiragana", "Katakana"]) {
	for (const [lo, hi] of scriptSet(name).ranges) cjSet.addRange(lo, hi);
}
const hangulSet = CpSet.range(0xac00, 0xd7a3);

// ---------------------------------------------------------------------------
// Dictionary tries → .bin files
// ---------------------------------------------------------------------------
const cjDict = parseDict("cjdict.dict");
const thaiDict = parseDict("thaidict.dict");
const laoDict = parseDict("laodict.dict");
const khmerDict = parseDict("khmerdict.dict");
const burmeseDict = parseDict("burmesedict.dict");
if (!cjDict.isUChars) throw new Error("cjdict trie expected UChars");
for (const d of [thaiDict, laoDict, khmerDict, burmeseDict]) {
	if (d.isUChars) throw new Error("SEA dict trie expected Bytes");
}
writeFileSync(join(OUT_DATA, "cjdict-trie.bin"), cjDict.trie);
writeFileSync(join(OUT_DATA, "thaidict-trie.bin"), thaiDict.trie);
writeFileSync(join(OUT_DATA, "laodict-trie.bin"), laoDict.trie);
writeFileSync(join(OUT_DATA, "khmerdict-trie.bin"), khmerDict.trie);
writeFileSync(join(OUT_DATA, "burmesedict-trie.bin"), burmeseDict.trie);
const dictOffset = (d) => d.transform & 0x1fffff;

// ---------------------------------------------------------------------------
// NFKC data (replicating buildNfkc() in word-segmenter.mjs)
// ---------------------------------------------------------------------------
const cccOf = (c) => {
	const r = ud.get(c);
	return r ? r.ccc : 0;
};
const exclusions = new Set();
const exclText = readFileSync(dataPath("CompositionExclusions.txt"), "utf8");
for (const line of exclText.split("\n")) {
	const body = line.split("#")[0].trim();
	if (!body) continue;
	const [rng] = body.split(";");
	if (rng.includes("..")) {
		const [a, b] = rng.split("..");
		for (let c = parseInt(a, 16); c <= parseInt(b, 16); c++) exclusions.add(c);
	} else exclusions.add(parseInt(rng, 16));
}
const pair = new Map();
const qcMaybe = new Set();
for (const [cp, rec] of ud) {
	if (!rec.decomp || rec.compat || rec.decomp.length !== 2) continue;
	const [x, y] = rec.decomp;
	if (cccOf(x) !== 0) continue;
	if (exclusions.has(cp)) continue;
	pair.set(x * 0x110000 + y, cp);
	qcMaybe.add(y);
}
for (let c = 0x1161; c <= 0x1175; c++) qcMaybe.add(c);
for (let c = 0x11a8; c <= 0x11c2; c++) qcMaybe.add(c);

function decomposeCp(c, out) {
	if (c >= 0xac00 && c <= 0xd7a3) {
		const si = c - 0xac00;
		out.push(0x1100 + Math.floor(si / 588), 0x1161 + Math.floor((si % 588) / 28));
		const t = 0x11a7 + (si % 28);
		if (t !== 0x11a7) out.push(t);
		return;
	}
	const rec = ud.get(c);
	if (rec && rec.decomp) {
		for (const e of rec.decomp) decomposeCp(e, out);
	} else out.push(c);
}

// Full recursive NFKC decompositions (Hangul stays algorithmic in Rust).
// Only code points that actually decompose to something other than themselves.
const decompKeys = [];
const decompFlat = [];
const decompOffs = [0];
for (const [cp, rec] of [...ud.entries()].sort((a, b) => a[0] - b[0])) {
	if (cp >= 0xac00 && cp <= 0xd7a3) continue; // Hangul: algorithmic
	if (!rec.decomp) continue;
	const out = [];
	decomposeCp(cp, out);
	if (out.length === 1 && out[0] === cp) continue;
	decompKeys.push(cp);
	decompFlat.push(...out);
	decompOffs.push(decompFlat.length);
}

// Composition pairs (canonical, non-excluded, starter-led), sorted by (x, y).
const pairTriples = [...pair.entries()]
	.map(([k, v]) => [Math.floor(k / 0x110000), k % 0x110000, v])
	.sort((a, b) => a[0] - b[0] || a[1] - b[1]);

// ccc range table (nonzero combining classes)
const cccRanges = [];
{
	const cccCps = [...ud.entries()].filter(([, r]) => r.ccc !== 0).map(([c]) => c).sort((a, b) => a - b);
	let start = cccCps[0];
	let prev = cccCps[0];
	const flush = () => cccRanges.push([start, prev, cccOf(prev)]);
	for (let i = 1; i < cccCps.length; i++) {
		if (cccCps[i] === prev + 1 && cccOf(cccCps[i]) === cccOf(prev)) {
			prev = cccCps[i];
		} else {
			flush();
			start = prev = cccCps[i];
		}
	}
	flush();
}

// hasBoundaryBefore → precomputed "no boundary before" set
const noBoundary = [];
{
	let start = -1;
	for (let c = 0; c <= 0x10ffff; c++) {
		let boundary = true;
		if (cccOf(c) !== 0 || qcMaybe.has(c)) boundary = false;
		else {
			const out = [];
			decomposeCp(c, out);
			const first = out[0];
			if (!(out.length === 1 && first === c)) {
				if (cccOf(first) !== 0 || qcMaybe.has(first)) boundary = false;
			}
		}
		if (!boundary) {
			if (start < 0) start = c;
		} else if (start >= 0) {
			noBoundary.push([start, c - 1]);
			start = -1;
		}
	}
	if (start >= 0) noBoundary.push([start, 0x10ffff]);
}

// ---------------------------------------------------------------------------
// Emit
// ---------------------------------------------------------------------------
const parts = [];
parts.push(`//! ICU 78.3 word-break data (Intl.Segmenter word granularity), carved
//! from Node 24.22.0's icudt78l.dat and the vendored UCD files.
//!
//! AUTO-GENERATED by \`scripts/gen-word-break-data.mjs\` — do not edit.
//! Executed by \`word_break.rs\`, which mirrors the validated prototype
//! \`scripts/lib/word-segmenter.mjs\` 1:1.

// --- word.brk RBBI forward state table -------------------------------------
// Rows are u8 (RBBI_8BITS_ROWS): [accepting, lookAhead, tagsIdx, nextState[31]].
pub const RBBI_NUM_STATES: usize = ${fwd.numStates};
pub const RBBI_ROW_LEN: usize = ${fwd.rowLen};
pub const RBBI_DICT_CATEGORIES_START: u8 = ${fwd.dictCategoriesStart};
pub const RBBI_CAT_COUNT: usize = ${brk.fCatCount ?? 31};

`);
parts.push(emitU8Array("RBBI_ROWS", rowsFlat, 34));
parts.push("\n");
parts.push(emitI32Array("RBBI_STATUS", brk.statusTable, 14));

parts.push(`\n// --- UCPTrie category lookup (range-compressed over all code points) ------
`);
parts.push(emitRanges3("CATEGORY_RANGES", catRanges));

parts.push(`\n// --- Scripts.txt ------------------------------------------------------------
`);
parts.push(emitRanges3("SCRIPT_RANGES", scriptRanges));
parts.push(`
pub const SCRIPT_UNKNOWN: u8 = 0;
pub const SCRIPT_${"THAI"}: u8 = ${scriptId.get("Thai")};
pub const SCRIPT_LAO: u8 = ${scriptId.get("Lao")};
pub const SCRIPT_KHMER: u8 = ${scriptId.get("Khmer")};
pub const SCRIPT_MYANMAR: u8 = ${scriptId.get("Myanmar")};
pub const SCRIPT_HAN: u8 = ${scriptId.get("Han")};
pub const SCRIPT_HIRAGANA: u8 = ${scriptId.get("Hiragana")};
pub const SCRIPT_KATAKANA: u8 = ${scriptId.get("Katakana")};
`);

parts.push(`\n// --- Dictionary trie payloads (include_bytes!) ------------------------------
// cjdict: UCharsTrie units, little-endian u16. SEA dicts: BytesTrie bytes with
// offset transforms.
pub static CJ_DICT_TRIE_BYTES: &[u8] = include_bytes!("data/cjdict-trie.bin");
pub static THAI_DICT_TRIE: &[u8] = include_bytes!("data/thaidict-trie.bin");
pub static LAO_DICT_TRIE: &[u8] = include_bytes!("data/laodict-trie.bin");
pub static KHMER_DICT_TRIE: &[u8] = include_bytes!("data/khmerdict-trie.bin");
pub static BURMESE_DICT_TRIE: &[u8] = include_bytes!("data/burmesedict-trie.bin");
pub const THAI_DICT_OFFSET: u32 = ${hex(dictOffset(thaiDict))};
pub const LAO_DICT_OFFSET: u32 = ${hex(dictOffset(laoDict))};
pub const KHMER_DICT_OFFSET: u32 = ${hex(dictOffset(khmerDict))};
pub const BURMESE_DICT_OFFSET: u32 = ${hex(dictOffset(burmeseDict))};
`);

parts.push(`\n// --- DictionaryBreakEngine character sets (dictbe.cpp ctors) ----------------
`);
parts.push(emitRanges2("THAI_WORD_SET", thaiWordSet.ranges));
parts.push(emitRanges2("THAI_END_WORD_SET", thaiEnd.ranges));
parts.push(emitRanges2("THAI_BEGIN_WORD_SET", thaiBegin.ranges));
parts.push(emitRanges2("THAI_MARK_SET", thaiMark.ranges));
parts.push(emitRanges2("THAI_SUFFIX_SET", thaiSuffix.ranges));
parts.push(emitRanges2("LAO_WORD_SET", laoWordSet.ranges));
parts.push(emitRanges2("LAO_END_WORD_SET", laoEnd.ranges));
parts.push(emitRanges2("LAO_BEGIN_WORD_SET", laoBegin.ranges));
parts.push(emitRanges2("LAO_MARK_SET", laoMark.ranges));
parts.push(emitRanges2("KHMER_WORD_SET", khmerWordSet.ranges));
parts.push(emitRanges2("KHMER_END_WORD_SET", khmerEnd.ranges));
parts.push(emitRanges2("KHMER_BEGIN_WORD_SET", khmerBegin.ranges));
parts.push(emitRanges2("KHMER_MARK_SET", khmerMark.ranges));
parts.push(emitRanges2("BURMESE_WORD_SET", burmeseWordSet.ranges));
parts.push(emitRanges2("BURMESE_END_WORD_SET", burmeseEnd.ranges));
parts.push(emitRanges2("BURMESE_BEGIN_WORD_SET", burmeseBegin.ranges));
parts.push(emitRanges2("BURMESE_MARK_SET", burmeseMark.ranges));
parts.push(emitRanges2("CJ_SET", cjSet.ranges));
parts.push(emitRanges2("HANGUL_SET", hangulSet.ranges));

parts.push(`\n// --- NFKC (from UnicodeData.txt + CompositionExclusions.txt) -----------------
// Full recursive compat decompositions (Hangul is algorithmic in code).
`);
parts.push(emitU32Array("NFKC_DECOMP_KEYS", decompKeys, 12));
parts.push(emitU32Array("NFKC_DECOMP_OFFS", decompOffs, 12));
parts.push(emitU32Array("NFKC_DECOMP_FLAT", decompFlat, 12));
parts.push("\n");
parts.push(emitTriples("NFKC_PAIRS", pairTriples));
parts.push("\n");
parts.push(emitRanges3("CCC_RANGES", cccRanges));
parts.push("\n");
parts.push(emitRanges2("NFKC_NO_BOUNDARY_BEFORE", noBoundary, 8));

writeFileSync(OUT_RS, parts.join(""));

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------
const sizeOf = (p) => readFileSync(p).length;
console.log(`wrote ${OUT_RS}`);
console.log(`  RBBI_ROWS ${rowsFlat.length} bytes, STATUS ${brk.statusTable.length} i32`);
console.log(`  CATEGORY_RANGES ${catRanges.length} ranges`);
console.log(`  SCRIPT_RANGES ${scriptRanges.length} ranges (${scriptNames.length} scripts)`);
console.log(`  engine sets: thai ${thaiWordSet.ranges.length} lao ${laoWordSet.ranges.length} khmer ${khmerWordSet.ranges.length} burmese ${burmeseWordSet.ranges.length} cj ${cjSet.ranges.length}`);
console.log(`  NFKC: decomp ${decompKeys.length} keys/${decompFlat.length} cps, pairs ${pairTriples.length}, ccc ${cccRanges.length} ranges, noBoundary ${noBoundary.length} ranges`);
console.log(`  data/cjdict-trie.bin ${sizeOf(join(OUT_DATA, "cjdict-trie.bin"))} bytes`);
for (const n of ["thaidict", "laodict", "khmerdict", "burmesedict"]) {
	console.log(`  data/${n}-trie.bin ${sizeOf(join(OUT_DATA, `${n}-trie.bin`))} bytes`);
}
