//! Parity-harness entries for the facade wave tools: `model-runtime`,
//! `model-registry`.
//!
//! Wire protocol (TS mirror contract): JSON args with a `fn` discriminator
//! (or a `steps` array of such records, run in order against one runtime);
//! results are `{ result: { results: [...], storeOps: [...], routes: [...] } }`,
//! step errors surface in-band as `{ error, causes }` entries.
//!
//! Common (construction) args:
//! - `env`: injected environment map (both sides scrub the ambient env).
//! - `offline`: inject `PI_OFFLINE` (default false).
//! - `authJson`: auth.json content (written to a per-case temp file).
//! - `modelsJson`: models.json content (written to a per-case temp file;
//!   absent = the TS `modelsPath: null`).
//! - `stored`: providerId → stored catalog entry (seed of the recording
//!   models store injected on both sides).
//! - `fetch`: `{ status, lastModified?, etag?, body? }` scripted catalog
//!   response — LAZY: a case without a spec only errors when the port
//!   actually performs a network request.
//! - `allowModelNetwork` (default false), `refreshOnCreate` (default
//!   false), `nowMs` (default 0; the deterministic `Date.now`).
//!
//! Steps (`fn` values) mirror `ModelRuntime`/`ModelRegistry` methods; see
//! the match arms below. `settle: true` on register/unregister steps runs
//! an explicit `refresh({ allowNetwork: false })` afterwards (the TS
//! fire-and-forget refresh is deliberately not run inline by the Rust
//! port); cases with a settle step omit `storeOps` from the output because
//! the TS side runs the discarded void refresh alongside the explicit one.

use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use pi_providers::config_value::Env;
use pi_providers::error::JsError;
use pi_providers::model_registry::ModelRegistry;
use pi_providers::model_runtime::{CatalogHooks, CreateModelRuntimeOptions, ModelRuntime};
use pi_providers::pi_ai::credential::{
	AuthCheck, AuthResult, AuthType, CredentialInfo,
};
use pi_providers::pi_ai::model::{AnyModel, Model, ModelThinkingLevel, ModelType, ProviderHeaders};
use pi_providers::pi_ai::models::{AuthInteraction, AuthPrompt, AuthEvent, ModelsRefreshOptions};
use pi_providers::pi_ai::models_store::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use pi_providers::pi_ai::credential::ProviderAuth;
use pi_providers::pi_ai::provider::Provider;
use pi_providers::provider_composer::{
	ExtensionOAuthConfig, ProviderClassifier, ProviderConfigInput, ProviderImages,
};
use pi_providers::remote_catalog::{CatalogRequest, CatalogResponse};
use pi_providers::virtual_models::{
	ModelRoute, ModelRouteReason, ModelRouteRequest, RouteFn, VirtualModelDefinition,
};

use crate::composer_harness::provider_headers_value;

pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
	match tool {
		"model-runtime" => run_cases(input, false),
		"model-registry" => run_cases(input, true),
		other => Err(format!("unknown facade tool: {other}")),
	}
}

// ============================================================================
// Shared case scaffolding
// ============================================================================

/// The scripted catalog fetch — LAZY: without a response spec the closure
/// errors (only observable when a network request actually happens).
fn scripted_fetch(spec: Option<Value>) -> pi_providers::model_runtime::CatalogFetchFn {
	Arc::new(move |_request: &CatalogRequest| match &spec {
		Some(spec) => Ok(catalog_response(spec)?),
		None => Err(JsError::new("no scripted fetch response")),
	})
}

fn catalog_response(spec: &Value) -> Result<CatalogResponse, JsError> {
	let status = spec["status"].as_u64().unwrap_or(200) as u16;
	Ok(CatalogResponse {
		status,
		ok: (200..300).contains(&status),
		last_modified: spec.get("lastModified").and_then(Value::as_str).map(str::to_string),
		etag: spec.get("etag").and_then(Value::as_str).map(str::to_string),
		body: spec.get("body").cloned().unwrap_or(Value::Null),
	})
}

/// A `ModelsStore` that records every operation (the TS harness injects
/// the mirror class; `storeOps` in the result compares them).
struct RecordingModelsStore {
	ops: Arc<Mutex<Vec<Value>>>,
	entries: Arc<Mutex<Map<String, Value>>>,
}

impl ModelsStore for RecordingModelsStore {
	fn read(
		&self,
		provider_id: &str,
		_options: Option<&ModelsStoreOperationOptions>,
	) -> Result<Option<ModelsStoreEntry>, JsError> {
		self.ops
			.lock()
			.unwrap_or_else(|p| p.into_inner())
			.push(json!({ "op": "read", "providerId": provider_id }));
		let entry = self
			.entries
			.lock()
			.unwrap_or_else(|p| p.into_inner())
			.get(provider_id)
			.cloned();
		Ok(entry.as_ref().and_then(ModelsStoreEntry::from_value))
	}
	fn write(
		&mut self,
		provider_id: &str,
		entry: &ModelsStoreEntry,
		_options: Option<&ModelsStoreOperationOptions>,
	) -> Result<(), JsError> {
		let value = entry.to_value();
		self.ops
			.lock()
			.unwrap_or_else(|p| p.into_inner())
			.push(json!({ "op": "write", "providerId": provider_id, "entry": value }));
		self.entries
			.lock()
			.unwrap_or_else(|p| p.into_inner())
			.insert(provider_id.to_string(), value);
		Ok(())
	}
	fn delete(
		&mut self,
		provider_id: &str,
		_options: Option<&ModelsStoreOperationOptions>,
	) -> Result<(), JsError> {
		self.ops
			.lock()
			.unwrap_or_else(|p| p.into_inner())
			.push(json!({ "op": "delete", "providerId": provider_id }));
		self.entries
			.lock()
			.unwrap_or_else(|p| p.into_inner())
			.shift_remove(provider_id);
		Ok(())
	}
}

/// Per-case temp directory (auth-storage precedent).
fn temp_case_dir() -> String {
	use std::sync::atomic::{AtomicU64, Ordering};
	static COUNTER: AtomicU64 = AtomicU64::new(0);
	let dir = std::env::temp_dir()
		.join("pi-parity-model-runtime")
		.join(format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
	std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).expect("temp dir");
	dir.to_string_lossy().into_owned()
}

