//! Parity-harness entries for the auth/composer wave tools:
//! `auth-storage`, `remote-catalog`.
//!
//! Stub — replaced by the wave implementation.

use serde_json::Value;

pub fn run(tool: &str, _input: &Value) -> Result<Value, String> {
	Err(format!("auth_harness: tool {tool:?} not implemented yet"))
}
