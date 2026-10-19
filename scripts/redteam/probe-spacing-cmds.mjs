// Spacing-command differential probe — closes the gap left by r2-sweep's
// tsSet() extraction bug (the TS array's first element is the string ",", so
// naive comma-splitting dropped it from the behavioral sweep).
import { execFileSync } from "node:child_process";

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const TS = ["node", join(repoRoot, "parity", "ts-harness.js")];
const RS = [join(repoRoot, "target", "release", "pi-cli"), "harness", "tool", "tui-latex"];
const WORKDIR = "/tmp/pi-parity/run";

function tsRun(source, display) {
	const out = execFileSync(TS[0], [TS[1], WORKDIR + "/spacing-probe/ts"], {
		input: JSON.stringify({ tool: "tui-latex", args: { fn: "render", source, display } }),
		encoding: "utf8",
	});
	return JSON.parse(out);
}

function rsRun(source, display) {
	const out = execFileSync(RS[0], [...RS.slice(1), "--cwd", WORKDIR + "/spacing-probe/rs"], {
		input: JSON.stringify({ fn: "render", source, display }),
		encoding: "utf8",
	});
	return JSON.parse(out);
}

const SPACING = [",", ":", ";", " ", ">", "enspace", "enskip", "medspace", "quad", "qquad", "thickspace", "thinspace"];
const NEG = ["!", "negmedspace", "negthickspace", "negthinspace"];

let pass = 0, fail = 0;
for (const cmd of [...SPACING, ...NEG]) {
	for (const display of [false, true]) {
		const source = `a \\${cmd} b`;
		const ts = tsRun(source, display);
		const rs = rsRun(source, display);
		if (JSON.stringify(ts) === JSON.stringify(rs)) pass++;
		else {
			fail++;
			console.log(`MISMATCH \\${cmd} display=${display}`);
			console.log("  ts:", JSON.stringify(ts));
			console.log("  rs:", JSON.stringify(rs));
		}
	}
}
console.log(`${pass}/${pass + fail} spacing-command cases match`);
process.exit(fail ? 1 : 0);
