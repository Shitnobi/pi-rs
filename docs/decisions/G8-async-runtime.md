# G8: Async runtime decision

**Date:** 2026-10-19
**Status:** Decided
**Decision:** tokio with a `new_current_thread()` runtime for the agent-session/loop layer (Phase 3, per `docs/plans/phase3-agent-session.md` §3.3c). Providers and MCP parity layers stay **synchronous**.

## Context

The TS original is single-threaded cooperative async (Node event loop). Three options were
evaluated in `docs/plans/phase3-agent-session.md` §3.3:

- **(a) threads + channels** — rejected: OS-scheduling-dependent interleaving breaks parity
  determinism; forces rewriting ~6k LOC of straight-line async into explicit state machines,
  hurting 1:1 faithfulness.
- **(b) trait-abstracted poll loop / hand-rolled futures** — rejected: re-implements a worse
  tokio; abortable sleep, streaming, and join-order semantics need a real executor.
- **(c) tokio, current-thread runtime** — chosen.

## Decision detail

- Agent loop written as plain `async fn`s mirroring TS control flow 1:1, run on
  `tokio::runtime::Builder::new_current_thread()` everywhere (harness, CLI, later TUI on the
  same thread).
- **No `tokio::spawn` inside the loop** (exception: fire-and-forget extension `sendMessage`
  → deferred-action queue replayed deterministically in tests, spawned only in prod).
- Sleeps go through a `Sleeper` trait (`RealSleeper` = `tokio::time`, with `time::pause()` +
  `advance()` in tests) so retry-backoff parity is exact.
- Abort = `AbortHandle` (AtomicBool + Notify) checked at the exact TS check points;
  `select!` only inside `sleep` and provider-stream iteration.
- Parity determinism: fake `StreamFn` yields a scripted, finite event list; parallel tool
  calls become `join_all` over fake tools appending to a shared log in start order —
  identical to `Promise.all` over already-started thunks.

## Layer scoping (what stays sync)

- **pi-providers** (`docs/plans/phase3-providers.md` §4.3): parity layer fully sync.
  Pure resolution/composition logic is plain sync functions; concurrency state machines
  (availability refresh, single-flight reloads) are explicit state machines with an
  injectable "run pending refreshes" step driven by the harness; `HttpDispatcher` trait is
  sync (`fn fetch(...) -> Result<HttpResponse, HttpError>`), designed with plain-data
  request/response types so an async twin can wrap it later when real networking lands
  (remote catalog refresh, OAuth device flow, SSE streaming — Phase 4).
- **MCP/extensions** (`docs/plans/phase3-mcp-extensions.md` §4): sync traits everywhere
  (`McpTransport`, `McpToolCaller`, `McpConfigFs`, `SleepFn`); `McpServerConnection`
  single-flight/retry/reconnect is a synchronous state machine; background connection in
  the real binary runs on a `std::thread` (like `bash_executor`), leaving the trait surface
  unchanged under tokio later.

## Consequences

- tokio enters the workspace as a dependency of pi-session (agent layer) only, when the
  agent-session tranche starts — not for the providers/MCP tranches.
- Real network I/O (HTTP providers, MCP transports, SSE) remains behind traits until
  Phase 4; parity always drives scripted fakes.
