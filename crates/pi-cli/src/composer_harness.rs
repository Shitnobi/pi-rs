//! Parity-harness entry for the provider-composer wave tool:
//! `provider-composer`.
//!
//! Wire protocol (TS mirror contract): JSON args with a `fn` discriminator;
//! results are `{ result: ... }`, errors surface as `{"error": msg}`.
//!
//! Common args:
//! - `providerId`
//! - `base`: scripted base provider `{ name?, baseUrl?, headers?,
//!   models: [...], allModels?, auth?: { apiKey?: { name?, check?, resolve? },
//!   oauth?: { name, isSubscription?, toAuth? } }, refreshModels?: { models
//!   } | { error } }` (absent = undefined). `check`/`resolve`/`toAuth` are
//!   scripted outcomes (`null` = the method returns undefined; key absent =
//!   the method itself is absent where optional).
//! - `modelConfig`: `{ providers: { pid: ... } }` (models.json shape)
//! - `extension`: scripted `ProviderConfigInput` `{ name?, baseUrl?,
//!   apiKey?, api?, models?, headers?, authHeader?, oauth?: { name,
//!   isSubscription?, getApiKey: string | { field }, modifyModels?: { op } },
//!   refreshModels?: { models } | { error }, images?: { api: { marker } },
//!   classifiers?: { api: { marker } } }`
//! - `env`, `credential` (`{ type: "api_key", key?, env? }` or an OAuth
//!   credential map)
//!
//! Fns:
//! - `validateExtensionProvider` → null | error
//! - `composeModelProvider` → `{ name, baseUrl, headers, getModels,
//!   getAllModels, apiKey, oauth, hasRefreshModels }` | error
//! - `refreshModels` { publishAccept?, credential?, aborted? } →
//!   `{ getModels, getAllModels, published: { update } | null }` | error
//! - `resolveConfiguredModelHeaders` { model } → headers | null | error
//! - `resolveCompatibilityRequestConfig` { model } → `{ headers, authHeader }`
//! - `configuredRequestAuthStatus` → status | null
//! - `apiKeyCheck` → `{ source }` | null | error
//! - `apiKeyResolve` → `{ auth, env, source }` | null | error
//! - `oauthToAuth` { credential } → `{ auth }` | error
//! - `generateImages` { model } → AssistantImages JSON | null
//! - `classify` { model } → ClassifierResult JSON | null
//!
//! `modifyModels` ops: `identity`, `reverse`, `dropFirst`, `prefixIds`.
//! Timestamps in image/classifier results are normalized to 0 on both sides.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use pi_providers::error::JsError;
use pi_providers::model_config::ModelConfig;
use pi_providers::pi_ai::credential::{
	AbortFlag, ApiKeyAuth, ApiKeyCredential, AuthCheck, AuthContext, AuthResult, AuthType,
	Credential, ModelAuth, OAuthAuth, OAuthCredential, ProviderAuth,
};
use pi_providers::pi_ai::model::ProviderHeaders;
use pi_providers::pi_ai::models::RefreshModelsContext;
use pi_providers::pi_ai::model::{AnyModel, ClassifierModel, ImageModel, Model};
use pi_providers::pi_ai::provider::{
	GetAllModelsFn, GetModelsFn, Provider, StreamFn, StreamSimpleFn, TranscriptContext,
};
use pi_providers::provider_composer::{
	self, CompatibilityRequestConfig, ExtensionOAuthConfig, ProviderClassifier, ProviderConfigInput,
	ProviderImages, any_model_from_map,
};

pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
	if tool != "provider-composer" {
		return Err(format!("composer_harness: unknown tool {tool:?}"));
	}
	// Command config values execute through the shared cached runner.
	provider_composer::set_command_runner(Arc::new(ShellCommandRunner));
	let fn_name = input["fn"].as_str().ok_or("missing fn")?;
	match fn_name {
		"validateExtensionProvider" => {
			let provider_id = str_arg(input, "providerId")?;
			let base = build_base(input.get("base"))?;
			let extension = build_extension(input.get("extension"))?.ok_or("missing extension")?;
			let entry = provider_entry_arg(input, provider_id);
			provider_composer::validate_extension_provider(
				provider_id,
				base.as_ref(),
				entry.as_ref(),
				&extension,
			)
			.map_err(|e| e.message)?;
			Ok(json!({ "result": Value::Null }))
		}
		"composeModelProvider" => {
			let provider = compose(input)?;
			Ok(json!({ "result": provider_summary(&provider) }))
		}
		"refreshModels" => {
			let provider = compose(input)?;
			if provider.refresh_models_fn().is_none() {
				return Err("composed provider has no refreshModels".to_string());
			}
			let credential = build_credential(input.get("credential"))?;
			let mut published: Option<Value> = None;
			let mut publish = |publication: pi_providers::pi_ai::models::ModelsPublication| {
				published = Some(json!({ "update": publication.update.is_some() }));
				if let Some(update) = publication.update {
					update();
				}
				Ok(input["publishAccept"].as_bool().unwrap_or(true))
			};
			let signal = AbortFlag::new();
			if input["aborted"].as_bool().unwrap_or(false) {
				signal.abort();
			}
			{
				let mut context = RefreshModelsContext {
					credential: credential.as_ref(),
					stored: None,
					publish: &mut publish,
					allow_network: true,
					force: false,
					signal: Some(&signal),
					base_url: provider.base_url.as_deref(),
				};
				let refresh = provider.refresh_models_fn().expect("checked above");
				refresh(&mut context).map_err(|e| e.message)?;
			}
			Ok(json!({
				"result": {
					"getModels": provider.get_models().iter().map(Model::to_value).collect::<Vec<_>>(),
					"getAllModels": provider.get_all_models().iter().map(AnyModel::to_value).collect::<Vec<_>>(),
					"published": published.unwrap_or(Value::Null),
				}
			}))
		}
		"resolveConfiguredModelHeaders" => {
			let model = any_model_arg(input)?;
			let extension = build_extension(input.get("extension"))?;
			let env = parse_env(input);
			let provider_id = str_arg(input, "providerId")?;
			let entry = provider_entry_arg(input, provider_id);
			let headers = provider_composer::resolve_configured_model_headers(
				&model,
				entry.as_ref(),
				extension.as_ref(),
				Some(&env),
			)
			.map_err(|e| e.message)?;
			Ok(json!({ "result": headers_value(headers.as_deref()) }))
		}
		"resolveCompatibilityRequestConfig" => {
			let model = any_model_arg(input)?;
			let extension = build_extension(input.get("extension"))?;
			let env = parse_env(input);
			let CompatibilityRequestConfig { headers, auth_header } =
				provider_composer::resolve_compatibility_request_config(
					&model,
					provider_entry_arg(input, str_arg(input, "providerId")?).as_ref(),
					extension.as_ref(),
					Some(&env),
				)
				.map_err(|e| e.message)?;
			Ok(json!({
				"result": {
					"headers": headers_value(headers.as_deref()),
					"authHeader": auth_header,
				}
			}))
		}
		"configuredRequestAuthStatus" => {
			let extension = build_extension(input.get("extension"))?;
			let env = parse_env(input);
			let status = provider_composer::configured_request_auth_status(
				provider_entry_arg(input, str_arg(input, "providerId")?).as_ref(),
				extension.as_ref(),
				Some(&env),
			);
			Ok(json!({ "result": status.map(|s| s.to_value()).unwrap_or(Value::Null) }))
		}
		"apiKeyCheck" | "apiKeyResolve" => {
			let provider = compose(input)?;
			let auth = provider
				.auth
				.api_key
				.clone()
				.ok_or("no composed API key auth")?;
			let env = parse_env(input);
			let ctx = HarnessAuthContext { env };
			let credential = match build_credential(input.get("credential"))? {
				Some(Credential::ApiKey(credential)) => Some(credential),
				_ => None,
			};
			if fn_name == "apiKeyCheck" {
				let check = auth.check(&ctx, credential.as_ref(), None).map_err(|e| e.message)?;
				Ok(json!({
					"result": check
						.map(|AuthCheck { source, auth_type }| {
							let mut out = Map::new();
							out.insert("type".into(), json!(auth_type.as_str()));
							if let Some(source) = source {
								out.insert("source".into(), json!(source));
							}
							Value::Object(out)
						})
						.unwrap_or(Value::Null)
				}))
			} else {
				let resolved = auth.resolve(&ctx, credential.as_ref(), None).map_err(|e| e.message)?;
				Ok(json!({
					"result": resolved
						.map(|AuthResult { auth, env, source }| json!({
							"auth": auth.to_value(),
							"env": env_to_value(env.as_ref()),
							"source": source,
						}))
						.unwrap_or(Value::Null)
				}))
			}
		}
		"oauthToAuth" => {
			let provider = compose(input)?;
			let oauth = provider.auth.oauth.clone().ok_or("no composed OAuth auth")?;
			let credential = match build_credential(input.get("credential"))? {
				Some(Credential::OAuth(credential)) => credential,
				_ => return Err("oauthToAuth requires an OAuth credential".to_string()),
			};
			let auth = oauth.to_auth(&credential).map_err(|e| e.message)?;
			Ok(json!({ "result": auth.to_value() }))
		}
		"generateImages" | "classify" => {
			let provider = compose(input)?;
			if fn_name == "generateImages" {
				let model = image_model_arg(input)?;
				let result = provider.generate_images(&model, &TranscriptContext(Value::Null).into_images_context(), None);
				Ok(json!({ "result": result.map(|r| r.0).unwrap_or(Value::Null) }))
			} else {
				let model = classifier_model_arg(input)?;
				let result = provider.classify(&model, &TranscriptContext(Value::Null).into_classifier_context(), None);
				Ok(json!({ "result": result.map(|r| r.0).unwrap_or(Value::Null) }))
			}
		}
		other => Err(format!("unknown provider-composer fn: {other}")),
	}
}

