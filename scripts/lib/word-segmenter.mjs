// JS reference prototype of Node 24.22.0's Intl.Segmenter (word granularity).
//
// Executes the ACTUAL ICU 78.3 machinery:
//   - word.brk RBBI forward state machine (handleNext)
//   - UCPTrie category lookup (via icu-data.mjs)
//   - DictionaryBreakEngine dispatch by script (brkeng.cpp)
//   - Thai/Lao/Khmer/Burmese PossibleWord engines (dictbe.cpp)
//   - CjkBreakEngine Viterbi DP with NFKC normalization (dictbe.cpp)
//   - BytesTrie / UCharsTrie dictionary walkers (exact node arithmetic)
//   - DictionaryCache population + forward iteration (rbbi_cache.cpp,
//     reduced to the observable BreakIterator::first()/current()/next() sequence)
//
// All binary data comes from scripts/data/icu78/ (carved from Node's icudt78l.dat).
// NFKC is implemented from UnicodeData.txt + CompositionExclusions.txt so the
// same table-driven approach ports to Rust.

import { readFileSync } from "node:fs";
import {
	parseWordBrk,
	makeTrieLookup,
	parseDict,
	parseScripts,
	parseSaCodePoints,
	parseUnicodeData,
	dataPath,
} from "./icu-data.mjs";

// ---------------------------------------------------------------------------
// Trie result codes (ustringtrie.h):
//   NO_MATCH=0 NO_VALUE=1 FINAL_VALUE=2 INTERMEDIATE_VALUE=3
//   HAS_NEXT = (r & 1), HAS_VALUE = (r & 2)
// ---------------------------------------------------------------------------
const NO_MATCH = 0;
const NO_VALUE = 1;
const FINAL_VALUE = 2;
const INTERMEDIATE_VALUE = 3;

// ---------------------------------------------------------------------------
// UText-equivalent over a JS string (UTF-16, native index = code-unit index)
// ---------------------------------------------------------------------------
export function cpAt(s, i) {
	if (i < 0 || i >= s.length) return -1;
	const c = s.charCodeAt(i);
	if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
		const c2 = s.charCodeAt(i + 1);
		if (c2 >= 0xdc00 && c2 <= 0xdfff) return (c - 0xd800) * 0x400 + (c2 - 0xdc00) + 0x10000;
	}
	return c;
}
export function cpEndAt(s, i) {
	// Code point ENDING at index i (like utext_previous32's read)
	if (i <= 0 || i > s.length) return -1;
	const c1 = s.charCodeAt(i - 1);
	if (c1 >= 0xdc00 && c1 <= 0xdfff && i >= 2) {
		const c0 = s.charCodeAt(i - 2);
		if (c0 >= 0xd800 && c0 <= 0xdbff) return (c0 - 0xd800) * 0x400 + (c1 - 0xdc00) + 0x10000;
	}
	return c1;
}
export function cpSize(c) {
	return c > 0xffff ? 2 : 1;
}

export class UText {
	constructor(s) {
		this.s = s;
		this.i = 0;
	}
	setIndex(i) {
		this.i = i;
	}
	index() {
		return this.i;
	}
	current32() {
		return cpAt(this.s, this.i);
	}
	next32() {
		const c = cpAt(this.s, this.i);
		if (c >= 0) this.i += cpSize(c);
		return c;
	}
	previous32() {
		const c = cpEndAt(this.s, this.i);
		if (c >= 0) this.i -= cpSize(c);
		return c;
	}
	char32At(i) {
		return cpAt(this.s, i);
	}
	moveIndex32(n) {
		// advance n code points (or to text end)
		while (n > 0 && this.i < this.s.length) {
			this.i += cpSize(cpAt(this.s, this.i));
			n--;
		}
		while (n < 0 && this.i > 0) {
			this.i -= cpSize(cpEndAt(this.s, this.i));
			n++;
		}
	}
}

// ---------------------------------------------------------------------------
// Sorted-range character sets (like ICU UnicodeSet)
// ---------------------------------------------------------------------------
export class CpSet {
	constructor(ranges = []) {
		this.ranges = ranges; // sorted [[lo,hi],...]
	}
	static fromCps(cps) {
		const sorted = [...cps].sort((a, b) => a - b);
		const ranges = [];
		for (const c of sorted) {
			if (ranges.length && ranges[ranges.length - 1][1] === c - 1) ranges[ranges.length - 1][1] = c;
			else ranges.push([c, c]);
		}
		return new CpSet(ranges);
	}
	static range(lo, hi) {
		return new CpSet([[lo, hi]]);
	}
	clone() {
		return new CpSet(this.ranges.map(([a, b]) => [a, b]));
	}
	intersect(o) {
		const out = [];
		let i = 0,
			j = 0;
		while (i < this.ranges.length && j < o.ranges.length) {
			const [a1, a2] = this.ranges[i];
			const [b1, b2] = o.ranges[j];
			const lo = Math.max(a1, b1),
				hi = Math.min(a2, b2);
			if (lo <= hi) out.push([lo, hi]);
			if (a2 < b2) i++;
			else j++;
		}
		return new CpSet(out);
	}
	remove(o) {
		// subtract ranges of o
		const out = [];
		for (const [a1, a2] of this.ranges) {
			let lo = a1;
			const parts = [];
			for (const [b1, b2] of o.ranges) {
				if (b2 < lo || b1 > a2) continue;
				if (b1 > lo) parts.push([lo, b1 - 1]);
				lo = Math.max(lo, b2 + 1);
				if (lo > a2) break;
			}
			if (lo <= a2) parts.push([lo, a2]);
			out.push(...parts);
		}
		return new CpSet(out);
	}
	addCp(c) {
		// ordered insert with merge (scan past ranges entirely below c)
		const rs = this.ranges;
		let k = 0;
		while (k < rs.length && rs[k][1] < c) k++;
		if (k < rs.length && rs[k][0] <= c && rs[k][1] >= c) return this;
		if (k < rs.length && rs[k][0] === c + 1) {
			rs[k][0] = c;
			if (k > 0 && rs[k - 1][1] === c - 1) {
				rs[k - 1][1] = rs[k][1];
				rs.splice(k, 1);
			}
			return this;
		}
		if (k > 0 && rs[k - 1][1] === c - 1) {
			rs[k - 1][1] = c;
			return this;
		}
		rs.splice(k, 0, [c, c]);
		return this;
	}
	addRange(lo, hi) {
		for (let c = lo; c <= hi; c++) this.addCp(c);
		return this;
	}
	has(c) {
		const rs = this.ranges;
		let lo = 0,
			hi = rs.length - 1;
		while (lo <= hi) {
			const mid = (lo + hi) >> 1;
			const [a, b] = rs[mid];
			if (c < a) hi = mid - 1;
			else if (c > b) lo = mid + 1;
			else return true;
		}
		return false;
	}
}

