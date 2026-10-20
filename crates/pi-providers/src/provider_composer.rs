//! `provider-composer.ts` port — composes built-in, models.json and
//! extension provider layers without reading credentials.
//!
//! Ported exports: `validateExtensionProvider`, `composeModelProvider`,
//! `resolveConfiguredModelHeaders`, `resolveCompatibilityRequestConfig`,
//! `configuredRequestAuthStatus`, `clearApiKeyCache` (+ the config types).
//!
//! Deviations (documented): the TS module-level `commandResultCache` lives
//! behind [`set_command_runner`]/[`clear_api_key_cache`] here (ports never
//! read the ambient environment; the harness injects the shell runner);
//! `configuredRequestAuthStatus`/`resolveCompatibilityRequestConfig` read
//! `process.env` in TS via the env-less `isConfigValueConfigured` /
//! `resolveHeadersOrThrow` overloads — here the env is an explicit argument
//! the harness supplies.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Map, Value};


use crate::config_value::{
	CommandRunner, Env, get_config_value_env_var_names, is_command_config_value,
	is_config_value_configured, resolve_config_value_or_throw, resolve_headers_or_throw,
};
use crate::error::JsError;
use crate::model_config::ModelConfig;
use crate::pi_ai::compat;
use crate::pi_ai::credential::{
	ApiKeyAuth, ApiKeyCredential, AuthCheck, AuthContext, AuthResult, AuthType, Credential,
	ModelAuth, OAuthAuth, OAuthCredential, OAuthCredentials, ProviderAuth,
};
use crate::pi_ai::model::{AnyModel, ClassifierModel, ImageModel, Model, ProviderHeaders};
use crate::pi_ai::models::{ModelsPublication, Persist, RefreshModelsContext};
use crate::pi_ai::provider::{
	AssistantImages, AssistantMessageEventStream, ClassifierContext, ClassifierOptions,
	ClassifierResult, DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, ImagesContext, ImagesOptions, Provider, SimpleStreamOptions, StreamOptions,
	TranscriptContext,
};

// ============================================================================
// Config types
// ============================================================================

/// TS `ExtensionOAuthConfig` (function seams injected by the host).
pub struct ExtensionOAuthConfig {
	pub name: String,
	pub is_subscription: Option<bool>,
	#[allow(dead_code)]
	pub uses_callback_server: Option<bool>,
	/// `login(callbacks)` — seam (not parity-exercised; the callback record
	/// rides as JSON).
	#[allow(clippy::type_complexity)]
	pub login: Arc<dyn Fn(&Value) -> Result<OAuthCredentials, JsError> + Send + Sync>,
	/// `refreshToken(credentials, signal)` — seam.
	#[allow(clippy::type_complexity)]
	pub refresh_token: Arc<dyn Fn(&OAuthCredentials) -> Result<OAuthCredentials, JsError> + Send + Sync>,
	/// `getApiKey(credentials)` — receives the stored credential
	/// (`OAuthCredentials` in TS; structurally the `OAuthCredential` it is
	/// handed by `toAuth`).
	pub get_api_key: Arc<dyn Fn(&OAuthCredential) -> String + Send + Sync>,
	/// `modifyModels(models, credentials)` — chat-only hook. Models are the
	/// raw JSON maps (TS `Model` objects are plain records; the spread
	/// semantics need arbitrary key sets).
	#[allow(clippy::type_complexity)]
	pub modify_models: Option<
		Arc<dyn Fn(&[JsonMap], &OAuthCredential) -> Vec<JsonMap> + Send + Sync>,
	>,
}

/// TS `ProviderImages`.
pub struct ProviderImages {
	#[allow(clippy::type_complexity)]
	pub generate_images:
		Arc<dyn Fn(&ImageModel, &ImagesContext, Option<&ImagesOptions>) -> AssistantImages + Send + Sync>,
}

/// TS `ProviderClassifier`.
pub struct ProviderClassifier {
	#[allow(clippy::type_complexity)]
	pub classify: Arc<
		dyn Fn(&ClassifierModel, &ClassifierContext, Option<&ClassifierOptions>) -> ClassifierResult
			+ Send
			+ Sync,
	>,
}

pub type ExtensionStreamSimpleFn = Arc<
	dyn Fn(&Model, &TranscriptContext, Option<&SimpleStreamOptions>) -> AssistantMessageEventStream
		+ Send
		+ Sync,
>;

pub type ExtensionRefreshModelsFn = Arc<
	dyn for<'a, 'b> Fn(&'a mut RefreshModelsContext<'b>) -> Result<Vec<Map<String, Value>>, JsError>
		+ Send
		+ Sync,
>;

/// TS `ProviderConfigInput` — the extension registerProvider API input.
/// `models` entries are the raw JSON `ProviderModelConfig` maps (the TS
/// spread semantics need arbitrary key sets).
#[derive(Clone)]
pub struct ProviderConfigInput {
	pub name: Option<String>,
	pub base_url: Option<String>,
	pub api_key: Option<String>,
	pub api: Option<String>,
	pub stream_simple: Option<ExtensionStreamSimpleFn>,
	pub images: Option<std::collections::BTreeMap<String, Arc<ProviderImages>>>,
	pub classifiers: Option<std::collections::BTreeMap<String, Arc<ProviderClassifier>>>,
	/// Raw header pairs in JSON key order.
	pub headers: Option<Vec<(String, String)>>,
	pub auth_header: Option<bool>,
	pub oauth: Option<Arc<ExtensionOAuthConfig>>,
	pub models: Option<Vec<Map<String, Value>>>,
	pub refresh_models: Option<ExtensionRefreshModelsFn>,
}

/// TS `AuthStatus`.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthStatus {
	pub configured: bool,
	pub source: Option<String>,
	pub label: Option<String>,
}

impl AuthStatus {
	pub fn to_value(&self) -> Value {
		let mut out = Map::new();
		out.insert("configured".into(), json!(self.configured));
		if let Some(source) = &self.source {
			out.insert("source".into(), json!(source));
		}
		if let Some(label) = &self.label {
			out.insert("label".into(), json!(label));
		}
		Value::Object(out)
	}
}

/// TS `CompatibilityRequestConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompatibilityRequestConfig {
	pub headers: Option<Vec<(String, String)>>,
	pub auth_header: bool,
}

/// TS `clearApiKeyCache` (= `clearConfigValueCache`).
pub fn clear_api_key_cache() {
	global_cache().lock().unwrap().results.clear();
}

// ============================================================================
// Command cache (TS module-level commandResultCache)
// ============================================================================

struct CommandCache {
	runner: Option<Arc<dyn CommandRunner + Send + Sync>>,
	results: HashMap<String, Option<String>>,
}

fn global_cache() -> &'static Mutex<CommandCache> {
	static CACHE: OnceLock<Mutex<CommandCache>> = OnceLock::new();
	CACHE.get_or_init(|| Mutex::new(CommandCache { runner: None, results: HashMap::new() }))
}

/// Install the shell runner the cached resolutions execute through
/// (the harness owns environment concerns; ports never read `std::env`).
pub fn set_command_runner(runner: Arc<dyn CommandRunner + Send + Sync>) {
	global_cache().lock().unwrap().runner = Some(runner);
}

fn cached_resolve_command(config: &str) -> Option<String> {
	let mut cache = global_cache().lock().unwrap();
	if let Some(hit) = cache.results.get(config) {
		return hit.clone();
	}
	let result = cache.runner.as_ref().and_then(|runner| runner.run(&config[1..]));
	cache.results.insert(config.to_string(), result.clone());
	result
}

