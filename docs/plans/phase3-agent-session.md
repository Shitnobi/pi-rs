# Phase 3 Port Plan — Agent Session / Loop Layer

Scope: the agent-loop + agent-session layer of the TS original, ported to Rust with green
parity at every tranche. Sources analyzed in full:

| Source (TS) | LOC | Port status |
|---|---|---|
| `packages/agent/src/agent-loop.ts` | 949 | **not ported** (the real main loop) |
| `packages/agent/src/agent.ts` | 613 | **not ported** (stateful `Agent` wrapper) |
| `packages/agent/src/types.ts` | 539 | **not ported** (AgentEvent/AgentContext/AgentTool/…) |
| `packages/agent/src/proxy.ts` | 393 | not ported (provider proxy; defer) |
| `packages/agent/src/stream-fn.ts` | 20 | not ported |
| `packages/coding-agent/src/core/agent-session.ts` | 4428 | **not ported** (biggest piece) |
| `packages/coding-agent/src/core/agent-session-runtime.ts` | 447 | not ported |
| `packages/coding-agent/src/core/agent-session-services.ts` | 233 | not ported |
| `packages/coding-agent/src/core/tools/file-mutation-queue.ts` | 63 | not ported |
| `packages/coding-agent/src/core/nested-tool-calls.ts` | 263 | half ported (recorder only) |
| `packages/coding-agent/src/core/event-bus.ts` | 33 | not ported |

Everything else `agent-session.ts` imports is a *dependency*, not part of this phase's
core, but the trait boundaries below must anticipate them (see §1.4).

---

## 1. Module inventory

### 1.1 `packages/agent/src` (pi-agent-core) — the actual loop

**`agent-loop.ts` (949 LOC).** Exports: `agentLoop`, `agentLoopContinue` (EventStream-returning
wrappers), `runAgentLoop`, `runAgentLoopContinue`, `runToolCall`, `type AgentEventSink`,
`type ToolCallHooks`. The heart is `runLoop()` (≈ line 160):

```
while (true) {                                  // outer: follow-up messages
    while (hasMoreToolCalls || pendingMessages.length > 0) {   // inner: tool calls + steering
        if (lastCompletedTurn) {
            prepareNextTurn(lastCompletedTurn)  // → context/messages/model/thinkingLevel swap
            re-poll steering if earlier poll was empty
            emit turn_start
        }
        declareToolChanges(context, [...prepared, ...pending]) → message_start/message_end per message
        prepareRequest({context, model, thinkingLevel})       // may swap model/context
        message = streamAssistantResponse(...)                // LLM call, streaming events
        if stopReason error|aborted → finishTurn, turn_end, agent_end, return
        toolCalls = content.filter(toolCall)
        if stopReason === "length" → failToolCallsFromTruncatedMessage (all calls error results)
        else executeToolCalls (sequential | parallel)
        hasMoreToolCalls = !executedToolBatch.terminate
        finishTurn(turn, signal) → decision {action: "end"|"continue"|undefined}
        emit turn_end; if "end" → agent_end, return
        pendingMessages = getSteeringMessages()
    }
    followUps = getFollowUpMessages(); if any → continue outer loop
    if (explicitContinuation) continue         // context-only turn from finishTurn decision
    break
}
emit agent_end
```

Other key functions, with signatures (line numbers from source):

- `declareToolChanges(context, pendingMessages): AgentMessage[]` (≈ 285) — computes
  `toolsAdded`/`toolsRemoved` system-message patch so replay always yields `context.tools`.
- `streamAssistantResponse(context, config, signal, emit, streamFunction)` (≈ 296) —
  `transformContext` → `convertToLlm` (AgentMessage[]→Message[]) → `getApiKey` →
  `streamFunction(model, llmContext, {...config, apiKey, signal})`; iterates stream events,
  pushes/replaces `partialMessage` in `context.messages` at the **last index**, emits
  `message_start`/`message_update`/`message_end`; `result()` attaches
  `thinkingLevel: config.reasoning ?? "off"`.
- `executeToolCalls` (≈ 522) → `executeToolCallsSequential` / `executeToolCallsParallel`.
  Parallel mode: preparation loop first (validation, beforeToolCall, immediate results emitted),
  then thunks pushed and started by `Promise.all` in array order; tool-result messages are
  emitted **after all calls finish**, in original call order.
- `prepareToolCall` (≈ 704): tool lookup (`Tool <name> not found` immediate error),
  `prepareArguments`, `validateToolArguments`, `beforeToolCall` hook (block → immediate
  error result, optional `terminate: true`), `signal?.aborted` checks at three points.
- `runToolCall(toolCall, options)` (≈ 776, exported) — the same pipeline for one call, no
  events, never rejects; used by nested tool calls.
- `executePreparedToolCall` (≈ 806): `performance.now()` duration, `onUpdate` partial
  results collected into a `Promise[]` sink, flushed with `Promise.all` after execute
  settles (`acceptingUpdates` flag), thrown errors → error result.
- `finalizeExecutedToolCall` (≈ 839): `afterToolCall` hook result merge rules (content
  fallback, structuredContent deletion rule, terminate/isError override), hook throw →
  error result.
- `shouldTerminateToolBatch`: every result `terminate === true`.
- `failToolCallsFromTruncatedMessage` (≈ 470): length-stop calls all become error results.

**`agent.ts` (613 LOC).** `class Agent` (≈ line 249):

- `MutableAgentState` with getters/setters that **copy arrays** (`set messages` does
  `nextMessages.slice()`), computed `systemPrompt`/`tools` getters.
- `PendingMessageQueue` — `enqueue/peek/drain/clear` with `mode: "all" | "one-at-a-time"`
  (peek returns one message in one-at-a-time mode).
- Public mutable hook **fields** (not methods): `convertToLlm`, `transformContext`,
  `streamFunction`, `getApiKey`, `beforeToolCall`, `afterToolCall`, `finishTurn`,
  `prepareRequest`, `prepareNextTurn`, `prepareNextTurnWithContext`. AgentSession
  monkey-patches these in `_installAgent*` methods, chaining onto `previous` — a chain
  built at construction time.
- `prompt()` (≈ 371) / `continue()` (≈ 386): reject if `activeRun` exists. `continue()`
  from an assistant tail drains steering first, then follow-ups, else throws
  `Cannot continue from message role: assistant`.
- `runWithLifecycle` (≈ 470): AbortController + deferred promise; sets
  `isStreaming/streamingMessage/errorMessage`; on throw → `handleRunFailure` which
  **synthesizes an assistant error message** and replays `message_start → message_end →
  turn_end → agent_end` through `processEvents`; `finishRun` resolves the idle promise.
