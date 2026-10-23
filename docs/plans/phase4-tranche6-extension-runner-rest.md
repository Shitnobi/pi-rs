# Phase 4 Tranche 6: extension-runner remaining slices (runner.ts L402–1565)

Status: IN-PROGRESS (Rust port + harness + cases)
Started: 2026-10-22

## Sources read in full

- `core/extensions/runner.ts` (1565) — everything from `bindCore` (L417) through
  `emitInput` (L1525); the tranche-1 core slice (constructor, `emit` loop,
  `emitMessageEnd`, flags/commands/shortcuts, `reportUnhandledMcpServers`,
  `invalidate`, `emitSessionShutdown`/`emitProjectTrust`, `setUIContext` stub)
  is already ported.
- `core/extensions/loader.ts` (876) — `createExtensionRuntime` stubs (L157–243),
  `createExtensionAPI` action methods (L377–413: `sendMessage`,
  `sendUserMessage`, `appendEntry`, `setSessionName`, `getSessionName`,
  `setLabel`, `exec`), `registerMcpServer`/`registerVirtualModel`/`events`
  (L480–535), `commit`/`discard` (L532–555), cache + discovery (L129–151,
  L559–876) — **discovery/cache = tranche 7**.
- `core/exec.ts` (107) — `execCommand` (spawn, timeout SIGTERM → 5 s SIGKILL,
  abort signal, `waitForChildProcess`).
- `core/utils/child-process.ts` — `spawnProcess`, `waitForChildProcess`
  (exit + pipe-idle grace; the 100 ms grace/re-arm logic is timing behavior we
  do not reproduce exactly — see divergences).
- `core/system-prompt.ts` — `normalizeBuildSystemPromptOptions` (L60),
  `buildSystemPromptSections` (L128), `buildSystemPromptState` (L199),
  `buildSystemPrompt` (L208) = `getSystemMessageText({role, ...state,
  timestamp: 0})`.
- `packages/ai/src/utils/text.ts` — `contentText`, `getSystemMessageText`
  (content first, then every non-null section value, `"\n\n"`.join,
  empty parts dropped).
