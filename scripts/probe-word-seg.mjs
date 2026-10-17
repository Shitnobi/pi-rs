// Validation probe: scripts/lib/word-segmenter.mjs segmentWords() vs live
// Intl.Segmenter (word granularity, default locale) on fixed + fuzzed inputs.
// Usage: node scripts/probe-word-seg.mjs [--fuzz N] [--seed S] [-v]

import { segmentWords } from "./lib/word-segmenter.mjs";

const refSeg = new Intl.Segmenter(undefined, { granularity: "word" });

function reference(str) {
	const out = [];
	for (const s of refSeg.segment(str)) {
		out.push({ segment: s.segment, isWordLike: s.isWordLike });
	}
	return out;
}

let verbose = false;
let fuzzCount = 20000;
let seed = 1234;
let emitFixture = null;
const args = process.argv.slice(2);
for (let i = 0; i < args.length; i++) {
	if (args[i] === "-v") verbose = true;
	else if (args[i] === "--fuzz") fuzzCount = parseInt(args[++i], 10);
	else if (args[i] === "--seed") seed = parseInt(args[++i], 10);
	else if (args[i] === "--emit-fixture") emitFixture = args[++i];
}

// ---------------------------------------------------------------------------
// deterministic PRNG (mulberry32)
// ---------------------------------------------------------------------------
function mulberry32(a) {
	return function () {
		a |= 0;
		a = (a + 0x6d2b79f5) | 0;
		let t = Math.imul(a ^ (a >>> 15), 1 | a);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

// ---------------------------------------------------------------------------
// fixed test cases
// ---------------------------------------------------------------------------
const fixedCases = [
	"",
	" ",
	"a",
	"ab",
	"hello world",
	"  leading and trailing   ",
	"foo.bar",
	"foo...bar",
	"foo: bar",
	"path/to/file",
	"/path/to/file.txt",
	"don't stop",
	"it's 3:14pm, ok?",
	"(parens) [brackets] {braces} <angles>",
	"num 123 45.67 -8 +9 1e10 0x1F",
	"a-b_c d.e f,g",
	"$100 @user #tag % pct ^ & * ( ) _ +",
	"mixedCASE MiXeD",
	"ALLCAPS lowercase",
	"\t\ttabs\nnewlines\r\r\n",
	"nbsp and en–dash—em",
	"…ellipsis …",
	"«guillemets»",
	"\u00A0\u2000\u2001\u2028\u2029",
	// CJK
	"你好世界",
	"你好世界 foo bar",
	"漢字かな交じり文",
	"日本語のテキストです",
	"カタカナ ひらがな 漢字",
	"ァィゥェォ ガギグゲゴ パピプペポ",
	"ーーー長音符ーー",
	"ｱｲｳｴｵ ﾊﾞｲﾄ ﾊﾟｿｺﾝ", // halfwidth katakana incl. ﾞ (Extend)
	"〆切 々惜 〻", // Han-ish radicals/symbols
	"⺀⺁⺂ radicals",
	"韓国語 한글 텍스트",
	"한국어와 English 혼용",
	// Thai
	"สวัสดีครับ",
	"ทดสอบการตัดคำภาษาไทย",
	"กรุงเทพมหานคร",
	"ภาษาไทยเป็นภาษาที่ไม่มีช่องว่างระหว่างคำ",
	"ไม่มีข้อมูล ๆ ฯลฯ",
	"ครับๆ ไปๆมาๆ",
	"โปรแกรมคอมพิวเตอร์",
	// Lao
	"ສະບາຍດີ",
	"ພາສາລາວບໍ່ມີຊ່ອງຫວ່າງ",
	"ວຽງຈັນ",
	// Khmer
	"សួស្តី",
	"ភាសាខ្មែរ",
	"ក្រុងភ្នំពេញ",
	"ការប្រើប្រាស់ពាក្យ",
	// Burmese
	"မင်္ဂလာပါ",
	"မြန်မာဘာသာစကား",
	"ရန်ကုန်မြို့",
	// mixed dictionary + non-dictionary
	"ไทย English 漢字",
	"กรุงเทพ Bangkok 東京",
	"Thai123 ไทย123",
	"漢字123 かな456",
	// emoji
	"emoji 👍 test",
	"👨‍👩‍👧‍👦 family ZWJ",
	"👍🏽 skin tone",
	"🇺🇸 flag",
	"1️⃣2️⃣3️⃣ keycaps",
	"☺️ vs ☺",
	// supplementary planes
	"𐌰𐌱𐌲 Gothic",
	"𝕳𝖊𝖑𝖑𝖔 math",
	"𓀀𓀁 Egyptian",
	"𠮷野家",
	"𩸽 ほっけ",
	// lone/paired surrogates (JS strings can hold them)
	"\uD800",
	"\uDC00",
	"\uD800\uDC00",
	"abc\uD800def",
	"a\uDC00b",
	"\uD83D\uDE00 smile surrogate pair",
	// combining marks
	"e\u0301 é e\u0301\u0302",
	"a\u0300\u0301b",
	"ก\u0E31 combining Thai",
	"กำ กึ กี",
	"각 jamo",
	// ZWJ / format chars
	"a\u200Db",
	"\u200B\u200C\u200D zero width",
	"‍‍",
	// mixed whitespace/punct with dictionary scripts
	"สวัสดี, ครับ!",
	"(你好)",
	"「日本語」",
	"『カタカナ』",
	"한국,어.",
	// long runs
	"a".repeat(100),
	"你".repeat(100),
	"สวัสดี".repeat(50),
	"ア".repeat(50),
	"한".repeat(50),
	"x".repeat(1000) + " " + "y".repeat(1000),
	// numbers + CJK
	"令和3年5月15日",
	"第1234章",
	// punctuation soup
	"!!!???...,,,;;;:::",
	"---===+++***###",
	"''\"\"``~~",
	"(){}[]<><<<>>>",
];

// ---------------------------------------------------------------------------
// fuzz pools — ranges biased toward interesting scripts
// ---------------------------------------------------------------------------
const pools = [
	{ ranges: [[0x20, 0x7e]], w: 6 }, // ASCII
	{ ranges: [[0x21, 0x2f], [0x3a, 0x40], [0x5b, 0x60], [0x7b, 0x7e]], w: 3 }, // punct
	{ ranges: [[0x09, 0x0d], [0x20], [0xa0], [0x2000, 0x200a], [0x2028, 0x2029], [0x3000]], w: 2 }, // ws
	{ ranges: [[0x30, 0x39]], w: 2 }, // digits
	{ ranges: [[0xc0, 0x24f]], w: 2 }, // Latin ext
	{ ranges: [[0x370, 0x3ff], [0x400, 0x4ff]], w: 2 }, // Greek/Cyrillic
	{ ranges: [[0x590, 0x5ff], [0x600, 0x6ff]], w: 2 }, // Hebrew/Arabic
	{ ranges: [[0x900, 0x97f]], w: 2 }, // Devanagari
	{ ranges: [[0xe00, 0xe5b]], w: 8 }, // Thai
	{ ranges: [[0xe80, 0xedd]], w: 6 }, // Lao
	{ ranges: [[0x1780, 0x17f0]], w: 6 }, // Khmer
	{ ranges: [[0x1000, 0x103f], [0x1040, 0x109f]], w: 6 }, // Burmese
	{ ranges: [[0x1100, 0x11ff]], w: 2 }, // Hangul jamo
	{ ranges: [[0xac00, 0xd7a3]], w: 5 }, // Hangul syllables
	{ ranges: [[0x3040, 0x30ff]], w: 5 }, // Hiragana/Katakana
	{ ranges: [[0x31f0, 0x31ff], [0xff66, 0xff9f]], w: 3 }, // katakana ext/halfwidth
	{ ranges: [[0x4e00, 0x9fff]], w: 8 }, // CJK Unified
	{ ranges: [[0x3400, 0x4dbf]], w: 2 }, // Ext A
	{ ranges: [[0xf900, 0xfaff], [0x2e80, 0x2eff], [0x2f00, 0x2fdf]], w: 2 }, // compat/radicals
	{ ranges: [[0x3000, 0x303f], [0xff01, 0xff60]], w: 2 }, // CJK punct
	{ ranges: [[0x2000, 0x206f]], w: 2 }, // gen punct (ZWJ etc.)
	{ ranges: [[0x0300, 0x036f]], w: 2 }, // combining
	{ ranges: [[0x1f300, 0x1f5ff], [0x1f600, 0x1f64f], [0x1f680, 0x1f6ff]], w: 3 }, // emoji
	{ ranges: [[0x2600, 0x27bf]], w: 2 }, // misc symbols
	{ ranges: [[0x10000, 0x1007f], [0x10300, 0x1032f], [0x13000, 0x1342f]], w: 1 }, // ancient
	{ ranges: [[0x20000, 0x200ff], [0x2a700, 0x2a7ff]], w: 2 }, // Ext B
	{ ranges: [[0x11700, 0x1174f]], w: 1 }, // Ahom (non-dict SA)
	{ ranges: [[0x1a00, 0x1a1f]], w: 1 }, // Tai Le (cat 14)
	{ ranges: [[0xd800, 0xdfff]], w: 1 }, // surrogates (lone)
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
	const r = p.ranges[rand() * p.ranges.length | 0];
	return r[0] + ((rand() * (r[1] - r[0] + 1)) | 0);
}
function fuzzString(rand) {
	const len = 1 + ((rand() * 24) | 0);
	let s = "";
	for (let i = 0; i < len; i++) {
		s += String.fromCodePoint(pickChar(rand));
	}
	return s;
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------
function compare(str) {
	const a = segmentWords(str);
	const b = reference(str);
	if (a.length !== b.length) return { kind: "length", a, b };
	for (let i = 0; i < a.length; i++) {
		if (a[i].segment !== b[i].segment || a[i].isWordLike !== b[i].isWordLike) {
			return { kind: "segment", index: i, a, b };
		}
	}
	return null;
}

function showStr(s) {
	return JSON.stringify(s.length > 80 ? s.slice(0, 80) + `…(${s.length})` : s);
}
function showSegs(arr, n = 12) {
	return JSON.stringify(arr.slice(0, n).map((x) => [x.segment, x.isWordLike]));
}

let pass = 0;
let fail = 0;
const failures = [];
const checked = [];

function check(str, label) {
	const bad = compare(str);
	if (!bad) {
		pass++;
		checked.push(str);
		if (verbose) console.log(`PASS ${label}: ${showStr(str)}`);
	} else {
		fail++;
		failures.push({ label, str, bad });
		console.log(`FAIL ${label} (${bad.kind}): ${showStr(str)}`);
		console.log(`  proto: ${showSegs(bad.a)}`);
		console.log(`   live: ${showSegs(bad.b)}`);
	}
}

for (const s of fixedCases) check(s, "fixed");

if (fuzzCount > 0) {
	const rand = mulberry32(seed);
	for (let i = 0; i < fuzzCount; i++) {
		check(fuzzString(rand), `fuzz#${i}`);
		if (fail >= 20) {
			console.log("stopping after 20 failures");
			break;
		}
	}
}

console.log(`\n${pass} pass, ${fail} fail`);
if (emitFixture) {
	if (fail !== 0) {
		console.error("refusing to emit fixture: probe has failures");
		process.exit(1);
	}
	const { writeFileSync } = await import("node:fs");
	const seg = segmentWords;
	const bufs = [];
	const u32 = (v) => {
		const b = Buffer.alloc(4);
		b.writeUInt32LE(v >>> 0);
		return b;
	};
	const u16 = (v) => {
		const b = Buffer.alloc(2);
		b.writeUInt16LE(v & 0xffff);
		return b;
	};
	bufs.push(u32(checked.length));
	for (const str of checked) {
		bufs.push(u32(str.length));
		for (let i = 0; i < str.length; i++) bufs.push(u16(str.charCodeAt(i)));
		const segs = seg(str);
		bufs.push(u32(segs.length));
		for (const s of segs) {
			bufs.push(u32(s.segment.length));
			for (let i = 0; i < s.segment.length; i++) bufs.push(u16(s.segment.charCodeAt(i)));
			bufs.push(Buffer.from([s.isWordLike ? 1 : 0]));
		}
	}
	writeFileSync(emitFixture, Buffer.concat(bufs));
	console.log(`wrote fixture: ${emitFixture} (${checked.length} cases)`);
}
if (failures.length > 0) {
	// dump failing inputs (code points) for offline debugging
	const { writeFileSync } = await import("node:fs");
	writeFileSync(
		"/tmp/ws-failures.json",
		JSON.stringify(
			failures.map((f) => ({
				label: f.label,
				cps: [...f.str].map((ch) => ch.codePointAt(0)),
				proto: f.bad.a.map((x) => [x.segment, x.isWordLike]),
				live: f.bad.b.map((x) => [x.segment, x.isWordLike]),
			})),
			null,
			1,
		),
	);
}
process.exit(fail === 0 ? 0 : 1);
