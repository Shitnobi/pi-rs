//! Credential surface — port of `pi-ai` `src/auth/types.ts` +
//! `src/auth/credential-store.ts`.
//!
//! The TS `CredentialStore` is Promise-based with per-provider promise-chain
//! serialization; per the G8 async-runtime decision this layer is fully
//! synchronous, so the trait methods are sync and the serialization a
//! single-threaded caller gets for free. Observable semantics preserved:
//! `read` → `None` for missing entries, `list` exposes metadata only,
//! `modify` is the only write path and resolves with the post-write
//! credential, aborts reject with `JsError::abort()`.
//!
//! Login/OAuth *flows* (prompt interaction, device-code polling, token
//! exchange) are deferred to the async phase; the data shapes and the
//! check/resolve seams they need are here.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::error::JsError;
use crate::pi_ai::model::{ProviderEnv, ProviderHeaders};

// ============================================================================
// ModelAuth
// ============================================================================

js_object! {
	/// `ModelAuth` — request auth for a single model request.
	pub struct ModelAuth {
		api_key opt "apiKey" : Option<String>,
		headers opt "headers" : Option<ProviderHeaders>,
		base_url opt "baseUrl" : Option<String>,
	}
}

// ============================================================================
// Credentials
// ============================================================================

js_object! {
	/// `ApiKeyCredential` — stored api-key credential. `env` holds
	/// provider-scoped environment/config values (Cloudflare account ids …).
	pub struct ApiKeyCredential {
		key opt "key" : Option<String>,
		env opt "env" : Option<ProviderEnv>,
	}
}

js_object! {
	/// `OAuthCredentials` — OAuth token data. Open shape: unknown fields
	/// round-trip through `extra` (TS `[key: string]: unknown`).
	pub struct OAuthCredentials {
		refresh req "refresh" : String,
		access req "access" : String,
		expires req "expires" : crate::pi_ai::model::JsNumber,
	}
}

js_object! {
	/// `OAuthCredential` — stored canonical OAuth credential
	/// (`OAuthCredentials` + `type: "oauth"`).
	pub struct OAuthCredential {
		refresh req "refresh" : String,
		access req "access" : String,
		expires req "expires" : crate::pi_ai::model::JsNumber,
	}
}

/// One type-tagged credential per provider — the shape of today's auth.json.
#[derive(Debug, Clone, PartialEq)]
pub enum Credential {
	ApiKey(ApiKeyCredential),
	OAuth(OAuthCredential),
}

impl Credential {
	/// The `type` discriminant (`"api_key"` / `"oauth"`).
	pub fn credential_type(&self) -> &'static str {
		match self {
			Credential::ApiKey(_) => "api_key",
			Credential::OAuth(_) => "oauth",
		}
	}

	/// Ordered JSON object form: `type` first, then the credential fields
	/// (matches how auth.json entries are constructed in TS).
	pub fn to_value(&self) -> Value {
		let map = match self {
			Credential::ApiKey(c) => c.ordered_map(),
			Credential::OAuth(c) => c.ordered_map(),
		};
		// `type` is the first key in TS literals ({ type: "api_key", key, env }).
		let mut out = Map::new();
		out.insert("type".to_string(), Value::String(self.credential_type().to_string()));
		for (k, v) in map {
			if k != "type" {
				out.insert(k, v);
			}
		}
		Value::Object(out)
	}

	/// Parse from the auth.json entry form (requires the `type` tag).
	pub fn from_map(map: Map<String, Value>) -> Result<Credential, String> {
		match map.get("type").and_then(Value::as_str) {
			Some("api_key") => ApiKeyCredential::from_map(map).map(Credential::ApiKey),
			Some("oauth") => OAuthCredential::from_map(map).map(Credential::OAuth),
			other => Err(format!("unknown credential type: {other:?}")),
		}
	}
}

/// Non-secret credential metadata for account/status enumeration.
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialInfo {
	pub provider_id: String,
	pub credential_type: &'static str,
}

// ============================================================================
// Store
// ============================================================================

/// Optional cancellation for auth/credential operations. Sync port of
/// `AuthOperationOptions`; the flag is checked at the entry points where TS
/// calls `signal.throwIfAborted()`.
pub type AbortFlag = pi_tools::shell::AbortSignal;

/// `AuthOperationOptions` — `signal` only.
#[derive(Default, Clone)]
pub struct AuthOperationOptions<'a> {
	pub signal: Option<&'a AbortFlag>,
}

pub(crate) fn check_aborted(options: Option<&AuthOperationOptions>) -> Result<(), JsError> {
	if let Some(opts) = options {
		if let Some(signal) = opts.signal {
			if signal.aborted() {
				return Err(JsError::abort());
			}
		}
	}
	Ok(())
}

