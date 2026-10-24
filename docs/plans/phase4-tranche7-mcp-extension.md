# Phase 4 · Tranche 7 — the built-in MCP extension

Design/decision record for `docs/plans/phase3-mcp-extensions.md` step 10
(the last unimplemented piece of that plan): `extensions/mcp/index.ts`
(L1–L1353, 1347 lines) wired against the extension runner, plus the
`mcp-section` parity harness the step promised.

The other MCP slices — core pure functions (step 1–2), config (3),
tools/resources conversion (4–5), extension registry/API (6), log +
transport (7), connection runtime (8), and the extension runner/loader
core (9) — are already ported. This tranche is the glue: one extension,
its lifecycle, its tool registration, and `/mcp`.

## 1. What exists in `index.ts` (inventory)

| Area | TS lines | Depends on | Status |
|---|---|---|---|
| Option/type surface (`McpExtensionOptions`, `McpServer`) | 69–122 | config types, connection | types ported in 10c (`McpExtensionOptions`, `McpServerState`) |
| `renderServersSection` + exposure predicates + section budgets | 124–255 | pure | **done** (10a, `mcp/section.rs`, `mcs/` 15 cases) |
| `describeState` / `attentionRank` / `describeTransport` / `formatStatus` / completions | 257–310, 1050–1100, 1264–1310 | pure | **done** (10b, `mcp/status.rs`, unit-tested; status/completions also parity-tested through the extension) |
| Extension factory state + session lifecycle (`session_start`, `registeredServers`, `ensureDiscoveryActive`, `reportProblems`, `startConnection`, `waitForServers`) | 279–700, 1114–1175 | runner API, runtime, config | **10c done** (state machine + `McpHost` seam, connections deferred) |
| `before_agent_start` section injection | 1178–1190 | section | **10c done** |
| `tool_call` reach/owner/resource waiting | 1191–1211 | connections, tool owners | 10d |
| `turn_start` sign-in retry, `mcp_servers_change`, `session_shutdown` close ordering | 1212–1259 | connections, credentials | bookkeeping done in 10c; connection halves 10d |
| `registerTools` / `hideTools` / resource-tool exposure sync | 387–530 | `createMcpToolDefinition`, resource defs, registry | 10d |
| `/mcp` command: status, picker, login/logout/reconnect, enable/exposure, manager | 1040–1350, `ui.ts` | OAuth, updateConfig, TUI | 10e (non-TUI empty/usage paths done in 10c) |
| Tool renderers (`registerToolRenderer`) | 383 | TUI | deferred (renderers are TUI-only in this port) |

## 2. Slices

- **10a — section** (landed, `5860e20`): `render_servers_section`, exposure
  predicates, UTF-16 budgets/truncation, omission lines; harness tool
  `mcp-section`; `mcs/` 15 cases.
- **10b — status/completions** (landed, `8f906a7`): `describe_state`,
  `attention_rank`/`sort_for_menu`, `describe_transport`, `format_status`,
  `argument_completions`, `MCP_USAGE`; unit tests (the dist exports none of
  these, so parity goes through the extension command in 10c).
- **10c — lifecycle state machine** (landed, `f6d944d`): `McpExtension` +
  `McpHost`, `session_start`, `mcp_servers_change`, `before_agent_start`,
  `session_shutdown`, `/mcp` non-TUI handling; harness tool
  `mcp-extension`; `mce/` 15 cases.
- **10d — connections + tools** (prep landed, `f6ad4af`: connections are
  `Send`; remaining wiring below): `McpServerConnection` instances per server,
  `registerTools`/`hideTools`, `createMcpToolDefinition`, resource-tool
  exposure sync, `tool_call` waiting, startup waits (`startupWaitMs`,
  direct-server wait, `pending`), close ordering.
- **10e — `/mcp` actions**: picker with connections, `login`/`logout`
  (OAuth seams), `reconnect`, enable/disable (+ project overrides),
  exposure changes, `updateConfig`; the TUI manager (`ui.ts`) stays out.
- **10f — runner adapter**: register the factory with `ExtensionApi`
  (`on`, `register_command`, `get_mcp_servers`, runtime actions, UI), so
  `pi-cli` can load the built-in extension.
- **10g — docs/tracking**: PARITY/TRACKING updates, deferred list refresh.

## 3. Decisions

### 3.1 The state machine is an explicit object, not a closure

`createMcpExtension` returns a closure over mutable state. The port makes
the state a `McpExtension` struct with `&mut self` lifecycle methods, and
keeps the factory as a thin builder. This is what makes it parity-testable
without a JS runtime: the harness calls the same methods the future runner
adapter will call in the same order.

### 3.2 `McpHost` seam instead of `ExtensionApi`/`CtxView`

The extension needs `pi.getMcpServers/getAllTools/getActiveTools/
setActiveTools` and `ctx.cwd/isProjectTrusted/ui.notify/ui.select` plus
`getAgentDir()`. `CtxView` models guarded getters but not `ctx.ui`, and the
runtime actions are optional pre-bind. A dedicated trait keeps the state
machine `Send + Sync` and lets the harness script every read (including
whether a tool "reachesTools" and the selected server for ambiguous
pickers).

