//! `ModelsError` — port of `pi-ai` `src/utils/models-error.ts`.
//!
//! TS shape: `class ModelsError extends Error { code: ModelsErrorCode }`
//! with `withCauseDetail` folding the cause's formatted value into the
//! message (callers surface `error.message` only). The Rust port keeps the
//! code (spec calls it `kind`; all six TS codes are kept so `ModelsImpl`
//! can produce parity-exact messages) and folds the cause detail into
//! `message` at construction, then converts to the layer's error channel
//! (`JsError`) — the message is the parity-visible string, so the fold is
//! the only cause surface that matters here.

use crate::error::JsError;

/// TS `ModelsErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsErrorKind {
	ModelSource,
	ModelValidation,
	Provider,
	Stream,
	Auth,
	OAuth,
}

impl ModelsErrorKind {
	pub fn as_str(&self) -> &'static str {
		match self {
			ModelsErrorKind::ModelSource => "model_source",
			ModelsErrorKind::ModelValidation => "model_validation",
			ModelsErrorKind::Provider => "provider",
			ModelsErrorKind::Stream => "stream",
			ModelsErrorKind::Auth => "auth",
			ModelsErrorKind::OAuth => "oauth",
		}
	}
}

/// TS `ModelsError` — a code plus a message (cause detail pre-folded).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelsError {
	pub kind: ModelsErrorKind,
	pub message: String,
}

impl ModelsError {
	pub fn new(kind: ModelsErrorKind, message: impl Into<String>) -> ModelsError {
		ModelsError { kind, message: message.into() }
	}

	/// TS `new ModelsError(code, message, { cause })` — `withCauseDetail`
	/// appends the cause's formatted value when it is non-empty and not
	/// already contained in the message. For the Rust error channel the
	/// cause is a `JsError`, whose formatted value is its message.
	pub fn with_cause(
		kind: ModelsErrorKind,
		message: impl Into<String>,
		cause: &JsError,
	) -> ModelsError {
		let message = message.into();
		let detail = cause.message.trim();
		if detail.is_empty() || message.contains(detail) {
			ModelsError { kind, message }
		} else {
			ModelsError { kind, message: format!("{message}: {detail}") }
		}
	}
}

impl std::fmt::Display for ModelsError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.message)
	}
}

impl std::error::Error for ModelsError {}

impl From<ModelsError> for JsError {
	fn from(error: ModelsError) -> JsError {
		// The message already carries the folded cause detail; the code is
		// not parity-visible (TS callers surface `error.message` only).
		JsError::new(error.message)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn codes_match_ts() {
		assert_eq!(ModelsErrorKind::Provider.as_str(), "provider");
		assert_eq!(ModelsErrorKind::Auth.as_str(), "auth");
		assert_eq!(ModelsErrorKind::OAuth.as_str(), "oauth");
		assert_eq!(ModelsErrorKind::ModelSource.as_str(), "model_source");
	}

	#[test]
	fn cause_detail_folding() {
		let cause = JsError::new("boom");
		let e = ModelsError::with_cause(ModelsErrorKind::Auth, "outer", &cause);
		assert_eq!(e.message, "outer: boom");
		// Already contained → unchanged; empty detail → unchanged.
		let e = ModelsError::with_cause(ModelsErrorKind::Auth, "outer boom", &cause);
		assert_eq!(e.message, "outer boom");
		let e = ModelsError::with_cause(ModelsErrorKind::Auth, "outer", &JsError::new("  "));
		assert_eq!(e.message, "outer");
		// Conversion keeps the message.
		let js: JsError = ModelsError::new(ModelsErrorKind::Provider, "m").into();
		assert_eq!(js.message, "m");
	}
}
