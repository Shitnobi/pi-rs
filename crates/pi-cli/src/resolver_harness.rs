//! Parity-harness entries for the config/resolution wave tool:
//! `model-resolver`.
//!
//! Wire protocol is documented in `docs/plans/phase3-providers.md` §5.
//! This stub is replaced by the wave implementation.

use serde_json::Value;

/// Dispatch a model-resolver harness tool.
pub fn run(tool: &str, _input: &Value) -> Result<Value, String> {
    Err(format!("resolver_harness: tool {tool:?} not implemented yet"))
}
