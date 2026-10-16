#!/usr/bin/env node
/**
 * TS-side parity harness.
 *
 * Drives the ORIGINAL pi coding-agent tools (built dist) with a JSON case on
 * stdin: { "tool": "...", "args": { ... } }. The working directory is passed
 * as argv[2]. Prints the tool result as JSON on stdout (same shape the Rust
 * harness emits), or { "error": "<message>" } when the tool throws.
 *
 * Mirrors the pi agent loop: `tool.prepareArguments(args)` is applied before
 * `tool.execute(toolCallId, args)`.
 */
const DIST = process.env.PI_DIST || "/tmp/pi-repo/packages/coding-agent/dist/index.js";
const m = require(DIST);

const FACTORIES = {
	read: m.createReadTool,
	write: m.createWriteTool,
	edit: m.createEditTool,
	bash: m.createBashTool,
	grep: m.createGrepTool,
	find: m.createFindTool,
	ls: m.createLsTool,
};

let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => (input += d));
process.stdin.on("end", async () => {
	const { tool, args } = JSON.parse(input);
	const cwd = process.argv[2];
	try {
		const t = FACTORIES[tool](cwd);
		const prepared = t.prepareArguments ? t.prepareArguments(args) : args;
		const result = await t.execute("parity-call", prepared);
		process.stdout.write(JSON.stringify(result));
	} catch (e) {
		process.stdout.write(
			JSON.stringify({ error: e && e.message !== undefined ? e.message : String(e) }),
		);
	}
});