/// `resolveConfigValueOrThrow` over the process-lifetime command cache.
fn resolve_or_throw(config: &str, description: &str, env: Option<&Env>) -> Result<String, JsError> {
	if is_command_config_value(config) {
		if let Some(value) = cached_resolve_command(config) {
			return Ok(value);
		}
	}
	let runner = {
		let cache = global_cache().lock().unwrap();
		cache.runner.clone().ok_or_else(|| JsError::new("no command runner installed"))?
	};
	resolve_config_value_or_throw(config, description, env, runner.as_ref())
		.map_err(|error| JsError::new(error.message))
}

// ============================================================================
// JSON helpers (JS semantics)
// ============================================================================

type JsonMap = Map<String, Value>;

fn truthy(value: Option<&Value>) -> bool {
	match value {
		None | Some(Value::Null) => false,
		Some(Value::Bool(b)) => *b,
		Some(Value::Number(n)) => n.as_f64() != Some(0.0),
		Some(Value::String(s)) => !s.is_empty(),
		Some(Value::Array(_)) | Some(Value::Object(_)) => true,
	}
}

/// `a ?? b` over JSON values (null/undefined fall through).
fn nz<'a>(over: Option<&'a Value>, base: &'a Value) -> &'a Value {
	match over {
		Some(Value::Null) | None => base,
		Some(v) => v,
	}
}

fn opt_nz<'a>(over: Option<&'a Value>, base: Option<&'a Value>) -> Option<&'a Value> {
	match over {
		Some(Value::Null) | None => base,
		Some(v) => Some(v),
	}
}

fn get_str(map: &JsonMap, key: &str) -> Option<String> {
	map.get(key).and_then(Value::as_str).map(str::to_string)
}

/// `{ apiKey }` / `{ apiKey, headers, baseUrl }` literals (js_object structs
/// have private `order`; construction goes through the ordered-map parser).
fn model_auth(
	api_key: Option<String>,
	headers: Option<ProviderHeaders>,
	base_url: Option<String>,
) -> ModelAuth {
	let mut map = Map::new();
	if let Some(api_key) = api_key {
		map.insert("apiKey".into(), json!(api_key));
	}
	if let Some(headers) = headers {
		map.insert("headers".into(), serde_json::to_value(&headers).unwrap_or(Value::Null));
	}
	if let Some(base_url) = base_url {
		map.insert("baseUrl".into(), json!(base_url));
	}
	ModelAuth::from_map(map).expect("valid ModelAuth fields")
}

/// `{ type: "api_key", key }` (fresh synthetic credential passed to an
/// inherited resolver).
fn synthetic_api_key_credential(key: String) -> ApiKeyCredential {
	ApiKeyCredential::from_map(
		[("key".to_string(), json!(key))].into_iter().collect(),
	)
	.expect("valid ApiKeyCredential fields")
}

fn cfg_str(cfg: Option<&Value>, key: &str) -> Option<String> {
	cfg?.get(key).and_then(Value::as_str).map(str::to_string)
}

pub type HeaderPair = (String, String);

/// `{...a, ...b}` — first-position keys, later values win.
fn merge_header_vecs(
	base: Option<&[HeaderPair]>,
	over: Option<&[HeaderPair]>,
) -> Vec<HeaderPair> {
	let mut out: Vec<(String, String)> = base.map(|b| b.to_vec()).unwrap_or_default();
	if let Some(over) = over {
		for (key, value) in over {
			if let Some(entry) = out.iter_mut().find(|(k, _)| k == key) {
				entry.1 = value.clone();
			} else {
				out.push((key.clone(), value.clone()));
			}
		}
	}
	out
}

fn value_headers(value: &Value) -> Option<Vec<(String, String)>> {
	value.as_object().map(|map| {
		map.iter()
			.filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
			.collect()
	})
}

/// Parse a model JSON map into an `AnyModel` (type-field dispatch).
pub fn any_model_from_map(map: JsonMap) -> Result<AnyModel, String> {
	match map.get("type").and_then(Value::as_str) {
		Some("image") => Ok(AnyModel::Image(ImageModel::from_map(map)?)),
		Some("classifier") => Ok(AnyModel::Classifier(ClassifierModel::from_map(map)?)),
		_ => Ok(AnyModel::Chat(Model::from_map(map)?)),
	}
}

fn any_model_to_map(model: &AnyModel) -> JsonMap {
	model.to_value().as_object().cloned().unwrap_or_default()
}

fn is_chat_map(map: &JsonMap) -> bool {
	matches!(map.get("type").and_then(Value::as_str), None | Some("chat"))
}

fn model_type_of(map: &JsonMap) -> String {
	map.get("type").and_then(Value::as_str).unwrap_or("chat").to_string()
}

// ============================================================================
// Model merging (internal helpers, verbatim TS)
// ============================================================================

const NESTED_COMPAT_KEYS: [&str; 4] =
	["openRouterRouting", "vercelGatewayRouting", "chatTemplateKwargs", "chatTemplateArgs"];

fn merge_compat(base: Option<&Value>, over: Option<&Value>) -> Option<Value> {
	let over = match over {
		None | Some(Value::Null) => return base.cloned(),
		Some(v) => v,
	};
	let base_map = base.and_then(Value::as_object);
	let over_map = over.as_object();
	let mut merged = base_map.cloned().unwrap_or_default();
	if let Some(over_map) = over_map {
		for (key, value) in over_map {
			merged.insert(key.clone(), value.clone());
		}
	}
	for key in NESTED_COMPAT_KEYS {
		let base_value = base_map.and_then(|m| m.get(key));
		let over_value = over_map.and_then(|m| m.get(key));
		let base_obj = base_value.is_some_and(|v| v.is_object());
		let over_obj = over_value.is_some_and(|v| v.is_object());
		if base_obj || over_obj {
			let mut nested = base_value.and_then(Value::as_object).cloned().unwrap_or_default();
			if let Some(over_nested) = over_value.and_then(Value::as_object) {
				for (k, v) in over_nested {
					nested.insert(k.clone(), v.clone());
				}
			}
			merged.insert(key.to_string(), Value::Object(nested));
		}
	}
	Some(Value::Object(merged))
}

fn merge_input_limits(base: Option<&Value>, over: Option<&Value>) -> Option<Value> {
	let over = match over {
		None | Some(Value::Null) => return base.cloned(),
		Some(v) => v,
	};
	let base_map = base.and_then(Value::as_object).cloned().unwrap_or_default();
	let over_map = over.as_object().cloned().unwrap_or_default();
	let mut merged = base_map.clone();
	for (key, value) in &over_map {
		merged.insert(key.clone(), value.clone());
	}
	let base_images = base_map.get("images").and_then(Value::as_object);
	let over_images = over_map.get("images").and_then(Value::as_object);
	let images = if truthy(over_map.get("images")) {
		let mut merged_images = base_images.cloned().unwrap_or_default();
		if let Some(over_images) = over_images {
			for (key, value) in over_images {
				merged_images.insert(key.clone(), value.clone());
			}
		}
		let resize = if truthy(over_images.and_then(|o| o.get("resize"))) {
			let mut merged_resize = base_images
				.and_then(|b| b.get("resize"))
				.and_then(Value::as_object)
				.cloned()
				.unwrap_or_default();
			if let Some(over_resize) = over_images.and_then(|o| o.get("resize")).and_then(Value::as_object) {
				for (key, value) in over_resize {
					merged_resize.insert(key.clone(), value.clone());
				}
			}
			Value::Object(merged_resize)
		} else {
			base_images
				.and_then(|b| b.get("resize"))
				.cloned()
				.unwrap_or(Value::Null)
		};
		merged_images.insert("resize".to_string(), resize);
		Value::Object(merged_images)
	} else {
		base_map.get("images").cloned().unwrap_or(Value::Null)
	};
	merged.insert("images".to_string(), images);
	Some(Value::Object(merged))
}

