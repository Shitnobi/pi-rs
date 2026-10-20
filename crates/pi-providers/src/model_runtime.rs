//! `ModelRuntime` — port of `coding-agent` `src/core/model-runtime.ts`.
//!
//! The configured pi-ai Models collection used by coding-agent and SDK
//! consumers: builtin catalog + models.json + extension/virtual overlays,
//! the availability snapshot machine, credential synchronization, and
//! request dispatch through `prepareRequest`.
//!
//! G8 sync decisions and documented divergences (aligned with
//! `pi_ai::models_impl`):
//! - Every Promise-valued TS operation is sync; error channels are
//!   `Result<_, JsError>`. The availability refresh "queue" and the
//!   credential-operation serialization queue collapse to inline runs
//!   (single-threaded callers cannot interleave); the seq counters and
//!   their guards are kept because `getError` observes them.
//! - `PI_OFFLINE` / auth env come from an injected `Env` map — the library
//!   never reads `std::env` (spec rule). `modelNetworkEnabled` =
//!   `!env.contains_key("PI_OFFLINE")` (TS `process.env.PI_OFFLINE ===
//!   undefined`: presence, even empty, disables network).
//! - `modelsPath` is injected: TS defaults it to the agent-dir
//!   `models.json`; the Rust port takes `None` as the TS `null`
//!   (no models.json) and requires the harness to pass the default path
//!   explicitly. Same for `authPath` (required unless `credentials` is
//!   injected).
//! - Builtin providers are constructed from `pi_ai::builtin_catalog()`.
//!   The catalog carries models only, so: provider display names are a
//!   vendored table (from the TS `providers/*.ts` `createProvider` calls);
//!   builtin api-key auth is a generic stored-credential + env-var auth
//!   (env var table from `pi-ai` `env-api-keys.ts`); anthropic's
//!   `ANTHROPIC_AUTH_TOKEN` Bearer path is kept, federation/ADC/Bedrock
//!   ambient sources are not (async phase). Builtin `stream`/`streamSimple`
//!   emit a setup-error stream (network clients are the async phase).
//! - Catalog network refresh uses injected `catalog_fetch` /
//!   `personalize_catalog` closures (TS `fetchWithRetry` /
//!   `personalizeOpenRouterCatalog`); without injection, network refresh
//!   fails per provider. The `personalize` default is the identity.
//! - `modelRefreshTimeoutMs` cannot fire mid-operation sync — the initial
//!   refresh runs with the caller's signal only (timeout vacuous).
//! - Publication `update` closures are vacuous (the sync drivers apply
//!   overlay state inline, like `models_impl`); `persist` still goes
//!   through `context.publish`.
//! - `options.transformHeaders` cannot ride in JSON option records (see
//!   `models_impl`); `generateImages`/`classify` error results always
//!   report `aborted: false`; `Date.now()` is an injectable clock
//!   (`now_ms`, default `pi_core::ai::date_now_millis`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use indexmap::{IndexMap, IndexSet};
use pi_core::ai::date_now_millis;
use pi_tools::shell::AbortSignal;
use serde_json::{json, Map, Value};

use crate::auth_storage::AuthStorage;
use crate::config_value::{CommandRunner, ConfigValueResolver, Env};
use crate::error::JsError;
use crate::model_config::ModelConfig;
use crate::models_store::{FileModelsStore, InMemoryCodingAgentModelsStore};
use crate::provider_composer::{
	compose_model_provider_shared, configured_request_auth_status,
	resolve_compatibility_request_config, resolve_configured_model_headers,
	validate_extension_provider, CompatibilityRequestConfig, ProviderConfigInput,
};
use crate::remote_catalog::{
	refresh_with_personalized_catalog, refresh_with_remote_catalog, remote_catalog_get_all_models,
	remote_catalog_get_models, CatalogRequest, CatalogResponse, PublishRecord, StoredCatalog,
	DEFAULT_CATALOG_BASE_URL,
};
use crate::runtime_credentials::RuntimeCredentials;
use crate::virtual_models::{
	create_virtual_model, find_latest_response, model_is_virtual,
	with_virtual_models_shared, FailedRouteRequest, ModelRoute, ModelRouteReason,
	ModelRouteRequest, RouteFn, VirtualModelDefinition,
};

use crate::pi_ai::credential::{
	ApiKeyAuth, ApiKeyCredential, AuthCheck, AuthContext, AuthOperationOptions, AuthResult,
	AuthType, Credential, CredentialInfo, CredentialStore, ModelAuth, OAuthAuth,
	OAuthCredential, ProviderAuth,
};
use crate::pi_ai::lazy::lazy_stream;
use crate::pi_ai::model::{
	AnyModel, ClassifierModel, ImageModel, Model, ModelThinkingLevel, ProviderEnv, ProviderHeaders,
};
use crate::pi_ai::model_operations::{
	assert_chat_model, assert_classifier_input_supported, assert_classifier_model,
	assert_image_model, classifier_error_result, image_error_result,
};
use crate::pi_ai::models::{clamp_thinking_level, is_model_type, Models, ModelType, MutableModels, RefreshModelsContext};
use crate::pi_ai::models_error::{ModelsError, ModelsErrorKind};
use crate::pi_ai::models_impl::{
	create_models, merge_headers, stream_terminal_result, CreateModelsOptions, ModelsImpl,
};
use crate::pi_ai::models_store::{ModelsStore, ModelsStoreEntry};
use crate::pi_ai::provider::{
	AssistantImages, AssistantMessage, AssistantMessageEventStream, ClassifierContext,
	ClassifierResult, DeferredHandle, ImagesContext, Provider, TranscriptContext,
};
use crate::pi_ai::transcript::normalize_context;

// ============================================================================
// Credential synchronization error
// ============================================================================

/// TS `CredentialSynchronizationOperation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSynchronizationOperation {
	Login,
	Logout,
	SetRuntimeApiKey,
	RemoveRuntimeApiKey,
}

impl CredentialSynchronizationOperation {
	pub fn as_str(&self) -> &'static str {
		match self {
			CredentialSynchronizationOperation::Login => "login",
			CredentialSynchronizationOperation::Logout => "logout",
			CredentialSynchronizationOperation::SetRuntimeApiKey => "setRuntimeApiKey",
			CredentialSynchronizationOperation::RemoveRuntimeApiKey => "removeRuntimeApiKey",
		}
	}
}

/// Credentials changed successfully, but the local model/auth snapshot
/// could not be synchronized (TS `CredentialSynchronizationError`).
#[derive(Debug, Clone)]
pub struct CredentialSynchronizationError {
	pub provider_id: String,
	pub operation: CredentialSynchronizationOperation,
	pub credential: Option<Credential>,
	pub cause: JsError,
}

impl CredentialSynchronizationError {
	pub fn message(&self) -> String {
		format!(
			"Credential {} committed for {}, but local synchronization failed",
			self.operation.as_str(),
			self.provider_id
		)
	}

	/// The layer's error channel: message plus the chained cause.
	pub fn into_js_error(self) -> JsError {
		JsError::with_cause(self.message(), self.cause)
	}
}

// ============================================================================
// Injected clock / env context / shared credentials
// ============================================================================

/// Injectable `Date.now()` (`None` → ambient `date_now_millis`).
#[derive(Clone)]
pub(crate) struct Clock(Option<f64>);

impl Clock {
	/// The current time in milliseconds.
	pub(crate) fn now(&self) -> f64 {
		self.0.unwrap_or_else(date_now_millis)
	}
}

/// Env-backed `AuthContext` over an injected map (never `std::env`).
pub struct EnvMapAuthContext {
	pub env: Env,
}

impl AuthContext for EnvMapAuthContext {
	fn env(&self, name: &str) -> Option<String> {
		self.env.get(name).cloned()
	}

	fn file_exists(&self, _path: &str) -> bool {
		false
	}
}

/// A `RuntimeCredentials` handle shared between `ModelRuntime` and the
/// owned `ModelsImpl` (TS passes the same object to `createModels`).
pub struct SharedCredentials {
	inner: Arc<Mutex<RuntimeCredentials>>,
}

impl SharedCredentials {
	pub fn handle(&self) -> Arc<Mutex<RuntimeCredentials>> {
		Arc::clone(&self.inner)
	}
}

impl CredentialStore for SharedCredentials {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError> {
		self.inner.lock().unwrap_or_else(|p| p.into_inner()).read(provider_id, options)
	}

	fn list(&self, options: Option<&AuthOperationOptions>) -> Result<Vec<CredentialInfo>, JsError> {
		self.inner.lock().unwrap_or_else(|p| p.into_inner()).list(options)
	}

