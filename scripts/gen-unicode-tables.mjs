#!/usr/bin/env node
// Generates crates/pi-tui/src/unicode/tables.rs — the Unicode data tables the
// TUI width engine needs, extracted from the *live Node/V8 runtime* (and the
// get-east-asian-width package actually used by the original) so the Rust port
// is 1:1 by construction with the TypeScript implementation.
//
// Sources, in order of authority:
//   1. V8 regex property data  (scanned codepoint-by-codepoint, u/v flags)
//   2. get-east-asian-width     (the real npm package via its public API)
//   3. UCD 15.1 emoji-test.txt  (candidates only — every candidate is then
//                                verified against V8's RGI_Emoji regex; V8 wins)
//
// Usage: node scripts/gen-unicode-tables.mjs > crates/pi-tui/src/unicode/tables.rs
// Deterministic; no network access required (UCD file is vendored in scripts/data).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { eastAsianWidthType } from "/tmp/pi-repo/node_modules/get-east-asian-width/index.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const MAX_CP = 0x10ffff;

function fromCp(cp) {
	return String.fromCodePoint(cp);
}

// --- range scanning ----------------------------------------------------------

function scanRanges(re) {
	const ranges = [];
	let start = -1;
	for (let cp = 0; cp <= MAX_CP; cp++) {
		if (re.test(fromCp(cp))) {
			if (start < 0) start = cp;
		} else if (start >= 0) {
			ranges.push([start, cp - 1]);
			start = -1;
		}
	}
	if (start >= 0) ranges.push([start, MAX_CP]);
	return ranges;
}

// --- east-asian width (via the actual npm package API) -----------------------

function scanEastAsian() {
	// categories: F, H, W, Na, A, N — emit one range list per category
	const cats = {};
	for (let cp = 0; cp <= MAX_CP; cp++) {
		const t = eastAsianWidthType(cp);
		(cats[t] ??= []).push(cp);
	}
	const out = {};
	for (const [t, cps] of Object.entries(cats)) {
		const ranges = [];
		let start = cps[0];
		let prev = cps[0];
		for (let i = 1; i < cps.length; i++) {
			if (cps[i] === prev + 1) {
				prev = cps[i];
			} else {
				ranges.push([start, prev]);
				start = prev = cps[i];
			}
		}
		ranges.push([start, prev]);
		out[t] = ranges;
	}
	return out;
}

// --- RGI_Emoji string set ----------------------------------------------------

function rgiCandidates() {
	// Vendored UCD 15.1 emoji-test.txt: every fully-qualified + minimally-qualified
	// sequence is a candidate. V8's regex is the authority — verified below.
	const text = readFileSync(join(HERE, "data", "emoji-test-15.1.txt"), "utf8");
	const seqs = new Set();
	for (const line of text.split("\n")) {
		if (line.startsWith("#") || line.trim() === "") continue;
		const [cps] = line.split(";");
		const status = line.split(";")[1].trim().split(" ")[0];
		if (status === "component") continue;
		const hex = cps.trim().split(/\s+/);
		const s = hex.map((h) => fromCp(parseInt(h, 16))).join("");
		seqs.add(s);
	}
	return [...seqs];
}

function scanRgi() {
	const rgiRe = /^\p{RGI_Emoji}$/v;
	const verified = [];
	let rejected = 0;
	for (const s of rgiCandidates()) {
		if (rgiRe.test(s)) verified.push(s);
		else rejected++;
	}
	// Completeness guard: every single codepoint the regex accepts must be in
	// the candidate set (catches a V8/UCD version skew on basic emoji).
	let missingSingles = 0;
	for (let cp = 0; cp <= MAX_CP; cp++) {
		const s = fromCp(cp);
		if (rgiRe.test(s) && !verified.includes(s)) {
			verified.push(s);
			missingSingles++;
		}
	}
	verified.sort();
	return { verified, rejected, missingSingles };
}

// --- UCD GraphemeBreakProperty: Prepend (not regex-exposable) ---------------

