// Shared parsing of the vendored ICU 78.3 binaries + UCD 17.0.0 files.
//
// Source of truth: scripts/data/icu78/ —
//   word.brk + *.dict  carved from Node 24.22.0's embedded icudt78l.dat
//   (symbol icudt78_dat), Scripts.txt / LineBreak.txt / UnicodeData.txt
//   from unicode.org Public/17.0.0/ucd.
//
// Everything here is deterministic and offline.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const DATA_DIR = join(HERE, "..", "data", "icu78");

export function dataPath(name) {
	return join(DATA_DIR, name);
}

// ---------------------------------------------------------------------------
// word.brk — RBBIDataHeader + forward state table + UCPTrie + status table
// ---------------------------------------------------------------------------

export function parseWordBrk() {
	const d = readFileSync(dataPath("word.brk"));
	const headerSize = d.readUInt16LE(0);
	const base = headerSize;
	const f = [];
	for (let i = 0; i < 10; i++) f.push(d.readUInt32LE(base + 16 + 4 * i));
	const [fFTable, fFTableLen, fRTable, fRTableLen, fTrie, fTrieLen, fRuleSource, fRuleSourceLen, fStatusTable, fStatusTableLen] = f;
	if (d.readUInt32LE(base) !== 0xb1a0) throw new Error("bad word.brk magic");
	const fmt = [d[base + 4], d[base + 5], d[base + 6], d[base + 7]];
	if (fmt[0] !== 6) throw new Error("unsupported word.brk format version " + fmt.join("."));

	const fwd = d.subarray(base + fFTable, base + fFTable + fFTableLen);
	const table = {
		numStates: fwd.readUInt32LE(0),
		rowLen: fwd.readUInt32LE(4),
		dictCategoriesStart: fwd.readUInt32LE(8),
		lookAheadResultsSize: fwd.readUInt32LE(12),
		flags: fwd.readUInt32LE(16),
		rows: fwd.subarray(20), // 8-bit rows: accepting, lookAhead, tagsIdx, nextState[catCount]
	};
	const status = [];
	for (let off = 0; off < fStatusTableLen; off += 4) status.push(d.readInt32LE(base + fStatusTable + off));

	return {
		headerSize,
		catCount: d.readUInt32LE(base + 12),
		forward: table,
		statusTable: status,
		trie: d.subarray(base + fTrie, base + fTrie + fTrieLen),
		ruleSource: d.subarray(base + fRuleSource, base + fRuleSource + fRuleSourceLen).toString("utf8"),
	};
}

// UCPTrie (type FAST, value width 8) lookup over the raw trie bytes.
export function makeTrieLookup(trie) {
	if (trie.readUInt32LE(0) !== 0x54726933) throw new Error("bad trie signature");
	const options = trie.readUInt16LE(4);
	const valueWidth = options & 7;
	if (valueWidth !== 2) throw new Error("expected 8-bit values, got width " + valueWidth);
	const indexLength = trie.readUInt16LE(6);
	const dataLength = trie.readUInt16LE(8);
	const dataNullOffset = trie.readUInt16LE(12);
	const shiftedHighStart = trie.readUInt16LE(14);
	const highStart = shiftedHighStart << 9;
	const BMP_INDEX_LENGTH = 0x400;
	const OMITTED1 = 4;
	const data = 16 + 2 * indexLength; // data starts after header + index

	const idx = (i) => trie.readUInt16LE(16 + 2 * i);
	const val = (i) => trie[data + i];

	function get(c) {
		if (c <= 0xffff) return val(idx(c >> 6) + (c & 63));
		if (c >= highStart) return val(dataLength - 2);
		const i1 = (c >> 14) + BMP_INDEX_LENGTH - OMITTED1;
		let i3Block = idx(idx(i1) + ((c >> 9) & 31));
		const i3 = (c >> 4) & 31;
		let dataBlock;
		if ((i3Block & 0x8000) === 0) {
			dataBlock = idx(i3Block + i3);
		} else {
			i3Block = (i3Block & 0x7fff) + (i3 & ~7) + (i3 >> 3);
			const j = i3 & 7;
			dataBlock = ((idx(i3Block) << (2 + 2 * j)) & 0x30000) | idx(i3Block + 1 + j);
		}
		return val(dataBlock + (c & 15));
	}
	return { get, indexLength, dataLength };
}

// Compress code point → value into (start, end, value) ranges.
export function rangeCompress(get, maxCp = 0x10ffff) {
	const ranges = [];
	let start = -1, cur = -1;
	for (let c = 0; c <= maxCp; c++) {
		const v = get(c);
		if (v === cur && start >= 0) continue;
		if (start >= 0) ranges.push([start, c - 1, cur]);
		start = c; cur = v;
	}
	ranges.push([start, maxCp, cur]);
	return ranges;
}

