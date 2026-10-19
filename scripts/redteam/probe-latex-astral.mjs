#!/usr/bin/env node
/** Astral/surrogate differential battery for tui-latex (TS vs RS). */
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const PI_RS = join(dirname(dirname(fileURLToPath(import.meta.url))), "..");
const WORK = "/tmp/pi-parity/redteam";

function tsHarness(source, display) {
  const out = execFileSync("node", ["parity/ts-harness.js", WORK], {
    input: JSON.stringify({ tool: "tui-latex", args: { fn: "render", source, display } }),
    cwd: PI_RS, encoding: "utf8", maxBuffer: 1e8 });
  return JSON.parse(out);
}
function rsHarness(source, display) {
  const out = execFileSync(PI_RS + "/target/release/pi-cli", ["harness", "tool", "tui-latex", "--cwd", WORK], {
    input: JSON.stringify({ fn: "render", source, display }),
    cwd: PI_RS, encoding: "utf8", maxBuffer: 1e8 });
  return JSON.parse(out);
}

const E = "\u{1F600}", F = "\u{1F602}", LB = "\u{10000}"; // emoji, emoji2, linear B
const cases = [
  ["x^" + E, true],
  ["x^" + E, false],
  ["\\sqrt" + E, false],
  ["\\underline" + E, false],
  ["\\frac" + E + "+", false],
  ["\\vec" + E, false],
  ["\\hat" + E, false],
  ["\\hat{" + E + "}", false],
  ["\\not{" + E + "}", false],
  ["\\mathbb{" + E + "}", false],
  ["\\sqrt[" + E + "]{x}", false],
  ["\\sqrt[x^" + E + "]{y}", false],
  ["\\begin{pmatrix} x^" + E + " \\\\ b \\end{pmatrix}", true],
  ["\\frac" + E + F, false],
  ["x^" + E + "_" + F, false],
  ["x^" + E + "_" + F, true],
  ["\\text{x^" + E + "}", false],
  ["a" + E + "b", false],
  ["\\sqrt{" + LB + "}", false],
  ["\\begin{cases} x^" + E + " & c \\end{cases}", true],
  ["\\sum^" + E, true],
  ["x^{" + E + "}", false],
  ["\\overline" + E, false],
  ["\\pmod" + E, false],
  ["\\left" + E, false],
  ["\\frac" + E + "{+}", true],
  ["\\sqrt[3]{" + E + "}", false],
  ["\\lim^" + E + " x", true],
  ["\\" + E, false],
  ["\\bar" + E + E, false],
  ["x_" + E, false],
  ["x^^" + E, false],
];

let bad = 0;
for (const [src, disp] of cases) {
  const t = tsHarness(src, disp);
  const r = rsHarness(src, disp);
  const same = JSON.stringify(t) === JSON.stringify(r);
  if (!same) bad++;
  console.log((same ? "ok  " : "MISMATCH ") + "src=" + JSON.stringify(src) + " display=" + disp);
  if (!same) {
    console.log("  TS:", JSON.stringify(t));
    console.log("  RS:", JSON.stringify(r));
  }
}
console.log(bad === 0 ? "ALL MATCH" : bad + " mismatches");
process.exit(bad === 0 ? 0 : 1);