// ---------------------------------------------------------------------------
// NFKC — table-driven from UnicodeData.txt + CompositionExclusions.txt
// (String.prototype.normalize('NFKC') is used only for validation elsewhere)
// ---------------------------------------------------------------------------
function buildNfkc() {
	const ud = parseUnicodeData();
	const ccc = (c) => {
		const r = ud.get(c);
		return r ? r.ccc : 0;
	};
	// composition exclusions file (explicit list)
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
	// canonical decomposition pairs → composition map, honoring full composition exclusion:
	// excluded if in the file, singleton decomp, or decomp starts with a non-starter.
	const pair = new Map(); // key: x * 0x110000 + y → composite
	const qcMaybe = new Set(); // second elements (composition participants)
	for (const [cp, rec] of ud) {
		if (!rec.decomp || rec.compat || rec.decomp.length !== 2) continue;
		const [x, y] = rec.decomp;
		if (ccc(x) !== 0) continue; // non-starter decomposition → excluded
		if (exclusions.has(cp)) continue;
		pair.set(x * 0x110000 + y, cp);
		qcMaybe.add(y);
	}
	// Hangul V/T jamo are composition participants (NFC_QC=Maybe)
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
	function canonicalOrder(seq) {
		// stable insertion sort by ccc
		for (let i = 1; i < seq.length; i++) {
			const ci = ccc(seq[i]);
			if (ci === 0) continue;
			let j = i;
			while (j > 0 && ccc(seq[j - 1]) > ci) {
				const t = seq[j];
				seq[j] = seq[j - 1];
				seq[j - 1] = t;
				j--;
			}
		}
	}
	function composePair(x, y) {
		if (x >= 0x1100 && x <= 0x1112 && y >= 0x1161 && y <= 0x1175) {
			return 0xac00 + ((x - 0x1100) * 21 + (y - 0x1161)) * 28;
		}
		if (x >= 0xac00 && x <= 0xd7a3 && (x - 0xac00) % 28 === 0 && y >= 0x11a8 && y <= 0x11c2) {
			return x + (y - 0x11a7);
		}
		const v = pair.get(x * 0x110000 + y);
		return v === undefined ? -1 : v;
	}
	function nfkc(s) {
		const dec = [];
		for (let i = 0; i < s.length; ) {
			const c = cpAt(s, i);
			decomposeCp(c, dec);
			i += cpSize(c);
		}
		canonicalOrder(dec);
		const out = [];
		let starterPos = -1,
			starterCh = 0,
			lastClass = 0,
			haveStarter = false;
		for (const ch of dec) {
			const cl = ccc(ch);
			let comp = -1;
			if (haveStarter && (lastClass < cl || lastClass === 0)) {
				comp = composePair(starterCh, ch);
			}
			if (comp >= 0) {
				out[starterPos] = starterCh = comp;
				continue;
			}
			out.push(ch);
			if (cl === 0) {
				starterPos = out.length - 1;
				starterCh = ch;
				haveStarter = true;
			}
			lastClass = cl;
		}
		return String.fromCodePoint(...out);
	}
	function hasBoundaryBefore(c) {
		if (ccc(c) !== 0) return false;
		if (qcMaybe.has(c)) return false;
		// A character whose (full, compat) decomposition starts with a
		// backward-combining character (a mark, or a Hangul V/T jamo) also
		// normalization-interacts with preceding text: e.g. FF9E/FF9F map to
		// 3099/309A which compose with a preceding starter (は+ﾞ→ば), so
		// ICU's nfkcNorm2->hasBoundaryBefore() returns false for them.
		let cached = boundaryCache.get(c);
		if (cached === undefined) {
			const out = [];
			decomposeCp(c, out);
			const first = out[0];
			cached = out.length === 1 && first === c ? 1 : ccc(first) !== 0 || qcMaybe.has(first) ? 0 : 1;
			boundaryCache.set(c, cached);
		}
		return cached === 1;
	}
	const boundaryCache = new Map();
	return { nfkc, hasBoundaryBefore, ccc };
}

// ---------------------------------------------------------------------------
// BytesTrie (exact bytestrie.cpp node arithmetic)
// ---------------------------------------------------------------------------
const BT = {
	MAX_BRANCH_LINEAR: 5,
	MIN_LINEAR_MATCH: 0x10,
	MIN_VALUE_LEAD: 0x20,
	VALUE_IS_FINAL: 1,
	MIN_ONE_BYTE_VALUE_LEAD: 0x10,
	MIN_TWO_BYTE_VALUE_LEAD: 0x51,
	MIN_THREE_BYTE_VALUE_LEAD: 0x6c,
	FOUR_BYTE_VALUE_LEAD: 0x7e,
	MIN_TWO_BYTE_DELTA_LEAD: 0xc0,
	MIN_THREE_BYTE_DELTA_LEAD: 0xf0,
	FOUR_BYTE_DELTA_LEAD: 0xfe,
};

function valueResultB(node) {
	return INTERMEDIATE_VALUE - (node & BT.VALUE_IS_FINAL);
}
function readValueB(b, pos, lead) {
	if (lead < BT.MIN_TWO_BYTE_VALUE_LEAD) return lead - BT.MIN_ONE_BYTE_VALUE_LEAD;
	if (lead < BT.MIN_THREE_BYTE_VALUE_LEAD) return ((lead - BT.MIN_TWO_BYTE_VALUE_LEAD) << 8) | b[pos];
	if (lead < BT.FOUR_BYTE_VALUE_LEAD)
		return ((lead - BT.MIN_THREE_BYTE_VALUE_LEAD) << 16) | (b[pos] << 8) | b[pos + 1];
	if (lead === BT.FOUR_BYTE_VALUE_LEAD)
		return (b[pos] << 16) | (b[pos + 1] << 8) | b[pos + 2];
	return (((b[pos] << 24) | (b[pos + 1] << 16) | (b[pos + 2] << 8) | b[pos + 3]) >>> 0);
}
function skipValueB(b, pos, lead) {
	if (lead >= BT.MIN_TWO_BYTE_VALUE_LEAD << 1) {
		if (lead < BT.MIN_THREE_BYTE_VALUE_LEAD << 1) pos += 1;
		else if (lead < BT.FOUR_BYTE_VALUE_LEAD << 1) pos += 2;
		else pos += 3 + ((lead >> 1) & 1);
	}
	return pos;
}
function skipValueFromB(b, pos) {
	const lead = b[pos++];
	return skipValueB(b, pos, lead);
}
function jumpByDeltaB(b, pos) {
	let delta = b[pos++];
	if (delta < BT.MIN_TWO_BYTE_DELTA_LEAD) {
		// one byte
	} else if (delta < BT.MIN_THREE_BYTE_DELTA_LEAD) {
		delta = ((delta - BT.MIN_TWO_BYTE_DELTA_LEAD) << 8) | b[pos++];
	} else if (delta < BT.FOUR_BYTE_DELTA_LEAD) {
		delta = ((delta - BT.MIN_THREE_BYTE_DELTA_LEAD) << 16) | (b[pos] << 8) | b[pos + 1];
		pos += 2;
	} else if (delta === BT.FOUR_BYTE_DELTA_LEAD) {
		delta = (b[pos] << 16) | (b[pos + 1] << 8) | b[pos + 2];
		pos += 3;
	} else {
		delta = (b[pos] << 24) | (b[pos + 1] << 16) | (b[pos + 2] << 8) | b[pos + 3];
		pos += 4;
	}
	return pos + delta;
}
function skipDeltaB(b, pos) {
	const delta = b[pos++];
	if (delta >= BT.MIN_TWO_BYTE_DELTA_LEAD) {
		if (delta < BT.MIN_THREE_BYTE_DELTA_LEAD) pos += 1;
		else if (delta < BT.FOUR_BYTE_DELTA_LEAD) pos += 2;
		else pos += 3 + (delta & 1);
	}
	return pos;
}