	fn modify(
		&mut self,
		provider_id: &str,
		f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError> {
		self.inner.lock().unwrap_or_else(|p| p.into_inner()).modify(provider_id, f, options)
	}

	fn delete(&mut self, provider_id: &str, options: Option<&AuthOperationOptions>) -> Result<(), JsError> {
		self.inner.lock().unwrap_or_else(|p| p.into_inner()).delete(provider_id, options)
	}
}

/// No-op command runner: `!command` config values stay unresolved in this
/// port (the TS executes them through the shell; parity harnesses script
/// auth at a higher level).
struct NoopCommandRunner;

impl CommandRunner for NoopCommandRunner {
	fn run(&self, _command: &str) -> Option<String> {
		None
	}
}

// ============================================================================
// Builtin provider catalog
// ============================================================================

/// Provider display names (vendored from the TS `providers/*.ts`
/// `createProvider({ name })` calls; the catalog JSON carries models only).
fn provider_display_name(provider_id: &str) -> String {
	let name = match provider_id {
		"amazon-bedrock" => "Amazon Bedrock",
		"ant-ling" => "Ant Ling",
		"anthropic" => "Anthropic",
		"azure" => "Azure",
		"azure-openai-responses" => "Azure",
		"baseten" => "Baseten",
		"cerebras" => "Cerebras",
		"cloudflare-ai-gateway" => "Cloudflare AI Gateway",
		"cloudflare-workers-ai" => "Cloudflare Workers AI",
		"deepseek" => "DeepSeek",
		"fireworks" => "Fireworks",
		"github-copilot" => "GitHub Copilot",
		"google" => "Google",
		"google-vertex" => "Google Vertex AI",
		"groq" => "Groq",
		"huggingface" => "Hugging Face",
		"kimi-coding" => "Kimi For Coding",
		"meta" => "Meta",
		"minimax" => "MiniMax",
		"minimax-cn" => "MiniMax CN",
		"mistral" => "Mistral",
		"moonshotai" => "Moonshot AI",
		"moonshotai-cn" => "Moonshot AI CN",
		"nvidia" => "NVIDIA",
		"openai" => "OpenAI",
		"openai-codex" => "OpenAI Codex",
		"opencode" => "OpenCode",
		"opencode-go" => "OpenCode",
		"openrouter" => "OpenRouter",
		"qwen-token-plan" => "Qwen Token Plan",
		"qwen-token-plan-cn" => "Qwen Token Plan CN",
		"qwen-token-plan-individual" => "Qwen Token Plan Individual",
		"radius" => "Radius",
		"together" => "Together",
		"typesafe" => "TypeSafe",
		"vercel-ai-gateway" => "Vercel AI Gateway",
		"xai" => "xAI",
		"xiaomi" => "Xiaomi",
		"xiaomi-token-plan-ams" => "Xiaomi Token Plan AMS",
		"xiaomi-token-plan-cn" => "Xiaomi Token Plan CN",
		"xiaomi-token-plan-sgp" => "Xiaomi Token Plan SGP",
		"zai" => "Z.AI",
		"zai-coding-cn" => "Z.AI Coding CN",
		other => other,
	};
	name.to_string()
}

/// API-key env var names per provider (vendored from pi-ai
/// `env-api-keys.ts`; the anthropic Bearer token participates in status
/// but is applied as a header, like the TS).
fn api_key_env_vars(provider_id: &str) -> Vec<&'static str> {
	match provider_id {
		"github-copilot" => vec!["COPILOT_GITHUB_TOKEN"],
		"anthropic" => vec!["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],
		"ant-ling" => vec!["ANT_LING_API_KEY"],
		"qwen-token-plan" | "qwen-token-plan-individual" => vec!["QWEN_TOKEN_PLAN_API_KEY"],
		"qwen-token-plan-cn" => vec!["QWEN_TOKEN_PLAN_CN_API_KEY"],
		"openai" => vec!["OPENAI_API_KEY"],
		"azure" => vec!["AZURE_OPENAI_API_KEY"],
		"nvidia" => vec!["NVIDIA_API_KEY"],
		"deepseek" => vec!["DEEPSEEK_API_KEY"],
		"google" => vec!["GEMINI_API_KEY"],
		"google-vertex" => vec!["GOOGLE_CLOUD_API_KEY"],
		"groq" => vec!["GROQ_API_KEY"],
		"cerebras" => vec!["CEREBRAS_API_KEY"],
		"xai" => vec!["XAI_API_KEY"],
		"typesafe" => vec!["TYPESAFE_API_KEY"],
		"radius" => vec!["RADIUS_API_KEY"],
		"openrouter" => vec!["OPENROUTER_API_KEY"],
		"vercel-ai-gateway" => vec!["AI_GATEWAY_API_KEY"],
		"zai" => vec!["ZAI_API_KEY"],
		"zai-coding-cn" => vec!["ZAI_CODING_CN_API_KEY"],
		"mistral" => vec!["MISTRAL_API_KEY"],
		"minimax" => vec!["MINIMAX_API_KEY"],
		"minimax-cn" => vec!["MINIMAX_CN_API_KEY"],
		"moonshotai" | "moonshotai-cn" => vec!["MOONSHOT_API_KEY"],
		"huggingface" => vec!["HF_TOKEN"],
		"fireworks" => vec!["FIREWORKS_API_KEY"],
		"together" => vec!["TOGETHER_API_KEY"],
		"baseten" => vec!["BASETEN_API_KEY"],
		"opencode" | "opencode-go" => vec!["OPENCODE_API_KEY"],
		"kimi-coding" => vec!["KIMI_API_KEY"],
		"meta" => vec!["META_API_KEY"],
		"cloudflare-workers-ai" | "cloudflare-ai-gateway" => vec!["CLOUDFLARE_API_KEY"],
		"xiaomi" => vec!["XIAOMI_TOKEN_PLAN_API_KEY"].into_iter().map(|_| "XIAOMI_API_KEY").collect(),
		"xiaomi-token-plan-cn" => vec!["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
		"xiaomi-token-plan-ams" => vec!["XIAOMI_TOKEN_PLAN_AMS_API_KEY"],
		"xiaomi-token-plan-sgp" => vec!["XIAOMI_TOKEN_PLAN_SGP_API_KEY"],
		_ => Vec::new(),
	}
}

/// Generic builtin api-key auth: stored credential first, then the
/// provider's env vars (anthropic's auth token becomes a Bearer header).
struct BuiltinApiKeyAuth {
	name: String,
	env_vars: Vec<&'static str>,
	bearer_env_var: Option<&'static str>,
}

impl BuiltinApiKeyAuth {
	fn new(provider_id: &str) -> BuiltinApiKeyAuth {
		let env_vars = api_key_env_vars(provider_id);
		let bearer_env_var =
			(provider_id == "anthropic").then_some("ANTHROPIC_AUTH_TOKEN");
		BuiltinApiKeyAuth { name: format!("{} API key", provider_display_name(provider_id)), env_vars, bearer_env_var }
	}
}

impl ApiKeyAuth for BuiltinApiKeyAuth {
	fn name(&self) -> &str {
		&self.name
	}

	fn has_check(&self) -> bool {
		false // TS builtin auths define no `check`; Models falls back to resolve.
	}

	fn check(
		&self,
		_ctx: &dyn AuthContext,
		_credential: Option<&ApiKeyCredential>,
		_signal: Option<&AbortSignal>,
	) -> Result<Option<AuthCheck>, JsError> {
		Ok(None)
	}

	fn resolve(
		&self,
		ctx: &dyn AuthContext,
		credential: Option<&ApiKeyCredential>,
		_signal: Option<&AbortSignal>,
	) -> Result<Option<AuthResult>, JsError> {
		if let Some(credential) = credential {
			if let Some(key) = credential.key.as_deref().filter(|k| !k.is_empty()) {
				return Ok(Some(AuthResult {
					auth: model_auth_from_parts(Some(key.to_string()), None, None),
					env: credential.env.clone(),
					source: Some("stored credential".to_string()),
				}));
			}
		}
		if let Some(bearer_var) = self.bearer_env_var {
			if let Some(token) = ctx.env(bearer_var).filter(|t| !t.is_empty()) {
				let mut headers = ProviderHeaders::new();
				headers.insert("Authorization".to_string(), Some(format!("Bearer {token}")));
				return Ok(Some(AuthResult {
					auth: model_auth_from_parts(None, Some(headers), None),
					env: None,
					source: Some(bearer_var.to_string()),
				}));
			}
		}
		for env_var in &self.env_vars {
			if Some(*env_var) == self.bearer_env_var {
				continue;
			}
			if let Some(key) = ctx.env(env_var).filter(|k| !k.is_empty()) {
				return Ok(Some(AuthResult {
					auth: model_auth_from_parts(Some(key), None, None),
					env: None,
					source: Some(env_var.to_string()),
				}));
			}
		}
		Ok(None)
	}
}

/// Radius OAuth marker (TS `lazyOAuth({ name, load: loadRadiusOAuth })`;
/// the real token exchange is the async phase — `to_auth` derives Bearer
/// auth from a stored credential).
struct RadiusOAuthAuth {
	name: String,
}

impl OAuthAuth for RadiusOAuthAuth {
	fn name(&self) -> &str {
		&self.name
	}

	fn is_subscription(&self) -> bool {
		true
	}

	fn login_label(&self) -> Option<&str> {
		None
	}

	fn to_auth(&self, credential: &OAuthCredential) -> Result<ModelAuth, JsError> {
		let mut headers = ProviderHeaders::new();
		headers.insert(
			"Authorization".to_string(),
			Some(format!("Bearer {}", credential.access)),
		);
		Ok(model_auth_from_parts(None, Some(headers), None))
	}
}

/// `ModelAuth` from parts (ordered JSON form: apiKey?, headers?, baseUrl?).
fn model_auth_from_parts(
	api_key: Option<String>,
	headers: Option<ProviderHeaders>,
	base_url: Option<String>,
) -> ModelAuth {
	let mut map = Map::new();
	if let Some(api_key) = api_key {
		map.insert("apiKey".to_string(), json!(api_key));
	}
	if let Some(headers) = headers {
		let mut out = Map::new();
		for (name, value) in headers {
			out.insert(name.clone(), value.clone().map(Value::String).unwrap_or(Value::Null));
		}
		map.insert("headers".to_string(), Value::Object(out));
	}
	if let Some(base_url) = base_url {
		map.insert("baseUrl".to_string(), json!(base_url));
	}
	ModelAuth::from_map(map).expect("well-formed ModelAuth")
}

/// `Record<string, string | null>` option field → `ProviderHeaders`.
fn json_headers(options: Option<&Value>, key: &str) -> Option<ProviderHeaders> {
	let map = options?.get(key)?.as_object()?;
	let mut out = ProviderHeaders::new();
	for (k, v) in map {
		if v.is_null() {
			out.insert(k.clone(), None);
		} else if let Some(s) = v.as_str() {
			out.insert(k.clone(), Some(s.to_string()));
		}
	}
	Some(out)
}

/// `Record<string, string>` option field → `ProviderEnv`.
fn json_env(options: Option<&Value>, key: &str) -> Option<ProviderEnv> {
	let map = options?.get(key)?.as_object()?;
	Some(
		map.iter()
			.filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
			.collect(),
	)
}

/// Header pairs → `ProviderHeaders` (every value `Some`).
fn header_pairs_to_provider_headers(pairs: &[(String, String)]) -> ProviderHeaders {
	pairs.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect()
}

/// Minimal ISO-8601 (`YYYY-MM-DDTHH:MM:SS(.sss)?Z`) → epoch ms, for the
/// catalog's `generatedAt` stamp (TS `Date.parse`).
fn iso_to_epoch_ms(s: &str) -> Option<f64> {
	let bytes = s.as_bytes();
	if bytes.len() < 20 {
		return None;
	}
	let num = |from: usize, len: usize| -> Option<f64> {
		s.get(from..from + len)?.parse::<f64>().ok()
	};
	let year = num(0, 4)? as i64;
	let month = num(5, 2)? as i64;
	let day = num(8, 2)? as i64;
	let hour = num(11, 2)? as i64;
	let minute = num(14, 2)? as i64;
	let second = num(17, 2)? as i64;
	let mut millis = 0.0;
	if bytes.len() > 19 && (bytes[19] == b'.' || bytes[19] == b',') {
		let rest = &s[20..];
		let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
		if !digits.is_empty() {
			millis = format!("0.{digits}").parse::<f64>().unwrap_or(0.0) * 1000.0;
		}
	}
	// Days from civil (Howard Hinnant's algorithm).
	let y = if month <= 2 { year - 1 } else { year };
	let era = y.div_euclid(400);
	let yoe = y - era * 400;
	let mp = (month + 9) % 12;
	let doy = (153 * mp + 2) / 5 + day - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	let days = era * 146097 + doe - 719468;
	Some(((days * 24 + hour) * 3600 + minute * 60 + second) as f64 * 1000.0 + millis)
}

/// The scripted catalog fetch (TS `fetchWithRetry`); default refuses
/// network (parity harnesses inject a lazy scripted fetch).
pub type CatalogFetchFn =
	Arc<dyn Fn(&CatalogRequest) -> Result<CatalogResponse, JsError> + Send + Sync>;

/// The openrouter catalog personalizer (TS `personalizeOpenRouterCatalog`).
pub type PersonalizeCatalogFn =
	Arc<dyn Fn(&[Value]) -> Result<Vec<Value>, JsError> + Send + Sync>;

fn unavailable_fetch() -> CatalogFetchFn {
	Arc::new(|_request: &CatalogRequest| {
		Err(JsError::new("Model catalog network access is not available in this port"))
	})
}

/// A setup-error stream for builtin stream operations (network clients are
/// the async phase).
fn builtin_unavailable_stream(
	model: &Model,
	_context: &TranscriptContext,
	_options: Option<&serde_json::Value>,
) -> AssistantMessageEventStream {
	let error = JsError::new(format!(
		"Provider {} streaming is not available in this port",
		model.provider
	));
	crate::pi_ai::lazy::setup_error_stream(&crate::pi_ai::lazy::create_setup_error_message(
		model,
		&error,
		date_now_millis(),
	))
}

/// Build the builtin provider list from the vendored catalog, applying the
/// TS `builtinProviders()` wrappers (radius pass-through, openrouter
/// personalized, others remote-catalog).
fn builtin_providers(
	catalog_base_url: &str,
	generated_at: Option<f64>,
	fetch: CatalogFetchFn,
	personalize: Option<PersonalizeCatalogFn>,
	now: Clock,
) -> Vec<Provider> {
	let catalog = crate::pi_ai::builtin_catalog();
	let providers = catalog
		.get("providers")
		.and_then(Value::as_object)
		.expect("builtin catalog providers");
	let mut out = Vec::new();
	for (provider_id, apis) in providers {
		let apis = apis.as_object().expect("catalog api map");
		let mut all_values: Vec<Value> = Vec::new();
		for (_api, models) in apis {
			for (_key, model) in models.as_object().expect("catalog models") {
				all_values.push(model.clone());
			}
		}
		let builtin_all: Vec<AnyModel> =
			all_values.iter().filter_map(|m| serde_json::from_value(m.clone()).ok()).collect();
		let builtin_chat: Vec<Value> = builtin_all
			.iter()
			.filter(|m| is_model_type(m, &ModelType::Chat))
			.map(|m| m.to_value())
			.collect();
		let builtin_all_values: Vec<Value> = builtin_all.iter().map(|m| m.to_value()).collect();
		let chat_models: Arc<Vec<Model>> = Arc::new(
			builtin_chat
				.iter()
				.filter_map(|m| serde_json::from_value(m.clone()).ok())
				.collect(),
		);
		let base_url = all_values
			.iter()
			.find_map(|m| m.get("baseUrl").and_then(Value::as_str))
			.map(str::to_string);
		let auth = ProviderAuth {
			api_key: Some(Arc::new(BuiltinApiKeyAuth::new(provider_id))),
			oauth: None,
		};
		let dynamic: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let state_dynamic = Arc::clone(&dynamic);
		// `getModels` merges the remote-catalog overlay (TS
		// `withRemoteCatalog` wraps the provider's own model list); radius
		// is a pass-through with a static list.
		let get_models: crate::pi_ai::provider::GetModelsFn = if provider_id == "radius" {
			let chat_models = Arc::clone(&chat_models);
			Box::new(move || chat_models.to_vec())
		} else {
			let builtin_chat = builtin_chat.clone();
			let dynamic = Arc::clone(&dynamic);
			Box::new(move || {
				let overlay = dynamic.lock().unwrap_or_else(|p| p.into_inner()).clone();
				remote_catalog_get_models(&builtin_chat, &overlay)
					.into_iter()
					.filter_map(|m| serde_json::from_value(m).ok())
					.collect()
			})
		};
		let mut provider = Provider::new(
			provider_id.clone(),
			provider_display_name(provider_id),
			auth,
			get_models,
			Box::new(builtin_unavailable_stream),
			Box::new(builtin_unavailable_stream),
		)
		.with_base_url(base_url);

		if provider_id == "radius" {
			out.push(provider);
			continue;
		}

		provider = provider
			.with_get_all_models({
				let builtin_all_values = builtin_all_values.clone();
				let dynamic = Arc::clone(&dynamic);
				Box::new(move || {
					let overlay = dynamic.lock().unwrap_or_else(|p| p.into_inner()).clone();
					remote_catalog_get_all_models(&builtin_all_values, &overlay)
						.into_iter()
						.filter_map(|m| serde_json::from_value(m).ok())
						.collect()
				})
			})
			.with_refresh_models({
				let provider_id = provider_id.clone();
				let personalize = personalize.clone();
				let catalog_base_url = catalog_base_url.to_string();
				let fetch = Arc::clone(&fetch);
				let now = now.clone();
				let builtin_all_values = builtin_all_values.clone();
				Box::new(move |context: &mut RefreshModelsContext<'_>| {
					let stored = context
						.stored
						.map(|entry| StoredCatalog::from_value(&entry.to_value()));
					let mut publishes: Vec<PublishRecord> = Vec::new();
					let signal = context.signal;
					let mut accept =
						|| !signal.is_some_and(AbortSignal::aborted);
					let result = if provider_id == "openrouter" {
						let personalize = personalize
							.as_ref()
							.unwrap_or_else(|| {
								unreachable!("openrouter personalize is always injected")
							})
							.clone();
						let mut personal = state_dynamic.lock().unwrap_or_else(|p| p.into_inner());
						refresh_with_personalized_catalog(
							&mut personal,
							&mut publishes,
							&provider_id,
							&catalog_base_url,
							&builtin_all_values,
							generated_at,
							stored.as_ref(),
							context.allow_network,
							now.now(),
							fetch.as_ref(),
							personalize.as_ref(),
							&mut accept,
						)
					} else {
						let mut dynamic = state_dynamic.lock().unwrap_or_else(|p| p.into_inner());
						refresh_with_remote_catalog(
							&mut dynamic,
							&mut publishes,
							&provider_id,
							&catalog_base_url,
							generated_at,
							stored.as_ref(),
							context.allow_network,
							context.force,
							now.now(),
							fetch.as_ref(),
							&mut accept,
						)
					};
					result?;
					for record in publishes {
						let persist = record
							.persist
							.as_ref()
							.map(ModelsStoreEntry::from_value)
							.unwrap_or(None);
						let publication = crate::pi_ai::models::ModelsPublication {
							persist: match persist {
								Some(entry) => crate::pi_ai::models::Persist::Write(entry),
								None => crate::pi_ai::models::Persist::Unchanged,
							},
							// Vacuous: the sync driver applied overlay state inline.
							update: record.update.then(|| Box::new(|| {}) as Box<dyn FnOnce() + Send>),
						};
						(context.publish)(publication)?;
					}
					Ok(())
				})
			});
		out.push(provider);
	}
	out
}

/// `builtinProviderCatalog.radiusProvider({ id, name, gateway })` — the
/// catalog radius models re-homed to a custom gateway.
fn radius_provider(id: &str, name: &str, gateway: &str) -> Provider {
	let catalog = crate::pi_ai::builtin_catalog();
	let models = catalog
		.get("providers")
		.and_then(|p| p.get("radius"))
		.and_then(|p| p.get("pi-messages"))
		.and_then(Value::as_object)
		.expect("radius catalog");
	let default_gateway = models
		.values()
		.find_map(|m| m.get("baseUrl").and_then(Value::as_str))
		.unwrap_or("")
		.to_string();
	let mut chat: Vec<Model> = Vec::new();
	for model in models.values() {
		// TS: the default gateway serves every model; a custom gateway
		// only the models it declares.
		if gateway != default_gateway
			&& model.get("baseUrl").and_then(Value::as_str) != Some(gateway)
		{
			continue;
		}
		let mut value = model.clone();
		if let Some(map) = value.as_object_mut() {
			map.insert("provider".to_string(), json!(id));
			map.insert("baseUrl".to_string(), json!(gateway));
		}
		if let Ok(parsed) = serde_json::from_value::<Model>(value) {
			chat.push(parsed);
		}
	}
	let chat = Arc::new(chat);
	Provider::new(
		id.to_string(),
		name.to_string(),
		ProviderAuth {
			api_key: Some(Arc::new(BuiltinApiKeyAuth::new(id))),
			oauth: Some(Arc::new(RadiusOAuthAuth { name: name.to_string() })),
		},
		{
			let chat = Arc::clone(&chat);
			Box::new(move || chat.to_vec())
		},
		Box::new(builtin_unavailable_stream),
		Box::new(builtin_unavailable_stream),
	)
	.with_base_url(Some(gateway.to_string()))
}

// ============================================================================
// Snapshot / options
// ============================================================================

struct RegisteredVirtualModel {
	model: Model,
	route: RouteFn,
}

/// The runtime's availability snapshot (TS `ModelRuntimeSnapshot`).
#[derive(Clone)]
pub struct ModelRuntimeSnapshot {
	pub all: Vec<Model>,
	pub available: Vec<Model>,
	pub configured_providers: IndexSet<String>,
	pub stored_providers: IndexSet<String>,
	pub auth: IndexMap<String, Option<AuthCheck>>,
}

impl Default for ModelRuntimeSnapshot {
	fn default() -> Self {
		ModelRuntimeSnapshot {
			all: Vec::new(),
			available: Vec::new(),
			configured_providers: IndexSet::new(),
			stored_providers: IndexSet::new(),
			auth: IndexMap::new(),
		}
	}
}

/// Injected catalog fetch + personalizer pair kept together for options.
#[derive(Clone)]
pub struct CatalogHooks {
	pub fetch: CatalogFetchFn,
	pub personalize: Option<PersonalizeCatalogFn>,
}

impl Default for CatalogHooks {
	fn default() -> Self {
		CatalogHooks { fetch: unavailable_fetch(), personalize: None }
	}
}

/// TS `CreateModelRuntimeOptions` (injected; no ambient env/agent-dir).
pub struct CreateModelRuntimeOptions<'a> {
	/// Credential storage. Defaults to `AuthStorage::create(auth_path)`
	/// (which requires `auth_path`).
	pub credentials: Option<Box<dyn CredentialStore>>,
	pub auth_path: Option<String>,
	/// `None` = the TS `null` (no models.json). The TS default
	/// (agent-dir models.json) is the caller's duty.
	pub models_path: Option<String>,
	pub models_store: Option<Box<dyn ModelsStore>>,
	/// Overrides the default `dirname(models_path)/models-store.json`.
	pub models_store_path: Option<String>,
	/// Allow create() to refresh model catalogs over the network.
	pub allow_model_network: bool,
	/// Timeout for the create-time refresh (vacuous sync; see module docs).
	pub model_refresh_timeout_ms: Option<f64>,
	pub catalog_base_url: Option<String>,
	/// Catalog network seams (scripted in tests/harness).
	pub catalog_hooks: CatalogHooks,
	pub signal: Option<&'a AbortSignal>,
	/// Skip the initial catalog and availability refresh.
	pub refresh_on_create: bool,
	/// Injected environment (auth env vars + `PI_OFFLINE`).
	pub env: Option<Env>,
	/// Injectable clock (`Date.now()`).
	pub now_ms: Option<f64>,
}

