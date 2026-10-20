//! `ModelsImpl` + `createModels` — port of `pi-ai` `src/models.ts` l395–1000,
//! implementing the [`MutableModels`] trait from `pi_ai::models`.
//!
//! G8 sync decisions (see the sibling modules): every Promise-valued TS
//! operation is sync; `AbortSignal.any` collapses to checking both flags at
//! each TS `signal.aborted` site (nothing can abort mid-operation
//! single-threaded — supersede needs `&mut self`); publication chains are
//! vacuous (TS queues `publish` behind the previous publication's promise;
//! sync publications run inline in order).
//!
//! Documented divergences from the TS:
//! - `normalizeContext` is the caller's duty: the `Models` trait settled on
//!   `&TranscriptContext` parameters, so `ModelsImpl::stream` receives an
//!   already-normalized transcript (Phase 2 calls `normalize_context` first).
//! - The TS `try/catch` around provider `getModels()`/`getAllModels()` is
//!   vacuous: Rust closures cannot throw. Best-effort now means "unknown
//!   provider → no models", which is the observable part.
//! - `options.transformHeaders`/`options.signal` cannot ride in the JSON
//!   placeholder option records; `applyAuth` therefore never runs the
//!   Models-only header transform, and `generateImages`/`classify` always
//!   report `aborted: false` in their error results.
//! - `Date.now()` is `self.now_ms` (injectable; defaults to
//!   `pi_core::ai::date_now_millis`) — ports never read the ambient clock.
//! - The default auth context is empty (`EmptyAuthContext`), never
//!   `std::env` (spec rule); the TS default reads `process.env`.
//! - `OAuthAuth::refresh` runs inline without the TS 15s timeout (sync port;
//!   network flows are the async phase's concern). A refresh that started
//!   always persists its result (refresh-survives-abort), like the TS.
//! - `Models.login`'s interaction carries no abort signal (the Rust
//!   `AuthInteraction` trait settled without one), so the TS abort dance
//!   around the credential mutation is vacuous here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use pi_core::ai::date_now_millis;
use pi_tools::shell::AbortSignal;
use serde_json::{json, Map, Value};

use crate::error::JsError;
use crate::pi_ai::credential::{
	AuthCheck, AuthContext, AuthOperationOptions, AuthResult, AuthType, Credential,
	CredentialStore, OAuthAuth, OAuthCredential, ProviderAuth,
};
use crate::pi_ai::lazy::{is_error_stream, lazy_stream, stream_result};
use crate::pi_ai::model::{AnyModel, Model, ProviderEnv, ProviderHeaders};
use crate::pi_ai::model_operations::{
	assert_classifier_input_supported, classifier_error_result, image_error_result,
};
use crate::pi_ai::models::{
	AuthResolutionOverrides, ModelType, Models, ModelsPublication, ModelsRefreshOptions,
	ModelsRefreshResult, MutableModels, Persist, RefreshModelsContext, get_model_type,
	is_model_type,
};
use crate::pi_ai::models_error::{ModelsError, ModelsErrorKind};
use crate::pi_ai::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry};
use crate::pi_ai::provider::{
	AssistantImages, AssistantMessage, AssistantMessageEventStream, ClassifierContext,
	ClassifierOptions, ClassifierResult, DeferredFetchOptions, DeferredHandle, ImagesContext,
	ImagesOptions, Provider, RefreshModelsFn, SimpleStreamOptions, StreamOptions,
	TranscriptContext,
};

/// TS default OAuth validity floor: 5 minutes.
const DEFAULT_OAUTH_MINIMUM_VALIDITY_MS: f64 = 5.0 * 60.0 * 1000.0;

// ============================================================================
// Header merging (models.ts l381)
// ============================================================================

/// TS `mergeHeaders` — case-insensitive override: each override key deletes
/// every existing key with the same lowercase name (order of the survivors
/// preserved, `shift_remove`) and is then appended/updated; later entries
/// win. `None` only when both inputs are `None` (an empty map is a value,
/// like an empty JS object).
pub fn merge_headers(
	base: Option<&ProviderHeaders>,
	override_headers: Option<&ProviderHeaders>,
) -> Option<ProviderHeaders> {
	if base.is_none() && override_headers.is_none() {
		return None;
	}
	let mut merged = base.cloned().unwrap_or_default();
	if let Some(override_headers) = override_headers {
		for (name, value) in override_headers {
			let lower_name = name.to_lowercase();
			let matching: Vec<String> = merged
				.keys()
				.filter(|existing| existing.to_lowercase() == lower_name)
				.cloned()
				.collect();
			for existing in matching {
				merged.shift_remove(&existing);
			}
			merged.insert(name.clone(), value.clone());
		}
	}
	Some(merged)
}

// ============================================================================
// JSON helpers (ProviderEnv / ProviderHeaders <-> Value)
// ============================================================================

/// `ProviderHeaders` → ordered JSON object (`null` for `None` values).
fn provider_headers_to_value(headers: &ProviderHeaders) -> Value {
	let mut map = Map::new();
	for (name, value) in headers {
		map.insert(
			name.clone(),
			value.clone().map(Value::String).unwrap_or(Value::Null),
		);
	}
	Value::Object(map)
}

/// `ProviderEnv` → ordered JSON object.
fn provider_env_to_value(env: &ProviderEnv) -> Value {
	let mut map = Map::new();
	for (name, value) in env {
		map.insert(name.clone(), Value::String(value.clone()));
	}
	Value::Object(map)
}

/// A `Record<string, string>` option field → `ProviderEnv` (non-string
/// entries cannot occur from typed TS callers; they are skipped).
fn json_env(options: Option<&Value>, key: &str) -> Option<ProviderEnv> {
	let map = options?.get(key)?.as_object()?;
	Some(
		map.iter()
			.filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
			.collect(),
	)
}

/// A `Record<string, string | null>` option field → `ProviderHeaders`.
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

/// `Record<string, string>` (model `headers`) → `ProviderHeaders` (every
/// value `Some`).
fn env_headers(env: Option<&ProviderEnv>) -> Option<ProviderHeaders> {
	env.map(|e| e.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect())
}

/// Build an `ApiKeyCredential` (`{ type: "api_key", key?, env? }`) from
/// parts — the js_object structs construct via their JSON form.
fn api_key_credential(key: Option<String>, env: Option<ProviderEnv>) -> crate::pi_ai::credential::ApiKeyCredential {
	let mut map = Map::new();
	if let Some(key) = key {
		map.insert("key".to_string(), json!(key));
	}
	if let Some(env) = env {
		map.insert("env".to_string(), provider_env_to_value(&env));
	}
	crate::pi_ai::credential::ApiKeyCredential::from_map(map).expect("valid api_key credential")
}

/// Build a `ModelAuth` (`{ apiKey?, headers?, baseUrl? }`) from parts.
// (Test-fixture helper; the lib build never constructs a `ModelAuth` from
// parts — production code goes through `ApiKeyAuth`/`OAuthAuth` impls.)
#[cfg(test)]
fn model_auth(
	api_key: Option<String>,
	headers: Option<ProviderHeaders>,
	base_url: Option<String>,
) -> crate::pi_ai::credential::ModelAuth {
	let mut map = Map::new();
	if let Some(api_key) = api_key {
		map.insert("apiKey".to_string(), json!(api_key));
	}
	if let Some(headers) = headers {
		map.insert("headers".to_string(), json!(provider_headers_to_value(&headers)));
	}
	if let Some(base_url) = base_url {
		map.insert("baseUrl".to_string(), json!(base_url));
	}
	crate::pi_ai::credential::ModelAuth::from_map(map).expect("valid ModelAuth")
}

/// `OverlayAuthContext` — TS `overlayEnvAuthContext`: `env[name]` with JS
/// `||` fallback (empty overlay values fall through to the base context).
struct OverlayAuthContext<'a> {
	base: &'a dyn AuthContext,
	env: &'a ProviderEnv,
}

impl AuthContext for OverlayAuthContext<'_> {
	fn env(&self, name: &str) -> Option<String> {
		match self.env.get(name) {
			Some(value) if !value.is_empty() => Some(value.clone()),
			_ => self.base.env(name),
		}
	}

	fn file_exists(&self, path: &str) -> bool {
		self.base.file_exists(path)
	}
}

/// The default auth context: an empty environment. The TS default reads
/// `process.env`; ports never read ambient env (spec rule), so callers
/// inject a real context when env-based auth must resolve.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyAuthContext;

impl AuthContext for EmptyAuthContext {
	fn env(&self, _name: &str) -> Option<String> {
		None
	}

	fn file_exists(&self, _path: &str) -> bool {
		false
	}
}

// ============================================================================
// Refresh plumbing (free fns — unit-testable in isolation)
// ============================================================================

/// TS `supersedeProviderRefresh`: bump the generation, abort and drop the
/// running controller. Returns the new generation.
pub(crate) fn supersede_provider_refresh(
	refresh_generations: &mut HashMap<String, u64>,
	refresh_signals: &mut HashMap<String, AbortSignal>,
	provider_id: &str,
) -> u64 {
	let generation = refresh_generations.get(provider_id).copied().unwrap_or(0) + 1;
	refresh_generations.insert(provider_id.to_string(), generation);
	if let Some(previous) = refresh_signals.remove(provider_id) {
		previous.abort();
	}
	generation
}

/// TS `beginProviderRefresh`: supersede, then install a fresh controller.
pub(crate) fn begin_provider_refresh(
	refresh_generations: &mut HashMap<String, u64>,
	refresh_signals: &mut HashMap<String, AbortSignal>,
	provider_id: &str,
) -> (u64, AbortSignal) {
	let generation = supersede_provider_refresh(refresh_generations, refresh_signals, provider_id);
	let signal = AbortSignal::new();
	refresh_signals.insert(provider_id.to_string(), signal.clone());
	(generation, signal)
}

/// TS `withKnownModelTypes` — drop stored models whose type this version
/// does not know.
fn with_known_model_types(entry: &ModelsStoreEntry) -> ModelsStoreEntry {
	ModelsStoreEntry {
		models: entry
			.models
			.iter()
			.filter(|model| {
				matches!(
					get_model_type(model),
					ModelType::Chat | ModelType::Image | ModelType::Classifier
				)
			})
			.cloned()
			.collect(),
		last_modified: entry.last_modified,
		checked_at: entry.checked_at,
		etag: entry.etag.clone(),
	}
}