- `processEvents` (≈ 537): state reduction (`message_end` pushes into
  `_state.messages`; `pendingToolCalls` set add/remove) then **awaits every listener in
  subscription order**; throws `Agent listener invoked outside active run` if no signal.
- `createLoopConfig` closes over mutable `skipInitialSteeringPoll`.

**`types.ts` (539 LOC):** `AgentEvent` union (agent_start/turn_start/message_start/
message_update/message_end/tool_execution_start/update/end/turn_end/agent_end),
`AgentContext {messages, tools}`, `AgentTool` (`name, description, parameters,
execute(toolCallId, args, signal, onUpdate), executionMode?, prepareArguments?`),
`AgentLoopConfig` (model, reasoning, toolExecution, hooks…), `AgentState`, `StreamFn`.

### 1.2 `agent-session.ts` (4428 LOC) — `class AgentSession` (line 379)

Responsibility groups (methods with line numbers):

- **Construction & hook installation** (486–528, 653–661, 788–846, 887–938, 1772–1813):
  `_installAgentToolHooks` (653), `_installAgentRequestProjection` (788), 
  `_installAgentBoundaryHooks` (887, wraps `agent.finishTurn`), 
  `_installAgentNextTurnRefresh` (899, wraps `prepareNextTurnWithContext`),
  `_installHiddenDeclarationsProjection` (1772) and
  `_installAgentForcedPromptProjection` (1792) — both chain `agent.transformContext`.
  Each captures the `previous` hook in a closure at construction.
- **Run orchestration**: `_startRun` (1837), `_prepareRun` (1890), `_handlePostAgentRun`
  (1943), `_runBeforeSettleBoundary` (1983). `_startRun`'s post-run `while` loop:
  ```
  await this.agent.prompt(messages);
  while (!this._agentRunAbortRequested) {
      if (await this._handlePostAgentRun()) {          // retry / compaction / queued msgs
          if (abortRequested) break;
          await this.agent.continue(); continue;
      }
      if (abortRequested || !(await this._runBeforeSettleBoundary())) break;
      if (abortRequested) break;
      await this.agent.continue();
  }
  // finally: _finishCancelledRetry, clear _failedResponse/_runSystemPromptOptions,
  // flush pending bash/custom messages, await _emitAgentSettled()
  ```
- **Prompting / queues**: `prompt` (2058), `steer` (2241), `followUp` (2257),
  `sendCustomMessage` (≈ 2318), `sendUserMessage` (2395), `clearQueue` (2432),
  `_tryExecuteExtensionCommand` (2146), `_expandSkillCommand` (2177),
  `_normalizePromptImages` (2027). Prompt order: extension-command dispatch →
  compaction-in-progress guard → input handlers → skill/template expansion → streaming
  queue vs. model/auth validation → `_startRun`.
- **Event handling / persistence**: `_handleAgentEvent` (1104, bound arrow field):
  abort-at-agent_start, nested-call record attachment on `message_start(toolResult)`,
  steering/follow-up bookkeeping on `message_start(user)`, extension dispatch, public
  emit, `message_end` → SessionManager append (`custom` vs. standard roles), retry
  counter reset, `turn_end` → `_flushPendingCustomMessages`.
- **Idle / settled**: `_getIdleWaitPromise` (1060), `_resolveIdleWaitIfIdle` (1069),
  `_emitAgentSettled` (1079) with `_deferredSettledActions` re-entrancy deferral
  (`prompt` while settling is deferred, line 2059).
- **Abort**: `abort()` (2464) sets `_agentRunAbortRequested`, aborts retry/compaction/
  branch-summary/bash, `agent.abort()`, `await waitForIdle()`.
- **Compaction**: `compact` (2794, manual), `_checkCompaction` (2977 — the decision
  tree), `_runAutoCompaction` (3127), `_compactBeforeNextAssistantResponse` (777,
  called from the `prepareNextTurnWithContext` wrapper), `_runDefaultCompaction` (2749),
  `_omitRecoveryAttempt` (1241), triggers: overflow-with/without-retry, threshold,
  pre-prompt check in `_prepareRun` (`_checkCompaction(lastAssistant, false)`), and
  virtual-model threshold re-check inside `prepareRequest` (line 826).
- **Retry**: `_prepareRetry` (3792) — `retryDelayMs` backoff, abortable `sleep`,
  `_omitRecoveryAttempt` to hide the failed response from projection while keeping it in
  raw history, `_failedResponse` replayed as `failed` in the next request.
- **Tool registry / loadout**: `_refreshToolRegistry` (3525), `_applyToolLoadout` (1580)
  with `prepareLoadout` hooks and hidden declarations, `_setActiveTools` (1532),
  allow/deny matching (`_isAllowedTool` 1542, `_isActivatable` 1554),
  `_getCallableTools` (1567), `_restoreToolsFromTranscript` (1815),
  `_rebuildSystemPrompt` (1705), `_preparePromptAndToolLoadout` (1743 — system-section
  diff message).
- **Model / thinking**: `setModel` (2507), `cycleModel` (2549), `setThinkingLevel`
  (2642), `cycleThinkingLevel` (2671), `_recordSelection` (629), virtual-model routing
  inside `prepareRequest` (788–846) incl. `VIRTUAL_MODEL_STATE_ENTRY` persistence.
- **Bash**: `executeBash` (3871) with `_bashAbortControllers` set and
  `_pendingBashMessages` deferral while streaming; `recordBashResult` (3909).
- **Tree navigation**: `navigateTree` (3994) — branch summary via
  `generateBranchSummary`, `session_before_tree`/`session_tree` extension events,
  leaf moves (`branchWithSummary`/`branch`/`resetLeaf`).
- **Stats/export/misc**: `getSessionStats` (4213), `getContextUsage` (4267),
  `exportToHtml` (4318), `exportToJsonl` (4344), `summarizeForBugReport` (4352),
  `dispose` (1397), `reload` (3690), `bindExtensions` (3288), `_bindExtensionCore`
  (3390 — fire-and-forget `sendMessage` with `.catch`), `createReplacedSessionContext`
  (4405).

### 1.3 Supporting modules

- **`agent-session-runtime.ts` (447)**: `class AgentSessionRuntime` — owns current
  session + cwd-bound services; `switchSession`/`newSession`/`fork`/`importFromJsonl`;
  each does `emitBeforeSwitch` (cancellable) → `teardownCurrent` (abort →
  `session_shutdown` → sync `beforeSessionInvalidate` → `dispose`) → `createRuntime`
  factory → `apply` → `finishSessionReplacement` (rebind + `withSession` context).
