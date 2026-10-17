// Verification driver for the tui-editor parity group (TS side only).
// Imports cases.mjs, runs every tui-editor case through ts-harness.js,
// validates the §6 output contract, and re-runs a sample for determinism.
import { spawnSync } from "node:child_process";
import { cases } from "/home/shinobi/AI-Trident/pi-rs/parity/cases.mjs";

const HARNESS = "/home/shinobi/AI-Trident/pi-rs/parity/ts-harness.js";
const edCases = cases.filter((c) => c.tool === "tui-editor");
console.log("tui-editor cases:", edCases.length);

const names = edCases.map((c) => c.name);
if (new Set(names).size !== names.length) throw new Error("duplicate case names");

const run = (c) => {
	const r = spawnSync("node", [HARNESS, "/tmp"], {
		input: JSON.stringify({ tool: c.tool, args: c.args }),
		encoding: "utf8",
		maxBuffer: 256 * 1024 * 1024,
	});
	if (r.status !== 0) return { __error: (r.stderr || r.stdout || "").split("\n").slice(-3).join(" | ") };
	try {
		return JSON.parse(r.stdout);
	} catch (e) {
		return { __error: "unparseable: " + r.stdout.slice(0, 200) };
	}
};

const contractKeys = ["name", "text", "expandedText", "cursor", "lines", "frames", "renderRequests", "events"];
let fail = 0;
const outputs = {};
for (const c of edCases) {
	const out = run(c);
	outputs[c.name] = JSON.stringify(out);
	if (out.__error) {
		fail++;
		console.log("ERROR", c.name, out.__error);
		continue;
	}
	const missing = contractKeys.filter((k) => !(k in out));
	if (missing.length) {
		fail++;
		console.log("BAD SHAPE", c.name, "missing:", missing.join(","));
	}
	if (out.cursor && typeof out.cursor.line !== "number") {
		fail++;
		console.log("BAD CURSOR", c.name);
	}
	if (!Array.isArray(out.frames) || out.frames.some((f) => typeof f.width !== "number" || !Array.isArray(f.lines))) {
		fail++;
		console.log("BAD FRAMES", c.name);
	}
	if (!out.events || !Array.isArray(out.events.onChange) || !Array.isArray(out.events.onSubmit)) {
		fail++;
		console.log("BAD EVENTS", c.name);
	}
}
console.log("errors/shape failures:", fail);

// Determinism: re-run every case and compare byte-for-byte.
let nondet = 0;
for (const c of edCases) {
	const again = JSON.stringify(run(c));
	if (again !== outputs[c.name]) {
		nondet++;
		console.log("NONDETERMINISTIC", c.name);
	}
}
console.log("nondeterministic:", nondet);

// Spot-check a few interesting outputs.
const pick = (n) => edCases.find((c) => c.name === "tui-editor/" + n);
for (const n of ["paste-two-markers-renumber", "history-up-down", "scroll-down-border", "jump-forward"]) {
	const c = pick(n);
	if (!c) { console.log("MISSING CASE", n); continue; }
	const o = JSON.parse(outputs[c.name]);
	console.log(
		"---", n,
		"text=" + JSON.stringify(o.text).slice(0, 120),
		"cursor=" + JSON.stringify(o.cursor),
		"expanded=" + JSON.stringify(o.expandedText).slice(0, 120),
	);
}
console.log(fail === 0 && nondet === 0 ? "ALL OK" : "PROBLEMS FOUND");
