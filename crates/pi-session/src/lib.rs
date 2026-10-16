//! pi-session: session persistence for the pi coding agent.
//!
//! Rust port of `src/core/session-manager.ts` and the config-path helpers it
//! needs from `src/config.ts`. Sessions are append-only trees stored as JSONL
//! files; behavior is validated against the original TypeScript
//! implementation with the differential parity harness in `parity/`.

pub mod compaction;
pub mod compaction_utils;
pub mod config;
pub mod nested_tool_calls;
pub mod session_manager;