export class BytesTrie {
	constructor(bytes) {
		this.b = bytes;
		this.pos = 0;
		this.remaining = -1;
	}
	stop() {
		this.pos = -1;
		this.remaining = -1;
	}
	first(inByte) {
		this.remaining = -1;
		return this.next(inByte);
	}
	next(inByte) {
		const b = this.b;
		if (this.pos < 0) return NO_MATCH;
		const length = this.remaining;
		if (length >= 0) {
			if (inByte === b[this.pos]) {
				this.remaining = length - 1;
				this.pos++;
				if (length - 1 < 0 && b[this.pos] >= BT.MIN_VALUE_LEAD)
					return valueResultB(b[this.pos]);
				return NO_VALUE;
			}
			this.stop();
			return NO_MATCH;
		}
		return this.nextImpl(this.pos, inByte);
	}
	nextImpl(pos, inByte) {
		const b = this.b;
		for (;;) {
			let node = b[pos++];
			if (node < BT.MIN_LINEAR_MATCH) {
				return this.branchNext(pos, node, inByte);
			} else if (node < BT.MIN_VALUE_LEAD) {
				const length = node - BT.MIN_LINEAR_MATCH;
				if (inByte === b[pos++]) {
					this.remaining = length - 1;
					this.pos = pos;
					if (length - 1 < 0 && b[pos] >= BT.MIN_VALUE_LEAD)
						return valueResultB(b[pos]);
					return NO_VALUE;
				}
				break;
			} else if (node & BT.VALUE_IS_FINAL) {
				break;
			} else {
				pos = skipValueB(b, pos, node);
			}
		}
		this.stop();
		return NO_MATCH;
	}
	branchNext(pos, length, inByte) {
		const b = this.b;
		if (length === 0) length = b[pos++];
		++length;
		while (length > BT.MAX_BRANCH_LINEAR) {
			if (inByte < b[pos++]) {
				length >>= 1;
				pos = jumpByDeltaB(b, pos);
			} else {
				length = length - (length >> 1);
				pos = skipDeltaB(b, pos);
			}
		}
		do {
			if (inByte === b[pos++]) {
				let node = b[pos];
				let result;
				if (node & BT.VALUE_IS_FINAL) {
					result = FINAL_VALUE;
				} else {
					pos++;
					const lead = node >> 1;
					let delta;
					if (lead < BT.MIN_TWO_BYTE_VALUE_LEAD) delta = lead - BT.MIN_ONE_BYTE_VALUE_LEAD;
					else if (lead < BT.MIN_THREE_BYTE_VALUE_LEAD) {
						delta = ((lead - BT.MIN_TWO_BYTE_VALUE_LEAD) << 8) | b[pos];
						pos += 1;
					} else if (lead < BT.FOUR_BYTE_VALUE_LEAD) {
						delta = ((lead - BT.MIN_THREE_BYTE_VALUE_LEAD) << 16) | (b[pos] << 8) | b[pos + 1];
						pos += 2;
					} else if (lead === BT.FOUR_BYTE_VALUE_LEAD) {
						delta = (b[pos] << 16) | (b[pos + 1] << 8) | b[pos + 2];
						pos += 3;
					} else {
						delta = (b[pos] << 24) | (b[pos + 1] << 16) | (b[pos + 2] << 8) | b[pos + 3];
						pos += 4;
					}
					pos += delta;
					node = b[pos];
					result = node >= BT.MIN_VALUE_LEAD ? valueResultB(node) : NO_VALUE;
				}
				this.pos = pos;
				return result;
			}
			--length;
			pos = skipValueFromB(b, pos);
		} while (length > 1);
		if (inByte === b[pos++]) {
			this.pos = pos;
			const node = b[pos];
			return node >= BT.MIN_VALUE_LEAD ? valueResultB(node) : NO_VALUE;
		}
		this.stop();
		return NO_MATCH;
	}
	getValue() {
		const b = this.b;
		const leadByte = b[this.pos];
		return readValueB(b, this.pos + 1, leadByte >> 1);
	}
}

// ---------------------------------------------------------------------------
// UCharsTrie (exact ucharstrie.cpp node arithmetic)
// ---------------------------------------------------------------------------
const UT = {
	MAX_BRANCH_LINEAR: 5,
	MIN_LINEAR_MATCH: 0x30,
	MIN_VALUE_LEAD: 0x40,
	NODE_TYPE_MASK: 0x3f,
	VALUE_IS_FINAL: 0x8000,
	MIN_TWO_UNIT_VALUE_LEAD: 0x4000,
	THREE_UNIT_VALUE_LEAD: 0x7fff,
	MIN_TWO_UNIT_NODE_VALUE_LEAD: 0x4040,
	THREE_UNIT_NODE_VALUE_LEAD: 0x7fc0,
	MIN_TWO_UNIT_DELTA_LEAD: 0xfc00,
	THREE_UNIT_DELTA_LEAD: 0xffff,
};

function valueResultU(node) {
	return INTERMEDIATE_VALUE - (node >> 15);
}
function readValueU(u, pos, lead) {
	if (lead < UT.MIN_TWO_UNIT_VALUE_LEAD) return lead;
	if (lead < UT.THREE_UNIT_VALUE_LEAD) return ((lead - UT.MIN_TWO_UNIT_VALUE_LEAD) << 16) | u[pos];
	return (u[pos] << 16) | u[pos + 1];
}
function readNodeValueU(u, pos, lead) {
	if (lead < UT.MIN_TWO_UNIT_NODE_VALUE_LEAD) return (lead >> 6) - 1;
	if (lead < UT.THREE_UNIT_NODE_VALUE_LEAD)
		return (((lead & 0x7fc0) - UT.MIN_TWO_UNIT_NODE_VALUE_LEAD) << 10) | u[pos];
	return (u[pos] << 16) | u[pos + 1];
}
function skipValueU(u, pos, lead) {
	if (lead >= UT.MIN_TWO_UNIT_VALUE_LEAD) pos += lead < UT.THREE_UNIT_VALUE_LEAD ? 1 : 2;
	return pos;
}
function skipValueFromU(u, pos) {
	const lead = u[pos++] & 0x7fff;
	return skipValueU(u, pos, lead);
}
function jumpByDeltaU(u, pos) {
	let delta = u[pos++];
	if (delta >= UT.MIN_TWO_UNIT_DELTA_LEAD) {
		if (delta === UT.THREE_UNIT_DELTA_LEAD) {
			delta = (u[pos] << 16) | u[pos + 1];
			pos += 2;
		} else {
			delta = ((delta - UT.MIN_TWO_UNIT_DELTA_LEAD) << 16) | u[pos++];
		}
	}
	return pos + delta;
}
function skipDeltaU(u, pos) {
	const delta = u[pos++];
	if (delta >= UT.MIN_TWO_UNIT_DELTA_LEAD) pos += delta === UT.THREE_UNIT_DELTA_LEAD ? 2 : 1;
	return pos;
}

