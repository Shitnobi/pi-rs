# Phase 3 Port Plan: MCP + Extensions Subsystem

Source analyzed (TS original, `/tmp/pi-repo/packages/coding-agent/src`):
- `core/mcp-servers.ts` (346 LOC)
- `core/extensions/` — `loader.ts` (876), `runner.ts` (1565), `types.ts` (2304), `index.ts` (225), `wrapper.ts` (29), `virtual-modules.ts` (38), `jiti-loader.ts` (3), `jiti-static-loader.ts` (3)
- `extensions/` (user-facing built-ins) — `index.ts` (14), `mcp/` (9 files, 3906), `codemode/` (5 files, 1390), `tool-search/` (2 files, 262), `llama/` (5 files, 1577)
- Consumers: `core/agent-session.ts` (imports at L114–116, construction at L3660–3684), root `config.ts` (constants/env helpers).

Total ≈ 12,550 LOC of TS. This plan ports the MCP + extension-core surface faithfully and defers the JS-engine-bound parts (jiti loading, QuickJS codemode sandbox, OAuth network flows, llama.cpp HTTP) behind traits with incremental parity.

---

## 1. File-by-file inventory

### 1.1 `core/mcp-servers.ts` (346 LOC)

Responsibility: MCP server config shapes, validation, tool-name matching, and the in-memory registry of extension-registered servers. **The core only validates and stores registrations** (header comment L1–6); connecting is the MCP extension's job via the `mcp_servers_change` event.

Exports (exact signatures, TS line numbers):

| Line | Export |
|---|---|
| L12 | `function toolPatternRegExp(pattern: string): RegExp` — splits on `*`, regex-escapes each part (`/[.+?^${}()\|[\]\\]/g`), joins with `.*`, anchors `^…$` |
| L21 | `export function createToolNameMatcher(entries: readonly string[]): (name: string) => boolean` — exact `Set` for non-wildcards, regex list for `*` patterns |
| L28–30 | `LIST_MCP_RESOURCES_TOOL = "list_mcp_resources"`, `LIST_MCP_RESOURCE_TEMPLATES_TOOL`, `READ_MCP_RESOURCE_TOOL` |
| L39 | `export function isMcpToolName(name: string): boolean` — `name.startsWith("mcp__") \|\| MCP_RESOURCE_TOOLS.has(name)` |
| L52 | `export type McpExposure = "codemode" \| "deferred" \| "direct" \| "hidden"` |
| L80–88 | `interface McpStdioServerConfig` (`type?: "stdio"`, `command`, `args?`, `env?`, `cwd?` + base: `exposure?`, `description?`, `toolExposure?`, `enabled?`, `timeout?`) |
| L91–126 | `interface McpOAuthConfig` (clientId, clientSecret, callbackPort, callbackUrl, scope, clientName, clientRegistration `"dcr"\|"cimd"`, authServerMetadataUrl) |
| L131 | `export function isLoopbackRedirectUri(value: string): boolean` — `URL.canParse`, `http:` protocol, host in `["localhost","127.0.0.1","[::1]"]`, empty query+hash |
| L137–148 | `interface McpHttpServerConfig` (`type?: "http"`, `url`, `headers?`, `oauth?`, `auth?: { provider }`) |
| L150 | `export type McpServerConfig = McpStdioServerConfig \| McpHttpServerConfig` |
| L155 | `export function mcpNamespace(server: string): string` — `` `mcp__${server.replace(/-/g, "_")}` `` |
| L234 | `export function getMcpToolExposure(config: McpServerConfig, toolName: string): McpExposure` — exact `toolExposure` entry wins, then first matching `*` pattern in object order, else `config.exposure ?? "codemode"` |
| L248 | `export function validateMcpServerConfig(name: string, raw: unknown): McpServerConfig \| string` — returns a **string error message** (not an exception). Checks: name `/^[A-Za-z0-9_-]+$/` (L152), record-ness, exposure/toolExposure/enabled/description/timeout types, rejects `type: "sse"` (L270), accepts `type: "streamable-http"` as alias of http (L272), url must parse to http/https, headers string-record, OAuth validation (`validateOAuth` L167–209 incl. cimd constraints L191–200), `auth` requires https-or-loopback (L281–289), stdio branch: args array-of-strings, env string-record, cwd string (L292–303), fallback error L304 |
| L308 | `interface RegisteredMcpServer { name; config; extensionPath }` |
| L316–345 | `export class McpServerRegistry` — `register(server)` (Map.set + changeListener), `unregister(name, extensionPath)` (ownership check L328), `get(name)`, `list()` (registration-order **structuredClone** copies, L339), `setChangeListener(listener)` |

Alias handling: `MCP_EXPOSURE_ALIASES = { "codemode-deferred": "codemode" }` (L57); `resolveExposureAliases` (L221) returns a **copy** with aliases replaced in both `exposure` and every `toolExposure` value.

Lifecycle: pure data; no load/start/stop. Instantiated once per `ExtensionRuntime` in `createExtensionRuntime()` (loader.ts L189).

### 1.2 `core/extensions/types.ts` (2304 LOC)

Responsibility: the entire extension API type surface — events, contexts, tool/command/flag/shortcut registrations, `ExtensionAPI` (L1592), provider registration types, `ExtensionRuntime` (L2265 = `ExtensionRuntimeState & ExtensionActions`), `Extension` (L2268), `LoadExtensionsResult` (L2287), `ExtensionError` (L2299). Runtime values exported: `defineTool` (L690) and the type guards `isBashToolResult` … `isToolCallEventType` (L1343–1403).

Key shapes for Rust:
- `ExtensionFactory = (pi: ExtensionAPI) => void | Promise<void>` (L2045)
- `InlineExtension` (L2047): `{ name, factory, replaceable?, builtin? }`
- `Extension` (L2268): `{ path, resolvedPath, sourceInfo, handlers: Map<string, HandlerFn[]>, tools, messageRenderers, entryRenderers, commands, flags, shortcuts }` (+ optional `markdownTransformer`, `toolRenderers`)
- `ExtensionRuntimeState` (L2154): `flagValues: Map<string, boolean|string>`, `mcpServers: McpServerRegistry`, pending provider/native-provider/virtual-model registration arrays, `assertActive`, `invalidate`, `trackEventBusSubscription`
- `McpServersChangeEvent` (L744): `{ type: "mcp_servers_change", servers: RegisteredMcpServer[] }`

### 1.3 `core/extensions/loader.ts` (876 LOC)

Responsibility: discovery + module loading + `ExtensionAPI` construction with commit/discard semantics and an extension-factory cache keyed by (cwd, generation).

