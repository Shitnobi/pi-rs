/**
 * Fuzz-batter regression aggregator.
 *
 * parity/fuzz/run-batter.mjs writes each confirmed (ddmin-minimized)
 * divergence to parity/regressions/reg-<target>-<seq>.mjs as a permanent
 * parity case. This module scans the directory and flattens them into the
 * main suite (parity/cases.mjs imports `regressionCases`), so regressions
 * are re-checked on every full parity run automatically.
 */
import { readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export const regressionCases = [];
for (const f of readdirSync(here)
	.filter((f) => /^reg-.*\.mjs$/.test(f))
	.sort()) {
	const mod = await import(pathToFileURL(join(here, f)).href);
	regressionCases.push(...mod.cases);
}
