//! Parity-harness entry for the auth/composer wave tool:
//! `provider-composer`.
//!
//! Stub — replaced by the wave implementation.

use serde_json::Value;

pub fn run(tool: &str, _input: &Value) -> Result<Value, String> {
	Err(format!("composer_harness: tool {tool:?} not implemented yet"))
}
