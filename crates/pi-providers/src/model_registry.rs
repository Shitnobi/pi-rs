//! `ModelRegistry` — port of `coding-agent` `src/core/model-registry.ts`.
//!
//! Synchronous compatibility facade exposed to extensions; coding-agent
//! internals use `ModelRuntime` directly. Every method is pure delegation
//! plus the `getApiKeyAndHeaders` `ResolvedRequestAuth` mapping.
//!
//! G8 sync decisions: the TS async surface (`getApiKeyAndHeaders`,
//! `getApiKeyForProvider`, `getAvailableOfType`, `refresh`) is sync here;
//! error channels are `Result<_, JsError>` / `Option`.
//!
//! Re-exports [`clear_api_key_cache`] (TS `export { clearApiKeyCache }`).

use std::sync::Arc;

use crate::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use crate::pi_ai::credential::{AuthOperationOptions, AuthResult};
use crate::pi_ai::model::{
	AnyModel, ClassifierModel, ImageModel, Model, ModelType, ProviderEnv, ProviderHeaders,
};
use crate::pi_ai::models::{ModelsRefreshOptions, ModelsRefreshResult};
use crate::pi_ai::provider::{
	AssistantImages, AssistantMessage, AssistantMessageEventStream, ClassifierContext,
	ClassifierResult, DeferredHandle, ImagesContext, Provider,
};
use crate::provider_composer::{AuthStatus, CompatibilityRequestConfig, ProviderConfigInput};
use crate::virtual_models::VirtualModelDefinition;
use crate::error::JsError;

/// TS `ResolvedRequestAuth` — the flat form: `ok: false` carries only
/// `error`; `ok: true` carries `apiKey`/`headers`/`baseUrl?`/`env`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedRequestAuth {
	pub ok: bool,
	pub api_key: Option<String>,
	pub headers: Option<ProviderHeaders>,
	pub base_url: Option<String>,
	pub env: Option<ProviderEnv>,
	pub error: Option<String>,
}

fn header_pairs_to_provider_headers(pairs: &[(String, String)]) -> ProviderHeaders {
	pairs.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect()
}

/// Synchronous compatibility facade over [`ModelRuntime`] (TS
/// `ModelRegistry`).
pub struct ModelRegistry {
	runtime: ModelRuntime,
}

impl ModelRegistry {
	pub fn new(runtime: ModelRuntime) -> ModelRegistry {
		ModelRegistry { runtime }
	}

	/// Convenience: create the runtime and wrap it (TS callers construct
	/// `ModelRegistry` around an externally created runtime; this mirrors
	/// `ModelRuntime.create`).
	pub fn create(options: CreateModelRuntimeOptions<'_>) -> Result<ModelRegistry, JsError> {
		Ok(ModelRegistry::new(ModelRuntime::create(options)?))
	}

	/// The wrapped runtime.
	pub fn runtime(&self) -> &ModelRuntime {
		&self.runtime
	}

	/// Reload models.json (TS: "await before making synchronous registry
	/// reads"; sync here).
	pub fn refresh(
		&mut self,
		options: Option<&ModelsRefreshOptions<'_>>,
	) -> Result<ModelsRefreshResult, JsError> {
		self.runtime.refresh(options)
	}

	pub fn get_error(&self) -> Option<String> {
		self.runtime.get_error()
	}

	/// TS `getAll()`.
	pub fn get_all(&self) -> Vec<Model> {
		self.runtime.get_models(None)
	}

	/// TS `getAvailable()`.
	pub fn get_available(&self) -> &[Model] {
		self.runtime.get_available_snapshot()
	}

	/// TS `find(provider, modelId)`.
	pub fn find(&self, provider: &str, model_id: &str) -> Option<Model> {
		self.runtime.get_model(provider, model_id)
	}

	/// TS `findOfType(type, provider, modelId)`.
	pub fn find_of_type(
		&self,
		ty: &ModelType,
		provider: &str,
		model_id: &str,
	) -> Option<AnyModel> {
		self.runtime.get_model_of_type(ty, provider, model_id)
	}

	pub fn has_configured_auth(&self, model: &Model) -> bool {
		self.runtime.has_configured_auth(&model.provider)
	}