- **`agent-session-services.ts` (233)**: `createAgentSessionServices` (ModelRuntime +
  SettingsManager + ResourceLoader creation, pending provider registrations,
  `modelRuntime.refresh({allowNetwork:false})`, extension flag validation →
  diagnostics) and `createAgentSessionFromServices` (delegates to `sdk.ts`
  `createAgentSession`).
- **`file-mutation-queue.ts` (63)**: module-global `Map<realpath, Promise<void>>` +
  `registrationQueue` promise chain; `withFileMutationQueue(filePath, fn)` serializes
  same-file mutations, parallel across files; queue entry deleted only if still the
  tail (`fileMutationQueues.get(key) === chainedQueue` identity check).
- **`nested-tool-calls.ts` (263)**: `NestedCallRecorder` (ported) +
  `NestedToolCallRunner` (**not ported**): `execute(callerId, name, args, options)`
  allocates id `<callerId>/<n>`, per-caller `CallScope` sharing the recorder,
  `queueTail` promise chain for exclusive (sequential) calls, `holdsQueue` re-entrancy,
  `takeRecord(toolCallId)` consumed by `_handleAgentEvent`.
- **`event-bus.ts` (33)**: `createEventBus()` over `node:events` EventEmitter; handlers
  wrapped in async safe-handler that swallows errors to `console.error`; used by
  extensions.

### 1.4 Directly-consumed core/ modules (dependency status)

Already ported: `session-manager` (pi-session), `compaction/index` pure logic,
`messages.ts` (pi-core/messages.rs), `usage-totals` (pi-core), `bash-executor` +
`output-accumulator` (pi-tools), all 7 built-in tools (pi-tools), `NestedCallRecorder`
(pi-session/nested_tool_calls.rs).

Not yet ported, needed as **traits/stubs** for this phase, full ports later:
`extensions/*` (ExtensionRunner — biggest), `settings-manager`, `model-runtime`,
`model-registry`, `resource-loader`, `system-prompt`, `prompt-templates`,
`mcp-servers`, `virtual-models`, `sdk.ts`, `cache-warmer`, `export-html`,
`bug-report`, `session-export`, `skills`, `source-info`, `auth-guidance`.

### 1.5 Control-flow summary (abort/interrupt, compaction triggers)

- **Abort** is cooperative and point-checked: `signal.aborted` at prepareToolCall (3
  points), tool `execute` receives signal, loop breaks after each sequential call;
  AgentSession adds `_agentRunAbortRequested` (checked in `_startRun`'s while-loop,
  `_handlePostAgentRun`, `_willRetryAfterAgentEnd`), `_abortDuringBeforeSettle`, and
  abort-during-before_agent_start handled at the `agent_start` event (line 1108).
- **Steering** is delivered between assistant turns (inner loop polls
  `getSteeringMessages` after each turn and after long preparations);
  **follow-ups** only when the agent would stop (outer loop).
- **Compaction triggers**: (a) pre-prompt in `_prepareRun`; (b) post-run in
  `_handlePostAgentRun` → `_checkCompaction` (overflow-retry once via
  `_overflowRecoveryAttempted`, overflow-no-retry, threshold); (c) pre-request via the
  `prepareNextTurnWithContext` wrapper `_compactBeforeNextAssistantResponse` and the
  virtual-model re-check inside `prepareRequest`.

---

## 2. JS-semantics traps for the Rust port

1. **Object identity as map key.** `_entryIdsByMessage: WeakMap<object, string>` and
   `_boundaryDispatchedMessages: WeakSet<object>` key on message *object identity*.
   Rust: introduce a message arena (`Vec<AgentMessage>` + `MsgId`) or an explicit
   `msg_id: u64` carried in messages, with `HashMap<MsgId, EntryId>`. The TS harness
   compares persisted output, so ids must be internal only.
2. **In-place message mutation.** `_replaceMessageInPlace` (1271) deletes all keys and
   `Object.assign`s the replacement *into the same object*, deliberately keeping
   agent state, session persistence (which appends `event.message` later), and
   listeners in sync. Rust: messages must be behind shared mutable slots
   (`Rc<RefCell<>>` in single-threaded runtime, or arena indices) — a value-copy port
   breaks the `message_end` replacement path (`_emitExtensionEvent` →
   `emitMessageEnd` replacement).
3. **Partial message replacement during streaming.** `streamAssistantResponse` pushes
   the partial then *replaces the last element* on every delta — index-based, not
   id-based; on `error`/`done` replaces with the final message. The Rust loop must do
   `*messages.last_mut() = final` at exactly the same points.
4. **Promise-chain serialization.** `file-mutation-queue` and
   `NestedToolCallRunner.queueTail` serialize via chained promises, and the mutation
   queue only deletes its entry if it is still the tail (identity comparison
   `fileMutationQueues.get(key) === chainedQueue`). Rust: `Mutex<HashMap<PathBuf,
   VecDeque<Waiter>>>` or an async mutex per key; the tail-identity check becomes a
   generation counter.
5. **Await-everything event ordering.** `emit` awaits each listener in subscription
   order; `processEvents` reduces state *before* awaiting listeners; extension events
   fire *before* public listeners (`_emitExtensionEvent` then `_emit`). Any Rust
   channel/spawn design that reorders these breaks parity. The event log
   (extension-side effects + public event sequence + persisted entries) is the parity
   oracle.
6. **try/finally ordering.** `_startRun`'s `finally` runs `_emitAgentSettled()` *before*
   the rejection reaches the caller of `prompt()`. `_runAutoCompaction`'s `finally`
   clears the abort controller and resolves idle waiters. Rust: must model with
   explicit drop-guard/scope functions, not by relying on `?` propagation.
7. **Single-slot deferred promise + re-entrancy deferral.** `_idleWaitPromise` is
   created lazily and resolved once; `_deferredSettledActions` re-queues
   `prompt`/`sendCustomMessage` calls made while `agent_settled` is being emitted.
   Rust: `Option<oneshot::Sender>` + `Vec<DeferredAction>` replayed in
   `_emitAgentSettled` — order of deferral vs. resolution (`_resolveIdleWaitIfIdle`
   in the `finally` of the deferred loop) must match.
8. **Fire-and-forget with swallowed errors.** `_bindExtensionCore`'s
   `sendMessage`/`sendUserMessage` call the async method and `.catch(err =>
   runner.emitError(...))` — errors never propagate to the extension. Rust: `tokio::spawn`
   (or queued task) with error routed to the error sink; must not surface as panics.