export class UCharsTrie {
	constructor(units) {
		this.u = units;
		this.pos = 0;
		this.remaining = -1;
	}
	stop() {
		this.pos = -1;
		this.remaining = -1;
	}
	first(uchar) {
		this.remaining = -1;
		return this.next(uchar);
	}
	next(uchar) {
		const u = this.u;
		if (this.pos < 0) return NO_MATCH;
		const length = this.remaining;
		if (length >= 0) {
			if (uchar === u[this.pos]) {
				this.remaining = length - 1;
				this.pos++;
				if (length - 1 < 0 && u[this.pos] >= UT.MIN_VALUE_LEAD)
					return valueResultU(u[this.pos]);
				return NO_VALUE;
			}
			this.stop();
			return NO_MATCH;
		}
		return this.nextImpl(this.pos, uchar);
	}
	nextImpl(pos, uchar) {
		const u = this.u;
		let node = u[pos++];
		for (;;) {
			if (node < UT.MIN_LINEAR_MATCH) {
				return this.branchNext(pos, node, uchar);
			} else if (node < UT.MIN_VALUE_LEAD) {
				const length = node - UT.MIN_LINEAR_MATCH;
				if (uchar === u[pos++]) {
					this.remaining = length - 1;
					this.pos = pos;
					if (length - 1 < 0 && u[pos] >= UT.MIN_VALUE_LEAD)
						return valueResultU(u[pos]);
					return NO_VALUE;
				}
				break;
			} else if (node & UT.VALUE_IS_FINAL) {
				break;
			} else {
				pos = skipNodeValueU(u, pos, node);
				node &= UT.NODE_TYPE_MASK;
			}
		}
		this.stop();
		return NO_MATCH;
	}
	branchNext(pos, length, uchar) {
		const u = this.u;
		if (length === 0) length = u[pos++];
		++length;
		while (length > UT.MAX_BRANCH_LINEAR) {
			if (uchar < u[pos++]) {
				length >>= 1;
				pos = jumpByDeltaU(u, pos);
			} else {
				length = length - (length >> 1);
				pos = skipDeltaU(u, pos);
			}
		}
		do {
			if (uchar === u[pos++]) {
				let node = u[pos];
				let result;
				if (node & UT.VALUE_IS_FINAL) {
					result = FINAL_VALUE;
				} else {
					pos++;
					let delta;
					if (node < UT.MIN_TWO_UNIT_VALUE_LEAD) delta = node;
					else if (node < UT.THREE_UNIT_VALUE_LEAD) {
						delta = ((node - UT.MIN_TWO_UNIT_VALUE_LEAD) << 16) | u[pos];
						pos += 1;
					} else {
						delta = (u[pos] << 16) | u[pos + 1];
						pos += 2;
					}
					pos += delta;
					node = u[pos];
					result = node >= UT.MIN_VALUE_LEAD ? valueResultU(node) : NO_VALUE;
				}
				this.pos = pos;
				return result;
			}
			--length;
			pos = skipValueFromU(u, pos);
		} while (length > 1);
		if (uchar === u[pos++]) {
			this.pos = pos;
			const node = u[pos];
			return node >= UT.MIN_VALUE_LEAD ? valueResultU(node) : NO_VALUE;
		}
		this.stop();
		return NO_MATCH;
	}
	getValue() {
		const u = this.u;
		const pos = this.pos;
		const lead = u[pos];
		return lead & UT.VALUE_IS_FINAL
			? readValueU(u, pos + 1, lead & 0x7fff)
			: readNodeValueU(u, pos + 1, lead);
	}
}
function skipNodeValueU(u, pos, lead) {
	if (lead >= UT.MIN_TWO_UNIT_NODE_VALUE_LEAD)
		pos += lead < UT.THREE_UNIT_NODE_VALUE_LEAD ? 1 : 2;
	return pos;
}

// ---------------------------------------------------------------------------
// Dictionary matchers (dictionarydata.cpp matches())
// ---------------------------------------------------------------------------
function ucharsMatches(ut, dictUnits, maxLength, limit, lengths, cpLengths, values, prefixOut) {
	const uct = new UCharsTrie(dictUnits);
	const startingTextIndex = ut.index();
	let wordCount = 0;
	let codePointsMatched = 0;
	for (let c = ut.next32(); c >= 0; c = ut.next32()) {
		const result = codePointsMatched === 0 ? uct.first(c) : uct.next(c);
		const lengthMatched = ut.index() - startingTextIndex;
		codePointsMatched += 1;
		if (result & 2) {
			// HAS_VALUE
			if (wordCount < limit) {
				if (values) values[wordCount] = uct.getValue();
				if (lengths) lengths[wordCount] = lengthMatched;
				if (cpLengths) cpLengths[wordCount] = codePointsMatched;
				wordCount++;
			}
			if (result === FINAL_VALUE) break;
		} else if (result === NO_MATCH) break;
		if (lengthMatched >= maxLength) break;
	}
	if (prefixOut) prefixOut.value = codePointsMatched;
	return wordCount;
}

function bytesMatches(ut, dict, maxLength, limit, lengths, cpLengths, values, prefixOut) {
	const bt = new BytesTrie(dict.trie);
	const startingTextIndex = ut.index();
	let wordCount = 0;
	let codePointsMatched = 0;
	const offset = dict.transform & 0x1fffff;
	const transform = (c) => {
		if (c === 0x200d) return 0xff;
		if (c === 0x200c) return 0xfe;
		const delta = c - offset;
		if (delta < 0 || delta > 0xfd) return -1;
		return delta;
	};
	for (let c = ut.next32(); c >= 0; c = ut.next32()) {
		const result =
			codePointsMatched === 0 ? bt.first(transform(c)) : bt.next(transform(c));
		const lengthMatched = ut.index() - startingTextIndex;
		codePointsMatched += 1;
		if (result & 2) {
			if (wordCount < limit) {
				if (values) values[wordCount] = bt.getValue();
				if (lengths) lengths[wordCount] = lengthMatched;
				if (cpLengths) cpLengths[wordCount] = codePointsMatched;
				wordCount++;
			}
			if (result === FINAL_VALUE) break;
		} else if (result === NO_MATCH) break;
		if (lengthMatched >= maxLength) break;
	}
	if (prefixOut) prefixOut.value = codePointsMatched;
	return wordCount;
}

// ---------------------------------------------------------------------------
// Lazy global state (parsed once)
// ---------------------------------------------------------------------------
let _state = null;