impl<'a> Default for CreateModelRuntimeOptions<'a> {
	fn default() -> Self {
		CreateModelRuntimeOptions {
			credentials: None,
			auth_path: None,
			models_path: None,
			models_store: None,
			models_store_path: None,
			allow_model_network: false,
			model_refresh_timeout_ms: None,
			catalog_base_url: None,
			catalog_hooks: CatalogHooks::default(),
			signal: None,
			refresh_on_create: true,
			env: None,
			now_ms: None,
		}
	}
}

/// TS `ModelRuntimeAuthOverrides` (minus `transformHeaders`-style fields
/// that cannot ride in this port).
#[derive(Default)]
pub struct ModelRuntimeAuthOverrides<'a> {
	pub api_key: Option<String>,
	pub env: Option<ProviderEnv>,
	pub min_oauth_validity_ms: Option<f64>,
	pub signal: Option<&'a AbortSignal>,
}

// ============================================================================
// ModelRuntime
// ============================================================================

/// Configured pi-ai Models collection used by coding-agent and SDK
/// consumers (TS `ModelRuntime`).
pub struct ModelRuntime {
	models: ModelsImpl<'static>,
	credentials: Arc<Mutex<RuntimeCredentials>>,
	default_builtins: IndexMap<String, Arc<Provider>>,
	builtins: IndexMap<String, Arc<Provider>>,
	native_extension_providers: IndexMap<String, Arc<Provider>>,
	extension_providers: IndexMap<String, ProviderConfigInput>,
	/// Virtual models by provider id, then model id.
	virtual_models: IndexMap<String, IndexMap<String, RegisteredVirtualModel>>,
	composition_errors: IndexMap<String, String>,
	models_path: Option<String>,
	model_network_enabled: bool,
	config: ModelConfig,
	snapshot: ModelRuntimeSnapshot,
	availability_refresh_seq: u64,
	availability_error_seq: u64,
	provider_availability_seq: HashMap<String, u64>,
	availability_error: Option<String>,
	env: Env,
	now: Clock,
}