/// TS `publishProviderModels` — generation/abort-checked persistence, then
/// the synchronous in-memory catalog update. The TS publication chain
/// (queue behind the provider's previous publication) is vacuous sync:
/// publications run inline in call order.
pub(crate) fn publish_provider_models(
	models_store: &mut dyn ModelsStore,
	refresh_generations: &HashMap<String, u64>,
	provider_id: &str,
	generation: u64,
	signal: Option<&AbortSignal>,
	publication: ModelsPublication,
) -> Result<bool, JsError> {
	let superseded = |signal: Option<&AbortSignal>| {
		signal.is_some_and(AbortSignal::aborted)
			|| refresh_generations.get(provider_id) != Some(&generation)
	};
	if superseded(signal) {
		return Ok(false);
	}
	let options = AuthOperationOptions { signal };
	match publication.persist {
		Persist::Delete => models_store.delete(provider_id, Some(&options))?,
		Persist::Write(entry) => models_store.write(provider_id, &entry, Some(&options))?,
		Persist::Unchanged => {}
	}
	if superseded(signal) {
		return Ok(false);
	}
	if let Some(update) = publication.update {
		update();
	}
	Ok(true)
}

/// TS `runProviderRefreshPhase` — read the stored entry, hand the provider a
/// refresh context whose `publish` is generation-checked.
#[allow(clippy::too_many_arguments)] // faithful to the TS parameter list
fn run_provider_refresh_phase(
	refresh_fn: &RefreshModelsFn,
	provider: &Provider,
	credential: Option<&Credential>,
	allow_network: bool,
	force: bool,
	generation: u64,
	signal: &AbortSignal,
	models_store: &mut dyn ModelsStore,
	refresh_generations: &mut HashMap<String, u64>,
) -> Result<(), JsError> {
	let options = AuthOperationOptions { signal: Some(signal) };
	let stored = models_store.read(&provider.id, Some(&options))?;
	let filtered = stored.as_ref().map(with_known_model_types);
	let provider_id = provider.id.clone();
	let mut publish = |publication: ModelsPublication| {
		publish_provider_models(
			models_store,
			refresh_generations,
			&provider_id,
			generation,
			Some(signal),
			publication,
		)
	};
	let mut context = RefreshModelsContext {
		credential,
		stored: filtered.as_ref(),
		publish: &mut publish,
		allow_network,
		// TS: `force: allowNetwork ? force : undefined` — the restore phase
		// never forces.
		force: allow_network && force,
		signal: Some(signal),
		base_url: provider.base_url.as_deref(),
	};
	refresh_fn(&mut context)
}

/// TS `refreshStoredOAuthCredential` (auth/resolve.ts) — refresh under the
/// credential-store lock. A refresh that started always persists its result
/// (a rotated refresh token must not be discarded); `signal` cancels only
/// the wait for the lock, which is vacuous sync. The TS 15s refresh timeout
/// is an async concern and is not ported.
fn refresh_stored_oauth_credential(
	credentials: &mut dyn CredentialStore,
	provider_id: &str,
	oauth: &dyn OAuthAuth,
	needs_refresh: &dyn Fn(&OAuthCredential) -> bool,
	signal: Option<&AbortSignal>,
) -> Result<Option<OAuthCredential>, JsError> {
	let mut refresh_error: Option<ModelsError> = None;
	let options = AuthOperationOptions { signal };
	let result = credentials.modify(
		provider_id,
		&mut |current| {
			// signal.throwIfAborted() — past this check the refresh runs to
			// completion even if the caller's signal aborts.
			if signal.is_some_and(AbortSignal::aborted) {
				return Err(JsError::abort());
			}
			let Some(Credential::OAuth(current)) = current else {
				return Ok(None); // logged out meanwhile
			};
			if !needs_refresh(current) {
				return Ok(None); // another caller refreshed
			}
			match oauth.refresh(current) {
				Ok(next) => Ok(Some(Credential::OAuth(next))),
				Err(error) => {
					// TS: throw new ModelsError("oauth", `OAuth refresh failed …`)
					let error = ModelsError::with_cause(
						ModelsErrorKind::OAuth,
						format!("OAuth refresh failed for {provider_id}"),
						&error,
					);
					refresh_error = Some(error.clone());
					Err(error.into())
				}
			}
		},
		Some(&options),
	);
	match result {
		Ok(post) => Ok(post.and_then(|credential| match credential {
			Credential::OAuth(oauth) => Some(oauth),
			_ => None,
		})),
		Err(error) => Err(match refresh_error.take() {
			// TS: `if (error instanceof ModelsError) throw error;`
			Some(error) => error.into(),
			None if signal.is_some_and(AbortSignal::aborted) => JsError::abort(),
			None => ModelsError::with_cause(
				ModelsErrorKind::Auth,
				format!("Credential store modify failed for {provider_id}"),
				&error,
			)
			.into(),
		}),
	}
}

/// TS `resolveStoredOAuth` — double-checked refresh, then request-auth
/// derivation.
fn resolve_stored_oauth(
	credentials: &mut dyn CredentialStore,
	provider_id: &str,
	oauth: &dyn OAuthAuth,
	stored: OAuthCredential,
	signal: Option<&AbortSignal>,
	min_oauth_validity_ms: Option<f64>,
	now: f64,
) -> Result<Option<AuthResult>, JsError> {
	let minimum_validity_ms =
		DEFAULT_OAUTH_MINIMUM_VALIDITY_MS.max(min_oauth_validity_ms.unwrap_or(0.0));
	let expires_soon =
		|credential: &OAuthCredential| now + minimum_validity_ms >= credential.expires.get();
	let mut credential = stored;

	if expires_soon(&credential) {
		// Optimistic check said expired; the authoritative check runs under
		// the lock.
		let post = refresh_stored_oauth_credential(
			credentials,
			provider_id,
			oauth,
			&expires_soon,
			signal,
		)?;
		let Some(post) = post else {
			return Ok(None); // logged out meanwhile
		};
		credential = post;
		// Explicit callers require the requested minimum after the refresh.
		if min_oauth_validity_ms.is_some() && expires_soon(&credential) {
			return Err(ModelsError::new(
				ModelsErrorKind::OAuth,
				format!("OAuth refresh returned a token that expires too soon for {provider_id}"),
			)
			.into());
		}
	}

	let auth = oauth.to_auth(&credential).map_err(|error| {
		ModelsError::with_cause(
			ModelsErrorKind::OAuth,
			format!("OAuth auth derivation failed for {provider_id}"),
			&error,
		)
	})?;
	Ok(Some(AuthResult {
		auth,
		env: None,
		source: Some("OAuth".to_string()),
	}))
}

/// TS `resolveApiKey` (auth/resolve.ts).
fn resolve_api_key(
	auth_context: &dyn AuthContext,
	api_key: &dyn crate::pi_ai::credential::ApiKeyAuth,
	provider_id: &str,
	credential: Option<&crate::pi_ai::credential::ApiKeyCredential>,
	signal: Option<&AbortSignal>,
) -> Result<Option<AuthResult>, JsError> {
	api_key
		.resolve(auth_context, credential, signal)
		.map_err(|error| {
			ModelsError::with_cause(
				ModelsErrorKind::Auth,
				format!("API key auth failed for provider {provider_id}"),
				&error,
			)
			.into()
		})
}

// ============================================================================
// ModelsImpl
// ============================================================================

/// TS `CreateModelsOptions`.
#[derive(Default)]
pub struct CreateModelsOptions<'a> {
	pub credentials: Option<Box<dyn CredentialStore + 'a>>,
	pub models_store: Option<Box<dyn ModelsStore + 'a>>,
	pub auth_context: Option<Box<dyn AuthContext + 'a>>,
}

/// What `applyAuth` produced for one request.
struct AppliedAuth {
	api_key: Option<String>,
	headers: Option<ProviderHeaders>,
	env: Option<ProviderEnv>,
	base_url: Option<String>,
}

/// TS `ModelsImpl` — see the module docs for the sync decisions. The
/// credential store sits behind a `RefCell` because trait methods like
/// `getAuth`/`checkAuth` take `&self` while OAuth refresh mutates stored
/// credentials (TS `getAuth`'s `resolveStoredOAuth` → `credentials.modify`).
pub struct ModelsImpl<'a> {
	providers: IndexMap<String, Arc<Provider>>,
	credentials: RefCell<Box<dyn CredentialStore + 'a>>,
	models_store: Box<dyn ModelsStore + 'a>,
	auth_context: Box<dyn AuthContext + 'a>,
	refresh_generations: HashMap<String, u64>,
	/// TS `refreshControllers` (the signal half; `AbortSignal` is `Clone`).
	refresh_signals: HashMap<String, AbortSignal>,
	/// Injected clock (`Date.now()`); `None` → `date_now_millis()`.
	now_ms: Option<f64>,
}

impl<'a> ModelsImpl<'a> {
	pub fn new(options: CreateModelsOptions<'a>) -> ModelsImpl<'a> {
		ModelsImpl {
			providers: IndexMap::new(),
			credentials: RefCell::new(
				options
					.credentials
					.unwrap_or_else(|| Box::new(crate::pi_ai::credential::InMemoryCredentialStore::new())),
			),
			models_store: options
				.models_store
				.unwrap_or_else(|| Box::new(InMemoryModelsStore::new())),
			auth_context: options
				.auth_context
				.unwrap_or_else(|| Box::new(EmptyAuthContext)),
			refresh_generations: HashMap::new(),
			refresh_signals: HashMap::new(),
			now_ms: None,
		}
	}

	/// Deterministic clock for tests (`Date.now()`).
	pub fn with_now_ms(mut self, now_ms: f64) -> ModelsImpl<'a> {
		self.now_ms = Some(now_ms);
		self
	}

	fn now(&self) -> f64 {
		self.now_ms.unwrap_or_else(date_now_millis)
	}