const THINKING_LEVEL_KEYS: [&str; 7] =
	["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn merge_sampling_params_by_thinking_level(base: Option<&Value>, over: Option<&Value>) -> Option<Value> {
	let over = match over {
		None | Some(Value::Null) => return base.cloned(),
		Some(v) => v,
	};
	let base_map = base.and_then(Value::as_object).cloned().unwrap_or_default();
	let over_map = over.as_object().cloned().unwrap_or_default();
	let mut merged = base_map;
	for level in THINKING_LEVEL_KEYS {
		if truthy(over_map.get(level)) {
			let mut params = merged.get(level).and_then(Value::as_object).cloned().unwrap_or_default();
			if let Some(over_params) = over_map.get(level).and_then(Value::as_object) {
				for (key, value) in over_params {
					params.insert(key.clone(), value.clone());
				}
			}
			merged.insert(level.to_string(), Value::Object(params));
		}
	}
	Some(Value::Object(merged))
}

fn apply_model_override(model: &JsonMap, over: &JsonMap) -> JsonMap {
	let mut out = model.clone();
	out.insert(
		"name".into(),
		nz(over.get("name"), model.get("name").unwrap_or(&Value::Null)).clone(),
	);
	out.insert(
		"reasoning".into(),
		nz(over.get("reasoning"), model.get("reasoning").unwrap_or(&Value::Null)).clone(),
	);
	// thinkingLevelMap: truthy override merges over the model's map.
	let tlm = if truthy(over.get("thinkingLevelMap")) {
		let mut merged = model
			.get("thinkingLevelMap")
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default();
		if let Some(over_map) = over.get("thinkingLevelMap").and_then(Value::as_object) {
			for (key, value) in over_map {
				merged.insert(key.clone(), value.clone());
			}
		}
		Value::Object(merged)
	} else {
		model.get("thinkingLevelMap").cloned().unwrap_or(Value::Null)
	};
	out.insert("thinkingLevelMap".into(), tlm);
	out.insert(
		"input".into(),
		nz(over.get("input"), model.get("input").unwrap_or(&Value::Null)).clone(),
	);
	out.insert(
		"inputLimits".into(),
		merge_input_limits(model.get("inputLimits"), over.get("inputLimits"))
			.unwrap_or(Value::Null),
	);
	let cost = if truthy(over.get("cost")) {
		let base_cost = model.get("cost").unwrap_or(&Value::Null);
		let over_cost = over.get("cost").unwrap_or(&Value::Null);
		let mut merged = Map::new();
		for field in ["input", "output", "cacheRead", "cacheWrite", "tiers"] {
			merged.insert(
				field.to_string(),
				nz(over_cost.get(field), base_cost.get(field).unwrap_or(&Value::Null)).clone(),
			);
		}
		Value::Object(merged)
	} else {
		model.get("cost").cloned().unwrap_or(Value::Null)
	};
	out.insert("cost".into(), cost);
	let prompt_cache = if truthy(over.get("promptCache")) {
		Value::Object(merge_json_maps(
			model.get("promptCache").and_then(Value::as_object),
			over.get("promptCache").and_then(Value::as_object),
		))
	} else {
		model.get("promptCache").cloned().unwrap_or(Value::Null)
	};
	out.insert("promptCache".into(), prompt_cache);
	out.insert(
		"contextWindow".into(),
		nz(over.get("contextWindow"), model.get("contextWindow").unwrap_or(&Value::Null)).clone(),
	);
	out.insert(
		"maxTokens".into(),
		nz(over.get("maxTokens"), model.get("maxTokens").unwrap_or(&Value::Null)).clone(),
	);
	let sampling = if truthy(over.get("samplingParams")) {
		Value::Object(merge_json_maps(
			model.get("samplingParams").and_then(Value::as_object),
			over.get("samplingParams").and_then(Value::as_object),
		))
	} else {
		model.get("samplingParams").cloned().unwrap_or(Value::Null)
	};
	out.insert("samplingParams".into(), sampling);
	out.insert(
		"samplingParamsByThinkingLevel".into(),
		merge_sampling_params_by_thinking_level(
			model.get("samplingParamsByThinkingLevel"),
			over.get("samplingParamsByThinkingLevel"),
		)
		.unwrap_or(Value::Null),
	);
	out.insert(
		"compat".into(),
		merge_compat(model.get("compat"), over.get("compat")).unwrap_or(Value::Null),
	);
	out
}

fn merge_json_maps(base: Option<&JsonMap>, over: Option<&JsonMap>) -> JsonMap {
	let mut out = base.cloned().unwrap_or_default();
	if let Some(over) = over {
		for (key, value) in over {
			out.insert(key.clone(), value.clone());
		}
	}
	out
}

fn model_from_json(
	provider_id: &str,
	definition: &JsonMap,
	provider_config: Option<&Value>,
	defaults: Option<&JsonMap>,
) -> Result<JsonMap, JsError> {
	let model_id = get_str(definition, "id").unwrap_or_default();
	let api = opt_nz(definition.get("api"), opt_nz(
		provider_config.and_then(|c| c.get("api")),
		defaults.and_then(|d| d.get("api")),
	))
	.and_then(Value::as_str)
	.map(str::to_string);
	let api = match api {
		Some(api) => api,
		None => {
			return Err(JsError::new(format!(
				"Provider {provider_id}, model {model_id}: no \"api\" specified. Set at provider or model level."
			)))
		}
	};
	let base_url = opt_nz(
		definition.get("baseUrl"),
		opt_nz(
			provider_config.and_then(|c| c.get("baseUrl")),
			defaults.and_then(|d| d.get("baseUrl")),
		),
	)
	.and_then(Value::as_str)
	.map(str::to_string);
	let base_url = match base_url {
		Some(base_url) if !base_url.is_empty() => base_url,
		_ => {
			return Err(JsError::new(format!(
				"Provider {provider_id}: \"baseUrl\" is required when defining custom models."
			)))
		}
	};
	for (field, label) in [("contextWindow", "contextWindow"), ("maxTokens", "maxTokens")] {
		if let Some(value) = definition.get(field) {
			let invalid = match value {
				Value::Null => true, // JS: null !== undefined && null <= 0
				Value::Number(n) => n.as_f64() <= Some(0.0),
				_ => false,
			};
			if invalid {
				return Err(JsError::new(format!(
					"Provider {provider_id}, model {model_id}: invalid {label}"
				)));
			}
		}
	}
	let mut out = Map::new();
	out.insert("id".into(), json!(model_id));
	out.insert(
		"name".into(),
		nz(definition.get("name"), &json!(model_id)).clone(),
	);
	out.insert("api".into(), json!(api));
	out.insert("provider".into(), json!(provider_id));
	out.insert("baseUrl".into(), json!(base_url));
	out.insert(
		"reasoning".into(),
		nz(definition.get("reasoning"), &json!(false)).clone(),
	);
	if let Some(tlm) = definition.get("thinkingLevelMap") {
		out.insert("thinkingLevelMap".into(), tlm.clone());
	}
	out.insert(
		"input".into(),
		nz(definition.get("input"), &json!(["text"])).clone(),
	);
	if let Some(v) = definition.get("inputLimits") {
		out.insert("inputLimits".into(), v.clone());
	}
	out.insert(
		"cost".into(),
		nz(
			definition.get("cost"),
			&json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}),
		)
		.clone(),
	);
	if let Some(v) = definition.get("promptCache") {
		out.insert("promptCache".into(), v.clone());
	}
	out.insert(
		"contextWindow".into(),
		nz(definition.get("contextWindow"), &json!(128000)).clone(),
	);
	out.insert(
		"maxTokens".into(),
		nz(definition.get("maxTokens"), &json!(16384)).clone(),
	);
	if let Some(v) = definition.get("samplingParams") {
		out.insert("samplingParams".into(), v.clone());
	}
	if let Some(v) = definition.get("samplingParamsByThinkingLevel") {
		out.insert("samplingParamsByThinkingLevel".into(), v.clone());
	}
	out.insert(
		"compat".into(),
		merge_compat(
			provider_config.and_then(|c| c.get("compat")),
			definition.get("compat"),
		)
		.unwrap_or(Value::Null),
	);
	Ok(out)
}