struct CaseInput {
	steps: Vec<Value>,
	env: Env,
	auth_path: String,
	models_path: Option<String>,
	stored: Map<String, Value>,
	allow_model_network: bool,
	refresh_on_create: bool,
	now_ms: f64,
	fetch: Option<Value>,
}

fn parse_case(input: &Value) -> Result<CaseInput, String> {
	let steps: Vec<Value> = match input.get("steps") {
		Some(Value::Array(steps)) => steps.clone(),
		_ => vec![input.clone()],
	};
	let mut env = Env::new();
	if let Some(Value::Object(map)) = input.get("env") {
		for (key, value) in map {
			if let Some(value) = value.as_str() {
				env.insert(key.clone(), value.to_string());
			}
		}
	}
	if input["offline"].as_bool().unwrap_or(false) {
		env.insert("PI_OFFLINE".to_string(), "1".to_string());
	}
	let dir = temp_case_dir();
	let auth_path = format!("{dir}/auth.json");
	std::fs::write(&auth_path, input.get("authJson").and_then(Value::as_str).unwrap_or("{}"))
		.map_err(|e| e.to_string())?;
	let models_path = match input.get("modelsJson").and_then(Value::as_str) {
		Some(content) => {
			let path = format!("{dir}/models.json");
			std::fs::write(&path, content).map_err(|e| e.to_string())?;
			Some(path)
		}
		None => None,
	};
	Ok(CaseInput {
		steps,
		env,
		auth_path,
		models_path,
		stored: input
			.get("stored")
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default(),
		allow_model_network: input["allowModelNetwork"].as_bool().unwrap_or(false),
		refresh_on_create: input["refreshOnCreate"].as_bool().unwrap_or(false),
		now_ms: input["nowMs"].as_f64().unwrap_or(0.0),
		fetch: input.get("fetch").cloned().filter(|v| !v.is_null()),
	})
}

impl CaseInput {
	fn options(&self) -> CreateModelRuntimeOptions<'_> {
		CreateModelRuntimeOptions {
			credentials: None,
			auth_path: Some(self.auth_path.clone()),
			models_path: self.models_path.clone(),
			models_store: None, // replaced by the recording store below
			models_store_path: None,
			allow_model_network: self.allow_model_network,
			model_refresh_timeout_ms: None,
			catalog_base_url: None,
			catalog_hooks: CatalogHooks {
				fetch: scripted_fetch(self.fetch.clone()),
				personalize: None,
			},
			signal: None,
			refresh_on_create: self.refresh_on_create,
			env: Some(self.env.clone()),
			now_ms: Some(self.now_ms),
		}
	}
}

fn run_cases(input: &Value, registry: bool) -> Result<Value, String> {
	let case = parse_case(input)?;
	let mut options = case.options();
	let store = RecordingModelsStore {
		ops: Arc::new(Mutex::new(Vec::new())),
		entries: Arc::new(Mutex::new(case.stored.clone())),
	};
	options.models_store = Some(Box::new(RecordingModelsStore {
		ops: Arc::clone(&store.ops),
		entries: Arc::new(Mutex::new(case.stored.clone())),
	}));
	let mut settle_seen = false;
	let routes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
	let mut results = Vec::new();
	if registry {
		let mut reg = ModelRegistry::new(
			ModelRuntime::create(options).map_err(|e| e.message)?,
		);
		for step in &case.steps {
			if step["settle"].as_bool().unwrap_or(false) {
				settle_seen = true;
			}
			results.push(match run_registry_step(&mut reg, step, &routes) {
				Ok(value) => value,
				Err(error) => error_value(&error),
			});
		}
	} else {
		let mut runtime = ModelRuntime::create(options).map_err(|e| e.message)?;
		for step in &case.steps {
			if step["settle"].as_bool().unwrap_or(false) {
				settle_seen = true;
			}
			results.push(match run_runtime_step(&mut runtime, step, &routes) {
				Ok(value) => value,
				Err(error) => error_value(&error),
			});
		}
	}
	let mut out = Map::new();
	out.insert("results".into(), Value::Array(results));
	if !settle_seen {
		out.insert(
			"storeOps".into(),
			Value::Array(store.ops.lock().unwrap_or_else(|p| p.into_inner()).clone()),
		);
	}
	out.insert(
		"routes".into(),
		Value::Array(routes.lock().unwrap_or_else(|p| p.into_inner()).clone()),
	);
	Ok(json!({ "result": Value::Object(out) }))
}

// ============================================================================
// Serialization helpers
// ============================================================================

fn error_value(error: &JsError) -> Value {
	let mut causes = Vec::new();
	let mut cause = error.cause.as_ref();
	while let Some(current) = cause {
		causes.push(json!(current.message));
		cause = current.cause.as_ref();
	}
	json!({ "error": error.message, "causes": causes })
}

fn step_result(result: Result<Value, JsError>) -> Value {
	match result {
		Ok(value) => json!({ "result": value }),
		Err(error) => error_value(&error),
	}
}

fn step_err(message: impl Into<String>) -> JsError {
	JsError::new(message)
}

fn provider_summary(provider: &Provider) -> Value {
	json!({
		"id": provider.id,
		"name": provider.name,
		"baseUrl": provider.base_url,
		"headers": provider_headers_value(provider.headers.as_ref()),
	})
}

fn check_value(check: Option<&AuthCheck>) -> Value {
	match check {
		None => Value::Null,
		Some(check) => {
			let mut out = Map::new();
			if let Some(source) = &check.source {
				out.insert("source".into(), json!(source));
			}
			out.insert("type".into(), json!(check.auth_type.as_str()));
			Value::Object(out)
		}
	}
}

fn auth_result_value(resolution: Option<&AuthResult>) -> Value {
	match resolution {
		None => Value::Null,
		Some(AuthResult { auth, env, source }) => json!({
			"auth": auth.to_value(),
			"env": env.as_ref().map(|env| json!(env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<Map<String, Value>>())),
			"source": source,
		}),
	}
}