	/// TS `getApiKeyAndHeaders` — the `ResolvedRequestAuth` mapping.
	pub fn get_api_key_and_headers(&self, model: &Model) -> ResolvedRequestAuth {
		self.get_api_key_and_headers_of_type(&AnyModel::Chat(model.clone()))
	}

	/// `getApiKeyAndHeaders` for a model of any type (chat/image/
	/// classifier); the TS signature is chat-only but the resolution path
	/// is model-type agnostic.
	pub fn get_api_key_and_headers_of_type(&self, model: &AnyModel) -> ResolvedRequestAuth {
		match self.runtime.get_auth_for_model(model, None) {
			Ok(Some(resolution)) => ResolvedRequestAuth {
				ok: true,
				api_key: resolution.auth.api_key.clone(),
				headers: resolution.auth.headers.clone(),
				base_url: resolution.auth.base_url.clone(),
				env: resolution.env,
				error: None,
			},
			Ok(None) => match self.runtime.get_compatibility_request_config(model) {
				Ok(compatibility) => self.no_resolution_result(model, &compatibility),
				Err(error) => ResolvedRequestAuth {
					ok: false,
					error: Some(error.message),
					..Default::default()
				},
			},
			Err(error) => ResolvedRequestAuth {
				ok: false,
				error: Some(self.translate_auth_error(model, error)),
				..Default::default()
			},
		}
	}

	fn no_resolution_result(
		&self,
		model: &AnyModel,
		compatibility: &CompatibilityRequestConfig,
	) -> ResolvedRequestAuth {
		if compatibility.auth_header {
			return ResolvedRequestAuth {
				ok: false,
				error: Some(format!("No API key found for \"{}\"", model.provider())),
				..Default::default()
			};
		}
		ResolvedRequestAuth {
			ok: true,
			headers: compatibility
				.headers
				.as_ref()
				.map(|pairs| header_pairs_to_provider_headers(pairs)),
			..Default::default()
		}
	}

	/// TS catch: `cause.message === "authHeader requires a resolved API
	/// key"` → the no-api-key error; otherwise the (cause) message.
	fn translate_auth_error(&self, model: &AnyModel, error: JsError) -> String {
		let message = match &error.cause {
			Some(cause) => cause.message.clone(),
			None => error.message,
		};
		if message == "authHeader requires a resolved API key" {
			format!("No API key found for \"{}\"", model.provider())
		} else {
			message
		}
	}

	pub fn get_provider_auth_status(&self, provider: &str) -> AuthStatus {
		self.runtime.get_provider_auth_status(provider)
	}

	pub fn get_provider(&self, provider: &str) -> Option<Arc<Provider>> {
		self.runtime.get_provider(provider)
	}

	/// Stream through the configured provider with request-time
	/// authentication.
	pub fn stream(
		&self,
		model: &Model,
		context: &serde_json::Value,
		options: Option<&serde_json::Value>,
	) -> AssistantMessageEventStream {
		self.runtime.stream(model, context, options)
	}

	/// Stream with provider-neutral options and request-time
	/// authentication.
	pub fn stream_simple(
		&self,
		model: &Model,
		context: &serde_json::Value,
		options: Option<&serde_json::Value>,
	) -> AssistantMessageEventStream {
		self.runtime.stream_simple(model, context, options)
	}

	pub fn complete(
		&self,
		model: &Model,
		context: &serde_json::Value,
		options: Option<&serde_json::Value>,
	) -> Result<AssistantMessage, JsError> {
		self.runtime.complete(model, context, options)
	}

	pub fn complete_simple(
		&self,
		model: &Model,
		context: &serde_json::Value,
		options: Option<&serde_json::Value>,
	) -> Result<AssistantMessage, JsError> {
		self.runtime.complete_simple(model, context, options)
	}

	/// Every known model of a type (chat, image, classifier), optionally
	/// for one provider.
	pub fn get_models_of_type(&self, ty: &ModelType, provider: Option<&str>) -> Vec<AnyModel> {
		self.runtime.get_models_of_type(ty, provider)
	}