- L41–61: runtime detection (`isNodeSeaBinary`, `isTypeScriptSourceRuntime`, `usesEmbeddedModules`) choosing jiti variant; lazy `getCreateJiti()` / `getVirtualModules()` dynamic imports.
- L69–125: `getAliases()` — module alias map for unbundled Node (typebox, pi-ai compat, etc.). Rust-irrelevant except as documentation of resolution order.
- L129–151: extension cache — `extensionCache: Map<path, ExtensionFactory>`, `extensionCacheCwd`, `extensionCacheGeneration`; `clearExtensionCache()` (L138), `useExtensionCacheCwd(cwd)` (L144) — cache is invalidated when cwd changes.
- L157: `export function createExtensionRuntime(): ExtensionRuntime` — throwing stubs for all action methods; `refreshTools: () => {}` no-op pre-bind; pending-registration queues for providers/virtual models; `invalidate(message?)` (L193) sets a stale message and unsubscribes tracked event-bus subscriptions.
- L244: `function createExtensionAPI(extension, runtime, cwd, eventBus): { api; commit; discard }` — registration methods write to the `Extension` object; action methods delegate to runtime; `applyRuntimeChange` queues changes while `state === "loading"`; `registerMcpServer(name, config)` (L471–488) validates, checks ownership and namespace clashes (`mcpNamespace(server.name) === mcpNamespace(name)`), deep-clones the validated config; `commit()` (L532) flushes pending flags and runtime changes, sets `state = "active"`; `discard()` (L542) unsubscribes and sets `state = "failed"`.
- L559: `async function loadExtensionModule(extensionPath, cacheToken?)` — jiti import, factory type check (`typeof factory !== "function"` → undefined).
- L595 `createExtension(path, resolvedPath)`, L613 `initializeExtension(factory, …)` — calls `await factory(load.api)` then `commit()`, or `discard()` and rethrow on error.
- L634 `loadExtension(...)` → `{ extension: Extension | null, error: string | null }` — resolves path with `normalizeUnicodeSpaces: true`, error string `"Failed to load extension: <message>"`.
- L662 `export async function loadExtensionFromFactory(factory, cwd, eventBus, runtime, extensionPath = "<inline>")`.
- L676 `loadExtensionsInternal(paths, cwd, eventBus?, runtime?, useCache = false)`, L718 `loadExtensions`, L727 `loadExtensionsCached`.
- L749 `resolveExtensionEntries(dir)` — package.json `pi.extensions` manifest paths (via `readPiManifest`), else `index.ts`/`index.js`.
- L791 `discoverExtensionsInDir(dir)` — direct `*.ts`/`*.js` files, or one-level subdirectories with index/manifest; **swallows all readdir errors returning `[]`** (L818).
- L828 `export async function discoverAndLoadExtensions(configuredPaths, cwd, agentDir = getAgentDir(), eventBus?)` — order: (1) `cwd/.pi/extensions`, (2) `<agentDir>/extensions`, (3) configured paths; dedupe by resolved path.

Lifecycle ordering (load): discover → resolve path → jiti import → `createExtensionAPI` → `factory(api)` → `commit()` (pending registrations flush at `bindCore`). Failed factory → `discard()`, error recorded, other extensions continue.

### 1.4 `core/extensions/runner.ts` (1565 LOC)

Responsibility: extension lifecycle post-load, event dispatch, and the bridge into agent-session. This is the highest-risk file for parity.

Module-level exports:
- L169 `export type BeforeAgentStartTrigger`
- L223 `export type ExtensionErrorListener = (error: ExtensionError) => void`
- L236–259 `NewSessionHandler`, `ForkHandler`, `NavigateTreeHandler`, `SwitchSessionHandler`, `ReloadHandler`, `ShutdownHandler` types
- L265 `export async function emitSessionShutdownEvent(extensionRunner, event): Promise<boolean>` — emits only if handlers exist
- L302 `export async function emitProjectTrustEvent(extensionsResult, event, ctx): Promise<{ result?; errors }>` — first non-`undecided` handler wins
- L364 `export class ExtensionRunner`

Constructor: `new ExtensionRunner(extensions, runtime, cwd, sessionManager, modelRegistry)` (L402–415).

Lifecycle methods (ordering matters):
1. `bindCore(actions: ExtensionActions, contextActions: ExtensionContextActions, providerActions?)` (L417–551) — copies action methods into the shared runtime; sets `runtime.mcpServers.setChangeListener` (L465) which emits `mcp_servers_change` and calls `reportUnhandledMcpServers()`; flushes pending provider/native/virtual-model registrations with per-item error capture (emitError, never aborts the loop); swaps `runtime.registerProvider` etc. to direct calls.
2. `bindCommandContext(actions?)` (L553) — newSession/fork/navigateTree/switchSession/reload/waitForIdle handlers.
3. `setUIContext(uiContext?, mode = "print")` (L572) — wraps select/confirm/input/editor/custom in `withUIPrompt` depth tracking that emits `ui_prompt_start`/`ui_prompt_end` via **`queueMicrotask`** (L619).

Query/registry methods: `getExtensionPaths`, `getAllRegisteredTools()` (first registration per tool name wins, L637), `getToolDefinition`, `getFlags`, `setFlagValue`, `getFlagValues`, `getShortcuts(resolvedKeybindings)` (L680 — reserved-keybinding conflict logic with `restrictOverride`; diagnostics collected), `getShortcutDiagnostics`, `invalidate(message?)` (L729), `onError`, `emitError`, `reportUnhandledMcpServers()` (L759 — only when no `mcp_servers_change` handlers; each server reported once via `reportedMcpServers` set), `hasHandlers(eventType)`, `getMessageRenderer`, `getMarkdownTransformers`, `resolveToolRenderers` (chained resolvers, L797), `getEntryRenderer`, `resolveRegisteredCommands()` (L814 — duplicate command names get `name:occurrence` invocation names), `getModelRegistry`, `getRegisteredCommands`, `getCommand(name)`, `shutdown()`, `getActiveTools()`.

Context factories: `createContext()` (L884 — guarded getters checking `assertActive()`), `createToolContext(toolCallId, signal)` (L968 — adds `tools` getter and `executeTool` with nested-call fallback result), `createCommandContext()` (L997 — copies property descriptors so guarded getters stay lazy).

Event emitters (all use `snapshotEventHandlers` = handler-list copy at emit start; all catch handler errors and route them to `emitError` — the emit itself never throws, **except `emitToolCall` L1249 and `emitUserBash` L1269 which rethrow**):
`emitBoundary` (L1036, turn_end/agent_before_settle — entries/continue accumulate, context rebuilt after each handler), generic `emit` (L1096, session_before_* short-circuits on `result.cancel`), `emitCacheWarmingDecision` (L1128, last override wins), `emitMessageEnd` (L1151, role-mismatch → error, message replaced), `emitToolResult` (L1190, per-field merge; content replaced without structuredContent deletes structuredContent, L1204), `emitToolCall` (L1249, `block` short-circuits), `emitUserBash` (L1269, result shape-validated by `isUserBashEventResult` L142), `emitContext` (L1305 — two phases: `context` sees non-system messages and in-place mutation is detected by **reference equality** `sameMessages` L280; system messages restored via `restoreSystemMessages` L292; then `context_with_system` sees everything), `emitBeforeProviderRequest` (L1368), `emitBeforeProviderHeaders` (L1399, in-place mutation only), `emitBeforeAgentStart` (L1427 — lazy `systemPrompt` getter renders current options; `result.systemPrompt` sets `forceSystemPrompt`), `emitResourcesDiscover` (L1479 — collects skill/prompt/theme paths tagged with extension path), `emitInput` (L1525 — transform chain, `handled` short-circuits).

### 1.5 `core/extensions/index.ts` (225), `wrapper.ts` (29), `virtual-modules.ts` (38), jiti loaders (3+3)

- `index.ts`: barrel re-export of ~150 types + `createExtensionRuntime`, `discoverAndLoadExtensions`, `loadExtensionFromFactory`, `loadExtensions`, `ExtensionRunner`, `wrapRegisteredTool(s)`, `defineTool`, type guards.
- `wrapper.ts` L242: `export function wrapRegisteredTool(registeredTool: RegisteredTool, runner: ExtensionRunner): AgentTool` — `wrapToolDefinition(registeredTool.definition, (toolCallId, signal) => runner.createToolContext(toolCallId, signal))`; L252 `wrapRegisteredTools`.
- `virtual-modules.ts` L274: `VIRTUAL_MODULES` — module map injected into jiti for bundled runtimes (typebox, pi-agent-core, pi-tui, pi-ai compat/oauth/providers, pi-coding-agent, @mariozechner aliases).
- `jiti-loader.ts` / `jiti-static-loader.ts`: `export { createJiti } from "jiti"` / `"jiti/static"` — chosen by `getCreateJiti()` in loader.ts.

### 1.6 `extensions/index.ts` (14 LOC)