fn models_value(models: &[Model]) -> Value {
	Value::Array(models.iter().map(Model::to_value).collect())
}

fn any_models_value(models: &[AnyModel]) -> Value {
	Value::Array(models.iter().map(AnyModel::to_value).collect())
}

fn model_type_arg(step: &Value) -> Result<ModelType, JsError> {
	match step["type"].as_str() {
		Some("chat") => Ok(ModelType::Chat),
		Some("image") => Ok(ModelType::Image),
		Some("classifier") => Ok(ModelType::Classifier),
		other => Err(step_err(format!("unknown model type: {other:?}"))),
	}
}

fn thinking_level_arg(step: &Value, key: &str) -> ModelThinkingLevel {
	step
		.get(key)
		.and_then(Value::as_str)
		.and_then(ModelThinkingLevel::parse)
		.unwrap_or(ModelThinkingLevel::Off)
}

fn reason_arg(step: &Value) -> ModelRouteReason {
	match step["reason"].as_str() {
		Some("user") => ModelRouteReason::User,
		Some("continuation") => ModelRouteReason::Continuation,
		Some("retry") => ModelRouteReason::Retry,
		_ => ModelRouteReason::Direct,
	}
}

/// Look up a chat model by `{ provider, modelId }` step args.
fn chat_model_ref(runtime: &ModelRuntime, step: &Value) -> Result<Model, JsError> {
	let provider = step["model"]["provider"].as_str().ok_or_else(|| step_err("missing model.provider"))?;
	let model_id = step["model"]["modelId"].as_str().ok_or_else(|| step_err("missing model.modelId"))?;
	runtime
		.get_model(provider, model_id)
		.ok_or_else(|| step_err(format!("model not found: {provider}/{model_id}")))
}

fn refresh_options(step: &Value) -> ModelsRefreshOptions<'static> {
	ModelsRefreshOptions {
		allow_network: step.get("allowNetwork").and_then(Value::as_bool),
		providers: step
			.get("providers")
			.and_then(Value::as_array)
			.map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect()),
		force: step["force"].as_bool().unwrap_or(false),
		signal: None,
	}
}

fn refresh_result_value(result: &pi_providers::pi_ai::models::ModelsRefreshResult) -> Value {
	let errors: Map<String, Value> = result
		.errors
		.iter()
		.map(|(id, error)| (id.clone(), json!(error.message)))
		.collect();
	json!({ "aborted": result.aborted, "errors": Value::Object(errors) })
}

/// A no-op auth interaction (builtin providers define no login flows, so
/// the interaction is never reached before the error).
struct NoopInteraction;

impl AuthInteraction for NoopInteraction {
	fn prompt(&mut self, _prompt: &AuthPrompt) -> Result<String, JsError> {
		Err(JsError::new("login prompt not exercised"))
	}
	fn notify(&mut self, _event: &AuthEvent) {}
}

/// The settle refresh (register/unregister with `settle: true`).
fn settle_refresh(runtime: &mut ModelRuntime) -> Result<(), JsError> {
	runtime.refresh(Some(&ModelsRefreshOptions {
		allow_network: Some(false),
		providers: None,
		force: false,
		signal: None,
	}))?;
	Ok(())
}

// ============================================================================
// Scripted registrations
// ============================================================================

fn dummy_stream() -> pi_providers::pi_ai::provider::StreamFn {
	Box::new(|_model, _context, _options| {
		pi_providers::pi_ai::provider::AssistantMessageEventStream(Value::Null)
	})
}

fn dummy_stream_simple() -> pi_providers::pi_ai::provider::StreamSimpleFn {
	Box::new(|_model, _context, _options| {
		pi_providers::pi_ai::provider::AssistantMessageEventStream(Value::Null)
	})
}

/// A scripted native `Provider` for `registerNativeProvider`.
fn build_native_provider(spec: &Value) -> Result<Provider, JsError> {
	let id = spec["id"].as_str().ok_or_else(|| step_err("missing provider id"))?.to_string();
	let name = spec
		.get("name")
		.and_then(Value::as_str)
		.unwrap_or(id.as_str())
		.to_string();
	let models = spec
		.get("models")
		.and_then(Value::as_array)
		.cloned()
		.unwrap_or_default();
	// TS: `getModels: () => chatOnly(spec.models)` (no type or "chat"),
	// `getAllModels: () => spec.models` (every type, in spec order).
	let chat: Vec<Model> = models
		.iter()
		.filter(|m| {
			m.get("type")
				.and_then(Value::as_str)
				.map(|ty| ty == "chat")
				.unwrap_or(true)
		})
		.filter_map(|m| serde_json::from_value(m.clone()).ok())
		.collect();
	let chat = Arc::new(chat);
	let get_models = {
		let chat = Arc::clone(&chat);
		Box::new(move || chat.to_vec()) as pi_providers::pi_ai::provider::GetModelsFn
	};
	let all_models = Arc::new(models.clone());
	let get_all_models = {
		let all_models = Arc::clone(&all_models);
		Box::new(move || {
			all_models
				.iter()
				.filter_map(|m| serde_json::from_value::<AnyModel>(m.clone()).ok())
				.collect::<Vec<AnyModel>>()
		}) as pi_providers::pi_ai::provider::GetAllModelsFn
	};
	let mut provider = Provider::new(
		id,
		name,
		ProviderAuth { api_key: None, oauth: None },
		get_models,
		dummy_stream(),
		dummy_stream_simple(),
	)
	.with_get_all_models(get_all_models)
	.with_base_url(spec.get("baseUrl").and_then(Value::as_str).map(str::to_string))
	.with_headers(spec.get("headers").and_then(Value::as_object).map(|headers| {
		headers
			.iter()
			.map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
			.collect::<ProviderHeaders>()
	}));
	if let Some((_api, impl_spec)) = spec.get("images").and_then(Value::as_object).and_then(|m| m.iter().next()) {
		let marker = impl_spec.get("marker").and_then(Value::as_str).unwrap_or("images").to_string();
		provider = provider.with_generate_images(Box::new(
			move |_model, _context, _options| {
				pi_providers::pi_ai::provider::AssistantImages(json!({ "marker": marker }))
			},
		));
	}
	if let Some((_api, impl_spec)) =
		spec.get("classifiers").and_then(Value::as_object).and_then(|m| m.iter().next())
	{
		let marker = impl_spec.get("marker").and_then(Value::as_str).unwrap_or("classify").to_string();
		provider = provider.with_classify(Box::new(
			move |_model, _context, _options| {
				pi_providers::pi_ai::provider::ClassifierResult(json!({ "marker": marker }))
			},
		));
	}
	Ok(provider)
}

