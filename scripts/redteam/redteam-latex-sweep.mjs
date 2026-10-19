#!/usr/bin/env node
/**
 * R2 redteam driver for tui-latex.
 * - Static diff of every symbol/command table (TS source vs Rust source).
 * - Dynamic sweep: every command through TS (direct import of latex.ts) and
 *   Rust (target/release/pi-cli harness tool tui-latex), canonical-JSON diff.
 */
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const PI_RS = join(dirname(dirname(fileURLToPath(import.meta.url))), "..");
const TS_SRC = process.env.PI_TS_SRC ?? "/tmp/pi-repo/packages/tui/src/latex.ts";
const RS_SRC = PI_RS + "/crates/pi-tui/src/latex.rs";
const WORK = "/tmp/pi-parity/redteam";

const ts = readFileSync(TS_SRC, "utf8");
const rs = readFileSync(RS_SRC, "utf8");

// ---------- static extraction ----------
function tsObj(name) {
  const m = ts.match(new RegExp(`const ${name}[^=]*= \\{([\\s\\S]*?)\\n\\};`));
  if (!m) throw new Error("no TS block " + name);
  return new Function("return ({" + m[1] + "})")();
}
function tsSet(name) {
  const m = ts.match(new RegExp(`const ${name} = new Set\\(\\[([\\s\\S]*?)\\]\\);`));
  if (!m) throw new Error("no TS set " + name);
  return new Set(
    m[1]
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean)
      .map((s) => s.replace(/^"|"$/g, "")),
  );
}
function region(src, startMarker, endMarker) {
  const i = src.indexOf(startMarker);
  if (i < 0) throw new Error("no region start " + startMarker);
  const j = src.indexOf(endMarker, i);
  return src.slice(i, j);
}
function rustPairs(regionText) {
  const out = new Map();
  for (const m of regionText.matchAll(/\("([^"]+)",\s*"((?:[^"\\]|\\.)*)"\)/g)) {
    out.set(m[1], rustUnescape(m[2]));
  }
  return out;
}
function rustUnescape(s) {
  return s.replace(/\\u\{([0-9a-fA-F]+)\}|\\\\|\\n|\\t/g, (all, hex) => {
    if (hex) return String.fromCodePoint(parseInt(hex, 16));
    if (all === "\\\\") return "\\";
    if (all === "\\n") return "\n";
    return "\t";
  });
}
function rustArray(name) {
  const m = rs.match(new RegExp(`const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`));
  if (!m) throw new Error("no Rust array " + name);
  return new Set(
    [...m[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((x) => rustUnescape(x[1])),
  );
}
function rustMatchStr(fnName) {
  const reg = region(rs, `fn ${fnName}(`, "\n}");
  const out = new Map();
  for (const m of reg.matchAll(/"([^"]+)" => "((?:[^"\\]|\\.)*)",/g)) {
    out.set(m[1], rustUnescape(m[2]));
  }
  return out;
}
function rustMatchChar(fnName) {
  const reg = region(rs, `fn ${fnName}(`, "\n}");
  const out = new Map();
  for (const m of reg.matchAll(/'(?:([^'\\])|\\u\{([0-9a-fA-F]+)\})' => '([^'\\]|\\u\{[0-9a-fA-F]+\})',/g)) {
    const key = m[2] ? String.fromCodePoint(parseInt(m[2], 16)) : m[1];
    const val = m[3].startsWith("\\u")
      ? String.fromCodePoint(parseInt(m[3].slice(3, -1), 16))
      : m[3];
    out.set(key, val);
  }
  return out;
}

const staticReport = [];
function diffMaps(label, tsMap, rsMap) {
  const tsk = [...Object.keys(tsMap)].sort();
  const rsk = [...rsMap.keys()].sort();
  const onlyTs = tsk.filter((k) => !rsMap.has(k));
  const onlyRs = rsk.filter((k) => !(k in tsMap));
  const valDiff = tsk.filter((k) => rsMap.has(k) && rsMap.get(k) !== tsMap[k]);
  if (onlyTs.length || onlyRs.length || valDiff.length) {
    staticReport.push({ label, onlyTs, onlyRs, valDiff: valDiff.map((k) => [k, tsMap[k], rsMap.get(k)]) });
  }
}
function diffSets(label, tsSet_, rsSet_) {
  const onlyTs = [...tsSet_].filter((k) => !rsSet_.has(k));
  const onlyRs = [...rsSet_].filter((k) => !tsSet_.has(k));
  if (onlyTs.length || onlyRs.length) staticReport.push({ label, onlyTs, onlyRs });
}

diffMaps("SYMBOLS", tsObj("SYMBOLS"), rustPairs(region(rs, "fn symbols()", ".collect()")));
diffMaps("NEGATED_SYMBOLS", tsObj("NEGATED_SYMBOLS"), rustPairs(region(rs, "fn negated_symbols()", ".collect()")));
diffMaps("ACCENTS", tsObj("ACCENTS"), rustMatchStr("accents"));
diffMaps("BLACKBOARD", tsObj("BLACKBOARD"), rustMatchChar("blackboard"));
diffMaps("SUPERSCRIPTS", tsObj("SUPERSCRIPTS"), rustMatchChar("superscript_char"));
diffMaps("SUBSCRIPTS", tsObj("SUBSCRIPTS"), rustMatchChar("subscript_char"));
for (const n of ["NAMED_OPERATORS", "LIMIT_OPERATORS", "DISPLAY_LIMIT_SYMBOLS", "RELATION_COMMANDS", "SPACING_COMMANDS", "NEGATIVE_SPACING_COMMANDS", "FONT_SWITCH_COMMANDS", "IGNORED_COMMANDS", "SIZE_COMMANDS", "PLAIN_WRAPPERS"]) {
  diffSets(n, tsSet(n), rustArray(n));
}

// ---------- dynamic runner ----------
const lx = await import(pathToFileURL(TS_SRC).href);
function tsRender(source, display) {
  return lx.renderLatex(source, display ? { display: true } : {}) ?? null;
}
function rsRender(source, display) {
  const input = JSON.stringify({ fn: "render", source, display: !!display });
  const out = execFileSync(PI_RS + "/target/release/pi-cli", ["harness", "tool", "tui-latex", "--cwd", WORK], {
    input, cwd: PI_RS, encoding: "utf8", maxBuffer: 1e8,
  });
  return JSON.parse(out).result ?? null;
}
function canon(v) {
  return JSON.stringify(v, (_k, x) => (x && typeof x === "object" && !Array.isArray(x) ? Object.keys(x).sort().reduce((a, kk) => ((a[kk] = x[kk]), a), {}) : x));
}
const mismatches = [];
let total = 0;
function cmp(source, display, tag) {
  total++;
  const t = tsRender(source, display);
  const r = rsRender(source, display);
  if (canon(t) !== canon(r)) mismatches.push({ tag, source, display, ts: t, rs: r });
}

// symbol sweep (all SYMBOLS keys, bare + in relation context + display for limit symbols)
const SYM = tsObj("SYMBOLS");
const DLS = tsSet("DISPLAY_LIMIT_SYMBOLS");
for (const k of Object.keys(SYM)) {
  cmp("\\" + k, false, "symbol:" + k);
  cmp("a \\" + k + " b", false, "symbol-ctx:" + k);
  if (DLS.has(k)) cmp("\\" + k + "_i^n x", true, "symbol-dsp:" + k);
}
for (const k of tsSet("NAMED_OPERATORS")) cmp("\\" + k + " x", false, "namedop:" + k);
for (const k of tsSet("LIMIT_OPERATORS")) cmp("\\" + k + "_x y", true, "limitop:" + k);
for (const k of tsSet("SPACING_COMMANDS")) cmp("a \\" + k + " b", false, "spacing:" + JSON.stringify(k));
for (const k of tsSet("NEGATIVE_SPACING_COMMANDS")) cmp("a \\" + k + " b", false, "negspacing:" + k);
for (const k of tsSet("FONT_SWITCH_COMMANDS")) cmp("\\" + k + " x", false, "fontswitch:" + k);
for (const k of tsSet("IGNORED_COMMANDS")) cmp("x \\" + k + " y", false, "ignored:" + k);
for (const k of tsSet("SIZE_COMMANDS")) cmp("\\" + k + "( x )", false, "size:" + k);
for (const k of tsSet("PLAIN_WRAPPERS")) cmp("\\" + k + "{X y}", false, "wrapper:" + k);
for (const [k] of Object.entries(tsObj("NEGATED_SYMBOLS"))) cmp("\\not" + k, false, "negated:" + k);
for (const [k] of Object.entries(tsObj("ACCENTS"))) cmp("\\" + k + "{x}", false, "accent:" + k);
for (const [k] of Object.entries(tsObj("BLACKBOARD"))) cmp("\\mathbb{" + k + "}", false, "bb:" + k);
for (const [k] of Object.entries(tsObj("SUPERSCRIPTS"))) cmp("x^{" + k + "}", false, "sup:" + k);
for (const [k] of Object.entries(tsObj("SUBSCRIPTS"))) cmp("x_{" + k + "}", false, "sub:" + k);

// ---------- structural / edge battery ----------
const cases = [
  ["", false], ["", true], [" ", false], ["   \n\t ", false],
  ["{", false], ["}", false], ["{a", false], ["a}", false], ["{{a}}", false], ["{a{b}c}", false],
  ["\\", false], ["\\\\", false], ["\\\\\\", false], ["a\\\\b\\\\c", false],
  ["x^{", false], ["x_}", false], ["x^^2", false], ["x__2", false], ["x^", false], ["x_", false],
  ["x^{}", false], ["x_{}", false], ["x^_2", false], ["x_^2", false], ["_2 x", false], ["^2 x", false],
  ["x^{a b}", false], ["x^{ a }", false], ["x^2^3", false],
  ["\\frac{}{}", false], ["\\frac{a}{}", false], ["\\frac{}{b}", false], ["\\frac{a}", false],
  ["\\frac {a} {b}", false], ["\\frac{a}{b}{c}", false],
  ["\\frac{\\frac{a}{b}}{c}", false], ["\\frac{\\frac{a}{b}}{c}", true],
  ["\\frac{a+b}{c}", true], ["\\frac12", false], ["\\frac1x", false],
  ["\\dfrac{a}{b}", false], ["\\tfrac{a}{b}", true],
  ["\\sqrt", false], ["\\sqrt{}", false], ["\\sqrt[]{x}", false], ["\\sqrt[ ]{x}", false],
  ["\\sqrt[2]{x}", false], ["\\sqrt[10]{x}", false], ["\\sqrt[n+1]{x}", false],
  ["\\sqrt[3]{x+1}", true], ["\\sqrt[n]{x}", true],
  ["\\sum_{i=1}^{n} i", true], ["\\sum_{i=1}^{n} i", false],
  ["\\int_a^b f", true], ["\\int_{a}{b} f", true],
  ["x^{ab}", true], ["x^{AB}", true], ["x_{ab}", true], ["x_{AB}", true],
  ["x^{*}", true], ["x^{∗}", true], ["x^{a*}", true],
  ["x^{2}+x_{10}", true], ["x^{n+1}", true], ["x_{i,j}", true],
  ["\\lim_{x \\to 0} f", true], ["\\lim_{x \\to 0} f", false],
  ["\\max_{x} f", false], ["\\min_x y", true],
  ["\\operatorname{sgn}_{i}", true], ["\\operatorname*{sgn}_{i}", true],
  ["\\left( x \\right)", false], ["\\left. x \\right)", false], ["\\left(x\\right)^2", true],
  ["\\left( \\frac{a}{b} \\right)", true],
  ["a & b", false], ["a ~ b", false], ["\\{a\\}", false], ["\\|x\\|", false],
  ["\\%", false], ["\\$", false], ["\\#", false], ["\\_", false], ["\\&", false],
  ["a = b < c > d", false], ["a<=b", false], ["a >= b", false],
  ["x''", false], ["x'''", true], ["x^{\\prime}", false],
  ["\\overline{x}", false], ["\\overline{xy}", false], ["\\underline{x+1}", false],
  ["\\hat{x}\\vec{y}\\bar{z}", true],
  ["\\mathbb{R}^n", true], ["\\mathbb{a}", false], ["\\mathbb{}", false],
  ["\\mathcal{F}", false], ["\\mathbf{v}", false], ["\\mathfrak{g}", false],
  ["\\text{hello world}", false], ["\\text{}", false], ["\\text{a b}", true],
  ["\\text{if } x", false],
  ["\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}", true],
  ["\\begin{pmatrix} 1 & 2 & 3 \\\\ 4 & 5 \end{pmatrix}", false],
  ["\\begin{matrix} 1 \\\\ 2 \\\\ 3 \\end{matrix}", false],
  ["\\begin{matrix} 1 \\end{matrix}", false],
  ["\\begin{pmatrix} \\end{pmatrix}", false],
  ["\\begin{bmatrix} \\frac{a}{b} & 2 \\end{bmatrix}", true],
  ["\\begin{vmatrix} a & b \\\\ c & d \\end{vmatrix}", false],
  ["\\begin{Vmatrix} a \\end{Vmatrix}", false],
  ["\\begin{Bmatrix} a \\\\ b \\end{Bmatrix}", false],
  ["\\begin{array}{cc} 1 & 2 \\\\ 3 & 4 \\end{array}", false],
  ["\\begin{array} 1 & 2 \\end{array}", false],
  ["\\begin{smallmatrix} 1 \\\\ 2 \\end{smallmatrix}", false],
  ["\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}.", false],
  ["\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix} .", false],
  ["\\begin{cases} x & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}", false],
  ["\\begin{cases} a \\\\ b \\\\ c \\end{cases}", false],
  ["\\begin{cases} a \\\\ b \\\\ c \\\\ d \\end{cases}", false],
  ["\\begin{cases} a \\\\ b \\\\ c \\\\ d \\\\ e \\end{cases}", false],
  ["\\begin{cases} x, & \\text{if } x \\\\ y, & \\text{IF } y \\end{cases}", false],
  ["\\begin{cases} x, & \\text{iff } x \\end{cases}", false],
  ["\\begin{cases} x & \\text{for} y \\\\ z & \\text{whenever} w \\end{cases}", false],
  ["\\begin{aligned} a &= 1 \\\\ b &= 2 \\end{aligned}", true],
  ["\\begin{align} a &= 1 \\\\ b &= 2 \\end{align}", true],
  ["\\begin{alignedat}{2} a &=1 & b &=2 \\end{alignedat}", false],
  ["\\begin{alignat}{2} a &=1 & b &=2 \\end{alignat}", false],
  ["\\begin{gather} a \\\\ b \\end{gather}", true],
  ["\\begin{equation} x = 1 \\end{equation}", true],
  ["\\begin{equation*} \\frac{a}{b} \\end{equation*}", true],
  ["\\begin{unknown} x \\end{unknown}", false],
  ["\\begin{pmatrix} x \\end{pmatrix} \\begin{cases} a \\\\ b \\end{cases}", false],
  ["\\not", false], ["\\not{}", false], ["\\not{ab}", false], ["\\not\\alpha", false],
  ["\\not\\exists", false], ["\\not\\infty", false], ["\\not +", false],
  ["\\mod{3}", false], ["\\pmod{3}", false], ["\\pod{3}", false], ["a \\bmod b", false],
  ["\\boxed{x}", false], ["\\fbox{y}", false], ["\\binom{n}{k}", false], ["\\tbinom{n}{k}", false],
  ["\\overset{a}{b}", false], ["\\overset{\\text{def}}{=}", false],
  ["\\underset{a}{b}", false], ["\\stackrel{a}{b}", true],
  ["\\substack{a \\\\ b}", true], ["\\substack{a \\\\ b}", false],
  ["\\smash{x}", false], ["\\pmb{x}", false], ["\\bm{x}", false], ["\\boldsymbol{x}", false],
  ["\\emph{x}", false], ["\\mbox{a b}", false], ["\\textrm{a b}", false],
  ["\\unknowncmd", false], ["\\unknowncmd{x}", false], ["\\Alpha", false], ["\\Beta", false],
  ["\\alpha\\", false], ["\\alpha{x}", false],
  ["x\\!y", false], ["a\\,b", false], ["a\\;b", false], ["a\\:b", false], ["a\\ b", false],
  ["a\\quad b", false], ["a\\qquad b", false], ["a\\enspace b", false],
  ["\\alpha \\! \\beta", false], ["\\sin \\! x", false],
  ["\\lim\\limits_{x} f", false], ["\\sum\\nolimits_{i} x", true],
  ["\\int\\limits_0^1", true], ["\\int\\nolimits_0^1", true],
  ["a\\lim b", false], ["\\sin_2 x", false],
  ["\\text{a😀b}", false], ["x^😀", false], ["x_{😀}", false],
  ["\\text{中文}", false], ["\\frac{中}{文}", false], ["x^{中文}", false],
  ["\\hat{😀}", false], ["\\mathbb{😀}", false],
  ["x^{é}", false], ["\\frac{é}{è}", false],
  ["a\u00A0b", false], ["a\u2028b", false], ["a\u0085b", false], ["a\uFEFFb", false],
  ["a\u3000b", false],
  ["\\alpha\u00A0\\beta", false],
  ["\\text{a\u00A0b}", false],
  ["0.5 + 1.5", false], ["12345", false], ["x_1x_2", false],
  ["f''(x)", false], ["\\sum_{k=0}^{n} \\binom{n}{k}", true],
  ["\\frac{d}{dx}\\int_{a}^{b}", true],
  ["\\sqrt{\\frac{a}{b}}", true], ["\\sqrt[3]{\\frac{a}{b}}", false],
  ["\\overset{abc}{=}", true], ["\\overset{a}{b}", true],
  ["\\begin{cases} \\frac{a}{b} & c \\\\ d & e \\end{cases}", true],
  ["\\operatorname{arg\\,max}", false],
  ["\\text{ x }", false], ["\\text{x}", true],
  ["\\sin x \\cos y \\tan z", false],
  ["\\begin{pmatrix} a \\\\ \\end{pmatrix}", false],
  ["\\begin{cases} \\\\ a \\end{cases}", false],
  ["\\left\\| x \\right\\|", false],
  ["\\mathrm{d}x", false], ["\\mathit{x}", false], ["\\mathsf{x}", false], ["\\mathtt{x}", false],
  ["\\varphi + \\phi", false], ["\\varepsilon + \\epsilon", false],
  ["\\varsigma + \\sigma", false], ["\\vartheta + \\theta", false],
  ["\\varrho", false], ["\\varkappa", false], ["\\varpi", false], ["\\varnothing", false],
];
for (const [src, disp] of cases) cmp(src, disp, "case");

// ---------- report ----------
const report = { staticReport, total, mismatchCount: mismatches.length, mismatches };
writeFileSync(WORK + "/r2-results.json", JSON.stringify(report, null, 1));
console.log("static findings:", JSON.stringify(staticReport));
console.log("dynamic cases:", total, "mismatches:", mismatches.length);
for (const m of mismatches) {
  console.log("---", m.tag, "display=" + m.display, "src=" + JSON.stringify(m.source));
  console.log("  TS:", JSON.stringify(m.ts));
  console.log("  RS:", JSON.stringify(m.rs));
}
