# Phase 4 · Tranche 5: mcp-connection — COMPLETE

Live working document for the tranche. The permanent record is in
`TRACKING.md` (tranche-5 entry). Last updated: 2026-10-22 (session 4,
tranche complete: parity 2851/2851, committed).

## ✅ STATE (2026-10-22, session 4) — TRANCHE COMPLETE

The old RESUME-HERE block ("client.rs complete / transport.rs refactored /
runtime.rs + harness + cases NOT STARTED", progress items 1-4) is obsolete.
Final state, all verified and committed:

- Items 1-9 of the original checklist COMPLETE (see below), with one
  mid-implementation correction: **`McpLogFs` lives in `log.rs`** (moved
  there from the runtime design sketch) — the mll/ harness writes via
  `McpServerLog`, and runtime.ts itself imports McpServerLog from log.ts.
- 40 unit tests across pi-extensions (25 client + transport + runtime).
- 52 mcc/ parity cases (plan estimated ~30).
- Full parity **2851/2851**; clippy clean on all new code.
- One stale unit-test expectation corrected (see runtime.rs
  `tools_list_changed_refreshes_tools`): a `tools/list_changed` arriving
  during the handshake's own `tools/list` send attaches to the OLD
  (undefined) client in TS — the refresh's `tools/list` IS sent
  (observable) but its result is DISCARDED. Rust matches by client-id
  seq check; differential parity was the arbiter.

**Build env for every bash call:**
`export RUSTUP_HOME=$HOME/AI/.rustup CARGO_HOME=$HOME/AI/.cargo PATH=$HOME/AI/.cargo/bin:$PATH`
Parity runner: `node parity/run-parity.mjs [--filter mcc/]` from `~/AI/pi-rs`
(PI_REPO defaults to `/home/shinobi/AI/pi-repo`; pi-cli picked from
`target/release`).

## TS sources (ground truth)

- `pi-repo/node_modules/@earendil-works/pi-mcp/src/client.ts` (621 LOC)
- `pi-repo/packages/coding-agent/src/extensions/mcp/runtime.ts` (454 LOC)
- Supporting: `protocol/jsonrpc.ts` (error classes), `protocol/types.ts`,
  `transports/streamable-http.ts` (McpHttpError family),
  `oauth/errors.ts` (`McpOAuthAuthorizationRequiredError`),
  `extensions/mcp/oauth.ts` (`createMcpAuthProvider` — fully lazy).
- dist entry: `pi-repo/packages/coding-agent/dist/extensions/mcp/runtime.js`
  (exports `McpServerConnection`, `createDefaultTransport`, …; bare
  `@earendil-works/pi-mcp` imports resolve via pi-repo node_modules —
  verified working when imported by file URL from the harness).
- pi-mcp dist also importable directly:
  `PI_REPO/node_modules/@earendil-works/pi-mcp/dist/index.js` (McpClient,
  error classes) and `.../dist/oauth/index.js`
  (McpOAuthAuthorizationRequiredError) — the TS harness needs these to
  construct Fail-step error instances.

## Key facts confirmed from TS

- `VERSION = 1.3.0` → initialize `clientInfo.version`.
- Error messages (exact): `McpConnectionClosedError` default
  "MCP connection closed"; `requireTransport` throws
  `` `MCP client is ${state}` ``; `McpTimeoutError` `` `MCP request timed
  out after ${timeoutMs}ms` ``; `McpAbortError` "MCP request aborted";
  `McpHttpError(status, message, body)`; `McpAuthRequiredError` 401 "MCP
  server requires authentication"; `McpSessionExpiredError` 404 "MCP session
  expired" (both subclass McpHttpError); `McpOAuthAuthorizationRequiredError`
  "MCP OAuth authorization requires user interaction".
- `isTransientError`: McpHttpError && (408 || 429 || (>=500 && !=501)), or
  TypeError.
- Runtime constants: `DEFAULT_TIMEOUT_SECONDS=60`, `STDERR_TAIL_CHARS=2000`,
  `CONNECT_RETRY_DELAYS_MS=[250,1000]` (url configs only; stdio → no
  retries), client `DEFAULT_REQUEST_TIMEOUT_MS=30000`,
  `MAX_LIST_PAGES=1000`.
- `SUPPORTED_PROTOCOL_VERSIONS = ["2025-11-25","2025-06-18","2025-03-26","2024-11-05"]`.
- `usesOAuth(entry)`: url config, no `auth`, no `authorization`
  (case-insensitive) header.
- Connection constructor calls `credentials.forServer(name, url)` EAGERLY
  when `oauthUrl`; everything else in the auth provider is lazy.
- Connection close ordering (TS): `shutdown.abort()` fires abort listeners
  SYNCHRONOUSLY (but none are armed at op boundaries — connectOnce removes
  its abort listener in `finally`), then `state="closed"; changed();` then
  `this.client=undefined; client?.close(); authProvider.settled()`.
- `handleClientClose` only acts when `this.client === client && !closed`:
  client=undefined, state="disconnected", error "Connection closed" (or
  `Connection closed\n${stderr}` for stdio), changed().

## Design decisions (settled — follow these exactly)

### Client async→sync mapping (implemented in client.rs)

- **Mailbox**: transport listeners push `ClientEvent`s; after each
  transport op the client drains exactly the range appended during that
  op (index-based, fixed end). Nested sends (notification listeners →
  refreshTools → tools/list) drain their own appended ranges recursively.
- **Tasks**: responses to server→client requests (ping, roots/list) are
  deferred (`Task::SendResponse`) and run when `op_depth` reaches 0.
  Unknown-method `-32601` responses are sent SYNCHRONOUSLY during drain
  (TS: the `transport.send(...)` call precedes its `await`), known-handler
  responses are deferred (TS: `await handler(...)` suspends first).
- **Parked requests**: if a reply didn't arrive during its own send, only
  the timeout can settle it → sleep via `SleepFn` then
  `cancel_pending(Timeout, cancellable, "Request timed out")` (sends
  `notifications/cancelled` for non-initialize). `timeout_ms == 0`
  returns a synthetic error (TS would hang — cases never do this).
- Send failures: drain first (TS batch delivery precedes the Fail throw),
  then `cancel_pending(key, error, notify=false)` unless already settled.

### runtime.rs — McpServerConnection port design (NOT YET WRITTEN)

```rust
pub struct McpServerConnection {
    pub entry: McpServerEntry,          // name + config (see below)
    pub state: ServerState,             // connecting/connected/disconnected/needs-auth/failed/closed
    pub error: Option<String>,
    pub tools: Vec<Value>,
    pub has_resources: bool,
    pub instructions: Option<String>,
    pub challenge: Option<Value>,
    client: Option<McpClient>,
    opening: bool,                      // TS `opening` promise flag (always settled at op boundaries)
    shutdown: bool,                     // TS AbortController `closed` getter
    stderr_tail: Option<String>,        // structurally dead with fake transports (not StdioTransport)
    cwd: String,
    create_transport: Box<dyn FnMut(...) -> Box<dyn McpTransport>>,
    auth_provider: Option<McpAuthProvider>,   // presence-only model
    on_tools: Box<dyn FnMut()>,         // harness records markers
    on_change: Option<Box<dyn FnMut()>>,
    log: Option<Box<dyn McpLogSink>>,   // see below
}
```

- **McpServerEntry**: `{ name: String, config: McpServerConfig }` — define
  LOCALLY in runtime.rs (the full TS entry has source/scope/override, used
  only by config-file code, already covered by the mcp-config tranche).
  `McpServerConfig` from `pi-core::mcp_servers` (kind Stdio/Http +
  preserved `fields` Map) gives url/command/timeout/headers/auth checks:
  `uses_oauth` = kind Http && no `auth` field && no `authorization`
  (case-insens.) in `headers`; retry delays only for Http; `timeout`
  (seconds) defaults 60.
- **Auth seam (presence-only)**: `pub struct McpAuthProvider;` with
  `settled()` no-op. `needs_sign_in(err)` =
  `OauthAuthorizationRequired || (auth_provider.is_some() &&
  matches!(err, Http{auth_required: true, ..}))`. Constructor: when
  oauthUrl → call `credentials.for_server(name, url)` (trait
  `McpCredentialStore { fn for_server(&mut self, name, url) {} }`,
  harness passes no-op) and set auth_provider; when url+`auth.provider`
  config → set auth_provider (token provider branch, presence only);
  else None.
- **handleClientClose seam (IMPORTANT)**: the connection CANNOT register
  closures on the client (borrow). Instead the client exposes
  `take_close_fired() -> bool` (set when markClosed fires listeners).
  The connection calls `process_client_close()` at these exact points:
  (a) in `with_client` after `run(client)` returns, BEFORE examining the
  error; (b) in `refresh_tools` after `list_tools` returns, BEFORE the
  catch sets error; (c) after `deliver_external` (emit op); (d) after
  connect attempts (no-op — client not assigned). Processing: if flag &&
  `self.client` is that client && !shutdown → client=None,
  state=disconnected, error="Connection closed", changed().
  Dropped clients are NOT retained (TS keeps them alive via closures; their
  only residual behavior needs emits while disconnected — excluded from
  cases; documented).
- **connectOnce**: new McpClient (name "pi", version "1.3.0",
  request_timeout_ms = timeout_ms, roots =
  `[{uri: path_to_file_url(cwd), name: basename(cwd)}]`), register
  `notifications/message` → log sink, transport = create_transport(...),
  `client.connect(transport)` (propagates errors), register
  `notifications/tools/list_changed` → refresh_tools, `tools =
  if serverCapabilities.tools { client.list_tools() } else { [] }`
  (errors propagate), shutdown/closed + connectionState checks
  ("shut down while connecting" / "connection closed during setup"),
  assign self.client, tools, has_resources (capabilities.resources
  present), instructions (trim, empty→None), state=connected,
  error=None, on_tools(), changed().
  Error path: `client.close()` (ignore), rethrow.
  Client id note: the connection can't hold `&McpClient` while calling
  methods — use an incrementing `client_seq: u64`; `self.client_id:
  Option<u64>`; refresh_tools takes the seq; all comparisons by seq.
- **with_client(run, read_only)**: loop attempt=1..: get_client()?; result
  = run(client); process_client_close(); on Err: if read_only &&
  attempt==1 && Http && transient → sleep(250ms real via SleepFn),
  continue; if SessionExpired (Http{session_expired}) && attempt==1 →
  if self.client_id == client_id { self.client = None } continue; if
  !needs_sign_in → return Err; else drop_client(client);
  mark_needs_auth(); return Err(Other(sign_in_required_message)).
- **open()**: state=connecting, changed(); retries = Http ? [250,1000] :
  []; loop attempt: stderr_tail=None; connect_once() → Ok return; Err →
  delay = retries[attempt]; if shutdown || delay None || !transient →
  return Err(connect_failed(error)); sleep(delay); if shutdown →
  return Err(connect_failed(error)).
- **connect_failed(err)**: if needs_sign_in && !shutdown →
  mark_needs_auth, return Err(sign_in_required_message); else state =
  shutdown ? closed : failed; error = stderr_tail.map(append) or message;
  changed(); Err(`MCP server "${name}" failed to connect: ${error}`).
- **reconnect()**: (opening always settled) drop client if any;
  get_client(). **signOut()**: drop client; if !shutdown →
  mark_needs_auth. **close()**: shutdown=true; state=closed; changed();
  if let Some(client) = take → client.close() (ignore); auth settled
  (no-op).
- **get_client()**: if shutdown → Err(`MCP server "${name}" is shut
  down`); if client Some && connection_state==Connected → return; if
  opening (never at op boundaries) …; else open().
- **refresh_tools(client_id)**: list_tools → process_client_close → Ok:
  if self.client_id == Some(id) && !shutdown { tools = …; on_tools() }
  Err: error = `Failed to refresh tools: ${message}`; then changed().
- **withoutTemplates**: resourcesPage templates wrapper: on
  Rpc{METHOD_NOT_FOUND} → `{resourceTemplates: []}`; allResources
  templates: → `[]`.
- **Log sink**: `pub trait McpLogSink { fn write(&mut self, server: &str,
  params: &Value); }` — harness impl wraps
  `McpServerLog::write(&fs, server, params, now_ms)` with the pinned
  step clock. TS side: real `McpServerLog` over a temp file with the
  global Date pinned per-step (mll pattern).
- **path_to_file_url(cwd)**: POSIX port of Node's `pathToFileURL`:
  "file://" + percent-encode every byte that is not RFC3986 unreserved
  (A-Za-z0-9-._~) and not '/'. Harness cwd is a fixed simple ASCII path
  (`/tmp/pi-parity-mcc`) so encoding edge cases are not exercised; add a
  unit test for space/non-ASCII anyway.

### Harness protocol (mcp-connection) — both sides

```json
{
  "entry": { "name": "srv", "config": { "url": "https://x" } },
  "cwd": "/tmp/pi-parity-mcc",
  "logPath": "logs/srv.log"?,
  "steps": [
    { "op": "callTool", "name": "t", "args"?: {}, "options"?: { "timeoutMs": 5 } },
    { "op": "readResource", "uri": "u" },
    { "op": "resourcesPage", "cursor"?: "c" },
    { "op": "resourceTemplatesPage", "cursor"?: "c" },
    { "op": "allResources" }, { "op": "allResourceTemplates" },
    { "op": "reconnect" }, { "op": "signOut" }, { "op": "close" },
    { "op": "snapshot" },
    { "op": "emit", "message"?: {...} } | { "op": "emit", "error"?: "text" } | { "op": "emit", "close": true },
    { "op": ..., "nowMs"?: 0 }
  ],
  "transports": [ { "startSteps": [...], "sendSteps": [[...], ...] } ]
}
```
Step shape: `{ "message": {...} } | { "error": "text" } | { "close": true }
| { "fail": { "kind": "http"|"authRequired"|"sessionExpired"|
"oauthRequired"|"typeError"|"error"|"closed", "status"?, "message"?,
"body"? } }`.
Result: `{ "results": [...], "markers": [...], "sent": [...],
"files": {...}, "snapshot": {...} }` where:
- `results[i]`: op return value — request payloads (callTool result,
  resource pages, …) on success; `{ "error": message }` on rejection;
  null for reconnect/signOut/close/emit; snapshot for snapshot op
  ({state, error, tools, hasResources, instructions, challenge}).
- `markers`: per-STEP arrays of "onTools"/"onChange" (harness resets after
  each step).
- `sent`: all messages recorded by all created transports, in creation
  order.
- `files`: log fixture files (only when logPath given).
- TS error construction for Fail steps: `new
  piMcp.McpHttpError(status, message, body)`,
  `new piMcp.McpAuthRequiredError({ headers: { get: () => null } }, body)`,
  `new piMcp.McpSessionExpiredError(body)`,
  `new piMcpOauth.McpOAuthAuthorizationRequiredError()`,
  `new TypeError(msg)`, `new Error(msg)`,
  `new piMcp.McpConnectionClosedError(msg?)` — import from
  `PI_REPO/node_modules/@earendil-works/pi-mcp/dist/index.js` (+ `/oauth/index.js`).
- TS fake transport: `async start()` (closed → throw McpConnectionClosedError;
  first start runs startSteps), `async send(msg, opts)` (record; pop one
  sendSteps BATCH; run steps COLLECTING events; deliver collected events to
  listeners synchronously after the batch; Fail → deliver first, then
  throw), `async close()` (idempotent, emit onClose once), plus
  `emit(message)`/`emitError(text)` delivering directly to listeners for
  the emit op. Rust FakeTransport already matches (mailbox push =
  deferred delivery; per-send batches).
- TS per-step: `await op()` then `await new Promise(r => setImmediate(r))`
  barrier, then collect markers.
- Rust factory: shared registry `Vec<Rc<RefCell<FakeTransport>>>` via a
  `TransportRef` wrapper implementing McpTransport by delegation so the
  harness can read `sent` at the end; scripts consumed in order per
  connectOnce (missing script → error).
- Rust emit op: `connection.deliver_external(event)` → current client's
  `client.deliver_external(ClientEvent::…)` (no-op when disconnected —
  case constraint); then process_client_close. TS emit: call the LAST
  created transport's listener delivery (fake.emit(message)) — for Close
  use fake.close() (marks closed + emits).

### Case constraints (for authors — violations diverge TS↔Rust)

1. Every request's reply must be scripted in the SAME send's step batch,
   or the case tests the timeout with small `options.timeoutMs`
   (real sleep both sides; keep ≤ 20ms; connect-retry cases 250ms are OK
   but slow — use sparingly).
2. Never leave a request parked across ops; never use `timeout: 0`.
3. `tools/list_changed` refresh replies must be scripted in the
   `tools/list` send batch that the refresh itself triggers.
4. Incoming server→client requests (ping/roots/list/unknown): deliver in
   batches that do NOT also contain a response resolving the blocked
   request when that request's continuation sends (e.g. during connect).
   Safest: use the `emit` op for incoming requests, or dedicated batches.
5. No emits while disconnected (Rust drops old clients; TS retains them).
6. initialize timeout tests need `entry.config.timeout >= 1` (seconds; 1s
   sleep). Prefer callTool/readResource timeouts with
   `options.timeoutMs: 5`.
7. Cases must count handshake sends: connect = initialize batch,
   initialized batch (can be empty), then tools/list batch (if
   capabilities.tools). A reconnect consumes a NEW transport script and
   the SAME send sequence (with fresh request ids starting at 1).

### Case list (~30, prefix `mcc/`)

happy-path connect + callTool; initialize validation errors (missing
fields); unsupported protocol version; no tools capability (tools=[]); no
resources capability vs has; instructions trim; pagination multi-page +
cursor passthrough + null/"" cursor; duplicate cursor; list validation
errors (bad entry, bad cursor); resourcesPage/templatesPage;
allResources name-defaulting; methodNotFound templates → []; other
templates error propagates; readResource validation; callTool content
defaulting + structuredContent; JSON-RPC error from tool; transient
readOnly retry (Http 429 on readResource → retry succeeds); transient
callTool NOT retried; connect transient retry (url config, Fail then
success); stdio config no connect retry; SessionExpired retry on new
client; AuthRequired with auth provider → needs-auth + sign-in message;
AuthRequired without provider → error propagates; OauthAuthorizationRequired
→ needs-auth; timeout (callTool timeoutMs 5, cancelled notification sent);
timeout on initialize (no cancelled); transport Close step mid-op →
disconnected + next callTool reconnects; emit close between ops; emit ping
→ response; emit roots/list → roots; emit unknown method → -32601;
tools/list_changed refresh; notifications/message → log file;
reconnect; signOut; close ordering + markers; getClient after close →
shut down error; snapshot fields.

## Original tranche goal + remaining roadmap after this tranche

Phase 4 remaining after tranche 5 (per `docs/plans/phase3-mcp-extensions.md`
and TRACKING roadmap): extension-loader (step 9), mcp-section (step 10),
tools/* slices, then the shared typebox-walker refactor. `transport.rs`
(~523 LOC) landed ahead of its tranche and is now exercised by
mcp-connection cases.

## Original checklist

- [x] 1-3. client.rs + McpErrorKind + unit tests (25 passing).
- [x] 4. transport.rs refactor (McpErrorKind, per-send batches) + tests.
- [x] 5. runtime.rs McpServerConnection (design above).
      (Correction: `McpLogFs` lives in `log.rs`, not runtime.rs.)
- [x] 6. runtime.rs unit tests (40 total across the crate).
- [x] 7. Rust harness + main.rs wiring.
- [x] 8. TS harness entry.
- [x] 9. Parity cases mcc/ (52 — plan estimated ~30).
- [x] 10. cargo build --release + cargo test + clippy clean on new code.
- [x] 11. parity --filter mcc/ green (52/52).
- [x] 12. Full parity suite green (**2851/2851**).
- [x] 13. TRACKING.md entry + commit (+ docs commit).
