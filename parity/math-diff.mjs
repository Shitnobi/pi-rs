#!/usr/bin/env node
/**
 * Differential float hunt: compares Math.* bit patterns between Node (V8)
 * and the Rust pi-cli `math` harness over large random input batches.
 *
 * Usage: node parity/math-diff.mjs [count]
 */
import { spawnSync } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

const count = Number(process.argv[2] || 100000);
const RS = "/home/shinobi/AI-Trident/pi-rs/target/release/pi-cli";
const TS = "/home/shinobi/AI-Trident/pi-rs/parity/ts-harness.js";

// PRNG with float32-free full-mantissa doubles
let state = 0x2f6e2b1 ^ Date.now();
function rnd() {
	// xorshift64
	let x = state;
	x ^= (x << 13n) ? 0n : 0n;
	return Math.random();
}
const rand = Math.random;

function uniform(lo, hi) {
	return lo + (hi - lo) * rand();
}
function bitPatternDouble() {
	// random finite double with exponent in a moderate band
	const buf = new ArrayBuffer(8);
	const v = new DataView(buf);
	v.setUint32(0, (Math.floor(rand() * 0x40_0000) << 0) >>> 0);
	v.setUint32(4, Math.floor(rand() * 0x100000000) >>> 0);
	let bits = v.getBigUint64(0, false);
	// clamp exponent to [1023-30, 1023+30]
	bits = (bits & 0x800fffffffffffffn) | (0x3ffn - 30n + BigInt(Math.floor(rand() * 61)) * (1n << 52n));
	v.setBigUint64(0, bits, false);
	const f = v.getFloat64(0, false);
	return Number.isFinite(f) ? f : uniform(-10, 10);
}

const ops = [];
function add(fn, args) {
	ops.push({ fn, args });
}

for (let i = 0; i < count; i++) {
	const t = i % 10;
	switch (t) {
		case 0: add("atan2", [uniform(-0.5, 0.5), uniform(-0.5, 0.5)]); break;
		case 1: add("atan2", [uniform(-1, 1), uniform(-1, 1)]); break;
		case 2: add("atan2", [bitPatternDouble(), bitPatternDouble()]); break;
		case 3: add("sin", [uniform(0, Math.PI * 2)]); break;
		case 4: add("sin", [bitPatternDouble()]); break;
		case 5: add("cos", [uniform(0, Math.PI * 2)]); break;
		case 6: add("cos", [bitPatternDouble()]); break;
		case 7: add("cbrt", [uniform(-1, 1)]); break;
		case 8: add("cbrt", [bitPatternDouble()]); break;
		case 9: add("hypot", [uniform(-0.5, 0.5), uniform(-0.5, 0.5)]); break;
	}
}

const payload = JSON.stringify({ tool: "math", args: { ops } });
const dir = mkdtempSync(join(tmpdir(), "mathdiff-"));
const file = join(dir, "input.json");
writeFileSync(file, payload);

const rsInput = JSON.stringify({ ops });

const rs = spawnSync(RS, ["harness", "tool", "math", "--cwd", dir], { input: rsInput, encoding: "utf8", maxBuffer: 1 << 28 });
const ts = spawnSync("node", [TS, dir], { input: payload, encoding: "utf8", maxBuffer: 1 << 28 });
if (rs.status !== 0) { console.error("RUST FAILED:", rs.stderr); process.exit(1); }
if (ts.status !== 0) { console.error("TS FAILED:", ts.stderr); process.exit(1); }

const rsResults = JSON.parse(rs.stdout).results;
const tsResults = JSON.parse(ts.stdout).results;

const mismatch = {};
const samples = {};
for (let i = 0; i < ops.length; i++) {
	if (rsResults[i] !== tsResults[i]) {
		const key = ops[i].fn;
		mismatch[key] = (mismatch[key] || 0) + 1;
		if (!samples[key] || samples[key].length < 3) {
			(samples[key] ||= []).push({ args: ops[i].args, ts: tsResults[i], rs: rsResults[i] });
		}
	}
}

const totals = {};
for (const op of ops) totals[op.fn] = (totals[op.fn] || 0) + 1;
console.log(`total ops: ${ops.length}`);
for (const fn of Object.keys(totals)) {
	const bad = mismatch[fn] || 0;
	console.log(`  ${fn}: ${bad}/${totals[fn]} mismatches`);
	if (bad && samples[fn]) {
		for (const s of samples[fn]) {
			console.log(`    args=${JSON.stringify(s.args)} ts=${s.ts} rs=${s.rs}`);
		}
	}
}