fn find_model_defaults(
	models: &[JsonMap],
	model_id: &str,
	api: Option<&str>,
) -> Option<JsonMap> {
	let chat_models: Vec<&JsonMap> = models.iter().filter(|m| is_chat_map(m)).collect();
	chat_models
		.iter()
		.find(|m| get_str(m, "id").as_deref() == Some(model_id))
		.or_else(|| {
			api.and_then(|api| {
				chat_models.iter().find(|m| get_str(m, "api").as_deref() == Some(api))
			})
		})
		.or_else(|| {
			chat_models.iter().find(|m| get_str(m, "api").as_deref() == Some("openai-completions"))
		})
		.or_else(|| chat_models.first())
		.map(|m| (*m).clone())
}

fn find_extension_model_defaults(models: &[JsonMap], definition: &JsonMap) -> Option<JsonMap> {
	let ty = model_type_of(definition);
	let candidates: Vec<&JsonMap> = models.iter().filter(|m| model_type_of(m) == ty).collect();
	candidates
		.iter()
		.find(|m| get_str(m, "id").as_deref() == get_str(definition, "id").as_deref())
		.or_else(|| {
			definition.get("api").and_then(Value::as_str).and_then(|api| {
				candidates.iter().find(|m| get_str(m, "api").as_deref() == Some(api))
			})
		})
		.or_else(|| {
			if ty == "chat" {
				candidates.iter().find(|m| get_str(m, "api").as_deref() == Some("openai-completions"))
			} else {
				None
			}
		})
		.or_else(|| candidates.first())
		.map(|m| (*m).clone())
}

fn extension_model_from_definition(
	provider_id: &str,
	models: &[JsonMap],
	config: &ProviderConfigInput,
	definition: &JsonMap,
) -> Result<JsonMap, JsError> {
	let ty = model_type_of(definition);
	let model_id = get_str(definition, "id").unwrap_or_default();
	let defaults = find_extension_model_defaults(models, definition);
	let provider_api: Option<&str> = if ty == "chat" { config.api.as_deref() } else { None };
	let api = opt_nz(
		definition.get("api"),
		opt_nz(
			provider_api.map(|api| Value::String(api.to_string())).as_ref(),
			defaults.as_ref().and_then(|d| d.get("api")),
		),
	)
	.and_then(Value::as_str)
	.map(str::to_string);
	let api = match api {
		Some(api) => api,
		None => {
			let suffix = if ty == "chat" { " or provider level" } else { "" };
			return Err(JsError::new(format!(
				"Provider {provider_id}, model {model_id}: no \"api\" specified. Set it at model level{suffix}."
			)));
		}
	};
	let base_url = opt_nz(
		definition.get("baseUrl"),
		opt_nz(
			config.base_url.as_deref().map(|url| Value::String(url.to_string())).as_ref(),
			defaults.as_ref().and_then(|d| d.get("baseUrl")),
		),
	)
	.and_then(Value::as_str)
	.map(str::to_string);
	let base_url = match base_url {
		Some(base_url) if !base_url.is_empty() => base_url,
		_ => {
			return Err(JsError::new(format!(
				"Provider {provider_id}: \"baseUrl\" is required when defining custom models."
			)))
		}
	};
	let mut out = definition.clone();
	out.insert("api".into(), json!(api));
	out.insert("provider".into(), json!(provider_id));
	out.insert("baseUrl".into(), json!(base_url));
	// `headers: undefined` — the key is removed (JS-delete semantics).
	out.shift_remove("headers");
	Ok(out)
}

fn get_all_provider_models(base: Option<&Provider>) -> Vec<JsonMap> {
	base.map(|b| b.get_all_models().iter().map(any_model_to_map).collect())
		.unwrap_or_default()
}

fn apply_models_json(
	provider_id: &str,
	base_models: Vec<JsonMap>,
	config: Option<&Value>,
) -> Result<Vec<JsonMap>, JsError> {
	let Some(config) = config else {
		return Ok(base_models);
	};
	let cfg = config.as_object().cloned().unwrap_or_default();
	let oauth = truthy(cfg.get("oauth"));
	let base_url = cfg.get("baseUrl").and_then(Value::as_str);
	if oauth && base_url.is_none() {
		return Err(JsError::new(format!(
			"Provider {provider_id}: \"baseUrl\" is required when \"oauth\" is set."
		)));
	}
	let has_overrides = cfg
		.get("modelOverrides")
		.and_then(Value::as_object)
		.map(|m| !m.is_empty())
		.unwrap_or(false);
	let models_empty = cfg
		.get("models")
		.and_then(Value::as_array)
		.map(|a| a.is_empty())
		.unwrap_or(true);
	let auth_header_set = cfg.get("authHeader").is_some_and(|v| !v.is_null());
	if models_empty
		&& !truthy(cfg.get("baseUrl"))
		&& !truthy(cfg.get("headers"))
		&& !truthy(cfg.get("compat"))
		&& !has_overrides
		&& !truthy(cfg.get("apiKey"))
		&& !oauth
		&& !auth_header_set
	{
		return Err(JsError::new(format!(
			"Provider {provider_id}: must specify \"baseUrl\", \"headers\", \"compat\", \"modelOverrides\", or \"models\"."
		)));
	}
	let mut models: Vec<JsonMap> = base_models
		.into_iter()
		.map(|model| {
			let url = if cfg.get("oauth").and_then(Value::as_str) == Some("radius") {
				model.get("baseUrl").cloned().unwrap_or(Value::Null)
			} else {
				nz(cfg.get("baseUrl"), model.get("baseUrl").unwrap_or(&Value::Null)).clone()
			};
			let mut out = model.clone();
			out.insert("baseUrl".into(), url);
			if is_chat_map(&model) {
				out.insert(
					"compat".into(),
					merge_compat(model.get("compat"), cfg.get("compat")).unwrap_or(Value::Null),
				);
			}
			out
		})
		.collect();
	let definitions = cfg.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
	for definition in definitions {
		let definition = definition.as_object().cloned().unwrap_or_default();
		let existing_index = models
			.iter()
			.position(|m| is_chat_map(m) && get_str(m, "id") == get_str(&definition, "id"));
		let defaults = find_model_defaults(
			&models,
			&get_str(&definition, "id").unwrap_or_default(),
			opt_nz(definition.get("api"), cfg.get("api")).and_then(Value::as_str),
		);
		let model = model_from_json(provider_id, &definition, Some(config), defaults.as_ref())?;
		match existing_index {
			Some(index) => models[index] = model,
			None => models.push(model),
		}
	}
	Ok(models)
}

fn apply_extension(
	provider_id: &str,
	models: Vec<JsonMap>,
	config: Option<&ProviderConfigInput>,
) -> Result<Vec<JsonMap>, JsError> {
	let Some(config) = config else {
		return Ok(models);
	};
	let Some(definitions) = &config.models else {
		return Ok(match &config.base_url {
			Some(base_url) if !base_url.is_empty() => models
				.into_iter()
				.map(|mut model| {
					model.insert("baseUrl".into(), json!(base_url));
					model
				})
				.collect(),
			_ => models,
		});
	};
	definitions
		.iter()
		.map(|definition| extension_model_from_definition(provider_id, &models, config, definition))
		.collect()
}