```ts
export const builtInExtensions: InlineExtension[] = [
  { name: "llama.cpp", factory: llamaExtension, builtin: true },
  { name: "codemode", factory: codemodeExtension, replaceable: true, builtin: true },
  { name: "tool-search", factory: toolSearchExtension, replaceable: true, builtin: true },
  { name: "mcp", factory: mcpExtension, replaceable: true, builtin: true },
];
```

### 1.7 `extensions/mcp/` — the built-in MCP extension (3906 LOC)

#### `index.ts` (1347)
`export function createMcpExtension(options: McpExtensionOptions = {}): ExtensionFactory` (L279); `export default createMcpExtension()`. Options (L69–95): `loadConfig?`, `createTransport?: McpTransportFactory`, `credentials?`, `logPath?`, `openUrl?`, `updateConfig?`, `startupWaitMs?` (default 10 000). Internal `McpServer` state per entry (L108–122): `entry`, `connection?`, `registeredConfig?` (JSON string to detect re-registration), `message?`, `attempt?: symbol`, `ready?: Promise<void>`, `closing?: Promise<void>`.

Exports for parity: `MCP_SERVERS_SECTION = "mcp_servers"` (L156), `MAX_SERVERS_SECTION_CHARS = 4096` (L163), `McpServerListing` (L179), `renderServersSection(servers): string | undefined` (L195 — the `mcp_servers` system-prompt section builder with `serverSummary`, `truncate`, `reachesServer` filtering).

Event wiring inside the factory: `session_start` (L1114 — starts connections in background, `Promise.all(enabled.map(startConnection))` L1141), `before_agent_start` (L1178 — injects the `mcp_servers` prompt section), `tool_call` (L1191 — routes permission handling to per-server tool metadata), `turn_start` (L1212 — waits for connecting direct-tool servers up to `startupWaitMs`), `mcp_servers_change` (L1217 — connects newly registered extension servers right away), `session_shutdown` (L1252 — closes connections), `registerCommand("mcp", …)` (L1260 — the `/mcp` manager). Tool registration on connect: `registerTools(connection)` (L387) builds `createMcpToolDefinition` per server tool with `getMcpToolExposure`, registers hidden placeholders, activates codemode/tool_search depending on exposure (L513–525).

#### `config.ts` (242)
- L51 `interface McpServerEntry { name; config; source: string; scope?: "global"|"project"|"extension"; override?: string }`
- L65 `interface LoadedMcpConfig { servers; autoEnableCodemode?; errors: string[]; projectConfig?: string }`
- L77 `isOverride(value)` — project entry with no `command`/`url`/`type` overrides only `enabled`/`exposure`/`toolExposure` (`OVERRIDE_KEYS` L74) of the same-named global server; merged as `{ ...base.config, ...value }` then re-validated (L116).
- L91 `readConfigFile(path, scope, state)` — `JSON.parse(readFileSync(path, "utf8"))` with error string `` `${path}: ${message}` ``; top-level shape check; `autoEnableCodemode` boolean check; per-entry validation with namespace-clash detection (L128) and the project-`auth` rejection (L133).
- L145 `export function loadMcpConfig(options: { agentDir: string; cwd: string; projectTrusted: boolean }): LoadedMcpConfig`
- L159 `interface McpServerConfigPatch { enabled?; exposure? }`
- L169 `updateMcpServerConfig(path, name, patch, options?)` — default-valued patches **delete** the key (`patch.enabled === true` deletes `enabled` unless the entry is an override); via `editMcpServers`.
- L199 `addMcpServerConfig(path, name, config): boolean`, L212 `removeMcpServerConfig(path, name): boolean`
- L228 `editMcpServers(path, edit)` — reads (missing file → `{}`), runs edit on `parsed.mcpServers`, writes back with the file's detected indentation (`/^([ \t]+)\S/m` → captured prefix, else two spaces) via `JSON.stringify(parsed, null, indent)` + trailing `\n`; `mkdirSync(dirname, { recursive: true })`.

#### `runtime.ts` (454)
- L59 `export type McpTransportFactory = (entry: McpServerEntry, cwd: string, authProvider: AuthProvider | undefined) => McpTransport`
- L98 `export function createDefaultTransport(entry, cwd, authProvider)` — `StreamableHttpTransport` for url servers (headers via `resolveHeadersOrThrow`), `StdioTransport` for command servers (env values via `resolveConfigValueOrThrow`, `expandHome` for command/args/cwd L90–96, `stderr: "pipe"`).
- L139 `export class McpServerConnection implements McpToolCaller, McpResourceServer` — state machine `"connecting"|"connected"|"disconnected"|"needs-auth"|"failed"|"closed"`; lazy single-flight `getClient()` (`this.opening ??= this.open().finally(...)`, L237–244); `withClient` (L278) — read-only requests retried once after a transient HTTP error (`isTransientError` L70: 408/429/5xx-except-501 or `TypeError`); `McpSessionExpiredError` retries once on a fresh session; needs-sign-in errors drop the client, mark `needs-auth`, and throw `` `MCP server "…" requires sign-in. Run /mcp to sign in.` ``; `open()` retries url servers with delays `[250, 1000]` ms; `connectOnce()` (L359) creates `McpClient` (name "pi", `VERSION`, `requestTimeoutMs`, roots `[{ uri: pathToFileURL(cwd).href, name: basename(cwd) }]`), subscribes `notifications/message` → log, `notifications/tools/list_changed` → `refreshTools`, records stderr tail (last 2000 chars) on failure; `close()` aborts the shutdown controller and awaits `authProvider.settled()` (L443–453).
- `McpToolCaller` surface: `callTool(name, args, options: McpRequestOptions)`.
- `McpResourceServer` surface: `resourcesPage`, `resourceTemplatesPage`, `allResources`, `allResourceTemplates`, `readResource`.

#### `log.ts` (69)
- L20 (in-file) `export function formatMcpLogMessage(server: string, params: unknown, now: Date = new Date()): string` — `params` record → `level` (default `"info"`), optional `logger` suffix, `formatData` (`typeof data === "string" ? data : JSON.stringify(data) ?? String(data)`, stringify failure → `String(data)`), continuation lines indented 4 spaces, timestamp `now.toISOString()`.
- `export class McpServerLog` — synchronous appends to `mcp.log`, rotates to `mcp.log.1` past 5 MB, **all errors swallowed** (best-effort logging).

