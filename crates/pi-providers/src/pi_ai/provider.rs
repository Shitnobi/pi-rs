//! Provider — port of `pi-ai` `src/models.ts` `Provider` interface (l155)
//! and `src/types.ts` `ProviderStreams` / `StreamFunction` (l311–402).
//!
//! The TS interface is structural; the Rust port is a struct of boxed
//! closures so the composer wave can override single operations while
//! keeping the rest (the TS layer does the same with `{ ...provider,
//! stream: … }` spreads). Per the G8 async-runtime decision every
//! Promise-valued operation is sync; `cancelDeferred`/`generateImages`/
//! `classify` (TS `Promise`s) return results directly, encoding failures
//! the way the TS contract documents (`never rejects` → error results).
//!
//! The transcript/event/message shapes are agent-layer types that land with
//! the agent-session tranches (docs/plans/phase3-agent-session.md); here
//! they are opaque placeholders over their JSON form so the provider layer
//! can hold and pass them through, and parity cases can construct them.

use crate::error::JsError;
use crate::pi_ai::credential::{Credential, ProviderAuth};
use crate::pi_ai::model::{
	AnyModel, ClassifierModel, ImageModel, Model, ProviderHeaders,
};
use crate::pi_ai::models::RefreshModelsContext;

// ============================================================================
// Agent-layer placeholder shapes
// ============================================================================

/// TS `TranscriptContext` — the normalized transcript (system prompt and
/// tools in the leading system message). Placeholder JSON form until the
/// typed port lands with the agent-session tranches.
#[derive(Debug, Clone, Default)]
pub struct TranscriptContext(pub serde_json::Value);

/// TS `AssistantMessageEventStream` — the event stream every stream
/// operation returns. Placeholder JSON handle (the real async-iterator
/// port lands with the agent-session tranches).
#[derive(Debug, Clone, Default)]
pub struct AssistantMessageEventStream(pub serde_json::Value);

/// TS `AssistantMessage` — one completed assistant response.
#[derive(Debug, Clone, Default)]
pub struct AssistantMessage(pub serde_json::Value);

/// TS `ImagesContext` — image-generation request context.
#[derive(Debug, Clone, Default)]
pub struct ImagesContext(pub serde_json::Value);

/// TS `ClassifierContext` — classification request context.
#[derive(Debug, Clone, Default)]
pub struct ClassifierContext(pub serde_json::Value);

/// TS `DeferredHandle` — durable handle of a deferred response.
#[derive(Debug, Clone, Default)]
pub struct DeferredHandle(pub serde_json::Value);

/// TS `AssistantImages` — image-generation result (error results carry
/// `stopReason: "error"` inside the value).
#[derive(Debug, Clone, Default)]
pub struct AssistantImages(pub serde_json::Value);

/// TS `ClassifierResult` — classification result.
#[derive(Debug, Clone, Default)]
pub struct ClassifierResult(pub serde_json::Value);

// ============================================================================
// Option records (untyped pass-through at this layer)
// ============================================================================

/// TS `StreamOptions` — the untyped option record API modules accept.
pub type StreamOptions = serde_json::Value;
/// TS `SimpleStreamOptions` (provider-neutral fields ride in the JSON).
pub type SimpleStreamOptions = serde_json::Value;
/// TS `DeferredFetchOptions`.
pub type DeferredFetchOptions = serde_json::Value;
/// TS `DeferredCancelOptions`.
pub type DeferredCancelOptions = serde_json::Value;
/// TS `ImagesOptions`.
pub type ImagesOptions = serde_json::Value;
/// TS `ClassifierOptions`.
pub type ClassifierOptions = serde_json::Value;

// ============================================================================
// Provider operation types (TS `ProviderStreams` / `StreamFunction` etc.)
// ============================================================================

