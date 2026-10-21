//! pi-agent: the agent-loop / agent-session layer of the pi coding agent
//! (Rust port of `packages/agent`, TS `pi-agent-core`).
//!
//! Tranche 0 of phase 3 (see docs/plans/phase3-agent-session.md):
//! - [`types`]: the data model (`QueueMode`, `AgentEvent`, `AgentContext`,
//!   `AgentToolResult`, …) and the [`types::AgentTool`] trait boundary
//!   (plan §4).
//! - [`queue`] / [`agent_surface`]: `PendingMessageQueue` and the
//!   steer/follow-up queue surface of `class Agent`.
//! - [`provider`]: the `StreamFn` seam and the deterministic
//!   `ScriptedProvider` fake.
//! - [`host`]: injected clocks, abort handles, and abortable sleep.
//!
//! Async runtime (plan §3, decision G8): everything runs on a
//! **current-thread** tokio runtime — single-threaded cooperative
//! scheduling reproduces the Node event loop's only interleaving points
//! (awaits), keeping parity replays bit-deterministic. No `tokio::spawn`
//! inside the loop; sleeps go through the [`host::Sleeper`] seam.

pub mod agent;
pub mod agent_surface;
pub mod conversion;
pub mod host;
pub mod provider;
pub mod queue;
pub mod r#loop;
pub mod stream_assistant;
pub mod tool_call;
pub mod types;
pub mod validation;

pub use agent::{default_model, empty_usage, Agent, AgentListener, AgentOptions, AgentRun};
pub use agent_surface::AgentQueues;
pub use host::{AbortHandle, Aborted, Clock, FixedClock, Sleeper, TokioSleeper};
pub use provider::{ScriptedProvider, ScriptedTurn, StreamFn, StreamOutput};
pub use queue::PendingMessageQueue;
pub use r#loop::{
    AgentEventSink, AgentLoopTurnUpdate, AgentRequestUpdate, CompletedTurn, ConvertToLlm,
    FnSink, LoopConfig, LoopStreamFn, TransformContext,
};
pub use stream_assistant::stream_assistant_response;
pub use types::{
    AgentContext, AgentEvent, AgentTool, AgentToolCallOutcome, AgentToolResult,
    AgentToolUpdateCallback, BoxFuture, QueueMode, ReplayPolicy, ThinkingLevel,
    ToolExecutionMode,
};

/// Run `future` to completion on a fresh current-thread tokio runtime
/// (plan §3 G8). Used by the parity harness, the CLI, and tests.
pub fn run_current_thread<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current-thread tokio runtime")
        .block_on(future)
}
