#!/usr/bin/env node
/**
 * Whitespace/line-terminator divergence probe for the marked port.
 *
 * Runs `tui-markdown-lex` cases through BOTH the TS original and the Rust
 * port and reports any canonical-JSON divergences. Covers every JS `\s`
 * char (plus NEL U+0085, which is Rust-but-not-JS whitespace) across the
 * block shapes whose rules embed `\s` or `.`: heading, lheading, paragraph,
 * table, fences, list, blockquote, hr, indented code, inline text.
 *
 * Usage: node scripts/probe-ws-parity.mjs
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");
const piCli = existsSync(join(repoRoot, "target", "release", "pi-cli"))
	? join(repoRoot, "target", "release", "pi-cli")
	: join(repoRoot, "target", "debug", "pi-cli");
const tsHarness = join(repoRoot, "parity", "ts-harness.js");
const workDir = "/tmp/pi-parity/wsprobe";

const WS_CHARS = [
	["space", " "],
	["tab", "\t"],
	["nbsp", "\u00a0"],
	["u1680", "\u1680"],
	["u2000", "\u2000"],
	["u200a", "\u200a"],
	["ls", "\u2028"],
	["ps", "\u2029"],
	["nnbsp", "\u202f"],
	["mmsp", "\u205f"],
	["ideo", "\u3000"],
	["bom", "\ufeff"],
	["nel", "\u0085"], // Rust \s but NOT JS \s
];

// Shapes exercising rules with \s or . in them. %s = the probe char.
const SHAPES = [
	["heading", "#%sfoo"],
	["heading-after", "# foo%sbar"],
	["lheading", "foo%s===\n"],
	["paragraph-join", "foo%sbar"],
	["table", "| a%sb |\n| --- |\n| c |"],
	["fence-lang", "```%sjs\nx\n```"],
	["list", "-%sfoo"],
	["blockquote", ">%sfoo"],
	["hr", "*%s*%s*\n"],
	["code-indent", "   %sx"],
	["inline-text", "a%s*b*c"],
];

function canon(v) {
	if (Array.isArray(v)) return v.map(canon);
	if (v && typeof v === "object") {
		const r = {};
		for (const k of Object.keys(v).sort()) r[k] = canon(v[k]);
		return r;
	}
	return v;
}

function runTool(kind, source) {
	const input = JSON.stringify({ tool: "tui-markdown-lex", args: { fn: "lex", source } });
	if (kind === "ts") {
		const r = spawnSync(process.execPath, [tsHarness, workDir], { input, encoding: "utf8" });
		return r.status === 0 ? r.stdout : `EXIT ${r.status}: ${r.stderr.slice(0, 200)}`;
	}
	const r = spawnSync(piCli, ["harness", "tool", "tui-markdown-lex", "--cwd", workDir], {
		input: JSON.stringify({ fn: "lex", source }),
		encoding: "utf8",
	});
	return r.status === 0 ? r.stdout : `EXIT ${r.status}: ${r.stderr.slice(0, 200)}`;
}

rmSync(workDir, { recursive: true, force: true });
mkdirSync(workDir, { recursive: true });

let total = 0;
let diverged = 0;
const failures = [];
for (const [cname, ch] of WS_CHARS) {
	for (const [sname, tmpl] of SHAPES) {
		const source = tmpl.replaceAll("%s", ch);
		total++;
		const ts = runTool("ts", source);
		const rs = runTool("rs", source);
		let a, b;
		try {
			a = JSON.stringify(canon(JSON.parse(ts)));
			b = JSON.stringify(canon(JSON.parse(rs)));
		} catch (e) {
			a = ts;
			b = rs;
		}
		if (a !== b) {
			diverged++;
			failures.push({ cname, sname, source, ts: a.slice(0, 400), rs: b.slice(0, 400) });
		}
	}
}

console.log(`ws-probe: ${total - diverged}/${total} match, ${diverged} divergent`);
for (const f of failures) {
	console.log(`--- ${f.cname} / ${f.sname}  source=${JSON.stringify(f.source)}`);
	console.log(`  TS: ${f.ts}`);
	console.log(`  RS: ${f.rs}`);
}
process.exit(diverged === 0 ? 0 : 1);