9. **Clock semantics.** `Date.now()` for message timestamps, system-patch timestamps,
   and the compaction-boundary comparison
   `assistantMessage.timestamp <= new Date(compactionEntry.timestamp).getTime()`
   (line ~3010); `performance.now()` for tool/nested-call durations
   (`Math.round`). Parity needs an injectable `Clock` trait; harness uses a
   scripted/fake clock (also fixes `durationMs` determinism).
10. **UTF-16 vs UTF-8.** `NestedCallRecorder.start` counts *UTF-8 bytes* via
    `TextEncoder` of `JSON.stringify(arguments)`; JS string ops elsewhere
    (indexOf/splice on steering text, `contentText`) are UTF-16. Use the established
    `js_string` helpers; byte counting must be UTF-8 on the JSON re-serialization.
11. **JSON quirks.** `JSON.stringify`/`parse` round-trip is load-bearing in
    NestedCallRecorder (argument cloning + byte size); `JSON.stringify(undefined)`
    → absent fields; `structuredContent` deletion rule in
    `finalizeExecutedToolCall`. Keep serde_json with `skip_serializing_if` matching
    the JS optional-field behavior (empty arrays omit fields, e.g.
    `withToolChanges`).
12. **Error shapes.** `error instanceof Error`, `error.cause` (auth header check at
    line 548), `String(err)`, `err.stack` passed to `emitError`. Define a Rust
    `PiError` with `message`, optional `cause: Option<String>`, and a JS-style
    display (`String(err)` → "Error: msg" in V8 for `new Error`, plain text for
    strings — check what the TS harness actually prints and match that).
13. **Closure-captured mutable state.** `createLoopConfig`'s
    `skipInitialSteeringPoll` flag; the `previous` hook chains captured at
    construction (reload does *not* reinstall them — the callbacks read
    `this._extensionRunner` at execution time, see comment at 653). Rust: builder
    that captures `Arc<Mutex<…>>`/`Weak<RefCell<AgentSession>>` hooks, with reload
    swapping the runner behind the same hook closure.
14. **AbortController/AbortSignal.** `signal.throwIfAborted()` (auto-compaction),
    abort mid-sleep in `_prepareRetry` (cancels retry with
    `_finishCancelledRetry`), `sleep(delayMs, signal)` rejection semantics. Model as
    `AbortHandle { flag: Arc<AtomicBool>, notify: Notify }`; sleep selects on the
    notify; `throwIfAborted` → `Err(Aborted)`.
15. **`Promise.all` ordering.** Parallel tool calls: all thunks started, results
    awaited as an array (order preserved); tool-result messages emitted in call
    order *after* all finish. `join_all` on already-polled futures reproduces this;
    do not use `FuturesUnordered`-style completion ordering.
16. **Array-copy semantics on state assignment.** `set messages/tools` copies the
    top-level array (shallow): later mutation of the original array must not leak
    into state and vice versa. In Rust this is mostly free (Vec clone), but the
    *element* sharing (same message objects in context snapshot and state) is not —
    see traps 1–2.

---

## 3. The async-runtime question

### 3.1 Await-point inventory

| # | Await point | Where | Blocking? |
|---|---|---|---|
| A1 | Provider streaming (`for await (event of response)` + `response.result()`) | `streamAssistantResponse` | long |
| A2 | Tool `execute(id, args, signal, onUpdate)` | `executePreparedToolCall`, `runToolCall` | long |
| A3 | Loop hooks: `prepareRequest`, `prepareNextTurnWithContext`, `finishTurn`, `transformContext`, `convertToLlm`, `getApiKey` | loop + AgentSession wrappers | medium (extensions, compaction, model routing) |
| A4 | `emit` → listeners awaited in order | everywhere | short-medium (session persistence, extension handlers) |
| A5 | `getSteeringMessages`/`getFollowUpMessages` queue drains | loop | instant (but async seam) |
| A6 | Compaction summarization (`compact()`, `generateBranchSummary`) → streamFn | AgentSession | long |
| A7 | Retry backoff `sleep(delayMs, signal)` | `_prepareRetry` | seconds, abortable |
| A8 | Auth resolution `modelRuntime.getAuth/checkAuth` (network/OAuth) | prompt validation, `_getRequiredRequestAuth` | network |
| A9 | Standalone bash `executeBash` + `onChunk` streaming | AgentSession | long, abortable |
| A10 | File mutation queue chain | tools (write/edit) | short |
| A11 | Nested tool call queue (`queueTail`) | NestedToolCallRunner | medium |
| A12 | `waitForIdle` deferred promise | AgentSession | long |
| A13 | Extension emit / boundary dispatch (JS extension code) | everywhere | arbitrary |
| A14 | Session replacement (`teardownCurrent` → `createRuntime`) | AgentSessionRuntime | medium |

### 3.2 Concurrency sources

1. Parallel tool calls within a turn (A2 × N via Promise.all).
2. Same-file mutations serialized by the mutation queue across parallel tools (A10).
3. Nested tool calls serialized per exclusive mode (A11).
4. User input (steer/followUp/abort) arriving *while* the loop is between awaits — in
   JS this is just microtask interleaving on a single thread; queues are plain arrays.
5. Deferred settled actions replayed while `agent_settled` handlers run.
6. Cache warmer (out of scope; trait stub).

### 3.3 Options

**(a) threads + channels.** Every await point becomes a channel receive; the loop
becomes a state machine driven by an enum of events. Rejected: tool parallelism and
abort timing then depend on OS scheduling → parity replays become nondeterministic;
also the JS code interleaves only at awaits (single-threaded cooperative), which
threads do not model. Massive rewrite of ~6k LOC of straight-line async code into
explicit state machines, hurting 1:1 faithfulness.

**(b) trait-abstracted poll loop (no runtime).** Define `trait Executor { fn
block_on; fn sleep; }` and hand-roll futures. Rejected: re-implements a bad tokio;
abortable sleep (A7, A9), streaming (A1), and join-order semantics (trap 15) all
need a real executor; the fake-provider determinism argument doesn't require
avoiding tokio.

**(c) tokio (recommended).** Write the loop as plain `async fn`s mirroring the TS
structure 1:1, run on a **`new_current_thread()` runtime** everywhere in pi-rs
(harness, CLI, later TUI on the same thread). Rationale:

