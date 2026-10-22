//! Rust port of the pi coding-agent extensions
//! (`packages/coding-agent/src/extensions`).
//!
//! Tightly scoped crates so the harness binary stays small:
//! - [`mcp::log`] — `mcp.log` formatting and appending
//!   (`extensions/mcp/log.ts`)
//! - [`mcp::transport`] — the MCP transport seam for the client runtime
//!   (the `@earendil-works/pi-mcp` package's protocol/transport surface)

pub mod mcp;