	/// TS `readCredential` — read with the `ModelsError("auth", …)` wrap.
	fn read_credential(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Credential>, JsError> {
		let credentials = self.credentials.borrow_mut();
		credentials.read(provider_id, options).map_err(|error| {
			ModelsError::with_cause(
				ModelsErrorKind::Auth,
				format!("Credential store read failed for {provider_id}"),
				&error,
			)
			.into()
		})
	}

	/// TS `resolveProviderAuth` (auth/resolve.ts) — a stored credential owns
	/// the provider; ambient/env is consulted only when nothing is stored.
	fn resolve_provider_auth(
		&self,
		provider_id: &str,
		auth: &ProviderAuth,
		overrides: Option<&AuthResolutionOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError> {
		let signal = overrides.and_then(|o| o.signal);
		if signal.is_some_and(AbortSignal::aborted) {
			return Err(JsError::abort());
		}
		let overlay_env = overrides.and_then(|o| o.env.clone());
		let overlay;
		let request_context: &dyn AuthContext = match &overlay_env {
			Some(env) => {
				overlay = OverlayAuthContext { base: self.auth_context.as_ref(), env };
				&overlay
			}
			None => self.auth_context.as_ref(),
		};

		// Explicit api-key override short-circuits storage and ambient env.
		if let (Some(api_key), Some(key)) =
			(auth.api_key.as_deref(), overrides.and_then(|o| o.api_key.clone()))
		{
			let credential = api_key_credential(Some(key), overlay_env.clone());
			return resolve_api_key(request_context, api_key, provider_id, Some(&credential), signal);
		}

		let options = AuthOperationOptions { signal };
		let stored = {
			let credentials = self.credentials.borrow_mut();
			credentials.read(provider_id, Some(&options)).map_err(|error| -> JsError {
				ModelsError::with_cause(
					ModelsErrorKind::Auth,
					format!("Credential store read failed for {provider_id}"),
					&error,
				)
				.into()
			})?
		};
		if let Some(stored) = stored {
			match stored {
				Credential::OAuth(oauth_credential) => {
					if let Some(oauth) = auth.oauth.as_deref() {
						return resolve_stored_oauth(
							self.credentials.borrow_mut().as_mut(),
							provider_id,
							oauth,
							oauth_credential,
							signal,
							overrides.and_then(|o| o.min_oauth_validity_ms),
							self.now(),
						);
					}
					return Ok(None); // stored type without a matching handler
				}
				Credential::ApiKey(api_key_credential) => {
					if let Some(api_key) = auth.api_key.as_deref() {
						// TS: `{ ...stored, env: { ...stored.env, ...overrides.env } }`
						let credential = match &overlay_env {
							Some(env) => {
								let mut value = api_key_credential.to_value();
								let mut merged =
									api_key_credential.env.clone().unwrap_or_default();
								for (name, v) in env {
									merged.insert(name.clone(), v.clone());
								}
								value
									.as_object_mut()
									.expect("to_value returns an object")
									.insert("env".to_string(), provider_env_to_value(&merged));
								crate::pi_ai::credential::ApiKeyCredential::from_map(
									value.as_object().expect("object").clone(),
								)
								.expect("valid api_key credential")
							}
							None => api_key_credential,
						};
						return resolve_api_key(
							request_context,
							api_key,
							provider_id,
							Some(&credential),
							signal,
						);
					}
					return Ok(None);
				}
			}
		}

		// Ambient (env vars, AWS profiles, ADC files).
		match auth.api_key.as_deref() {
			Some(api_key) => {
				resolve_api_key(request_context, api_key, provider_id, None, signal)
			}
			None => Ok(None),
		}
	}

	/// `getAuth` model overload without the model lookup: resolve, then
	/// merge the model's static headers into the request auth headers.
	fn resolve_auth_with_model_headers(
		&self,
		provider_id: &str,
		model_headers: Option<&ProviderHeaders>,
		overrides: Option<&AuthResolutionOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError> {
		let Some(provider) = self.providers.get(provider_id) else {
			return Ok(None);
		};
		let mut result = self.resolve_provider_auth(provider_id, &provider.auth, overrides)?;
		if let (Some(result), Some(headers)) = (&mut result, model_headers) {
			result.auth.headers = merge_headers(result.auth.headers.as_ref(), Some(headers));
		}
		Ok(result)
	}

	/// TS `resolveRefreshCredential` — the credential the network refresh
	/// phase runs with. `apiKey.resolve` errors propagate raw (no wrap,
	/// unlike `resolveProviderAuth`).
	fn resolve_refresh_credential(
		&self,
		provider: &Provider,
		stored: Option<&Credential>,
		signal: &AbortSignal,
	) -> Result<Option<Credential>, JsError> {
		if let Some(Credential::OAuth(stored)) = stored {
			let Some(oauth) = provider.auth.oauth.as_deref() else {
				return Ok(None);
			};
			if self.now() < stored.expires.get() {
				return Ok(Some(Credential::OAuth(stored.clone())));
			}
			if signal.aborted() {
				return Ok(None);
			}
			// A refresh that has started survives cancellation or a
			// superseding model refresh, so a rotated refresh token is
			// always persisted.
			let now = self.now();
			let needs_refresh = |current: &OAuthCredential| now >= current.expires.get();
			return refresh_stored_oauth_credential(
				self.credentials.borrow_mut().as_mut(),
				&provider.id,
				oauth,
				&needs_refresh,
				Some(signal),
			)
			.map(|oauth| oauth.map(Credential::OAuth));
		}

		let Some(api_key) = provider.auth.api_key.as_deref() else {
			return Ok(None);
		};
		let credential = match stored {
			Some(Credential::ApiKey(credential)) => Some(credential),
			_ => None,
		};
		let Some(result) = api_key.resolve(self.auth_context.as_ref(), credential, Some(signal))?
		else {
			return Ok(None);
		};
		Ok(Some(Credential::ApiKey(api_key_credential(
			result.auth.api_key.clone(),
			result.env.clone(),
		))))
	}

	/// TS `checkProviderAuth`.
	fn check_provider_auth(
		&self,
		provider: &Provider,
		credential: Option<&Credential>,
		signal: Option<&AbortSignal>,
	) -> Result<Option<AuthCheck>, JsError> {
		if let Some(Credential::OAuth(_)) = credential {
			return Ok(provider.auth.oauth.is_some().then(|| AuthCheck {
				source: Some("OAuth".to_string()),
				auth_type: AuthType::OAuth,
			}));
		}
		let Some(api_key) = provider.auth.api_key.as_deref() else {
			return Ok(None);
		};
		if api_key.has_check() {
			let credential = match credential {
				Some(Credential::ApiKey(credential)) => Some(credential),
				_ => None,
			};
			return api_key
				.check(self.auth_context.as_ref(), credential, signal)
				.map_err(|error| {
					ModelsError::with_cause(
						ModelsErrorKind::Auth,
						format!("API key auth check failed for provider {}", provider.id),
						&error,
					)
					.into()
				});
		}
		let overrides = AuthResolutionOverrides {
			api_key: None,
			env: None,
			min_oauth_validity_ms: None,
			signal,
		};
		let resolution = self.resolve_provider_auth(&provider.id, &provider.auth, Some(&overrides))?;
		Ok(resolution.map(|resolution| AuthCheck {
			source: resolution.source,
			auth_type: AuthType::ApiKey,
		}))
	}

	/// TS `getAuthenticatedProviders` — providers with a complete auth
	/// configuration, with the credential each check ran against.
	fn authenticated_providers(
		&self,
		provider_id: Option<&str>,
		signal: Option<&AbortSignal>,
	) -> Result<Vec<(&Provider, Option<Credential>)>, JsError> {
		if signal.is_some_and(AbortSignal::aborted) {
			return Err(JsError::abort());
		}
		let providers: Vec<&Provider> = match provider_id {
			Some(provider_id) => self
				.providers
				.get(provider_id)
				.map(|p| p.as_ref())
				.into_iter()
				.collect(),
			None => self.providers.values().map(|p| p.as_ref()).collect(),
		};
		let options = AuthOperationOptions { signal };
		let mut authenticated = Vec::new();
		for provider in providers {
			let credential = self.read_credential(&provider.id, Some(&options))?;
			if self
				.check_provider_auth(provider, credential.as_ref(), signal)?
				.is_some()
			{
				authenticated.push((provider, credential));
			}
		}
		Ok(authenticated)
	}

	fn require_provider(&self, provider_id: &str) -> Result<&Provider, JsError> {
		self.providers.get(provider_id).map(|p| p.as_ref()).ok_or_else(|| {
			ModelsError::new(
				ModelsErrorKind::Provider,
				format!("Unknown provider: {provider_id}"),
			)
			.into()
		})
	}

	/// TS `requireChatProvider` — assert the model is a chat model, then the
	/// provider lookup. (`assertChatModel` on the already-chat-typed Rust
	/// parameter; only a hand-built `Model` with a foreign `type` tag fails.)
	fn require_chat_provider(&self, model: &Model) -> Result<&Provider, JsError> {
		if model.model_type.as_deref().unwrap_or("chat") != "chat" {
			return Err(ModelsError::new(
				ModelsErrorKind::Provider,
				format!("Model {}/{} is not a chat model", model.provider, model.id),
			)
			.into());
		}
		self.require_provider(&model.provider)
	}

	/// TS `applyAuth` — resolve auth for the model, then build the request
	/// overlays. `model_headers` are the model's static headers (the TS
	/// `getAuth(model)` overload merges them into the resolved auth headers).
	fn apply_auth(
		&self,
		provider_id: &str,
		model_headers: Option<ProviderHeaders>,
		options: Option<&Value>,
	) -> Result<AppliedAuth, JsError> {
		self.require_provider(provider_id)?;
		let overrides = AuthResolutionOverrides {
			api_key: options
				.and_then(|o| o.get("apiKey"))
				.and_then(Value::as_str)
				.map(str::to_string),
			env: json_env(options, "env"),
			min_oauth_validity_ms: None,
			signal: None,
		};
		let Some(resolution) =
			self.resolve_auth_with_model_headers(provider_id, model_headers.as_ref(), Some(&overrides))?
		else {
			return Err(ModelsError::new(
				ModelsErrorKind::Auth,
				format!("Provider is not configured: {provider_id}"),
			)
			.into());
		};
		let auth = resolution.auth;

		// Explicit request options win per-field; the Models-only
		// `transformHeaders` transform cannot ride in JSON options (see the
		// module docs).
		let api_key = options
			.and_then(|o| o.get("apiKey"))
			.and_then(Value::as_str)
			.map(str::to_string)
			.or(auth.api_key.clone());
		let headers = merge_headers(auth.headers.as_ref(), json_headers(options, "headers").as_ref());
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
		Ok(AppliedAuth { api_key, headers, env, base_url: auth.base_url.clone() })
	}

	/// TS `requestModel = auth.baseUrl ? { ...model, baseUrl } : model` —
	/// the override applies only for a non-empty base URL (JS truthiness).
	fn apply_base_url<M: BaseUrlModel>(model: &M, applied: &AppliedAuth) -> M {
		let mut request_model = model.clone();
		if let Some(base_url) = &applied.base_url {
			if !base_url.is_empty() {
				request_model.set_base_url(base_url.clone());
			}
		}
		request_model
	}
}

/// Models with an overridable `baseUrl` (the `applyAuth` request-model
/// spread).
trait BaseUrlModel: Clone {
	fn set_base_url(&mut self, base_url: String);
}

impl BaseUrlModel for Model {
	fn set_base_url(&mut self, base_url: String) {
		self.base_url = base_url;
	}
}

impl BaseUrlModel for crate::pi_ai::model::ImageModel {
	fn set_base_url(&mut self, base_url: String) {
		self.base_url = base_url;
	}
}

impl BaseUrlModel for crate::pi_ai::model::ClassifierModel {
	fn set_base_url(&mut self, base_url: String) {
		self.base_url = base_url;
	}
}

/// TS `requestOptions = { ...options minus transformHeaders, apiKey,
/// headers, env }` — present keys keep their position (JS spread), absent
/// `Some` values are appended, `None` values are removed (JSON.stringify
/// drops `undefined`).
fn build_request_options(options: Option<&Value>, applied: &AppliedAuth) -> Value {
	let mut map: Map<String, Value> = match options.and_then(Value::as_object) {
		Some(options) => options.clone(),
		None => Map::new(),
	};
	map.shift_remove("transformHeaders");
	match &applied.api_key {
		Some(api_key) => {
			map.insert("apiKey".to_string(), json!(api_key));
		}
		None => {
			map.shift_remove("apiKey");
		}
	}
	match &applied.headers {
		Some(headers) => {
			map.insert("headers".to_string(), provider_headers_to_value(headers));
		}
		None => {
			map.shift_remove("headers");
		}
	}
	match &applied.env {
		Some(env) => {
			map.insert("env".to_string(), provider_env_to_value(env));
		}
		None => {
			map.shift_remove("env");
		}
	}
	Value::Object(map)
}

/// The settled complete/fetchDeferred convention: a successful stream
/// resolves with its terminal result (`AssistantMessage(stream.0)` in the
/// placeholder world); setup errors propagate as `Err` (TS `result()`
/// rejects).
pub fn stream_terminal_result(stream: &AssistantMessageEventStream) -> Result<AssistantMessage, JsError> {
	if is_error_stream(stream) {
		let message = stream_result(stream)
			.map(|message| message.0)
			.unwrap_or_else(|| json!({}));
		let error = message
			.get("errorMessage")
			.and_then(Value::as_str)
			.unwrap_or("stream error");
		return Err(JsError::new(error));
	}
	Ok(AssistantMessage(stream.0.clone()))
}

impl Models for ModelsImpl<'_> {
	fn get_providers(&self) -> Vec<&Provider> {
		self.providers.values().map(|p| p.as_ref()).collect()
	}

	fn get_provider(&self, id: &str) -> Option<&Provider> {
		self.providers.get(id).map(|p| p.as_ref())
	}

	fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
		// TS try/catch is vacuous (closures cannot throw).
		match provider {
			Some(provider) => match self.providers.get(provider) {
				Some(provider) => provider.get_models(),
				None => Vec::new(),
			},
			None => self
				.providers
				.values()
				.flat_map(|provider| provider.get_models())
				.collect(),
		}
	}

	fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
		self.get_models(Some(provider)).into_iter().find(|model| model.id == id)
	}