// ---------------------------------------------------------------------------
// .dict — DataHeader + 8×i32 indexes + trie payload
// ---------------------------------------------------------------------------

export function parseDict(name) {
	const d = readFileSync(dataPath(name));
	const headerSize = d.readUInt16LE(0);
	const ix = [];
	for (let i = 0; i < 8; i++) ix.push(d.readUInt32LE(headerSize + 4 * i));
	const trie = d.subarray(headerSize + ix[0], headerSize + ix[3]);
	return {
		trieType: ix[4],       // bits: TRIE_TYPE_MASK(7) | HAS_VALUES(8)
		transform: ix[5],
		trie,                  // UCharsTrie data = LE u16 units; BytesTrie = bytes
		isUChars: (ix[4] & 7) === 1,
		hasValues: (ix[4] & 8) !== 0,
	};
}

// ---------------------------------------------------------------------------
// UCD 17.0.0 text file parsing
// ---------------------------------------------------------------------------

// Scripts.txt → [{start, end, script}]
export function parseScripts() {
	const out = [];
	const text = readFileSync(dataPath("Scripts.txt"), "utf8");
	for (const line of text.split("\n")) {
		const body = line.split("#")[0].trim();
		if (!body) continue;
		const [rng, script] = body.split(";").map((s) => s.trim());
		let start, end;
		if (rng.includes("..")) {
			const [a, b] = rng.split("..");
			start = parseInt(a, 16); end = parseInt(b, 16);
		} else {
			start = end = parseInt(rng, 16);
		}
		out.push({ start, end, script });
	}
	out.sort((a, b) => a.start - b.start);
	return out;
}

// LineBreak.txt → SA set (as sorted list of code points)
export function parseSaCodePoints() {
	const sa = [];
	const text = readFileSync(dataPath("LineBreak.txt"), "utf8");
	for (const line of text.split("\n")) {
		const body = line.split("#")[0].trim();
		if (!body) continue;
		const parts = body.split(";").map((s) => s.trim());
		if (parts[1] !== "SA") continue;
		const rng = parts[0];
		if (rng.includes("..")) {
			const [a, b] = rng.split("..");
			for (let c = parseInt(a, 16); c <= parseInt(b, 16); c++) sa.push(c);
		} else {
			sa.push(parseInt(rng, 16));
		}
	}
	sa.sort((a, b) => a - b);
	return sa;
}

// UnicodeData.txt → per-code-point records {ccc, gc, decomp (array of cp, null if none), compat (bool)}
export function parseUnicodeData() {
	const byCp = new Map(); // cp -> {ccc, gc, decomp, compat}
	const text = readFileSync(dataPath("UnicodeData.txt"), "utf8");
	for (const line of text.split("\n")) {
		if (!line) continue;
		const fields = line.split(";");
		const cp = parseInt(fields[0], 16);
		const ccc = parseInt(fields[3], 10);
		const gc = fields[2];
		let decomp = null, compat = false;
		if (fields[5]) {
			compat = fields[5].startsWith("<");
			const body = compat ? fields[5].slice(fields[5].indexOf(">") + 1) : fields[5];
			decomp = body.trim().split(/\s+/).filter(Boolean).map((h) => parseInt(h, 16));
		}
		byCp.set(cp, { ccc, gc, decomp, compat });
	}
	// Ranges like "AC00;<Hangul Syllable, First>;Lo;0;L;;;;;N;;;;;" have empty names:
	// expand them so every assigned cp has a record. (First/Last pairs)
	// Expand: track them in a second pass over ordered entries.
	const entries = [...byCp.entries()].sort((a, b) => a[0] - b[0]);
	const expanded = new Map();
	let pending = null;
	for (const [cp, rec] of entries) {
		// UnicodeData range markers arrive with decomp=null and we detect by scanning
		// the original text again — see below.
		expanded.set(cp, rec);
	}
	// Handle <..., First>/<..., Last> range markers directly from the text.
	const lines = text.split("\n");
	for (let i = 0; i < lines.length; i++) {
		const f = lines[i].split(";");
		if (f.length < 4) continue;
		if (!f[1].startsWith("<") || !f[1].endsWith(", First>")) continue;
		const start = parseInt(f[0], 16);
		let j = i + 1;
		while (j < lines.length) {
			const g = lines[j].split(";");
			if (g[1].startsWith("<") && g[1].endsWith(", Last>")) break;
			j++;
		}
		const end = parseInt(lines[j].split(";")[0], 16);
		const gc = f[2];
		for (let c = start; c <= end; c++) expanded.set(c, { ccc: 0, gc, decomp: null, compat: false });
	}
	return expanded;
}