	/// Models of a type whose provider has working credentials.
	pub fn get_available_of_type(
		&self,
		ty: &ModelType,
		provider: Option<&str>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Vec<AnyModel>, JsError> {
		self.runtime.get_available_of_type(ty, provider, options)
	}

	pub fn get_model_of_type(
		&self,
		ty: &ModelType,
		provider: &str,
		model_id: &str,
	) -> Option<AnyModel> {
		self.runtime.get_model_of_type(ty, provider, model_id)
	}

	/// Classify structured state with request-time authentication. Never
	/// rejects (errors carry into the result).
	pub fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<&serde_json::Value>,
	) -> ClassifierResult {
		self.runtime.classify(model, context, options)
	}

	/// Generate images with request-time authentication. Never rejects.
	pub fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<&serde_json::Value>,
	) -> AssistantImages {
		self.runtime.generate_images(model, context, options)
	}

	pub fn get_provider_display_name(&self, provider: &str) -> String {
		self.runtime
			.get_provider(provider)
			.map(|p| p.name.clone())
			.unwrap_or_else(|| provider.to_string())
	}

	pub fn get_provider_auth(
		&self,
		provider: &str,
	) -> Result<Option<AuthResult>, JsError> {
		self.runtime.get_auth_for_provider(provider, None)
	}

	pub fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
		// TS: catch → undefined.
		self.runtime
			.get_auth_for_provider(provider, None)
			.ok()
			.flatten()
			.and_then(|resolution| resolution.auth.api_key.clone())
	}

	pub fn is_using_oauth(&self, model: &Model) -> bool {
		self.runtime.is_using_oauth(&model.provider)
	}

	/// TS `registerProvider(provider: Provider)`.
	pub fn register_native_provider(&mut self, provider: Provider) {
		self.runtime.register_native_provider(provider)
	}

	/// TS `registerProvider(providerName, config)`.
	pub fn register_provider(&mut self, provider_name: &str, config: ProviderConfigInput) {
		self.runtime.register_provider(provider_name, config)
	}

	/// The checked form of both TS overloads: `Provider` objects delegate
	/// to `registerNativeProvider`; by-name registrations require a config
	/// (`Provider config is required when registering by name`).
	pub fn register_provider_entry(
		&mut self,
		provider_or_name: ProviderOrName<'_>,
	) -> Result<(), JsError> {
		match provider_or_name {
			ProviderOrName::Provider(provider) => {
				self.runtime.register_native_provider_checked(provider)
			}
			ProviderOrName::ByName { name, config } => match config {
				Some(config) => self.runtime.register_provider_checked(name, config),
				None => Err(JsError::new(
					"Provider config is required when registering by name",
				)),
			},
		}
	}

	pub fn unregister_provider(&mut self, provider_name: &str) {
		self.runtime.unregister_provider(provider_name)
	}

	pub fn register_virtual_model(&mut self, definition: VirtualModelDefinition) {
		self.runtime.register_virtual_model(definition)
	}

	pub fn unregister_virtual_model(&mut self, provider_name: &str, id: &str) {
		self.runtime.unregister_virtual_model(provider_name, id)
	}

	pub fn get_registered_provider_config(&self, provider_name: &str) -> Option<&ProviderConfigInput> {
		self.runtime.get_registered_provider_config(provider_name)
	}

	pub fn get_registered_native_provider(&self, provider_name: &str) -> Option<&Provider> {
		self.runtime.get_registered_native_provider(provider_name)
	}

	pub fn get_registered_provider_ids(&self) -> Vec<String> {
		self.runtime.get_registered_provider_ids()
	}

	/// Deferred-response surface (delegation; TS registry does not expose
	/// these but the harness needs them through one facade).
	pub fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&serde_json::Value>,
	) -> AssistantMessageEventStream {
		self.runtime.stream_deferred(model, handle, options)
	}

	pub fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&serde_json::Value>,
	) -> Result<AssistantMessage, JsError> {
		self.runtime.fetch_deferred(model, handle, options)
	}

	pub fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&serde_json::Value>,
	) -> Result<(), JsError> {
		self.runtime.cancel_deferred(model, handle, options)
	}
}

/// TS `registerProvider(providerOrName: Provider | string, config?)` —
/// the overload argument.
pub enum ProviderOrName<'a> {
	Provider(Provider),
	ByName {
		name: &'a str,
		config: Option<ProviderConfigInput>,
	},
}

/// TS re-export `clearApiKeyCache`.
pub use crate::provider_composer::clear_api_key_cache;