/// App-owned credential storage, keyed by provider id, one credential per
/// provider (TS `CredentialStore`, sync port).
///
/// Error semantics: `read` returns `None` for missing entries. Methods fail
/// only on storage errors; `Models` wraps those in `ModelsError` with code
/// `"auth"`.
pub trait CredentialStore {
	/// Read the stored credential, possibly expired.
	fn read(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError>;

	/// List stored credential metadata without resolving or exposing secrets.
	fn list(&self, options: Option<&AuthOperationOptions>) -> Result<Vec<CredentialInfo>, JsError>;

	/// The only write path: serialized read-modify-write. `fn` sees the
	/// current credential; return `Some(new)` to write, `None` to leave the
	/// entry unchanged. Resolves with the post-write credential.
	#[allow(clippy::type_complexity)] // faithful to the TS callback shape
	fn modify(
		&mut self,
		provider_id: &str,
		f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError>;

	/// Remove a credential (logout).
	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<(), JsError>;
}

/// Default in-memory credential store (TS `InMemoryCredentialStore`).
/// Single-threaded, so the TS per-provider promise-chain serialization is
/// trivially satisfied; aborts are checked where TS throws.
#[derive(Default)]
pub struct InMemoryCredentialStore {
	credentials: BTreeMap<String, Credential>,
}

impl InMemoryCredentialStore {
	pub fn new() -> InMemoryCredentialStore {
		InMemoryCredentialStore { credentials: BTreeMap::new() }
	}
}

impl CredentialStore for InMemoryCredentialStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError> {
		check_aborted(options)?;
		Ok(self.credentials.get(provider_id).cloned())
	}

	fn list(&self, options: Option<&AuthOperationOptions>) -> Result<Vec<CredentialInfo>, JsError> {
		check_aborted(options)?;
		Ok(self
			.credentials
			.iter()
			.map(|(provider_id, credential)| CredentialInfo {
				provider_id: provider_id.clone(),
				credential_type: credential.credential_type(),
			})
			.collect())
	}

	#[allow(clippy::type_complexity)] // faithful to the TS callback shape
	fn modify(
		&mut self,
		provider_id: &str,
		f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError> {
		// TS checks the signal again after `fn` runs, before committing.
		check_aborted(options)?;
		let current = self.credentials.get(provider_id);
		let next = f(current)?;
		check_aborted(options)?;
		let result = match next {
			Some(next) => {
				self.credentials.insert(provider_id.to_string(), next);
				self.credentials.get(provider_id).cloned()
			}
			None => current.cloned(),
		};
		Ok(result)
	}

	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<(), JsError> {
		check_aborted(options)?;
		self.credentials.remove(provider_id);
		Ok(())
	}
}

// ============================================================================
// Resolution types
// ============================================================================

/// Environment access for auth resolution. Injectable for tests.
/// Sync port of TS `AuthContext` (`env`/`fileExists`).
pub trait AuthContext {
	/// `env(name)` — empty string is distinct from missing in TS
	/// (`string | undefined`), so `Option<String>` here; callers apply the
	/// JS `||`-fallback semantics themselves where the TS does.
	fn env(&self, name: &str) -> Option<String>;
	/// Check whether a file exists. Supports a leading `~`.
	fn file_exists(&self, path: &str) -> bool;
}

/// Result of resolving auth for a model (TS `AuthResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct AuthResult {
	pub auth: ModelAuth,
	/// Provider-scoped environment/config values resolved from credentials
	/// and ambient context.
	pub env: Option<ProviderEnv>,
	/// Human-readable label for status UI: "ANTHROPIC_API_KEY", "OAuth", …
	pub source: Option<String>,
}

/// TS `AuthCheck`.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthCheck {
	pub source: Option<String>,
	pub auth_type: AuthType,
}

/// TS `AuthType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
	ApiKey,
	OAuth,
}

impl AuthType {
	pub fn as_str(&self) -> &'static str {
		match self {
			AuthType::ApiKey => "api_key",
			AuthType::OAuth => "oauth",
		}
	}
}

// ============================================================================
// Provider auth seams (data + check/resolve; login flows deferred)
// ============================================================================

/// TS `ApiKeyAuth`, sync port. `login` is deferred to the async phase.
pub trait ApiKeyAuth {
	/// Display name, e.g. "Anthropic API key".
	fn name(&self) -> &str;
	/// Optional side-effect-free availability check.
	fn check(
		&self,
		ctx: &dyn AuthContext,
		credential: Option<&ApiKeyCredential>,
		signal: Option<&AbortFlag>,
	) -> Result<Option<AuthCheck>, JsError>;
	/// Resolve auth from the stored credential and/or ambient sources.
	/// `None` = not configured.
	fn resolve(
		&self,
		ctx: &dyn AuthContext,
		credential: Option<&ApiKeyCredential>,
		signal: Option<&AbortFlag>,
	) -> Result<Option<AuthResult>, JsError>;
}