// ============================================================================
// Shared scripted pieces
// ============================================================================

/// Real shell execution for `!command` config values (mirrors TS execSync).
struct ShellCommandRunner;

impl pi_providers::config_value::CommandRunner for ShellCommandRunner {
	fn run(&self, command: &str) -> Option<String> {
		let output = std::process::Command::new("/bin/sh")
			.arg("-c")
			.arg(command)
			.output()
			.ok()?;
		if !output.status.success() {
			return None;
		}
		let stdout = String::from_utf8_lossy(&output.stdout);
		let trimmed = stdout.trim();
		(!trimmed.is_empty()).then(|| trimmed.to_string())
	}
}

fn parse_env(input: &Value) -> pi_providers::config_value::Env {
	let mut env = pi_providers::config_value::Env::new();
	if let Some(map) = input.get("env").and_then(Value::as_object) {
		for (key, value) in map {
			if let Some(value) = value.as_str() {
				env.insert(key.clone(), value.to_string());
			}
		}
	}
	env
}

/// TS `AuthContext` over the injected env (fileExists is unexercised).
struct HarnessAuthContext {
	env: pi_providers::config_value::Env,
}

impl AuthContext for HarnessAuthContext {
	fn env(&self, name: &str) -> Option<String> {
		self.env.get(name).cloned()
	}
	fn file_exists(&self, _path: &str) -> bool {
		false
	}
}

fn env_to_value(env: Option<&pi_providers::pi_ai::model::ProviderEnv>) -> Value {
	match env {
		None => Value::Null,
		Some(env) => Value::Object(
			env.iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
		),
	}
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
	input[key].as_str().ok_or_else(|| format!("missing {key}"))
}

fn model_config_arg(input: &Value) -> Result<Option<Value>, String> {
	let providers = match input.get("modelConfig") {
		None | Some(Value::Null) => return Ok(None),
		Some(Value::Object(map)) => match map.get("providers") {
			Some(providers) => providers.clone(),
			None => json!({}),
		},
		Some(other) => return Err(format!("invalid modelConfig: {other}")),
	};
	Ok(Some(json!({ "providers": providers })))
}