	fn get_models_of_type(&self, ty: &ModelType, provider: Option<&str>) -> Vec<AnyModel> {
		self.get_all_models(provider)
			.into_iter()
			.filter(|model| is_model_type(model, ty))
			.collect()
	}

	fn get_model_of_type(
		&self,
		ty: &ModelType,
		provider: &str,
		id: &str,
	) -> Option<AnyModel> {
		self.get_models_of_type(ty, Some(provider))
			.into_iter()
			.find(|model| model.id() == id)
	}

	fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel> {
		// TS try/catch is vacuous (closures cannot throw).
		match provider {
			Some(provider) => match self.providers.get(provider) {
				Some(provider) => provider.get_all_models(),
				None => Vec::new(),
			},
			None => self
				.providers
				.values()
				.flat_map(|provider| provider.get_all_models())
				.collect(),
		}
	}

	fn refresh(&mut self, options: Option<&ModelsRefreshOptions<'_>>) -> ModelsRefreshResult {
		let allow_network = options.and_then(|o| o.allow_network).unwrap_or(true);
		let force = options.is_some_and(|o| o.force);
		let caller_signal = options.and_then(|o| o.signal);
		let mut result = ModelsRefreshResult::default();
		if caller_signal.is_some_and(AbortSignal::aborted) {
			result.aborted = true;
			return result;
		}
		let selected = options.and_then(|o| o.providers.as_deref());

		// Borrow strategy (settled): take the mutable state the publish
		// closure needs out of `self` so providers can be borrowed while a
		// publication holds `&mut` to the store and generations.
		let mut models_store: Box<dyn ModelsStore> =
			std::mem::replace(&mut self.models_store, Box::new(InMemoryModelsStore::new()));
		let mut refresh_generations = std::mem::take(&mut self.refresh_generations);
		let mut errors = IndexMap::new();

		for provider in self.providers.values() {
			let refresh_fn = match provider.refresh_models_fn() {
				Some(refresh_fn) => refresh_fn,
				None => continue, // static provider
			};
			if let Some(selected) = selected {
				if !selected.iter().any(|id| id == &provider.id) {
					continue;
				}
			}

			let (generation, controller) = begin_provider_refresh(
				&mut refresh_generations,
				&mut self.refresh_signals,
				&provider.id,
			);
			// TS `AbortSignal.any([callerSignal, controller.signal])` — a
			// snapshot is exact here: supersede needs `&mut self`, so no
			// other controller can replace this one mid-refresh.
			let signal = AbortSignal::new();
			if caller_signal.is_some_and(AbortSignal::aborted) || controller.aborted() {
				signal.abort();
			}

			let operation = (|| -> Result<(), JsError> {
				// Read the stored credential, but restore cached provider
				// state before auth resolution or network access (and before
				// surfacing the credential error, like the TS).
				let stored_credential = self.read_credential(&provider.id, Some(&AuthOperationOptions {
					signal: Some(&signal),
				}));
				let credential_error = stored_credential.as_ref().err().cloned();
				let stored_credential = stored_credential.unwrap_or(None);

				run_provider_refresh_phase(
					refresh_fn,
					provider,
					stored_credential.as_ref(),
					false,
					force,
					generation,
					&signal,
					models_store.as_mut(),
					&mut refresh_generations,
				)?;

				if let Some(credential_error) = credential_error {
					return Err(credential_error);
				}
				if !allow_network || signal.aborted() {
					return Ok(());
				}

				let credential =
					self.resolve_refresh_credential(provider, stored_credential.as_ref(), &signal)?;
				let Some(credential) = credential else {
					return Ok(());
				};
				run_provider_refresh_phase(
					refresh_fn,
					provider,
					Some(&credential),
					true,
					force,
					generation,
					&signal,
					models_store.as_mut(),
					&mut refresh_generations,
				)?;
				Ok(())
			})();

			if let Err(error) = operation {
				if !signal.aborted() {
					errors.insert(provider.id.clone(), error);
				}
			}
			// TS finally: drop the controller when it is still the current
			// one — always true single-threaded (supersede needs `&mut self`).
			self.refresh_signals.remove(&provider.id);
		}

		self.models_store = models_store;
		self.refresh_generations = refresh_generations;
		result.aborted = caller_signal.is_some_and(AbortSignal::aborted);
		result.errors = errors;
		result
	}