- **Parity-testability.** A single-threaded cooperative runtime reproduces the Node
  event loop's only interleaving points (awaits). A fake `StreamFn` that yields a
  *scripted, finite* list of provider events plus `tokio::time::pause()` +
  `advance()` for sleeps makes every run bit-deterministic: no timers, no I/O, no
  spawn races. Parallel tool calls become `join_all` over fake tools that append to
  a shared log in start-order — identical to `Promise.all` over already-started
  thunks.
- **Faithfulness.** `async/await` maps line-for-line onto the TS control flow,
  including try/finally drop guards and sequential listener awaits. Threads would
  force semantic redesign precisely where parity is hardest.
- **Future needs.** MCP (A13, real network), HTTP providers (A1), bash streaming
  (A9) all need real async I/O eventually; tokio is the ecosystem default.

**Decision: tokio, current-thread runtime, no `tokio::spawn` inside the loop**
(exceptions: fire-and-forget extension `sendMessage` — trap 8 — implemented as a
deferred-action queue replayed deterministically in tests, spawned only in prod).
Sleeps go through a `Sleeper` trait (`RealSleeper` = tokio::time with
`time::pause()` support; tests use paused clock and `advance()`), so retry backoff
parity is exact. Abort = `AbortHandle` (AtomicBool + Notify) checked at the exact TS
check points, plus `select!` only inside `sleep` and provider-stream iteration where
JS aborts manifest as stream termination. This satisfies the constraint that parity
tests drive a fake provider/transport deterministically.

---

## 4. Pure-logic vs I/O split and trait boundaries

### Pure logic (no traits, directly parity-testable)

- `declareToolChanges` / `withToolChanges` / tool-state diffing.
- `PendingMessageQueue` (all / one-at-a-time peek/drain).
- `shouldTerminateToolBatch`, `createToolResultMessage` normalization,
  `createErrorToolResult`, truncated-message failure results.
- Steering/follow-up bookkeeping (indexOf/splice on queue texts).
- `_checkCompaction` decision tree (given messages/entries/settings/model →
  decision enum), `shouldCompact` (already ported), `_willRetryAfterAgentEnd`.
- Retry policy arithmetic (`retryDelayMs` backoff, attempt counting).
- `_buildBoundaryContext` / `_applyBoundaryDrafts` / preview-manager construction
  (pure over a SessionManager).
- Tool registry/loadout algebra: `_isAllowedTool`, `_isActivatable`,
  `_getToolExposure`, `_applyToolLoadout` (minus hooks), `_restoreToolsFromTranscript`.
- `estimateMessagesTokens`, `getSessionStats`, `getContextUsage`,
  `getUserMessagesForForking`, `getLastAssistantText`, `parseSkillBlock`,
  `_normalizePromptSnippet/Guidelines`, `extractUserMessageText`.
- Auth-guidance message formatting; `withoutDeletedHeaders`.

### Trait boundaries (I/O and host seams)

```rust
// crates/pi-agent/src/provider.rs
trait StreamFn: Send + Sync {
    fn stream(&self, model: &Model, ctx: LlmContext, opts: StreamOpts<'_>)
        -> PiEventStream; // struct: events: BoxStream<AssistantStreamEvent>, result: oneshot
}
// Fake: ScriptedProvider { turns: Vec<Vec<StreamEvent>> } — deterministic replay.

// crates/pi-agent/src/tool.rs (extends existing pi-tools AgentTool port)
trait AgentTool {
    fn name(&self) -> &str; fn description(&self) -> &str;
    fn parameters(&self) -> &JsonSchema;
    fn execution_mode(&self) -> ExecutionMode; // parallel | sequential
    fn prepare_arguments(&self, args: Json) -> Json;
    fn execute<'a>(&'a self, id: &str, args: Json, signal: &'a AbortHandle,
                   on_update: onUpdate<'a>) -> BoxFuture<'a, AgentToolResult>;
}
// Fake: ScriptedTool { name, mode, outcome script }.

// crates/pi-agent/src/host.rs — everything AgentSession needs from the host
trait ExtensionHost {          // stand-in for ExtensionRunner
    fn has_handlers(&self, event: &str) -> bool;
    fn emit<'a>(&'a self, ev: ExtensionEvent) -> BoxFuture<'a, Json>;
    fn emit_boundary<'a>(&'a self, ev: BoundaryEvent, build_ctx: BoundaryCtxFn<'a>)
        -> BoxFuture<'a, BoundaryOutcome>;
    fn emit_input<'a>(&'a self, text: &str, images: Option<&[ImageContent]>,
                      source: InputSource, behavior: Option<&str>) -> BoxFuture<'a, InputOutcome>;
    fn get_command(&self, name: &str) -> Option<CommandHandle>;
    fn emit_error(&self, err: ExtensionError);
    // … plus get_all_registered_tools, wrap_registered_tools for the registry tranche
}
// Fake: RecordingExtensions { handlers: BTreeMap<&str, ScriptedHandler>, log: Vec<Json> }.

trait ModelRuntimeHost {       // stand-in for ModelRuntime
    fn get_auth<'a>(&'a self, model: &Model, signal: &'a AbortHandle) -> BoxFuture<'a, Result<AuthResult>>;
    fn check_auth<'a>(&'a self, provider: &str) -> BoxFuture<'a, Option<AuthInfo>>;
    fn has_configured_auth(&self, provider: &str) -> bool;
    fn is_using_oauth(&self, provider: &str) -> bool;
    fn resolve_model<'a>(&'a self, sel: &Model, msgs: Vec<LlmMessage>, reason: ResolveReason<'a>)
        -> BoxFuture<'a, Result<ModelRoute>>;
    fn get_model(&self, provider: &str, id: &str) -> Option<Model>;
    fn get_physical_model(&self, provider: &str, id: &str) -> Option<Model>;
    fn get_available_snapshot(&self) -> Vec<Model>;
    // register/unregister provider/virtual-model for reload
}

trait SettingsStore {          // getters used by AgentSession
    fn compaction_settings(&self, model: Option<&Model>) -> CompactionSettings;
    fn retry_settings(&self) -> RetrySettings;
    fn steering_mode(&self) -> QueueMode; fn follow_up_mode(&self) -> QueueMode;
    fn image_auto_resize(&self) -> bool;
    fn shell_command_prefix(&self) -> Option<String>; fn shell_path(&self) -> Option<String>;
    fn default_tools(&self) -> Option<Vec<String>>;
    fn default_thinking_level(&self) -> Option<ThinkingLevel>;
    fn model_thinking_level(&self, provider: &str, id: &str) -> Option<ThinkingLevel>;
    // + setters used by setModel/persist paths (trait object with &mut or interior mutability)
}

trait Clock { fn date_now_ms(&self) -> f64; fn perf_now_ms(&self) -> f64; }
trait Sleeper { fn sleep<'a>(&'a self, ms: u64, signal: &'a AbortHandle) -> BoxFuture<'a, Result<()>>; }
trait SkillExpander { fn expand_skill(&self, text: &str) -> String; } // readFileSync seam
```