/// The models.json provider entry for `providerId` (the direct-call fns take
/// the entry, not the wrapped `ModelConfig`).
fn provider_entry_arg(input: &Value, provider_id: &str) -> Option<Value> {
	input
		.get("modelConfig")
		.and_then(|m| m.get("providers"))
		.and_then(|p| p.get(provider_id))
		.cloned()
}

/// A `ModelConfig` over the injected providers map.
fn build_model_config(input: &Value) -> Result<ModelConfig, String> {
	let content = match model_config_arg(input)? {
		Some(config) => config.to_string(),
		None => json!({ "providers": {} }).to_string(),
	};
	Ok(ModelConfig::from_load(Some("/models.json"), Some(&content), None))
}

fn headers_value(headers: Option<&[(String, String)]>) -> Value {
	match headers {
		None | Some([]) => Value::Null,
		Some(pairs) => Value::Object(
			pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
		),
	}
}

fn provider_headers_value(headers: Option<&ProviderHeaders>) -> Value {
	match headers {
		None => Value::Null,
		Some(headers) => Value::Object(
			headers
				.iter()
				.map(|(k, v)| (k.clone(), json!(v)))
				.collect(),
		),
	}
}

// ============================================================================
// Scripted base provider
// ============================================================================

/// Scripted base `ApiKeyAuth`: `check`/`resolve` specs are fixed outcomes.
/// Key absent = method absent (check only); `null` = returns undefined.
struct ScriptedApiKeyAuth {
	name: String,
	check: Option<Value>,
	resolve: Option<Value>,
}

impl ApiKeyAuth for ScriptedApiKeyAuth {
	fn name(&self) -> &str {
		&self.name
	}
	fn has_check(&self) -> bool {
		self.check.is_some()
	}
	fn check(
		&self,
		_ctx: &dyn AuthContext,
		_credential: Option<&ApiKeyCredential>,
		_signal: Option<&AbortFlag>,
	) -> Result<Option<AuthCheck>, JsError> {
		match &self.check {
			None => Err(JsError::new("scripted base has no check")),
			Some(Value::Null) => Ok(None),
			Some(spec) => Ok(Some(AuthCheck {
				source: spec.get("source").and_then(Value::as_str).map(str::to_string),
				auth_type: AuthType::ApiKey,
			})),
		}
	}
	fn resolve(
		&self,
		_ctx: &dyn AuthContext,
		_credential: Option<&ApiKeyCredential>,
		_signal: Option<&AbortFlag>,
	) -> Result<Option<AuthResult>, JsError> {
		match &self.resolve {
			None | Some(Value::Null) => Ok(None),
			Some(spec) => {
				let auth = match spec.get("auth") {
					Some(auth) => ModelAuth::from_map(auth.as_object().cloned().unwrap_or_default())
						.map_err(|e| JsError::new(format!("invalid scripted auth: {e}")))?,
					None => ModelAuth::from_map(Map::new()).expect("empty ModelAuth"),
				};
				Ok(Some(AuthResult { auth, env: None, source: None }))
			}
		}
	}
}

/// Scripted base `OAuthAuth` with a fixed `toAuth` outcome.
struct ScriptedOAuthAuth {
	name: String,
	is_subscription: bool,
	to_auth: Value,
}

impl OAuthAuth for ScriptedOAuthAuth {
	fn name(&self) -> &str {
		&self.name
	}
	fn is_subscription(&self) -> bool {
		self.is_subscription
	}
	fn login_label(&self) -> Option<&str> {
		None
	}
	fn to_auth(&self, _credential: &OAuthCredential) -> Result<ModelAuth, JsError> {
		match &self.to_auth {
			Value::Null => Ok(ModelAuth::from_map(Map::new()).expect("empty ModelAuth")),
			spec => match spec.get("error") {
				Some(message) => Err(JsError::new(message.as_str().unwrap_or("error").to_string())),
				_ => Ok(ModelAuth::from_map(spec.as_object().cloned().unwrap_or_default())
					.map_err(|e| JsError::new(format!("invalid scripted toAuth: {e}")))?),
			},
		}
	}
}

fn dummy_stream() -> StreamFn {
	Box::new(|_model, _context, _options| pi_providers::pi_ai::provider::AssistantMessageEventStream(Value::Null))
}

