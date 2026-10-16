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
 *
 * Module-level entries (not AgentTools) drive the dist modules directly,
 * mirroring the corresponding Rust harness entries:
 * - "bash-executor": executeBashWithOperations with local shell operations
 * - "output-accumulator": OutputAccumulator append/finish/snapshot/...
 * - "messages": core/messages.js pure functions
 */
const path = require("path");
const DIST = process.env.PI_DIST || "/tmp/pi-repo/packages/coding-agent/dist/index.js";
const distDir = path.dirname(DIST);
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

/** Drive module-level APIs (mirrors pi-cli's module harness entries). */
async function runModule(tool, args, cwd) {
	if (tool === "bash-executor") {
		const { executeBashWithOperations } = require(path.join(distDir, "core", "bash-executor.js"));
		const { createLocalBashOperations } = require(path.join(distDir, "core", "tools", "bash.js"));
		return await executeBashWithOperations(args.command, cwd, createLocalBashOperations(), {});
	}
	if (tool === "output-accumulator") {
		const { OutputAccumulator } = require(path.join(distDir, "core", "tools", "output-accumulator.js"));
		const acc = new OutputAccumulator({
			maxLines: args.maxLines,
			maxBytes: args.maxBytes,
			tempFilePrefix: args.tempFilePrefix,
		});
		for (const chunk of args.chunks || []) {
			acc.append(Buffer.from(chunk, "base64"));
		}
		const out = {};
		if (args.finish !== false) acc.finish();
		if (args.snapshot !== false) {
			out.snapshot = acc.snapshot({ persistIfTruncated: args.persistIfTruncated !== false });
		}
		await acc.closeTempFile();
		if (args.readFullOutputMaxBytes != null) {
			out.full = await acc.readFullOutput(args.readFullOutputMaxBytes);
		}
		out.lastLineBytes = acc.getLastLineBytes();
		return out;
	}
	if (tool === "messages") {
		const msgs = require(path.join(distDir, "core", "messages.js"));
		switch (args.fn) {
			case "bashExecutionToText":
				return { text: msgs.bashExecutionToText(args.message) };
			case "convertToLlm":
				return { messages: msgs.convertToLlm(args.messages) };
			case "createBranchSummaryMessage":
				return msgs.createBranchSummaryMessage(args.summary, args.fromId, args.timestamp);
			case "createCompactionSummaryMessage":
				return msgs.createCompactionSummaryMessage(args.summary, args.tokensBefore, args.timestamp);
			case "createCustomMessage":
				return msgs.createCustomMessage(args.customType, args.content, args.display, args.details, args.timestamp);
			default:
				throw new Error("unknown messages fn: " + args.fn);
		}
	}
	return undefined;
}

let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => (input += d));
process.stdin.on("end", async () => {
	const { tool, args } = JSON.parse(input);
	const cwd = process.argv[2];
	try {
		const moduleResult = await runModule(tool, args, cwd);
		if (moduleResult !== undefined) {
			process.stdout.write(JSON.stringify(moduleResult));
			return;
		}
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