#### `tools.ts` (391)
- L44 `export function toToolExposure(exposure: McpExposure): ToolExposure` — `codemode → deferred`, else unchanged.
- L51 `MCP_OUTPUT_MAX_BYTES = 20 * 1024`; L57 `McpToolDetails { server, tool, fullOutputPath? }`; L68 `McpOutputSaver = (data: string | Uint8Array, extension: string) => Promise<string>`; L70 `saveToTempFile` via `writeOutputFile("pi-mcp", …)`.
- L84 `export function createMcpToolName(server, tool, isTaken = () => false): string` — `` `mcp__${server}__${tool}`.replace(/[^A-Za-z0-9_]/g, "_") ``, ≤64 chars; else sha256(`${server}\0${tool}`) hex prefix (8 chars) with `name.slice(0, 64 - 8 - 1)}_${hash}`.
- L135 `relocateSchema(schema, pointer)` — rewrites local `$ref`s under a JSON pointer, respecting data keywords (`const/enum/default/examples`) and map keywords (`properties/patternProperties/dependentSchemas/dependencies/$defs/definitions`).
- L159 `createMcpResultSchema(structuredContentSchema)` — CallToolResult output schema with `content` blocks, `structuredContent`, `isError`, `required: ["content"]`, `additionalProperties: true`.
- L180 `export async function limitMcpContent(content, saveOutput = saveToTempFile)` — Codex-style middle truncation (`truncateMiddle`), token estimate `Math.ceil(totalBytes / 4)`, format `Warning: truncated output (original token count: N)\nTotal output lines: M\n\n…\n\n[Full output: path (read it with offset/limit)]`; images kept after the text.
- L224 `blockToContent` — resource_link → text description; embedded text/blob resource → text (text/*, json, +xml mime types) or temp-file save with extension from URI (`/\.[A-Za-z0-9]{1,8}$/` else `.bin`); other blocks via `toLlmContent`.
- L257 `toModelContent(server, blocks, options)`; L266 `convertMcpResult(server, tool, result, options): Promise<AgentToolResult<McpToolDetails>>` — `_meta` stripped from the script-visible structured result; `isError` results keep the structured result and gain `isError: true`; empty error text gets `` `MCP tool ${server}/${tool} returned an error` `` appended.
- L292 `toParameters(schema)` — forces `type: "object"` and `properties: {}` defaults.
- L303 `toToolAnnotations` — boolean hints readOnlyHint/destructiveHint/idempotentHint/openWorldHint.
- L312 `export function createMcpToolDefinition(options: { server; tool: McpTool; name; exposure; namespace; timeoutMs; getClient; readableResources? }): ToolDefinition` — `execute` calls `client.callTool(tool.name, params, { signal, timeoutMs, onProgress })` (progress → `onUpdate` text), converts via `convertMcpResult`.
- L354 `createMcpToolRenderers(label)` — TUI renderers (pi-tui side).

#### `resources.ts` (339)
- L427 `interface McpResourceServer` (name, timeoutMs, page/all/read methods).
- L438 `isMcpAppResource` — `ui://` URIs or `profile=mcp-app` mime filtered out.
- L444 `listed(server, item)` — strips `_meta` and `icons`, prepends `server`.
- L585 `export function createMcpResourceToolDefinitions(options: { exposure; servers: () => readonly McpResourceServer[] }): ToolDefinition[]` — the three Codex-compatible resource tools; `list` handles one page of one server or `Promise.allSettled` over all servers sorted by `name.localeCompare`, collecting per-server `errors`; `readResource` labels multi-content results with URIs; all outputs JSON via `jsonResult` + `limitMcpContent`; `stringArgument` trims and rejects non-strings (`"server must be provided"` / `"uri must be provided"` / `"cursor can only be used when a server is specified"`).

#### `oauth.ts` (558)
- L58 `McpOAuthSettings`; L87 `callbackSettings`; L109 `mergeScopes`; L116 `parseStates` / L122 `serializeStates` (2-space JSON + newline); L130 `storeKeys` — key `` `${mcpNamespace(name)}|${String(new URL(serverUrl))}` `` plus legacy URL-only key migrated on first load.
- L141 `export class McpOAuthCredentialStore` — `forServer(name, serverUrl): McpOAuthServerStore` with `load`/`save`/`withRefreshLock` (proper-lockfile per-server lock file `mcp-auth-refresh-<sha256(key)[:16]>`, stale after 20 s, waits 25 s).
- L227 `timedFetch` (15 s bound); L239 `callbackId`; L251 `clientMetadataDocument` (pi.dev CIMD); L273 `createProvider`; L295 `McpAuthProvider extends AuthProvider`; L313 `createMcpAuthProvider`; L376 `McpSignInPrompt`; L387 `McpSignInCancelledError`; L473 `signInMcpServer` — PKCE authorization-code flow on a loopback callback server (host 127.0.0.1, path `/callback`).

#### `ui.ts` (259) / `cli.ts` (594) / lazy files (2+2)
- `ui.ts`: `McpManagerView` (TUI component), `showMcpManager` — pi-tui territory.
- `cli.ts`: `runMcpCommand(args, options): Promise<number>` (L185) with subcommands add/remove/list/login; `parseOptions`, `parsePairs` — `pi mcp` CLI.
- `runtime.lazy.ts` / `cli.lazy.ts`: `loadMcpRuntime = () => import("./runtime.ts")`, `loadMcpCommand = () => import("./cli.ts")` — dynamic import seams.

### 1.8 `extensions/codemode/` (1390 LOC)

- `index.ts` (49): `createCodemodeExtension(options)` — registers the `codemode` tool `defaultActive: false`; reads `codemode.mode` / `codemode.inlineBudget` settings.
- `tool.ts` (411): `CODEMODE_TOOL_NAME = "codemode"`, `CODEMODE_STORE_ENTRY_TYPE = "codemode-store"`, `codemodeSchema = Type.Object({ code: Type.String(…) })`, `CodemodeToolOptions`, `CodemodeNestedCall`, `CodemodeToolDetails`, `createCodemodeToolDefinition` (description built from docs, `getCodemodeCallableTools`, `toCodemodeDeclaration`), `CODEMODE_DOCS_PATH`.
- `execute.ts` (772): QuickJS sandbox execution (256 MB heap limit, max 2 concurrent workers, `// @options:` parsing, `store()`/`load()` via session entries, nested tool calls through `ctx.executeTool`, `models.*` with max 4 concurrent calls, BM25 via tool-search).
- `worker.ts` (6), `execute.lazy.ts` (2), `renderer.ts` (156, TUI).

### 1.9 `extensions/tool-search/` (262 LOC)

- `index.ts` (18): `createToolSearchExtension()` — registers `tool_search` inactive.
- `tool.ts` (244): `TOOL_SEARCH_TOOL_NAME = "tool_search"`, `DEFAULT_TOOL_SEARCH_LIMIT = 8`, `ToolSearchDocument`, `ToolRanker`, `tokenize(text)` (camelCase split, lowercase, non-alnum split, stop-word list, naive stemmer: `ies→y`, `ches|shes|sses|xes|zes→-2`, trailing `s` unless `ss`), `createToolSearchDocument(tool, namespace?)` (name, name with `_`→space, description, recursive schema descriptions/property names), `Bm25Ranker` (k1/b params as in the file), `createToolSearchToolDefinition`.

### 1.10 `extensions/llama/` (1577 LOC)

`index.ts` (230) default-exports `llamaExtension(pi)` — registers the `llama.cpp` native provider, `/llama` command with model load/unload flows; `client.ts` (348) `LlamaClient` (llama.cpp server REST + SSE progress); `provider.ts` (299) `createLlamaProvider()` / `LLAMA_PROVIDER_ID = "llama.cpp"`; `huggingface.ts` (158) HF API client + token discovery; `ui.ts` (542) TUI. Network-heavy; lowest parity priority.

### 1.11 Config types consumed and agent-session exposure

From root `config.ts`: `CONFIG_DIR_NAME` (L581), `getAgentDir()` (L605), `APP_NAME` (L579), `VERSION` (L582), `getDocsPath()` (L449), `getQuickJSWasmPath`/`getCodemodeWorkerSpecifier` (codemode), `isBunBinary`/`isBundledNode` (loader runtime detection). From core: `resolvePath` (utils/paths), `createEventBus` (event-bus), `execCommand` (exec), `readPiManifest` (pi-manifest), source-info, `time` (timings), virtual-models, ModelRegistry, SessionManager, system-prompt builders, settings, `resolveConfigValueOrThrow`/`resolveHeadersOrThrow` (resolve-config-value), `AuthStorageBackend`/`FileAuthStorageBackend` (auth-storage), `openBrowser`, `writeOutputFile`, `truncateMiddle`/`formatSize`, render-utils.

`agent-session.ts` consumes: `createToolNameMatcher`, `isMcpToolName` (L116); constructs `ExtensionRunner(extensionsResult.extensions, extensionsResult.runtime, cwd, sessionManager, new ModelRegistry(...))` (L3662–3672), `_bindExtensionCore`, `_applyExtensionBindings`, flag-value seeding from options (L3661), `emitSessionShutdownEvent` + `invalidate` on reload (L3699–3701), `reportUnhandledMcpServers()` (L3717), and keeps MCP tools when `allowedToolNames` is empty or contains `mcp__`-prefixed entries (L506).

---

## 2. JS-semantics traps for Rust

1. **Union-error returns, not exceptions.** `validateMcpServerConfig(name, raw): McpServerConfig | string` returns the error *message* as a string; call sites do `if (typeof config === "string") errors.push(...)`. Rust should mirror as `Result<McpServerConfig, String>` but the harness must serialize exactly the same message strings (quotes, punctuation, `server "name": …` prefixes).
2. **undefined vs null vs absent.** All config validation branches on `!== undefined`; JSON `null` for `enabled` fails the boolean check and produces the same error as a wrong type. Rust model: `Option<T>` with a `JsonNull` distinction only where the TS checks `typeof` — here `null` fails `typeof x === "boolean"`, so plain `Option` + type check is faithful. In `emitToolResult`, `delete currentEvent.structuredContent` when content is replaced without structuredContent (L1204) — field absence, not null.
3. **Object key order and JSON rewrite parity.** `editMcpServers` rewrites `mcp.json` with `JSON.stringify(parsed, null, indent)` where `indent` is sniffed from the first indented line (`/^([ \t]+)\S/m`, default `"  "`). New keys append at the end. Rust needs `serde_json` with the **`preserve_order`** feature and must replicate JS JSON.stringify formatting: 2-space-equivalent indentation, no trailing spaces, `"` escaping rules (JS escapes only `"` `\` and control chars as `\b \t \n \f \r` or `\uXXXX`), and the trailing newline. Duplicate JSON keys: JS last-wins — serde_json `Value` also last-wins; OK.
4. **Spread-copy alias resolution.** `resolveExposureAliases` builds `{ ...value }` then conditionally reassigns `exposure`/`toolExposure` — key order preserved from the original. `getMcpToolExposure` iterates `Object.entries(toolExposure)` in insertion order (first matching `*` pattern wins). Rust: `IndexMap`/preserve_order maps.
5. **Map insertion-order semantics.** `McpServerRegistry.servers` is a JS `Map`; re-`set` of an existing key keeps its original position. `list()` order = registration order. Rust `IndexMap::insert` matches this behavior.
6. **structuredClone deep copies** in `McpServerRegistry.list()` and `registerMcpServer` — the Rust registry must hand out deep clones (serde_json `Value` clone or explicit clone) so callers cannot mutate registered state.
7. **Reference-identity mutation detection.** `emitContext` detects in-place edits of `event.messages` by comparing element references (`sameMessages`, L280/1313–1320). In Rust this becomes explicit: handlers receive the message list and may return `None` (unchanged) or `Some(messages)`; the "edited in place" path has no Rust equivalent — model handler results as `Option<Vec<Message>>` and document the divergence (the observable behavior — changed vs unchanged — is preserved).
8. **Symbol-tagged attempts and single-flight promises.** `attempt?: symbol` invalidates stale connection work; `this.opening ??= this.open().finally(…)` memoizes. Rust: a monotonically increasing `u64` attempt id and an `Option<Shared<JoinHandle>>`/flag; no Promise needed if the transport is sync under a trait.
9. **Error swallowing.** Systematic patterns to mirror deliberately: `catch { return [] }` (discoverExtensionsInDir L818), `client.close().catch(() => undefined)` (runtime.ts L337/399/449), McpServerLog ignores all write errors, `JSON.stringify(data) ?? String(data)` and the try/catch → `String(data)` in `formatData`. In Rust these become `Result` mapped to defaults with **identical fallback values**; never propagate.
10. **queueMicrotask emission** of ui_prompt events (runner.ts L619) — deferred-to-after-current-task ordering. Rust: a queued-callback list drained at defined points; parity harness should assert event *sets*, and where order matters (ui_prompt_start before handler-visible state changes) drain after the emit call returns.
11. **Dynamic imports as lazy seams** (`runtime.lazy.ts`, `cli.lazy.ts`, `execute.lazy.ts`, `getCreateJiti`, `getVirtualModules`). In Rust the equivalent is `OnceLock`/lazy static or simply not loading the heavyweight backend unless needed; parity-wise these are invisible.
12. **jiti/TS extension loading is JS-engine work.** `jiti.import(extensionPath, { default: true })` executes arbitrary TS/JS factories. The Rust port cannot use jiti; extension *code execution* is out of scope for this phase (see §3) — port the discovery/registry/commit-discard machinery around a `Box<dyn ExtensionFactory>` trait so built-in Rust extensions and (later) a JS-engine-backed loader plug in identically.
13. **URL semantics.** `URL.canParse`, `url.hostname` returning `"[::1]"` for IPv6 literals (LOOPBACK_HOSTS includes the bracketed form), `url.port` as a string, empty-string query/hash checks. Use the `url` crate and replicate `URL.canParse` acceptance exactly (WHATWG spec). `pathToFileURL(cwd)` for MCP roots.
14. **UTF-16 string operations.** `createMcpToolName`'s `.replace(/[^A-Za-z0-9_]/g, "_")` and `slice` operate on UTF-16 code units: a non-BMP character (surrogate pair) becomes **two** underscores in JS but would be one `_` in naive Rust. Use `js_length`/code-unit iteration (existing helpers in `pi-tui/src/utils.rs` / `pi-core`). Same class of issue for `truncateMiddle` byte counting (that one is byte-based already — keep it byte-based).
15. **Regex construction from user input.** `toolPatternRegExp` escapes `.+?^${}()|[]\` but intentionally leaves `*` (wildcard) unescaped — a user pattern like `"a*b**c"` yields `a.*b.*.*c`. Port by building the same escaped string and compiling with the `regex` crate; note JS regex and Rust `regex` differ on empty-alternation and backtracking edge cases, but the generated patterns here are simple (`^` + literal/`.*` mix + `$`) so behavior matches. `SERVER_NAME = /^[A-Za-z0-9_-]+$/` anchors matter.
16. **Number checks.** `timeout > 0` (NaN and 0 rejected), `Number.isInteger(port) && 1..=65535`, `Math.ceil(totalBytes / 4)`. `structuredClone` of numbers preserves NaN — but validation rejects NaN for timeout.
17. **`Date.toISOString()`** in `formatMcpLogMessage` — use the existing JS-epoch helper conventions (pi-core `parse_date_millis` family); the harness must pass a fixed `now`.
18. **First/last-wins rules per event type.** project_trust: first decided wins; cache-warming: last override wins; tool_call: first `block` wins; message_end: last valid message wins with role check; commands: `name:occurrence` disambiguation with `takenInvocationNames` retry loop (L834–840). Each must be ported individually — there is no single rule.
19. **`type: "streamable-http"` and `"codemode-deferred"` aliases** are accepted silently and normalized (alias map + `resolveExposureAlias`), while `type: "sse"` is rejected with a specific message.
20. **Namespace clash detection** compares `mcpNamespace(other) === mcpNamespace(name)` — `-` and `_` collide. Appears in three places (mcp-servers via loader registerMcpServer L482–485, mcp/config readConfigFile L128, and validate side). Keep the exact error strings.
21. **JSON.stringify of `structuredContent` in resource tools** (`jsonResult`) — key order = insertion order of the payload construction; preserve_order serde_json matches.

---

## 3. Pure-logic vs I/O split

### Pure (parity-testable with fakes, no process/network)

| TS source | Pure parts | Fake seam |
|---|---|---|
| mcp-servers.ts | **entire file** — pattern matching, validation, exposure resolution, namespace, `McpServerRegistry` | none needed (registry is in-memory) |
| mcp/config.ts | parsing/validation/merging of already-read file text; patch semantics; the *decision* of what to write | read/write via an `McpFs` trait (string in/string out) so tests feed file contents directly |
| mcp/tools.ts | `createMcpToolName`, `relocateSchema`, `createMcpResultSchema`, `toParameters`, `toToolAnnotations`, `limitMcpContent`, `blockToContent`/`toModelContent`, `convertMcpResult` | `McpOutputSaver` fake returning fixed paths (mirrors `BashOperations`) |
| mcp/resources.ts | listing/read orchestration, filtering, payload construction, error collection | `McpResourceServer` fake with canned pages/errors |
| mcp/log.ts | `formatMcpLogMessage` | inject `now: Date` |
| mcp/index.ts | `renderServersSection`, exposure bookkeeping, activate-codemode/tool_search decisions, `describeState`/`attentionRank` ranking | feeds on `McpServer` state snapshots |
| loader.ts | discovery rules (`resolveExtensionEntries`, `discoverExtensionsInDir`), cache cwd/generation logic, `createExtensionAPI` commit/discard/pending queues, `registerMcpServer` validation + ownership/clash checks | `FsList` trait for directory reads; `ExtensionFactory` trait instead of jiti |
| runner.ts | all `emit*` pipelines, command/shortcut resolution, `reportUnhandledMcpServers`, context factories | handlers are trait objects/closures supplied by tests |
| tool-search/tool.ts | **entire file** — tokenize, stem, BM25, document building | none |
| oauth.ts | `callbackSettings`, `mergeScopes`, `parseStates`/`serializeStates`, `storeKeys` | storage backend trait |

### I/O (inherently process-spawning / network)

- `StdioTransport` — spawns the MCP server process (command + args + env with `${VAR}`/`!cmd` resolution + cwd). Trait: `McpTransport` with a scripted in-memory fake for parity; real impl spawns and speaks newline-delimited JSON-RPC over stdin/stdout with a reader thread (mirrors the existing `BashOperations` + `bash_executor` pattern in pi-tools).
- `StreamableHttpTransport` — HTTP + SSE; defer behind the same `McpTransport` trait.
- OAuth loopback callback server, `proper-lockfile` refresh locks, browser opening — behind `OAuthFlow` / `LockDir` traits; parity covers only the state serialization/keys.
- `McpServerLog` file appends/rotation — `LogSink` trait, fake collects lines.
- `LlamaClient` / `HuggingFaceClient` — full HTTP; defer entirely.
- codemode QuickJS sandbox — needs a JS runtime (candidate: the same engine chosen for extension loading); defer, keep `CodemodeExecutor` trait.

### Async story

MCP is JSON-RPC over stdio, but **parity testing does not need async**. Every behavior under test is expressible as request → response with canned fakes:

- Model the transport as a **sync trait**: `fn request(&mut self, json: &str) -> Result<String, McpErrorKind>` plus `fn notifications(&mut self) -> Vec<String>` drained by the fake. A scripted fake replies from a queue; error kinds (`transient-http(status)`, `session-expired`, `auth-required`, `method-not-found`) drive the retry/needs-auth state machine deterministically.
- The real stdio transport uses a reader thread + mpsc channel (exactly like `pi-tools::bash_executor`); the connection object drives it synchronously with a timeout — matching the TS `requestTimeoutMs` semantics without an async runtime.
- Timers (`CONNECT_RETRY_DELAYS_MS`, startup wait) must be injectable (`SleepFn`/clock trait) so parity cases don't actually sleep.
- Conclusion: no tokio decision is forced by this phase. Keep pi-core/pi-session sync; if tokio is adopted later, only `StdioTransport`/`StreamableHttpTransport` implementations change, not the trait surface.

---

## 4. Proposed Rust module placement

Guiding rules: pure logic in `pi-core`; session-coupled logic in `pi-session`; process/I/O backends in `pi-tools`-style traits; user-facing built-in extensions in a new `pi-extensions` crate (avoids pi-cli ↔ pi-session cycles and keeps parity harness dispatch thin).

```
pi-core/src/mcp_servers.rs                 (~550 LOC)
    pub enum McpExposure { Codemode, Deferred, Direct, Hidden }
    pub struct McpStdioServerConfig / McpHttpServerConfig / McpOAuthConfig
    pub enum McpServerConfig { Stdio(..), Http(..) }        // serde with type alias handling
    pub fn tool_pattern_regexp(pattern: &str) -> String     // escaped source, compiled lazily
    pub fn create_tool_name_matcher(entries: &[String]) -> impl Fn(&str) -> bool
    pub fn is_mcp_tool_name(name: &str) -> bool
    pub fn is_loopback_redirect_uri(value: &str) -> bool
    pub fn mcp_namespace(server: &str) -> String
    pub fn get_mcp_tool_exposure(config: &McpServerConfig, tool_name: &str) -> McpExposure
    pub fn validate_mcp_server_config(name: &str, raw: &serde_json::Value) -> Result<McpServerConfig, String>
    pub struct RegisteredMcpServer { pub name: String, pub config: McpServerConfig, pub extension_path: String }
    pub struct McpServerRegistry { /* IndexMap */ }
        impl: register / unregister / get / list (deep clone) / set_change_listener(Box<dyn Fn()>)