	fn check_auth(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<AuthCheck>, JsError> {
		crate::pi_ai::credential::check_aborted(options)?;
		let Some(provider) = self.providers.get(provider_id) else {
			return Ok(None);
		};
		let credential = self.read_credential(provider_id, options)?;
		self.check_provider_auth(provider, credential.as_ref(), options.and_then(|o| o.signal))
	}

	fn get_available(
		&self,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<Model>, JsError> {
		let signal = options.and_then(|o| o.signal);
		let authenticated = self.authenticated_providers(provider_id, signal)?;
		Ok(authenticated
			.into_iter()
			.flat_map(|(provider, credential)| {
				provider.filter_models(&provider.get_models(), credential.as_ref())
			})
			.collect())
	}

	fn get_available_of_type(
		&self,
		ty: &ModelType,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<AnyModel>, JsError> {
		Ok(self
			.get_all_available(provider_id, options)?
			.into_iter()
			.filter(|model| is_model_type(model, ty))
			.collect())
	}

	fn get_all_available(
		&self,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<AnyModel>, JsError> {
		let signal = options.and_then(|o| o.signal);
		let authenticated = self.authenticated_providers(provider_id, signal)?;
		Ok(authenticated
			.into_iter()
			.flat_map(|(provider, credential)| {
				let models = provider.get_all_models();
				if provider.has_filter_all_models() {
					return provider.filter_all_models(&models, credential.as_ref());
				}
				if !provider.has_filter_models() {
					return models;
				}
				let available_chat_ids: std::collections::HashSet<String> = provider
					.filter_models(&provider.get_models(), credential.as_ref())
					.iter()
					.map(|model| model.id.clone())
					.collect();
				models
					.into_iter()
					.filter(|model| {
						!is_model_type(model, &ModelType::Chat)
							|| available_chat_ids.contains(model.id())
					})
					.collect()
			})
			.collect())
	}

	fn get_auth_for_provider(
		&self,
		provider_id: &str,
		overrides: Option<&AuthResolutionOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError> {
		let Some(provider) = self.providers.get(provider_id) else {
			return Ok(None);
		};
		self.resolve_provider_auth(provider_id, &provider.auth, overrides)
	}

	fn get_auth_for_model(
		&self,
		model: &AnyModel,
		overrides: Option<&AuthResolutionOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError> {
		self.resolve_auth_with_model_headers(
			model.provider(),
			model_headers(model).as_ref(),
			overrides,
		)
	}

	fn login(
		&mut self,
		provider_id: &str,
		ty: AuthType,
		interaction: &mut dyn crate::pi_ai::models::AuthInteraction,
		options: Option<&crate::pi_ai::models::LoginOptions>,
	) -> Result<Credential, JsError> {
		let Some(provider) = self.providers.get(provider_id) else {
			return Err(ModelsError::new(
				ModelsErrorKind::Provider,
				format!("Unknown provider: {provider_id}"),
			)
			.into());
		};
		let provider_name = provider.name.clone();
		// TS: method = type === "oauth" ? provider.auth.oauth : provider.auth.apiKey
		let login_result = match ty {
			AuthType::OAuth => provider
				.auth
				.oauth
				.as_ref()
				.filter(|oauth| oauth.has_login())
				.map(|oauth| oauth.login(interaction, options)),
			AuthType::ApiKey => provider
				.auth
				.api_key
				.as_ref()
				.filter(|api_key| api_key.has_login())
				.map(|api_key| api_key.login(interaction, options)),
		};
		let credential = match login_result {
			Some(credential) => credential?,
			None => {
				return Err(ModelsError::new(
					ModelsErrorKind::Auth,
					format!("{provider_name} does not support {} login", ty.as_str()),
				)
				.into())
			}
		};
		// Persist via the store's modify (the TS abort dance around the
		// mutation is vacuous sync; the Rust AuthInteraction carries no
		// signal).
		let mutation = {
			let mut credentials = self.credentials.borrow_mut();
			credentials.modify(provider_id, &mut |_| Ok(Some(credential.clone())), None)
		};
		match mutation {
			Ok(_) => Ok(credential),
			Err(error) => Err(ModelsError::with_cause(
				ModelsErrorKind::Auth,
				format!("Credential store modify failed for {provider_id}"),
				&error,
			)
			.into()),
		}
	}

	fn logout(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<(), JsError> {
		crate::pi_ai::credential::check_aborted(options)?;
		let signal = options.and_then(|o| o.signal);
		let mut credentials = self.credentials.borrow_mut();
		credentials.delete(provider_id, options).map_err(|error| {
			// TS: signal.throwIfAborted() before wrapping.
			if signal.is_some_and(AbortSignal::aborted) {
				JsError::abort()
			} else {
				ModelsError::with_cause(
					ModelsErrorKind::Auth,
					format!("Credential store delete failed for {provider_id}"),
					&error,
				)
				.into()
			}
		})
	}

	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&StreamOptions>,
	) -> AssistantMessageEventStream {
		lazy_stream(model, self.now(), || {
			let provider = self.require_chat_provider(model)?;
			let applied = self.apply_auth(
				&model.provider,
				env_headers(model.headers.as_ref()),
				options,
			)?;
			let request_model = ModelsImpl::apply_base_url(model, &applied);
			let request_options = build_request_options(options, &applied);
			Ok(provider.stream(&request_model, context, Some(&request_options)))
		})
	}

	fn complete(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&StreamOptions>,
	) -> Result<AssistantMessage, JsError> {
		stream_terminal_result(&self.stream(model, context, options))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&SimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		lazy_stream(model, self.now(), || {
			let provider = self.require_chat_provider(model)?;
			let applied = self.apply_auth(
				&model.provider,
				env_headers(model.headers.as_ref()),
				options,
			)?;
			let request_model = ModelsImpl::apply_base_url(model, &applied);
			let request_options = build_request_options(options, &applied);
			Ok(provider.stream_simple(&request_model, context, Some(&request_options)))
		})
	}

	fn complete_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&SimpleStreamOptions>,
	) -> Result<AssistantMessage, JsError> {
		stream_terminal_result(&self.stream_simple(model, context, options))
	}

	fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredFetchOptions>,
	) -> AssistantMessageEventStream {
		lazy_stream(model, self.now(), || {
			let provider = self.require_chat_provider(model)?;
			if !provider.has_fetch_deferred() {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Provider {} does not support deferred responses", model.provider),
				)
				.into());
			}
			let applied = self.apply_auth(
				&model.provider,
				env_headers(model.headers.as_ref()),
				options,
			)?;
			let request_model = ModelsImpl::apply_base_url(model, &applied);
			let request_options = build_request_options(options, &applied);
			Ok(provider
				.fetch_deferred(&request_model, handle, Some(&request_options))
				.expect("checked has_fetch_deferred"))
		})
	}

	fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredFetchOptions>,
	) -> Result<AssistantMessage, JsError> {
		stream_terminal_result(&self.stream_deferred(model, handle, options))
	}

	fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&crate::pi_ai::provider::DeferredCancelOptions>,
	) -> Result<(), JsError> {
		let provider = self.require_chat_provider(model)?;
		if !provider.has_cancel_deferred() {
			return Err(ModelsError::new(
				ModelsErrorKind::Provider,
				format!("Provider {} does not support deferred responses", model.provider),
			)
			.into());
		}
		let applied =
			self.apply_auth(&model.provider, env_headers(model.headers.as_ref()), options)?;
		let request_model = ModelsImpl::apply_base_url(model, &applied);
		let request_options = build_request_options(options, &applied);
		provider
			.cancel_deferred(&request_model, handle, Some(&request_options))
			.expect("checked has_cancel_deferred")
	}

	fn generate_images(
		&self,
		model: &crate::pi_ai::model::ImageModel,
		context: &ImagesContext,
		options: Option<&ImagesOptions>,
	) -> AssistantImages {
		let result: Result<AssistantImages, JsError> = (|| {
			// TS assertImageModel (the parameter is already image-typed; a
			// hand-built model with a foreign `type` tag fails here).
			if model.model_type.as_deref() != Some("image") {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Model {}/{} is not an image model", model.provider, model.id),
				)
				.into());
			}
			let provider = self.require_provider(&model.provider)?;
			if !provider.has_generate_images() {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Provider {} does not support image generation", model.provider),
				)
				.into());
			}
			let applied =
				self.apply_auth(&model.provider, env_headers(model.headers.as_ref()), options)?;
			let request_model = ModelsImpl::apply_base_url(model, &applied);
			let request_options = build_request_options(options, &applied);
			Ok(provider
				.generate_images(&request_model, context, Some(&request_options))
				.expect("checked has_generate_images"))
		})();
		// options?.signal?.aborted cannot ride in JSON options (module docs).
		result.unwrap_or_else(|error| image_error_result(model, &error, false, self.now()))
	}

	fn classify(
		&self,
		model: &crate::pi_ai::model::ClassifierModel,
		context: &ClassifierContext,
		options: Option<&ClassifierOptions>,
	) -> ClassifierResult {
		let result: Result<ClassifierResult, JsError> = (|| {
			if model.model_type.as_deref() != Some("classifier") {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Model {}/{} is not a classifier model", model.provider, model.id),
				)
				.into());
			}
			assert_classifier_input_supported(model, context).map_err(JsError::from)?;
			let provider = self.require_provider(&model.provider)?;
			if !provider.has_classify() {
				return Err(ModelsError::new(
					ModelsErrorKind::Provider,
					format!("Provider {} does not support classification", model.provider),
				)
				.into());
			}
			let applied =
				self.apply_auth(&model.provider, env_headers(model.headers.as_ref()), options)?;
			let request_model = ModelsImpl::apply_base_url(model, &applied);
			let request_options = build_request_options(options, &applied);
			Ok(provider
				.classify(&request_model, context, Some(&request_options))
				.expect("checked has_classify"))
		})();
		result.unwrap_or_else(|error| classifier_error_result(model, &error, false, self.now()))
	}
}

/// The model's static `headers` (`Record<string, string>`), as
/// `ProviderHeaders` for merging.
fn model_headers(model: &AnyModel) -> Option<ProviderHeaders> {
	env_headers(match model {
		AnyModel::Chat(model) => model.headers.as_ref(),
		AnyModel::Image(model) => model.headers.as_ref(),
		AnyModel::Classifier(model) => model.headers.as_ref(),
	})
}

impl ModelsImpl<'_> {
	/// Shared-provider accessor: the provider handles behind `Arc`
	/// (callers that keep providers behind `Arc`, e.g. `ModelRuntime`,
	/// re-register them without cloning). Added for the model-runtime.ts
	/// port.
	pub fn provider_arcs(&self) -> Vec<Arc<Provider>> {
		self.providers.values().cloned().collect()
	}

	/// Shared-provider accessor for a single provider.
	pub fn provider_arc(&self, id: &str) -> Option<Arc<Provider>> {
		self.providers.get(id).cloned()
	}

	/// Shared-provider variant of [`MutableModels::set_provider`] for
	/// callers (e.g. `ModelRuntime`) that hold providers behind `Arc` and
	/// must re-register them without cloning. Added for the
	/// model-runtime.ts port; behavior is identical to the trait method.
	pub fn set_provider_shared(&mut self, provider: Arc<Provider>) {
		supersede_provider_refresh(
			&mut self.refresh_generations,
			&mut self.refresh_signals,
			&provider.id,
		);
		self.providers.insert(provider.id.clone(), provider);
	}
}

impl MutableModels for ModelsImpl<'_> {
	fn set_provider(&mut self, provider: Provider) {
		supersede_provider_refresh(
			&mut self.refresh_generations,
			&mut self.refresh_signals,
			&provider.id,
		);
		self.providers.insert(provider.id.clone(), Arc::new(provider));
	}

	fn delete_provider(&mut self, id: &str) {
		supersede_provider_refresh(&mut self.refresh_generations, &mut self.refresh_signals, id);
		self.providers.shift_remove(id);
	}

	fn clear_providers(&mut self) {
		// TS: supersede every provider and every running controller (the
		// union, deduplicated), then clear.
		let mut ids: Vec<String> = self.providers.keys().cloned().collect();
		for id in self.refresh_signals.keys() {
			if !ids.iter().any(|existing| existing == id) {
				ids.push(id.clone());
			}
		}
		for id in ids {
			supersede_provider_refresh(&mut self.refresh_generations, &mut self.refresh_signals, &id);
		}
		self.providers.clear();
	}
}