fn dummy_stream_simple() -> StreamSimpleFn {
	Box::new(|_model, _context, _options| pi_providers::pi_ai::provider::AssistantMessageEventStream(Value::Null))
}

fn build_base(spec: Option<&Value>) -> Result<Option<Provider>, String> {
	let Some(spec) = spec.filter(|v| !v.is_null()) else {
		return Ok(None);
	};
	let models_value = spec.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
	let all_models_value = spec
		.get("allModels")
		.and_then(Value::as_array)
		.cloned()
		.unwrap_or_else(|| models_value.clone());
	let mut chat_models = Vec::new();
	for map in &models_value {
		if let Some(AnyModel::Chat(model)) = parse_model(map)? {
			chat_models.push(model);
		}
	}
	let chat_models = Arc::new(chat_models);
	let get_models: GetModelsFn = {
		let chat_models = chat_models.clone();
		Box::new(move || chat_models.to_vec())
	};
	let all_models = Arc::new(parse_models(&all_models_value)?);
	let get_all_models = {
		let all_models = all_models.clone();
		Box::new(move || all_models.to_vec()) as GetAllModelsFn
	};

	let mut api_key: Option<Arc<dyn ApiKeyAuth + Send + Sync>> = None;
	let mut oauth: Option<Arc<dyn OAuthAuth + Send + Sync>> = None;
	if let Some(auth) = spec.get("auth").and_then(Value::as_object) {
		if let Some(key_spec) = auth.get("apiKey").filter(|v| !v.is_null()) {
			api_key = Some(Arc::new(ScriptedApiKeyAuth {
				name: key_spec.get("name").and_then(Value::as_str).unwrap_or("API key").to_string(),
				check: key_spec.get("check").cloned(),
				resolve: Some(key_spec.get("resolve").cloned().unwrap_or(Value::Null)),
			}));
		}
		if let Some(oauth_spec) = auth.get("oauth").filter(|v| !v.is_null()) {
			oauth = Some(Arc::new(ScriptedOAuthAuth {
				name: oauth_spec.get("name").and_then(Value::as_str).unwrap_or("OAuth").to_string(),
				is_subscription: oauth_spec.get("isSubscription").and_then(Value::as_bool).unwrap_or(false),
				to_auth: oauth_spec.get("toAuth").cloned().unwrap_or(Value::Null),
			}));
		}
	}

	let mut provider = Provider::new(
		str_arg(spec, "id").map(str::to_string).unwrap_or_else(|_| "base".to_string()),
		spec.get("name").and_then(Value::as_str).unwrap_or("base").to_string(),
		ProviderAuth { api_key, oauth },
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

	if let Some(refresh_spec) = spec.get("refreshModels").filter(|v| !v.is_null()) {
		let refresh_spec = refresh_spec.clone();
		provider = provider.with_refresh_models(Box::new(
			move |_context: &mut RefreshModelsContext<'_>| match &refresh_spec {
				Value::Object(map) => {
					if let Some(error) = map.get("error").and_then(Value::as_str) {
						return Err(JsError::new(error.to_string()));
					}
					Ok(())
				}
				_ => Ok(()),
			},
		));
	}
	Ok(Some(provider))
}

// ============================================================================
// Scripted extension
// ============================================================================

fn parse_models(value: &[Value]) -> Result<Vec<AnyModel>, String> {
	value
		.iter()
		.filter_map(|map| parse_model(map).transpose())
		.collect()
}

fn parse_model(value: &Value) -> Result<Option<AnyModel>, String> {
	match value {
		Value::Object(map) => any_model_from_map(map.clone()).map(Some),
		_ => Err(format!("invalid model spec: {value}")),
	}
}

/// The scripted `modifyModels` ops.
fn apply_modify_models(op: &str, models: &[Map<String, Value>]) -> Vec<Map<String, Value>> {
	match op {
		"reverse" => models.iter().rev().cloned().collect(),
		"dropFirst" => models.iter().skip(1).cloned().collect(),
		"prefixIds" => models
			.iter()
			.map(|model| {
				let mut out = model.clone();
				if let Some(id) = out.get("id").and_then(Value::as_str) {
					out.insert("id".into(), json!(format!("x-{id}")));
				}
				out
			})
			.collect(),
		_ => models.to_vec(),
	}
}