- `core/extensions/types.ts` — full event/result surface (BoundaryState/
  BoundaryResult/BoundaryContextPreview, UserBashEvent/Result,
  InputEvent/Result, ToolCall/ToolResult events + results,
  BeforeProviderRequest/Headers, BeforeAgentStart* (+ readonly systemPrompt
  getter), ResourcesDiscover*, CacheWarmingDecision* (cache-warmer.ts L88–119,
  `CacheWarmingAction = "warm" | "stop"`), MessageEnd*, ContextEventResult,
  ExtensionActions (L2189) = 16 methods, ExtensionContextActions (L2211),
  ExtensionCommandContextActions (L2235), ExtensionUIContext (L149).
- Existing port: `pi-session/src/agent_session/extension_runner.rs` (1822;
  tranche-1 slice), `extension_runner_harness.rs` (618), ts-harness.js
  "extension-runner" entry (L3552–3820), 40 `exr/` cases.

## Scope (tranche 6 = the runner.ts remainder + the api action surface + exec)

### In

1. **Full `ExtensionActions` seam** (bindCore L417–464): 16 action methods
   (`sendMessage`, `sendUserMessage`, `appendEntry`, `setSessionName`,
   `getSessionName`, `setLabel`, `getActiveTools`, `getAllTools`,
   `getSettings`, `setActiveTools`, `refreshTools`, `getCommands`, `setModel`,
   `getThinkingLevel`, `setThinkingLevel`) copied into the shared runtime;
   pre-bind stubs (loader.ts L157–243): all throw
   `Extension runtime not initialized. …` **except** `refreshTools` (no-op) and
   `setModel` (returns a **rejected** promise → script steps must await).
2. **Full `ExtensionContextActions` seam** (L2211): `getModel`,
   `getScopedModels`, `isIdle`, `isProjectTrusted`, `getSignal`, `abort`,
   `hasPendingMessages`, `shutdown`, `getContextUsage`, `compact`,
   `getSystemPrompt`, optional `getSystemPromptOptions` (bindCore: `??` →
   default `normalize({cwd: this.cwd})`), optional `executeTool`, optional
   `getCallableTools`. Pre-bind = the class-field defaults (isIdle → true,
   isProjectTrusted → true, getModel → undefined, getScopedModels → [],
   getSignal → undefined, abort → noop, hasPendingMessages → **false**,
   getContextUsage → undefined, compact → noop, getSystemPrompt → "",
   shutdown → noop).
3. **`ExtensionApi` action methods** (loader.ts L377–413) incl. `pi.exec`
   → `core/exec.ts` port (new `pi-session/src/agent_session/exec.rs`).
4. **`bindCommandContext(actions?)`** (L553) + `ExtensionCommandContextActions`
   (NewSession/Fork/NavigateTree/SwitchSession/Reload handlers, all
   `async () => ({cancelled: false})` defaults).
5. **`setUIContext` full** (L572): `wrapUIPromptContext` (spread `{...ui, …}` —
   only select/confirm/input/editor/custom wrapped) + `withUIPrompt` depth
   tracking + `ui_prompt_start`/`ui_prompt_end` emitted via
   **`queueMicrotask`** (L619). See "Microtask engine" below.
   `hasUI()` = `uiContext !== noOpUIContext`. `getUIContext()`.
   The no-op context: `select`/`input`/`editor`/`custom` → undefined,
   `confirm` → false, `setTheme` → `{success: false, error: "UI not
   available"}`, `getEditorText` → "", everything else noop.
6. **`isUserBashEventResult`** (L140): `operations` XOR `result` (both or
   neither = invalid); `operations` must be a non-null object with
   `typeof operations.exec === "function"`; `result` must be a non-null
   object with `typeof output === "string"`, `"exitCode" in result` and
   (`undefined` or number), `typeof cancelled === "boolean"`,
   `typeof truncated === "boolean"`, `fullOutputPath` undefined-or-string.
7. **`createContext` full guarded getters** (L884): value getters
   (`ui`, `mode`, `hasUI`, `cwd`, `sessionManager`, `modelRegistry`, `model`,
   `scopedModels`, `thinkingLevel`, `signal`) assert stale **at read**;
   methods (`isIdle`, `isProjectTrusted`, `abort`, `hasPendingMessages`,
   `shutdown`, `getContextUsage`, `compact`, `getSystemPrompt`) assert stale
   **at call** (harness `reads` invoke methods: `typeof v === "function" ?
   v() : v`). `sessionManager` = the constructor arg (harness: null);
   `modelRegistry` = the constructor arg (harness: the fake registry → jstr
   `{}`).
8. **`createToolContext(toolCallId, signal)`** (L968): `tools` guarded getter
   → `getCallableToolsFn()`; `executeTool(name, args, options)` → guarded;
   no `executeToolFn` → the nested-call fallback result
   `{toolCall: {type: "toolCall", id: "${toolCallId}/0", name, arguments:
   {}}, result: {content: [{type: "text", text: "Nested tool calls are not
   available in this context"}], details: {}}, isError: true}`; with a fn →
   `executeToolFn(toolCallId, name, args, {...options, signal:
   options.signal ?? signal})`.
9. **`createCommandContext()`** (L997): property-descriptor copy of
   createContext (lazy guarded getters preserved) + `getSystemPromptOptions`,
   `waitForIdle`, `newSession`, `fork`, `navigateTree`, `switchSession`,
   `reload` (guarded).
10. **All remaining emitters** (event dispatch table below).
11. **`emitBeforeAgentStart`'s `systemPrompt` getter** →
    `buildSystemPrompt(normalizeBuildSystemPromptOptions(trigger options))`
    — needs the un-ported tail of system-prompt.ts (`buildSystemPromptState`/
    `buildSystemPrompt`) + a faithful raw `normalize` (see "Raw normalize"
    below).
12. Harness ops (both ts-harness.js "extension-runner" entry and
    `extension_runner_harness.rs`) for every new surface + ~60–80 `exr/`
    parity cases.

### Out (deferred)

- **Tranche 7 (next)**: loader.ts remainder — extension cache
  (cwd/generation), `loadExtension`/`initializeExtension`/
  `loadExtensionsInternal`/`loadExtensions`/`loadExtensionsCached`,
  `resolveExtensionEntries`/`discoverExtensionsInDir`/
  `discoverAndLoadExtensions` (+ `pi-manifest.ts`), around a
  factory-by-path resolver seam (jiti stays out of scope per phase-3 plan
  §2.12).
- Not in scope (phase-3 plan): jiti/virtual-modules, codemode QuickJS,
  OAuth flows, StreamableHttpTransport, llama, TUI views (`ui.ts`,
  renderers), mcp extension wiring (tranche 8), tools/* slices,
  typebox-walker refactor.

## Event dispatch table (all use snapshotEventHandlers; ordering rules)

| Emitter | Result rules | Handler errors |
|---|---|---|
| `emitBoundary` (L1036) turn_end/agent_before_settle | `entries`/`continue` accumulate per-handler result fields (undefined = keep); `buildContext(entries)` host callback after EVERY handler (initially with `[]`); buildContext throw → `valid = false` + `emitError("Invalid boundary entries: " + msg)`; invalid final → `{entries: [], continue: false, context, valid: false}` | caught → emitError, loop continues |
| generic `emit` (L1096) — done tr. 1 | session_before_* results accumulate, `result.cancel` short-circuits | caught |
| `emitCacheWarmingDecision` (L1128) | `action = event.action`; `result.action !== undefined` → override (last wins) | caught |
| `emitMessageEnd` (L1151) — done tr. 1 | role-mismatch → error + skip; last same-role wins; `!modified` → undefined | caught |
| `emitToolResult` (L1190) | per-field merge onto a copy of the event (`content`/`details`/`structuredContent`/`isError`/`usage`, `!== undefined`); content replaced without structuredContent → **`delete currentEvent.structuredContent`** (absence, not null); `!modified` → undefined | caught |
| `emitToolCall` (L1249) | last result wins; `result.block` short-circuits; result truthiness via `if (handlerResult)` (any truthy value counts) | **NOT caught — rethrows** |
| `emitUserBash` (L1269) | first non-undefined result wins (`return handlerResult`); invalid shape → `throw new Error("Invalid user_bash handler result: return undefined for local execution or exactly one valid { operations } or { result } object")` | caught → emitError, **then rethrown** |
| `emitContext` (L1305) | 2 phases; see below | caught |
| `emitBeforeProviderRequest` (L1368) | `handlerResult !== undefined` → replace payload | caught |
| `emitBeforeProviderHeaders` (L1399) | in-place mutation of `headers` only (return ignored) | caught |
| `emitBeforeAgentStart` (L1427) | see below | caught |
| `emitResourcesDiscover` (L1479) | `result?.skillPaths?.length` etc. → collect `{path, extensionPath: ext.path}` (only `.length`-truthy arrays) | caught |
| `emitInput` (L1525) | transform chain; `action === "handled"` short-circuits (returns result); `action === "transform"` → text/images (images `?? currentImages`); final: `currentText !== text \|\| currentImages !== images` (reference compare!) → `{action: "transform", text, images}` else `{action: "continue"}` | caught |

`emitToolCall`/`emitUserBash` rethrow: harness ops wrap in try/catch →
`emitToolCall!msg` / `emitUserBash!msg` log lines (emitters never rethrow
into other emitters).

### emitContext (the reference-identity model)

TS:
1. `currentMessages = structuredClone(messages)`.
2. Phase `context`: per handler — `visibleMessages = currentMessages.filter(m
   => m.role !== "system")` (element refs shared), `visibleSnapshot =
   visibleMessages.slice()`, `event = {type, messages: visibleMessages}`;
   `returned = handlerResult?.messages ?? (sameMessages(visibleMessages,
   visibleSnapshot) ? undefined : visibleMessages)`; `if (!returned)
   continue; currentMessages = restoreSystemMessages(currentMessages,
   visibleSnapshot, returned)`.
   `sameMessages(a, b)` = equal length && element-wise `===` (reference).
3. Phase `context_with_system`: per handler — `hadLeadingSystemMessage =
   currentMessages[0]?.role === "system"`; `currentMessages =
   handlerResult?.messages ?? currentMessages`; if the head was system and
   is no longer → `emitError("Handler removed the leading system message;
   the request has no prompt or initial tool declarations. Keep it at index
   0 or replace a dropped prefix with getCurrentSystemMessage().")` (handler
   output still honored).
4. Return `currentMessages`.

`restoreSystemMessages(current, visible, returned)`:
`sameMessages(returned, visible)` → keep `current`; else `head =
getCurrentSystemMessage(current)` (FIRST system message anywhere, replayed)
→ `head ? [head, ...returned] : returned`.

**Rust model (documented divergence):** handler results are
`Option<Value>` where `returns.messages` (a JSON array) = a returned array;
in-place mutation is modeled by scripted `mutate` steps on the event's
messages array (set/push/unshift/pop/shift), executed **by replacing whole
elements** (never element-field edits). `sameMessages` = element-wise
serde `Value` equality (reference identity ≡ our script ops never alias
mutated elements). Element-field in-place mutation (invisible in TS via
`===` even when returned!) is a documented divergence: parity cases never
use it (a JSON script cannot distinguish element identity from element
equality when elements are replaced wholesale). The runner API:
`emit_context(messages) -> Vec<Value>` with handler results checked via
`returns` (Some) + `mutate` (structural) — both sides execute the same
script semantics.

### emitBeforeAgentStart (L1427)

```
currentOptions = normalizeBuildSystemPromptOptions(systemPromptOptions)   // throws propagate!
renderCurrentSystemPrompt = () => buildSystemPrompt(currentOptions)
ctx = defineProperties({}, descriptors(createContext()))
ctx.getSystemPrompt = () => { assertActive(); return renderCurrentSystemPrompt(); }
messages = []
per handler:
  event = { ...trigger, type: "before_agent_start",
            get systemPrompt() { return renderCurrentSystemPrompt(); },
            systemPromptOptions: currentOptions }
  handlerResult = await handler(event, ctx)
  if (handlerResult) {                        // truthy check
    if (result.message) messages.push(result.message)
    if (result.systemPrompt !== undefined)    // !== undefined (null IS a set!)
      currentOptions.forceSystemPrompt = result.systemPrompt
  }
return { messages, systemPromptOptions: currentOptions }
```

- `result.systemPrompt = null` → `null !== undefined` → TRUE →
  `forceSystemPrompt = null` → next render:
  `buildSystemPromptState`: `input.forceSystemPrompt !== undefined` → null
  passes → `{content: null}` → `getSystemMessageText` →
  `contentText(null)` → `null.filter` → **TypeError** `Cannot read
  properties of null (reading 'filter')` — thrown at the getter read
  (handler `eventReads: ["systemPrompt"]`) or at `ctx.getSystemPrompt()`,
  inside makeHandler's try → `h{n}.e.systemPrompt!msg` log.
- `emitBeforeAgentStart` itself never catches normalize/render throws —
  `normalizeBuildSystemPromptOptions(null)` → `Cannot read properties of
  null (reading 'customPrompt')` propagates to the op (harness
  `emitBeforeAgentStart!msg`).
- Event field order (eventReads jstr): `{...trigger, type, systemPrompt
  (getter), systemPromptOptions}`.
- The event is a spread of the trigger: user events `{messageType: "user",
  prompt, images?}`, custom `{messageType: "custom", message}`.

#### Raw normalize (faithful JS semantics; op-driven inputs)

The existing `normalize_build_system_prompt_options(input, cwd)` (agent-
session path, options always carry the session cwd) stays. The runner path
gets `normalize_options_raw(input) -> Result<Value, String>` reproducing
`normalizeBuildSystemPromptOptions` field-for-field (V8 error messages,
empirically pinned against the real node at case time):

- `customPrompt`, `forceSystemPrompt`: passthrough (`input.x` — key
  absent = undefined = **omitted**; null preserved).
- `selectedTools: [...(input.selectedTools ?? ["read","bash","edit","write"])]`,
  `hiddenTools: [...(input.hiddenTools ?? [])]`,
  `promptGuidelines: [...(input.promptGuidelines ?? [])]` — **array
  spread**: array → copy; string → code-unit chars; other primitive →
  `TypeError: <v> is not iterable (cannot read property
  'Symbol(Symbol.iterator)')`.
- `toolSnippets: {...(input.toolSnippets ?? {})}` — **object spread**
  (forgiving: primitives → `{}`, arrays → index keys, string → char keys).
- `toolGuidelines: Object.fromEntries(Object.entries(input.toolGuidelines
  ?? {}).map(([n, g]) => [n, [...g]]))` — `Object.entries` on array/string
  → index keys, on primitive → `[]`; inner `[...g]` = array spread (may
  throw).
- `appendSystemPrompt: input.appendSystemPrompt ?? ""` (passthrough
  non-null; falsy check happens later in sections).
- `sections: {...(input.sections ?? {})}`.
- `cwd: input.cwd` — passthrough (undefined preserved).
- `contextFiles: (input.contextFiles ?? []).map(f => ({...f}))`,
  `skills: (input.skills ?? []).map(s => ({...s}))` — non-array non-null →
  `TypeError: <expr>.map is not a function`.

`buildSystemPromptSections` (raw path): section-name throw
`Invalid system prompt section name: ${name}` (name !== "preamble" + regex
`/^[a-z][a-z0-9_-]*$/`); `promptSections.cwd = cwd.replace(/\\/g, "/")` —
cwd undefined/null/number → `TypeError: Cannot read properties of
<undefined|null|number> (reading 'replace')`; `if (appendSystemPrompt)` =
JS truthy; custom sections `if (content)` truthy (only string content
reaches); TS object key order = insertion order.
`buildSystemPromptState`: `forced = input.forceSystemPrompt !== undefined`
→ JSON-null IS forced; forced → `{content: forceSystemPrompt}`; else
`{content: "", sections}`.
`buildSystemPrompt` = `getSystemMessageText({role: "system", ...state,
timestamp: 0})`: parts = `[contentText(content)]` (string passthrough;
array → text blocks join "\n"; **null → throw** `Cannot read properties of
null (reading 'filter')`) + every non-null section value; drop empty parts;
join `"\n\n"`.

Host doc paths: the **non-forced, non-customPrompt** path renders the
`docs` section with host paths (`getReadmePath()` etc.) — the harness
**pins `customPrompt`** (the established agent-session parity convention);
`DocPaths` placeholders stay unobservable.

### emitBoundary host callback

`buildContext(entries) → BoundaryContextPreview` is host-side. Harness op
`emitBoundary {baseEvent, buildContext: {mode: "rec"|"throw"|{steps…},
preview?, throwOn?: entries[], throwMsg?}}` — TS side builds
`buildContext = async (entries) => { log("bc:" + jstr(entries)); if
(throwOn…throw) return preview ?? null }`. Every invocation logs
`bc:jstr(entries)`; buildContext runs (a) initially with `[]`, (b) after
every handler, (c) once more in the failure path? — no: on handler error
the same `try { context = await buildContext(entries) }` block runs (the
error catch and the buildContext catch are siblings). Initial + per-handler
(2 catch blocks per iteration). `emitError("Invalid boundary entries: " +
err.message)` uses the throw message; `stack` = undefined (jstr omits).

## Microtask engine (withUIPrompt + mcp_servers_change)

TS timing facts (verified against the real code):

1. The registry change listener (bindCore L465) runs `void
   this.emit(mcp_servers_change)` — **immediate** emit start: the emit loop
   runs synchronously up to the first `await handler(...)`, the handler
   (makeHandler) runs synchronously up to its first `await
   Promise.resolve()` — so the handler *body* (its logs) is deferred to the
   microtask queue, **pushed at mutation time**.
2. `withUIPrompt` (L594): start event `queueMicrotask(() => void
   this.emit(...))` (pushed at prompt start); `finish()` (run() settlement)
   pushes the end event. Handler bodies/loop resumptions are pushed as the
   queue drains.
3. Each `await` resumption is a queued microtask: handler body resume (`r`)
   runs its logs and resolves; the emit-loop resume (`e`) processes the
   result (session_before/error → emitError) and invokes the next handler
   (which suspends at its first await → next `r`). `queueMicrotask(m)`
   (start/end) pushes `m`, which itself pushes the first `r`.
4. The op loop's `await new Promise(setImmediate)` runs **after** all
   microtasks: two same-op mcp changes interleave as
   `h1:A, h1:B, h2:A, h2:B`; a ui_prompt start/end pair (1 handler)
   interleaves as `h1:ui_prompt_start, h1:ui_prompt_end` (m2/h1end-r
   runs between m1's r and its loop e), and with 2 handlers:
   `h1:start, h1:end, h2:start, h2:end`.

Rust harness engine (replaces `drain_mcp_events`): a `VecDeque<Slot>` with
`Slot::Start{job}` | `Slot::Res{job, h}` | `Slot::Loop{job, h, result}`,
`job = {event, handlers, kind}`. Push rules: registry mutation → push
`Res{job, 0}` (immediate-start: the emit loop invoked h[0] synchronously,
which suspended); ui prompt start/end → push `Start{job}`; `Start` pop →
invoke h[0] → push `Res{job, 0}`; `Res` pop → run the handler body (log →
reads → calls → eventReads/mutate → returns/throws) → push
`Loop{job, h, result}`; `Loop` pop → result processing (error →
`emitError` sync; session_before logic) → next handler → push `Res{job,
h+1}` or done. The engine drains to exhaustion after every op.
Handler snapshots are taken **at push time** (mutation time) — a handler
registered later in the same op (via an `api`-op step) does not observe an
earlier mutation (real TS behavior; the old per-op-drain snapshot was
taken too late).

`reportUnhandledMcpServers()` stays synchronous inside the listener (runs
with the registry lock held; consumes the passed list).

## withUIPrompt details

```
withUIPrompt(kind, title, run):
  outerPrompt = this.uiPromptDepth++ === 0
  if (outerPrompt) { activeUIPrompt = {kind, title}; emitUIPromptEvent({type: "ui_prompt_start", reason: "ui_prompt", kind, ...(title ? {title} : {})}) }
  finish = () => {
    if (--this.uiPromptDepth > 0) return;
    this.uiPromptDepth = 0;                      // double guard (negative-safe)
    prompt = this.activeUIPrompt ?? {kind, title}
    this.activeUIPrompt = undefined
    emitUIPromptEvent({type: "ui_prompt_end", reason: "ui_prompt", kind: prompt.kind, ...(prompt.title ? {title: prompt.title} : {})})
  }
  try { return run().finally(finish) } catch (err) { finish(); throw err }
```

- `run().finally(finish)` — end event pushed (microtask) when run settles,
  **after** a sync throw path also calls `finish()` (the catch calls
  finish again → `--depth` again! `if (--depth > 0) return; depth = 0` —
  the double-finish decrements twice; depth 0 → -1 ≤ 0 → second end event
  emitted?! — NO: after first finish, depth = 0; catch's finish:
  `--this.uiPromptDepth` → -1; `-1 > 0` false → `depth = 0` → prompt =
  activeUIPrompt ?? {kind, title} → **second ui_prompt_end**! — wait:
  run() sync throw → `.finally` never attached?? `run().finally(finish)`
  — run() throwing SYNCHRONOUSLY (before returning a promise) → the
  expression throws → `.finally` never called → catch → finish() (FIRST
  call). Only one finish. A rejected-promise run → finally(finish) → finish
  once; catch not taken (no sync throw). So double-finish never happens.
- Nested prompts (custom → select): inner start (depth 1 → outerPrompt
  false — NO start event), inner finish (depth 1 → 0 → **end event** with
  the OUTER kind/title (activeUIPrompt = outer's!), outer finish (depth 0
  → -1?? — no: outer start depth 0→1, inner start 1→2, inner finish 2→1
  `1 > 0` → return (NO end event), outer finish 1→0 → end event with
  activeUIPrompt (outer)). Hmm — inner finish at depth 2→1: `if (1 > 0)
  return` → NO inner end. Only ONE end (outer, at outer finish). But
  nested uiCall ops (harness ops are sequential) — nesting only happens if
  the fake UI's method itself calls another prompt method (scripted
  nesting) — out of harness scope; ui_prompt_end fires once per
  start.
- `getUIContext()` returns the wrapped context (`{...ui, select:
  wrapped…}` — spread + 5 wrapped keys).

## Harness protocol additions (ts-harness.js + extension_runner_harness.rs)

- Handler spec extensions: `eventReads: string[]` (read fields off the
  EVENT: `h{n}.e.{field}={jstr}` / `!{msg}` — after ctx reads/calls),
  `mutate: [{field: "messages", op: "set"|"push"|"unshift"|"pop"|
  "shift", index?, value?}]` (after eventReads; whole-element replace),
  `returns`, `throws` (existing).
- `bindCore` op: `actions` gains `{sendMessage|sendUserMessage|
  appendEntry|setSessionName|setLabel|getAllTools|getSettings|
  setActiveTools|getCommands|setModel|setThinkingLevel: mode|"throw",
  …}` (record `a.<m>:jstr([args])`; `getSessionName` returns cfg;
  `getSettings`/`getAllTools`/`getCommands` return cfg values) +
  existing `activeTools`/`thinkingLevel`/`refreshTools`. `contextActions`
  gains `{scopedModels, signal, abort (rec), hasPendingMessages,
  contextUsage, compact (rec), systemPrompt, systemPromptOptions: {mode
  rec|"throw"|missing, returns?}, executeTool: {mode: rec|"throw",
  returns?}|missing, callableTools}` (record `ca.<m>:jstr(args)`).
  `providerActions` unchanged. Old op shapes (4 contextActions keys)
  keep behavior — the TS-side object literal now always provides all
  methods (defaults mirror the class-field defaults).
- `bindCommandContext` op: `{waitForIdle: "rec", newSession: {mode:
  "rec"|"throw"|"cancelled", result?}, fork/navigateTree/switchSession:
  {…}, reload: "rec"}` — record `cc.<m>:jstr([args])`, `cancelled`
  returns `{cancelled: true}`, throw `"ns boom"`. No-arg op → defaults.
- `setUIContext` op: `ui: true` → `{}` (old, hasUI=true); `ui: {<method>:
  {returns?, throws?}…}` → the fake UI (methods log
  `ui.<m>:jstr([args])`); `ui` falsy/absent → undefined → no-op.
- `uiCall` op: `{method, args: [...]}` → `await
  getUIContext()[method](...args)` → `uiCall.<method>:return:jstr(r)` /
  `uiCall.<method>!<msg>`. All no-op methods callable (select →
  undefined, confirm → false, notify → nothing). jstr(fake-UI objects) =
  `{}`.
- `createToolContext` op: `{toolCallId, signal?}` → returns a context
  proxy: `tct` op steps `{read: field}` / `{call: method, args?}` /
  `{callExecute: {name, args, options}}` → log `tct.{field}={jstr}` /
  `tct.{field}!{msg}` / `tct.executeTool:return:jstr` / `!{msg}` /
  `tct.call:{m}:ok`. `reads` invoke methods (tools/executeTool read via
  `tct.{field}`).
- `createCommandContext` op: same proxy steps + `newSession/fork/
  navigateTree/switchSession/reload/waitForIdle/getSystemPromptOptions`
  calls log `ccWait.<m>:jstr(args)` + `ccWait.<m>:return:jstr` /
  `!{msg}`.
- New emitter ops (all take full event/value configs):
  `emitBoundary`, `emitCacheWarmingDecision` (log
  `emitCacheWarmingDecision:return:jstr`), `emitToolResult`,
  `emitToolCall`, `emitUserBash`, `emitContext` (log
  `emitContext:return:jstr(messages)`), `emitBeforeProviderRequest`,
  `emitBeforeProviderHeaders`, `emitBeforeAgentStart` (log
  `emitBeforeAgentStart:return:jstr({messages,
  systemPromptOptions})`), `emitResourcesDiscover` (log
  `emitResourcesDiscover:return:jstr({skillPaths, promptPaths,
  themePaths})`), `emitInput` (log `emitInput:return:jstr`).
  `emitToolCall`/`emitUserBash` ops catch rethrows →
  `<op>!<msg>`.
- api script steps: `sendMessage {message, options?}`, `sendUserMessage
  {content, options?}`, `appendEntry {customType, data?}`,
  `setSessionName {name}`, `getSessionName`, `setLabel {entryId,
  label?}`, `setModel {model?}` (awaited), `setThinkingLevel {level?}`,
  `getAllTools`, `getSettings`, `setActiveTools {names}`,
  `getCommands`, `exec {command, args?, options?}` (awaited →
  `api.exec:return:jstr({stdout, stderr, code, killed})` /
  `api.exec!<msg>`).
- Factory scripts run with `runScript` — new steps record via the bound
  actions (pre-bind steps throw `caught:Extension runtime not
  initialized. …`).

## exec.ts port (`pi-session/src/agent_session/exec.rs`)

`exec_command(command, args, cwd, options) -> ExecResult {stdout, stderr,
code, killed}`:
- `spawn(command, args, {cwd, shell: false, stdio: ["ignore", "pipe",
  "pipe"]})` — Rust `std::process::Command` w/ piped stdio, no shell.
- `killProcess()` = first call sets `killed = true` + SIGTERM; 5 s later
  SIGKILL (real timer; parity cases don't hit the 5 s path).
- abort signal: pre-aborted → kill immediately; else the engine slot
  aborts (parity: `signal: "aborted"` in script options → a pre-aborted
  flag).
- timeout > 0 → kill on expiry (real sleep).
- stdout/stderr accumulated as **UTF-8 lossy strings** (JS
  `data.toString()`).
- `waitForChildProcess` → resolve `{code: code ?? 0, killed}`; spawn error
  (ENOENT) → reject → resolve `{code: 1, killed}`; **spawn-error stderr
  stays empty**.
- Documented divergence: `waitForChildProcess`'s 100 ms post-exit pipe-idle
  grace (detached descendants) — Rust resolves on the pipe reads' natural
  end (stdout/stderr drained to EOF). Parity cases use self-terminating
  commands.
- Parity exec cases: `/usr/bin/printf` (stdout), stderr, exit codes,
  `sleep` + timeout (killed=true), missing binary (code=1), pre-aborted
  signal. Real processes on both sides (bwrap allows exec).
- `pi.exec(command, args, options)` (loader L413): `assertActive()` →
  `execCommand(command, args, options?.cwd ?? cwd, options)` — cwd
  fallback = the extension's loading cwd (harness: "/proj").

## Divergences (documented, observably preserved)

1. `emitContext` reference-identity vs `Value` equality (see above).
2. `ExtensionError.stack` never populated (existing).
3. Handler event snapshots (existing) — except `emitContext`/
   `emitBeforeAgentStart`/`emitBoundary`/`emitToolResult`/`emitInput`
   events, which are constructed per the runner semantics (fresh event
   objects per handler; `mutate` steps model in-place array edits).
4. `exec`'s pipe-idle grace (above).
5. `emitToolCall`'s `if (handlerResult)` — truthy check on the raw result
   (Rust: `js_truthy`).
6. `queueMicrotask` ordering = the harness microtask engine (harness-level;
   the runner library exposes a push seam).
7. `sessionManager`/`modelRegistry`/`signal`/`ui` ctx values = jstr-`{}`
   stand-ins (null/fake objects both sides).
8. V8 error messages for the raw-normalize/section/cwd TypeError paths are
   pinned empirically (case-time `node -e` probes; cases lock the exact
   strings).
9. `wrapUIPromptContext` spread preserves the *fake-UI* non-prompt methods
   verbatim (both sides `{...ui, …}` over the same scripted object).
10. `withUIPrompt` sync-throw vs rejected-promise paths (above) — harness
    fake-UI methods never throw synchronously (`async` arrow), so only the
    `.finally` path is observable.

## Implementation checklist

- [x] 1. `exec.rs` (exec.ts + waitForChildProcess subset) + unit tests
      **DONE (396a92a)**: probe-pinned extras — signal-kill →
      `{code: 0, killed: true}` (exitCode ?? 0); ENOENT-**and-bad-cwd**
      → RESOLVED `{code: 1, killed, empty-stderr}`; non-string command /
      non-array args → REJECT (exact node messages); **args elements
      string-coerced** ([5] → "5", [null] → "null"); no shell; timeout >
      0 (full JS ToNumber); pre-aborted + ENOENT+pre-aborted =
      killed:true; 29 tests + clippy clean
- [x] 2. `extension_runner.rs`: full `ExtensionActions` trait + pre-bind
      stub matrix **DONE**: `RuntimeActions` = all 15 actions (Value-typed;
      `Err` = throw / rejected promise); pre-bind stubs = loader.ts L158
      ("Extension runtime not initialized. Action methods cannot be called
      during extension loading."), `refreshTools` no-op, `setModel` rejects
      with the SHORT message ("Extension runtime not initialized"). Unit
      test covers the matrix.
       stub matrix (setModel rejected-promise = async Err)
- [x] 3. `ExtensionApi` action methods + `pi.exec` **DONE**: all 15
      actions = `assertActive()` (failed-load / stale messages) then runtime
      delegation; `pi.exec` = assertActive → `execCommand(command, args,
      options.cwd ?? loadingCwd, options)` (API carries the loader cwd).
      Test: pre-bind API call throws the loader stub, exec runs during
      loading, stale runtime fails assertActive.
       delegation)
- [x] 4. Full `ContextActions` + `CommandContextActions` traits +
      `bind_command_context` **DONE**: ContextActions = 13 methods (the 3
      optional ones — getSystemPromptOptions/executeTool/getCallableTools —
      are Rust default impls returning None, matching the TS optionality);
      CommandContextActions = 6 future-based methods;
      `RunnerShared.command_actions` + `bind_command_context(Option<..>)`
      (`None` = the TS default handlers).
       `bind_command_context`
- [x] 5. `RunnerShared` extension **DONE**: `ui_context`
      (`Option<Arc<dyn UiContext>>`; `None` = noOp), `ui_prompt_depth`,
      `active_ui_prompt`, `ui_prompt_queue` (the `queueMicrotask` seam,
      drained via `drain_ui_prompt_events`), `command_actions`; the old
      `ui: bool` field retired (`hasUI` = a context is bound; `ctx_read`
      routes through it).
       activeUIPrompt, command actions, context defaults pre/post-bind,
       microtask push seams (`ui_prompt_emit`)
- [x] 6. `set_ui_context` full + `wrap_ui_prompt_context` **DONE**:
      `set_ui_context(Option<Arc<dyn UiContext>>, mode)`; `UiContext` =
      one Value-typed `invoke(method, args)` seam (the fake-UI script
      backs it) + runner-level `ui_select`/`ui_confirm`/`ui_input`/
      `ui_editor`/`ui_custom`; `with_ui_prompt` = outermost-only
      start/end events, depth clamp (finish double-guard), end event
      carries the active prompt, throw still emits end; **noOp context is
      unwrapped** (no events — TS parity). Test covers no-op/nested/throw/
      custom.
       `with_ui_prompt` (kind/title/start/end + finish double-guard) +
       `get_ui_context` proxy
- [x] 7. `ctx_read` full getter map **DONE**: getters = ui/sessionManager/
      modelRegistry (Null stand-ins: function bags), mode, hasUI, cwd,
      model, scopedModels, **thinkingLevel (loader stub throws pre-bind)**,
      signal; methods = `ctx_call`/`ctx_call_args`: isIdle/isProjectTrusted/
      hasPendingMessages/getContextUsage/getSystemPrompt/compact/abort/
      shutdown with the class-field defaults pre-bind (true/true/false/
      undefined/""/no-op) and `getSystemPromptOptions` →
      normalize({cwd}). Test pins both sides.
       class-field defaults; jstr stand-ins)
- [x] 8. `create_tool_context`/`create_command_context` **DONE**:
      ToolContextView = guarded `tools` getter (absent → []) +
      `execute_tool` with the exact canned fallback
      (`{toolCall:{type,id:"<id>/0",name,arguments:{}}, result:{content:
      [text "Nested tool calls are not available in this context"],
      details:{}}, isError:true}`) and the `signal: options.signal ??
      signal` merge; CommandContextView = getSystemPromptOptions +
      waitForIdle/newSession/fork/navigateTree/switchSession/reload with
      the default `{cancelled:false}`/no-op handlers; every entry guarded
      by the shared stale check. Test: fallback object, defaults, stale.
       call fallback; executeTool options merge)
- [x] 9. `is_user_bash_event_result` + `emit_user_bash` **DONE**: exactly
      one of operations/result (operations.exec = key-presence stand-in);
      result needs string output, **present** exitCode (number/undefined
      — Null in the jstr model), boolean cancelled/truncated, optional
      string fullOutputPath. First non-undefined result wins; invalid →
      emitError + the exact Invalid-user_bash message RETHROWN; handler
      throws likewise reported + rethrown. Validator matrix + emitter
      tests.
       Invalid-user_bash… → rethrow)
- [ ] 10. `emit_boundary` (buildContext host hook trait + Invalid boundary
        entries) + `emit_cache_warming_decision` + `emit_tool_result`
        (delete-structuredContent) + `emit_tool_call` (truthy + block +
        rethrow) + `emit_before_provider_request`/`_headers` +
        `emit_resources_discover` + `emit_input`
- [ ] 11. `emit_context` (2-phase; sameMessages Value-eq;
        restoreSystemMessages head; leading-system-lost error)
- [x] 12. `system_prompt.rs`: `normalize_options_raw` (full JS semantics) +
        `build_system_prompt_state` + `build_system_prompt` +
        `contentText`/`getSystemMessageText` (null-content TypeError) +
        cwd.replace TypeError + sections truthy checks
- [x] 13. `emit_before_agent_start` + `ExtensionHandler` mutation
      read-back **DONE**: `HandlerOutcome { event, result }` — handlers hand
      their event copy back, so in-place edits are observable (TS aliasing)
      → `emitContext` messages / `emitBeforeProviderHeaders` headers /
      `before_agent_start.systemPromptOptions` live-ref all modelable.
      Emitter: leading `normalize_options_raw` (throw ESCAPES), per-handler
      fresh `{...trigger, type, systemPromptOptions: current, systemPrompt}`
      event, `ctx.getSystemPrompt` = assertActive + render(live options),
      truthy-result gate (message truthy-push; `systemPrompt !== undefined`
      = present-key, **null reassigns**), in-place options write-back,
      handler errors → `emitError`. Runner gained `set_doc_paths`
      (host injects; default empty). 8 unit tests (merge/null-reassign/
      live-options/normalize-throw/render-surfaces/event+ctx render).
      **Divergence**: `event.systemPrompt` is materialized per handler
      (options change only between handlers; a render error omits the key
      where TS throws at read — reads surface via ctx.getSystemPrompt).
        systemPrompt getter; truthy result; `!== undefined` systemPrompt)
- [ ] 14. Harness: microtask engine for mcp AND ui_prompt (replaces
       `drain_mcp_events`; push `Res{job,0}` at mutation-apply (active) /
       commit-apply (loading) + `OpMark` at load-resolution + `Start{job}`
       at ui_prompt start/end — see Addendum B)
- [ ] 15. Harness: all new ops + handler-spec extensions (both sides)
- [ ] 16. Cases: ~60–80 new `exr/` (lifecycle/actions, context reads,
        command context, ui prompt + engine interleave, exec, every
        emitter × result/error shapes, emitContext mutation matrix,
        before_agent_start incl. forceSystemPrompt-null + normalize V8
        errors)
- [ ] 17. Full parity + workspace tests + clippy clean (new code)
- [ ] 18. TRACKING.md + this doc → COMPLETE; feat commit + docs commit

## Build env note

Same as tranche 5: `export RUSTUP_HOME=$HOME/AI/.rustup CARGO_HOME=
$HOME/AI/.cargo PATH=$HOME/AI/.cargo/bin:$PATH` per bash call; parity via
`node parity/run-parity.mjs`; clippy baseline (pre-existing pi-cli
args.rs warnings) tolerated, new code clean.

## Addendum A — V8 error-message catalog (probed against compiled dist;
error strings in parity come from the dist = the arbiter)

### normalizeBuildSystemPromptOptions (L57)

- `normalize(null)` → `Cannot read properties of null (reading
  'customPrompt')`; undefined → same with `undefined`. In
  `emitBeforeAgentStart` the options argument is always present
  (op-provided); model options as `Option<&Value>`;
  `Null`/`None` → **the emitter throws out** (no emitError), op log
  `!Cannot read properties of null (reading 'customPrompt')`.
  **Rust pitfall**: `Value::Null.get("customPrompt")` = None ≠ throw —
  the Rust port must explicitly match Null/undefined variants.
- Primitive input (5/0/"s"/true/[1,2]) → **tolerated**: property access on
  a primitive yields undefined, every `??` falls through → full defaults
  (`n(5)`, `n(0)`, `n("s")`, `n([1,2])`, `n(true)` = defaults;
  `n(5).cwd` = undefined).
- `selectedTools: 5|{}|true` → `(input.selectedTools ?? ["read",
  "bash","edit","write"]) is not iterable` — full compiled expression
  text, NO `(cannot read its property 'Symbol...')` suffix. Same shape
  for `hiddenTools`/`promptGuidelines` (with their own `?? []` text).
- `selectedTools: "abc"` → `["a","b","c"]` (spread chars); `"ab"` same.
- `toolGuidelines: 5` → `{}` (Object.entries(5) = []);
  `toolGuidelines: "ab"` → `{"0":["a"],"1":["b"]}` (fromEntries of
  entries-of-string); `toolGuidelines: {x:5}` → **throw** `guidelines
  is not iterable` (V8 names the variable `[...guidelines]`, minimal
  text); `null` → `{}`.
- `toolSnippets`/`sections`: 5 → `{}`; `"ab"` → `{"0":..,"1":..}`;
  `sections: ["a"]` → `{"0":"a"}` (spread array).
- `contextFiles`/`skills`: 5|"ab" → `(input.contextFiles ??
  []).map is not a function` (same text for `skills`); null → `[]`;
  `[5, null, "ab", []]` → `[{},{},{"0":"a","1":"b"},{}]`
  (element spread: primitives `{}`, strings char-keys).
- `appendSystemPrompt: 0`/`false` → **kept** (?? skips only
  null/undefined); null → `""`.
- `cwd: 5` → `"5"` (?? tolerates); `cwd: null` → `""`?? — NO:
  `"cwd": null` → `""` (?? → "" default... actually `?? ""` → "")
  — probe-confirmed: null/undefined → `""`, falsy-but-present (0,
  "", false) → String(v) ("0", """, "false").

### buildSystemPromptSections (L128)

- Name validation loop FIRST over `Object.keys(customSections)`:
  `/^[a-z][a-z0-9_-]*$/`; "A"/"0"/"preamble" ("preamble" IS rejected)
  → throw `Invalid system prompt section name: ${name}`.
- `customPrompt` **truthy** → `promptSections.preamble = customPrompt`,
  skipping the default preamble+tools+rules+docs block entirely.
- `customPrompt` **falsy (incl. null!)** → default sections
  (preamble, tools, `rules = buildRules(declaredTools, toolGuidelines,
  promptGuidelines)`, docs).
- Custom sections run LAST: `for (const [name, content] of
  Object.entries(customSections)) if (content)
  promptSections[name] = content` — **truthy-kept, template-coerced,
  overwrite built sections** (same name = replaces).
- `sections = {preamble: raw}` (preamble NOT wrapped) + every other
  key `` `<${name}>\n${content}\n</${name}>` `` — content
  template-coerced (""" → empty tag body).
- `rule.trim is not a function` (rules non-string element); skills/
  escapeXml undefined field → `Cannot read properties of undefined
  (reading 'replace')`.
- `cwd.replace(/\\/g, "/")` non-string cwd → TypeError (cwd.replace
  is not a function) — reached only post-normalize (cwd = "5" etc.
  never throws).

### getSystemMessageText / contentText

- `getSystemMessageText(msg)`: `parts.filter(part => part.length > 0)`
  — **silently drops non-strings** (numbers/objects → length undefined
  → false → filtered OUT); null content (msg.content = null) →
  `contentText(null)`?? → `null.filter`?? — no: contentText(null)
  returns ""?? — probe: msg `{role:"system", content: null}` →
  TypeError **`Cannot read properties of null (reading 'filter')`**
  (contentText's Array.isArray(null) = false → null.filter path);
  non-string non-array (5) → `content.filter is not a function`;
  **arrays ARE contentText'd** (content = ["a", 5] → joined "" —
  non-strings in array → content.map... → "a" + ""?
  — array elements non-string → contentText(elem) → String(elem)
  — `"a"`+`"5"` = "a5"?? — probe cp5: ["a", 5] → "a5"... actually
  contentText(5)?? undefined branch → String(5)? — pinned: array
  elements go through contentText(elem) = elements non-string
  non-array → `String(elem)`; null elem → TypeError? NO —
  contentText(null): null?.filter — TS source null-coalesces??
  — PROBED: null elem → throws `Cannot read properties of null
  (reading 'filter')`?? → NO: cp-probe = contentText(null) →
  **TypeError null filter** only at TOP level; array elements:
  `[null]` → part.length on ""?? — array content [null] →
  contentText(null) → throws. ["a",5] → "a5"?? → "a" + "5" = "a5"
  — 5.length = undefined → filtered? NO: the filter runs on the
  RESULTING parts (content.map(contentText)); map first (every
  element contentText'd) then filter(part.length > 0).
  contentText(null) THROWS → [null] throws; ["a",5] →
  map → ["a", "5"] → filter → ["a","5"]??  "5".length=1 → kept
  → join → "a5". "" elements → filtered. **Pinned.**
- `buildSystemPromptState(msgs)`: forced check
  `msg.options?.forceSystemPrompt !== undefined` — null/false/0 all
  **force** (null ≠ undefined); forced arrays ARE contentText'd
  (null content → the `'filter'` TypeError).

### emitProjectTrustEvent / renderProjectContext

- `emitProjectTrustEvent` non-object result tolerated (r.result =
  jstr'd as-is).
- `renderProjectContext({5})` / primitives → spread → `{}` →
  template "undefined"?? — probe: `{...5}` = {} → `${pc.something}`
  → "undefined"; null → "null" (template of null).

## Addendum B — P1 corrected: the microtask engine IS needed for mcp

**P1 as originally resolved ("no MQ for mcp; post-op drain = same
order") was WRONG.** The full microtask traces (re-derived
end-to-end against ts-harness L3571–3578 makeHandler + L3674–3691
load op + L3806–3807 per-op settle + loader L514 `await
factory(load.api)` L515 `load.commit()` + runner L244–246
listener):

- makeHandler L3571: `async (event, ctx) => { await
  Promise.resolve(); log(\`h${id}:${event.type}\`); ... }` —
  **entire handler body is one microtask** (H) pushed at
  `await handler(...)` (emit loop L1078, sync up to it).
- The api-op mutation chain: `registerMcpServer` (api step, sync
  op code) → `applyRuntimeChange` (loader L200): **state =
  "loading" → `pendingRuntimeChanges.push`** (queue!); state =
  "active" → run now. `load.commit()` (L434–444) runs pending
  changes **inside the `await factory` continuation F** — so
  **load-time (factory-script) mutations apply at commit (mid-F),
  api-time mutations apply in the op step sync**.
- Listener (runner L244, installed in **bindCore**): `void
  this.emit({type: "mcp_servers_change", servers:
  mcpServers.list()})` (sync emit start, fire-and-forget) +
  **`this.reportUnhandledMcpServers()` SYNCHRONOUSLY inside the
  listener** (with the registry lock held → consumes the passed
  list; no-op if `mcp_servers_change` has handlers;
  `reportedMcpServers` name-Set dedup).
- Per-op settle L3806: `await new Promise((r) =>
  setImmediate(r))` after EVERY op — all microtasks drain
  between ops; errors from H/E land within the same drain.

**Trace 1 — api-time M=2 (two `registerMcpServer` api steps in one
op) × N=2 handlers (both registered pre-mutation):**
mutation A (op step) → emitA starts sync → `await h1A` →
H(A,1) pushed; mutation B (next op step) → emitB sync →
`await h1B` → H(B,1) pushed. Op end → setImmediate → drain:
H(A,1) (`h1:A`) → E(A,1) pushed → H(B,1) (`h1:B`) → E(B,1) →
E(A,1) (`await h2A` → H(A,2) pushed) → E(B,1) (`await h2B` →
H(B,2)) → H(A,2) (`h2:A`) → E(A,2) → H(B,2) (`h2:B`) → E(B,2)
→ done.
**TS = `h1:A, h1:B, h2:A, h2:B`** — per-HANDLER rounds,
mutations interleaved WITHIN each round. Rust post-op full-emit
drain = `h1:A, h2:A, h1:B, h2:B` — **MISMATCH. M=1 (existing
case mcp-handled-change) is the only reason the current Rust
passes.**

**Trace 2 — load-time (factory script) M=1 × N=2:** load op →
`await loadExtensionFromFactory(async (pi) => runScript(pi,
script), ...)` → factory body (runScript) runs **fully sync**
(registerMcpServer → pending) → factory promise resolves →
`await factory` continuation F pushed (microtask) → op loop
suspends at `await LEFF(...)`, its continuation (OP = `log
load:ok; extensions.push`) pending on LEFF's promise. F runs:
`commit()` → applies pending → mutation → emit starts sync →
`await h1` → **H(1) pushed**; reportUnhandled runs sync at
mutation; `commit()` returns → `state = "active"` →
`clearPending()` → `return extension` → LEFF's promise resolves
→ **OP pushed (queue end, after H(1))**. Drain: H(1) (`h1:...`)
→ E(1) pushed → OP (`load:/ext/a.js:ok`) → E(1) (`await h2` →
H(2)) → H(2) (`h2:...`) → E(2) → done.
**TS = `h1:…`, `load:path:ok`, `h2:…`** — `load:ok` between
handler 1's body and the emit-loop resumption.

**Pre-bindCore mutations** (every existing `load` op precedes
`createRunner`/`bindCore`): listener not yet installed (bindCore
L244 installs it) → **no emit, no report at commit** — servers
sit in the registry ("registered during loading are read on
session_start", runner L242); `reportUnhandledMcpServers` ops
report manually (name-Set dedup). Post-bindCore load ops
(extension loaded late) → full Trace-2 with report-unhandled
sync at commit (new-case territory).

**P1 CORRECTED → Rust engine (L294 model stands, for mcp AND
ui_prompt):** `VecDeque<Slot>`, `Slot =
Start{job} | Res{job,h} | Loop{job,h,result} | OpMark(log
line)`, `job = {event, handlers, kind}` (handler snapshots
taken **at push time** — a handler registered later in the same
op does not observe an earlier mutation). Push rules:
- registry mutation-apply (active) or commit-apply (loading) →
  push `Res{jobA, 0}` (emit invoked h[0] sync, which
  suspended) + reportUnhandled **sync at push** (in the
  mutation, not the engine);
- load-resolution (Rust `load()` completing) → push
  `OpMark("load:{path}:ok")` (Trace 2);
- ui_prompt start / `finish()` end → push `Start{job}` (L594
  `queueMicrotask(() => void this.emit(...))`);
- engine drains to exhaustion after every op (replaces
  `drain_mcp_events`);
- `Start` pop → emit starts → push `Res{job, 0}`; `Res` pop →
  run the handler body (log → reads → calls → eventReads →
  mutate → returns/throws) → push `Loop{job, h, result}`;
  `Loop` pop → result processing (error → `emitError` sync;
  session_before logic) → next handler → push `Res{job, h+1}`
  or done; `OpMark` pop → push the log line.

This reproduces Trace 1 (Res{A,0} → h1:A/Loop{A,0} → Res{B,0} →
h1:B/Loop{B,0} → Loop{A,0} → Res{A,1} → h2:A → ... =
h1:A,h1:B,h2:A,h2:B), Trace 2 (Res{A,0} → h1 → OpMark(load:ok)
→ Loop{A,0} → Res{A,1} → h2 = h1, load:ok, h2), M=1/N=1
(Res → h1 → Loop → done), and the ui_prompt interleave
(push-point trace: `hN:x` → `ui.select:[...]` (fake sync) →
`hS:ui_prompt_start` → `hN.ret` (H resume) → `hE:ui_prompt_end`).

`reportUnhandledMcpServers()` (manual op) stays as-is (runner
method). The `emitError`-on-`register_provider`-flush loops
(runner L249–282) run at bindCore (sync) — unchanged.

## Addendum C — P2 + emitter edge semantics (verbatim-pinned)

- **P2 `emitBeforeProviderRequest`** (L1075–1080):
  `if (handlerResult !== undefined) currentPayload =
  handlerResult` — **null REPLACES** (null ≠ undefined). Errors
  → `emitError` → continue (loop survives).
- **`emit`** (L1068+): `currentPayload = payload` per emit
  (NOT per handler); `session_before` reads (L1095+)…
  (as ported); errors → `emitError` + continue.
- **`emitCacheWarmingDecision`** (L860):
  `if (result?.action !== undefined)` override (null ≠
  undefined — {action: null} overrides).
- **`emitMessageEnd`** (L877): falsy `.message` → skip
  (return); **role mismatch** check (result.role !==
  currentMessage.role): undefined role → mismatch error
  ("Session response message role \"…\" was overwritten…");
  loop continues (no rethrow).
- **`emitToolResult`** (L902): `if (!handlerResult) continue`
  (falsy-skip); merge = 5-field `??`?? — NO: 5-field spread
  merge `{...currentMessage, ...pick(handlerResult,
  ["toolCallId","toolCallId","description","content",
  "structuredContent"])}`?? — **verbatim**: merge only
  present (≠ undefined) keys; **JSON null ≠ undefined =
  replaces**; `structuredContent` undefined → **deleted** from
  the merged message.
- **`emitToolCall`** (L932): **no ext, no try/catch** — throws
  PROPAGATE (op log `emitToolCall!...`); truthy result
  (`if (handlerResult)`) → merge; `result.block` truthy →
  short-circuit (return blocked, skip remaining extensions).
- **`emitUserBash`** (L961): `handlerResult === null` →
  **invalid** (typed null check, null ≠ undefined); invalid
  (non-object / null) → `emitError("Invalid user_bash handler
  result ...")` + **RETHROW** ("Unsupported user_bash handler
  result type: ..."?? — verbatim: error report + throw
  `Invalid user_bash handler result ...`); first valid wins
  (subsequent handlers skipped).
- **`emitContext`** (L995): 2-phase (current/visible);
  `handlerResult?.messages ?? (sameMessages ? undefined :
  visible)` — null → identity check ("Reference-identical null
  result"); 0/""/false → falsy-continue?? — NO: `??`-based →
  `[].messages`... — verbatim: `const m = handlerResult !==
  null && handlerResult !== undefined ?
  handlerResult.messages : undefined; if (m ??
  (sameMessages ? undefined : visible)) !== undefined...`??
  — see divergences: **pinned**: `handlerResult?.messages ??
  (sameMessages ? undefined : visible)` — result null /
  undefined / `.messages` null/undefined → identity check
  (sameMessages = messages === visible); `.messages` = 0/""/
  false?? → `??` skips → same → identity; **`[]` truthy →
  restore-empty**; 5-field?? — merge = `messages =
  restoreSystemMessages(current, visibleSnapshot,
  returned)` (head = system messages of current (leading
  system-lost → error...)); non-array truthy messages →
  TypeError.
- **`emitResourcesDiscover`** (L1030):
  `result?.promptPaths?.length` truthy?? — **verbatim**:
  `if (result?.[key]?.length)` per key (skillPaths /
  promptPaths / themePaths) → push `{path, extensionPath}`;
  "ab" (string) → `result.x.map is not a function` → caught
  (emitError). Non-0 falsy length ("") → skipped.
- **`emitInput`** (L1207) — most subtle:
  result.action: "handled" → **return result as-is**;
  "transform" → `currentText = result.text` (**absent →
  undefined → changed!**), `currentImages = result.images ??
  currentImages` (null/undefined → keep; **any non-nullish →
  changed := true** — reference identity ≠ even for equal
  JSON); final `currentText !== text || currentImages !==
  images` — **text = primitive value-eq; images = reference
  identity**; `{action: "transform", text: undefined}` → key
  dropped by `jstr` (JSON.stringify drops undefined keys;
  null keys KEPT). Rust model: `JSVal` enum (`Undefined` |
  `Val(Value)`) for result.text/result.images identity
  tracking.

## Addendum D — remaining divergences + api surface

1. **normalize-null throw** (Addendum A) — Rust must explicitly
   throw on `Null` input (Value::Null.get() = None ≠ throw).
2. **`emitToolCall` no-ext no-catch**; **`emitUserBash` null
   invalid + rethrow**; **P2 null-replaces** — all above.
3. **registerShortcut (L256) / registerFlag (L260)**:
   `...options` spread into the stored object (name/validation
   as registerCommand L242 — non-empty string name, error
   includes extension.path).
4. **`events.on` (L422)**: `const unsubscribe =
   runtime.trackEventBusSubscription(eventBus.on(channel,
   handler)); if (loading) loadingUnsubscribers.push(unsub);
   return unsubscribe` (ts-harness L3624 wraps makeHandler).
5. **exec.ts**: `ExecResult {stdout, stderr, code, killed}`;
   ENOENT/spawn error → **resolve** `{code: 1, killed}`;
   **spawn-error stderr stays empty** (code 1, no throw);
   stdout/stderr UTF-8-**lossy**; killProcess = SIGTERM, 5 s →
   SIGKILL (first call sets killed = true); pre-aborted
   signal → kill immediately; `pi.exec` (L413) = `assertActive`
   → `execCommand(command, args, options?.cwd ?? cwd)` (cwd
   fallback = extension loading cwd = "/proj").
6. **Harness JSON = identity model**: op values pass through
   jstr (JSON.parse on the Rust side) — primitives = value-eq;
   objects/arrays = distinct parse nodes = reference-≠
   (emitInput `changed` semantics rely on this).
7. **V8 error text catalog** (Addendum A) = byte-arbiter; all
   error-path cases lock exact strings.
8. **emitToolResult JSON-null-replaces; 5-field merge;
   structuredContent delete** (Addendum C).

Addenda A–D supersede any conflicting earlier statements above;
where an earlier section says "post-op mcp drain" or "MQ =
ui_prompt only", Addendum B's engine (mcp + ui_prompt) wins.


## Addendum E — sp1–sp8 errata + item-12 completion notes (c0973a7)

**The Addendum A sections were corrected by 8 further probe rounds**
(sp1–sp8, run against the compiled dist live); where Addendum A/E-less
text conflicts with the following, THIS addendum wins:

### normalizeBuildSystemPromptOptions (sp1/sp2 — corrected)

- **`customPrompt` / `forceSystemPrompt` / `cwd` = RAW `input.x`
  reads**: present (ANY value incl. null) → kept raw; missing
  (undefined) → key ABSENT (jstr drops it). `n(5)` = 9 jstr-keys
  (Object.keys sees 12 — undefined-valued; unobservable = documented
  divergence). The Addendum A "cwd: 5 → String(5)" bullet was WRONG.
- `selectedTools`/`hiddenTools`/`promptGuidelines` = `[...(x ?? def)]`
  — non-iterable non-nullish throws the FULL compiled expression text
  (`(input.selectedTools ?? ["read", "bash", "edit", "write"]) is not
  iterable`, NO Symbol-suffix); `"ab"` → `["a","b"]`; elements raw;
  null → default/`[]`.
- `toolSnippets`/`sections` = `{...(x ?? {})}` — **never throws**;
  primitives → `{}`; **`"ab"` → `{"0":"a","1":"b"}` (strings spread to
  char-keys); `["a"]` → `{"0":"a"}` (arrays to index-keys)**.
- `toolGuidelines` = `fromEntries(entries(?? {}).map(([n, g]) => [n,
  ...g]))` — primitive → `{}`; per-VALUE spread: non-iterable g (5,
  null, `{a:1}`, undefined) → **`guidelines is not iterable`** (V8
  names the variable); `{x: "ab"}` → `{x: ["a","b"]}`.
- `appendSystemPrompt` = `x ?? ""` (0/false/5 kept; null → "").
- `contextFiles`/`skills` = `(x ?? []).map(e => ({...e}))` — non-array
  non-nullish → `(input.x ?? []).map is not a function`; elements
  spread (primitives → `{}`, strings → char-keys).
- **Rust pitfall confirmed**: `Value::Null.get()` ≠ throw (matched
  explicitly); serde has no undefined (missing key ≠ present null).

### buildSystemPromptSections (sp4/sp5/sp7 — corrected)

- **cp-gate = the default cluster ONLY**
  (preamble+tools+rules+docs); **addendum / project_context / skills /
  cwd / custom sections all still run** (B4/B5/B6/B7/B8 verbatim).
- `customPrompt` **truthy** → `promptSections.preamble = customPrompt`
  **RAW** (non-strings survive into the map — the gsmt len>0 filter
  drops them); falsy (incl. null/0/"") → defaults.
- `toolSnippets[String(name)]` truthy w/ **raw templates**:
  `{read: 5}` → `- read: 5`; **`{read: []}` → `- read: ` (empty-array
  TRUTHY!)**; 0/""/null → `(none)`; non-declared not listed.
- `appendSystemPrompt` truthy-raw (5 → `<addendum>
5
</addendum>`).
- `renderProjectContext` = **raw `${path}`/`${content}`**: missing →
  **"undefined"**, null → **"null"**, 5 → "5", 0 → "0" (sp8 A–F).
- skills: `escapeXml` throws — non-string name/desc/filePath →
  `str.replace is not a function`; **missing filePath → `'replace'`
  of undefined**; null → of-null. dmi:true → no section; no reader
  (`selectedTools: ["x"]`) → no section; **`skills: []` → no
  section**.
- `cwd.replace` throws: 5 → `cwd.replace is not a function`;
  present-null → of-null; **MISSING → `'replace'` of undefined**
  (`bp(n({}))` / `st(msgs)`).
- custom sections **last**, `if (content)` truthy-kept **raw**
  (5 → `<a>
5
</a>`; null/0/"" skipped), **overwrite** built.
- `sections = {preamble: raw}` (the ONLY non-templated map entry) +
  others `<name>
${content}
</name>` (template-coerced).
- **rules throw**: `rule.trim` (non-string; null → of-'trim');
  guidelines looked up by `String(name)`; pg/tg rules dedup (trim +
  seen); **hidden bash → no bash rule** (declared = selected − hidden).

### buildSystemPromptState / buildSystemPrompt / gsmt (sp3/sp4)

- **`buildSystemPromptState(input)` = 1-arg (the OPTIONS!)** — the
  Addendum-A "st(msgs)" reading was wrong: `input` null →
  `null.forceSystemPrompt` throw; **`forceSystemPrompt !== undefined`
  = KEY-PRESENT** (null/0/false/"" all force) → `{content: fsp}`
  (sections ABSENT); else `{content: "", sections:
  buildSections(input)}`.
- `buildSystemPrompt(input)` = `getSystemMessageText({role: "system",
  ...state, timestamp: 0})` — `getSystemMessageText`/`contentText`
  live in **pi-ai utils/text.js** (NOT system-prompt.ts): gsmt =
  `[ct(content)] + Object.values(sections ?? {})` (null values
  filtered BEFORE push; **raw Values pushed**) → `filter(p => p.length
  > 0)` (**non-strings DROPPED**; empty strings dropped) → `"

"`.
  join. ct = string | array `.filter(b => b.type === "text").map(b =>
  b.text).join(sep)` (**`b.type` on null throws of-'type'**;
  non-objects drop; b.text missing = undefined → join-coerced "") |
  null → `null.filter` throw | other → `content.filter is not a
  function`.
- **forced fsp matrix**: `"F"` → "F"; **null → `'filter'` of null**;
  **0 → `content.filter`**; **`["x"]` → ""** (ct drops non-text
  blocks!). Forced-falsy ≠ unforced (contains_key!).

### emitBeforeAgentStart (runner.js L1119–1161 — verbatim-pinned)

```
async emitBeforeAgentStart(trigger, systemPromptOptions) {
    const currentOptions = normalizeBuildSystemPromptOptions(systemPromptOptions);  // 1st normalize — V8 throws ESCAPE
    const renderCurrentSystemPrompt = () => buildSystemPrompt(currentOptions);
    const ctx = Object.defineProperties({}, Object.getOwnPropertyDescriptors(this.createContext()));
    ctx.getSystemPrompt = () => { this.assertActive(); return renderCurrentSystemPrompt(); };
    const messages = [];
    for (const { ext, handlers } of snapshotEventHandlers(this.extensions, "before_agent_start")) {
        for (const handler of handlers) {
            try {
                const event = { ...trigger, type: "before_agent_start",
                    get systemPrompt() { return renderCurrentSystemPrompt(); },
                    systemPromptOptions: currentOptions };   // LIVE-REF (in-place mutations hit currentOptions)
                const handlerResult = await handler(event, ctx);
                if (handlerResult) {
                    const result = handlerResult;
                    if (result.message) messages.push(result.message);
                    if (result.systemPrompt !== undefined)   // NULL REASSIGNS
                        currentOptions.forceSystemPrompt = result.systemPrompt;
                }
            } catch (err) { this.emitError({extensionPath: ext.path, event: "before_agent_start", error, stack}); }
        }
    }
    return { messages, systemPromptOptions: currentOptions };
}
```

- ctx = **defineProperties-over-fresh-createContext** +
  `getSystemPrompt` = assertActive + **lazy** render (V8 throws from
  the normalize-throw-options + fsp-null/0 render paths surface at the
  event/ctx READS).
- `event.systemPrompt` = **lazy getter** (renders at read).
- `event.systemPromptOptions` = **currentOptions live-ref** —
  in-place event mutations mutate currentOptions (emitBoundary-style
  mutate steps must see it).
- `handlerResult` **truthy** → `result.message` **truthy**-push +
  `result.systemPrompt !== undefined` (**null REASSIGNS**).
- jstr(return) drops the `content`/`sections` distinctions: the op
  log = `{messages, systemPromptOptions: currentOptions}` (jstr =
  the 12/11-key normalize output).
- **Rust model**: `emit_before_agent_start(trigger, options: &Value)`
  → `normalize_options_raw(options)` (Err → the emitter THROWS OUT —
  op log `!...`); `build_system_prompt(&current, &doc_paths)` at the
  reads (ctx.getSystemPrompt + event.systemPrompt — LAZY: the event
  object carries an Arc<Mutex<Option<Result>>>-style render cache?? —
  NO: jstr drops functions; the harness reads systemPrompt → logs
  jstr(render) or !err-message → the event = a proxy w/ a lazy render
  fn; reads throw → caught by the read-try); **`doc_paths` must live
  on the runner** (host paths; the plan's DocPaths plumbing).

### exec.ts (item 1 — DONE 396a92a; probe-pinned)

`execCommand(command, args, cwd, options)`:
- `{stdout, stderr, code, killed}` literal order (both branches);
  stdout/stderr UTF-8-**lossy**; `code = exitCode ?? 0` (**signal-kill
  (ours OR the child's own) → 0**); killed = killProcess ran.
- **spawn errors (missing binary AND missing cwd) → the promise
  RESOLVES** `{stdout: "", stderr: "", code: 1, killed}` (empty
  stderr!; killed = pre-aborted signal) — **NO rejection**.
- **spawn-ARG validation → REJECTION** (exact node v24 messages):
  non-string command → `The "file" argument must be of type string.
  Received type number (5)`; non-array args → `The "args" argument
  must be of type object. Received type number (5)`; **array ELEMENTS
  are string-coerced, not validated** ([5] → "5", [null] → "null",
  [{}] → "[object Object]", [[1],[2,3]] → "1,2,3").
- No shell (`"/bin/sh -c 'echo hi'"` → code 1).
- `options.timeout > 0` (JS `>`; null/""/false → no; "120"/true →
  yes) → setTimeout(killProcess).
- abort: pre-aborted → killProcess immediately (ENOENT +
  pre-aborted → `{code: 1, killed: true}`); mid-run abort →
  `{code: 0, killed: true}`; partial stdout kept.
- `killProcess` = idempotent guard (killed = true + SIGTERM;
  detached 5 s SIGKILL).
- **Divergence**: the 100 ms post-exit pipe-idle grace (detached
  descendants) not ported (reads end at the pipes' EOF).
- Rust: `exec.rs` — `std::process` + reader threads (tokio has no
  "process" feature); `ExecSignal` seam (PreAborted/Live);
  `pi.exec` = assertActive → `execCommand(command, args,
  options?.cwd ?? loadingCwd, options)`.