/// A scripted `ProviderConfigInput` for `registerProvider`.
fn build_extension_config(spec: &Value) -> Result<ProviderConfigInput, JsError> {
	// TS `buildExtensionConfig` reads `spec.name` first — a missing config
	// throws the same TypeError the TS harness surfaces.
	let object = spec.as_object().ok_or_else(|| {
		step_err(match spec {
			Value::Null => "Cannot read properties of null (reading 'name')",
			_ => "Cannot read properties of undefined (reading 'name')",
		})
	})?;
	let get = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
	let headers = object.get("headers").and_then(Value::as_object).map(|headers| {
		headers
			.iter()
			.filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
			.collect::<Vec<(String, String)>>()
	});
	let models = object
		.get("models")
		.and_then(Value::as_array)
		.map(|models| {
			models
				.iter()
				.map(|m| m.as_object().cloned().ok_or_else(|| step_err("invalid config model")))
				.collect::<Result<Vec<Map<String, Value>>, JsError>>()
		})
		.transpose()?;

	let oauth = match object.get("oauth").filter(|v| !v.is_null()) {
		None => None,
		Some(oauth) => {
			let oauth = oauth.as_object().ok_or_else(|| step_err("oauth must be an object"))?;
			let name = oauth
				.get("name")
				.and_then(Value::as_str)
				.ok_or_else(|| step_err("missing oauth name"))?
				.to_string();
			let get_api_key_spec = oauth.get("getApiKey").cloned().unwrap_or(Value::Null);
			let get_api_key = move |credential: &pi_providers::pi_ai::credential::OAuthCredential| -> String {
				match &get_api_key_spec {
					Value::String(key) => key.clone(),
					spec => {
						let field = spec.get("field").and_then(Value::as_str).unwrap_or("access");
						credential
							.to_value()
							.get(field)
							.and_then(Value::as_str)
							.unwrap_or_default()
							.to_string()
					}
				}
			};
			Some(Arc::new(ExtensionOAuthConfig {
				name,
				is_subscription: oauth.get("isSubscription").and_then(Value::as_bool),
				uses_callback_server: None,
				login: Arc::new(|_callbacks: &Value| Err(JsError::new("login not exercised"))),
				refresh_token: Arc::new(|_credentials| Err(JsError::new("refreshToken not exercised"))),
				get_api_key: Arc::new(get_api_key),
				modify_models: None,
			}))
		}
	};

	let refresh_models = match object.get("refreshModels").filter(|v| !v.is_null()) {
		None => None,
		Some(spec) => {
			let spec = spec.clone();
			Some(Arc::new(
				move |_context: &mut pi_providers::pi_ai::models::RefreshModelsContext<'_>| {
					if let Some(error) = spec.get("error").and_then(Value::as_str) {
						return Err(JsError::new(error.to_string()));
					}
					Ok(spec
						.get("models")
						.and_then(Value::as_array)
						.map(|models| {
							models
								.iter()
								.map(|m| m.as_object().cloned().unwrap_or_default())
								.collect()
						})
						.unwrap_or_default())
				},
			) as pi_providers::provider_composer::ExtensionRefreshModelsFn)
		}
	};

	let images = object.get("images").and_then(Value::as_object).map(|images| {
		images
			.iter()
			.map(|(api, impl_spec)| {
				let marker = impl_spec.get("marker").and_then(Value::as_str).unwrap_or("images").to_string();
				(
					api.clone(),
					Arc::new(ProviderImages {
						generate_images: Arc::new(move |_model, _context, _options| {
							pi_providers::pi_ai::provider::AssistantImages(json!({ "marker": marker }))
						}),
					}),
				)
			})
			.collect()
	});
	let classifiers = object.get("classifiers").and_then(Value::as_object).map(|classifiers| {
		classifiers
			.iter()
			.map(|(api, impl_spec)| {
				let marker = impl_spec.get("marker").and_then(Value::as_str).unwrap_or("classify").to_string();
				(
					api.clone(),
					Arc::new(ProviderClassifier {
						classify: Arc::new(move |_model, _context, _options| {
							pi_providers::pi_ai::provider::ClassifierResult(json!({ "marker": marker }))
						}),
					}),
				)
			})
			.collect()
	});

	Ok(ProviderConfigInput {
		name: get("name"),
		base_url: get("baseUrl"),
		api_key: get("apiKey"),
		api: get("api"),
		stream_simple: None,
		images,
		classifiers,
		headers,
		auth_header: object.get("authHeader").and_then(Value::as_bool),
		oauth,
		models,
		refresh_models,
	})
}