function ucdGcbClasses() {
	// All Grapheme_Cluster_Break classes — these are the exact rule inputs
	// ICU's segmenter uses. Unicode **16.0** to match the ICU vintage in
	// Node 24 (validated against the live Intl.Segmenter corpus).
	const wanted = ["Prepend", "Extend", "SpacingMark", "Control", "Regional_Indicator", "ZWJ"];
	const out = {};
	for (const line of readFileSync(join(HERE, "data", "GraphemeBreakProperty-16.0.txt"), "utf8").split("\n")) {
		const m = line.match(/^([0-9A-F]+)(?:\.\.([0-9A-F]+))?\s*;\s*(\w+)\s*#/);
		if (!m) continue;
		const cls = m[3];
		if (!wanted.includes(cls)) continue;
		(out[cls] ??= []).push([parseInt(m[1], 16), parseInt(m[2] ?? m[1], 16)]);
	}
	return out;
}

// --- UCD DerivedCoreProperties: InCB (Indic_Conjunct_Break, GB9c) -----------

function ucdIncb() {
	const out = { Consonant: [], Linker: [], Extend: [] };
	const text = readFileSync(join(HERE, "data", "DerivedCoreProperties-16.0.txt"), "utf8");
	for (const line of text.split("\n")) {
		const m = line.match(/^([0-9A-F]+)(?:\.\.([0-9A-F]+))?\s*;\s*InCB\s*;\s*(\w+)\s*#/);
		if (!m) continue;
		const range = [parseInt(m[1], 16), parseInt(m[2] ?? m[1], 16)];
		out[m[3]]?.push(range);
	}
	return out;
}

// --- emit --------------------------------------------------------------------

function rustRanges(name, ranges) {
	const body = ranges.map(([a, b]) => `(0x${a.toString(16)}, 0x${b.toString(16)})`).join(", ");
	return `pub static ${name}: &[(u32, u32)] = &[${body}];`;
}

function rustStrings(name, strings) {
	const body = strings.map((s) => `"${rustEscape(s)}"`).join(", ");
	return `pub static ${name}: &[&str] = &[${body}];`;
}

function rustEscape(s) {
	let out = "";
	for (const ch of s) {
		const cp = ch.codePointAt(0);
		if (cp === 0x22) out += '\\"';
		else if (cp === 0x5c) out += "\\\\";
		else if (cp >= 0x20 && cp <= 0x7e) out += ch;
		else if (cp <= 0xff) out += `\\u{${cp.toString(16)}}`;
		else out += `\\u{${cp.toString(16)}}`;
	}
	return out;
}

// --- main --------------------------------------------------------------------

const PROPS = [
	["DEFAULT_IGNORABLE", "Default_Ignorable_Code_Point", "u"],
	["CONTROL", "Control", "u"],
	["MARK", "Mark", "u"],
	["FORMAT", "Format", "u"],
	["SURROGATE", "Surrogate", "u"],
	["GRAPHEME_EXTEND", "Grapheme_Extend", "u"],
	["SPACING_MARK", "Spacing_Mark", "u"],
	["EXTENDED_PICTOGRAPHIC", "Extended_Pictographic", "u"],
	["EMOJI_MODIFIER", "Emoji_Modifier", "u"],
	["REGIONAL_INDICATOR", "Regional_Indicator", "u"],
	["JOIN_CONTROL", "Join_Control", "u"],
	["EMOJI", "Emoji", "u"],
];

const parts = [];
parts.push(`// @generated by scripts/gen-unicode-tables.mjs — DO NOT EDIT by hand.
// Source of truth: the live Node/V8 runtime this file was generated under
// (regex property data, u/v flags) and the get-east-asian-width package.
// Regenerate with: node scripts/gen-unicode-tables.mjs > this file
#![allow(clippy::all)]
`);

const ea = scanEastAsian();
for (const [t, ranges] of Object.entries(ea)) {
	const name = "EAW_" + t.toUpperCase().replace(/[^A-Z]/g, "_");
	parts.push(rustRanges(name, ranges));
}

for (const [rustName, propName, flag] of PROPS) {
	const re = new RegExp(`^\\p{${propName}}$`, flag);
	const ranges = scanRanges(re);
	parts.push(rustRanges(`PROP_${rustName}`, ranges));
	process.stderr.write(`${propName}: ${ranges.length} ranges\n`);
}

const rgi = scanRgi();
process.stderr.write(
	`RGI_Emoji: ${rgi.verified.length} verified, ${rgi.rejected} candidates rejected by V8, ${rgi.missingSingles} singles added\n`,
);
parts.push(rustStrings("RGI_EMOJI_SEQUENCES", rgi.verified));

const gcb = ucdGcbClasses();
const gcbNames = {
	Prepend: "GCB_PREPEND",
	Extend: "GCB_EXTEND",
	SpacingMark: "GCB_SPACING_MARK",
	Control: "GCB_CONTROL",
	Regional_Indicator: "GCB_REGIONAL_INDICATOR",
	ZWJ: "GCB_ZWJ",
};
for (const [cls, rustName] of Object.entries(gcbNames)) {
	const ranges = gcb[cls] ?? [];
	process.stderr.write(`GCB ${cls}: ${ranges.length} ranges\n`);
	parts.push(rustRanges(rustName, ranges));
}

const incb = ucdIncb();
const incbNames = {
	Consonant: "INCB_CONSONANT",
	Linker: "INCB_LINKER",
	Extend: "INCB_EXTEND",
};
for (const [cls, rustName] of Object.entries(incbNames)) {
	const ranges = incb[cls] ?? [];
	process.stderr.write(`InCB ${cls}: ${ranges.length} ranges\n`);
	parts.push(rustRanges(rustName, ranges));
}

process.stdout.write(parts.join("\n\n") + "\n");