fn configured_api_key(
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
) -> Option<String> {
	extension
		.and_then(|e| e.api_key.clone())
		.or_else(|| cfg_str(config, "apiKey"))
}

fn configured_headers(
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
) -> Option<Vec<(String, String)>> {
	let config_headers = config.and_then(|c| c.get("headers")).and_then(value_headers);
	let extension_headers = extension.and_then(|e| e.headers.clone());
	if config_headers.is_none() && extension_headers.is_none() {
		return None;
	}
	Some(merge_header_vecs(config_headers.as_deref(), extension_headers.as_deref()))
}

fn raw_model_headers(
	model: &JsonMap,
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
) -> Option<Vec<(String, String)>> {
	let chat = is_chat_map(model);
	let chat_definition = if chat {
		config
			.and_then(|c| c.get("models"))
			.and_then(Value::as_array)
			.and_then(|models| {
				models
					.iter()
					.find(|entry| entry.get("id").and_then(Value::as_str) == get_str(model, "id").as_deref())
			})
	} else {
		None
	};
	let extension_model = extension.and_then(|e| e.models.as_ref()).and_then(|models| {
		models.iter().find(|entry| {
			model_type_of(entry) == model_type_of(model) && get_str(entry, "id") == get_str(model, "id")
		})
	});
	let override_headers = if chat {
		config
			.and_then(|c| c.get("modelOverrides"))
			.and_then(Value::as_object)
			.and_then(|overrides| {
				overrides
					.get(get_str(model, "id").as_deref().unwrap_or(""))
					.and_then(|entry| entry.get("headers"))
			})
			.and_then(value_headers)
	} else {
		None
	};
	let chat_headers = chat_definition.and_then(|d| d.get("headers")).and_then(value_headers);
	let extension_headers = extension_model.and_then(|m| m.get("headers")).and_then(value_headers);
	let headers = merge_header_vecs(
		Some(&merge_header_vecs(
			override_headers.as_deref(),
			chat_headers.as_deref(),
		)),
		extension_headers.as_deref(),
	);
	if headers.is_empty() {
		None
	} else {
		Some(headers)
	}
}

/// TS `configContextEnv` — explicit values first, then `ctx.env` for the
/// config-value env-var names not already present.
fn config_context_env(
	values: &[String],
	ctx: &dyn AuthContext,
	explicit: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<Env> {
	let mut env: Env = explicit.cloned().unwrap_or_default();
	let mut names: Vec<String> = Vec::new();
	for value in values {
		for name in get_config_value_env_var_names(value) {
			if !names.contains(&name) {
				names.push(name);
			}
		}
	}
	for name in names {
		if env.contains_key(&name) {
			continue;
		}
		if let Some(value) = ctx.env(&name) {
			env.insert(name, value);
		}
	}
	if env.is_empty() {
		None
	} else {
		Some(env)
	}
}

fn with_configured_auth(
	auth: &ModelAuth,
	headers: Option<&[(String, String)]>,
	auth_header: bool,
) -> Result<ModelAuth, JsError> {
	// `{ ...auth, headers: mergedHeaders }` — spread keeps key positions.
	let mut out = auth.clone();
	if auth.headers.is_some() || headers.is_some() {
		let mut merged: ProviderHeaders = auth.headers.clone().unwrap_or_default();
		if let Some(headers) = headers {
			for (key, value) in headers {
				merged.insert(key.clone(), Some(value.clone()));
			}
		}
		out.set("headers", serde_json::to_value(&merged).unwrap_or(Value::Null))
			.expect("valid headers");
	}
	if auth_header {
		let api_key = out
			.api_key
			.clone()
			.ok_or_else(|| JsError::new("authHeader requires a resolved API key"))?;
		let mut merged = out.headers.clone().unwrap_or_default();
		merged.insert("Authorization".to_string(), Some(format!("Bearer {api_key}")));
		out.set("headers", serde_json::to_value(&merged).unwrap_or(Value::Null))
			.expect("valid headers");
	}
	Ok(out)
}

fn header_env_from_value(env: Option<&Value>) -> Option<Env> {
	let map = env.and_then(Value::as_object)?;
	Some(
		map.iter()
			.filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
			.collect(),
	)
}

// ============================================================================
// Composed auth
// ============================================================================

pub struct ComposedApiKeyAuth {
	provider_id: String,
	inherited: Option<Arc<dyn ApiKeyAuth + Send + Sync>>,
	raw_key: Option<String>,
	raw_headers: Option<Vec<(String, String)>>,
	auth_header: bool,
}

impl ApiKeyAuth for ComposedApiKeyAuth {
	fn name(&self) -> &str {
		match &self.inherited {
			Some(inherited) => inherited.name(),
			None => "API key",
		}
	}

	fn has_check(&self) -> bool {
		true
	}

	fn check(
		&self,
		ctx: &dyn AuthContext,
		credential: Option<&ApiKeyCredential>,
		signal: Option<&crate::pi_ai::credential::AbortFlag>,
	) -> Result<Option<AuthCheck>, JsError> {
		if let Some(credential) = credential {
			if let Some(inherited) = &self.inherited {
				if inherited.has_check() {
					return inherited.check(ctx, Some(credential), signal);
				}
			}
			if credential.key.as_deref().is_some_and(|key| !key.is_empty()) {
				return Ok(Some(AuthCheck {
					source: Some("stored credential".to_string()),
					auth_type: AuthType::ApiKey,
				}));
			}
			if let Some(inherited) = &self.inherited {
				let resolved = inherited.resolve(ctx, Some(credential), signal)?;
				return Ok(resolved.map(|r| AuthCheck { source: r.source, auth_type: AuthType::ApiKey }));
			}
			return Ok(None);
		}
		if let Some(raw_key) = &self.raw_key {
			if is_command_config_value(raw_key) {
				return Ok(Some(AuthCheck {
					source: Some("configured API key".to_string()),
					auth_type: AuthType::ApiKey,
				}));
			}
			for name in get_config_value_env_var_names(raw_key) {
				if ctx.env(&name).is_none() {
					return Ok(None);
				}
			}
			return Ok(Some(AuthCheck {
				source: Some("configured API key".to_string()),
				auth_type: AuthType::ApiKey,
			}));
		}
		if let Some(inherited) = &self.inherited {
			if inherited.has_check() {
				return inherited.check(ctx, credential, signal);
			}
			let resolved = inherited.resolve(ctx, credential, signal)?;
			return Ok(resolved.map(|r| AuthCheck { source: r.source, auth_type: AuthType::ApiKey }));
		}
		Ok(None)
	}

	fn resolve(
		&self,
		ctx: &dyn AuthContext,
		credential: Option<&ApiKeyCredential>,
		signal: Option<&crate::pi_ai::credential::AbortFlag>,
	) -> Result<Option<AuthResult>, JsError> {
		let result: Option<AuthResult> = if let Some(credential) = credential {
			match &self.inherited {
				Some(inherited) => inherited.resolve(ctx, Some(credential), signal)?,
				None => credential.key.as_ref().map(|key| AuthResult {
					auth: model_auth(Some(key.clone()), None, None),
					env: credential.env.clone(),
					source: Some("stored credential".to_string()),
				}),
			}
		} else if let Some(raw_key) = &self.raw_key {
			let env = config_context_env(std::slice::from_ref(raw_key), ctx, None);
			let key = resolve_or_throw(
				raw_key,
				&format!("API key for provider \"{}\"", self.provider_id),
				env.as_ref(),
			)?;
			match &self.inherited {
				Some(inherited) => inherited.resolve(
					ctx,
					Some(&synthetic_api_key_credential(key)),
					signal,
				)?,
				None => Some(AuthResult {
					auth: model_auth(Some(key), None, None),
					env: None,
					source: Some("configured API key".to_string()),
				}),
			}
		} else {
			match &self.inherited {
				Some(inherited) => inherited.resolve(ctx, credential, signal)?,
				None => None,
			}
		};
		let Some(result) = result else {
			return Ok(None);
		};
		let mut explicit: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
		if let Some(credential) = credential {
			if let Some(env) = &credential.env {
				for (key, value) in env {
					explicit.insert(key.clone(), value.clone());
				}
			}
		}
		if let Some(env) = &result.env {
			for (key, value) in env {
				explicit.insert(key.clone(), value.clone());
			}
		}
		let values: Vec<String> = self
			.raw_headers
			.as_ref()
			.map(|headers| headers.iter().map(|(_, v)| v.clone()).collect())
			.unwrap_or_default();
		let header_env = config_context_env(&values, ctx, Some(&explicit));
		let headers = resolve_headers_or_throw(
			self.raw_headers.as_deref(),
			&format!("provider \"{}\"", self.provider_id),
			header_env.as_ref(),
			dummy_runner(),
		)
		.map_err(|error| JsError::new(error.message))?;
		let auth = with_configured_auth(&result.auth, headers.as_deref(), self.auth_header)?;
		Ok(Some(AuthResult { auth, env: result.env, source: result.source }))
	}
}

/// Placeholder runner for `resolve_headers_or_throw` (command header values
/// route through the cached path in `resolve_or_throw` — plain env-var and
/// literal values never reach a runner).
fn dummy_runner() -> &'static dyn CommandRunner {
	struct NoopRunner;
	impl CommandRunner for NoopRunner {
		fn run(&self, _command: &str) -> Option<String> {
			None
		}
	}
	static RUNNER: NoopRunner = NoopRunner;
	&RUNNER
}