`loadConfig` stays an injected `Arc<dyn Fn(&dyn McpHost) -> LoadedMcpConfig>`:
the default implementation (agent-dir/project `mcp.json`) is the adapter's
job, the same way the TS options allow injection.

### 3.3 `!Send` connections vs `Send` handlers (10d prerequisite)

`ExtensionHandler` is `Arc<dyn Fn(...) + Send + Sync>` and returns
`BoxFuture + Send`; `McpServerConnection` currently holds `Rc<RefCell<…>>`
(log sink, pending-events queue) and its `McpTransport`/`McpLogSink` traits
are not `Send`. So connections cannot simply live in the extension state
that handlers capture.

Two viable designs were considered:

1. **Thread-local connection registry.** Keep `McpServerConnection` as is;
   the adapter stores connections in a `thread_local!` keyed by server id
   and handlers proxy into it. Works only if every handler poll happens on
   the owning thread; the harness's and CLI's current-thread runtimes do,
   but it is a fragile invariant for a `Send` future.
2. **Make connections `Send`.** Refactor `Rc<RefCell>` → `Arc<Mutex>`
   (~35 references across `client.rs`, `runtime.rs`, harness fakes),
   add `Send` bounds to `McpTransport`/`McpLogSink`, and replace the
   `FnMut()` callbacks with an event queue drained after each operation
   (the runtime already queues `ConnEvent`s for exactly this reason).
   The extension then owns `Arc<Mutex<McpServerConnection>>` and reads
   snapshots after releasing the lock — no re-entrancy, no thread
   invariant.

**Decision: option 2 — landed in `f6ad4af`** (connections and clients are
`Send`; `onTools`/`onChange` carry a `McpConnectionSnapshot`). The
queue-drain is a small extension of the
existing `ConnEvent` pattern, and it removes a class of deadlocks
(callbacks into state that owns the connection). The refactor lands as
10d's first commit, before any connection is wired, with the
`mcp-connection` harness re-run to prove behavior is unchanged.

### 3.4 What the state machine does while connections are absent

10c keeps every observable that does not need a connection and explicitly
does nothing where the TS launches background work:

- `session_start` starts no connections; it reports startup problems
  immediately **only** when no server is enabled (the TS path that never
  touches the runtime), otherwise it defers the report to 10d.
- `mcp_servers_change` drops/adds entries and re-runs discovery; it does
  not close or start connections.
- `before_agent_start` flips `waitedForStartup` but has nothing to wait
  for; the `startupWaitMs` timer lands with connections.
- `tool_call`/`turn_start` are no-ops (no tool owners, no sign-ins).
- `/mcp` covers status/usage/picker messages; sign-in flows stay stubs
  until OAuth.

The harness cases are written against those boundaries (no op that would
await `pending` after an enabled server).

## 4. `mcp-extension` harness protocol

Same in-band style as the other tools; TS drives the real factory with a
fake `pi` API and fake ctx, Rust drives `McpExtension` + a scripted
`McpHost`. Both return `{ log: [line…] }`, where the log interleaves host
side effects and op results in call order:

- `notify:<level>:<message>` from `ctx.ui.notify`
- `activeTools:<json>` from `pi.setActiveTools`
- `select:<title>:<json items>` from `ctx.ui.select`
- `<op>:ok` / `beforeAgentStart:<event json>` / `completions:<json|null>`

Ops: `sessionStart`, `beforeAgentStart`, `setRegistered`, `serversChange`,
`turnStart`, `toolCall`, `command`, `completions`, `shutdown`.

Host args mirror the real values (`registeredServers`, `cwd`, `agentDir`,
`projectTrusted`, `tools` with optional `reachesTools`, `activeTools`,
`selectChoice`). `loadConfig` is the literal `LoadedMcpConfig` the TS
option returns; `startupWaitMs` defaults to 10000.

TS pins `process.env.PI_CODING_AGENT_DIR` to the case's `agentDir`, because
`formatStatus` resolves the agent dir through `getAgentDir()` and the dist
has no injection point for it.

## 5. Deferred (with seams)

- Real transports (`createDefaultTransport`, stdio/HTTP construction,
  `expandHome`, env/header resolution) — `TransportFactory` seam; the
  adapter supplies the default.
- OAuth (`signInMcpServer`, credential store, lockfiles) —
  `McpCredentialStore` + `/mcp` action seam; 10c stores `oauthUrl` only in
  the connection snapshot.
- TUI manager (`ui.ts`, menus, sign-in screens, tool lists) — the TUI port;
  `McpHost::select` and the 10b helpers are the shared pieces.
- Tool renderers (`createMcpToolRenderers`) — TUI only.
- `pi.events` / extension tool pipeline integration beyond `registerTool`
  (10d uses the runner's `register_tool` exactly like other extensions).