impl ModelRuntime {
	/// TS `static async create`.
	pub fn create(options: CreateModelRuntimeOptions<'_>) -> Result<ModelRuntime, JsError> {
		let env = options.env.unwrap_or_default();
		let credentials_store: Box<dyn CredentialStore> = match options.credentials {
			Some(credentials) => credentials,
			None => {
				let auth_path = options.auth_path.clone().ok_or_else(|| {
					JsError::new("authPath is required when credentials is not provided")
				})?;
				let resolver = ConfigValueResolver::new(&NoopCommandRunner);
				let ambient = env.clone();
				Box::new(AuthStorage::create(auth_path, resolver, ambient))
			}
		};
		let credentials = RuntimeCredentials::new(credentials_store);
		// The credential store trait is not `Send + Sync` (harness stores may
		// capture non-thread-safe scripted state); access stays single-threaded.
		#[allow(clippy::arc_with_non_send_sync)]
		let credentials = Arc::new(Mutex::new(credentials));
		let shared = SharedCredentials { inner: Arc::clone(&credentials) };

		let models_path = options.models_path;
		let config = load_model_config(models_path.as_deref());
		let models_store: Box<dyn ModelsStore> = match options.models_store {
			Some(store) => store,
			None => match &models_path {
				Some(path) => {
					let store_path = options.models_store_path.clone().unwrap_or_else(|| {
						let dir = std::path::Path::new(path)
							.parent()
							.map(|p| p.to_path_buf())
							.filter(|p| !p.as_os_str().is_empty())
							.unwrap_or_else(|| std::path::PathBuf::from("."));
						dir.join("models-store.json").to_string_lossy().into_owned()
					});
					Box::new(FileModelsStore::new(store_path))
				}
				None => Box::new(InMemoryCodingAgentModelsStore::new()),
			},
		};

		let generated_at = crate::pi_ai::builtin_catalog()
			.get("generatedAt")
			.and_then(Value::as_str)
			.and_then(iso_to_epoch_ms);
		let catalog_base_url =
			options.catalog_base_url.clone().unwrap_or_else(|| DEFAULT_CATALOG_BASE_URL.to_string());
		let now = Clock(options.now_ms);
		let mut hooks = options.catalog_hooks.clone();
		if hooks.personalize.is_none() {
			hooks.personalize = Some(Arc::new(|catalog: &[Value]| Ok(catalog.to_vec())));
		}
		let providers = builtin_providers(
			&catalog_base_url,
			generated_at,
			Arc::clone(&hooks.fetch),
			hooks.personalize.clone(),
			now.clone(),
		);

		let mut models = create_models(CreateModelsOptions {
			credentials: Some(Box::new(shared)),
			models_store: Some(models_store),
			auth_context: Some(Box::new(EnvMapAuthContext { env: env.clone() })),
		});
		if let Some(now_ms) = options.now_ms {
			models = models.with_now_ms(now_ms);
		}

		let mut runtime = ModelRuntime {
			models,
			credentials,
			default_builtins: providers.into_iter().map(|p| (p.id.clone(), Arc::new(p))).collect(),
			builtins: IndexMap::new(),
			native_extension_providers: IndexMap::new(),
			extension_providers: IndexMap::new(),
			virtual_models: IndexMap::new(),
			composition_errors: IndexMap::new(),
			models_path,
			model_network_enabled: !env.contains_key("PI_OFFLINE"),
			config,
			snapshot: ModelRuntimeSnapshot::default(),
			availability_refresh_seq: 0,
			availability_error_seq: 0,
			provider_availability_seq: HashMap::new(),
			availability_error: None,
			env,
			now,
		};
		runtime.builtins = runtime.default_builtins.clone();
		runtime.rebuild_providers();
		if options.refresh_on_create {
			let refresh_from_network =
				runtime.model_network_enabled && options.allow_model_network;
			let refresh_options = crate::pi_ai::models::ModelsRefreshOptions {
				allow_network: Some(refresh_from_network),
				providers: None,
				force: false,
				signal: options.signal,
			};
			runtime.refresh(Some(&refresh_options))?;
		}
		Ok(runtime)
	}

	/// Deterministic clock accessor.
	pub fn now_ms(&self) -> f64 {
		self.now.now()
	}

	// -- provider composition ------------------------------------------------

	/// TS `configureRadiusProviders`.
	fn configure_radius_providers(&mut self) {
		self.builtins = self.default_builtins.clone();
		for provider_id in self.config.get_provider_ids() {
			let Some(config) = self.config.get_provider(provider_id) else { continue };
			let is_radius = config.get("oauth").and_then(Value::as_str) == Some("radius");
			let Some(base_url) = config.get("baseUrl").and_then(Value::as_str) else {
				if is_radius {
					continue;
				}
				continue;
			};
			if !is_radius {
				continue;
			}
			let name = config
				.get("name")
				.and_then(Value::as_str)
				.unwrap_or(provider_id)
				.to_string();
			let gateway = base_url
				.strip_suffix("/v1/")
				.or_else(|| base_url.strip_suffix("/v1"))
				.unwrap_or(base_url)
				.to_string();
			self.builtins.insert(
				provider_id.to_string(),
				Arc::new(radius_provider(provider_id, &name, &gateway)),
			);
		}
	}

	/// TS `providerIds` — union in order: builtins, native extension,
	/// config, extension, virtual.
	fn provider_ids(&self) -> Vec<String> {
		let mut ids: IndexSet<String> = IndexSet::new();
		for id in self.builtins.keys() {
			ids.insert(id.clone());
		}
		for id in self.native_extension_providers.keys() {
			ids.insert(id.clone());
		}
		for id in self.config.get_provider_ids() {
			ids.insert(id.to_string());
		}
		for id in self.extension_providers.keys() {
			ids.insert(id.clone());
		}
		for id in self.virtual_models.keys() {
			ids.insert(id.clone());
		}
		ids.into_iter().collect()
	}

	/// TS `recomposeProvider` — returns the provider without virtual models.
	fn recompose_provider(&mut self, provider_id: &str) -> Option<()> {
		let provider = self.compose_provider(provider_id);
		let virtual_models: Vec<Model> = self
			.virtual_models
			.get(provider_id)
			.map(|models| models.values().map(|entry| entry.model.clone()).collect())
			.unwrap_or_default();
		match provider {
			Some(provider) => {
				if !virtual_models.is_empty() {
					self.models.set_provider_shared(Arc::new(with_virtual_models_shared(
						provider_id,
						Some(provider),
						virtual_models,
					)));
				} else {
					self.models.set_provider_shared(provider);
				}
				Some(())
			}
			None => {
				// TS: virtual models are set even when the provider itself is
				// undefined (a provider of only virtual models).
				if !virtual_models.is_empty() {
					self.models.set_provider_shared(Arc::new(with_virtual_models_shared(
						provider_id,
						None,
						virtual_models,
					)));
				} else {
					self.models.delete_provider(provider_id);
				}
				None
			}
		}
	}

	/// TS `composeProvider` — the provider without virtual models, or
	/// `None` when nothing defines it.
	fn compose_provider(&mut self, provider_id: &str) -> Option<Arc<Provider>> {
		let base = self
			.native_extension_providers
			.get(provider_id)
			.cloned()
			.or_else(|| self.builtins.get(provider_id).cloned());
		let extension = self.extension_providers.get(provider_id).cloned();
		if self.config.get_provider(provider_id).is_none() && extension.is_none() {
			// No overlays: use the builtin untouched.
			self.composition_errors.shift_remove(provider_id);
			return base;
		}
		match compose_model_provider_shared(
			provider_id,
			base.clone(),
			&self.config,
			extension,
		) {
			Ok(provider) => {
				self.composition_errors.shift_remove(provider_id);
				Some(Arc::new(provider))
			}
			Err(error) => {
				self.composition_errors.insert(provider_id.to_string(), error.message);
				base
			}
		}
	}

	/// TS `rebuildProviders`.
	fn rebuild_providers(&mut self) {
		self.models.clear_providers();
		self.composition_errors.clear();
		for provider_id in self.provider_ids() {
			self.recompose_provider(&provider_id);
		}
		self.update_model_snapshot();
	}

	/// TS `updateModelSnapshot`.
	fn update_model_snapshot(&mut self) {
		let all = self.models.get_models(None);
		let available = all
			.iter()
			.filter(|model| self.snapshot.configured_providers.contains(&model.provider))
			.cloned()
			.collect();
		self.snapshot.all = all;
		self.snapshot.available = available;
	}

	// -- availability machine ------------------------------------------------

	/// TS `runAvailabilityRefresh` (sync; the seq guard is kept).
	fn run_availability_refresh(
		&mut self,
		seq: u64,
		error_seq: u64,
		signal: Option<&AbortSignal>,
	) -> Result<(), JsError> {
		let providers: Vec<String> = self.models.get_providers().iter().map(|p| p.id.clone()).collect();
		let options = AuthOperationOptions { signal };
		let available = self.models.get_available(None, Some(&options))?;
		let mut checks: Vec<(String, Option<AuthCheck>)> = Vec::new();
		for provider_id in &providers {
			checks.push((provider_id.clone(), self.models.check_auth(provider_id, Some(&options))?));
		}
		let credentials = {
			let store = self.credentials.lock().unwrap_or_else(|p| p.into_inner());
			store.list(Some(&options))?
		};
		if seq != self.availability_refresh_seq {
			return Ok(());
		}
		let mut auth = IndexMap::new();
		let mut configured_providers = IndexSet::new();
		for (provider_id, check) in checks {
			if check.is_some() {
				configured_providers.insert(provider_id.clone());
			}
			auth.insert(provider_id, check);
		}
		let stored_providers: IndexSet<String> =
			credentials.into_iter().map(|entry| entry.provider_id).collect();
		self.snapshot = ModelRuntimeSnapshot {
			all: self.models.get_models(None),
			available,
			configured_providers,
			stored_providers,
			auth,
		};
		if error_seq == self.availability_error_seq {
			self.availability_error = None;
		}
		Ok(())
	}

	/// TS `queueAvailabilityRefresh` (runs inline; errors recorded + rethrown).
	fn queue_availability_refresh(&mut self, signal: Option<&AbortSignal>) -> Result<(), JsError> {
		self.availability_refresh_seq += 1;
		let seq = self.availability_refresh_seq;
		for seq_value in self.provider_availability_seq.values_mut() {
			*seq_value += 1;
		}
		self.availability_error_seq += 1;
		let error_seq = self.availability_error_seq;
		self.run_availability_refresh(seq, error_seq, signal).inspect_err(|error| {
			if error_seq == self.availability_error_seq && !signal.is_some_and(AbortSignal::aborted) {
				self.availability_error = Some(error.message.clone());
			}
		})
	}

	/// TS `refreshProviderAvailability` — per-provider snapshot merge.
	fn refresh_provider_availability(
		&mut self,
		provider_id: &str,
		signal: Option<&AbortSignal>,
	) -> Result<(), JsError> {
		self.availability_refresh_seq += 1;
		let provider_seq = self.provider_availability_seq.get(provider_id).copied().unwrap_or(0) + 1;
		self.provider_availability_seq.insert(provider_id.to_string(), provider_seq);
		self.availability_error_seq += 1;
		let error_seq = self.availability_error_seq;
		let result = (|| -> Result<(), JsError> {
			let options = AuthOperationOptions { signal };
			let available = self.models.get_available(Some(provider_id), Some(&options))?;
			let auth = self.models.check_auth(provider_id, Some(&options))?;
			let credential = {
				let store = self.credentials.lock().unwrap_or_else(|p| p.into_inner());
				store.read(provider_id, Some(&options))?
			};
			if signal.is_some_and(AbortSignal::aborted) {
				return Err(JsError::abort());
			}
			if self.provider_availability_seq.get(provider_id) != Some(&provider_seq) {
				return Ok(());
			}
			let mut configured_providers = self.snapshot.configured_providers.clone();
			let mut stored_providers = self.snapshot.stored_providers.clone();
			let mut auth_by_provider = self.snapshot.auth.clone();
			if let Some(auth) = auth {
				configured_providers.insert(provider_id.to_string());
				auth_by_provider.insert(provider_id.to_string(), Some(auth));
			} else {
				configured_providers.shift_remove(provider_id);
				auth_by_provider.shift_remove(provider_id);
			}
			if credential.is_some() {
				stored_providers.insert(provider_id.to_string());
			} else {
				stored_providers.shift_remove(provider_id);
			}
			let all = self.models.get_models(None);
			let mut available_by_id: IndexMap<String, Model> = IndexMap::new();
			for model in self
				.snapshot
				.available
				.iter()
				.filter(|model| model.provider != provider_id)
				.chain(available.iter())
			{
				available_by_id.insert(format!("{}\0{}", model.provider, model.id), model.clone());
			}
			let available: Vec<Model> = all
				.iter()
				.filter_map(|model| available_by_id.get(&format!("{}\0{}", model.provider, model.id)))
				.cloned()
				.collect();
			self.snapshot = ModelRuntimeSnapshot {
				all,
				available,
				configured_providers,
				stored_providers,
				auth: auth_by_provider,
			};
			if error_seq == self.availability_error_seq {
				self.availability_error = None;
			}
			Ok(())
		})();
		result.inspect_err(|error| {
			if self.provider_availability_seq.get(provider_id) == Some(&provider_seq)
				&& error_seq == self.availability_error_seq
				&& !signal.is_some_and(AbortSignal::aborted)
			{
				self.availability_error = Some(error.message.clone());
			}
		})
	}

	// -- collection reads (delegation) ---------------------------------------

	pub fn get_providers(&self) -> Vec<Arc<Provider>> {
		self.models.provider_arcs().to_vec()
	}

	pub fn get_provider(&self, provider_id: &str) -> Option<Arc<Provider>> {
		self.models.provider_arc(provider_id)
	}