`SessionManager` (sync, ported) is used directly. `BashOperations` pattern (already
established) extends to bash streaming. The Agent-loop hooks
(`beforeToolCall/afterToolCall/finishTurn/prepareRequest/prepareNextTurnWithContext/
transformContext/convertToLlm/getApiKey`) become an `AgentHooks` struct of boxed
closures — mirroring the TS *mutable fields with chained previous* — with
`install_*` methods that chain, preserving the closure-chain semantics (trap 13).

---

## 5. Rust placement and incremental tranches

### Crate/module placement

- **New crate `crates/pi-agent`** — port of `packages/agent`:
  `types.rs` (AgentEvent/Context/State/Tool/LoopConfig), `queue.rs`
  (PendingMessageQueue), `loop.rs` (agent-loop.ts), `agent.rs` (Agent class),
  `hooks.rs` (AgentHooks chain), `provider.rs` (StreamFn trait + ScriptedProvider),
  `run_tool_call.rs`. Depends: pi-core. Adds tokio (per §3).
- **`crates/pi-session`** — gains `agent_session/` module tree (below) and keeps
  `nested_tool_calls.rs` (recorder); the runner moves to
  `pi-agent/src/nested_runner.rs` because it needs `runToolCall` (pi-session stays
  runtime-agnostic where possible; pi-agent → pi-session dependency is fine since
  session-manager is sync).
- **`crates/pi-cli`** — harness entries + production wiring; later
  `agent_session_runtime.rs` can live in pi-session too (depends only on traits +
  SessionManager).
- `pi-tools` write/edit gain the file mutation queue (`pi-tools/src/file_mutation_queue.rs`),
  wired through the existing operations traits.

`agent_session/` module tree in pi-session:

```
agent_session/mod.rs        — AgentSession facade struct + config
agent_session/events.rs     — AgentSessionEvent enum, emit, queue updates
agent_session/event_handler.rs — _handleAgentEvent port (persistence mapping)
agent_session/tools.rs      — registry, loadout, exposure, allowlists
agent_session/prompt.rs     — prompt/steer/followUp/sendCustom/sendUser, expansion
agent_session/run.rs        — _startRun/_prepareRun/_handlePostAgentRun/before-settle/idle
agent_session/compaction.rs — compact/_checkCompaction/_runAutoCompaction/omitRecovery
agent_session/retry.rs      — retry state machine + backoff
agent_session/model.rs      — setModel/cycle/thinking/virtual-model routing/recordSelection
agent_session/bash.rs       — executeBash/pending flush/abort
agent_session/tree.rs       — navigateTree/branch summary/fork listings
agent_session/stats.rs      — stats/context usage/exports (export-html deferred)
agent_session/runtime.rs    — AgentSessionRuntime (switch/new/fork/import)
```

### Tranches (each ends with green parity vs the TS dist, fake StreamFn on both sides)