function getState() {
	if (_state) return _state;
	const brk = parseWordBrk();
	const cat = makeTrieLookup(brk.trie);
	const fwd = brk.forward;
	const rows = [];
	for (let s = 0; s < fwd.numStates; s++) {
		const off = s * fwd.rowLen;
		rows.push({
			accepting: fwd.rows[off],
			lookAhead: fwd.rows[off + 1],
			tagsIdx: fwd.rows[off + 2],
			next: Array.from(fwd.rows.subarray(off + 3, off + fwd.rowLen)),
		});
	}

	// --- dictionaries ---
	const cjDict = parseDict("cjdict.dict");
	const cjUnits = new Uint16Array(cjDict.trie.length / 2);
	for (let i = 0; i < cjUnits.length; i++) cjUnits[i] = cjDict.trie.readUInt16LE(2 * i);
	const thaiDict = parseDict("thaidict.dict");
	const laoDict = parseDict("laodict.dict");
	const khmerDict = parseDict("khmerdict.dict");
	const burmeseDict = parseDict("burmesedict.dict");

	// --- UCD-derived sets ---
	const scripts = parseScripts();
	const scriptOf = (c) => {
		let lo = 0,
			hi = scripts.length - 1;
		while (lo <= hi) {
			const mid = (lo + hi) >> 1;
			const r = scripts[mid];
			if (c < r.start) hi = mid - 1;
			else if (c > r.end) lo = mid + 1;
			else return r.script;
		}
		return "Unknown";
	};
	const scriptSet = (name) => {
		const rs = scripts.filter((r) => r.script === name).map((r) => [r.start, r.end]);
		return new CpSet(rs);
	};
	const saSet = CpSet.fromCps(parseSaCodePoints());
	const ud = parseUnicodeData();
	const markSetFor = (scriptSet_) => {
		const m = [];
		for (const [lo, hi] of scriptSet_.ranges) {
			for (let c = lo; c <= hi; c++) {
				const r = ud.get(c);
				if (r && r.gc[0] === "M") m.push(c);
			}
		}
		return CpSet.fromCps(m);
	};

	// Thai engine sets (dictbe.cpp ThaiBreakEngine ctor)
	const thaiWordSet = scriptSet("Thai").intersect(saSet);
	const thaiEngine = {
		name: "Thai",
		dict: thaiDict,
		set: thaiWordSet,
		endWordSet: thaiWordSet.clone().remove(CpSet.range(0x0e31, 0x0e31)).remove(CpSet.range(0x0e40, 0x0e44)),
		beginWordSet: CpSet.range(0x0e01, 0x0e2e).clone().addRange(0x0e40, 0x0e44),
		markSet: markSetFor(scriptSet("Thai").intersect(saSet)).clone().addCp(0x20),
		suffixSet: CpSet.fromCps([0x0e2f, 0x0e46]),
		isThai: true,
	};
	// Lao
	const laoWordSet = scriptSet("Lao").intersect(saSet);
	const laoEngine = {
		name: "Lao",
		dict: laoDict,
		set: laoWordSet,
		endWordSet: laoWordSet.clone().remove(CpSet.range(0x0ec0, 0x0ec4)),
		beginWordSet: CpSet.range(0x0e81, 0x0eae).clone().addRange(0x0edc, 0x0edd).addRange(0x0ec0, 0x0ec4),
		markSet: markSetFor(scriptSet("Lao").intersect(saSet)).clone().addCp(0x20),
		isThai: false,
	};
	// Khmer
	const khmerWordSet = scriptSet("Khmer").intersect(saSet);
	const khmerEngine = {
		name: "Khmer",
		dict: khmerDict,
		set: khmerWordSet,
		endWordSet: khmerWordSet.clone().remove(CpSet.range(0x17d2, 0x17d2)),
		beginWordSet: CpSet.range(0x1780, 0x17b3),
		markSet: markSetFor(scriptSet("Khmer").intersect(saSet)).clone().addCp(0x20),
		isThai: false,
	};
	// Burmese
	const burmeseWordSet = scriptSet("Myanmar").intersect(saSet);
	const burmeseEngine = {
		name: "Burmese",
		dict: burmeseDict,
		set: burmeseWordSet,
		endWordSet: burmeseWordSet.clone(),
		beginWordSet: CpSet.range(0x1000, 0x102a),
		markSet: markSetFor(scriptSet("Myanmar").intersect(saSet)).clone().addCp(0x20),
		isThai: false,
	};
	// CJK (Chinese/Japanese): [[:Han:][:Hiragana:][:Katakana:] ｰ ｱ-style extras]
	const cjSet = scriptSet("Han")
		.clone()
		.addRange(0x30fc, 0x30fc)
		.addRange(0xff70, 0xff70)
		.addRange(0xff9e, 0xff9e)
		.addRange(0xff9f, 0xff9f);
	// (Hiragana/Katakana scripts are subsets of the added ranges + main blocks)
	for (const name of ["Hiragana", "Katakana"]) {
		for (const [lo, hi] of scriptSet(name).ranges) cjSet.addRange(lo, hi);
	}
	const hangulSet = CpSet.range(0xac00, 0xd7a3);

	const nfkcData = buildNfkc();

	const cjEngine = { set: cjSet, dictUnits: cjUnits };
	const engineObjs = {
		Thai: makeSeaEngine(thaiEngine),
		Lao: makeSeaEngine(laoEngine),
		Khmer: makeSeaEngine(khmerEngine),
		Myanmar: makeSeaEngine(burmeseEngine),
		Cjk: makeCjkEngine(cjEngine),
	};

	_state = {
		brk,
		cat,
		rows,
		dictStart: fwd.dictCategoriesStart,
		statusTable: brk.statusTable,
		scriptOf,
		engineObjs,
		cjEngine,
		hangulSet,
		nfkc: nfkcData,
		scriptSetRanges: (name) => scriptSet(name).ranges,
	};
	return _state;
}

// ---------------------------------------------------------------------------
// RBBI handleNext (rbbi.cpp) — forward state machine over word.brk
// ---------------------------------------------------------------------------
const START_STATE = 1;
const STOP_STATE = 0;
const DONE = -1;

function handleNext(text, initialPosition) {
	// returns { pos, statusIdx, dictCount, done }
	const st = getState();
	const rows = st.rows;
	const dictStart = st.dictStart;
	let statusIdx = 0;
	let dictCount = 0;

	text.setIndex(initialPosition);
	let result = initialPosition;
	let c = text.next32();
	if (c < 0) {
		return { done: true };
	}

	let state = START_STATE;
	let row = rows[state];
	let mode = 0; // RBBI_RUN=0, RBBI_START=1, RBBI_END=2
	let category = 0;
	// word.brk flags: RBBI_8BITS_ROWS only — no BOF, so mode stays RBBI_RUN.

	for (;;) {
		if (c < 0) {
			if (mode === 2) break;
			mode = 2;
			category = 1;
		}
		if (mode === 0) {
			category = c < 0 ? 0 : st.cat.get(c);
			if (category >= dictStart) dictCount++;
		}

		state = row.next[category];
		row = rows[state];

		const accepting = row.accepting;
		if (accepting === 1) {
			if (mode !== 1) result = text.index();
			statusIdx = row.tagsIdx;
		} else if (accepting > 1) {
			// look-ahead match completed — word.brk has no lookahead rules
			// (fLookAheadResultsSize == 0), unreachable.
			throw new Error("unexpected lookahead state in word.brk");
		}
		const rule = row.lookAhead;
		if (rule > 1) throw new Error("unexpected lookahead rule in word.brk");

		if (state === STOP_STATE) break;

		if (mode === 0) {
			c = text.next32();
		} else if (mode === 1) {
			mode = 0;
		}
	}

	if (result === initialPosition) {
		// failed to advance; force ahead by one code point
		text.setIndex(initialPosition);
		text.next32();
		result = text.index();
		statusIdx = 0;
	}
	return { pos: result, statusIdx, dictCount, done: false };
}

// ---------------------------------------------------------------------------
// PossibleWord (dictbe.cpp)
// ---------------------------------------------------------------------------
const POSSIBLE_WORD_LIST_MAX = 20;

class PossibleWord {
	constructor() {
		this.count = 0;
		this.prefix = 0;
		this.offset = -1;
		this.mark = 0;
		this.current = 0;
		this.cuLengths = new Array(POSSIBLE_WORD_LIST_MAX);
		this.cpLengths = new Array(POSSIBLE_WORD_LIST_MAX);
	}
	candidates(text, dict, rangeEnd) {
		const start = text.index();
		if (start !== this.offset) {
			this.offset = start;
			const prefixOut = { value: 0 };
			this.count = bytesMatches(
				text,
				dict,
				rangeEnd - start,
				POSSIBLE_WORD_LIST_MAX,
				this.cuLengths,
				this.cpLengths,
				null,
				prefixOut,
			);
			this.prefix = prefixOut.value;
			if (this.count <= 0) {
				text.setIndex(start);
			}
		}
		if (this.count > 0) {
			text.setIndex(start + this.cuLengths[this.count - 1]);
		}
		this.current = this.count - 1;
		this.mark = this.current;
		return this.count;
	}
	acceptMarked(text) {
		text.setIndex(this.offset + this.cuLengths[this.mark]);
		return this.cuLengths[this.mark];
	}
	backUp(text) {
		if (this.current > 0) {
			this.current--;
			text.setIndex(this.offset + this.cuLengths[this.current]);
			return true;
		}
		return false;
	}
	longestPrefix() {
		return this.prefix;
	}
	markCurrent() {
		this.mark = this.current;
	}
	markedCPLength() {
		return this.cpLengths[this.mark];
	}
}

