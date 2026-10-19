#!/usr/bin/env node
/** R2 battery 2: deeper layout/display/width/JS-semantics cases. */
import { execFileSync } from "node:child_process";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const PI_RS = join(dirname(dirname(fileURLToPath(import.meta.url))), "..");
const WORK = "/tmp/pi-parity/redteam";
const lx = await import(pathToFileURL(process.env.PI_TS_SRC ?? "/tmp/pi-repo/packages/tui/src/latex.ts").href);

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

const cases = [
  // nested script layout (scriptDepth>0 in display)
  ["x^{y^z}", true], ["x^{y^z}", false], ["x_{y_z}", true], ["x^{y^2}", true],
  ["x^{y^{z^w}}", true], ["x_{i}^{j}", true], ["x^{ab}_{cd}", true],
  ["x^\\frac{a}{b}", true], ["x^{\\frac{a}{b}}", true], ["x^{\\frac{a}{b}}", false],
  ["x^{\\text{a b}}", true], ["x^{\\text{a b}}", false],
  ["\\sum_{\\frac{a}{b}}^{n}", true], ["\\lim_{x^{2}} f", true], ["\\lim_{x^{2}} f", false],
  ["\\sum^n_i x", true], ["\\int^1_0 f", true],
  ["\\sum_{\\substack{a \\\\ b}} x", true],
  // width-sensitive layout content
  ["\\begin{pmatrix} 中 & 文 \\\\ 字 & 节 \\end{pmatrix}", true],
  ["\\begin{pmatrix} 中 & 文 \\\\ 字 & 节 \\end{pmatrix}", false],
  ["\\begin{cases} 中 & x \\\\ 文 & y \\end{cases}", true],
  ["\\frac{中}{文}", true], ["\\frac{中}{文}", false],
  ["\\frac{😀}{😂}", true], ["\\sqrt{😀}", false], ["\\sqrt{中}", false],
  ["x^{中文}", true], ["x_{中文}", true],
  ["a\u0301b", false], ["\\frac{a\u0301}{b}", true], ["\\hat{a\u0301}", false],
  ["\\begin{pmatrix} a & b\\\\c & d\\end{pmatrix}", false],
  ["\\begin{Bmatrix} \\frac{a}{b} \\\\ c \\end{Bmatrix}", true],
  ["\\begin{cases} a & b \\\\ c & d \\\\ e & f \\\\ g & h \\\\ i & j \\\\ k & l \\end{cases}", false],
  // JS whitespace class
  ["a\u000Bb", false], ["a\u000Cb", false], ["a\rb", false], ["a\r\nb", false],
  ["a\n\nb", false], ["a\tb", false], ["a\n \t b", false],
  ["\\alpha\n\\beta", false], ["x\n^2", false],
  ["\\begin{matrix} a \\\\[1em] b \\end{matrix}", false],
  ["\\begin{matrix} a \\\\[x\ny] b \\end{matrix}", false],
  // named operator spacing markers
  ["(\\sin x)", false], ["x\\sin(y)", false], ["\\sin2x", false], ["2\\sin x", false],
  ["\\sin\\sin x", false], ["\\sin\\!x", false], ["\\cos^2 x", false],
  ["\\log_2 16", false], ["\\exp_{x}", true],
  // size/delimiter commands
  ["\\bigl(x\\bigr)", false], ["\\Big(x\\Big)", false], ["\\left| x \\right|", false],
  ["\\bigg\\{ x \\bigg\\}", false],
  // sqrt optional arg variants
  ["\\sqrt[ 3]{x}", false], ["\\sqrt[3 ]{x}", false], ["\\sqrt[\\alpha]{x}", false],
  ["\\sqrt[\\frac{a}{b}]{x}", false], ["\\sqrt[3]{\\frac{a}{b}}", true],
  ["\\sqrt[10]{x+1}", false], ["\\sqrt[2+2]{x}", false],
  // script squeeze regex
  ["x^{ = }", false], ["x^{ - 1 }", false], ["x^{a + b}", true], ["x^{a + b}", false],
  ["x_{ - 1 }", false], ["x^{+}", false], ["x^{=}", false],
  ["x^{a\u00A0=\u00A0b}", false], ["x^{a\u2028b}", false],
  // under/overbrace + scripts in display
  ["\\underbrace{x}_{y}", true], ["\\overbrace{x}^{y}", true],
  ["\\underbrace{xy}_{ab}", true], ["\\underbrace{x}_{ab}", true],
  // operatorname / pmod / overset nesting
  ["\\operatorname{a\\,b}", false], ["\\operatorname{\\alpha}", false],
  ["\\operatorname{arg max}_{x}", true],
  ["\\pmod{\\frac{a}{b}}", false], ["\\overset{a}{bc}", false], ["\\overset{ab}{c}", true],
  ["\\underset{ab}{c}", true], ["\\stackrel{\\text{def}}{=}", true],
  // not fallbacks
  ["\\not{\\alpha}", false], ["\\not{ab}", false], ["\\not{\\alpha\\beta}", false],
  ["\\not\\ge", false], ["\\not\\le", false], ["\\not\\sim", false], ["\\not\\cong", false],
  ["\\not\\asymp", false], ["\\not\\equiv", false], ["\\not\\prec", false], ["\\not\\preceq", false],
  // marker-adjacent text spacing
  ["\\begin{cases} a & b \\end{cases} c \\begin{pmatrix} d \\\\ e \\end{pmatrix}", false],
  ["\\begin{pmatrix} a \\\\ b \\end{pmatrix} \\\\ c", false],
  ["\\begin{pmatrix} a \\\\ b \\end{pmatrix}x", false],
  ["x\\begin{pmatrix} a \\\\ b \\end{pmatrix}", false],
  ["\\begin{aligned} \\frac{a}{b} \\end{aligned}", true],
  ["\\begin{align} \\frac{a}{b} &= \\frac{c}{d} \\end{align}", true],
  ["\\begin{gathered} \\frac{a}{b} \\\\ \\frac{c}{d} \\end{gathered}", true],
  // display fraction joins
  ["x = \\frac{a}{b} + y", true], ["\\frac{a}{b}\\frac{c}{d}", true],
  ["\\frac{a}{b} + \\frac{c}{d}", true], ["\\frac{aa}{bbb}", true],
  ["\\frac{a+b}{c+d+e}", true], ["\\frac{}{}", true],
  ["\\lim_{x \\to 0} \\frac{1}{x}", true],
  ["\\frac{1}{x} + \\lim_{x \\to 0} y", true],
  ["\\frac{a}{b} \\\\ \\frac{c}{d}", true],
  ["\\frac{\\sum_{i}^{n} x}{y}", true],
  ["\\frac{x^{ab}}{y}", true],
  // misc
  ["\\begin{} x \\end{}", false],
  ["x\\limits^2", false], ["\\displaystyle\\frac{a}{b}", true],
  ["{\\textstyle x}", false],
  ["\\text{ }", false], ["\\text{\t}", false],
  ["\\mathrm{d}x", true],
  ["a\\alpha\\!\\beta", false],
  ["\\left(\\frac{a}{b}\\right)^2", true],
  ["\\prod_{i=1}^{n} i^2", true],
  ["\\bigcap_{i} X", true], ["\\bigcup_{i} X", true], ["\\oint_C F", true],
  ["\\iiint_{V} f", true],
  ["\\liminf_{n} x", true], ["\\argmax_{x} f", true], ["\\argmin_{x} f", false],
  ["\\injlim", false], ["\\projlim_{n} x", true],
  ["x^{†}", false], ["\\dagger\\ddagger", false],
  ["\\begin{array}{ccc} 1 & 2 & 3 \\\\ 4 & 5 & 6 \\\\ 7 & 8 & 9 \\end{array}", false],
  ["\\begin{array}{c|c} 1 & 2 \\end{array}", false],
  ["\\begin{matrix} 1 & 2 & 3 \\\\ 4 \\end{matrix}", false],
  ["\\sqrt{\\sqrt{x}}", false],
  ["\\frac{\\sqrt{x}}{\\sqrt{y}}", true],
  ["\\hat{\\alpha}", false], ["\\vec{\\alpha}", true], ["\\widetilde{xy}", false],
  ["\\overline{\\frac{a}{b}}", true],
  ["f'(x) + f''(x)", true],
  ["\\sum_{k=0}^{n} \\binom{n}{k} = 2^n", true],
  ["e^{i\\pi} + 1 = 0", true],
  ["\\int_{-\\infty}^{\\infty} e^{-x^2} dx", true],
];

let mismatches = 0;
for (const [src, disp] of cases) {
  const t = tsRender(src, disp);
  const r = rsRender(src, disp);
  if (JSON.stringify(t) !== JSON.stringify(r)) {
    mismatches++;
    console.log("--- src=" + JSON.stringify(src) + " display=" + disp);
    console.log("  TS:", JSON.stringify(t));
    console.log("  RS:", JSON.stringify(r));
  }
}
console.log("battery2 cases:", cases.length, "mismatches:", mismatches);