fn build_extension(spec: Option<&Value>) -> Result<Option<ProviderConfigInput>, String> {
	let Some(spec) = spec.filter(|v| !v.is_null()) else {
		return Ok(None);
	};
	let object = spec.as_object().ok_or("extension must be an object")?;
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
				.map(|model| model.as_object().cloned().ok_or_else(|| "invalid extension model".to_string()))
				.collect::<Result<Vec<Map<String, Value>>, String>>()
		})
		.transpose()?;

	let oauth = match object.get("oauth").filter(|v| !v.is_null()) {
		None => None,
		Some(oauth) => {
			let oauth = oauth.as_object().ok_or("oauth must be an object")?;
			let name = oauth.get("name").and_then(Value::as_str).ok_or("missing oauth name")?.to_string();
			let get_api_key_spec = oauth.get("getApiKey").cloned().ok_or("missing oauth getApiKey")?;
			let get_api_key = move |credential: &OAuthCredential| -> String {
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
			let modify_models = oauth.get("modifyModels").and_then(|m| m.get("op")).and_then(Value::as_str).map(|op| {
				let op = op.to_string();
				Arc::new(move |models: &[Map<String, Value>], _credential: &OAuthCredential| {
					apply_modify_models(&op, models)
				}) as std::sync::Arc<
					dyn Fn(&[Map<String, Value>], &OAuthCredential) -> Vec<Map<String, Value>> + Send + Sync,
				>
			});
			Some(Arc::new(ExtensionOAuthConfig {
				name,
				is_subscription: oauth.get("isSubscription").and_then(Value::as_bool),
				uses_callback_server: oauth.get("usesCallbackServer").and_then(Value::as_bool),
				login: Arc::new(|_callbacks: &Value| Err(JsError::new("login not exercised"))),
				refresh_token: Arc::new(|_credentials: &pi_providers::pi_ai::credential::OAuthCredentials| {
					Err(JsError::new("refreshToken not exercised"))
				}),
				get_api_key: Arc::new(get_api_key),
				modify_models,
			}))
		}
	};

	let refresh_models = match object.get("refreshModels").filter(|v| !v.is_null()) {
		None => None,
		Some(spec) => {
			let spec = spec.clone();
			Some(Arc::new(
				move |_context: &mut RefreshModelsContext<'_>| -> Result<Vec<Map<String, Value>>, JsError> {
					if let Some(error) = spec.get("error").and_then(Value::as_str) {
						return Err(JsError::new(error.to_string()));
					}
					Ok(spec
						.get("models")
						.and_then(Value::as_array)
						.map(|models| {
							models
								.iter()
								.map(|model| model.as_object().cloned().unwrap_or_default())
								.collect()
						})
						.unwrap_or_default())
				},
			) as pi_providers::provider_composer::ExtensionRefreshModelsFn)
		}
	};

	let images = object
		.get("images")
		.and_then(Value::as_object)
		.map(|images| {
			images
				.iter()
				.filter_map(|(api, spec)| {
					let marker = spec.get("marker").and_then(Value::as_str)?.to_string();
					#[allow(clippy::type_complexity)]
					let generate_images: std::sync::Arc<
						dyn Fn(
							&ImageModel,
							&pi_providers::pi_ai::provider::ImagesContext,
							Option<&Value>,
						) -> pi_providers::pi_ai::provider::AssistantImages
							+ Send
							+ Sync,
					> = Arc::new(move |_model, _context, _options| {
						pi_providers::pi_ai::provider::AssistantImages(json!({ "marker": marker }))
					});
					Some((
						api.clone(),
						Arc::new(ProviderImages { generate_images }),
					))
				})
				.collect()
		});
	let classifiers = object
		.get("classifiers")
		.and_then(Value::as_object)
		.map(|classifiers| {
			classifiers
				.iter()
				.filter_map(|(api, spec)| {
					let marker = spec.get("marker").and_then(Value::as_str)?.to_string();
					#[allow(clippy::type_complexity)]
					let classify: std::sync::Arc<
						dyn Fn(
							&ClassifierModel,
							&pi_providers::pi_ai::provider::ClassifierContext,
							Option<&Value>,
						) -> pi_providers::pi_ai::provider::ClassifierResult
							+ Send
							+ Sync,
					> = Arc::new(move |_model, _context, _options| {
						pi_providers::pi_ai::provider::ClassifierResult(json!({ "marker": marker }))
					});
					Some((api.clone(), Arc::new(ProviderClassifier { classify })))
				})
				.collect()
		});

	Ok(Some(ProviderConfigInput {
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
	}))
}