	pub fn get_models(&self, provider_id: Option<&str>) -> Vec<Model> {
		self.models.get_models(provider_id)
	}

	pub fn get_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
		self.models.get_model(provider_id, model_id)
	}

	pub fn get_models_of_type(&self, ty: &ModelType, provider_id: Option<&str>) -> Vec<AnyModel> {
		self.models.get_models_of_type(ty, provider_id)
	}

	pub fn get_model_of_type(
		&self,
		ty: &ModelType,
		provider_id: &str,
		model_id: &str,
	) -> Option<AnyModel> {
		self.models.get_model_of_type(ty, provider_id, model_id)
	}

	pub fn get_all_models(&self, provider_id: Option<&str>) -> Vec<AnyModel> {
		self.models.get_all_models(provider_id)
	}

	pub fn get_available_of_type(
		&self,
		ty: &ModelType,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Vec<AnyModel>, JsError> {
		self.models.get_available_of_type(ty, provider_id, options)
	}

	pub fn get_all_available(
		&self,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Vec<AnyModel>, JsError> {
		self.models.get_all_available(provider_id, options)
	}

	pub fn check_auth(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<AuthCheck>, JsError> {
		self.models.check_auth(provider_id, options)
	}

	/// TS `getAvailable` — per-provider passthrough with error recording,
	/// or a full availability refresh + snapshot read.
	pub fn get_available(
		&mut self,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Vec<Model>, JsError> {
		if let Some(provider_id) = provider_id {
			self.availability_error_seq += 1;
			let error_seq = self.availability_error_seq;
			match self.models.get_available(Some(provider_id), options) {
				Ok(available) => {
					if error_seq == self.availability_error_seq {
						self.availability_error = None;
					}
					Ok(available)
				}
				Err(error) => {
					let signal = options.and_then(|o| o.signal);
					if error_seq == self.availability_error_seq
						&& !signal.is_some_and(AbortSignal::aborted)
					{
						self.availability_error = Some(error.message.clone());
					}
					Err(error)
				}
			}
		} else {
			let signal = options.and_then(|o| o.signal);
			self.queue_availability_refresh(signal)?;
			Ok(self.snapshot.available.clone())
		}
	}

	pub fn get_available_snapshot(&self) -> &[Model] {
		&self.snapshot.available
	}

	/// TS `getError` — config error, composition errors, availability error.
	pub fn get_error(&self) -> Option<String> {
		let mut errors: Vec<String> = Vec::new();
		if let Some(config_error) = self.config.get_error() {
			errors.push(config_error.to_string());
		}
		for (provider_id, error) in &self.composition_errors {
			errors.push(format!("Provider \"{provider_id}\": {error}"));
		}
		if let Some(availability_error) = &self.availability_error {
			errors.push(format!("Availability refresh: {availability_error}"));
		}
		(!errors.is_empty()).then(|| errors.join("\n\n"))
	}

	pub fn get_registered_provider_config(&self, provider_id: &str) -> Option<&ProviderConfigInput> {
		self.extension_providers.get(provider_id)
	}

	pub fn get_registered_provider_ids(&self) -> Vec<String> {
		let mut ids: IndexSet<String> = IndexSet::new();
		for id in self.extension_providers.keys() {
			ids.insert(id.clone());
		}
		for id in self.native_extension_providers.keys() {
			ids.insert(id.clone());
		}
		ids.into_iter().collect()
	}

	pub fn get_registered_native_provider(&self, provider_id: &str) -> Option<&Provider> {
		self.native_extension_providers.get(provider_id).map(|p| p.as_ref())
	}

	/// TS `getCompatibilityRequestConfig` (env injected).
	pub fn get_compatibility_request_config(
		&self,
		model: &AnyModel,
	) -> Result<CompatibilityRequestConfig, JsError> {
		resolve_compatibility_request_config(
			model,
			self.config.get_provider(model.provider()),
			self.extension_providers.get(model.provider()),
			Some(&self.env),
		)
	}

	pub fn is_using_oauth(&self, provider_id: &str) -> bool {
		self.snapshot
			.auth
			.get(provider_id)
			.and_then(|check| check.as_ref())
			.is_some_and(|check| check.auth_type == AuthType::OAuth)
	}

	pub fn is_using_subscription(&self, provider_id: &str) -> bool {
		self.is_using_oauth(provider_id)
			&& self
				.models
				.get_provider(provider_id)
				.and_then(|provider| provider.auth.oauth.as_ref())
				.is_some_and(|oauth| oauth.is_subscription())
	}

	pub fn has_configured_auth(&self, provider_id: &str) -> bool {
		self.snapshot.configured_providers.contains(provider_id)
	}

	// -- auth ----------------------------------------------------------------

	/// TS `getAuth(providerId)`.
	pub fn get_auth_for_provider(
		&self,
		provider_id: &str,
		overrides: Option<&ModelRuntimeAuthOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError> {
		self.models.get_auth_for_provider(provider_id, Some(&resolution_overrides(overrides)))
	}

	/// TS `getAuth(model)` — resolution plus configured model headers.
	pub fn get_auth_for_model(
		&self,
		model: &AnyModel,
		overrides: Option<&ModelRuntimeAuthOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError> {
		let Some(resolution) =
			self.models.get_auth_for_model(model, Some(&resolution_overrides(overrides)))?
		else {
			return Ok(None);
		};
		let mut merged_env: Env = resolution.env.clone().unwrap_or_default().into_iter().collect();
		if let Some(overlay) = overrides.and_then(|o| o.env.as_ref()) {
			for (name, value) in overlay {
				merged_env.insert(name.clone(), value.clone());
			}
		}
		let configured_headers = resolve_configured_model_headers(
			model,
			self.config.get_provider(model.provider()),
			self.extension_providers.get(model.provider()),
			Some(&merged_env),
		)?;
		let configured_headers =
			configured_headers.as_ref().map(|pairs| header_pairs_to_provider_headers(pairs));
		let mut auth = resolution.auth.clone();
		auth.headers = merge_headers(auth.headers.as_ref(), configured_headers.as_ref());
		Ok(Some(AuthResult { auth, env: resolution.env.clone(), source: resolution.source.clone() }))
	}

	/// TS `getProviderAuthStatus` — runtime → stored → configured →
	/// environment → unconfigured.
	pub fn get_provider_auth_status(&self, provider_id: &str) -> crate::provider_composer::AuthStatus {
		{
			let store = self.credentials.lock().unwrap_or_else(|p| p.into_inner());
			if store.has_runtime_api_key(provider_id) {
				return configured_status(true, Some("runtime"), None);
			}
		}
		if self.snapshot.stored_providers.contains(provider_id) {
			return configured_status(true, Some("stored"), None);
		}
		if let Some(configured) = configured_request_auth_status(
			self.config.get_provider(provider_id),
			self.extension_providers.get(provider_id),
			Some(&self.env),
		) {
			return configured;
		}
		match self.snapshot.auth.get(provider_id) {
			Some(Some(check)) => configured_status(true, Some("environment"), check.source.clone()),
			_ => configured_status(false, None, None),
		}
	}

	// -- credential operations -----------------------------------------------

	/// TS `synchronizeCredentialState`.
	fn synchronize_credential_state(
		&mut self,
		provider_id: &str,
		operation: CredentialSynchronizationOperation,
		credential: Option<Credential>,
		signal: Option<&AbortSignal>,
	) -> Result<(), JsError> {
		let inner = (|| -> Result<(), JsError> {
			if signal.is_some_and(AbortSignal::aborted) {
				return Err(JsError::abort());
			}
			self.recompose_provider(provider_id);
			if let Some(error) = self.composition_errors.get(provider_id) {
				return Err(JsError::new(error.clone()));
			}
			let result = self.models.refresh(Some(&crate::pi_ai::models::ModelsRefreshOptions {
				allow_network: Some(false),
				providers: Some(vec![provider_id.to_string()]),
				force: false,
				signal,
			}));
			if result.aborted && signal.is_some_and(AbortSignal::aborted) {
				return Err(JsError::abort());
			}
			if let Some(error) = result.errors.get(provider_id) {
				return Err(error.clone());
			}
			self.update_model_snapshot();
			self.refresh_provider_availability(provider_id, signal)
		})();
		inner.map_err(|cause| {
			CredentialSynchronizationError {
				provider_id: provider_id.to_string(),
				operation,
				credential,
				cause,
			}
			.into_js_error()
		})
	}

	pub fn set_runtime_api_key(
		&mut self,
		provider_id: &str,
		api_key: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<(), JsError> {
		let signal = options.and_then(|o| o.signal);
		{
			let mut store = self.credentials.lock().unwrap_or_else(|p| p.into_inner());
			store.set_runtime_api_key(provider_id, api_key);
		}
		let credential = Credential::ApiKey(
			ApiKeyCredential::from_map(
				serde_json::Map::from_iter([("key".to_string(), json!(api_key))]),
			)
			.expect("valid api_key credential"),
		);
		self.synchronize_credential_state(
			provider_id,
			CredentialSynchronizationOperation::SetRuntimeApiKey,
			Some(credential),
			signal,
		)
	}

	pub fn remove_runtime_api_key(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<(), JsError> {
		let signal = options.and_then(|o| o.signal);
		{
			let mut store = self.credentials.lock().unwrap_or_else(|p| p.into_inner());
			store.remove_runtime_api_key(provider_id);
		}
		self.synchronize_credential_state(
			provider_id,
			CredentialSynchronizationOperation::RemoveRuntimeApiKey,
			None,
			signal,
		)
	}

	pub fn list_credentials(
		&self,
		options: Option<&AuthOperationOptions>,
	) -> Result<Vec<CredentialInfo>, JsError> {
		let store = self.credentials.lock().unwrap_or_else(|p| p.into_inner());
		store.list(options)
	}

	pub fn login(
		&mut self,
		provider_id: &str,
		ty: AuthType,
		interaction: &mut dyn crate::pi_ai::models::AuthInteraction,
		options: Option<&crate::pi_ai::models::LoginOptions>,
	) -> Result<Credential, JsError> {
		let credential = self.models.login(provider_id, ty, interaction, options)?;
		self.synchronize_credential_state(
			provider_id,
			CredentialSynchronizationOperation::Login,
			Some(credential.clone()),
			None,
		)?;
		Ok(credential)
	}

	pub fn logout(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<(), JsError> {
		let signal = options.and_then(|o| o.signal);
		self.models.logout(provider_id, options)?;
		self.synchronize_credential_state(
			provider_id,
			CredentialSynchronizationOperation::Logout,
			None,
			signal,
		)
	}

	// -- refresh -------------------------------------------------------------

	/// TS `refresh` — reload models.json, recompose, refresh catalogs,
	/// refresh availability.
	pub fn refresh(
		&mut self,
		options: Option<&crate::pi_ai::models::ModelsRefreshOptions<'_>>,
	) -> Result<crate::pi_ai::models::ModelsRefreshResult, JsError> {
		self.config = load_model_config(self.models_path.as_deref());
		self.configure_radius_providers();
		let selected = options.and_then(|o| o.providers.clone());
		match &selected {
			Some(providers) => {
				let mut seen = IndexSet::new();
				for provider_id in providers {
					if seen.insert(provider_id.clone()) {
						self.recompose_provider(provider_id);
					}
				}
				self.update_model_snapshot();
			}
			None => self.rebuild_providers(),
		}
		let allow_network = options
			.and_then(|o| o.allow_network)
			.unwrap_or(self.model_network_enabled);
		let refresh_options = crate::pi_ai::models::ModelsRefreshOptions {
			allow_network: Some(allow_network),
			providers: selected.clone(),
			force: options.is_some_and(|o| o.force),
			signal: options.and_then(|o| o.signal),
		};
		let result = self.models.refresh(Some(&refresh_options));
		let mut errors = result.errors.clone();
		self.update_model_snapshot();
		match &selected {
			Some(providers) => {
				let mut seen = IndexSet::new();
				for provider_id in providers {
					if !seen.insert(provider_id.clone()) {
						continue;
					}
					if let Err(error) =
						self.refresh_provider_availability(provider_id, options.and_then(|o| o.signal))
					{
						if !options.and_then(|o| o.signal).is_some_and(AbortSignal::aborted) {
							errors.insert(provider_id.clone(), error);
						}
					}
				}
			}
			None => {
				// Availability errors are recorded by the latest pass;
				// refreshed models remain usable.
				let _ = self.queue_availability_refresh(options.and_then(|o| o.signal));
			}
		}
		Ok(crate::pi_ai::models::ModelsRefreshResult {
			aborted: result.aborted
				|| options.and_then(|o| o.signal).is_some_and(AbortSignal::aborted),
			errors,
		})
	}

	// -- registration --------------------------------------------------------

	pub fn register_native_provider(&mut self, provider: Provider) {
		if provider.id.trim().is_empty() {
			panic!("Provider id must not be empty.");
		}
		// The empty-id throw is modeled as a Result at the boundary; the
		// TS throw is reproduced by `register_native_provider_checked`.
		self.register_native_provider_checked(provider).expect("checked id");
	}

	/// `registerNativeProvider` with the TS throw surfaced as `Err`.
	pub fn register_native_provider_checked(&mut self, provider: Provider) -> Result<(), JsError> {
		if provider.id.trim().is_empty() {
			return Err(JsError::new("Provider id must not be empty."));
		}
		let provider_id = provider.id.clone();
		let is_oauth = provider.auth.oauth.is_some() && provider.auth.api_key.is_none();
		self.extension_providers.shift_remove(&provider_id);
		self.native_extension_providers.insert(provider_id.clone(), Arc::new(provider));
		self.recompose_provider(&provider_id);
		self.update_model_snapshot();
		let configured = configured_request_auth_status(
			self.config.get_provider(&provider_id),
			None,
			Some(&self.env),
		);
		self.mark_provisionally_configured(
			&provider_id,
			configured.as_ref(),
			if is_oauth { AuthType::OAuth } else { AuthType::ApiKey },
		);
		// TS fires `void this.refresh({ allowNetwork: false })` here — an
		// async fire-and-forget whose effects are only observable after the
		// caller yields. The synchronous facade contract (ModelRegistry) reads
		// provisional state, so the refresh is intentionally NOT run inline;
		// harness/tests call `refresh` explicitly to emulate settling.
		Ok(())
	}

	/// TS `markProvisionallyConfigured`.
	fn mark_provisionally_configured(
		&mut self,
		provider_id: &str,
		configured_status: Option<&crate::provider_composer::AuthStatus>,
		ty: AuthType,
	) {
		if !self.snapshot.stored_providers.contains(provider_id)
			&& !configured_status.is_some_and(|status| status.configured)
		{
			return;
		}
		let mut configured_providers = self.snapshot.configured_providers.clone();
		configured_providers.insert(provider_id.to_string());
		let mut auth = self.snapshot.auth.clone();
		// Never clobber a real check result.
		if auth.get(provider_id).map(|check| check.is_none()).unwrap_or(true) {
			auth.insert(
				provider_id.to_string(),
				Some(AuthCheck { source: Some("configured provider".to_string()), auth_type: ty }),
			);
		}
		self.snapshot.configured_providers = configured_providers;
		self.snapshot.auth = auth;
		self.snapshot.available = self
			.snapshot
			.all
			.iter()
			.filter(|model| self.snapshot.configured_providers.contains(&model.provider))
			.cloned()
			.collect();
	}

	pub fn register_provider(&mut self, provider_id: &str, config: ProviderConfigInput) {
		self.register_provider_checked(provider_id, config).expect("validated");
	}

	/// `registerProvider` with the TS throw surfaced as `Err`.
	pub fn register_provider_checked(
		&mut self,
		provider_id: &str,
		config: ProviderConfigInput,
	) -> Result<(), JsError> {
		// Validate the incoming registration on its own; a broken
		// re-registration must throw without touching the stored config.
		validate_extension_provider(
			provider_id,
			self.builtins.get(provider_id).map(|p| p.as_ref()),
			self.config.get_provider(provider_id),
			&config,
		)?;
		self.native_extension_providers.shift_remove(provider_id);
		// Re-registration merges defined values over the previous
		// registration and preserves undefined ones.
		let effective = match self.extension_providers.get(provider_id) {
			Some(previous) => merge_provider_config(previous, &config),
			None => config,
		};
		let is_oauth = effective.oauth.is_some() && effective.api_key.is_none();
		self.extension_providers.insert(provider_id.to_string(), effective);
		self.recompose_provider(provider_id);
		self.update_model_snapshot();
		let configured = configured_request_auth_status(
			self.config.get_provider(provider_id),
			self.extension_providers.get(provider_id),
			Some(&self.env),
		);
		self.mark_provisionally_configured(
			provider_id,
			configured.as_ref(),
			if is_oauth { AuthType::OAuth } else { AuthType::ApiKey },
		);
		// TS fires `void this.refresh({ allowNetwork: false })` here — an
		// async fire-and-forget whose effects are only observable after the
		// caller yields. The synchronous facade contract (ModelRegistry) reads
		// provisional state, so the refresh is intentionally NOT run inline;
		// harness/tests call `refresh` explicitly to emulate settling.
		Ok(())
	}

	pub fn unregister_provider(&mut self, provider_id: &str) {
		self.extension_providers.shift_remove(provider_id);
		self.native_extension_providers.shift_remove(provider_id);
		self.recompose_provider(provider_id);
		self.update_model_snapshot();
		// TS fires `void this.refresh({ allowNetwork: false })` here — an
		// async fire-and-forget whose effects are only observable after the
		// caller yields. The synchronous facade contract (ModelRegistry) reads
		// provisional state, so the refresh is intentionally NOT run inline;
		// harness/tests call `refresh` explicitly to emulate settling.
	}

	pub fn register_virtual_model(&mut self, definition: VirtualModelDefinition) {
		self.register_virtual_model_checked(definition).expect("validated");
	}

	/// `registerVirtualModel` with the TS throws surfaced as `Err`.
	pub fn register_virtual_model_checked(
		&mut self,
		definition: VirtualModelDefinition,
	) -> Result<(), JsError> {
		let provider_id = definition.provider.clone();
		let id = definition.id.clone();
		if provider_id.trim().is_empty() || id.trim().is_empty() {
			return Err(JsError::new("Virtual model provider and id must not be empty."));
		}
		if let Some(existing) = self.models.get_model(&provider_id, &id) {
			if !model_is_virtual(&existing) {
				return Err(JsError::new(format!(
					"Virtual model {provider_id}/{id} conflicts with a physical model."
				)));
			}
		}
		let model = create_virtual_model(&definition);
		let route = definition.route;
		let models = self
			.virtual_models
			.entry(provider_id.clone())
			.or_default();
		models.insert(id, RegisteredVirtualModel { model, route });
		let composed = self.recompose_provider(&provider_id);
		if composed.is_none() && !self.snapshot.configured_providers.contains(&provider_id) {
			// A provider of only virtual models needs no credentials.
			let mut auth = self.snapshot.auth.clone();
			auth.insert(
				provider_id.clone(),
				Some(AuthCheck {
					source: Some("virtual".to_string()),
					auth_type: AuthType::ApiKey,
				}),
			);
			let mut configured_providers = self.snapshot.configured_providers.clone();
			configured_providers.insert(provider_id);
			self.snapshot.auth = auth;
			self.snapshot.configured_providers = configured_providers;
		}
		self.update_model_snapshot();
		// TS fires `void this.refresh({ allowNetwork: false })` here — an
		// async fire-and-forget whose effects are only observable after the
		// caller yields. The synchronous facade contract (ModelRegistry) reads
		// provisional state, so the refresh is intentionally NOT run inline;
		// harness/tests call `refresh` explicitly to emulate settling.
		Ok(())
	}

	pub fn unregister_virtual_model(&mut self, provider_id: &str, id: &str) {
		let Some(models) = self.virtual_models.get_mut(provider_id) else { return };
		if models.shift_remove(id).is_none() {
			return;
		}
		if models.is_empty() {
			self.virtual_models.shift_remove(provider_id);
		}
		self.recompose_provider(provider_id);
		self.update_model_snapshot();
		// TS fires `void this.refresh({ allowNetwork: false })` here — an
		// async fire-and-forget whose effects are only observable after the
		// caller yields. The synchronous facade contract (ModelRegistry) reads
		// provisional state, so the refresh is intentionally NOT run inline;
		// harness/tests call `refresh` explicitly to emulate settling.
	}

	// -- virtual model routing ----------------------------------------------

	/// TS `resolveModel` — ask a virtual model's router for the model and
	/// thinking level of one request.
	#[allow(clippy::too_many_arguments)]
	pub fn resolve_model(
		&self,
		model: &Model,
		messages: &[Value],
		reason: ModelRouteReason,
		thinking_level: ModelThinkingLevel,
		state: Option<Value>,
		failed: Option<&Value>,
		signal: Option<&AbortSignal>,
	) -> Result<ModelRoute, JsError> {
		let name = format!("Virtual model {}/{}", model.provider, model.id);
		let Some(virtual_model) = self
			.virtual_models
			.get(&model.provider)
			.and_then(|models| models.get(&model.id))
		else {
			return Err(JsError::new(format!("{name} is not registered.")));
		};
		let latest = find_latest_response(messages);
		let physical = |value: &Value| {
			let provider = value.get("provider").and_then(Value::as_str)?;
			let model = value.get("model").and_then(Value::as_str)?;
			self.get_physical_model(provider, model)
		};
		let previous_model = latest.and_then(physical);
		let failed_model = failed.and_then(physical);
		let request = ModelRouteRequest {
			model: model.clone(),
			thinking_level,
			reason,
			previous: previous_model.map(|model| {
				(
					model,
					latest
						.and_then(|latest| latest.get("thinkingLevel"))
						.and_then(Value::as_str)
						.and_then(ModelThinkingLevel::parse),
				)
			}),
			failed: failed_model
				.zip(failed)
				.map(|(model, failed)| FailedRouteRequest {
					model,
					thinking_level: failed
						.get("thinkingLevel")
						.and_then(Value::as_str)
						.and_then(ModelThinkingLevel::parse),
					message: failed.clone(),
				}),
			state,
			messages: messages.to_vec(),
			signal: signal.cloned(),
		};
		let route = (virtual_model.route)(&request)?;
		let target = self.get_physical_model(&route.model.provider, &route.model.id);
		let routed = format!("{name} routed to {}/{}", route.model.provider, route.model.id);
		let Some(target) = target else {
			return Err(JsError::new(format!("{routed}, which is not a physical model.")));
		};
		if !self.has_configured_auth(&target.provider) {
			return Err(JsError::new(format!("{routed}, which has no credentials.")));
		}
		let thinking_level = clamp_thinking_level(&target, route.thinking_level);
		Ok(ModelRoute {
			model: target,
			thinking_level,
			state: route.state,
		})
	}

	/// TS `getPhysicalModel` — a catalog chat model that is not virtual.
	pub fn get_physical_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
		self.models
			.get_model(provider_id, model_id)
			.filter(|model| !model_is_virtual(model))
	}

	// -- request dispatch ----------------------------------------------------

	/// TS `prepareRequest`.
	fn prepare_request(
		&self,
		model: &Model,
		options: Option<&Value>,
	) -> Result<PreparedRequest<'_>, JsError> {
		let provider = self.models.get_provider(&model.provider).ok_or_else(|| {
			ModelsError::new(
				ModelsErrorKind::Provider,
				format!("Unknown provider: {}", model.provider),
			)
		})?;
		let overrides = ModelRuntimeAuthOverrides {
			api_key: options
				.and_then(|o| o.get("apiKey"))
				.and_then(Value::as_str)
				.map(str::to_string),
			env: json_env(options, "env"),
			min_oauth_validity_ms: None,
			signal: None,
		};
		let resolution = self
			.get_auth_for_model(&AnyModel::Chat(model.clone()), Some(&overrides))?
			.ok_or_else(|| {
				ModelsError::new(
					ModelsErrorKind::Auth,
					format!("Provider is not configured: {}", model.provider),
				)
			})?;
		let option_headers = json_headers(options, "headers");
		let headers = merge_headers(resolution.auth.headers.as_ref(), option_headers.as_ref());
		let env = match (&resolution.env, json_env(options, "env")) {
			(None, None) => None,
			(resolved, option_env) => {
				let mut merged = resolved.clone().unwrap_or_default();
				if let Some(option_env) = option_env {
					for (name, value) in option_env {
						merged.insert(name, value);
					}
				}
				Some(merged)
			}
		};
		let mut request_model = model.clone();
		if let Some(base_url) = resolution.auth.base_url.as_deref().filter(|u| !u.is_empty()) {
			request_model.base_url = base_url.to_string();
		}
		let mut map: Map<String, Value> = options
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default();
		map.shift_remove("transformHeaders");
		match options
			.and_then(|o| o.get("apiKey"))
			.and_then(Value::as_str)
			.map(str::to_string)
			.or_else(|| resolution.auth.api_key.clone())
		{
			Some(api_key) => {
				map.insert("apiKey".to_string(), json!(api_key));
			}
			None => {
				map.shift_remove("apiKey");
			}
		}
		match &headers {
			Some(headers) => {
				let mut out = Map::new();
				for (name, value) in headers {
					out.insert(
						name.clone(),
						value.clone().map(Value::String).unwrap_or(Value::Null),
					);
				}
				map.insert("headers".to_string(), Value::Object(out));
			}
			None => {
				map.shift_remove("headers");
			}
		}
		match &env {
			Some(env) => {
				let mut out = Map::new();
				for (name, value) in env {
					out.insert(name.clone(), json!(value));
				}
				map.insert("env".to_string(), Value::Object(out));
			}
			None => {
				map.shift_remove("env");
			}
		}
		Ok(PreparedRequest { provider, model: request_model, options: Value::Object(map) })
	}

	/// TS `stream` — raw context form (normalizes internally).
	pub fn stream(
		&self,
		model: &Model,
		context: &Value,
		options: Option<&Value>,
	) -> AssistantMessageEventStream {
		let transcript = normalize_context(context);
		self.stream_normalized(model, &transcript, options)
	}

	/// `stream` over an already-normalized transcript.
	pub fn stream_normalized(
		&self,
		model: &Model,
		transcript: &TranscriptContext,
		options: Option<&Value>,
	) -> AssistantMessageEventStream {
		let now = self.now.now();
		lazy_stream(model, now, || {
			assert_chat_model(&AnyModel::Chat(model.clone())).map_err(JsError::from)?;
			let prepared = self.prepare_request(model, options)?;
			Ok(prepared
				.provider
				.stream(&prepared.model, transcript, Some(&prepared.options)))
		})
	}

	pub fn complete(
		&self,
		model: &Model,
		context: &Value,
		options: Option<&Value>,
	) -> Result<AssistantMessage, JsError> {
		stream_terminal_result(&self.stream(model, context, options))
	}

	/// TS `streamSimple` — raw context form.
	pub fn stream_simple(
		&self,
		model: &Model,
		context: &Value,
		options: Option<&Value>,
	) -> AssistantMessageEventStream {
		let transcript = normalize_context(context);
		self.stream_simple_normalized(model, context, &transcript, options)
	}

	/// `streamSimple` with the virtual-model routing branch.
	pub fn stream_simple_normalized(
		&self,
		model: &Model,
		context: &Value,
		transcript: &TranscriptContext,
		options: Option<&Value>,
	) -> AssistantMessageEventStream {
		let now = self.now.now();
		if model_is_virtual(model) {
			let model = model.clone();
			let setup_model = model.clone();
			let context = context.clone();
			let options = options.cloned();
			return lazy_stream(&model, now, move || {
				let model = setup_model;
				let route = self.resolve_model(
					&model,
					&transcript_messages_value(transcript),
					ModelRouteReason::Direct,
					options
						.as_ref()
						.and_then(|o| o.get("reasoning"))
						.and_then(Value::as_str)
						.and_then(ModelThinkingLevel::parse)
						.unwrap_or(ModelThinkingLevel::Off),
					None,
					None,
					None,
				)?;
				let limit = route.model.max_tokens.get();
				let max_tokens = options
					.as_ref()
					.and_then(|o| o.get("maxTokens"))
					.and_then(Value::as_f64)
					.filter(|_| limit > 0.0)
					.map(|requested| requested.min(limit));
				let reasoning = if route.thinking_level == ModelThinkingLevel::Off {
					None
				} else {
					Some(route.thinking_level.as_str())
				};
				let mut map: Map<String, Value> = options
					.as_ref()
					.and_then(Value::as_object)
					.cloned()
					.unwrap_or_default();
				map.shift_remove("apiKey");
				map.shift_remove("headers");
				map.shift_remove("env");
				if route.model.provider == model.provider {
					if let Some(api_key) =
						options.as_ref().and_then(|o| o.get("apiKey")).cloned()
					{
						map.insert("apiKey".to_string(), api_key);
					}
					if let Some(headers) = options.as_ref().and_then(|o| o.get("headers")).cloned() {
						map.insert("headers".to_string(), headers);
					}
					if let Some(env) = options.as_ref().and_then(|o| o.get("env")).cloned() {
						map.insert("env".to_string(), env);
					}
				}
				if let Some(max_tokens) = max_tokens {
					map.insert("maxTokens".to_string(), json!(max_tokens));
				} else {
					map.shift_remove("maxTokens");
				}
				match reasoning {
					Some(reasoning) => {
						map.insert("reasoning".to_string(), json!(reasoning));
					}
					None => {
						map.shift_remove("reasoning");
					}
				}
				Ok(self.stream_simple_normalized(
					&route.model,
					&context,
					transcript,
					Some(&Value::Object(map)),
				))
			});
		}
		lazy_stream(model, now, || {
			assert_chat_model(&AnyModel::Chat(model.clone())).map_err(JsError::from)?;
			let prepared = self.prepare_request(model, options)?;
			Ok(prepared
				.provider
				.stream_simple(&prepared.model, transcript, Some(&prepared.options)))
		})
	}

	pub fn complete_simple(
		&self,
		model: &Model,
		context: &Value,
		options: Option<&Value>,
	) -> Result<AssistantMessage, JsError> {
		stream_terminal_result(&self.stream_simple(model, context, options))
	}

	pub fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&Value>,
	) -> AssistantMessageEventStream {
		let now = self.now.now();
		lazy_stream(model, now, || {
			assert_chat_model(&AnyModel::Chat(model.clone())).map_err(JsError::from)?;
			let prepared = self.prepare_request(model, options)?;
			if !prepared.provider.has_fetch_deferred() {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Provider {} does not support deferred responses", model.provider),
				)
				.into());
			}
			Ok(prepared
				.provider
				.fetch_deferred(&prepared.model, handle, Some(&prepared.options))
				.expect("checked has_fetch_deferred"))
		})
	}

	pub fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&Value>,
	) -> Result<AssistantMessage, JsError> {
		stream_terminal_result(&self.stream_deferred(model, handle, options))
	}

	pub fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&Value>,
	) -> Result<(), JsError> {
		assert_chat_model(&AnyModel::Chat(model.clone())).map_err(JsError::from)?;
		let prepared = self.prepare_request(model, options)?;
		if !prepared.provider.has_cancel_deferred() {
			return Err(ModelsError::new(
				ModelsErrorKind::Provider,
				format!("Provider {} does not support deferred responses", model.provider),
			)
			.into());
		}
		prepared
			.provider
			.cancel_deferred(&prepared.model, handle, Some(&prepared.options))
			.expect("checked has_cancel_deferred")
	}

	pub fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<&Value>,
	) -> AssistantImages {
		let result: Result<AssistantImages, JsError> = (|| {
			assert_image_model(&AnyModel::Image(model.clone())).map_err(JsError::from)?;
			let prepared = self.prepare_request(&image_as_chat(model), options)?;
			if !prepared.provider.has_generate_images() {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Provider {} does not support image generation", model.provider),
				)
				.into());
			}
			Ok(prepared
				.provider
				.generate_images(model, context, Some(&prepared.options))
				.expect("checked has_generate_images"))
		})();
		result.unwrap_or_else(|error| image_error_result(model, &error, false, self.now.now()))
	}

	pub fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<&Value>,
	) -> ClassifierResult {
		let result: Result<ClassifierResult, JsError> = (|| {
			assert_classifier_model(&AnyModel::Classifier(model.clone())).map_err(JsError::from)?;
			assert_classifier_input_supported(model, context).map_err(JsError::from)?;
			let prepared = self.prepare_request(&classifier_as_chat(model), options)?;
			if !prepared.provider.has_classify() {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Provider {} does not support classification", model.provider),
				)
				.into());
			}
			Ok(prepared
				.provider
				.classify(model, context, Some(&prepared.options))
				.expect("checked has_classify"))
		})();
		result.unwrap_or_else(|error| classifier_error_result(model, &error, false, self.now.now()))
	}
}