// ---------------------------------------------------------------------------
// SEA engines: Thai/Lao/Khmer/Burmese divideUpDictionaryRange (dictbe.cpp)
// ---------------------------------------------------------------------------
function seaDivideUpDictionaryRange(text, rangeStart, rangeEnd, foundBreaks, eng) {
	const MIN_WORD_SPAN = 4; // *_MIN_WORD * 2
	if (eng.isThai) {
		// Thai (dictbe.cpp:241): move 4 CODE POINTS, bail if that reaches rangeEnd
		text.setIndex(rangeStart);
		text.moveIndex32(MIN_WORD_SPAN);
		if (text.index() >= rangeEnd) {
			return 0; // Not enough characters for two words
		}
	} else if (rangeEnd - rangeStart < MIN_WORD_SPAN) {
		// Lao/Khmer/Burmese (dictbe.cpp:480/879/672): CODE UNIT arithmetic
		return 0;
	}
	text.setIndex(rangeStart);

	let wordsFound = 0;
	let cpWordLength = 0;
	let cuWordLength = 0;
	let current;
	const words = [new PossibleWord(), new PossibleWord(), new PossibleWord()];

	text.setIndex(rangeStart);
	while ((current = text.index()) < rangeEnd) {
		cpWordLength = 0;
		cuWordLength = 0;

		const candidates = words[wordsFound % 3].candidates(text, eng.dict, rangeEnd);

		if (candidates === 1) {
			cuWordLength = words[wordsFound % 3].acceptMarked(text);
			cpWordLength = words[wordsFound % 3].markedCPLength();
			wordsFound += 1;
		} else if (candidates > 1) {
			// If we're already at the end of the range, we're done
			foundBest: {
				if (text.index() >= rangeEnd) break foundBest;
				do {
					if (words[(wordsFound + 1) % 3].candidates(text, eng.dict, rangeEnd) > 0) {
						words[wordsFound % 3].markCurrent();
						if (text.index() >= rangeEnd) break foundBest;
						do {
							if (words[(wordsFound + 2) % 3].candidates(text, eng.dict, rangeEnd)) {
								words[wordsFound % 3].markCurrent();
								break foundBest;
							}
						} while (words[(wordsFound + 1) % 3].backUp(text));
					}
				} while (words[wordsFound % 3].backUp(text));
			}
			cuWordLength = words[wordsFound % 3].acceptMarked(text);
			cpWordLength = words[wordsFound % 3].markedCPLength();
			wordsFound += 1;
		}

		let uc = 0;
		if (text.index() < rangeEnd && cpWordLength < 3) {
			if (
				words[wordsFound % 3].candidates(text, eng.dict, rangeEnd) <= 0 &&
				(cuWordLength === 0 || words[wordsFound % 3].longestPrefix() < 3)
			) {
				// Look for a plausible word boundary
				let remaining = rangeEnd - (current + cuWordLength);
				let chars = 0;
				let pc;
				for (;;) {
					const pcIndex = text.index();
					pc = text.next32();
					const pcSize = text.index() - pcIndex;
					chars += pcSize;
					remaining -= pcSize;
					if (remaining <= 0) break;
					uc = text.current32();
					if (eng.endWordSet.has(pc) && eng.beginWordSet.has(uc)) {
						const numCandidates = words[(wordsFound + 1) % 3].candidates(text, eng.dict, rangeEnd);
						text.setIndex(current + cuWordLength + chars);
						if (numCandidates > 0) break;
					}
				}
				if (cuWordLength <= 0) wordsFound += 1;
				cuWordLength += chars;
			} else {
				text.setIndex(current + cuWordLength);
			}
		}

		// Never stop before a combining mark.
		while (text.index() < rangeEnd && eng.markSet.has(text.current32())) {
			const currPos = text.index();
			text.next32();
			cuWordLength += text.index() - currPos;
		}

		// Thai suffix block (PAIYANNOI / MAIYAMOK)
		if (eng.isThai && text.index() < rangeEnd && cuWordLength > 0) {
			if (
				words[wordsFound % 3].candidates(text, eng.dict, rangeEnd) <= 0 &&
				eng.suffixSet.has((uc = text.current32()))
			) {
				if (uc === 0x0e2f) {
					// PAIYANNOI
					if (!eng.suffixSet.has(text.previous32())) {
						text.next32();
						const paiyannoiIndex = text.index();
						text.next32();
						cuWordLength += text.index() - paiyannoiIndex;
						uc = text.current32();
					} else {
						text.next32();
					}
				}
				if (uc === 0x0e46) {
					// MAIYAMOK
					if (text.previous32() !== 0x0e46) {
						text.next32();
						const maiyamokIndex = text.index();
						text.next32();
						cuWordLength += text.index() - maiyamokIndex;
					} else {
						text.next32();
					}
				}
			} else {
				text.setIndex(current + cuWordLength);
			}
		}

		if (cuWordLength > 0) {
			foundBreaks.push(current + cuWordLength);
		}
	}

	if (foundBreaks.length > 0 && foundBreaks[foundBreaks.length - 1] >= rangeEnd) {
		foundBreaks.pop();
		wordsFound -= 1;
	}
	return wordsFound;
}

// ---------------------------------------------------------------------------
// CjkBreakEngine::divideUpDictionaryRange (dictbe.cpp) — Viterbi + NFKC
// ---------------------------------------------------------------------------
const KUINT32MAX = 0xffffffff;
const MAX_SNLP = 255;
const MAX_KATAKANA_LENGTH = 8;
const MAX_KATAKANA_GROUP_LENGTH = 20;
const KATAKANA_COST = [8192, 984, 408, 240, 204, 252, 300, 372, 480];

function getKatakanaCost(wordLength) {
	return wordLength > MAX_KATAKANA_LENGTH ? 8192 : KATAKANA_COST[wordLength];
}
function isKatakana(v) {
	return (
		(v >= 0x30a1 && v <= 0x30fe && v !== 0x30fb) || (v >= 0xff66 && v <= 0xff9f)
	);
}