fn config_summary(config: &ProviderConfigInput) -> Value {
	let mut out = Map::new();
	if let Some(name) = &config.name {
		out.insert("name".into(), json!(name));
	}
	if let Some(base_url) = &config.base_url {
		out.insert("baseUrl".into(), json!(base_url));
	}
	if let Some(api_key) = &config.api_key {
		out.insert("apiKey".into(), json!(api_key));
	}
	if let Some(api) = &config.api {
		out.insert("api".into(), json!(api));
	}
	if let Some(headers) = &config.headers {
		out.insert(
			"headers".into(),
			Value::Object(headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
		);
	}
	if let Some(auth_header) = config.auth_header {
		out.insert("authHeader".into(), json!(auth_header));
	}
	if let Some(models) = &config.models {
		out.insert("models".into(), Value::Array(models.iter().map(|m| Value::Object(m.clone())).collect()));
	}
	out.insert(
		"oauth".into(),
		match &config.oauth {
			None => Value::Null,
			Some(oauth) => json!({
				"name": oauth.name,
				"isSubscription": oauth.is_subscription,
			}),
		},
	);
	out.insert("hasRefreshModels".into(), json!(config.refresh_models.is_some()));
	out.insert(
		"images".into(),
		match &config.images {
			None => Value::Null,
			Some(images) => Value::Array(images.keys().map(|k| json!(k)).collect()),
		},
	);
	out.insert(
		"classifiers".into(),
		match &config.classifiers {
			None => Value::Null,
			Some(classifiers) => Value::Array(classifiers.keys().map(|k| json!(k)).collect()),
		},
	);
	Value::Object(out)
}

/// A scripted `VirtualModelDefinition` (fixed route; `echo` records the
/// request the router received).
fn build_virtual_model(
	spec: &Value,
	routes: &Arc<Mutex<Vec<Value>>>,
) -> Result<VirtualModelDefinition, JsError> {
	let provider = spec["provider"].as_str().ok_or_else(|| step_err("missing definition.provider"))?.to_string();
	let id = spec["id"].as_str().ok_or_else(|| step_err("missing definition.id"))?.to_string();
	let name = spec
		.get("name")
		.and_then(Value::as_str)
		.unwrap_or(id.as_str())
		.to_string();
	let route_spec = spec.get("route").cloned().unwrap_or(Value::Null);
	let route: RouteFn = {
		let routes = Arc::clone(routes);
		Box::new(move |request: &ModelRouteRequest| {
			if route_spec.get("echo").and_then(Value::as_bool).unwrap_or(false) {
				routes.lock().unwrap_or_else(|p| p.into_inner()).push(route_request_value(request));
			}
			let provider = route_spec
				.get("provider")
				.and_then(Value::as_str)
				.ok_or_else(|| step_err("missing route.provider"))?
				.to_string();
			let model_id = route_spec
				.get("model")
				.and_then(Value::as_str)
				.ok_or_else(|| step_err("missing route.model"))?
				.to_string();
			Ok(ModelRoute {
				model: chat_stand_in(&provider, &model_id),
				thinking_level: route_spec
					.get("thinkingLevel")
					.and_then(Value::as_str)
					.and_then(ModelThinkingLevel::parse)
					.unwrap_or(ModelThinkingLevel::Off),
				state: route_spec.get("state").cloned().filter(|v| !v.is_null()),
			})
		})
	};
	Ok(VirtualModelDefinition {
		provider,
		id,
		name,
		thinking_levels: spec.get("thinkingLevels").and_then(Value::as_array).map(|levels| {
			levels
				.iter()
				.filter_map(Value::as_str)
				.filter_map(ModelThinkingLevel::parse)
				.collect()
		}),
		context_window: spec.get("contextWindow").and_then(Value::as_f64),
		max_tokens: spec.get("maxTokens").and_then(Value::as_f64),
		input: None,
		route,
	})
}

/// A minimal chat model standing in for a routed target (only provider/id
/// are read from the route's model).
fn chat_stand_in(provider: &str, id: &str) -> Model {
	serde_json::from_value(json!({
		"id": id, "name": id, "api": "a", "provider": provider, "baseUrl": "",
		"input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
		"reasoning": false, "contextWindow": 0, "maxTokens": 0, "headers": null
	}))
	.expect("stand-in chat model")
}

fn route_request_value(request: &ModelRouteRequest) -> Value {
	json!({
		"model": { "provider": request.model.provider, "id": request.model.id },
		"thinkingLevel": request.thinking_level.as_str(),
		"reason": route_reason_str(&request.reason),
		"previous": request.previous.as_ref().map(|(model, level)| json!({
			"model": { "provider": model.provider, "id": model.id },
			"thinkingLevel": level.map(|l| l.as_str()),
		})),
		"failed": request.failed.as_ref().map(|failed| json!({
			"model": { "provider": failed.model.provider, "id": failed.model.id },
			"thinkingLevel": failed.thinking_level.map(|l| l.as_str()),
			"message": failed.message,
		})),
		"state": request.state,
		"messages": request.messages,
	})
}

fn route_reason_str(reason: &ModelRouteReason) -> &'static str {
	match reason {
		ModelRouteReason::User => "user",
		ModelRouteReason::Continuation => "continuation",
		ModelRouteReason::Retry => "retry",
		ModelRouteReason::Direct => "direct",
	}
}

// ============================================================================
// model-runtime steps
// ============================================================================