/// What `prepareRequest` produced for one request.
struct PreparedRequest<'a> {
	provider: &'a Provider,
	model: Model,
	options: Value,
}

/// Build a chat-shaped stand-in for image/classifier models so
/// `prepareRequest` (which resolves auth by provider + model headers) can
/// serve every model type.
fn image_as_chat(model: &ImageModel) -> Model {
	any_as_chat(&AnyModel::Image(model.clone()))
}

fn classifier_as_chat(model: &ClassifierModel) -> Model {
	any_as_chat(&AnyModel::Classifier(model.clone()))
}

fn any_as_chat(model: &AnyModel) -> Model {
	// prepareRequest only reads provider/id/headers; reuse the model JSON
	// with the chat shape so header/auth resolution matches.
	let value = model.to_value();
	serde_json::from_value(value).unwrap_or_else(|_| chat_stand_in(model.provider(), model.id()))
}

fn chat_stand_in(provider: &str, id: &str) -> Model {
	serde_json::from_value(json!({
		"id": id, "name": id, "api": "a", "provider": provider, "baseUrl": "",
		"input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
		"reasoning": false, "contextWindow": 0, "maxTokens": 0,
		"headers": null
	}))
	.expect("stand-in chat model")
}

/// Merge a new registration over the previous one (defined values win,
/// undefined values are preserved).
fn merge_provider_config(
	previous: &ProviderConfigInput,
	config: &ProviderConfigInput,
) -> ProviderConfigInput {
	let mut effective = previous.clone();
	if config.name.is_some() {
		effective.name = config.name.clone();
	}
	if config.base_url.is_some() {
		effective.base_url = config.base_url.clone();
	}
	if config.api_key.is_some() {
		effective.api_key = config.api_key.clone();
	}
	if config.api.is_some() {
		effective.api = config.api.clone();
	}
	if config.stream_simple.is_some() {
		effective.stream_simple = config.stream_simple.clone();
	}
	if config.images.is_some() {
		effective.images = config.images.clone();
	}
	if config.classifiers.is_some() {
		effective.classifiers = config.classifiers.clone();
	}
	if config.headers.is_some() {
		effective.headers = config.headers.clone();
	}
	if config.auth_header.is_some() {
		effective.auth_header = config.auth_header;
	}
	if config.oauth.is_some() {
		effective.oauth = config.oauth.clone();
	}
	if config.models.is_some() {
		effective.models = config.models.clone();
	}
	if config.refresh_models.is_some() {
		effective.refresh_models = config.refresh_models.clone();
	}
	effective
}