The TS harness constructs the real `Agent` with an injected scripted `streamFn`
(Agent's constructor takes `streamFn` — the perfect seam, no network ever) and the
real `AgentSession` over it; the Rust harness does the same with `ScriptedProvider`.

| # | Tranche | Content (source lines) | Est. Rust LOC | Harness entry |
|---|---|---|---|---|
| 0 | pi-agent skeleton | types.rs, queue.rs, provider trait + ScriptedProvider, Clock/Sleeper/AbortHandle, tokio setup | 700 | `agent-queue` |
| 1 | Message conversion + request building | declareToolChanges (285–345), default convertToLlm, system-patch diff, `estimateMessagesTokens` (367) | 350 | `declare-tool-changes` |
| 2 | Single tool-call pipeline | prepareToolCall (704), executePreparedToolCall (806), finalizeExecutedToolCall (839), runToolCall (776), truncated-failure (470) | 450 | `run-tool-call` |
| 3 | Streaming assistant response | streamAssistantResponse (296–400) against ScriptedProvider; partial replace, abort/error/done paths, thinkingLevel attach | 300 | `stream-assistant` |
| 4 | Full agent loop | runLoop (160–283), executeToolCalls sequential/parallel (522–630), steering/follow-up polls, finishTurn decisions | 650 | `agent-loop` |
| 5 | Agent class | state reduction, runWithLifecycle, handleRunFailure (synthetic error message), prompt/continue/steer/followUp/clearQueues/reset, hooks fields + chaining | 550 | `agent` |
| 6 | File mutation queue | whole file (63 LOC) + wiring into write/edit tools | 150 | `file-mutation-queue` |
| 7 | Nested tool-call runner | NestedToolCallRunner (157–263): scopes, id alloc, queueTail, takeRecord/clear, host trait | 350 | `nested-tool-calls` (extends existing `nested-calls`) |
| 8 | Session event handling | `_handleAgentEvent` (1104–1201), persistence mapping, queue bookkeeping, `_emitAgentSettled` + deferred actions, idle wait, subscribe/dispose | 500 | `agent-session-events` |
| 9 | Tool registry & loadout | `_refreshToolRegistry` (3525), `_applyToolLoadout` (1580), exposure/allowlist (1542–1578), `_rebuildSystemPrompt` (1705) glue, `_restoreToolsFromTranscript` (1815), hidden-declarations + forced-prompt projections (1772–1813) | 650 | `agent-session-tools` |
| 10 | Prompt pipeline | `prompt` (2058), steer/followUp/queues (2241–2297), sendCustomMessage/sendUserMessage, extension-command dispatch, skill/template expansion, image normalization seam | 550 | `agent-session-prompt` |
| 11 | Run orchestration | `_startRun` (1837), `_prepareRun` (1890), `_handlePostAgentRun` (1943), `_runBeforeSettleBoundary` (1983), boundary drafts/context (947–1038), abort() | 600 | `agent-session-run` |
| 12 | Retry | `_prepareRetry` (3792), `_finishCancelledRetry`, `_omitRecoveryAttempt` (1241), `_failedResponse` routing in prepareRequest (790) | 300 | `agent-session-retry` |
| 13 | Compaction | `_checkCompaction` (2977), `_runAutoCompaction` (3127), `compact` (2794), `_runDefaultCompaction` (2749), summarization auth (573), `_exceedsCompactionThreshold` (768) | 750 | `agent-session-compaction` |
| 14 | Model & thinking | setModel (2507), cycleModel (2549), setThinkingLevel (2642), `_recordSelection` (629), virtual-model routing in prepareRequest (805–840), scoped models | 450 | `agent-session-model` |
| 15 | Bash | executeBash (3871), recordBashResult (3909), pending flush, abortBash | 200 | `agent-session-bash` |
| 16 | Tree navigation | navigateTree (3994), branch summary path, getUserMessagesForForking (4191) | 450 | `agent-session-tree` |
| 17 | Stats & exports | getSessionStats (4213), getContextUsage (4267), exportToJsonl, summarizeForBugReport seam | 250 | `agent-session-stats` |
| 18 | Session runtime | AgentSessionRuntime (447 LOC): switch/new/fork/import, teardown ordering, replaced-session contexts | 450 | `agent-session-runtime` |
| 19 | Services + event-bus | createAgentSessionServices (diagnostics, flag validation), createEventBus | 200 | `event-bus`, `agent-session-services` |

Total Rust estimate: ≈ 8,900 LOC across ~20 tranches (TS source ≈ 7,200 LOC in
scope; ×1.25 for trait/arena scaffolding).

**Ordering rationale:** 0–7 build the loop bottom-up with no AgentSession
dependencies (pi-agent crate is self-contained + fakes); 8–17 port AgentSession
slices in dependency order (events → tools → prompt → run → retry → compaction →
model → bash → tree), each only needing previously-landed tranches plus trait fakes;
18–19 are thin. Extension-runner *real* port is out of scope (Phase 4+); the
`ExtensionHost` fake keeps parity green meanwhile because the TS harness for these
tranches must run with extensions absent or with in-process handlers registered
through the public API — parity cases choose scenarios with zero or stub
extensions.

---

## 6. Parity harness entries (arg/result shapes)

Convention (existing): `pi-cli harness tool <name>` reads `{"tool": "...", "args":
{...}}` (or just args) on stdin, prints result JSON on stdout; TS side
`parity/ts-harness.js` runs the same case against the dist and prints the same
shape. Clock and provider are fakes on **both** sides (TS side injects a scripted
streamFn + monkey-patched Date/performance via case setup in `ts-harness.js`).

- **`agent-queue`** — args:
  `{ mode: "all"|"one-at-a-time", ops: [{op:"enqueue", message: AgentMessageJson} | {op:"peek"} | {op:"drain"} | {op:"clear"} | {op:"setMode", mode}] }`
  → result `{ steps: [AgentMessageJson[]] }`.
- **`declare-tool-changes`** — args:
  `{ contextMessages: AgentMessageJson[], contextTools: [ToolDeclJson], pendingMessages: AgentMessageJson[] }`
  → result `{ messages: AgentMessageJson[] }` (system patch with toolsAdded/Removed).
- **`run-tool-call`** — args:
  `{ toolCall: {id, name, arguments}, tools: [{name, parameters, mode: "ok"|"block"|"throw"|"update"|"slow", result?, errorText?, updates?, executionMode?}], beforeToolCall?: "pass"|"block"|"throw", afterToolCall?: {content?, isError?, structuredContent?}, abortAt?: "before"|"during" }`
  → result `{ outcome: {toolCall, result, isError, durationMs}, updateLog: [...] }`.
  durationMs is clock-driven (fake clock) so it compares exactly.
- **`stream-assistant`** — args:
  `{ script: [ {type:"start"|"text_delta"|"toolcall_end"|"error"|"done", partial?, final?} ], transformContext?: "identity"|"dropSystem", abortAfter?: n }`
  → result `{ events: [AgentEventJson], finalMessage: AssistantMessageJson }` —
  the event log is the oracle (message_start/update/end ordering, partial replaces).
- **`agent-loop`** — args:
  `{ prompts: [AgentMessageJson], providerTurns: [[StreamEventJson]], tools: [ScriptedToolJson], toolExecution: "parallel"|"sequential", steeringInject: {afterStep: n, messages: [...]}, followUpInject: {...}, finishTurnDecisions: ["end"|"continue"|null], prepareRequest?: "swap-model", expectError?: string }`
  → result `{ events: [AgentEventJson], newMessages: AgentMessageJson[] }`.
- **`agent`** — args:
  `{ initialState, script: [{op:"prompt", input} | {op:"steer", message} | {op:"followUp", message} | {op:"continue"} | {op:"abort"} | {op:"waitForIdle"}], providerTurns, tools }`
  → result `{ events, stateMessages, error?: string }`. Includes re-entrancy error
  cases ("Agent is already processing…") and `handleRunFailure` synthetic messages.
- **`file-mutation-queue`** — args:
  `{ pathsWithFs: {realPath: string}, ops: [{path, tag, failIfMissing?: bool}], }`
  with a fake realpath/fs → result `{ order: [tag] }` (registration order,
  serialization per key, tail-cleanup verified via a second phase).
- **`nested-tool-calls`** (extends existing entry) — args:
  `{ calls: [{callerId, name, args, holdQueue?: bool}], host: { sequential: bool, tools: [ScriptedToolJson] } }`
  → result `{ outcomes, events (with parentToolCallId), takeRecords: {toolCallId: NestedCallSummary} }`.
- **`agent-session-events`** — args:
  `{ config: AgentSessionHarnessConfig, agentEvents: [AgentEventJson] }` where
  AgentSessionHarnessConfig = `{ messages, entries, settings, tools }` + fake
  services → result `{ sessionEvents: [AgentSessionEventJson], entries:
  [SessionEntryJson], queueState }`.
- **`agent-session-tools`** — args:
  `{ config, customTools: [ToolDefinitionJson], extensionTools: [...], allowedToolNames?, excludedToolNames?, transcript?, ops: [{op:"refreshRegistry"} | {op:"setActiveTools", names} | {op:"restoreFromTranscript", modifiers}] }`
  → result `{ activeTools, declaredTools, hiddenDeclarations, systemPromptOptions }`.
- **`agent-session-prompt`** — args:
  `{ config, commands: {name: script}, templates, skills, script: [{op:"prompt", text, options} | {op:"steer"} | {op:"followUp"} | {op:"sendCustomMessage"} | {op:"sendUserMessage"}], providerTurns }`
  → result `{ dispositions, sessionEvents, queueState, promptsSentToAgent }`.
- **`agent-session-run`** — args:
  `{ config, script: [{op:"prompt", text} | {op:"abort"} | {op:"waitForIdle"}], providerTurns (multi-run), tools, extensionHandlers: {before_agent_start: script, turn_end: script, agent_before_settle: script} }`
  → result `{ sessionEvents, entries, idleTimeline }` — the critical
  ordering oracle (settled/deferred actions/flush ordering).
- **`agent-session-retry`** — args:
  `{ config: {retry: {enabled, maxRetries, ...}}, providerTurns: [errorTurn, okTurn], clock: {startMs}, script: [...] }`
  → result `{ sessionEvents (auto_retry_*), sleepLog: [{ms}], omittedEntries }`.
- **`agent-session-compaction`** — args:
  `{ config: {compaction settings, model}, entries, scenario: "overflow-retry"|"overflow-complete"|"threshold"|"manual"|"already-compacted"|"too-small", providerTurns (summarizer script), extensionCancel?, script: [...] }`
  → result `{ sessionEvents, entries, compactionResult?|error }`.
- **`agent-session-model`** — args:
  `{ config, models: [ModelJson], script: [{op:"setModel", model, persist?} | {op:"cycleModel", direction} | {op:"setThinkingLevel", level} | {op:"cycleThinkingLevel"}] }`
  → result `{ sessionEvents, modelChanges, thinkingLevelChanges }`.
- **`agent-session-bash`** — args:
  `{ config, bashScript: [{delay, output, exitCode}], script: [{op:"executeBash", command, id?} | {op:"abortBash"}] }`
  → result `{ bashEvents, entries, pendingFlushed }`.
- **`agent-session-tree`** — args:
  `{ config, entries (tree), script: [{op:"navigateTree", targetId, options}] , providerTurns (summarizer) }`
  → result `{ result, entries, editorText }`.
- **`agent-session-runtime`** — args:
  `{ config, fs: {paths}, script: [{op:"switchSession", path} | {op:"newSession"} | {op:"fork", entryId, position} | {op:"importFromJsonl", path}], providerTurns }`
  → result `{ cancelled, sessionIds, shutdownOrder }` — verifies teardown ordering.
- **`event-bus`** — args: `{ ops: [{op:"on", channel} | {op:"emit", channel, data, handlerThrows?} | {op:"clear"}] }`
  → result `{ received: [{channel, data}], errors: [string] }`.
- **`agent-session-services`** — args: `{ options, fs fixtures, extensionFlags }` →
  result `{ diagnostics }`.

Parity cases live in `parity/cases.mjs`; each tranche adds 3–10 cases covering happy
path, abort/interrupt points, error propagation, and ordering. All cases comparing
timestamps/durations use the fake clock (values scripted in args) — never wall time.

---

## 7. Dependencies and checklist

### New dependencies

- `tokio` (workspace dep, `features = ["rt", "macros", "sync", "time"]`) — used with
  `new_current_thread()` runtimes; `time` only via the `Sleeper` seam so tests can
  `pause()`. This **decides** the open runtime question: tokio, current-thread.
- `futures` (streams/join_all). No other new deps. serde_json/pi-core already present.

### Step-by-step checklist

1. [ ] Add `crates/pi-agent` to workspace; tokio + futures workspace deps; CI runs
   parity with current-thread runtime. (0.5 day)
2. [ ] **T0** `agent-queue` green: types.rs, queue.rs, AbortHandle, Clock/Sleeper
   traits, ScriptedProvider skeleton. (1 day)
3. [ ] **T1** `declare-tool-changes` green: message conversion + system patch +
   `agent-queue` regressions. (1 day)
4. [ ] **T2** `run-tool-call` green: full single-call pipeline incl. hooks,
   validation, update sink, durations. (2 days)
5. [ ] **T3** `stream-assistant` green: streaming + partial replacement + abort.
   (1.5 days)
6. [ ] **T4** `agent-loop` green: full runLoop, sequential/parallel batches,
   steering/follow-up, finishTurn. (3 days) ← highest-risk tranche
7. [ ] **T5** `agent` green: Agent wrapper, lifecycle, synthetic failure messages,
   hook chaining. (2 days)
8. [ ] **T6** `file-mutation-queue` green + wired into pi-tools write/edit; re-run
   existing tool parity. (0.5 day)
9. [ ] **T7** `nested-tool-calls` runner green (recorder already ported). (1.5 days)
10. [ ] **T8** `agent-session-events` green: event handler, persistence mapping,
    idle/settled/deferred actions. (2.5 days)
11. [ ] **T9** `agent-session-tools` green: registry, loadout, projections. (3 days)
12. [ ] **T10** `agent-session-prompt` green: prompt/queue pipeline. (2.5 days)
13. [ ] **T11** `agent-session-run` green: run orchestration + boundaries + abort.
    (3 days) ← second highest risk
14. [ ] **T12** `agent-session-retry` green. (1.5 days)
15. [ ] **T13** `agent-session-compaction` green (reuses ported compaction pure
    logic; add decision-tree cases). (3 days)
16. [ ] **T14** `agent-session-model` green. (2 days)
17. [ ] **T15** `agent-session-bash` green. (1 day)
18. [ ] **T16** `agent-session-tree` green. (2 days)
19. [ ] **T17** `agent-session-stats` green. (1 day)
20. [ ] **T18** `agent-session-runtime` green. (2 days)
21. [ ] **T19** `event-bus` + `agent-session-services` green. (1 day)
22. [ ] Update PARITY.md/TRACKING.md; document the tokio decision in docs (this file
    §3 is the record); audit all `tokio::spawn` usage (only fire-and-forget
    extension sends, behind a test-visible deferral).

### Known deferrals (explicitly out of this phase)

- Real extension runner (`extensions/*`, resource-loader) — `ExtensionHost` fake
  covers parity; real port is Phase 4.
- Real provider transports (HTTP/SSE), MCP, cache-warmer, auth storage, export-html,
  bug-report — behind the traits above.
- `proxy.ts` (provider proxying) and `sdk.ts` assembly — after T18.

### Top risks

1. **Event-ordering parity** (trap 5/6/15): mitigated by making the event log the
   oracle in every harness entry and by drop-guard discipline in `run.rs`.
2. **In-place mutation + identity maps** (traps 1–3): decide the arena/`MsgId`
   design in T0 before any AgentSession tranche starts.
3. **Deferred-action and idle-promise re-entrancy** (trap 7): dedicated cases in
   `agent-session-run` (prompt-during-settled).
4. **Clock determinism**: every timestamp/duration compared in parity must come from
   the injected Clock; audit each case at authoring time.
