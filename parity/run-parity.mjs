#!/usr/bin/env node
/**
 * Differential parity runner.
 *
 * For every case in cases.mjs:
 *   1. create two fresh fixture directories (ts/ and rs/) with identical
 *      setup files,
 *   2. run the case through the ORIGINAL TypeScript implementation
 *      (parity/ts-harness.js driving the built pi dist),
 *   3. run the same case through the Rust port (pi-cli harness tool),
 *   4. compare the tool results (deep JSON equality after normalizing
 *      fixture paths and volatile temp-file names) and the resulting
 *      directory contents.
 *
 * Usage:
 *   node parity/run-parity.mjs [--filter <substring>] [--verbose]
 *
 * Requires: cargo build (target/debug/pi-cli) and the original pi repo built
 * at PI_DIST (default /tmp/pi-repo/packages/coding-agent/dist/index.js).
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { cases } from "./cases.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");
const piCli = process.env.PI_CLI
	?? (existsSync(join(repoRoot, "target", "release", "pi-cli"))
		? join(repoRoot, "target", "release", "pi-cli")
		: join(repoRoot, "target", "debug", "pi-cli"));
const tsHarness = join(here, "ts-harness.js");
const workDir = process.env.PARITY_DIR || "/tmp/pi-parity/run";

const args = process.argv.slice(2);
let filter = null;
let verbose = false;
for (let i = 0; i < args.length; i++) {
	if (args[i] === "--filter") filter = args[++i];
	else if (args[i] === "--verbose") verbose = true;
}

function setupDir(dir, setup) {
	rmSync(dir, { recursive: true, force: true });
	mkdirSync(dir, { recursive: true });
	for (const [rel, content] of Object.entries(setup || {})) {
		const p = join(dir, rel);
		mkdirSync(dirname(p), { recursive: true });
		if (typeof content === "object" && content !== null && content.base64) {
			// Binary fixture: { base64: "..." } so cases.mjs stays text-only.
			writeFileSync(p, Buffer.from(content.base64, "base64"));
		} else if (typeof content === "object" && content !== null && content.fixture) {
			// Binary fixture file from parity/fixtures/.
			writeFileSync(p, readFileSync(join(here, "fixtures", content.fixture)));
		} else {
			writeFileSync(p, content, "utf8");
		}
	}
}

function snapshotDir(dir) {
	const out = {};
	function walk(d) {
		for (const entry of readdirSync(d).sort()) {
			const p = join(d, entry);
			const rel = relative(dir, p);
			if (statSync(p).isDirectory()) {
				out[rel + "/"] = null;
				walk(p);
			} else {
				const buf = readFileSync(p);
				out[rel] = buf.includes(0)
					? "<binary:" + buf.toString("base64") + ">"
					: buf.toString("utf8");
			}
		}
	}
	walk(dir);
	return out;
}

/** Normalize a result: hide fixture paths, temp spill files, pids. */
function normalize(value, dir) {
	let s = JSON.stringify(value).split(dir).join("<DIR>");
	// The spill regexes have an unbounded leading character class; on multi-MB
	// base64 image payloads that backtracks quadratically. Guard with a
	// cheap literal scan so the regexes only run when a spill path exists.
	if (s.includes("pi-bash-") || s.includes("pi-output-")) {
		s = s
			// temp spill files: <tmpdir>/pi-bash-|pi-output-<16 hex chars>.log
			.replace(/[^\s"']*pi-(?:bash|output)-[0-9a-f]+\.log/g, "<SPILL>")
			// node/rust temp dirs occasionally appear in messages
			.replace(/\/tmp\/pi-(?:bash|output)-[^\s"']*/g, "<SPILL>");
	}
	return JSON.parse(
		s
			// wall time is timing-dependent
			.replace(/"wall_time_seconds":[0-9.eE+-]+/g, '"wall_time_seconds":0'),
	);
}

/** For cases that exercise the v1→v2 session migration (which generates
 * random 8-hex-char entry ids), rewrite those ids to stable sequential
 * tokens in order of first appearance. Linkage is preserved: two sides that
 * build the same id graph produce the same token sequence. */
function normalizeSessionIds(value) {
	let counter = 0;
	const map = new Map();
	const visit = (v, key) => {
		if (
			typeof v === "string" &&
			(key === "id" || key === "parentId" || key === "firstKeptEntryId") &&
			/^[0-9a-f]{8}$/.test(v)
		) {
			if (!map.has(v)) map.set(v, `<ID${counter++}>`);
			return map.get(v);
		}
		if (Array.isArray(v)) return v.map((x) => visit(x, undefined));
		if (v && typeof v === "object") {
			const out = {};
			for (const k of Object.keys(v)) out[k] = visit(v[k], k);
			return out;
		}
		return v;
	};
	return visit(value, undefined);
}

/** For cases whose output ordering is inherently racy (fd --max-results
 * early-stop, stdout/stderr pipe interleaving), sort the text lines of
 * every content block and of bash structuredContent.output. */
function sortResultLines(value) {
	if (value && Array.isArray(value.content)) {
		for (const block of value.content) {
			if (block.type === "text" && typeof block.text === "string") {
				block.text = block.text.split("\n").sort().join("\n");
			}
		}
	}
	if (value && value.structuredContent && typeof value.structuredContent.output === "string") {
		value.structuredContent.output = value.structuredContent.output.split("\n").sort().join("\n");
	}
	return value;
}

/** Canonical JSON with sorted keys, so object key order never matters. */
function canonical(value) {
	if (Array.isArray(value)) return value.map(canonical);
	if (value && typeof value === "object") {
		const out = {};
		for (const k of Object.keys(value).sort()) out[k] = canonical(value[k]);
		return out;
	}
	return value;
}

function runTs(caseInfo, dir) {
	const r = spawnSync("node", [tsHarness, dir], {
		input: JSON.stringify({ tool: caseInfo.tool, args: caseInfo.args }),
		encoding: "utf8",
		maxBuffer: 256 * 1024 * 1024,
	});
	if (r.error) return { __harnessError: String(r.error) };
	if (r.status !== 0) {
		return { __harnessError: (r.stderr || r.stdout || "").split("\n").slice(-6).join("\n") };
	}
	try {
		return JSON.parse(r.stdout);
	} catch (e) {
		return { __harnessError: "unparseable output: " + r.stdout.slice(0, 300) };
	}
}

function runRs(caseInfo, dir) {
	const r = spawnSync(piCli, ["harness", "tool", caseInfo.tool, "--cwd", dir], {
		input: JSON.stringify(caseInfo.args),
		encoding: "utf8",
		maxBuffer: 256 * 1024 * 1024,
	});
	if (r.error) return { __harnessError: String(r.error) };
	if (r.status !== 0) {
		return { __harnessError: (r.stderr || r.stdout || "").split("\n").slice(-6).join("\n") };
	}
	try {
		return JSON.parse(r.stdout);
	} catch (e) {
		return { __harnessError: "unparseable output: " + r.stdout.slice(0, 300) };
	}
}

function deepEqual(a, b) {
	return JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
}

/** Shorten long strings so diffs stay readable. */
function shorten(v) {
	if (typeof v === "string" && v.length > 160) return v.slice(0, 160) + `…(+${v.length - 160} chars)`;
	if (Array.isArray(v)) return v.map(shorten);
	if (v && typeof v === "object") {
		const out = {};
		for (const k of Object.keys(v)) out[k] = shorten(v[k]);
		return out;
	}
	return v;
}

function diffLines(a, b) {
	const la = JSON.stringify(shorten(a), null, 2).split("\n");
	const lb = JSON.stringify(shorten(b), null, 2).split("\n");
	const lines = [];
	const n = Math.max(la.length, lb.length);
	for (let i = 0; i < n; i++) {
		if (la[i] !== lb[i]) {
			lines.push(`  ts: ${la[i] ?? "<missing>"}`);
			lines.push(`  rs: ${lb[i] ?? "<missing>"}`);
		}
	}
	return lines.slice(0, 60).join("\n");
}

const selected = cases.filter((c) => !filter || c.name.includes(filter));
let pass = 0;
const failures = [];

for (const c of selected) {
	const tsDir = join(workDir, c.name.replace(/[^a-zA-Z0-9/_-]/g, "_"), "ts");
	const rsDir = join(workDir, c.name.replace(/[^a-zA-Z0-9/_-]/g, "_"), "rs");
	setupDir(tsDir, c.setup);
	setupDir(rsDir, c.setup);

	const tsRaw = runTs(c, tsDir);
	const rsRaw = runRs(c, rsDir);
	let ts = normalize(tsRaw, tsDir);
	let rs = normalize(rsRaw, rsDir);
	if (c.sortLines) {
		sortResultLines(ts);
		sortResultLines(rs);
	}
	if (c.normalizeSessionIds) {
		const tsNorm = normalizeSessionIds(ts);
		const rsNorm = normalizeSessionIds(rs);
		ts = tsNorm;
		rs = rsNorm;
	}

	const tsFs = normalize(snapshotDir(tsDir), tsDir);
	const rsFs = normalize(snapshotDir(rsDir), rsDir);

	const resultOk = deepEqual(ts, rs);
	const fsOk = deepEqual(tsFs, rsFs);

	if (resultOk && fsOk) {
		pass++;
		if (verbose) console.log(`PASS ${c.name}`);
	} else {
		failures.push(c.name);
		console.log(`FAIL ${c.name}`);
		if (!resultOk) {
			console.log("  result mismatch:");
			console.log(diffLines(ts, rs));
		}
		if (!fsOk) {
			console.log("  filesystem mismatch:");
			console.log(diffLines(tsFs, rsFs));
		}
	}
}

console.log(`\n${pass}/${selected.length} cases passed`);
if (failures.length) {
	console.log("failures:");
	for (const f of failures) console.log("  " + f);
	process.exit(1);
}