pi-core/src/mcp_tools.rs                   (~650 LOC)   // pure parts of extensions/mcp/tools.ts
    pub fn to_tool_exposure(McpExposure) -> ToolExposure
    pub const MCP_OUTPUT_MAX_BYTES: usize
    pub fn create_mcp_tool_name(server: &str, tool: &str, is_taken: impl Fn(&str) -> bool) -> String
    pub fn relocate_schema(schema: &Value, pointer: &str) -> Value
    pub fn create_mcp_result_schema(structured: Option<&Value>) -> Value
    pub fn to_parameters(schema: &Value) -> Value
    pub fn to_tool_annotations(tool: &McpTool) -> Option<ToolAnnotations>
    pub type McpOutputSaver = Arc<dyn Fn(&OutputData, &str) -> Result<String, String>>
    pub fn limit_mcp_content(content: &[ContentBlock], save: &McpOutputSaver) -> Result<LimitedContent, String>
    pub fn to_model_content(server: &str, blocks: &[ContentBlock], opts: ConvertOptions) -> Vec<ContentBlock>
    pub fn convert_mcp_result(server: &str, tool: &str, result: &CallToolResult, opts) -> AgentToolResult<McpToolDetails>
    pub trait McpToolCaller { fn call_tool(&self, name: &str, args: &Value, opts: &RequestOptions) -> Result<CallToolResult, McpErrorKind>; }