pub type GetModelsFn = Box<dyn Fn() -> Vec<Model> + Send + Sync>;
pub type GetAllModelsFn = Box<dyn Fn() -> Vec<AnyModel> + Send + Sync>;
pub type RefreshModelsFn =
	Box<dyn Fn(&mut RefreshModelsContext<'_>) -> Result<(), JsError> + Send + Sync>;
pub type FilterModelsFn =
	Box<dyn Fn(&[Model], Option<&Credential>) -> Vec<Model> + Send + Sync>;
pub type FilterAllModelsFn =
	Box<dyn Fn(&[AnyModel], Option<&Credential>) -> Vec<AnyModel> + Send + Sync>;
pub type StreamFn = Box<
	dyn Fn(&Model, &TranscriptContext, Option<&StreamOptions>) -> AssistantMessageEventStream
		+ Send
		+ Sync,
>;
pub type StreamSimpleFn = Box<
	dyn Fn(&Model, &TranscriptContext, Option<&SimpleStreamOptions>) -> AssistantMessageEventStream
		+ Send
		+ Sync,
>;
pub type FetchDeferredFn = Box<
	dyn Fn(&Model, &DeferredHandle, Option<&DeferredFetchOptions>) -> AssistantMessageEventStream
		+ Send
		+ Sync,
>;
pub type CancelDeferredFn =
	Box<dyn Fn(&Model, &DeferredHandle, Option<&DeferredCancelOptions>) -> Result<(), JsError> + Send + Sync>;
pub type GenerateImagesFn = Box<
	dyn Fn(&ImageModel, &ImagesContext, Option<&ImagesOptions>) -> AssistantImages + Send + Sync,
>;
pub type ClassifyFn = Box<
	dyn Fn(&ClassifierModel, &ClassifierContext, Option<&ClassifierOptions>) -> ClassifierResult
		+ Send
		+ Sync,
>;

// ============================================================================
// Provider
// ============================================================================

/// A provider is the concrete runtime unit: id/name/base metadata, auth
/// methods, model listing, and the operations its models support
/// (streaming, image generation, classification). Port of the TS
/// `Provider<TApi>` interface — the `TApi` typing of chat model lists is a
/// TS-only static distinction; here everything is `Model` and narrowing is
/// runtime via `crate::pi_ai::models::has_api`.
///
/// Construct with [`Provider::new`] (the required TS members) and add the
/// optional operations with the `with_*` builders.
pub struct Provider {
	pub id: String,
	pub name: String,
	pub base_url: Option<String>,
	pub headers: Option<ProviderHeaders>,
	/// Required: at least one of `apiKey`/`oauth`.
	pub auth: ProviderAuth,
	get_models_fn: GetModelsFn,
	get_all_models_fn: Option<GetAllModelsFn>,
	refresh_models_fn: Option<RefreshModelsFn>,
	filter_models_fn: Option<FilterModelsFn>,
	filter_all_models_fn: Option<FilterAllModelsFn>,
	stream_fn: StreamFn,
	stream_simple_fn: StreamSimpleFn,
	fetch_deferred_fn: Option<FetchDeferredFn>,
	cancel_deferred_fn: Option<CancelDeferredFn>,
	generate_images_fn: Option<GenerateImagesFn>,
	classify_fn: Option<ClassifyFn>,
}

impl Provider {
	/// The required TS members: id, name, auth, `getModels`, `stream`,
	/// `streamSimple`.
	pub fn new(
		id: String,
		name: String,
		auth: ProviderAuth,
		get_models: GetModelsFn,
		stream: StreamFn,
		stream_simple: StreamSimpleFn,
	) -> Provider {
		Provider {
			id,
			name,
			base_url: None,
			headers: None,
			auth,
			get_models_fn: get_models,
			get_all_models_fn: None,
			refresh_models_fn: None,
			filter_models_fn: None,
			filter_all_models_fn: None,
			stream_fn: stream,
			stream_simple_fn: stream_simple,
			fetch_deferred_fn: None,
			cancel_deferred_fn: None,
			generate_images_fn: None,
			classify_fn: None,
		}
	}

	pub fn with_base_url(mut self, base_url: Option<String>) -> Provider {
		self.base_url = base_url;
		self
	}

	pub fn with_headers(mut self, headers: Option<ProviderHeaders>) -> Provider {
		self.headers = headers;
		self
	}

	pub fn with_get_all_models(mut self, f: GetAllModelsFn) -> Provider {
		self.get_all_models_fn = Some(f);
		self
	}

	pub fn with_refresh_models(mut self, f: RefreshModelsFn) -> Provider {
		self.refresh_models_fn = Some(f);
		self
	}

	pub fn with_filter_models(mut self, f: FilterModelsFn) -> Provider {
		self.filter_models_fn = Some(f);
		self
	}

	pub fn with_filter_all_models(mut self, f: FilterAllModelsFn) -> Provider {
		self.filter_all_models_fn = Some(f);
		self
	}

	pub fn with_fetch_deferred(mut self, f: FetchDeferredFn) -> Provider {
		self.fetch_deferred_fn = Some(f);
		self
	}

	pub fn with_cancel_deferred(mut self, f: CancelDeferredFn) -> Provider {
		self.cancel_deferred_fn = Some(f);
		self
	}

	pub fn with_generate_images(mut self, f: GenerateImagesFn) -> Provider {
		self.generate_images_fn = Some(f);
		self
	}

	pub fn with_classify(mut self, f: ClassifyFn) -> Provider {
		self.classify_fn = Some(f);
		self
	}

	/// Current known chat models, sync. Must not throw (the TS contract);
	/// a panicking implementation is a bug, and `Models` treats failure as
	/// no models in TS by catching — the Rust collection applies the same
	/// policy at its boundary.
	pub fn get_models(&self) -> Vec<Model> {
		(self.get_models_fn)()
	}

	/// Current known models of every type. Providers with only chat models
	/// may omit `getAllModels`; the fallback is `getModels()` as chat
	/// models, exactly like the TS `Models` collection.
	pub fn get_all_models(&self) -> Vec<AnyModel> {
		match &self.get_all_models_fn {
			Some(f) => f(),
			None => self.get_models().into_iter().map(AnyModel::Chat).collect(),
		}
	}

	/// Whether this provider has its own `getAllModels` (the fallback above
	/// applies otherwise).
	pub fn has_get_all_models(&self) -> bool {
		self.get_all_models_fn.is_some()
	}

	/// Dynamic providers only: the `refreshModels` operation, when present.
	/// Returned as the boxed closure so callers can check support and call
	/// it in one place (TS `provider.refreshModels?.(context)`).
	pub fn refresh_models_fn(&self) -> Option<&RefreshModelsFn> {
		self.refresh_models_fn.as_ref()
	}

	/// Whether this provider has its own `filterModels` policy.
	pub fn has_filter_models(&self) -> bool {
		self.filter_models_fn.is_some()
	}

	/// Whether this provider has its own `filterAllModels` policy.
	pub fn has_filter_all_models(&self) -> bool {
		self.filter_all_models_fn.is_some()
	}

	/// Optional credential-specific chat model availability policy.
	/// `None` (the TS absent case) keeps every model.
	pub fn filter_models(
		&self,
		models: &[Model],
		credential: Option<&Credential>,
	) -> Vec<Model> {
		match &self.filter_models_fn {
			Some(f) => f(models, credential),
			None => models.to_vec(),
		}
	}

	/// Optional credential-specific availability policy across every model
	/// type.
	pub fn filter_all_models(
		&self,
		models: &[AnyModel],
		credential: Option<&Credential>,
	) -> Vec<AnyModel> {
		match &self.filter_all_models_fn {
			Some(f) => f(models, credential),
			None => models.to_vec(),
		}
	}

	/// Stream a normalized transcript (TS `stream`).
	pub fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&StreamOptions>,
	) -> AssistantMessageEventStream {
		(self.stream_fn)(model, context, options)
	}

	/// Stream with provider-neutral options (TS `streamSimple`).
	pub fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&SimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		(self.stream_simple_fn)(model, context, options)
	}

	/// Fetch a deferred response (TS `fetchDeferred`, optional).
	pub fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredFetchOptions>,
	) -> Option<AssistantMessageEventStream> {
		self.fetch_deferred_fn.as_ref().map(|f| f(model, handle, options))
	}

	/// Cancel a deferred response (TS `cancelDeferred`, optional).
	pub fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredCancelOptions>,
	) -> Option<Result<(), JsError>> {
		self.cancel_deferred_fn.as_ref().map(|f| f(model, handle, options))
	}

	/// Generate images (TS `generateImages`, optional, never rejects in TS:
	/// error results are carried inside `AssistantImages`).
	pub fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<&ImagesOptions>,
	) -> Option<AssistantImages> {
		self.generate_images_fn.as_ref().map(|f| f(model, context, options))
	}

	/// Classify structured state (TS `classify`, optional, never rejects).
	pub fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<&ClassifierOptions>,
	) -> Option<ClassifierResult> {
		self.classify_fn.as_ref().map(|f| f(model, context, options))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::credential::ProviderAuth;
	use serde_json::json;

	fn chat_model(id: &str, api: &str) -> Model {
		serde_json::from_value(json!({
			"id": id, "name": id, "api": api, "provider": "p",
			"baseUrl": "https://x", "input": ["text"],
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"reasoning": false, "contextWindow": 128000, "maxTokens": 16384
		}))
		.unwrap()
	}

	fn no_auth() -> ProviderAuth {
		// Keyless provider shape (like TS withVirtualModels): apiKey auth
		// whose resolve reports configured.
		ProviderAuth { api_key: None, oauth: None }
	}

	#[test]
	fn required_surface_and_fallbacks() {
		let models = vec![chat_model("m1", "openai-completions")];
		let provider = Provider::new(
			"p".to_string(),
			"P".to_string(),
			no_auth(),
			Box::new(move || models.clone()),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
		);
		assert_eq!(provider.get_models().len(), 1);
		// getAllModels falls back to getModels as chat models.
		let all = provider.get_all_models();
		assert_eq!(all.len(), 1);
		assert!(matches!(all[0], AnyModel::Chat(_)));
		assert!(!provider.has_get_all_models());
		// Optional operations are absent.
		assert!(provider.refresh_models_fn().is_none());
		assert_eq!(
			provider.filter_models(&provider.get_models(), None).len(),
			1
		);
		assert!(provider.fetch_deferred(&provider.get_models()[0], &DeferredHandle::default(), None).is_none());
		assert!(provider.cancel_deferred(&provider.get_models()[0], &DeferredHandle::default(), None).is_none());
	}

	#[test]
	fn builder_overrides_optional_operations() {
		let provider = Provider::new(
			"p".to_string(),
			"P".to_string(),
			no_auth(),
			Box::new(std::vec::Vec::new),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
		)
		.with_base_url(Some("https://api".to_string()))
		.with_filter_models(Box::new(|models, _| {
			models.iter().filter(|m| m.id != "hidden").cloned().collect()
		}));
		assert_eq!(provider.base_url.as_deref(), Some("https://api"));
		let models = vec![chat_model("hidden", "a"), chat_model("kept", "a")];
		assert_eq!(provider.filter_models(&models, None).len(), 1);
		assert_eq!(provider.filter_models(&models, None)[0].id, "kept");
	}

	#[test]
	fn stream_dispatch_passes_through() {
		let provider = Provider::new(
			"p".to_string(),
			"P".to_string(),
			no_auth(),
			Box::new(std::vec::Vec::new),
			Box::new(|_, _, options| {
				AssistantMessageEventStream(
					options.cloned().unwrap_or(serde_json::Value::Null),
				)
			}),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
		);
		let model = chat_model("m", "a");
		let opts = json!({"tag": 1});
		let stream = provider.stream(&model, &TranscriptContext::default(), Some(&opts));
		assert_eq!(stream.0, opts);
	}
}