/// `AuthResolutionOverrides` from the runtime's override shape.
fn resolution_overrides<'a>(
	overrides: Option<&ModelRuntimeAuthOverrides<'a>>,
) -> crate::pi_ai::models::AuthResolutionOverrides<'a> {
	let Some(overrides) = overrides else {
		return crate::pi_ai::models::AuthResolutionOverrides {
			api_key: None,
			env: None,
			min_oauth_validity_ms: None,
			signal: None,
		};
	};
	crate::pi_ai::models::AuthResolutionOverrides {
		api_key: overrides.api_key.clone(),
		env: overrides.env.clone(),
		min_oauth_validity_ms: overrides.min_oauth_validity_ms,
		signal: overrides.signal,
	}
}

/// The snapshot's `messages` as raw JSON (the transcript placeholder form).
fn transcript_messages_value(transcript: &TranscriptContext) -> Vec<Value> {
	crate::pi_ai::transcript::transcript_messages(transcript)
}

/// `AuthStatus { configured, source?, label? }`.
fn configured_status(
	configured: bool,
	source: Option<&str>,
	label: Option<String>,
) -> crate::provider_composer::AuthStatus {
	crate::provider_composer::AuthStatus {
		configured,
		source: source.map(str::to_string),
		label,
	}
}

/// `ModelConfig.load(modelsPath)` with a real file read (ENOENT → empty).
fn load_model_config(path: Option<&str>) -> ModelConfig {
	let Some(path) = path else {
		return ModelConfig::from_load(None, None, None);
	};
	match std::fs::read_to_string(path) {
		Ok(content) => ModelConfig::from_load(Some(path), Some(&content), None),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
			ModelConfig::from_load(Some(path), None, None)
		}
		Err(error) => ModelConfig::from_load(Some(path), None, Some(&error.to_string())),
	}
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::credential::InMemoryCredentialStore;
	use crate::pi_ai::models_impl::merge_headers;
	use crate::provider_composer::ProviderConfigInput;
	use crate::virtual_models::VirtualModelDefinition;

	fn blank_config() -> ProviderConfigInput {
		ProviderConfigInput {
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
		}
	}

	fn env(pairs: &[(&str, &str)]) -> Env {
		pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
	}

	fn test_runtime(env: Env, refresh_on_create: bool) -> ModelRuntime {
		ModelRuntime::create(CreateModelRuntimeOptions {
			credentials: Some(Box::new(InMemoryCredentialStore::new())),
			models_path: None,
			refresh_on_create,
			env: Some(env),
			now_ms: Some(0.0),
			..Default::default()
		})
		.unwrap()
	}

	fn openai_model(runtime: &ModelRuntime) -> Model {
		runtime
			.get_models(Some("openai"))
			.into_iter()
			.next()
			.expect("openai builtin chat models")
	}

	// -- mergeHeaders --------------------------------------------------------

	#[test]
	fn merge_headers_case_behavior() {
		let base: ProviderHeaders = [("Content-Type", Some("a")), ("X-A", Some("1"))]
			.into_iter()
			.map(|(k, v)| (k.to_string(), v.map(|s| s.to_string())))
			.collect();
		let over: ProviderHeaders = [("content-type", Some("b")), ("X-B", Some("2"))]
			.into_iter()
			.map(|(k, v)| (k.to_string(), v.map(|s| s.to_string())))
			.collect();
		let merged = merge_headers(Some(&base), Some(&over)).expect("merged");
		// TS delete-then-insert: the overridden key LOSES its base position and is
		// appended under the override's casing; later keys follow.
		let keys: Vec<&String> = merged.keys().collect();
		assert_eq!(keys, [&"X-A".to_string(), &"content-type".to_string(), &"X-B".to_string()]);
		assert_eq!(merged.get("Content-Type"), None);
		assert_eq!(merged.get("content-type"), Some(&Some("b".to_string())));
		assert_eq!(merged.get("X-A"), Some(&Some("1".to_string())));
		assert_eq!(merged.get("X-B"), Some(&Some("2".to_string())));
		// None only when both sides are None.
		assert!(merge_headers(None, None).is_none());
		let over_only: ProviderHeaders = [("x", Some("v"))]
			.into_iter()
			.map(|(k, v)| (k.to_string(), v.map(|s| s.to_string())))
			.collect();
		assert!(merge_headers(None, Some(&over_only)).is_some());
	}

	// -- getError ------------------------------------------------------------

	#[test]
	fn get_error_formatting_and_ordering() {
		let dir = std::env::temp_dir();
		let path = dir.join(format!("pi-models-runtime-{}.json", std::process::id()));
		std::fs::write(&path, r#"{"providers":{"ghost":{"oauth":"radius"}}}"#).unwrap();
		let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
			credentials: Some(Box::new(InMemoryCredentialStore::new())),
			models_path: Some(path.to_string_lossy().into_owned()),
			refresh_on_create: false,
			env: Some(Env::new()),
			now_ms: Some(0.0),
			..Default::default()
		})
		.unwrap();
		std::fs::remove_file(&path).ok();
		assert_eq!(
			runtime.get_error(),
			Some(
				"Provider \"ghost\": Provider ghost: \"baseUrl\" is required when \"oauth\" is set."
					.to_string()
			)
		);
		// Clean runtime: no error.
		assert_eq!(test_runtime(Env::new(), false).get_error(), None);
	}

	// -- getProviderAuthStatus precedence ------------------------------------

	#[test]
	fn get_provider_auth_status_precedence() {
		// Offline (PI_OFFLINE present), env key configured.
		let mut runtime = test_runtime(
			env(&[("PI_OFFLINE", "1"), ("OPENAI_API_KEY", "sk-env")]),
			true,
		);
		let status = runtime.get_provider_auth_status("openai");
		assert!(status.configured);
		assert_eq!(status.source.as_deref(), Some("environment"));

		// Unconfigured: nothing in env, no stored credential.
		let bare = test_runtime(env(&[("PI_OFFLINE", "1")]), true);
		let status = bare.get_provider_auth_status("openai");
		assert!(!status.configured);

		// Runtime key takes precedence over environment.
		runtime.set_runtime_api_key("openai", "sk-runtime", None).unwrap();
		let status = runtime.get_provider_auth_status("openai");
		assert!(status.configured);
		assert_eq!(status.source.as_deref(), Some("runtime"));
	}

	// -- registerProvider merge + validate-first ------------------------------

	#[test]
	fn register_provider_merges_and_validates_before_mutating() {
		let mut runtime = test_runtime(Env::new(), false);
		let mut first = blank_config();
		first.name = Some("Custom".to_string());
		first.api_key = Some("sk-custom".to_string());
		runtime.register_provider_checked("custom", first).unwrap();
		assert_eq!(
			runtime.get_registered_provider_config("custom").and_then(|c| c.name.clone()),
			Some("Custom".to_string())
		);

		// Re-registration with only baseUrl defined: undefined values are
		// preserved from the previous registration.
		let mut second = blank_config();
		second.base_url = Some("https://custom.example".to_string());
		runtime.register_provider_checked("custom", second).unwrap();
		let merged = runtime.get_registered_provider_config("custom").unwrap();
		assert_eq!(merged.name.clone(), Some("Custom".to_string()));
		assert_eq!(merged.api_key.clone(), Some("sk-custom".to_string()));
		assert_eq!(merged.base_url.clone(), Some("https://custom.example".to_string()));

		// An invalid re-registration throws without touching the stored
		// config (validate FIRST).
		let mut broken = blank_config();
		broken.stream_simple = Some(Arc::new(|_, _, _| unreachable!("never called")));
		let error = runtime.register_provider_checked("custom", broken).unwrap_err();
		assert_eq!(
			error.message,
			"Provider custom: \"api\" is required when registering streamSimple."
		);
		let stored = runtime.get_registered_provider_config("custom").unwrap();
		assert_eq!(stored.name.clone(), Some("Custom".to_string()));
	}

	// -- virtual model registration edge cases --------------------------------

	#[test]
	fn register_native_provider_requires_id() {
		let mut runtime = test_runtime(Env::new(), false);
		let provider = Provider::new(
			String::new(),
			"Empty".to_string(),
			crate::pi_ai::credential::ProviderAuth {
				api_key: Some(Arc::new(BuiltinApiKeyAuth::new("test"))),
				oauth: None,
			},
			Box::new(Vec::new),
			Box::new(builtin_unavailable_stream),
			Box::new(builtin_unavailable_stream),
		);
		let error = runtime.register_native_provider_checked(provider).unwrap_err();
		assert_eq!(error.message, "Provider id must not be empty.");
	}

	#[test]
	fn virtual_model_registration_edge_cases() {
		let mut runtime = test_runtime(Env::new(), false);
		let physical = openai_model(&runtime);

		// Empty provider/id.
		let mut empty = blank_definition();
		empty.provider = "  ".to_string();
		let error = runtime.register_virtual_model_checked(empty).unwrap_err();
		assert_eq!(error.message, "Virtual model provider and id must not be empty.");

		// Conflict with a physical model.
		let mut conflict = blank_definition();
		conflict.provider = "openai".to_string();
		conflict.id = physical.id.clone();
		let error = runtime.register_virtual_model_checked(conflict).unwrap_err();
		assert_eq!(
			error.message,
			format!("Virtual model openai/{} conflicts with a physical model.", physical.id)
		);

		// A virtual-only provider gets provisional {type: api_key, source:
		// "virtual"} auth without any credentials.
		let target = physical.clone();
		let mut def = blank_definition();
		def.provider = "vx".to_string();
		def.id = "v1".to_string();
		def.route = Box::new(move |_| {
			Ok(ModelRoute {
				model: target.clone(),
				thinking_level: ModelThinkingLevel::Off,
				state: None,
			})
		});
		runtime.register_virtual_model_checked(def).unwrap();
		assert!(runtime.has_configured_auth("vx"));
		assert!(runtime.get_model("vx", "v1").is_some());

		// resolveModel requires the routed target to have credentials.
		let unresolved = runtime
			.resolve_model(
				&runtime.get_model("vx", "v1").unwrap(),
				&[],
				ModelRouteReason::Direct,
				ModelThinkingLevel::Off,
				None,
				None,
				None,
			)
			.unwrap_err();
		assert_eq!(
			unresolved.message,
			"Virtual model vx/v1 routed to openai/gpt-4, which has no credentials."
		);
		runtime.set_runtime_api_key("openai", "sk-test", None).unwrap();

		// resolveModel routes to the physical model and clamps thinking.
		let route = runtime
			.resolve_model(
				&runtime.get_model("vx", "v1").unwrap(),
				&[],
				ModelRouteReason::Direct,
				ModelThinkingLevel::Off,
				None,
				None,
				None,
			)
			.unwrap();
		assert_eq!(route.model.provider, "openai");
	}

	fn blank_definition() -> VirtualModelDefinition {
		VirtualModelDefinition {
			provider: "p".to_string(),
			id: "v".to_string(),
			name: "V".to_string(),
			thinking_levels: None,
			context_window: None,
			max_tokens: None,
			input: None,
			route: Box::new(|_| unreachable!("not routed in this test")),
		}
	}

	// -- credential synchronization error wrapping ----------------------------

	#[test]
	fn credential_sync_error_wrapping() {
		let dir = std::env::temp_dir();
		let path = dir.join(format!("pi-models-sync-{}.json", std::process::id()));
		std::fs::write(&path, r#"{"providers":{"ghost":{"models":[{"id":"m","api":"openai-completions"}]}}}"#).unwrap();
		let mut runtime = ModelRuntime::create(CreateModelRuntimeOptions {
			credentials: Some(Box::new(InMemoryCredentialStore::new())),
			models_path: Some(path.to_string_lossy().into_owned()),
			refresh_on_create: false,
			env: Some(Env::new()),
			now_ms: Some(0.0),
			..Default::default()
		})
		.unwrap();
		std::fs::remove_file(&path).ok();
		// The credential commits, but recomposition reports the stored
		// composition error → CredentialSynchronizationError.
		let error = runtime.set_runtime_api_key("ghost", "k", None).unwrap_err();
		assert_eq!(
			error.message,
			"Credential setRuntimeApiKey committed for ghost, but local synchronization failed"
		);
		let cause = error.cause.as_ref().expect("chained cause");
		assert_eq!(
			cause.message,
			"Provider ghost: \"baseUrl\" is required when defining custom models."
		);
	}

	// -- snapshot filtering after setRuntimeApiKey ----------------------------

	#[test]
	fn snapshot_filters_by_configured_providers_after_set_runtime_api_key() {
		let mut runtime = test_runtime(env(&[("PI_OFFLINE", "1")]), true);
		// No credentials anywhere: nothing is available.
		assert!(runtime.get_available_snapshot().is_empty());

		runtime.set_runtime_api_key("openai", "sk-runtime", None).unwrap();
		let available = runtime.get_available_snapshot();
		assert!(!available.is_empty());
		assert!(available.iter().all(|model| model.provider == "openai"));
		assert!(runtime.has_configured_auth("openai"));
		// Other providers stay unavailable.
		assert!(!runtime.has_configured_auth("anthropic"));

		// Removing the runtime key filters the provider back out.
		runtime.remove_runtime_api_key("openai", None).unwrap();
		assert!(runtime.get_available_snapshot().is_empty());
	}
}

