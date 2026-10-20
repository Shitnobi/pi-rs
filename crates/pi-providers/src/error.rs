//! JavaScript-shaped error type for the provider layer.
//!
//! Mirrors `new Error(message, { cause })`: `message` is the parity-visible
//! string; `cause` chains nested errors (e.g. `getApiKeyAndHeaders` in
//! `model-registry.ts` inspects `error.cause` — plan §2.17).

use std::fmt;

/// A JS `Error`: message plus optional cause chain.
#[derive(Debug, Clone, PartialEq)]
pub struct JsError {
	pub message: String,
	pub cause: Option<Box<JsError>>,
}

impl JsError {
	pub fn new(message: impl Into<String>) -> JsError {
		JsError { message: message.into(), cause: None }
	}

	pub fn with_cause(message: impl Into<String>, cause: JsError) -> JsError {
		JsError { message: message.into(), cause: Some(Box::new(cause)) }
	}

	/// DOMException-style abort: name `"AbortError"`, message
	/// `"This operation was aborted"` (what `signal.throwIfAborted()` rejects
	/// with when no custom reason was set).
	pub fn abort() -> JsError {
		JsError::new("This operation was aborted")
	}
}

impl fmt::Display for JsError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.message)
	}
}

impl std::error::Error for JsError {}

impl From<String> for JsError {
	fn from(message: String) -> JsError {
		JsError::new(message)
	}
}

impl From<&str> for JsError {
	fn from(message: &str) -> JsError {
		JsError::new(message)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn abort_message_matches_dom() {
		assert_eq!(JsError::abort().message, "This operation was aborted");
	}

	#[test]
	fn cause_chain() {
		let inner = JsError::new("inner");
		let outer = JsError::with_cause("outer", inner.clone());
		assert_eq!(outer.message, "outer");
		assert_eq!(outer.cause.as_ref().map(|c| c.message.clone()), Some("inner".to_string()));
	}
}