/// TS `createModels`.
pub fn create_models<'a>(options: CreateModelsOptions<'a>) -> ModelsImpl<'a> {
	ModelsImpl::new(options)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::credential::{
		ApiKeyCredential, InMemoryCredentialStore, OAuthCredential, ProviderAuth,
	};
	use crate::pi_ai::model::ClassifierModel;
	use crate::pi_ai::models::LoginOptions;
	use crate::pi_ai::provider::DeferredHandle;
	use serde_json::json;
	use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
	use std::sync::{Arc, Mutex};

	// -- fixtures -----------------------------------------------------------

	fn chat_model(provider: &str, id: &str) -> Model {
		serde_json::from_value(json!({
			"id": id, "name": id, "api": "a", "provider": provider,
			"baseUrl": "https://x", "input": ["text"],
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"reasoning": false, "contextWindow": 100, "maxTokens": 10
		}))
		.unwrap()
	}

	fn image_model(provider: &str, id: &str) -> crate::pi_ai::model::ImageModel {
		serde_json::from_value(json!({
			"id": id, "name": id, "api": "a", "provider": provider,
			"baseUrl": "https://x", "input": ["text"], "type": "image",
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"output": ["image"]
		}))
		.unwrap()
	}

	fn stored_credential(value: Value) -> Credential {
		Credential::from_map(value.as_object().unwrap().clone()).unwrap()
	}

	fn oauth(refresh: &str, access: &str, expires: f64) -> OAuthCredential {
		let map = json!({"refresh": refresh, "access": access, "expires": expires});
		OAuthCredential::from_map(map.as_object().unwrap().clone()).unwrap()
	}

	/// Env-var api-key auth: resolves the stored key, else the env var.
	struct EnvKeyAuth {
		env_name: &'static str,
	}

	impl crate::pi_ai::credential::ApiKeyAuth for EnvKeyAuth {
		fn name(&self) -> &str {
			"Env key"
		}

		fn check(
			&self,
			ctx: &dyn AuthContext,
			credential: Option<&ApiKeyCredential>,
			_signal: Option<&AbortSignal>,
		) -> Result<Option<AuthCheck>, JsError> {
			let configured = credential
				.and_then(|c| c.key.as_deref())
				.is_some_and(|k| !k.is_empty())
				|| ctx.env(self.env_name).is_some();
			Ok(configured.then(|| AuthCheck {
				source: Some(self.env_name.to_string()),
				auth_type: AuthType::ApiKey,
			}))
		}

		fn resolve(
			&self,
			ctx: &dyn AuthContext,
			credential: Option<&ApiKeyCredential>,
			_signal: Option<&AbortSignal>,
		) -> Result<Option<AuthResult>, JsError> {
			let key = credential
				.and_then(|c| c.key.clone())
				.or_else(|| ctx.env(self.env_name));
			Ok(key.map(|key| AuthResult {
				auth: model_auth(Some(key), None, None),
				env: None,
				source: Some(self.env_name.to_string()),
			}))
		}
	}

	struct TestOAuthAuth {
		refresh_count: Arc<AtomicUsize>,
		supports_login: bool,
	}

	impl OAuthAuth for TestOAuthAuth {
		fn name(&self) -> &str {
			"Test OAuth"
		}

		fn is_subscription(&self) -> bool {
			false
		}

		fn login_label(&self) -> Option<&str> {
			None
		}

		fn to_auth(&self, credential: &OAuthCredential) -> Result<crate::pi_ai::credential::ModelAuth, JsError> {
			Ok(model_auth(Some(credential.access.clone()), None, None))
		}

		fn refresh(&self, credential: &OAuthCredential) -> Result<OAuthCredential, JsError> {
			self.refresh_count.fetch_add(1, Ordering::SeqCst);
			Ok(oauth(
				&format!("{}+", credential.refresh),
				&format!("{}+", credential.access),
				credential.expires.get() + 1_000_000.0,
			))
		}

		fn has_login(&self) -> bool {
			self.supports_login
		}

		fn login(
			&self,
			_interaction: &mut dyn crate::pi_ai::models::AuthInteraction,
			_options: Option<&LoginOptions>,
		) -> Result<Credential, JsError> {
			Ok(Credential::OAuth(oauth("lr", "login-access", 9_000_000.0)))
		}
	}

	struct MapAuthContext {
		env: HashMap<String, String>,
	}

	impl AuthContext for MapAuthContext {
		fn env(&self, name: &str) -> Option<String> {
			self.env.get(name).cloned()
		}

		fn file_exists(&self, _path: &str) -> bool {
			false
		}
	}

	fn env_auth(env_name: &'static str) -> ProviderAuth {
		ProviderAuth {
			api_key: Some(Arc::new(EnvKeyAuth { env_name })),
			oauth: None,
		}
	}

	fn test_provider(id: &str, name: &str, auth: ProviderAuth, models: Vec<Model>) -> Provider {
		Provider::new(
			id.to_string(),
			name.to_string(),
			auth,
			Box::new(move || models.clone()),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
		)
	}

	fn models_with(env: HashMap<String, String>) -> ModelsImpl<'static> {
		ModelsImpl::new(CreateModelsOptions {
			credentials: Some(Box::new(InMemoryCredentialStore::new())),
			models_store: Some(Box::new(InMemoryModelsStore::new())),
			auth_context: Some(Box::new(MapAuthContext { env })),
		})
		.with_now_ms(1_000_000.0)
	}

	/// A credential store whose operations fail with a fixed message.
	struct FailingCredentialStore {
		inner: InMemoryCredentialStore,
	}

	impl CredentialStore for FailingCredentialStore {
		fn read(
			&self,
			provider_id: &str,
			options: Option<&AuthOperationOptions>,
		) -> Result<Option<Credential>, JsError> {
			self.inner.read(provider_id, options)
		}

		fn list(&self, options: Option<&AuthOperationOptions>) -> Result<Vec<crate::pi_ai::credential::CredentialInfo>, JsError> {
			self.inner.list(options)
		}

		fn modify(
			&mut self,
			provider_id: &str,
			f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
			options: Option<&AuthOperationOptions>,
		) -> Result<Option<Credential>, JsError> {
			self.inner.modify(provider_id, f, options)
		}

		fn delete(&mut self, _provider_id: &str, _options: Option<&AuthOperationOptions>) -> Result<(), JsError> {
			Err(JsError::new("disk on fire"))
		}
	}

	// -- refresh supersede (free fns) ---------------------------------------

	#[test]
	fn supersede_bumps_generation_and_aborts_controller() {
		let mut generations = HashMap::new();
		let mut signals = HashMap::new();
		let g1 = supersede_provider_refresh(&mut generations, &mut signals, "p");
		assert_eq!(g1, 1);
		assert_eq!(generations.get("p"), Some(&1));
		assert!(signals.is_empty(), "supersede removes the controller");

		let (g2, controller) = begin_provider_refresh(&mut generations, &mut signals, "p");
		assert_eq!(g2, 2);
		assert!(signals.contains_key("p"));
		assert!(!controller.aborted());

		let g3 = supersede_provider_refresh(&mut generations, &mut signals, "p");
		assert_eq!(g3, 3);
		assert!(controller.aborted(), "supersede aborts the running refresh");
		assert!(!signals.contains_key("p"), "aborted controller is dropped");

		// begin twice: the second aborts the first.
		let (_g4, first) = begin_provider_refresh(&mut generations, &mut signals, "p");
		let (_g5, second) = begin_provider_refresh(&mut generations, &mut signals, "p");
		assert!(first.aborted());
		assert!(!second.aborted());
		assert_eq!(generations.get("p"), Some(&5));
	}

	#[test]
	fn set_delete_clear_supersede_provider_refreshes() {
		let mut models = models_with(HashMap::new());
		models.set_provider(test_provider("p", "P", env_auth("K"), vec![chat_model("p", "m")]));
		let (g, controller) = begin_provider_refresh(
			&mut models.refresh_generations,
			&mut models.refresh_signals,
			"p",
		);
		assert_eq!(g, 2); // set_provider already superseded once
		assert!(models.providers.contains_key("p"));

		models.delete_provider("p");
		assert!(controller.aborted());
		assert!(!models.providers.contains_key("p"));
		assert_eq!(models.get_models(None).len(), 0);

		// clear covers controller-only ids too.
		let (_g, orphan) = begin_provider_refresh(
			&mut models.refresh_generations,
			&mut models.refresh_signals,
			"q",
		);
		models.set_provider(test_provider("r", "R", env_auth("K"), vec![]));
		models.clear_providers();
		assert!(orphan.aborted());
		assert!(models.providers.is_empty());
		assert!(models.refresh_signals.is_empty());
		// deleted providers keep their generation counter (never reused).
		assert_eq!(models.refresh_generations.get("q"), Some(&2));
	}

	// -- publication chain ---------------------------------------------------

	#[test]
	fn publish_checks_generation_and_signal_before_and_after_persist() {
		let mut store = InMemoryModelsStore::new();
		let mut generations = HashMap::new();
		generations.insert("p".to_string(), 7u64);
		let entry = ModelsStoreEntry {
			models: vec![],
			last_modified: Some(5.0),
			checked_at: None,
			etag: None,
		};

		// Fresh signal, current generation → write + update.
		let updated = Arc::new(AtomicBool::new(false));
		let updated2 = updated.clone();
		let accepted = publish_provider_models(
			&mut store,
			&generations,
			"p",
			7,
			None,
			ModelsPublication {
				persist: Persist::Write(entry.clone()),
				update: {
					let flag = updated2.clone();
					Some(Box::new(move || flag.store(true, Ordering::SeqCst)))
				},
			},
		)
		.unwrap();
		assert!(accepted);
		assert!(updated.load(Ordering::SeqCst));
		assert_eq!(store.read("p", None).unwrap().unwrap(), entry);

		// Stale generation → rejected without writing.
		let accepted = publish_provider_models(
			&mut store,
			&generations,
			"p",
			6,
			None,
			ModelsPublication { persist: Persist::Write(entry.clone()), update: None },
		)
		.unwrap();
		assert!(!accepted);
		assert_eq!(store.read("p", None).unwrap().unwrap(), entry);

		// Aborted signal → rejected.
		let signal = AbortSignal::new();
		signal.abort();
		let accepted = publish_provider_models(
			&mut store,
			&generations,
			"p",
			7,
			Some(&signal),
			ModelsPublication { persist: Persist::Unchanged, update: None },
		)
		.unwrap();
		assert!(!accepted);

		// Persist::Delete removes the entry; Unchanged leaves it.
		publish_provider_models(
			&mut store,
			&generations,
			"p",
			7,
			None,
			ModelsPublication { persist: Persist::Delete, update: None },
		)
		.unwrap();
		assert_eq!(store.read("p", None).unwrap(), None);
		publish_provider_models(
			&mut store,
			&generations,
			"p",
			7,
			None,
			ModelsPublication { persist: Persist::Unchanged, update: None },
		)
		.unwrap();
		assert_eq!(store.read("p", None).unwrap(), None);
	}

	#[test]
	fn refresh_runs_both_phases_and_publishes_through_the_chain() {
		let phases: Arc<Mutex<Vec<(bool, bool)>>> = Arc::new(Mutex::new(Vec::new()));
		let updated = Arc::new(AtomicBool::new(false));
		let published = Arc::new(AtomicBool::new(false));
		let refresh_count = Arc::new(AtomicUsize::new(0));

		let mut models = models_with(HashMap::from([("K".to_string(), "env-key".to_string())]));
		{
			let mut credentials = models.credentials.borrow_mut();
			credentials
				.modify(
					"p",
					&mut |_| {
						Ok(Some(stored_credential(
							json!({"type": "api_key", "key": "stored-key"}),
						)))
					},
					None,
				)
				.unwrap();
		}

		let phases2 = phases.clone();
		let updated2 = updated.clone();
		let published2 = published.clone();
		let provider = test_provider("p", "P", env_auth("K"), vec![chat_model("p", "m")])
			.with_refresh_models(Box::new(move |ctx: &mut RefreshModelsContext| {
				phases2
					.lock()
					.unwrap()
					.push((ctx.allow_network, ctx.credential.is_some()));
				if ctx.allow_network {
					let accepted = (ctx.publish)(ModelsPublication {
						persist: Persist::Write(ModelsStoreEntry {
							models: vec![AnyModel::Chat(chat_model("p", "fresh"))],
							last_modified: Some(9.0),
							checked_at: None,
							etag: None,
						}),
						update: {
							let flag = updated2.clone();
							Some(Box::new(move || flag.store(true, Ordering::SeqCst)))
						},
					})?;
					published2.store(accepted, Ordering::SeqCst);
				}
				Ok(())
			}));
		models.set_provider(provider);

		let result = models.refresh(Some(&ModelsRefreshOptions::default()));
		assert!(!result.aborted);
		assert!(result.errors.is_empty(), "{:?}", result.errors);
		// Restore phase (offline) first, then the network phase with the
		// stored credential.
		assert_eq!(*phases.lock().unwrap(), vec![(false, true), (true, true)]);
		assert!(published.load(Ordering::SeqCst));
		assert!(updated.load(Ordering::SeqCst), "publication.update runs after the write");
		let stored = models.models_store.read("p", None).unwrap().unwrap();
		assert_eq!(stored.last_modified, Some(9.0));
		assert_eq!(stored.models.len(), 1);
		assert_eq!(refresh_count.load(Ordering::SeqCst), 0, "api-key auth never refreshes");

		// allowNetwork=false: restore phase only, nothing published.
		let phases3 = phases.clone();
		let provider = Provider::new(
			"p".to_string(),
			"P".to_string(),
			env_auth("K"),
			Box::new(Vec::new),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
		)
		.with_refresh_models(Box::new(move |ctx: &mut RefreshModelsContext| {
			phases3.lock().unwrap().push((ctx.allow_network, ctx.credential.is_some()));
			assert!(ctx.stored.is_some(), "restore phase sees the stored entry");
			Ok(())
		}));
		models.set_provider(provider);
		let result = models.refresh(Some(&ModelsRefreshOptions {
			allow_network: Some(false),
			..ModelsRefreshOptions::default()
		}));
		assert!(!result.aborted);
		assert_eq!(phases.lock().unwrap().last().copied(), Some((false, true)));
	}

	#[test]
	fn refresh_reports_provider_errors_and_honors_selection() {
		let mut models = models_with(HashMap::new());
		let failing = test_provider("bad", "Bad", env_auth("K"), vec![])
			.with_refresh_models(Box::new(|_ctx| Err(JsError::new("catalog down"))));
		let fine = test_provider("good", "Good", env_auth("K"), vec![])
			.with_refresh_models(Box::new(|_ctx| Ok(())));
		models.set_provider(failing);
		models.set_provider(fine);

		let result = models.refresh(None);
		assert!(!result.aborted);
		assert_eq!(result.errors.len(), 1);
		assert_eq!(result.errors.get("bad").unwrap().message, "catalog down");

		// providers selection skips unselected providers.
		let result = models.refresh(Some(&ModelsRefreshOptions {
			providers: Some(vec!["good".to_string()]),
			..ModelsRefreshOptions::default()
		}));
		assert!(result.errors.is_empty());

		// Aborted caller signal → aborted with no errors.
		let signal = AbortSignal::new();
		signal.abort();
		let result = models.refresh(Some(&ModelsRefreshOptions {
			signal: Some(&signal),
			..ModelsRefreshOptions::default()
		}));
		assert!(result.aborted);
		assert!(result.errors.is_empty());
	}

	// -- getAvailable filtering with checkAuth outcomes ----------------------

	#[test]
	fn get_available_filters_by_auth_configuration_and_policy() {
		let mut models = models_with(HashMap::from([("K".to_string(), "env-key".to_string())]));
		// configured via env
		models.set_provider(test_provider("env", "Env", env_auth("K"), vec![
			chat_model("env", "e1"),
			chat_model("env", "e2"),
		]));
		// configured via stored api key + filterModels policy
		{
			let mut credentials = models.credentials.borrow_mut();
			credentials
				.modify(
					"stored",
					&mut |_| {
						Ok(Some(stored_credential(
							json!({"type": "api_key", "key": "sk"}),
						)))
					},
					None,
				)
				.unwrap();
		}
		let filtered = test_provider("stored", "Stored", env_auth("K"), vec![
			chat_model("stored", "s1"),
			chat_model("stored", "hidden"),
		])
		.with_filter_models(Box::new(|models, _credential| {
			models.iter().filter(|m| m.id != "hidden").cloned().collect()
		}));
		models.set_provider(filtered);
		// unconfigured (no env var, no credential)
		models.set_provider(test_provider("none", "None", env_auth("MISSING"), vec![chat_model("none", "n1")]));

		let available = models.get_available(None, None).unwrap();
		let ids: Vec<&str> = available.iter().map(|m| m.id.as_str()).collect();
		assert_eq!(ids, vec!["e1", "e2", "s1"], "unconfigured provider drops out; filter applies");

		let available = models.get_available(Some("none"), None).unwrap();
		assert!(available.is_empty());

		// checkAuth mirrors the same outcomes.
		assert!(models.check_auth("env", None).unwrap().is_some());
		assert!(models.check_auth("none", None).unwrap().is_none());
		assert!(models.check_auth("unknown", None).unwrap().is_none());

		// getAllAvailable: chat filtering via filterModels, non-chat kept.
		let with_all = test_provider("stored", "Stored", env_auth("K"), vec![chat_model("stored", "s1")])
			.with_filter_models(Box::new(|models, _| {
				models.iter().filter(|m| m.id != "hidden").cloned().collect()
			}))
			.with_get_all_models(Box::new(|| {
				vec![
					AnyModel::Chat(chat_model("stored", "s1")),
					AnyModel::Chat(chat_model("stored", "hidden")),
					AnyModel::Image(image_model("stored", "img")),
				]
			}));
		models.set_provider(with_all);
		let all = models.get_all_available(Some("stored"), None).unwrap();
		let ids: Vec<&str> = all.iter().map(|m| m.id()).collect();
		assert_eq!(ids, vec!["s1", "img"], "filtered chat drops, image kept");
		let of_type = models
			.get_available_of_type(&ModelType::Image, Some("stored"), None)
			.unwrap();
		assert_eq!(of_type.len(), 1);
	}

	// -- getAuth precedence ---------------------------------------------------

	#[test]
	fn get_auth_precedence_override_stored_oauth_stored_key_ambient() {
		let mut models = models_with(HashMap::from([("K".to_string(), "ambient-key".to_string())]));

		// Stored api key beats ambient env.
		models.set_provider(test_provider("key", "Key", env_auth("K"), vec![]));
		{
			let mut credentials = models.credentials.borrow_mut();
			credentials
				.modify(
					"key",
					&mut |_| {
						Ok(Some(stored_credential(
							json!({"type": "api_key", "key": "stored-key"}),
						)))
					},
					None,
				)
				.unwrap();
		}
		let result = models.get_auth_for_provider("key", None).unwrap().unwrap();
		assert_eq!(result.auth.api_key.as_deref(), Some("stored-key"));

		// Explicit override beats the stored key.
		let overrides = AuthResolutionOverrides {
			api_key: Some("explicit".to_string()),
			env: None,
			min_oauth_validity_ms: None,
			signal: None,
		};
		let result = models
			.get_auth_for_provider("key", Some(&overrides))
			.unwrap()
			.unwrap();
		assert_eq!(result.auth.api_key.as_deref(), Some("explicit"));

		// Ambient env when nothing is stored.
		models.set_provider(test_provider("amb", "Amb", env_auth("K"), vec![]));
		let result = models.get_auth_for_provider("amb", None).unwrap().unwrap();
		assert_eq!(result.auth.api_key.as_deref(), Some("ambient-key"));

		// Stored valid OAuth resolves through to_auth with source "OAuth".
		let refresh_count = Arc::new(AtomicUsize::new(0));
		let oauth_auth = ProviderAuth {
			api_key: None,
			oauth: Some(Arc::new(TestOAuthAuth { refresh_count: refresh_count.clone(), supports_login: false })),
		};
		models.set_provider(test_provider("oa", "OA", oauth_auth, vec![]));
		{
			let mut credentials = models.credentials.borrow_mut();
			credentials
				.modify(
					"oa",
					&mut |_| {
						Ok(Some(Credential::OAuth(oauth("r1", "access-1", 2_000_000.0))))
					},
					None,
				)
				.unwrap();
		}
		let result = models.get_auth_for_provider("oa", None).unwrap().unwrap();
		assert_eq!(result.source.as_deref(), Some("OAuth"));
		assert_eq!(result.auth.api_key.as_deref(), Some("access-1"));
		assert_eq!(refresh_count.load(Ordering::SeqCst), 0, "valid token does not refresh");

		// Expired stored OAuth refreshes under the store lock and persists.
		{
			let mut credentials = models.credentials.borrow_mut();
			credentials
				.modify(
					"oa",
					&mut |_| Ok(Some(Credential::OAuth(oauth("r1", "access-1", 500_000.0)))),
					None,
				)
				.unwrap();
		}
		let result = models.get_auth_for_provider("oa", None).unwrap().unwrap();
		assert_eq!(result.auth.api_key.as_deref(), Some("access-1+"));
		assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
		let stored = models.credentials.borrow().read("oa", None).unwrap().unwrap();
		match stored {
			Credential::OAuth(stored) => assert_eq!(stored.access, "access-1+"),
			other => panic!("expected oauth, got {other:?}"),
		}

		// Unknown provider → None.
		assert!(models.get_auth_for_provider("nope", None).unwrap().is_none());
	}

	#[test]
	fn get_auth_for_model_merges_model_headers() {
		let mut models = models_with(HashMap::from([("K".to_string(), "ambient-key".to_string())]));
		models.set_provider(test_provider("p", "P", env_auth("K"), vec![]));
		let mut model = chat_model("p", "m");
		model.set(
			"headers",
			json!({"X-Static": "model", "Authorization": "model-auth"}),
		)
		.unwrap();
		let result = models
			.get_auth_for_model(&AnyModel::Chat(model), None)
			.unwrap()
			.unwrap();
		let headers = result.auth.headers.unwrap();
		assert_eq!(headers.get("Authorization"), Some(&Some("model-auth".to_string())));
		assert_eq!(headers.get("X-Static"), Some(&Some("model".to_string())));
	}

	// -- best-effort reads ----------------------------------------------------

	#[test]
	fn model_reads_are_best_effort_for_unknown_providers() {
		let mut models = models_with(HashMap::new());
		models.set_provider(test_provider("p", "P", env_auth("K"), vec![chat_model("p", "m1")]));
		models.set_provider(
			test_provider("q", "Q", env_auth("K"), vec![chat_model("q", "m2")])
				.with_get_all_models(Box::new(|| {
					vec![AnyModel::Chat(chat_model("q", "m2")), AnyModel::Image(image_model("q", "i"))]
				})),
		);

		assert!(models.get_models(Some("missing")).is_empty());
		assert!(models.get_all_models(Some("missing")).is_empty());
		assert!(models.get_model("missing", "m1").is_none());
		assert!(models.get_model("p", "nope").is_none());

		let chat_models = models.get_models(None);
		let ids: Vec<&str> = chat_models.iter().map(|m| m.id.as_str()).collect();
		assert_eq!(ids, vec!["m1", "m2"]);

		let all = models.get_all_models(None);
		assert_eq!(all.len(), 3, "provider q contributes chat + image");

		let chats = models.get_models_of_type(&ModelType::Chat, None);
		assert_eq!(chats.len(), 2);
		let images = models.get_models_of_type(&ModelType::Image, None);
		assert_eq!(images.len(), 1);
		assert!(models.get_model_of_type(&ModelType::Image, "q", "i").is_some());
		assert!(models.get_model_of_type(&ModelType::Chat, "q", "missing").is_none());
	}

	// -- login / logout -------------------------------------------------------

	struct NoInteraction;

	impl crate::pi_ai::models::AuthInteraction for NoInteraction {
		fn prompt(&mut self, _prompt: &Value) -> Result<String, JsError> {
			Ok(String::new())
		}

		fn notify(&mut self, _event: &Value) {}
	}

	#[test]
	fn login_error_messages_and_persistence() {
		let mut models = models_with(HashMap::new());

		// Unknown provider.
		let err = models
			.login("missing", AuthType::OAuth, &mut NoInteraction, None)
			.unwrap_err();
		assert_eq!(err.message, "Unknown provider: missing");

		// OAuth requested but the provider has no oauth method at all.
		models.set_provider(test_provider("p", "P", env_auth("K"), vec![]));
		let err = models
			.login("p", AuthType::OAuth, &mut NoInteraction, None)
			.unwrap_err();
		assert_eq!(err.message, "P does not support oauth login");

		// OAuth method present but without login support.
		let refresh_count = Arc::new(AtomicUsize::new(0));
		let oauth_auth = ProviderAuth {
			api_key: Some(Arc::new(EnvKeyAuth { env_name: "K" })),
			oauth: Some(Arc::new(TestOAuthAuth { refresh_count, supports_login: false })),
		};
		models.set_provider(test_provider("p", "P", oauth_auth, vec![]));
		let err = models
			.login("p", AuthType::OAuth, &mut NoInteraction, None)
			.unwrap_err();
		assert_eq!(err.message, "P does not support oauth login");

		// api_key login on a method without login support.
		let err = models
			.login("p", AuthType::ApiKey, &mut NoInteraction, None)
			.unwrap_err();
		assert_eq!(err.message, "P does not support api_key login");

		// Supported login persists the returned credential.
		let oauth_auth = ProviderAuth {
			api_key: None,
			oauth: Some(Arc::new(TestOAuthAuth {
				refresh_count: Arc::new(AtomicUsize::new(0)),
				supports_login: true,
			})),
		};
		models.set_provider(test_provider("p", "P", oauth_auth, vec![]));
		let credential = models
			.login("p", AuthType::OAuth, &mut NoInteraction, None)
			.unwrap();
		match &credential {
			Credential::OAuth(c) => assert_eq!(c.access, "login-access"),
			other => panic!("expected oauth, got {other:?}"),
		}
		let stored = models.credentials.borrow().read("p", None).unwrap().unwrap();
		assert_eq!(stored, credential);
	}

	#[test]
	fn logout_removes_and_wraps_store_errors() {
		let mut models = models_with(HashMap::new());
		models.set_provider(test_provider("p", "P", env_auth("K"), vec![]));
		{
			let mut credentials = models.credentials.borrow_mut();
			credentials
				.modify(
					"p",
					&mut |_| {
						Ok(Some(stored_credential(
							json!({"type": "api_key", "key": "k"}),
						)))
					},
					None,
				)
				.unwrap();
		}
		models.logout("p", None).unwrap();
		assert_eq!(models.credentials.borrow().read("p", None).unwrap(), None);

		// Failing store → ModelsError("auth") wrap with the cause folded in.
		let mut failing = ModelsImpl::new(CreateModelsOptions {
			credentials: Some(Box::new(FailingCredentialStore {
				inner: InMemoryCredentialStore::new(),
			})),
			models_store: None,
			auth_context: None,
		});
		failing.set_provider(test_provider("p", "P", env_auth("K"), vec![]));
		let err = failing.logout("p", None).unwrap_err();
		assert_eq!(err.message, "Credential store delete failed for p: disk on fire");
	}

	// -- request paths --------------------------------------------------------

	#[test]
	fn stream_applies_auth_and_builds_request_options() {
		let mut models = models_with(HashMap::from([("K".to_string(), "ambient-key".to_string())]));
		let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
		let seen2 = seen.clone();
		let provider = Provider::new(
			"p".to_string(),
			"P".to_string(),
			env_auth("K"),
			Box::new(Vec::new),
			Box::new(move |model, _context, options| {
				seen2.lock().unwrap().push(json!({
					"baseUrl": model.base_url,
					"apiKey": options.and_then(|o| o.get("apiKey").cloned()),
					"headers": options.and_then(|o| o.get("headers").cloned()),
				}));
				AssistantMessageEventStream(json!({"marker": true}))
			}),
			Box::new(|_, _, _| AssistantMessageEventStream::default()),
		)
		.with_base_url(Some("https://base".to_string()));
		models.set_provider(provider);
		let model = chat_model("p", "m");

		// Unconfigured provider → setup-error stream, complete propagates.
		let mut unconfigured = models_with(HashMap::new());
		unconfigured.set_provider(test_provider("p", "P", env_auth("MISSING"), vec![chat_model("p", "m")]));
		let stream = unconfigured.stream(&model, &TranscriptContext::default(), None);
		assert!(is_error_stream(&stream));
		let err = unconfigured.complete(&model, &TranscriptContext::default(), None).unwrap_err();
		assert_eq!(err.message, "Provider is not configured: p");

		// Unknown provider → "Unknown provider" setup error.
		let err = models
			.complete(&chat_model("nope", "m"), &TranscriptContext::default(), None)
			.unwrap_err();
		assert_eq!(err.message, "Unknown provider: nope");

		// Configured: request options carry apiKey (position of an existing
		// key preserved, absent keys appended) and the terminal result.
		let options = json!({"apiKey": null, "systemPrompt": "x", "headers": {"X-Opt": "1"}});
		let stream = models.stream(&model, &TranscriptContext::default(), Some(&options));
		assert!(!is_error_stream(&stream));
		let message = models
			.complete(&model, &TranscriptContext::default(), Some(&options))
			.unwrap();
		assert_eq!(message.0, json!({"marker": true}));
		let seen = seen.lock().unwrap();
		assert_eq!(seen.len(), 2);
		assert_eq!(seen[0], json!({
			"baseUrl": "https://x",
			"apiKey": json!("ambient-key"),
			"headers": json!({"X-Opt": "1"}),
		}));
		// baseUrl override rides the auth result when one is present; this
		// env-key auth sets none, so the model's baseUrl is kept.

		// Deferred without support → setup error with the exact message.
		let err = models
			.fetch_deferred(&model, &DeferredHandle::default(), None)
			.unwrap_err();
		assert_eq!(err.message, "Provider p does not support deferred responses");
		let err = models
			.cancel_deferred(&model, &DeferredHandle::default(), None)
			.unwrap_err();
		assert_eq!(err.message, "Provider p does not support deferred responses");
	}

	#[test]
	fn generate_images_and_classify_never_reject() {
		let mut models = models_with(HashMap::from([("K".to_string(), "ambient-key".to_string())]));
		models.set_provider(test_provider("p", "P", env_auth("K"), vec![]));
		let image = image_model("p", "i");
		let classifier: ClassifierModel = serde_json::from_value(json!({
			"id": "c", "name": "c", "api": "a", "provider": "p", "type": "classifier",
			"baseUrl": "", "input": ["text"],
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"contextWindow": 100
		}))
		.unwrap();

		// Provider without image generation → error result value.
		let images = models.generate_images(&image, &ImagesContext::default(), None);
		assert_eq!(images.0["stopReason"], json!("error"));
		assert_eq!(
			images.0["errorMessage"],
			json!("Provider p does not support image generation")
		);
		assert_eq!(images.0["model"], json!("i"));

		let result = models.classify(&classifier, &ClassifierContext::default(), None);
		assert_eq!(result.0["stopReason"], json!("error"));
		assert_eq!(
			result.0["errorMessage"],
			json!("Provider p does not support classification")
		);

		// Unconfigured auth → error result naming the provider (the
		// capability check runs first, like the TS).
		let mut unconfigured = models_with(HashMap::new());
		let with_images = test_provider("p", "P", env_auth("MISSING"), vec![])
			.with_generate_images(Box::new(|_model, _context, _options| {
				AssistantImages(json!({"output": []}))
			}));
		unconfigured.set_provider(with_images);
		let images = unconfigured.generate_images(&image, &ImagesContext::default(), None);
		assert_eq!(images.0["stopReason"], json!("error"));
		assert_eq!(images.0["errorMessage"], json!("Provider is not configured: p"));
	}

	// -- mergeHeaders ---------------------------------------------------------

	#[test]
	fn merge_headers_case_insensitive_later_wins() {
		let base: ProviderHeaders = [("A".to_string(), Some("1".to_string())), ("b".to_string(), Some("2".to_string()))]
			.into_iter()
			.collect();
		let override_headers: ProviderHeaders =
			[("a".to_string(), Some("3".to_string())), ("C".to_string(), None)]
				.into_iter()
				.collect();
		let merged = merge_headers(Some(&base), Some(&override_headers)).unwrap();
		let entries: Vec<(&String, &Option<String>)> = merged.iter().collect();
		assert_eq!(
			entries,
			vec![
				(&"b".to_string(), &Some("2".to_string())),
				(&"a".to_string(), &Some("3".to_string())),
				(&"C".to_string(), &None),
			],
			"case-insensitive delete-then-insert, survivors keep order, new keys append"
		);

		assert_eq!(merge_headers(None, None), None);
		let empty = ProviderHeaders::new();
		assert!(merge_headers(Some(&empty), None).is_some(), "an empty map is a value");
		// Later override entries win among themselves.
		let double: ProviderHeaders = [("X".to_string(), Some("first".to_string())), ("x".to_string(), Some("second".to_string()))]
			.into_iter()
			.collect();
		let merged = merge_headers(None, Some(&double)).unwrap();
		assert_eq!(merged.get("x"), Some(&Some("second".to_string())));
		assert_eq!(merged.len(), 1);
	}
}