struct AdaptedOAuth {
	config: Arc<ExtensionOAuthConfig>,
}

impl OAuthAuth for AdaptedOAuth {
	fn name(&self) -> &str {
		&self.config.name
	}
	fn is_subscription(&self) -> bool {
		self.config.is_subscription.unwrap_or(false)
	}
	fn login_label(&self) -> Option<&str> {
		None
	}
	fn to_auth(&self, credential: &OAuthCredential) -> Result<ModelAuth, JsError> {
		Ok(model_auth(Some((self.config.get_api_key)(credential)), None, None))
	}
}

pub struct ComposedOAuthAuth {
	provider_id: String,
	inner: Arc<dyn OAuthAuth + Send + Sync>,
	raw_headers: Option<Vec<(String, String)>>,
	auth_header: bool,
}

impl OAuthAuth for ComposedOAuthAuth {
	fn name(&self) -> &str {
		self.inner.name()
	}
	fn is_subscription(&self) -> bool {
		self.inner.is_subscription()
	}
	fn login_label(&self) -> Option<&str> {
		self.inner.login_label()
	}
	fn to_auth(&self, credential: &OAuthCredential) -> Result<ModelAuth, JsError> {
		let auth = self.inner.to_auth(credential)?;
		let env = credential.to_value().get("env").cloned();
		let headers = resolve_headers_or_throw(
			self.raw_headers.as_deref(),
			&format!("provider \"{}\"", self.provider_id),
			header_env_from_value(env.as_ref()).as_ref(),
			dummy_runner(),
		)
		.map_err(|error| JsError::new(error.message))?;
		with_configured_auth(&auth, headers.as_deref(), self.auth_header)
	}
}

pub fn compose_api_key_auth(
	provider_id: &str,
	base: Option<&Provider>,
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
) -> Option<Arc<ComposedApiKeyAuth>> {
	let inherited = base.and_then(|b| b.auth.api_key.clone());
	let raw_key = configured_api_key(config, extension);
	let oauth = extension
		.and_then(|e| e.oauth.clone())
		.map(|o| Arc::new(AdaptedOAuth { config: o }) as Arc<dyn OAuthAuth + Send + Sync>)
		.or_else(|| base.and_then(|b| b.auth.oauth.clone()));
	if inherited.is_none() && raw_key.is_none() && oauth.is_some() {
		return None;
	}
	let raw_headers = configured_headers(config, extension);
	let auth_header = extension
		.and_then(|e| e.auth_header)
		.or_else(|| config.and_then(|c| c.get("authHeader")).and_then(Value::as_bool))
		.unwrap_or(false);
	Some(Arc::new(ComposedApiKeyAuth {
		provider_id: provider_id.to_string(),
		inherited,
		raw_key,
		raw_headers,
		auth_header,
	}))
}

pub fn compose_oauth_auth(
	provider_id: &str,
	base: Option<&Provider>,
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
) -> Option<Arc<ComposedOAuthAuth>> {
	let inner: Option<Arc<dyn OAuthAuth + Send + Sync>> = match extension.and_then(|e| e.oauth.clone())
	{
		Some(config) => Some(Arc::new(AdaptedOAuth { config })),
		None => base.and_then(|b| b.auth.oauth.clone()),
	};
	let inner = inner?;
	let raw_headers = configured_headers(config, extension);
	let auth_header = extension
		.and_then(|e| e.auth_header)
		.or_else(|| config.and_then(|c| c.get("authHeader")).and_then(Value::as_bool))
		.unwrap_or(false);
	Some(Arc::new(ComposedOAuthAuth {
		provider_id: provider_id.to_string(),
		inner,
		raw_headers,
		auth_header,
	}))
}

// ============================================================================
// Error results (pi-ai utils/model-operations.ts)
// ============================================================================

fn image_error_result(model: &ImageModel, message: &str) -> AssistantImages {
	AssistantImages(json!({
		"api": model.api, "provider": model.provider, "model": model.id,
		"output": [], "stopReason": "error", "errorMessage": message, "timestamp": 0,
	}))
}

fn classifier_error_result(model: &ClassifierModel, message: &str) -> ClassifierResult {
	ClassifierResult(json!({
		"api": model.api, "provider": model.provider, "model": model.id,
		"answers": {}, "stopReason": "error", "errorMessage": message, "timestamp": 0,
	}))
}

// ============================================================================
// composeModelProvider
// ============================================================================

struct ComposerShared {
	provider_id: String,
	config: Option<Value>,
	base: Option<Arc<Provider>>,
	extension: Option<ProviderConfigInput>,
	refreshed_extension_models: Mutex<Option<Vec<JsonMap>>>,
	extension_oauth_credential: Mutex<Option<OAuthCredential>>,
	update_error: Mutex<Option<JsError>>,
}

impl ComposerShared {
	fn current_extension(&self) -> Option<ProviderConfigInput> {
		let refreshed = self.refreshed_extension_models.lock().unwrap();
		match (&self.extension, refreshed.as_ref()) {
			(Some(extension), Some(refreshed)) => Some(ProviderConfigInput {
				models: Some(refreshed.clone()),
				..extension.clone()
			}),
			(other, _) => other.clone(),
		}
	}

