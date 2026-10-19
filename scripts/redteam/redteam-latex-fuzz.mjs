#!/usr/bin/env node
/** R2 battery 3: randomized fuzz. */
import { execFileSync } from "node:child_process";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const PI_RS = join(dirname(dirname(fileURLToPath(import.meta.url))), "..");
const WORK = "/tmp/pi-parity/redteam";
const lx = await import(pathToFileURL(process.env.PI_TS_SRC ?? "/tmp/pi-repo/packages/tui/src/latex.ts").href);
function tsRender(s, d) { return lx.renderLatex(s, d ? { display: true } : {}) ?? null; }
function rsRender(s, d) {
  const out = execFileSync(PI_RS + "/target/release/pi-cli", ["harness", "tool", "tui-latex", "--cwd", WORK], {
    input: JSON.stringify({ fn: "render", source: s, display: !!d }),
    cwd: PI_RS, encoding: "utf8", maxBuffer: 1e8,
  });
  return JSON.parse(out).result ?? null;
}

// deterministic PRNG
let seed = 0xC0FFEE;
function rnd() { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed / 0x7fffffff; }
function pick(a) { return a[Math.floor(rnd() * a.length)]; }
function maybe(p) { return rnd() < p; }

const cmds = ["alpha","beta","gamma","delta","varepsilon","phi","varphi","pi","sigma","tau",
  "to","rightarrow","Rightarrow","le","ge","ne","equiv","approx","sim","in","notin","subset",
  "sum","prod","int","oint","lim","max","min","sup","inf","sin","cos","log","ln","exp","det",
  "frac","dfrac","tfrac","sqrt","mathbb","mathcal","text","mathrm","hat","vec","bar","tilde",
  "overline","underline","not","left","right","bigl","bigr","displaystyle","limits","nolimits",
  "quad","qquad",",",";","!","pmod","bmod","boxed","binom","overset","underset","operatorname",
  "pm","times","cdot","div","cdot","infty","partial","nabla","hbar","ell","wp","angle","top","bot"];
const atoms = ["a","b","x","y","n","0","1","2","+","-","=","<",">",".","(",")","[","]","{","}",
  "\\{","\\}","\\$","\\%","\\#","\\_","&","~","'","''","中","文","😀","é","\u00A0","\u2028","\r\n","\t"," "];

function genGroup(depth) {
  const n = 1 + Math.floor(rnd() * 4);
  let s = "";
  for (let i = 0; i < n; i++) {
    if (depth > 0 && maybe(0.35)) {
      const kind = pick(["braces","cmd","script","frac","sqrt","env","atom","ws","break"]);
      if (kind === "braces") s += "{" + genGroup(depth - 1) + "}";
      else if (kind === "cmd") s += genCmd(depth);
      else if (kind === "script") s += (maybe(0.5) ? "^" : "_") + genArg(depth - 1);
      else if (kind === "frac") s += "\\" + pick(["frac","dfrac","tfrac"]) + genArg(depth - 1) + genArg(depth - 1);
      else if (kind === "sqrt") s += "\\sqrt" + (maybe(0.3) ? "[3]" : maybe(0.2) ? "[n]" : "") + genArg(depth - 1);
      else if (kind === "env") s += genEnv(depth);
      else if (kind === "atom") s += pick(atoms);
      else if (kind === "ws") s += pick([" ", "\t", "\n", "\r\n", "\u00A0"]);
      else s += "\\\\";
    } else {
      s += pick(atoms);
    }
  }
  return s;
}
function genArg(depth) {
  if (maybe(0.5)) return "{" + genGroup(depth) + "}";
  if (maybe(0.3)) return "\\" + pick(cmds);
  return pick(["a","b","x","1","2","+","-","中","😀"]);
}
function genCmd(depth) {
  const c = pick(cmds);
  let s = "\\" + c;
  if (["frac","dfrac","tfrac","sqrt","mathbb","mathcal","text","mathrm","hat","vec","bar",
    "tilde","overline","underline","boxed","binom","overset","underset","operatorname",
    "pmod","not"].includes(c)) {
    const nargs = ["frac","dfrac","tfrac","binom","overset","underset"].includes(c) ? 2 : 1;
    for (let i = 0; i < nargs; i++) s += genArg(depth);
  }
  return s;
}
function genEnv(depth) {
  const env = pick(["pmatrix","bmatrix","cases","aligned","matrix","array","equation","gather"]);
  const rows = 1 + Math.floor(rnd() * 3);
  const cols = 1 + Math.floor(rnd() * 3);
  let body = "";
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      body += genGroup(depth);
      if (c < cols - 1) body += "&";
    }
    if (r < rows - 1) body += "\\\\";
  }
  let pre = "";
  if (env === "array") pre = "{cc}";
  return "\\begin{" + env + "}" + pre + body + "\\end{" + env + "}";
}

const N = 600;
let mismatches = 0;
for (let i = 0; i < N; i++) {
  const depth = 1 + Math.floor(rnd() * 3);
  let src;
  const shape = pick(["plain", "group", "cmd", "env", "script"]);
  if (shape === "plain") src = genGroup(depth);
  else if (shape === "group") src = "{" + genGroup(depth) + "}";
  else if (shape === "cmd") src = genCmd(depth);
  else if (shape === "env") src = genEnv(depth);
  else src = pick(["a","x","\\alpha","\\sum"]) + (maybe(0.5) ? "^" : "_") + genArg(depth) + (maybe(0.5) ? (maybe(0.5) ? "^" : "_") + genArg(depth) : "");
  const disp = maybe(0.5);
  const t = tsRender(src, disp);
  const r = rsRender(src, disp);
  if (JSON.stringify(t) !== JSON.stringify(r)) {
    mismatches++;
    console.log("--- src=" + JSON.stringify(src) + " display=" + disp);
    console.log("  TS:", JSON.stringify(t));
    console.log("  RS:", JSON.stringify(r));
    if (mismatches > 20) break;
  }
}
console.log("fuzz cases:", N, "mismatches:", mismatches);