function cjkDivideUpDictionaryRange(text, rangeStart, rangeEnd, foundBreaks, cj) {
	if (rangeStart >= rangeEnd) return 0;
	const st = getState();
	const nfkc = st.nfkc;

	// Contiguous-chunk fast path (UText over UTF-16 string is always one chunk):
	let inString = text.s.slice(rangeStart, rangeEnd);
	let inputMap = null; // null → 1:1 with rangeStart offset

	if (nfkc.nfkc(inString) !== inString) {
		const normalizedParts = [];
		let normalizedLength = 0;
		const normalizedMap = [];
		for (let srcI = 0; srcI < inString.length; ) {
			const fragmentStartI = srcI;
			let c = cpAt(inString, srcI);
			const fragment = [];
			for (;;) {
				fragment.push(c);
				srcI += cpSize(c);
				if (srcI === inString.length) break;
				c = cpAt(inString, srcI);
				if (nfkc.hasBoundaryBefore(c)) break;
			}
			const nf = nfkc.nfkc(String.fromCodePoint(...fragment));
			normalizedParts.push(nf);
			normalizedLength += nf.length;
			const fragmentOriginalStart = inputMap !== null ? inputMap[fragmentStartI] : fragmentStartI + rangeStart;
			while (normalizedMap.length < normalizedLength) normalizedMap.push(fragmentOriginalStart);
		}
		const nativeEnd = inputMap !== null ? inputMap[inString.length] : inString.length + rangeStart;
		normalizedMap.push(nativeEnd);
		inputMap = normalizedMap;
		inString = normalizedParts.join("");
	}

	let numCodePts = 0;
	for (let i = 0; i < inString.length; ) {
		numCodePts++;
		i += cpSize(cpAt(inString, i));
	}
	if (numCodePts !== inString.length) {
		// Supplementary characters: convert inputMap to code-point indexing
		const hadExistingMap = inputMap !== null;
		const newMap = [];
		let cpIdx = 0;
		let cuIdx = 0;
		for (;;) {
			if (hadExistingMap) newMap[cpIdx] = inputMap[cuIdx];
			else newMap.push(cuIdx + rangeStart);
			cpIdx++;
			if (cuIdx === inString.length) break;
			cuIdx += cpSize(cpAt(inString, cuIdx));
		}
		inputMap = newMap;
	}

	// bestSnlp[i] = snlp of best segmentation of first i code points
	const bestSnlp = new Array(numCodePts + 1).fill(KUINT32MAX);
	bestSnlp[0] = 0;
	const prev = new Array(numCodePts + 1).fill(-1);

	const maxWordSize = 20;
	const values = new Array(numCodePts).fill(0);
	const lengths = new Array(numCodePts).fill(0);

	// Viterbi DP
	let ix = 0;
	let is_prev_katakana = false;
	for (let i = 0; i < numCodePts; ++i, ix += cpSize(cpAt(inString, ix))) {
		if (bestSnlp[i] === KUINT32MAX) continue;

		const fu = new UText(inString);
		fu.setIndex(ix);
		// matches(&fu, maxWordSize, numCodePts, cuLengths=null, cpLengths=lengths, values=values, prefix=null)
		let count = ucharsMatches(fu, cj.dictUnits, maxWordSize, numCodePts, null, lengths, values, null);

		// if there are no single character matches found in the dictionary starting
		// with this character, treat character as a 1-character word with the highest
		// value possible. Exclude Hangul.
		if ((count === 0 || lengths[0] !== 1) && !st.hangulSet.has(cpAt(inString, ix))) {
			// UVector32::setElementAt is a no-op out of bounds — replicate exactly.
			if (count < values.length) values[count] = MAX_SNLP;
			if (count < lengths.length) lengths[count] = 1;
			count++;
		}

		for (let j = 0; j < count; j++) {
			const newSnlp = (bestSnlp[i] + values[j]) >>> 0;
			const lnji = lengths[j] + i;
			if (newSnlp < bestSnlp[lnji]) {
				bestSnlp[lnji] = newSnlp;
				prev[lnji] = i;
			}
		}

		const is_katakana = isKatakana(cpAt(inString, ix));
		if (!is_prev_katakana && is_katakana) {
			let katakanaRunLength = 1;
			let j = ix + cpSize(cpAt(inString, ix));
			while (
				j < inString.length &&
				katakanaRunLength < MAX_KATAKANA_GROUP_LENGTH &&
				isKatakana(cpAt(inString, j))
			) {
				j += cpSize(cpAt(inString, j));
				katakanaRunLength++;
			}
			if (katakanaRunLength < MAX_KATAKANA_GROUP_LENGTH) {
				const newSnlp = (bestSnlp[i] + getKatakanaCost(katakanaRunLength)) >>> 0;
				if (newSnlp < bestSnlp[i + katakanaRunLength]) {
					bestSnlp[i + katakanaRunLength] = newSnlp;
					prev[i + katakanaRunLength] = i;
				}
			}
		}
		is_prev_katakana = is_katakana;
	}

	// Backtrack
	const t_boundary = [];
	let numBreaks = 0;
	if (bestSnlp[numCodePts] === KUINT32MAX) {
		t_boundary.push(numCodePts);
		numBreaks++;
	} else {
		for (let i = numCodePts; i > 0; i = prev[i]) {
			t_boundary.push(i);
			numBreaks++;
		}
	}
	if (foundBreaks.length === 0 || foundBreaks[foundBreaks.length - 1] < rangeStart) {
		t_boundary.push(0);
		numBreaks++;
	}

	// Convert positions back to original text indices
	let prevUTextPos = -1;
	let correctedNumBreaks = 0;
	for (let i = numBreaks - 1; i >= 0; i--) {
		const cpPos = t_boundary[i];
		const utextPos = inputMap !== null ? inputMap[cpPos] : cpPos + rangeStart;
		if (utextPos > prevUTextPos) {
			// (isPhraseBreaking=false)
			if (utextPos !== rangeStart) {
				foundBreaks.push(utextPos);
				correctedNumBreaks++;
			}
		} else {
			numBreaks--;
		}
		prevUTextPos = utextPos;
	}

	if (foundBreaks.length > 0 && foundBreaks[foundBreaks.length - 1] === rangeEnd) {
		foundBreaks.pop();
		correctedNumBreaks--;
	}
	return correctedNumBreaks;
}

// ---------------------------------------------------------------------------
// Language break engine dispatch
// (rbbi.cpp getLanguageBreakEngine + brkeng.cpp ICULanguageBreakFactory,
//  UnhandledEngine, loadEngineFor) — process-global factory cache semantics.
// ---------------------------------------------------------------------------
// ICULanguageBreakFactory::fEngines — process-global list, iterated in
// REVERSE. Engines are created lazily by loadEngineFor and stay in the list
// for the lifetime of the process (the reference probe and Node both run in
// a single process, so module scope matches ICU's singleton).
let factoryEngineList = null;

// brkeng.cpp loadEngineFor: LSTM first (no LSTM .res in icudt78l.dat → always
// falls through), then loadDictionaryMatcherFor(script): Hani/Hira/Kana →
// cjdict, Khmr → khmerdict, Laoo → laodict, Mymr → burmesedict, Thai →
// thaidict. Common ('Zyyy') and Hangul have no dictionary → no engine.
function loadEngineFor(c) {
	const st = getState();
	switch (st.scriptOf(c)) {
		case "Thai":
			return st.engineObjs.Thai;
		case "Lao":
			return st.engineObjs.Lao;
		case "Khmer":
			return st.engineObjs.Khmer;
		case "Myanmar":
			return st.engineObjs.Myanmar;
		case "Han":
		case "Hiragana":
		case "Katakana":
			return st.engineObjs.Cjk;
		default:
			return null;
	}
}

// ICULanguageBreakFactory::getEngineFor: reverse iteration over the global
// engine list, then loadEngineFor creation (pushed to the global list).
function factoryGetEngineFor(c) {
	if (factoryEngineList === null) factoryEngineList = [];
	for (let i = factoryEngineList.length - 1; i >= 0; i--) {
		if (factoryEngineList[i].handles(c)) return factoryEngineList[i];
	}
	const eng = loadEngineFor(c);
	if (eng !== null) factoryEngineList.push(eng);
	return eng;
}