pi-core/src/mcp_resources.rs              (~450 LOC)   // resources.ts logic
    pub trait McpResourceServer { name / timeout_ms / resources_page / … / read_resource }
    pub fn create_mcp_resource_tool_definitions(exposure, servers: Box<dyn Fn() -> Vec<Rc<dyn McpResourceServer>>>) -> Vec<ToolDefinition>

pi-core/src/tool_search.rs                 (~350 LOC)  // tool-search/tool.ts
    pub fn tokenize(text: &str) -> Vec<String>
    pub fn create_tool_search_document(tool: &ToolInfo, namespace: Option<&ToolNamespace>) -> ToolSearchDocument
    pub struct Bm25Ranker;  impl ToolRanker

pi-session/src/extensions/mod.rs           (new submodule, ~3600 LOC total)
    types.rs      — Extension, RegisteredTool/Command/Flag/Shortcut, event enums, ExtensionError
                   (~1300; port as Rust enums + serde where harness-visible)
    registry.rs   — ExtensionRuntime equivalent: flag values, McpServerRegistry holder,
                   pending provider/virtual-model queues, invalidate/assert_active (~300)
    api.rs        — ExtensionApi: registration methods writing into an Extension,
                   commit()/discard() with pending queues (~450)
    loader.rs     — discovery (resolve_extension_entries, discover_extensions_in_dir) behind
                   a DirectoryLister trait; load_extension_from_factory; extension cache
                   with cwd/generation (~450)
    runner.rs     — ExtensionRunner: bind_core / bind_command_context / set_ui_context /
                   all emit* pipelines / context factories / report_unhandled_mcp_servers (~1500)

