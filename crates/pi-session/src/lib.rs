//! pi-session: the agent-session layer of the pi coding agent (Rust port
//! of `packages/coding-agent/src/core/agent-session.ts`).
//!
//! Tranche 8 of phase 3 (see docs/plans/phase3-agent-session.md): the
//! session event-handling slice — [`agent_session::AgentSession`]
//! (`_handleAgentEvent`, persistence mapping, queue bookkeeping,
//! `_emitAgentSettled` + deferred actions, idle wait, subscribe/dispose)
//! and [`retry_error`] (`isContextOverflow` / `isRetryableAssistantError`
//! from `packages/ai/src/utils`).
//!
//! Later tranches (9–19) extend the `agent_session` module tree: tools,
//! prompt, run orchestration, retry, compaction, model, bash, tree, stats,
//! runtime. Everything the host must inject lives behind traits
//! ([`agent_session::SessionSettingsStore`],
//! [`agent_session::ExtensionHost`], [`agent_session::NestedCallsStore`])
//! so no library code reads ambient state.

pub mod agent_session;
pub mod mcp_config;
pub mod retry_error;
