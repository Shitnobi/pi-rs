//! `pi-ai/compat` — the API-provider registry subset the composer needs.
//!
//! Port of `packages/ai/src/compat.ts`: `registerApiProvider`,
//! `getApiProvider`, `unregisterApiProviders`, `resetApiProviders` and the
//! `BUILTIN_APIS` list. The builtin API *stream implementations* (network
//! clients) are out of scope for the provider layer; registering a builtin
//! api here records its id (so `getApiProvider` finds it) with a stream that
//! reports the "not ported" setup error. Parity harnesses (and extensions)
//! replace entries with markers via `register_api_provider`, exactly like TS
//! `registerApiProvider` overwriting a builtin `Map` entry.

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;

use crate::pi_ai::model::Model;
use crate::pi_ai::provider::{
	AssistantMessageEventStream, StreamOptions, TranscriptContext,
};

/// The builtin api ids (`BUILTIN_APIS` in compat.ts, same order).
pub const BUILTIN_APIS: [&str; 10] = [
	"anthropic-messages",
	"openai-completions",
	"openai-responses",
	"openai-codex-responses",
	"azure-openai-responses",
	"google-generative-ai",
	"google-vertex",
	"mistral-conversations",
	"bedrock-converse-stream",
	"pi-messages",
];

pub type ApiStreamFn = Arc<
	dyn Fn(&Model, &TranscriptContext, Option<&StreamOptions>) -> AssistantMessageEventStream
		+ Send
		+ Sync,
>;

/// One registered api provider (`ApiProviderInternal`): the api id plus its
/// wrapped stream functions.
#[derive(Clone)]
pub struct ApiProvider {
	pub api: String,
	pub stream: ApiStreamFn,
	pub stream_simple: ApiStreamFn,
}

struct RegistryEntry {
	provider: ApiProvider,
	source_id: Option<String>,
}

fn registry() -> &'static Mutex<Vec<RegistryEntry>> {
	static REGISTRY: OnceLock<Mutex<Vec<RegistryEntry>>> = OnceLock::new();
	REGISTRY.get_or_init(|| {
		let mut entries = Vec::new();
		for api in BUILTIN_APIS {
			entries.push(RegistryEntry {
				provider: builtin_stub(api),
				source_id: None,
			});
		}
		Mutex::new(entries)
	})
}

/// The setup-error event stream every un-ported stream returns (the real
/// pi-ai clients land with the agent-session layer). Observable shape
/// mirrors `lazyStream`'s catch: an error event carrying `errorMessage`.
fn setup_error_stream(message: String) -> AssistantMessageEventStream {
	AssistantMessageEventStream(json!({
		"type": "error", "reason": "error",
		"error": { "errorMessage": message }
	}))
}

/// `wrapStream`/`wrapStreamSimple`: the api-mismatch guard (TS throws
/// synchronously inside the lazy setup; here the throw surfaces as the same
/// setup-error event the TS caller observes).
fn wrap(api: &str, _tag: &'static str, inner: ApiStreamFn) -> ApiStreamFn {
	let api = api.to_string();
	Arc::new(move |model: &Model, context, options| {
		if model.api != api {
			return setup_error_stream(format!("Mismatched api: {} expected {}", model.api, api));
		}
		inner(model, context, options)
	})
}

fn builtin_stub(api: &str) -> ApiProvider {
	let api = api.to_string();
	let message = format!("pi-ai api implementation not ported: {api}");
	let stream: ApiStreamFn = Arc::new(move |_, _, _| setup_error_stream(message.clone()));
	ApiProvider {
		api: api.clone(),
		stream: stream.clone(),
		stream_simple: stream,
	}
}

/// `registerApiProvider(provider, sourceId?)` — replaces any existing entry
/// for the api (TS `Map.set`).
pub fn register_api_provider(
	api: &str,
	stream: ApiStreamFn,
	stream_simple: ApiStreamFn,
	source_id: Option<String>,
) {
	let mut registry = registry().lock().unwrap();
	registry.retain(|entry| entry.provider.api != api);
	registry.push(RegistryEntry {
		provider: ApiProvider {
			api: api.to_string(),
			stream: wrap(api, "stream", stream),
			stream_simple: wrap(api, "streamSimple", stream_simple),
		},
		source_id,
	});
}

/// `getApiProvider(api)`.
pub fn get_api_provider(api: &str) -> Option<ApiProvider> {
	registry()
		.lock()
		.unwrap()
		.iter()
		.find(|entry| entry.provider.api == api)
		.map(|entry| entry.provider.clone())
}

/// `unregisterApiProviders(sourceId)` — removes entries registered with the
/// source id (builtin entries have none and survive).
pub fn unregister_api_providers(source_id: &str) {
	registry()
		.lock()
		.unwrap()
		.retain(|entry| entry.source_id.as_deref() != Some(source_id));
}

/// `resetApiProviders()` — clear and re-register the builtins.
pub fn reset_api_providers() {
	let mut registry = registry().lock().unwrap();
	registry.clear();
	for api in BUILTIN_APIS {
		registry.push(RegistryEntry { provider: builtin_stub(api), source_id: None });
	}
}
