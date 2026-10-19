import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const WORKDIR = "/tmp/pi-parity/redteam";
const PI_RS = join(dirname(dirname(fileURLToPath(import.meta.url))), "..");

function canon(v) {
  if (Array.isArray(v)) return v.map(canon);
  if (v && typeof v === "object") {
    const o = {};
    for (const k of Object.keys(v).sort()) o[k] = canon(v[k]);
    return o;
  }
  return v;
}

function tsRun(source, fn, tool) {
  const payload = JSON.stringify({ tool, args: { fn, source } });
  const p = spawnSync("node", ["parity/ts-harness.js", WORKDIR], {
    cwd: PI_RS, input: payload, encoding: "utf8", timeout: 30000,
  });
  return p.stdout.trim();
}

function rsRun(source, fn, tool) {
  const payload = JSON.stringify({ fn, source });
  const p = spawnSync(`${PI_RS}/target/release/pi-cli`,
    ["harness", "tool", tool, "--cwd", WORKDIR],
    { input: payload, encoding: "utf8", timeout: 30000, });
  return p.stdout.trim();
}

function compare(name, source, fn = "lex", tool = "tui-markdown-lex") {
  let ts, rs;
  try { ts = JSON.parse(tsRun(source, fn, tool)); } catch (e) { ts = { PARSE_ERR: String(e) }; }
  try { rs = JSON.parse(rsRun(source, fn, tool)); } catch (e) { rs = { PARSE_ERR: String(e) }; }
  const a = JSON.stringify(canon(ts)), b = JSON.stringify(canon(rs));
  if (a === b) { console.log(`PASS ${name}`); }
  else {
    console.log(`FAIL ${name}`);
    console.log("  src:", JSON.stringify(source));
    console.log("  TS :", a.slice(0, 900));
    console.log("  RS :", b.slice(0, 900));
  }
}

const cases = [
  ["astral-ldelim", "_\u{1F44D}_"],
  ["astral-strong", "**a \u{1F44D} b**"],
  ["astral-em-tail", "*\u{1F44D}* tail"],
  ["astral-strict-del", "~~\u{1F44D}~~ x"],
  ["astral-before-del", "\u{1F44D} *b* c"],
  ["sigma-def-reflink", "[\u0391\u03A3]: /u\n\n[x][\u0391\u03A3]"],
  ["turkish-def-reflink", "[\u0130d]: /u\n\n[i\u0307d][]"],
  ["table-escaped-pipe", "| a \\| b | c |\n|---|---|\n| 1 | 2 |"],
  ["table-code-pipe", "| a | b |\n| --- | --- |\n| `x` \\| y |"],
  ["table-blank-cells", "|  |  |\n| --- | --- |\n| | |"],
  ["ordered-mixed", "1. item\n1) two\n* three"],
  ["task-list", "- [ ] task\n- [x] done"],
  ["url-backpedal", "see www.example.com."],
  ["autolink-space", "<https://x.com/a b>"],
  ["partial-fence", "```\ncode\n``"],
  ["partial-fence-tilde", "~~~\ncode\n~~"],
  ["indented-fence", "  ```\ncode\n  ```"],
  ["nested-quote", "> quote\n> > nested"],
  ["latex-mix", "$$x^2$$ and $y_1$ and \\(z\\)"],
  ["latex-space-guard", "$ a$ b $c$"],
  ["latex-pending", "$$x_1"],
  ["tab-input", "a\tb"],
  ["cr-input", "x\r\ny\ry"],
  ["html-pre", "<pre>\nkeep\n</pre>"],
  ["inline-html", "text with <b>bold</b> html"],
  ["empty-link", "[](empty)"],
  ["angle-link-space", "[a](<b c>)"],
  ["image", "![img](x.png)"],
  ["nbS-before-em", "a\u00A0*em*b"],
  ["reflink-fallback", "[unknown][nope]"],
  ["link-parens-title", "[a](b(c) \"t (x)\")"],
  ["codespan-newline", "`a\nb`"],
  ["heading-h7", "####### seven"],
  ["setext-astral", "T\u{1F44D}\n==="],
];

for (const [name, src] of cases) compare(name, src);

// a couple of render checks
const renderCases = [
  ["render-astral", "\u{1F44D} *b* c", 40],
  ["render-table", "| a | b |\n|---|---|\n| 1 | 2 |", 30],
  ["render-partial-fence", "```\ncode\n``", 40],
  ["render-task", "- [ ] task\n- [x] done", 40],
];
for (const [name, src, w] of renderCases) compare(name, src, "render", "tui-markdown-render");
