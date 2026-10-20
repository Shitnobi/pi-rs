//! `lazyStream` — port of `pi-ai` `src/api/lazy.ts`.
//!
//! TS returns an `AssistantMessageEventStream` synchronously while async
//! setup (auth resolution, module loading) runs behind it; setup failures
//! terminate the stream with an error event carrying a full setup-error
//! `AssistantMessage`. Per the G8 sync decision the deferral collapses:
//! the setup closure runs at call time and sync errors surface as the same
//! error event the TS caller observes on first pull (the TS stream is
//! lazy — nothing is observable until the first read, so running setup at
//! construction is behaviorally identical for a sync caller).
//!
//! `lazyApi` (wrapping dynamically imported API modules as
//! `ProviderStreams`) has no Rust analog: dynamic imports do not exist and
//! the api registry (`pi_ai::compat`) already replaces the module-loading
//! seam — the un-ported builtin stubs there emit this module's error-event
//! shape. Not ported to avoid duplication.

use serde_json::{json, Value};

use crate::error::JsError;
use crate::pi_ai::model::Model;
use crate::pi_ai::provider::{AssistantMessage, AssistantMessageEventStream};

/// TS `createSetupErrorMessage` — a terminal assistant message carrying
/// the setup error with zeroed usage.
pub fn create_setup_error_message(
	model: &Model,
	error: &JsError,
	timestamp: f64,
) -> AssistantMessage {
	AssistantMessage(json!({
		"role": "assistant",
		"content": [],
		"api": model.api,
		"provider": model.provider,
		"model": model.id,
		"usage": {
			"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
		},
		"stopReason": "error",
		"errorMessage": error.message,
		"timestamp": timestamp,
	}))
}

/// The terminal error event TS pushes before ending the stream: the setup
/// error message rides as the event's `error` (and the stream result).
pub fn setup_error_stream(message: &AssistantMessage) -> AssistantMessageEventStream {
	AssistantMessageEventStream(json!({
		"type": "error",
		"reason": "error",
		"error": message.0,
		"result": message.0,
	}))
}

/// TS `lazyStream(model, setup)`. Runs the setup (deferred to first pull
/// in TS — collapsed here, see module docs) and returns either the inner
/// stream or the setup-error stream.
pub fn lazy_stream<F>(
	model: &Model,
	started_at: f64,
	setup: F,
) -> AssistantMessageEventStream
where
	F: FnOnce() -> Result<AssistantMessageEventStream, JsError>,
{
	match setup() {
		Ok(stream) => stream,
		Err(error) => setup_error_stream(&create_setup_error_message(model, &error, started_at)),
	}
}

/// Extract the terminal result of a stream placeholder, if it carries one
/// (setup-error streams record it under `result`).
pub fn stream_result(stream: &AssistantMessageEventStream) -> Option<AssistantMessage> {
	stream
		.0
		.get("result")
		.cloned()
		.map(AssistantMessage)
}

/// Whether the placeholder stream terminated with an error event.
pub fn is_error_stream(stream: &AssistantMessageEventStream) -> bool {
	stream.0.get("type").and_then(Value::as_str) == Some("error")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn model() -> Model {
		serde_json::from_value(json!({
			"id": "m", "name": "m", "api": "a", "provider": "p", "baseUrl": "",
			"input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"reasoning": false, "contextWindow": 100, "maxTokens": 10
		}))
		.unwrap()
	}

	#[test]
	fn setup_success_passes_the_inner_stream_through() {
		let inner = AssistantMessageEventStream(json!({"events": [1]}));
		let out = lazy_stream(&model(), 42.0, || Ok(AssistantMessageEventStream(json!({"events": [1]}))));
		assert_eq!(out.0, inner.0);
		assert!(!is_error_stream(&out));
		assert!(stream_result(&out).is_none());
	}

	#[test]
	fn setup_failure_terminates_with_error_event() {
		let out = lazy_stream(&model(), 42.5, || Err(JsError::new("no key")));
		assert!(is_error_stream(&out));
		let message = stream_result(&out).unwrap();
		assert_eq!(message.0["stopReason"], json!("error"));
		assert_eq!(message.0["errorMessage"], json!("no key"));
		assert_eq!(message.0["timestamp"], json!(42.5));
		assert_eq!(message.0["model"], json!("m"));
		assert_eq!(message.0["provider"], json!("p"));
		assert_eq!(message.0["api"], json!("a"));
		assert_eq!(message.0["usage"]["totalTokens"], json!(0));
		assert_eq!(
			message.0,
			out.0["error"],
			"the event's error is the terminal message"
		);
	}
}