fn run_runtime_step(
	runtime: &mut ModelRuntime,
	step: &Value,
	routes: &Arc<Mutex<Vec<Value>>>,
) -> Result<Value, JsError> {
	let fn_name = step["fn"].as_str().ok_or_else(|| step_err("missing fn"))?;
	let result: Result<Value, JsError> = match fn_name {
		"getProviders" => Ok(Value::Array(
			runtime.get_providers().iter().map(|p| provider_summary(p)).collect(),
		)),
		"getModels" => Ok(models_value(&runtime.get_models(
			step.get("providerId").and_then(Value::as_str),
		))),
		"getModel" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let model_id = step["modelId"].as_str().ok_or("missing modelId")?;
			Ok(runtime
				.get_model(provider, model_id)
				.map(|m| m.to_value())
				.unwrap_or(Value::Null))
		}
		"getAllModels" => Ok(any_models_value(&runtime.get_all_models(
			step.get("providerId").and_then(Value::as_str),
		))),
		"getModelsOfType" => Ok(any_models_value(&runtime.get_models_of_type(
			&model_type_arg(step)?,
			step.get("providerId").and_then(Value::as_str),
		))),
		"getModelOfType" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let model_id = step["modelId"].as_str().ok_or("missing modelId")?;
			Ok(runtime
				.get_model_of_type(&model_type_arg(step)?, provider, model_id)
				.map(|m| m.to_value())
				.unwrap_or(Value::Null))
		}
		"getError" => Ok(runtime.get_error().map(Value::String).unwrap_or(Value::Null)),
		"getAvailableSnapshot" => Ok(models_value(runtime.get_available_snapshot())),
		"getAvailable" => {
			let provider_id = step.get("providerId").and_then(Value::as_str);
			runtime
				.get_available(provider_id, None)
				.map(|models| models_value(&models))
		}
		"checkAuth" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			runtime
				.check_auth(provider, None)
				.map(|check| check_value(check.as_ref()))
		}
		"hasConfiguredAuth" => Ok(json!(runtime
			.has_configured_auth(step["providerId"].as_str().ok_or("missing providerId")?))),
		"isUsingOAuth" => Ok(json!(runtime
			.is_using_oauth(step["providerId"].as_str().ok_or("missing providerId")?))),
		"isUsingSubscription" => Ok(json!(runtime
			.is_using_subscription(step["providerId"].as_str().ok_or("missing providerId")?))),
		"getProviderAuthStatus" => Ok(runtime
			.get_provider_auth_status(step["providerId"].as_str().ok_or("missing providerId")?)
			.to_value()),
		"getRegisteredProviderIds" => Ok(json!(runtime.get_registered_provider_ids())),
		"getRegisteredProviderConfig" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			Ok(runtime
				.get_registered_provider_config(provider)
				.map(config_summary)
				.unwrap_or(Value::Null))
		}
		"getRegisteredNativeProvider" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			Ok(runtime
				.get_registered_native_provider(provider)
				.map(provider_summary)
				.unwrap_or(Value::Null))
		}
		"getAuth" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			runtime
				.get_auth_for_provider(provider, None)
				.map(|resolution| auth_result_value(resolution.as_ref()))
		}
		"getAuthModel" => {
			let model = chat_model_ref(runtime, step)?;
			runtime
				.get_auth_for_model(&AnyModel::Chat(model), None)
				.map(|resolution| auth_result_value(resolution.as_ref()))
		}
		"getCompatibilityRequestConfig" => {
			let model = chat_model_ref(runtime, step)?;
			runtime
				.get_compatibility_request_config(&AnyModel::Chat(model))
				.map(|config| {
					json!({
						"headers": config
							.headers
							.as_ref()
							.map(|pairs| Value::Object(
								pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect()
							))
							.unwrap_or(Value::Null),
						"authHeader": config.auth_header,
					})
				})
		}
		"setRuntimeApiKey" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let api_key = step["apiKey"].as_str().ok_or("missing apiKey")?;
			runtime.set_runtime_api_key(provider, api_key, None).map(|_| Value::Null)
		}
		"removeRuntimeApiKey" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			runtime.remove_runtime_api_key(provider, None).map(|_| Value::Null)
		}
		"listCredentials" => runtime.list_credentials(None).map(|entries| {
			Value::Array(entries.iter().map(credential_info_value).collect())
		}),
		"login" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let ty = match step["type"].as_str() {
				Some("oauth") => AuthType::OAuth,
				_ => AuthType::ApiKey,
			};
			let mut interaction = NoopInteraction;
			runtime
				.login(provider, ty, &mut interaction, None)
				.map(|credential| credential.to_value())
		}
		"logout" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			runtime.logout(provider, None).map(|_| Value::Null)
		}
		"registerProvider" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			// TS: `buildExtensionConfig(step.config)` throws on a missing
			// config before the runtime sees it.
			let config = build_extension_config(step.get("config").ok_or_else(|| {
				step_err("Cannot read properties of undefined (reading 'name')")
			})?)?;
			let outcome = runtime
				.register_provider_checked(provider, config)
				.map(|_| Value::Null);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(runtime)?;
			}
			outcome
		}
		"registerNativeProvider" => {
			let spec = step.get("provider").ok_or_else(|| step_err("missing provider"))?;
			let provider = build_native_provider(spec)?;
			let outcome = runtime
				.register_native_provider_checked(provider)
				.map(|_| Value::Null);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(runtime)?;
			}
			outcome
		}
		"unregisterProvider" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			runtime.unregister_provider(provider);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(runtime)?;
			}
			Ok(Value::Null)
		}
		"registerVirtualModel" => {
			let spec = step
				.get("definition")
				.ok_or_else(|| step_err("missing definition"))?;
			let outcome = runtime
				.register_virtual_model_checked(build_virtual_model(spec, routes)?)
				.map(|_| Value::Null);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(runtime)?;
			}
			outcome
		}
		"unregisterVirtualModel" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let id = step["modelId"].as_str().ok_or("missing modelId")?;
			runtime.unregister_virtual_model(provider, id);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(runtime)?;
			}
			Ok(Value::Null)
		}
		"resolveModel" => {
			let model = chat_model_ref(runtime, step)?;
			let messages = step
				.get("messages")
				.and_then(Value::as_array)
				.cloned()
				.unwrap_or_default();
			let route = runtime.resolve_model(
				&model,
				&messages,
				reason_arg(step),
				thinking_level_arg(step, "thinkingLevel"),
				step.get("state").cloned().filter(|v| !v.is_null()),
				step.get("failed").filter(|v| !v.is_null()),
				None,
			)?;
			Ok(json!({
				"model": route.model.to_value(),
				"thinkingLevel": route.thinking_level.as_str(),
				"state": route.state,
			}))
		}
		"refresh" => {
			let options = refresh_options(step);
			runtime.refresh(Some(&options)).map(|result| refresh_result_value(&result))
		}
		"generateImages" => {
			let model = typed_model_ref(runtime, step, ModelType::Image)?;
			let result = runtime.generate_images(
				&match model {
					AnyModel::Image(m) => m,
					_ => return Err(JsError::from("expected an image model")),
				},
				&pi_providers::pi_ai::provider::ImagesContext(
					step.get("context").cloned().unwrap_or(json!({})),
				),
				None,
			);
			Ok(normalize_timestamp(result.0))
		}
		"classify" => {
			let model = typed_model_ref(runtime, step, ModelType::Classifier)?;
			let result = runtime.classify(
				&match model {
					AnyModel::Classifier(m) => m,
					_ => return Err(JsError::from("expected a classifier model")),
				},
				&pi_providers::pi_ai::provider::ClassifierContext(
					step.get("context").cloned().unwrap_or(json!({})),
				),
				None,
			);
			Ok(normalize_timestamp(result.0))
		}
		other => return Err(JsError::from(format!("unknown model-runtime fn: {other}"))),
	};
	Ok(step_result(result))
}