fn build_credential(spec: Option<&Value>) -> Result<Option<Credential>, String> {
	let Some(spec) = spec.filter(|v| !v.is_null()) else {
		return Ok(None);
	};
	let map = spec.as_object().ok_or("credential must be an object")?.clone();
	match map.get("type").and_then(Value::as_str) {
		Some("api_key") => Ok(Some(Credential::ApiKey(
			ApiKeyCredential::from_map(map).map_err(|e| format!("invalid api key credential: {e}"))?,
		))),
		Some("oauth") => Ok(Some(Credential::OAuth(
			OAuthCredential::from_map(map).map_err(|e| format!("invalid oauth credential: {e}"))?,
		))),
		other => Err(format!("unknown credential type: {other:?}")),
	}
}

// ============================================================================
// Compose + summary
// ============================================================================

fn compose(input: &Value) -> Result<Provider, String> {
	let provider_id = str_arg(input, "providerId")?;
	let base = build_base(input.get("base"))?;
	let extension = build_extension(input.get("extension"))?;
	let model_config = build_model_config(input)?;
	provider_composer::compose_model_provider(
		provider_id,
		base,
		&model_config,
		extension,
	)
	.map_err(|e| e.message)
}

fn provider_summary(provider: &Provider) -> Value {
	json!({
		"name": provider.name,
		"baseUrl": provider.base_url,
		"headers": provider_headers_value(provider.headers.as_ref()),
		"getModels": provider.get_models().iter().map(Model::to_value).collect::<Vec<_>>(),
		"getAllModels": provider.get_all_models().iter().map(AnyModel::to_value).collect::<Vec<_>>(),
		"apiKey": provider.auth.api_key.as_ref().map(|a| a.name()),
		"oauth": provider.auth.oauth.as_ref().map(|o| o.name()),
		"hasRefreshModels": provider.refresh_models_fn().is_some(),
		"hasGenerateImages": provider.has_generate_images(),
		"hasClassify": provider.has_classify(),
	})
}

fn any_model_arg(input: &Value) -> Result<AnyModel, String> {
	let map = input["model"].as_object().ok_or("missing model")?.clone();
	any_model_from_map(map)
}

fn image_model_arg(input: &Value) -> Result<ImageModel, String> {
	let map = input["model"].as_object().ok_or("missing model")?.clone();
	Ok(match any_model_from_map(map)? {
		AnyModel::Image(model) => model,
		_ => return Err("expected an image model".to_string()),
	})
}

fn classifier_model_arg(input: &Value) -> Result<ClassifierModel, String> {
	let map = input["model"].as_object().ok_or("missing model")?.clone();
	Ok(match any_model_from_map(map)? {
		AnyModel::Classifier(model) => model,
		_ => return Err("expected a classifier model".to_string()),
	})
}

trait IntoContext {
	fn into_images_context(self) -> pi_providers::pi_ai::provider::ImagesContext;
	fn into_classifier_context(self) -> pi_providers::pi_ai::provider::ClassifierContext;
}

impl IntoContext for TranscriptContext {
	fn into_images_context(self) -> pi_providers::pi_ai::provider::ImagesContext {
		pi_providers::pi_ai::provider::ImagesContext(self.0)
	}
	fn into_classifier_context(self) -> pi_providers::pi_ai::provider::ClassifierContext {
		pi_providers::pi_ai::provider::ClassifierContext(self.0)
	}
}