/// TS `OAuthAuth`, sync port. `login`/`refresh` are deferred (network flows);
/// `to_auth` covers the request-auth derivation parity path.
pub trait OAuthAuth {
	/// Display name, e.g. "Anthropic (Claude Pro/Max)".
	fn name(&self) -> &str;
	/// Whether access is backed by a provider subscription.
	fn is_subscription(&self) -> bool;
	/// Selector label for the OAuth login option.
	fn login_label(&self) -> Option<&str>;
	/// Side-effect-free derivation of request auth from a valid credential.
	fn to_auth(&self, credential: &OAuthCredential) -> Result<ModelAuth, JsError>;
}

/// TS `ProviderAuth` — at least one of `api_key`/`oauth` must be present.
#[derive(Clone)]
pub struct ProviderAuth {
	pub api_key: Option<std::sync::Arc<dyn ApiKeyAuth + Send + Sync>>,
	pub oauth: Option<std::sync::Arc<dyn OAuthAuth + Send + Sync>>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn cred_api_key(json: Value) -> Credential {
		Credential::from_map(json.as_object().unwrap().clone()).unwrap()
	}

	#[test]
	fn credential_round_trip_preserves_order_and_type() {
		let v = json!({"type": "api_key", "key": "sk-1", "env": {"A": "1"}});
		let c = cred_api_key(v.clone());
		assert_eq!(c.to_value(), v);
		let oauth = json!({"type": "oauth", "refresh": "r", "access": "a", "expires": 123.5, "custom": "x"});
		let c = Credential::from_map(oauth.as_object().unwrap().clone()).unwrap();
		let out = c.to_value();
		// type first, then fields, extras last, order preserved
		assert_eq!(out["type"], "oauth");
		assert_eq!(out["custom"], "x");
		assert_eq!(out["expires"], 123.5);
		let keys: Vec<&str> = out.as_object().unwrap().keys().map(|s| s.as_str()).collect();
		assert_eq!(keys, vec!["type", "refresh", "access", "expires", "custom"]);
	}

	#[test]
	fn unknown_credential_type_errors() {
		let err = Credential::from_map(json!({"type": "nope"}).as_object().unwrap().clone());
		assert!(err.is_err());
	}

	#[test]
	fn in_memory_store_semantics() {
		let mut store = InMemoryCredentialStore::new();
		assert_eq!(store.read("p", None).unwrap(), None);
		// modify with None leaves entry unchanged (still missing)
		let r = store
			.modify("p", &mut |_| Ok(None), None)
			.unwrap();
		assert_eq!(r, None);
		// write via modify
		let cred = cred_api_key(json!({"type": "api_key", "key": "k"}));
		let r = store
			.modify("p", &mut |_| Ok(Some(cred.clone())), None)
			.unwrap();
		assert_eq!(r, Some(cred.clone()));
		assert_eq!(store.read("p", None).unwrap(), Some(cred.clone()));
		// list exposes metadata only
		let list = store.list(None).unwrap();
		assert_eq!(list.len(), 1);
		assert_eq!(list[0].provider_id, "p");
		assert_eq!(list[0].credential_type, "api_key");
		// fn sees current
		let seen = store
			.modify("p", &mut |cur| {
				assert_eq!(cur, Some(&cred));
				Ok(None)
			}, None)
			.unwrap();
		assert_eq!(seen, Some(cred.clone()));
		// fn error propagates
		let err = store.modify("p", &mut |_| Err(JsError::new("boom")), None);
		assert_eq!(err.unwrap_err().message, "boom");
		// delete
		store.delete("p", None).unwrap();
		assert_eq!(store.read("p", None).unwrap(), None);
	}

	#[test]
	fn aborted_operations_reject() {
		use pi_tools::shell::AbortSignal;
		let signal = AbortSignal::new();
		signal.abort();
		let opts = AuthOperationOptions { signal: Some(&signal) };
		let mut store = InMemoryCredentialStore::new();
		let err = store.read("p", Some(&opts)).unwrap_err();
		assert_eq!(err.message, "This operation was aborted");
		let err = store.modify("p", &mut |_| Ok(None), Some(&opts)).unwrap_err();
		assert_eq!(err.message, "This operation was aborted");
	}

	#[test]
	fn model_auth_round_trip() {
		let v = json!({"apiKey": "k", "baseUrl": "https://x"});
		let m: ModelAuth = serde_json::from_value(v.clone()).unwrap();
		assert_eq!(m.to_value(), v);
		let empty = json!({});
		let m: ModelAuth = serde_json::from_value(empty.clone()).unwrap();
		assert_eq!(m.to_value(), empty);
	}
}