	fn get_all_models(&self) -> Result<Vec<AnyModel>, JsError> {
		let mut models = apply_extension(
			&self.provider_id,
			apply_models_json(
				&self.provider_id,
				get_all_provider_models(self.base.as_deref()),
				self.config.as_ref(),
			)?,
			self.current_extension().as_ref(),
		)?;
		let oauth_credential = self.extension_oauth_credential.lock().unwrap().clone();
		let modify_models = self
			.extension
			.as_ref()
			.and_then(|e| e.oauth.as_ref())
			.and_then(|o| o.modify_models.clone());
		if let (Some(credential), Some(modify_models)) = (oauth_credential, modify_models) {
			// The extension hook is chat-only; other model types pass through untouched.
			let chat: Vec<JsonMap> = models.iter().filter(|m| is_chat_map(m)).cloned().collect();
			let mut merged = modify_models(&chat, &credential);
			merged.extend(models.iter().filter(|m| !is_chat_map(m)).cloned());
			models = merged;
		}
		let overrides = self
			.config
			.as_ref()
			.and_then(|c| c.get("modelOverrides"))
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default();
		let models: Vec<JsonMap> = models
			.into_iter()
			.map(|model| {
				match get_str(&model, "id").as_deref().and_then(|id| overrides.get(id)) {
					Some(over) if is_chat_map(&model) => {
						let over = over.as_object().cloned().unwrap_or_default();
						apply_model_override(&model, &over)
					}
					_ => model,
				}
			})
			.collect();
		models
			.into_iter()
			.map(any_model_from_map)
			.collect::<Result<Vec<_>, _>>()
			.map_err(|error| JsError::new(format!("invalid model entry: {error}")))
	}

	fn supports_base_api(&self, model: &Model) -> bool {
		self.base
			.as_ref()
			.map(|base| base.get_models().iter().any(|entry| entry.api == model.api))
			.unwrap_or(false)
	}

	fn stream_with(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&StreamOptions>,
		simple: bool,
	) -> AssistantMessageEventStream {
		if let Some(extension) = &self.extension {
			if let (Some(stream_simple), Some(api)) = (&extension.stream_simple, &extension.api) {
				if model.api == *api {
					return stream_simple(model, context, options.map(|o| o as &SimpleStreamOptions));
				}
			}
		}
		if let Some(base) = &self.base {
			if self.supports_base_api(model) {
				return if simple {
					base.stream_simple(model, context, options)
				} else {
					base.stream(model, context, options)
				};
			}
		}
		match compat::get_api_provider(&model.api) {
			Some(api) => {
				if simple {
					(api.stream_simple)(model, context, options)
				} else {
					(api.stream)(model, context, options)
				}
			}
			None => AssistantMessageEventStream(json!({
				"type": "error", "reason": "error",
				"error": { "errorMessage": format!("No API provider registered for api: {}", model.api) }
			})),
		}
	}
}

/// `validateExtensionProvider` — structural validation only.
pub fn validate_extension_provider(
	provider_id: &str,
	base: Option<&Provider>,
	models_config: Option<&Value>,
	extension: &ProviderConfigInput,
) -> Result<(), JsError> {
	if extension.stream_simple.is_some() && extension.api.is_none() {
		return Err(JsError::new(format!(
			"Provider {provider_id}: \"api\" is required when registering streamSimple."
		)));
	}
	apply_extension(
		provider_id,
		apply_models_json(provider_id, get_all_provider_models(base), models_config)?,
		Some(extension),
	)?;
	Ok(())
}

/// `composeModelProvider` — compose built-in, models.json and extension
/// layers without reading credentials.
pub fn compose_model_provider(
	provider_id: &str,
	base: Option<Provider>,
	model_config: &ModelConfig,
	extension: Option<ProviderConfigInput>,
) -> Result<Provider, JsError> {
	compose_model_provider_shared(provider_id, base.map(Arc::new), model_config, extension)
}

