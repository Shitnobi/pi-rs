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
	if (tool === "session-manager") {
		const sm = require(path.join(distDir, "core", "session-manager.js"));
		switch (args.fn) {
			case "assertValidSessionId":
				sm.assertValidSessionId(args.id);
				return { valid: true };
			case "migrateSessionEntries": {
				const entries = args.entries;
				sm.migrateSessionEntries(entries);
				return { entries };
			}
			case "parseSessionEntries":
				return { entries: sm.parseSessionEntries(args.content) };
			case "getLatestCompactionEntry":
				return { entry: sm.getLatestCompactionEntry(args.entries) };
			case "sessionEntryToContextMessages":
				return { messages: sm.sessionEntryToContextMessages(args.entry) };
			case "buildContextEntries":
				return { entries: sm.buildContextEntries(args.entries, args.leafId) };
			case "buildSessionProjection":
				return sm.buildSessionProjection(args.entries, args.leafId);
			case "buildSessionContext":
				return sm.buildSessionContext(args.entries, args.leafId);
			case "getDefaultSessionDir": {
				const agentDir = args.agentDir !== undefined ? path.resolve(cwd, args.agentDir) : undefined;
				return { dir: sm.getDefaultSessionDir(args.cwd, agentDir) };
			}
			case "loadEntriesFromFile":
				return { entries: sm.loadEntriesFromFile(path.resolve(cwd, args.filePath)) };
			case "findMostRecentSession": {
				const sessionDir = path.resolve(cwd, args.sessionDir);
				return { path: sm.findMostRecentSession(sessionDir, args.cwd) };
			}
			default:
				throw new Error("unknown session-manager fn: " + args.fn);
		}
	}
	if (tool === "compaction-utils") {
		const cu = require(path.join(distDir, "core", "compaction", "utils.js"));
		const toFileOps = (o) => ({
			read: new Set(o?.read ?? []),
			written: new Set(o?.written ?? []),
			edited: new Set(o?.edited ?? []),
		});
		const fromFileOps = (f) => ({
			read: [...f.read],
			written: [...f.written],
			edited: [...f.edited],
		});
		switch (args.fn) {
			case "createFileOps":
				return { result: fromFileOps(cu.createFileOps()) };
			case "extractFileOpsFromMessage": {
				const fileOps = toFileOps(args.fileOps);
				cu.extractFileOpsFromMessage(args.message, fileOps);
				return { result: fromFileOps(fileOps) };
			}
			case "computeFileLists":
				return { result: cu.computeFileLists(toFileOps(args.fileOps)) };
			case "formatFileOperations":
				return { result: cu.formatFileOperations(args.readFiles ?? [], args.modifiedFiles ?? []) };
			case "serializeConversation":
				return { result: cu.serializeConversation(args.messages ?? []) };
			case "summarizationSystemPrompt":
				return { result: cu.SUMMARIZATION_SYSTEM_PROMPT };
			default:
				throw new Error("unknown compaction-utils fn: " + args.fn);
		}
	}
	if (tool === "compaction") {
		const c = require(path.join(distDir, "core", "compaction", "compaction.js"));
		const sm = require(path.join(distDir, "core", "session-manager.js"));
		switch (args.fn) {
			case "defaultSettings":
				return { result: c.DEFAULT_COMPACTION_SETTINGS };
			case "calculateContextTokens":
				return { result: c.calculateContextTokens(args.usage) };
			case "getLastAssistantUsage":
				return { result: c.getLastAssistantUsage(args.entries) ?? null };
			case "estimateContextTokens":
				return { result: c.estimateContextTokens(args.messages) };
			case "estimateProjectedContextTokens":
				return {
					result: c.estimateProjectedContextTokens(sm.buildSessionProjection(args.entries), args.branchEntries),
				};
			case "shouldCompact":
				return { result: c.shouldCompact(args.contextTokens, args.contextWindow, args.settings) };
			case "estimateTokens":
				return { result: c.estimateTokens(args.message) };
			case "findCutPoint":
				return {
					result: c.findCutPoint(
						args.entries,
						args.startIndex ?? 0,
						args.endIndex ?? args.entries.length,
						args.keepRecentTokens,
					),
				};
			case "findTurnStartIndex":
				return { result: c.findTurnStartIndex(args.entries, args.entryIndex, args.startIndex ?? 0) };
			case "getSummarizationFailure":
				return { result: c.getSummarizationFailure(args.response, args.label) ?? null };
			case "prepareCompaction": {
				const prep = c.prepareCompaction(args.entries, args.settings);
				if (!prep) return { result: null };
				return {
					result: {
						...prep,
						fileOps: {
							read: [...prep.fileOps.read],
							written: [...prep.fileOps.written],
							edited: [...prep.fileOps.edited],
						},
					},
				};
			}
			default:
				throw new Error("unknown compaction fn: " + args.fn);
		}
	}
	if (tool === "nested-calls") {
		const nc = require(path.join(distDir, "core", "nested-tool-calls.js"));
		switch (args.fn) {
			case "limits":
				return { result: nc.NESTED_CALL_LIMITS };
			case "recorder": {
				const recorder = new nc.NestedCallRecorder();
				const records = [];
				const results = [];
				for (const op of args.ops ?? []) {
					switch (op.op) {
						case "start": {
							const rec = recorder.start(op.toolCall);
							if (rec) records.push(rec);
							results.push({ index: rec ? records.length - 1 : null });
							break;
						}
						case "finish":
							recorder.finish(
								op.index === null || op.index === undefined ? undefined : records[op.index],
								op.isError,
								op.error ?? "",
							);
							results.push({});
							break;
						case "addUsage":
							recorder.addUsage(op.usage);
							results.push({});
							break;
						case "totalUsage":
							results.push({ result: recorder.totalUsage ?? null });
							break;
						case "snapshot": {
							const snap = recorder.snapshot();
							if (snap) {
								for (const call of snap.calls) {
									if (call.durationMs !== undefined) call.durationMs = 0;
								}
							}
							results.push({ result: snap ?? null });
							break;
						}
						default:
							throw new Error("unknown nested-calls op: " + op.op);
					}
				}
				return { results };
			}
			default:
				throw new Error("unknown nested-calls fn: " + args.fn);
		}
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
