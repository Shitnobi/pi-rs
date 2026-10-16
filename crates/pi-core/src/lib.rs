//! pi-core: deterministic core of the pi coding agent.
//!
//! This crate is a Rust port of the deterministic parts of
//! `@earendil-works/pi-coding-agent` (`src/core/tools/*.ts`, `src/utils/*.ts`).
//! Behavior is validated against the original TypeScript implementation with
//! the differential parity harness in `parity/` (see PARITY.md).

pub mod ansi;
pub mod edit_diff;
pub mod js_string;
pub mod messages;
pub mod jsdiff;
pub mod normalize;
pub mod paths;
pub mod text;
pub mod text_decoder;
pub mod truncate;