fn credential_info_value(info: &CredentialInfo) -> Value {
	json!({ "providerId": info.provider_id, "type": info.credential_type })
}

/// Timestamps in image/classifier results are normalized to 0 on both sides.
fn normalize_timestamp(mut value: Value) -> Value {
	if let Some(map) = value.as_object_mut() {
		if map.get("timestamp").is_some() {
			map.insert("timestamp".to_string(), json!(0));
		}
	}
	value
}

fn typed_model_ref(
	runtime: &ModelRuntime,
	step: &Value,
	ty: ModelType,
) -> Result<AnyModel, String> {
	let provider = step["model"]["provider"]
		.as_str()
		.ok_or("missing model.provider")?;
	let model_id = step["model"]["modelId"].as_str().ok_or("missing model.modelId")?;
	runtime
		.get_model_of_type(&ty, provider, model_id)
		.ok_or_else(|| format!("model not found: {provider}/{model_id}"))
}

// ============================================================================
// model-registry steps
// ============================================================================

fn run_registry_step(
	registry: &mut ModelRegistry,
	step: &Value,
	routes: &Arc<Mutex<Vec<Value>>>,
) -> Result<Value, JsError> {
	let fn_name = step["fn"].as_str().ok_or_else(|| step_err("missing fn"))?;
	let result: Result<Value, JsError> = match fn_name {
		// Facade-only reads.
		"getAll" => Ok(models_value(&registry.get_all())),
		"getAvailable" => Ok(models_value(registry.get_available())),
		"find" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let model_id = step["modelId"].as_str().ok_or("missing modelId")?;
			Ok(registry
				.find(provider, model_id)
				.map(|m| m.to_value())
				.unwrap_or(Value::Null))
		}
		"findOfType" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let model_id = step["modelId"].as_str().ok_or("missing modelId")?;
			Ok(registry
				.find_of_type(&model_type_arg(step)?, provider, model_id)
				.map(|m| m.to_value())
				.unwrap_or(Value::Null))
		}
		"hasConfiguredAuth" => {
			let model = chat_model_ref(registry.runtime_mut(), step)?;
			Ok(json!(registry.has_configured_auth(&model)))
		}
		"getApiKeyAndHeaders" => {
			let model = chat_model_ref(registry.runtime_mut(), step)?;
			Ok(resolved_auth_value(&registry.get_api_key_and_headers(&model)))
		}
		"getProviderDisplayName" => Ok(json!(
			registry.get_provider_display_name(step["providerId"].as_str().ok_or("missing providerId")?)
		)),
		"getProviderAuth" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			registry
				.get_provider_auth(provider)
				.map(|resolution| auth_result_value(resolution.as_ref()))
		}
		"getApiKeyForProvider" => Ok(registry
			.get_api_key_for_provider(step["providerId"].as_str().ok_or("missing providerId")?)
			.map(Value::String)
			.unwrap_or(Value::Null)),
		"isUsingOAuth" => {
			let model = chat_model_ref(registry.runtime_mut(), step)?;
			Ok(json!(registry.is_using_oauth(&model)))
		}
		"getModelsOfType" => Ok(any_models_value(&registry.get_models_of_type(
			&model_type_arg(step)?,
			step.get("providerId").and_then(Value::as_str),
		))),
		"getAvailableOfType" => registry
			.get_available_of_type(
				&model_type_arg(step)?,
				step.get("providerId").and_then(Value::as_str),
				None,
			)
			.map(|models| any_models_value(&models)),
		"getModelOfType" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let model_id = step["modelId"].as_str().ok_or("missing modelId")?;
			Ok(registry
				.get_model_of_type(&model_type_arg(step)?, provider, model_id)
				.map(|m| m.to_value())
				.unwrap_or(Value::Null))
		}
		"getProvider" => Ok(registry
			.get_provider(step["providerId"].as_str().ok_or("missing providerId")?)
			.map(|p| provider_summary(&p))
			.unwrap_or(Value::Null)),
		// Delegating surface (same semantics as the runtime steps).
		"getModels" => Ok(models_value(&registry.runtime_mut().get_models(
			step.get("providerId").and_then(Value::as_str),
		))),
		"getModel" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let model_id = step["modelId"].as_str().ok_or("missing modelId")?;
			Ok(registry.runtime_mut()
				.get_model(provider, model_id)
				.map(|m| m.to_value())
				.unwrap_or(Value::Null))
		}
		"getError" => Ok(registry.get_error().map(Value::String).unwrap_or(Value::Null)),
		"getAvailableSnapshot" => Ok(models_value(registry.runtime_mut().get_available_snapshot())),
		"isUsingSubscription" => Ok(json!(registry
			.runtime_mut()
			.is_using_subscription(step["providerId"].as_str().ok_or("missing providerId")?))),
		"getAuth" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			registry
				.runtime_mut()
				.get_auth_for_provider(provider, None)
				.map(|resolution| auth_result_value(resolution.as_ref()))
		}
		"getAuthModel" => {
			let model = chat_model_ref(registry.runtime_mut(), step)?;
			registry
				.runtime_mut()
				.get_auth_for_model(&AnyModel::Chat(model), None)
				.map(|resolution| auth_result_value(resolution.as_ref()))
		}
		"getCompatibilityRequestConfig" => {
			let model = chat_model_ref(registry.runtime_mut(), step)?;
			registry
				.runtime_mut()
				.get_compatibility_request_config(&AnyModel::Chat(model))
				.map(|config| {
					json!({
						"headers": config
							.headers
							.as_ref()
							.map(|pairs| Value::Object(
								pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect()
							))
							.unwrap_or(Value::Null),
						"authHeader": config.auth_header,
					})
				})
		}
		"setRuntimeApiKey" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let api_key = step["apiKey"].as_str().ok_or("missing apiKey")?;
			registry
				.runtime_mut()
				.set_runtime_api_key(provider, api_key, None)
				.map(|_| Value::Null)
		}
		"removeRuntimeApiKey" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			registry
				.runtime_mut()
				.remove_runtime_api_key(provider, None)
				.map(|_| Value::Null)
		}
		"listCredentials" => registry.runtime_mut().list_credentials(None).map(|entries| {
			Value::Array(entries.iter().map(credential_info_value).collect())
		}),
		"login" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let ty = match step["type"].as_str() {
				Some("oauth") => AuthType::OAuth,
				_ => AuthType::ApiKey,
			};
			let mut interaction = NoopInteraction;
			registry
				.runtime_mut()
				.login(provider, ty, &mut interaction, None)
				.map(|credential| credential.to_value())
		}
		"logout" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			registry
				.runtime_mut()
				.logout(provider, None)
				.map(|_| Value::Null)
		}
		"checkAuth" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			registry.runtime_mut().check_auth(provider, None).map(|check| check_value(check.as_ref()))
		}
		"getProviderAuthStatus" => Ok(registry
			.get_provider_auth_status(step["providerId"].as_str().ok_or("missing providerId")?)
			.to_value()),
		"getRegisteredProviderIds" => Ok(json!(registry.get_registered_provider_ids())),
		"getRegisteredProviderConfig" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			Ok(registry
				.get_registered_provider_config(provider)
				.map(config_summary)
				.unwrap_or(Value::Null))
		}
		"getRegisteredNativeProvider" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			Ok(registry
				.get_registered_native_provider(provider)
				.map(provider_summary)
				.unwrap_or(Value::Null))
		}
		"registerProvider" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			// TS: `buildExtensionConfig(step.config)` throws on a missing
			// config before the registry sees it.
			let config = build_extension_config(step.get("config").ok_or_else(|| {
				step_err("Cannot read properties of undefined (reading 'name')")
			})?)?;
			let outcome = registry
				.register_provider_entry(pi_providers::model_registry::ProviderOrName::ByName {
					name: provider,
					config: Some(config),
				})
				.map(|_| Value::Null);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(registry.runtime_mut())?;
			}
			outcome
		}
		"registerNativeProvider" => {
			let spec = step.get("provider").ok_or_else(|| step_err("missing provider"))?;
			let provider = build_native_provider(spec)?;
			let outcome = registry
				.register_provider_entry(pi_providers::model_registry::ProviderOrName::Provider(
					provider,
				))
				.map(|_| Value::Null);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(registry.runtime_mut())?;
			}
			outcome
		}
		"unregisterProvider" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			registry.unregister_provider(provider);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(registry.runtime_mut())?;
			}
			Ok(Value::Null)
		}
		"registerVirtualModel" => {
			let spec = step.get("definition").ok_or_else(|| step_err("missing definition"))?;
			let outcome = (|| {
				registry.register_virtual_model(build_virtual_model(spec, routes)?);
				Ok(Value::Null)
			})();
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(registry.runtime_mut())?;
			}
			outcome
		}
		"unregisterVirtualModel" => {
			let provider = step["providerId"].as_str().ok_or("missing providerId")?;
			let id = step["modelId"].as_str().ok_or("missing modelId")?;
			registry.unregister_virtual_model(provider, id);
			if step["settle"].as_bool().unwrap_or(false) {
				settle_refresh(registry.runtime_mut())?;
			}
			Ok(Value::Null)
		}
		"resolveModel" => {
			let model = chat_model_ref(registry.runtime_mut(), step)?;
			let messages = step
				.get("messages")
				.and_then(Value::as_array)
				.cloned()
				.unwrap_or_default();
			let route = registry.runtime_mut().resolve_model(
				&model,
				&messages,
				reason_arg(step),
				thinking_level_arg(step, "thinkingLevel"),
				step.get("state").cloned().filter(|v| !v.is_null()),
				step.get("failed").filter(|v| !v.is_null()),
				None,
			)?;
			Ok(json!({
				"model": route.model.to_value(),
				"thinkingLevel": route.thinking_level.as_str(),
				"state": route.state,
			}))
		}
		"refresh" => {
			let options = refresh_options(step);
			registry.refresh(Some(&options)).map(|result| refresh_result_value(&result))
		}
		"generateImages" => {
			let model = typed_model_ref(registry.runtime_mut(), step, ModelType::Image)?;
			let result = registry.generate_images(
				&match model {
					AnyModel::Image(m) => m,
					_ => return Err(JsError::from("expected an image model")),
				},
				&pi_providers::pi_ai::provider::ImagesContext(
					step.get("context").cloned().unwrap_or(json!({})),
				),
				None,
			);
			Ok(normalize_timestamp(result.0))
		}
		"classify" => {
			let model = typed_model_ref(registry.runtime_mut(), step, ModelType::Classifier)?;
			let result = registry.classify(
				&match model {
					AnyModel::Classifier(m) => m,
					_ => return Err(JsError::from("expected a classifier model")),
				},
				&pi_providers::pi_ai::provider::ClassifierContext(
					step.get("context").cloned().unwrap_or(json!({})),
				),
				None,
			);
			Ok(normalize_timestamp(result.0))
		}
		other => return Err(JsError::from(format!("unknown model-registry fn: {other}"))),
	};
	Ok(step_result(result))
}

fn resolved_auth_value(auth: &pi_providers::model_registry::ResolvedRequestAuth) -> Value {
	let mut out = Map::new();
	out.insert("ok".into(), json!(auth.ok));
	if auth.ok {
		if let Some(api_key) = &auth.api_key {
			out.insert("apiKey".into(), json!(api_key));
		}
		if let Some(headers) = &auth.headers {
			out.insert(
				"headers".into(),
				Value::Object(headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
			);
		}
		if let Some(base_url) = &auth.base_url {
			out.insert("baseUrl".into(), json!(base_url));
		}
		if let Some(env) = &auth.env {
			out.insert(
				"env".into(),
				Value::Object(env.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
			);
		}
	} else if let Some(error) = &auth.error {
		out.insert("error".into(), json!(error));
	}
	Value::Object(out)
}