// DictionaryBreakEngine::handles is fSet.contains(c) for every engine.
function makeSeaEngine(cfg) {
	return {
		handles: (c) => cfg.set.has(c),
		// DictionaryBreakEngine::findBreaks (base, dictbe.cpp): span extension
		// while fSet.contains, then divideUpDictionaryRange over the span.
		findBreaks(text, startPos, endPos, foundBreaks) {
			text.setIndex(startPos);
			let current;
			let ch = text.current32();
			while ((current = text.index()) < endPos && cfg.set.has(ch)) {
				text.next32();
				ch = text.current32();
			}
			const n = seaDivideUpDictionaryRange(text, startPos, current, foundBreaks, cfg);
			text.setIndex(current);
			return n;
		},
	};
}
function makeCjkEngine(cj) {
	return {
		handles: (c) => cj.set.has(c),
		findBreaks(text, startPos, endPos, foundBreaks) {
			text.setIndex(startPos);
			let current;
			let ch = text.current32();
			while ((current = text.index()) < endPos && cj.set.has(ch)) {
				text.next32();
				ch = text.current32();
			}
			const n = cjkDivideUpDictionaryRange(text, startPos, current, foundBreaks, cj);
			text.setIndex(current);
			return n;
		},
	};
}

// brkeng.cpp UnhandledEngine — per BreakIterator instance, i.e. per
// segmentWords() call in the V8 mapping (each segment() builds a fresh
// iterator with a fresh instance engine stack + reject set).
function makeUnhandledEngine() {
	const st = getState();
	return {
		handled: new CpSet([]),
		handles(c) {
			return this.handled.has(c);
		},
		handleCharacter(c) {
			if (!this.handled.has(c)) {
				// UnicodeSet::applyIntPropertyValue(UCHAR_SCRIPT, script) —
				// replaces the set with the entire script of c.
				this.handled = new CpSet(st.scriptSetRanges(st.scriptOf(c)));
			}
		},
		findBreaks(text, startPos, endPos) {
			text.setIndex(startPos);
			let current;
			let ch = text.current32();
			while ((current = text.index()) < endPos && this.handled.has(ch)) {
				text.next32();
				ch = text.current32();
			}
			return 0;
		},
	};
}

// rbbi.cpp RuleBasedBreakIterator::getLanguageBreakEngine:
//  1. instance engine list in REVERSE (UnhandledEngine sits at index 0, so it
//     is checked last — but still BEFORE the factory)
//  2. factory (process-global, reverse iteration, lazy creation)
//  3. factory null → reject the character's whole script via UnhandledEngine
function getLanguageBreakEngine(c, ctx) {
	for (let i = ctx.engines.length - 1; i >= 0; i--) {
		if (ctx.engines[i].handles(c)) return ctx.engines[i];
	}
	const lbe = factoryGetEngineFor(c);
	if (lbe !== null) {
		ctx.engines.push(lbe); // fLanguageBreakEngines->push
		return lbe;
	}
	if (ctx.unhandled === null) {
		ctx.unhandled = makeUnhandledEngine();
		ctx.engines.unshift(ctx.unhandled); // insertElementAt(0)
	}
	ctx.unhandled.handleCharacter(c);
	return ctx.unhandled;
}

// ---------------------------------------------------------------------------
// DictionaryCache (rbbi_cache.cpp) — forward-only semantics
// ---------------------------------------------------------------------------
class DictionaryCache {
	constructor() {
		this.breaks = [];
		this.start = 0;
		this.limit = 0;
		this.firstRuleStatusIndex = 0;
		this.otherRuleStatusIndex = 0;
	}
	reset() {
		this.breaks = [];
		this.start = 0;
		this.limit = 0;
		this.firstRuleStatusIndex = 0;
		this.otherRuleStatusIndex = 0;
	}
	following(fromPos) {
		// returns { result, statusIndex } or null
		if (fromPos >= this.limit || fromPos < this.start) return null;
		for (const r of this.breaks) {
			if (r > fromPos) return { result: r, statusIndex: this.otherRuleStatusIndex };
		}
		return null; // unreachable in ICU (UPRV_UNREACHABLE_EXIT)
	}
	populate(startPos, endPos, firstRuleStatus, otherRuleStatus, text, ctx) {
		if (endPos - startPos <= 1) return; // keep stale cache (ICU does not reset)
		this.reset();
		this.firstRuleStatusIndex = firstRuleStatus;
		this.otherRuleStatusIndex = otherRuleStatus;

		const st = getState();
		const rangeEnd = endPos;
		let foundBreakCount = 0;

		text.setIndex(startPos);
		let c = text.current32();
		let category = c < 0 ? 0 : st.cat.get(c);
		const dictStart = st.dictStart;

		for (;;) {
			while (text.index() < rangeEnd && category < dictStart) {
				text.next32();
				c = text.current32();
				category = c < 0 ? 0 : st.cat.get(c);
			}
			if (text.index() >= rangeEnd) break;

			// dictionary character at text.index(); dispatch engine
			const lbe = getLanguageBreakEngine(c, ctx);
			foundBreakCount += lbe.findBreaks(text, text.index(), rangeEnd, this.breaks);

			c = text.current32();
			category = c < 0 ? 0 : st.cat.get(c);
		}

		if (foundBreakCount > 0) {
			if (startPos < this.breaks[0]) this.breaks.unshift(startPos);
			if (endPos > this.breaks[this.breaks.length - 1]) this.breaks.push(endPos);
			this.start = this.breaks[0];
			this.limit = this.breaks[this.breaks.length - 1];
		}
	}
}

// ---------------------------------------------------------------------------
// Forward boundary iteration — the observable first()/current()/next() sequence
// ---------------------------------------------------------------------------
function iterateBoundaries(str) {
	const st = getState();
	const text = new UText(str);
	const dict = new DictionaryCache();
	// Per-BreakIterator engine state: each segment() call builds a fresh
	// iterator in V8, so the instance engine stack + UnhandledEngine reject
	// set are per-call while the factory cache stays process-global.
	const ctx = { engines: [], unhandled: null };
	const out = [{ pos: 0, statusIdx: 0 }]; // first() → boundary 0 with status 0
	let pos = 0;
	let statusIdx = 0;

	for (;;) {
		// try dictionary cache first (populateFollowing)
		const d = dict.following(pos);
		if (d !== null) {
			out.push({ pos: d.result, statusIdx: d.statusIndex });
			pos = d.result;
			continue;
		}
		const hn = handleNext(text, pos);
		if (hn.done) break;
		if (hn.dictCount > 0) {
			dict.populate(pos, hn.pos, statusIdx, hn.statusIdx, text, ctx);
			const d2 = dict.following(pos);
			if (d2 !== null) {
				out.push({ pos: d2.result, statusIdx: d2.statusIndex });
				pos = d2.result;
				continue;
			}
		}
		out.push({ pos: hn.pos, statusIdx: hn.statusIdx });
		pos = hn.pos;
		statusIdx = hn.statusIdx;
	}
	return out;
}

// ---------------------------------------------------------------------------
// Public API — mirrors Intl.Segmenter word granularity
// ---------------------------------------------------------------------------
function ruleStatus(statusIdx) {
	const t = getState().statusTable;
	return t[statusIdx + t[statusIdx]];
}
function isWordLikeStatus(status) {
	// V8 CurrentSegmentIsWordLike (js-segments.cc)
	return (
		(status >= 100 && status < 300) ||
		(status >= 200 && status < 500) ||
		(status >= 400 && status < 700) ||
		(status >= 600 && status < 700)
	);
}

export function segmentWords(str) {
	const boundaries = iterateBoundaries(str);
	const out = [];
	for (let i = 1; i < boundaries.length; i++) {
		const start = boundaries[i - 1].pos;
		const { pos: end, statusIdx } = boundaries[i];
		out.push({
			segment: str.slice(start, end),
			isWordLike: isWordLikeStatus(ruleStatus(statusIdx)),
		});
	}
	return out;
}

export function getWordBreakState() {
	return getState();
}