pi-session/src/mcp_config.rs               (~400 LOC)  // extensions/mcp/config.ts
    pub struct McpServerEntry / LoadedMcpConfig / McpServerConfigPatch
    pub trait McpConfigFs { fn read(&self, path: &str) -> Option<String>; fn write(&self, path: &str, text: &str); fn exists(&self, path: &str) -> bool; }
    pub fn load_mcp_config(opts: {agent_dir, cwd, project_trusted}, fs: &dyn McpConfigFs) -> LoadedMcpConfig
    pub fn update_mcp_server_config / add_mcp_server_config / remove_mcp_server_config (…, fs)
    fn edit_mcp_servers(path, edit, fs)   // indent sniffing + JSON.stringify-compatible writer

pi-extensions (new crate)                  (~1800 LOC for phase 3)
    src/lib.rs        — built_in_extensions() equivalent
    src/mcp/mod.rs    — create_mcp_extension(options): wiring of session_start/turn_start/
                        mcp_servers_change handlers against pi-session's runner; render_servers_section
    src/mcp/log.rs    — format_mcp_log_message + McpServerLog behind LogSink (~120)
    src/mcp/runtime.rs— McpServerConnection state machine behind trait McpTransport + SleepFn (~700)
    src/mcp/transport.rs — trait McpTransport { fn connect(&mut self, hello: &InitializeRequest) -> Result<ServerInfo, McpErrorKind>; fn request(...); fn take_notifications(); fn stderr_tail(); }
    src/mcp/fake.rs   — scripted in-memory McpTransport for parity/tests (~250)
    src/tool_search.rs, src/codemode/mod.rs (declaration + deferred CodemodeExecutor trait), src/llama/ (deferred)