/// Shared-base variant of [`compose_model_provider`] for callers (e.g.
/// `ModelRuntime`) that keep their builtin providers behind `Arc` and must
/// compose repeatedly without consuming the base. Added for the
/// model-runtime.ts port; behavior is identical to the by-value entry point.
pub fn compose_model_provider_shared(
	provider_id: &str,
	base: Option<Arc<Provider>>,
	model_config: &ModelConfig,
	extension: Option<ProviderConfigInput>,
) -> Result<Provider, JsError> {
	let config = model_config.get_provider(provider_id).cloned();
	let shared = Arc::new(ComposerShared {
		provider_id: provider_id.to_string(),
		config: config.clone(),
		base: base.clone(),
		extension: extension.clone(),
		refreshed_extension_models: Mutex::new(None),
		extension_oauth_credential: Mutex::new(None),
		update_error: Mutex::new(None),
	});
	// Validate eagerly so registration/reload reports structural errors.
	shared.get_all_models()?;

	let api_key = compose_api_key_auth(provider_id, base.as_deref(), config.as_ref(), extension.as_ref())
		.map(|auth| auth as Arc<dyn ApiKeyAuth + Send + Sync>);
	let oauth = compose_oauth_auth(provider_id, base.as_deref(), config.as_ref(), extension.as_ref())
		.map(|auth| auth as Arc<dyn OAuthAuth + Send + Sync>);
	if api_key.is_none() && oauth.is_none() {
		return Err(JsError::new(format!(
			"Provider {provider_id}: no authentication method configured."
		)));
	}
	let auth = ProviderAuth { api_key, oauth };

	let name = extension
		.as_ref()
		.and_then(|e| e.name.clone())
		.or_else(|| cfg_str(config.as_ref(), "name"))
		.or_else(|| base.as_ref().map(|b| b.name.clone()))
		.or_else(|| {
			extension
				.as_ref()
				.and_then(|e| e.oauth.as_ref()).map(|o| o.name.clone())
		})
		.unwrap_or_else(|| provider_id.to_string());
	let base_url = extension
		.as_ref()
		.and_then(|e| e.base_url.clone())
		.or_else(|| cfg_str(config.as_ref(), "baseUrl"))
		.or_else(|| base.as_ref().and_then(|b| b.base_url.clone()));
	let headers = base.as_ref().and_then(|b| b.headers.clone());

	let get_models_shared = shared.clone();
	let get_models: crate::pi_ai::provider::GetModelsFn = Box::new(move || {
		get_models_shared
			.get_all_models()
			.unwrap_or_default()
			.into_iter()
			.filter_map(|model| match model {
				AnyModel::Chat(model) => Some(model),
				_ => None,
			})
			.collect()
	});
	let stream_shared = shared.clone();
	let stream = Box::new(
		move |model: &Model, context: &TranscriptContext, options: Option<&StreamOptions>| {
			stream_shared.stream_with(model, context, options, false)
		},
	);
	let stream_simple_shared = shared.clone();
	let stream_simple = Box::new(
		move |model: &Model, context: &TranscriptContext, options: Option<&SimpleStreamOptions>| {
			stream_simple_shared.stream_with(model, context, options.map(|o| o as &StreamOptions), true)
		},
	);

	let get_all_models_shared = shared.clone();
	let get_all_models: crate::pi_ai::provider::GetAllModelsFn =
		Box::new(move || get_all_models_shared.get_all_models().unwrap_or_default());

	let mut provider = Provider::new(provider_id.to_string(), name, auth, get_models, stream, stream_simple)
		.with_get_all_models(get_all_models)
		.with_base_url(base_url)
		.with_headers(headers);

	// refreshModels: base, extension or the oauth modifyModels hook.
	let has_refresh = base.as_ref().map(|b| b.refresh_models_fn().is_some()).unwrap_or(false)
		|| extension.as_ref().map(|e| e.refresh_models.is_some()).unwrap_or(false)
		|| extension
			.as_ref()
			.and_then(|e| e.oauth.as_ref())
			.map(|o| o.modify_models.is_some())
			.unwrap_or(false);
	if has_refresh {
		let refresh_shared = shared.clone();
		provider = provider.with_refresh_models(Box::new(
			move |context: &mut RefreshModelsContext<'_>| {
				if let Some(base) = &refresh_shared.base {
					if let Some(refresh) = base.refresh_models_fn() {
						refresh(context)?;
					}
				}
				let mut refreshed = None;
				if let Some(extension) = &refresh_shared.extension {
					if let Some(refresh_models) = &extension.refresh_models {
						refreshed = Some(refresh_models(context)?);
					}
				}
				if context.signal.is_some_and(|signal| signal.aborted()) {
					return Ok(());
				}
				let oauth_credential = match context.credential {
					Some(Credential::OAuth(credential)) => Some(credential.clone()),
					_ => None,
				};
				let update_shared = refresh_shared.clone();
				let update: Box<dyn FnOnce() + Send> = Box::new(move || {
					if let Some(refreshed) = refreshed {
						// Validate before publishing the new synchronous list.
						let extension_input = ProviderConfigInput {
							models: Some(refreshed.clone()),
							..update_shared.extension.clone().unwrap_or(ProviderConfigInput {
								name: None,
								base_url: None,
								api_key: None,
								api: None,
								stream_simple: None,
								images: None,
								classifiers: None,
								headers: None,
								auth_header: None,
								oauth: None,
								models: None,
								refresh_models: None,
							})
						};
						let validated = apply_models_json(
							&update_shared.provider_id,
							get_all_provider_models(update_shared.base.as_deref()),
							update_shared.config.as_ref(),
						)
						.and_then(|models| {
							apply_extension(&update_shared.provider_id, models, Some(&extension_input))
						});
						if let Err(error) = validated {
							*update_shared.update_error.lock().unwrap() = Some(error);
							return;
						}
						*update_shared.refreshed_extension_models.lock().unwrap() = Some(refreshed);
					}
					*update_shared.extension_oauth_credential.lock().unwrap() = oauth_credential;
				});
				(context.publish)(ModelsPublication { persist: Persist::Unchanged, update: Some(update) })?;
				if let Some(error) = refresh_shared.update_error.lock().unwrap().take() {
					return Err(error);
				}
				Ok(())
			},
		));
	}

	// filterModels / filterAllModels: pass-through to the base provider.
	if let Some(base) = &base {
		if base.has_filter_models() {
			let base = base.clone();
			provider = provider.with_filter_models(Box::new(move |models, credential| {
				base.filter_models(models, credential)
			}));
		}
		if base.has_filter_all_models() {
			let base = base.clone();
			provider = provider.with_filter_all_models(Box::new(move |models, credential| {
				base.filter_all_models(models, credential)
			}));
		}
		if base.has_fetch_deferred() {
			let base = base.clone();
			provider = provider.with_fetch_deferred(Box::new(
				move |model: &Model, handle: &DeferredHandle, options: Option<&DeferredFetchOptions>| {
					base.fetch_deferred(model, handle, options).unwrap_or_default()
				},
			));
		}
		if base.has_cancel_deferred() {
			let base = base.clone();
			provider = provider.with_cancel_deferred(Box::new(
				move |model: &Model, handle: &DeferredHandle, options: Option<&DeferredCancelOptions>| {
					base.cancel_deferred(model, handle, options).unwrap_or(Ok(()))
				},
			));
		}
	}

	// generateImages / classify: extension implementations first, then base,
	// else the never-rejects error result.
	let extension_images = extension.as_ref().and_then(|e| e.images.clone());
	let has_base_images = base.as_ref().map(|b| b.has_generate_images()).unwrap_or(false);
	if has_base_images || extension_images.as_ref().is_some_and(|m| !m.is_empty()) {
		let images_shared = shared.clone();
		provider = provider.with_generate_images(Box::new(
			move |model: &ImageModel, context: &ImagesContext, options: Option<&ImagesOptions>| {
				if let Some(implementation) = extension_images
					.as_ref()
					.and_then(|images| images.get(&model.api))
				{
					return (implementation.generate_images)(model, context, options);
				}
				if let Some(base) = images_shared.base.as_ref() {
					if let Some(result) = base.generate_images(model, context, options) {
						return result;
					}
				}
				image_error_result(
					model,
					&format!(
						"Provider {} has no image implementation for \"{}\"",
						images_shared.provider_id, model.api
					),
				)
			},
		));
	}
	let extension_classifiers = extension.as_ref().and_then(|e| e.classifiers.clone());
	let has_base_classify = base.as_ref().map(|b| b.has_classify()).unwrap_or(false);
	if has_base_classify || extension_classifiers.as_ref().is_some_and(|m| !m.is_empty()) {
		let classify_shared = shared.clone();
		provider = provider.with_classify(Box::new(
			move |model: &ClassifierModel, context: &ClassifierContext, options: Option<&ClassifierOptions>| {
				if let Some(implementation) = extension_classifiers
					.as_ref()
					.and_then(|classifiers| classifiers.get(&model.api))
				{
					return (implementation.classify)(model, context, options);
				}
				if let Some(base) = classify_shared.base.as_ref() {
					if let Some(result) = base.classify(model, context, options) {
						return result;
					}
				}
				classifier_error_result(
					model,
					&format!(
						"Provider {} has no classifier implementation for \"{}\"",
						classify_shared.provider_id, model.api
					),
				)
			},
		));
	}

	Ok(provider)
}

// ============================================================================
// Header/auth resolution exports
// ============================================================================

/// `resolveConfiguredModelHeaders`.
pub fn resolve_configured_model_headers(
	model: &AnyModel,
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
	env: Option<&Env>,
) -> Result<Option<Vec<(String, String)>>, JsError> {
	let map = any_model_to_map(model);
	let label = format!("model \"{}/{}\"", model.provider(), model.id());
	resolve_headers_or_throw(
		raw_model_headers(&map, config, extension).as_deref(),
		&label,
		env,
		dummy_runner(),
	)
	.map_err(|error| JsError::new(error.message))
}

/// `resolveCompatibilityRequestConfig` (env via `process.env` in TS; the
/// harness supplies it here).
pub fn resolve_compatibility_request_config(
	model: &AnyModel,
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
	env: Option<&Env>,
) -> Result<CompatibilityRequestConfig, JsError> {
	let map = any_model_to_map(model);
	let label = format!("model \"{}/{}\"", model.provider(), model.id());
	let configured = merge_header_vecs(
		configured_headers(config, extension).as_deref(),
		raw_model_headers(&map, config, extension).as_deref(),
	);
	let configured = resolve_headers_or_throw(
		Some(configured.as_slice()),
		&label,
		env,
		dummy_runner(),
	)
	.map_err(|error| JsError::new(error.message))?;
	let model_headers = model
		.to_value()
		.get("headers")
		.and_then(value_headers);
	let headers = if model_headers.is_some() || configured.is_some() {
		Some(merge_header_vecs(model_headers.as_deref(), configured.as_deref()))
	} else {
		None
	};
	let auth_header = extension
		.and_then(|e| e.auth_header)
		.or_else(|| config.and_then(|c| c.get("authHeader")).and_then(Value::as_bool))
		.unwrap_or(false);
	Ok(CompatibilityRequestConfig { headers, auth_header })
}

/// `configuredRequestAuthStatus` (env via `process.env` in TS; injected here).
pub fn configured_request_auth_status(
	config: Option<&Value>,
	extension: Option<&ProviderConfigInput>,
	env: Option<&Env>,
) -> Option<AuthStatus> {
	let value = configured_api_key(config, extension)?;
	if is_command_config_value(&value) {
		return Some(AuthStatus {
			configured: true,
			source: Some("models_json_command".to_string()),
			label: None,
		});
	}
	let names = get_config_value_env_var_names(&value);
	if !names.is_empty() {
		return if is_config_value_configured(&value, env) {
			Some(AuthStatus {
				configured: true,
				source: Some("environment".to_string()),
				label: Some(names.join(", ")),
			})
		} else {
			Some(AuthStatus { configured: false, source: None, label: None })
		};
	}
	Some(AuthStatus {
		configured: true,
		source: Some(if extension.and_then(|e| e.api_key.clone()).is_some() {
			"fallback".to_string()
		} else {
			"models_json_key".to_string()
		}),
		label: None,
	})
}
