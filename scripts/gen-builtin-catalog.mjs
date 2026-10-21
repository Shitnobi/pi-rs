// Regenerates crates/pi-providers/src/pi_ai/builtin-catalog.json from the
// TS reference dist (ground truth: /home/shinobi/pi-repo/packages/ai/dist).
// Structure: { generatedAt, schemaVersion, providers: { <provider>: { <api>:
// { "chat:<id>" | "image:<id>" | "classifier:<id>": <model> } } } }.
// Usage: env PATH=/home/shinobi/.config/goose/mcp-hermit/bin:$PATH node scripts/gen-builtin-catalog.mjs
import { readdirSync, readFileSync, writeFileSync } from "fs";
import { join } from "path";

const DIST_DATA = "/home/shinobi/pi-repo/packages/ai/dist/providers/data";
const ALL_JS = "/home/shinobi/pi-repo/packages/ai/dist/providers/all.js";
const OUT = "crates/pi-providers/src/pi_ai/builtin-catalog.json";

// Canonical provider order: the import order in providers/all.js (the
// builtinProviders() array order). NOT filename-sorted ('-' < '.', so
// 'google-vertex.json' would sort before 'google.json').
const allJs = readFileSync(ALL_JS, "utf8");
const order = [];
for (const m of allJs.matchAll(/from "\.\/([a-z0-9-]+)\.js"/g)) {
	if (!order.includes(m[1])) order.push(m[1]);
}

const providers = {};
let count = 0;
const files = readdirSync(DIST_DATA).filter((f) => f.endsWith(".json"));
for (const file of [
	...order.filter((f) => files.includes(f + ".json")).map((f) => f + ".json"),
	...files.filter((f) => !order.includes(f.slice(0, -5))),
]) {
	const provider = file.slice(0, -".json".length);
	const apis = JSON.parse(readFileSync(join(DIST_DATA, file), "utf8"));
	const out = {};
	for (const [api, models] of Object.entries(apis)) {
		const apiOut = {};
		for (const [id, model] of Object.entries(models)) {
			if (!model || typeof model !== "object" || !("id" in model)) continue;
			apiOut[id] = model; // keys are already "chat:<id>" / "image:<id>" / "classifier:<id>"
			count++;
		}
		if (Object.keys(apiOut).length > 0) out[api] = apiOut;
	}
	if (Object.keys(out).length > 0) providers[provider] = out;
}

const catalog = {
	generatedAt: new Date().toISOString(),
	schemaVersion: 6,
	providers,
};
writeFileSync(OUT, JSON.stringify(catalog));
console.log(`wrote ${OUT}: ${count} models across ${Object.keys(providers).length} providers`);