```

`pi-cli/src/main.rs` gains harness dispatch arms (following the existing `"compaction" => …` style, main.rs L161–185).

### Async story (restated as a decision)

- Phase 3 target: **sync traits everywhere** (`McpTransport`, `McpToolCaller`, `McpConfigFs`, `SleepFn`). Parity harness runs single-threaded with scripted fakes.
- `McpServerConnection`'s single-flight/retry/reconnect logic is ported as a synchronous state machine driven by explicit calls; background connection in the real binary runs on a `std::thread` (like `bash_executor`), leaving the trait surface unchanged if tokio is adopted later.

---

## 5. Parity harness tools

Following existing conventions (`parity/ts-harness.js` `{tool, args}` JSON on stdin, in-band `{error}` results; Rust dispatch `pi harness tool <name>`; cases in `parity/cases.mjs`; run via `node parity/run-parity.mjs`):

| Harness tool name | Covers | Args (JSON) → Result (JSON) |
|---|---|---|
| `mcp-servers` | validate_mcp_server_config, create_tool_name_matcher, is_mcp_tool_name, is_loopback_redirect_uri, mcp_namespace, get_mcp_tool_exposure, McpServerRegistry ops | `{op:"validate", name, raw}` → `{result: <config>}` or `{error: "<msg>"}`; `{op:"matcher", entries, names}` → `{result: [bool…]}`; `{op:"isMcpToolName"/"isLoopbackRedirectUri"/"namespace"/"toolExposure", …}` → `{result}`; `{op:"registry", steps:[{register|unregister|list}, …]}` → `{result: [...]}` |
| `mcp-config` | load_mcp_config, update/add/remove | `{op:"load", files:{"<path>": "<content>"}, agentDir, cwd, projectTrusted}` → `{result: {servers, autoEnableCodemode?, errors, projectConfig?}}`; `{op:"update"/"add"/"remove", files, path, name, patch/…}` → `{result: {replaced/removed, files: {<path>: <new content>}}}` |
| `mcp-tools` | name/schema/result conversion | `{op:"toolName", server, tool, taken?}` → `{result}`; `{op:"resultSchema", structuredContentSchema}` → `{result}`; `{op:"convert", server, tool, result, saverPath?}` (fake saver returns fixed `"/tmp/pi-mcp/x.txt"`) → `{result: AgentToolResult}`; `{op:"limit", content, saverFails?}` → `{result}`; `{op:"parameters"/"annotations"}` |
| `mcp-resources` | resource tool execute | `{op:"listResources"/"listTemplates"/"read", params, servers:[{name, timeoutMs, script:[…canned results/errors…]}]}` → `{result}` or `{error}` |
| `mcp-log` | format_mcp_log_message | `{server, params, now: "<ISO>"}` → `{result: "<line>"}` |
| `mcp-connection` | McpServerConnection state machine | `{entry, cwd, script:[{reply:"<json>"}, {error:"transient:429"}, {error:"auth"}], ops:[{callTool:{name,args}}, {reconnect}, {close}]}` → `{result: {states:[…], errors:[…], calls:[…requests sent…]}}` |
| `extension-loader` | discovery + API commit/discard | `{op:"discover", files:{"<dir>":[entries]}, configuredPaths, cwd, agentDir}` → `{result: [paths]}`; `{op:"factory", registrations:[…], failAt?}` → `{result: {extension | error, runtimePending, registryList}}` |
| `extension-runner` | emit pipelines | `{handlers:[{event, behavior:"ok"/"transform"/"throw"/"block", result?}], emit:{type, event…}}` → `{result: <emit return>, errors:[ExtensionError]}` (one sub-op per emit method: `context`, `toolResult`, `messageEnd`, `beforeAgentStart`, `resourcesDiscover`, `input`, `generic`) |
| `tool-search` | tokenize/BM25/documents | `{op:"tokenize", text}` → `{result:[terms]}`; `{op:"rank", query, tools:[…], limit}` → `{result:[{name,score}]}`; `{op:"document", tool, namespace?}` → `{result:{name,text}}` |
| `mcp-section` | render_servers_section | `{servers:[McpServerListing], reaches?}` → `{result: "<section>" \| null}` |

Errors always in-band as `{"error": "<exact TS message>"}` so string parity is diffable.

---

## 6. Dependencies and porting order

Unported dependencies (not yet in pi-rs) and their status/impact:

| Dependency | Needed by | Notes |
|---|---|---|
| `@earendil-works/pi-mcp` (McpClient, transports, oauth) | mcp/runtime.ts | Replaced by our `McpTransport` trait + protocol types; JSON-RPC types (Tool, CallToolResult, ContentBlock) become pi-core structs |
| jiti | loader.ts | Replaced by `ExtensionFactory` trait; JS loading deferred |
| event-bus | loader/runner | small; port as `EventBus` in pi-session (channel → handler map) |
| pi-manifest (`readPiManifest`) | resolveExtensionEntries | package.json `pi.extensions` parsing — small pure module |
| source-info, virtual-models, model-registry, system-prompt, settings-manager | runner/types | already on pi-session's critical path; port the minimal slices the runner's signatures require (getters can be trait'd initially) |
| `resolve-config-value` (`resolveConfigValueOrThrow` `${VAR}` / `!cmd`) | transport env/headers/oauth secret | port as pure string resolver in pi-core (command form needs exec trait) |
| auth-storage (`FileAuthStorageBackend`) | oauth.ts | port backend trait only |
| proper-lockfile | oauth refresh lock | defer; trait `RefreshLock` with no-op default |
| pi-codemode (QuickJS) | codemode | **deferred entirely** behind `CodemodeExecutor` trait |
| pi-ai (Provider types) | runner/provider registration | use existing pi-core::ai placeholders |

**Porting order (each step lands with parity cases):**

1. `pi-core::mcp_servers` — zero new deps (serde_json, url). Parity: `mcp-servers`.
2. `pi-core::tool_search` — zero deps. Parity: `tool-search`.
3. `pi-session::mcp_config` — needs McpServerConfig + `McpConfigFs` + preserve_order writer. Parity: `mcp-config`.
4. `pi-core::mcp_tools` — needs CallToolResult/ContentBlock types, truncate (exists), output-saver fake. Parity: `mcp-tools`.
5. `pi-session::extensions::{types,registry,api}` — minimal slice: Extension struct, McpServerRegistry integration, pending queues, commit/discard. Parity: `extension-loader` (factory/discovery parts).
6. `pi-core::mcp_resources` — needs mcp_tools. Parity: `mcp-resources`.
7. `pi-extensions::mcp::{log}` then `runtime` + `transport` + `fake` — needs error-kind classification, SleepFn. Parity: `mcp-log`, `mcp-connection`.
8. `pi-session::extensions::{loader,runner}` — the big one; needs EventBus, SessionManager (exists), system-prompt stubs. Parity: `extension-runner`, `extension-loader` (discovery).
9. `pi-extensions::mcp` extension wiring + `mcp-section` parity.
10. Deferred (post phase 3): OAuth flows, StreamableHttpTransport, llama extension, codemode sandbox, `/mcp` UI + `pi mcp` CLI.

Rationale: steps 1–4 are fully pure and unlock the majority of byte-level parity surface; 7 is where process behavior concentrates but is fully fake-driven; 8 is large but mechanical once the emit semantics table (§1.4) is followed.

---

## 7. Step-by-step porting checklist (with LOC estimates)

Rust LOC ≈ TS LOC × 1.1–1.4 here (structs/enums + serde derive inflate; no jiti/runtime-detection code needed).

- [ ] **1. `pi-core/src/mcp_servers.rs`** (~550): McpExposure + alias resolution; config enums with serde (`type` field defaulting, `"streamable-http"` alias, `"sse"` rejection); `tool_pattern_regexp` with JS escaping set; `create_tool_name_matcher`; `validate_mcp_server_config` with exact error strings (20+ branches — build a message table first); `McpServerRegistry` on IndexMap with deep-clone `list`. Harness arm `mcp-servers` + ~40 cases in `parity/cases.mjs` covering every validation branch, alias, clash, and matcher pattern. *(~1.5 days)*
- [ ] **2. `pi-core/src/tool_search.rs`** (~350): stop-word list, stemmer regexes (JS regex → `regex` crate, mind `([a-z0-9])([A-Z])` camelCase splits), tokenize, `create_tool_search_document` (recursive schema walk incl. anyOf/oneOf/allOf/items), BM25 with the file's k1/b. Harness `tool-search` + tokenizer/BM25 golden cases. *(~1 day)*
- [ ] **3. `pi-session/src/mcp_config.rs`** (~400): `McpConfigFs` trait; `read_config_file` (override merge `{...base.config, ...value}` via preserve_order map merge), `load_mcp_config`, patch semantics (default-deleting), `edit_mcp_servers` with a `js_json_stringify_pretty(indent)` helper (JS-compatible escaping/indentation) + indent sniffing regex. Harness `mcp-config` with in-memory file maps; include duplicate-key, bad-JSON, untrusted-project, override-with-extra-keys, auth-in-project cases. *(~1.5 days)*
- [ ] **4. Protocol types + `pi-core/src/mcp_tools.rs`** (~650 + ~300 types): `McpTool`, `CallToolResult`, `ContentBlock`, `RequestOptions`, `McpErrorKind` (transient statuses, session-expired, auth-required, method-not-found, transport). Port `create_mcp_tool_name` (UTF-16-aware replace + sha256 suffix), `relocate_schema`, `limit_mcp_content` (uses existing truncateMiddle), `block_to_content` (mime classification, extension sniffing), `convert_mcp_result` (_meta strip, isError handling), `to_parameters`, `to_tool_annotations`. Harness `mcp-tools` with a fixed-path fake saver. *(~2 days)*
- [ ] **5. `pi-core/src/mcp_resources.rs`** (~450): `McpResourceServer` trait; the three tool definitions with exact parameter/output schemas as serde_json values; list orchestration (page vs allSettled, localeCompare sort, MCP-App filtering, `listed` server-tagging, error collection); read with multi-content labeling. Harness `mcp-resources` with scripted servers incl. per-server failures and cursor misuse. *(~1.5 days)*
- [ ] **6. `pi-session/src/extensions/` core slice** (~1050): `types.rs` minimal (Extension, RegisteredTool/Command/Flag/Shortcut, ExtensionError, event type strings), `registry.rs` (runtime state, pending queues, invalidate), `api.rs` (registerTool/registerCommand/registerFlag/registerMcpServer incl. ownership+clash errors, getFlag pending semantics, commit/discard). Harness `extension-loader` op `factory` — drive a Rust closure "factory" that registers things or throws, assert committed/discarded state. *(~2 days)*
- [ ] **7. `pi-extensions` crate + `mcp/log.rs` + `transport.rs` + `fake.rs`** (~1000): crate scaffold; `format_mcp_log_message` (injectable now); `McpTransport` trait; scripted fake; `SleepFn`. Harness `mcp-log`. *(~1 day)*
- [ ] **8. `pi-extensions/src/mcp/runtime.rs`** (~700): `McpServerConnection` sync state machine — single-flight via `Option<OpenGuard>`, attempt ids, `with_client` retry rules (read-only transient retry, session-expired retry, needs-auth drop+mark), `open` with injectable retry delays, stderr tail capture, `close` with auth-settled hook. Harness `mcp-connection` — script sequences covering every state transition. *(~2.5 days)*
- [ ] **9. `pi-session/src/extensions/loader.rs` + `runner.rs`** (~1950): DirectoryLister trait; discovery rules; extension cache (cwd/generation); `ExtensionRunner` with `bind_core` (mcp change listener wiring, pending-registration flush with error capture), context factories (guarded getters → closure-based `ExtensionContext`), and each `emit*` from the semantics table in §1.4 — port one emitter per commit with its parity case. `extension-runner` harness with behavior-scripted handlers. *(~4–5 days, land incrementally)*
- [~] **10. `pi-extensions/src/mcp/mod.rs`** (~600) — *in progress; see `docs/plans/phase4-tranche7-mcp-extension.md`*: `create_mcp_extension` wiring against the runner (session_start/turn_start/mcp_servers_change/session_shutdown/before_agent_start), `render_servers_section`, exposure→activate-codemode/tool_search decisions, `/mcp` command registration stub. Harness `mcp-section`. *(~2 days)*
- [ ] **11. Docs + tracking**: update `PARITY.md`/`TRACKING.md` with the new harness groups and coverage notes; record the deferred list (OAuth network flows, HTTP transport, codemode sandbox, llama, jiti loading) with the trait seams that isolate them. *(~0.5 day)*

Estimated total: **~8,900 Rust LOC** (excluding deferred subsystems) and **~17–19 working days**.

### Deferred beyond phase 3 (explicit non-goals, with seams already in place)

- JS/TS extension loading (jiti, virtual modules, aliases) — `ExtensionFactory` trait; a `deno_core`/QuickJS-backed loader can slot in later.
- codemode sandbox (`execute.ts`, QuickJS wasm, worker limits) — `CodemodeExecutor` trait.
- `StreamableHttpTransport` + full OAuth (loopback server, DCR/CIMD, lockfiles) — `McpTransport`/`OAuthFlow`/`RefreshLock` traits; only state serialization (`parse_states`/`store_keys`) is parity-relevant now.
- llama.cpp extension (client/provider/HF) and all TUI views (`ui.ts`, `renderer.ts`).
