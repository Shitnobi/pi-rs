#!/usr/bin/env node
/** Directed astral-char probe for the markdown port (marked rules + render). */
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const PI_RS = join(dirname(dirname(fileURLToPath(import.meta.url))), "..");
const WORK = "/tmp/pi-parity/redteam";
const E = "\u{1F600}", M = "\u{1D400}", C = "\u{20000}";

function run(cmd, args, payload) {
  return execFileSync(cmd, args, {
    input: payload, cwd: PI_RS, encoding: "utf8", maxBuffer: 1e8, timeout: 30000,
  });
}
function tsRun(tool, args) {
  return JSON.parse(run("node", ["parity/ts-harness.js", WORK],
    JSON.stringify({ tool, args })));
}
function rsRun(tool, args) {
  return JSON.parse(run(PI_RS + "/target/release/pi-cli",
    ["harness", "tool", tool, "--cwd", WORK], JSON.stringify(args)));
}
function canon(v) {
  if (Array.isArray(v)) return v.map(canon);
  if (v && typeof v === "object") {
    const o = {};
    for (const k of Object.keys(v).sort()) o[k] = canon(v[k]);
    return o;
  }
  return v;
}
function compare(name, source, fn = "lex", tool = "tui-markdown-lex") {
  const args = tool === "tui-markdown-lex" ? { fn, source } : { fn, source, width: 40 };
  const ts = tsRun(tool, { fn, source: tool === "tui-markdown-lex" ? source : source, width: 40 });
  const rs = rsRun(tool, args);
  const a = JSON.stringify(canon(ts)), b = JSON.stringify(canon(rs));
  if (a === b) return true;
  console.log(`FAIL ${name}`);
  console.log("  src:", JSON.stringify(source).slice(0, 200));
  console.log("  TS :", a.slice(0, 400));
  console.log("  RS :", b.slice(0, 400));
  return false;
}

const cases = [
  ["para", "a" + E + "b"],
  ["para-math", "x" + M + "y"],
  ["para-cjk-ext", "x" + C + "y"],
  ["heading", "# " + E + " head"],
  ["heading-only", "#" + E],
  ["setext", "T" + E + "\n==="],
  ["setext-lh", "T" + E + "\n---"],
  ["lheading-astral", E + "\n==="],
  ["em", "**" + E + "**"],
  ["em-mixed", "a*" + E + "*b"],
  ["em-nested", "*" + E + "b*"],
  ["codespan", "`" + E + "`"],
  ["codespan-mix", "`a" + E + "b`"],
  ["codespan-dbl", "``" + E + "``"],
  ["link-text", "[" + E + "](url)"],
  ["link-url", "[t](u" + E + "rl)"],
  ["link-title", "[t](url \"" + E + "\")"],
  ["autolink", "<http://" + E + ".com>"],
  ["autolink-email", "<a" + E + "@b.com>"],
  ["html-tag", "<a" + E + ">text</a" + E + ">"],
  ["html-attr", '<div title="' + E + '">x</div>'],
  ["html-block", "<pre>\n" + E + "\n</pre>"],
  ["url-bare", "see http://" + E + ".com now"],
  ["table-cell", "| a | b |\n|---|---|\n| " + E + " | 2 |"],
  ["table-head", "| " + E + " | b |\n|---|---|\n| 1 | 2 |"],
  ["nptable", "| " + E + " | b |\n|---|---|\n| 1 | 2 |\n"],
  ["list-item", "- " + E],
  ["list-nested", "- a\n  - " + E],
  ["task-emoji", "- [" + E + "] task"],
  ["ordered", "1. " + E],
  ["blockquote", "> " + E],
  ["quote-nested", "> > " + E],
  ["hr-mid", "* * " + E + " *"],
  ["fence-lang", "```" + E + "\ncode\n```"],
  ["fence-body", "```\n" + E + "\n```"],
  ["fence-tilde", "~~" + E + "~~"],
  ["escape", "\\" + E],
  ["reflink", "[" + E + "][r]\n\n[r]: url"],
  ["ref-def-label", "[x][r" + E + "]\n\n[r" + E + "]: url"],
  ["hardbreak", "a" + E + "  \nb"],
  ["entity", "&" + E + ";"],
  ["strikethrough", "~~" + E + "~~"],
  ["br-in-em", "*" + E + "\n" + E + "*"],
  ["del-link", "[[" + E + "]](x)"],
  ["img-alt", "![" + E + "](x.png)"],
  ["mixed-bmp-astral", "\u00e9" + E + "\u0301" + M],
  ["zwj", "\u200d" + E + "\u200d"],
  ["flag", "\u{1F1FA}\u{1F1F8} flag"],
  ["tag-seq", "\u{1F3F4}\u{E0067}\u{E0062}\u{E0053}\u{E0063}\u{E0074}"],
  ["skin", "\u{1F44D}\u{1F3FB}"],
  ["vs16", "\u2764\uFE0F"],
  ["keycap", "1\uFE0F\u20E3"],
];

let bad = 0;
for (const [name, src] of cases) if (!compare(name, src)) bad++;

const renderCases = [
  ["render-astral", E + " *b* c"],
  ["render-astral-latex", "$$x^" + E + "$$"],
  ["render-astral-latex-inline", "$x^" + E + "$"],
  ["render-astral-table", "| " + E + " | b |\n|---|---|\n| 1 | 2 |"],
  ["render-astral-heading", "# " + E],
  ["render-astral-emphasis", "**" + E + "** and " + E],
  ["render-keycap", "1\uFE0F\u20E3 ok"],
];
for (const [name, src] of renderCases) if (!compare(name, src, "render", "tui-markdown-render")) bad++;

console.log(bad === 0 ? `ALL ${cases.length + renderCases.length} MATCH` : bad + " mismatches");
process.exit(bad === 0 ? 0 : 1);
