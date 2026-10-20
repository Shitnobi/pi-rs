//! Auth storage — port of `coding-agent` `src/core/auth-storage.ts`.
//!
//! Foundation slice: the `AuthStorageBackend` trait (`with_lock`
//! read-modify-write over a string-valued file) and
//! `FileAuthStorageBackend`. Auth/composer wave: `InMemoryAuthStorageBackend`,
//! `ReadOnlyAuthStorage` (strict validation, exact error strings),
//! `AuthStorage` (state machine with revision caching and credential
//! resolution) and `readStoredCredential`.
//!
//! G8 (sync) notes:
//! - The TS `proper-lockfile` cross-process locking is replaced by a
//!   process-global mutex (documented divergence, single process per user
//!   directory; see docs/plans/impl-notes-providers-foundation.md).
//! - The TS async machinery (`withLockAsync`, the `AuthFileReload`
//!   single-flight reload with `AbortController` + reader counting, the
//!   `InMemoryAuthStorageBackend.asyncChain` promise serialization) has no
//!   observable sync behavior and collapses: `withLockAsync` calls become
//!   `with_lock` calls, the single-flight reload becomes a direct
//!   synchronous reload (equivalent single-threaded), and the in-memory
//!   async chain only ever serialized concurrent async operations.
//! - `AuthStorage.read`/`ReadOnlyAuthStorage.read` resolve `api_key`
//!   credentials via `resolveConfigValue`. The TS reads
//!   `env?.[name] || process.env[name] || undefined`; here env is injected
//!   (the `ambient_env` field plays `process.env`) and command execution is
//!   behind the [`CommandRunner`] trait via an owned
//!   [`ConfigValueResolver`] (one per storage instance ≈ one per parity
//!   case, matching the accepted per-case cache divergence of the
//!   config-value tool).
//!
//! Documented divergences (TS behavior in parentheses):
//! - `FileAuthStorageBackend` creates the file lazily (TS pre-creates
//!   `"{}"`, mode 0o600; Unix modes not applied).
//! - Non-string `key` values in `api_key` credentials pass through
//!   unchanged (TS throws `config.startsWith is not a function`); the
//!   ReadOnly validation layer makes this unreachable for validated data.
//! - The `CredentialStore` impl converts raw entries via
//!   `Credential::from_map`; entries that do not parse surface as `None`
//!   (TS returns the raw object unchecked). The parity surface is the
//!   raw-`Value` `*_raw` methods, which are exact.
//! - Trait-level `list` maps a missing/unknown `type` to `"api_key"`
//!   (TS surfaces `undefined`); `list_raw` is exact.
//! - `delete` on an array root replaces the element with `null`
//!   (JSON.stringify-identical to a JS hole; `Object.entries` on a holed
//!   array would skip the index).

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde_json::{Map, Value};

use crate::config_value::{is_command_config_value, CommandRunner, ConfigValueResolver, Env};
use crate::error::JsError;
use crate::models_store::RevisionFn;
use crate::pi_ai::credential::{
	check_aborted, AuthOperationOptions, Credential, CredentialInfo, CredentialStore,
};
use crate::pi_ai::js::{js_json_parse, js_stringify_pretty};
use pi_core::text::strip_bom;
use pi_tools::node_fs::node_error_message;

/// The result of a `with_lock` closure: a value to return plus an optional
/// next content string to write back (TS `LockResult<T>`).
pub struct LockResult<T> {
	pub result: T,
	/// `Some(content)` replaces the file content; `None` leaves it alone.
	pub next: Option<String>,
}

/// TS `AuthStorageBackend` — a read-modify-write lock over a string-valued
/// file (G8: sync). The closure receives the current content (`None` when
/// the file does not exist) and returns the result plus optional next
/// content, written only when the closure succeeds.
pub trait AuthStorageBackend {
	fn with_lock<T>(
		&self,
		f: &mut dyn FnMut(Option<&str>) -> Result<LockResult<T>, JsError>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<T, JsError>;
}

/// Process-global serialization for file backends (the sync stand-in for
/// the TS `proper-lockfile` acquisition; see module docs).
fn global_file_lock() -> MutexGuard<'static, ()> {
	static LOCK: Mutex<()> = Mutex::new(());
	LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// TS `FileAuthStorageBackend` — JSON file storage with read-modify-write
/// locking. The TS backend creates the parent directory (mode 0o700) and
/// the file (mode 0o600) at construction; the Rust port creates the parent
/// directory and file lazily on first write (Unix file modes are not
/// applied — documented divergence, single-user hermit environment).
pub struct FileAuthStorageBackend {
	path: String,
}

impl FileAuthStorageBackend {
	pub fn new(path: impl Into<String>) -> FileAuthStorageBackend {
		FileAuthStorageBackend { path: path.into() }
	}

	pub fn path(&self) -> &str {
		&self.path
	}
}

impl AuthStorageBackend for FileAuthStorageBackend {
	fn with_lock<T>(
		&self,
		f: &mut dyn FnMut(Option<&str>) -> Result<LockResult<T>, JsError>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<T, JsError> {
		check_aborted(options)?;
		let _guard = global_file_lock();
		let content = match std::fs::read_to_string(&self.path) {
			Ok(content) => Some(content),
			// ENOENT → no content (the TS backend pre-creates "{}"; the
			// parse layer treats missing like empty either way).
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
			Err(e) => {
				return Err(JsError::new(node_error_message("read", &self.path, &e)));
			}
		};
		check_aborted(options)?;
		let outcome = f(content.as_deref())?;
		if let Some(next) = &outcome.next {
			if let Some(parent) = std::path::Path::new(&self.path).parent() {
				if !parent.as_os_str().is_empty() {
					if let Err(e) = std::fs::create_dir_all(parent) {
						return Err(JsError::new(node_error_message("mkdir", &self.path, &e)));
					}
				}
			}
			if let Err(e) = std::fs::write(&self.path, next) {
				return Err(JsError::new(node_error_message("write", &self.path, &e)));
			}
		}
		Ok(outcome.result)
	}
}

// ============================================================================
// JS value semantics helpers (parsed auth.json data is UNCHECKED in TS)
// ============================================================================

/// JS `data[key]` on a parsed-JSON value: objects do a key lookup, arrays
/// index by canonical numeric string, anything else is `undefined`.
fn js_get<'a>(data: &'a Value, key: &str) -> Option<&'a Value> {
	match data {
		Value::Object(map) => map.get(key),
		Value::Array(items) => js_array_index(key).and_then(|i| items.get(i)),
		_ => None,
	}
}

/// Whether `key` is a canonical array index string ("0", "1", …).
fn js_array_index(key: &str) -> Option<usize> {
	if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
		return None;
	}
	if key.len() > 1 && key.starts_with('0') {
		return None;
	}
	key.parse().ok()
}

/// JS `{...value}` spread into a record: objects clone, arrays spread to
/// index keys, primitives spread to nothing.
fn spread_to_record(value: &Value) -> Map<String, Value> {
	let mut out = Map::new();
	match value {
		Value::Object(map) => out.extend(map.iter().map(|(k, v)| (k.clone(), v.clone()))),
		Value::Array(items) => {
			for (i, item) in items.iter().enumerate() {
				out.insert(i.to_string(), item.clone());
			}
		}
		_ => {}
	}
	out
}

/// JS `Object.entries(value)`: objects yield key/value pairs in order,
/// arrays yield index/value pairs, primitives yield nothing.
fn object_entries(value: &Value) -> Vec<(String, Value)> {
	match value {
		Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
		Value::Array(items) => {
			items.iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect()
		}
		_ => Vec::new(),
	}
}

/// JS truthiness for JSON values (`null`, `false`, `0`, `""` are falsy).
fn js_truthy(value: &Value) -> bool {
	match value {
		Value::Null => false,
		Value::Bool(b) => *b,
		Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
		Value::String(s) => !s.is_empty(),
		_ => true,
	}
}

/// TS `parseStorageData`: empty/absent content → `{}`; otherwise an
/// UNCHECKED `JSON.parse` (any JSON value is accepted — the TS casts to
/// `AuthStorageData` without validation).
fn parse_storage_data(content: Option<&str>) -> Result<Value, JsError> {
	let Some(content) = content else { return Ok(Value::Object(Map::new())) };
	if content.is_empty() {
		return Ok(Value::Object(Map::new()));
	}
	js_json_parse(strip_bom(content)).map_err(JsError::new)
}

/// The merged resolution env for `resolveConfigValue(key, credential.env)`:
/// the TS looks up `env?.[name] || process.env[name] || undefined` — a
/// truthy-only chain — so the ambient env is seeded first and non-empty
/// credential-scoped entries override it (an empty credential entry falls
/// through to the ambient value, exactly like the JS `||` chain).
fn resolution_env(credential: &Value, ambient: &Env) -> Env {
	let mut env = ambient.clone();
	if let Some(Value::Object(cred_env)) = credential.get("env") {
		for (k, v) in cred_env {
			if let Some(s) = v.as_str() {
				if !s.is_empty() {
					env.insert(k.clone(), s.to_string());
				}
			}
		}
	}
	env
}

// ============================================================================
// InMemoryAuthStorageBackend
// ============================================================================

/// TS `InMemoryAuthStorageBackend`. The TS `asyncChain` promise
/// serialization has no observable sync behavior (it only ordered
/// concurrent async operations) and is not ported.
pub struct InMemoryAuthStorageBackend {
	value: RefCell<Option<String>>,
}

impl InMemoryAuthStorageBackend {
	pub fn new() -> InMemoryAuthStorageBackend {
		InMemoryAuthStorageBackend { value: RefCell::new(None) }
	}

	/// The current stored content (harness-observable; TS `value` is
	/// private but readable through a `withLock` no-op).
	pub fn value(&self) -> Option<String> {
		self.value.borrow().clone()
	}
}

impl Default for InMemoryAuthStorageBackend {
	fn default() -> Self {
		Self::new()
	}
}

impl AuthStorageBackend for InMemoryAuthStorageBackend {
	fn with_lock<T>(
		&self,
		f: &mut dyn FnMut(Option<&str>) -> Result<LockResult<T>, JsError>,
		_options: Option<&AuthOperationOptions<'_>>,
	) -> Result<T, JsError> {
		// The TS `withLock` (unlike `withLockAsync`) performs no abort
		// checks.
		let current = self.value.borrow().clone();
		let outcome = f(current.as_deref())?;
		if let Some(next) = outcome.next {
			*self.value.borrow_mut() = Some(next);
		}
		Ok(outcome.result)
	}
}

// ============================================================================
// ReadOnlyAuthStorage
// ============================================================================

/// TS `ReadOnlyAuthStorage` — a `CredentialStore` that loads auth.json once
/// (validating strictly) and never writes. Error strings are verbatim:
/// - read failure: `Failed to read auth.json: <message>` (missing file →
///   empty data instead)
/// - non-object root: `Invalid auth.json: expected an object`
/// - bad entry: `Invalid auth.json credential for provider "<id>"`
/// - writes: `Read-only credential storage cannot modify auth.json`
pub struct ReadOnlyAuthStorage<'a> {
	auth_path: String,
	data: RefCell<Option<Map<String, Value>>>,
	resolver: RefCell<ConfigValueResolver<'a>>,
	ambient_env: Env,
}

impl<'a> ReadOnlyAuthStorage<'a> {
	/// TS constructor (`authPath` default is the caller's concern here —
	/// `getAgentDir` lives in pi-session).
	pub fn new(
		auth_path: impl Into<String>,
		resolver: ConfigValueResolver<'a>,
		ambient_env: Env,
	) -> ReadOnlyAuthStorage<'a> {
		ReadOnlyAuthStorage {
			auth_path: auth_path.into(),
			data: RefCell::new(None),
			resolver: RefCell::new(resolver),
			ambient_env,
		}
	}

	/// TS private `load` — read + validate once, then cache. Exposed for
	/// the parity harness.
	pub fn load_data(&self) -> Result<Map<String, Value>, JsError> {
		if let Some(data) = self.data.borrow().as_ref() {
			return Ok(data.clone());
		}

		let content = match std::fs::read_to_string(&self.auth_path) {
			Ok(content) => content,
			// ENOENT → empty data (cached).
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
				*self.data.borrow_mut() = Some(Map::new());
				return Ok(Map::new());
			}
			Err(e) => {
				// readFileSync opens the file, hence syscall "open".
				return Err(JsError::new(format!(
					"Failed to read auth.json: {}",
					node_error_message("open", &self.auth_path, &e)
				)));
			}
		};

		let parsed = js_json_parse(strip_bom(&content))
			.map_err(|message| JsError::new(format!("Failed to read auth.json: {message}")))?;
		let map = match parsed {
			Value::Object(map) => map,
			_ => return Err(JsError::new("Invalid auth.json: expected an object")),
		};
		for (provider_id, credential) in &map {
			validate_credential_entry(provider_id, credential)?;
		}
		*self.data.borrow_mut() = Some(map.clone());
		Ok(map)
	}

	/// TS `read` — exact raw-value semantics (parity surface).
	pub fn read_raw(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Value>, JsError> {
		check_aborted(options)?;
		let data = self.load_data()?;
		let credential = data.get(provider_id).cloned();
		check_aborted(options)?;
		let Some(credential) = credential else { return Ok(None) };
		// TS `if (!credential) return undefined` (falsy entries).
		if !js_truthy(&credential) {
			return Ok(None);
		}
		// TS `if (credential.type !== "api_key" || !credential.key ||
		// isCommandConfigValue(credential.key)) return structuredClone(...)`.
		let is_api_key =
			credential.get("type").and_then(Value::as_str) == Some("api_key");
		let key = credential.get("key");
		let resolves = is_api_key
			&& key.map(js_truthy).unwrap_or(false)
			&& key.and_then(Value::as_str).map(|k| !is_command_config_value(k)).unwrap_or(false);
		if !resolves {
			return Ok(Some(credential));
		}
		let key_str = key.and_then(Value::as_str).unwrap_or_default();
		let env = resolution_env(&credential, &self.ambient_env);
		let resolved = self.resolver.borrow_mut().resolve_config_value(key_str, Some(&env));
		let mut out = credential;
		let obj = out.as_object_mut().expect("validated api_key entries are objects");
		// `{ ...credential, key: resolved }` — a `undefined` resolution
		// drops the key entirely in JSON.
		match resolved {
			Some(v) => {
				obj.insert("key".to_string(), Value::String(v));
			}
			None => {
				obj.shift_remove("key"); // JS delete preserves order (serde_json remove = swap_remove)
			}
		}
		Ok(Some(out))
	}

	/// TS `list` — exact raw-value semantics (parity surface):
	/// `[{ providerId, type }]` with `type: null` for missing types.
	pub fn list_raw(
		&self,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<(String, Value)>, JsError> {
		check_aborted(options)?;
		let data = self.load_data()?;
		check_aborted(options)?;
		Ok(data
			.iter()
			.map(|(provider_id, credential)| {
				(provider_id.clone(), credential.get("type").cloned().unwrap_or(Value::Null))
			})
			.collect())
	}
}

/// TS `ReadOnlyAuthStorage.load` per-entry validation. Every surviving
/// entry is either a valid `api_key` (string-or-absent `key`, `env` an
/// object of strings when present) or a valid `oauth` (string `access`,
/// string `refresh`, finite number `expires`); anything else throws
/// `Invalid auth.json credential for provider "<id>"`.
fn validate_credential_entry(provider_id: &str, credential: &Value) -> Result<(), JsError> {
	let invalid = || {
		JsError::new(format!("Invalid auth.json credential for provider \"{provider_id}\""))
	};
	// typeof credential !== "object" || null || Array.isArray → throw.
	let Value::Object(value) = credential else { return Err(invalid()) };
	let ty = value.get("type").and_then(Value::as_str);
	if ty == Some("api_key") {
		let valid_key = matches!(value.get("key"), None | Some(Value::String(_)));
		let valid_env = match value.get("env") {
			None => true,
			Some(Value::Object(env)) => env.values().all(Value::is_string),
			Some(_) => false,
		};
		if valid_key && valid_env {
			return Ok(());
		}
		return Err(invalid());
	}
	if ty == Some("oauth")
		&& matches!(value.get("access"), Some(Value::String(_)))
		&& matches!(value.get("refresh"), Some(Value::String(_)))
		&& value.get("expires").and_then(Value::as_f64).is_some()
	{
		return Ok(());
	}
	Err(invalid())
}

impl<'a> CredentialStore for ReadOnlyAuthStorage<'a> {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Credential>, JsError> {
		let raw = self.read_raw(provider_id, options)?;
		Ok(raw
			.and_then(|v| v.as_object().cloned())
			.and_then(|m| Credential::from_map(m).ok()))
	}

	fn list(&self, options: Option<&AuthOperationOptions<'_>>) -> Result<Vec<CredentialInfo>, JsError> {
		Ok(self
			.list_raw(options)?
			.into_iter()
			.map(|(provider_id, ty)| CredentialInfo {
				provider_id,
				// Validation guarantees "api_key" | "oauth" here.
				credential_type: if ty.as_str() == Some("oauth") { "oauth" } else { "api_key" },
			})
			.collect())
	}

	fn modify(
		&mut self,
		_provider_id: &str,
		_fn: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
		_options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Credential>, JsError> {
		Err(JsError::new("Read-only credential storage cannot modify auth.json"))
	}

	fn delete(
		&mut self,
		_provider_id: &str,
		_options: Option<&AuthOperationOptions<'_>>,
	) -> Result<(), JsError> {
		Err(JsError::new("Read-only credential storage cannot modify auth.json"))
	}
}

// ============================================================================
// AuthStorage
// ============================================================================

/// TS `AuthFileReadState` (`data` + `revision`).
#[derive(Default)]
struct AuthReadState {
	data: Value,
	revision: Option<String>,
}

/// Module-level shared read state (TS `sharedAuthFileReadState`): one entry
/// for the last path, so repeated construction for the common path reuses
/// the cache (matching the models-store port).
fn shared_auth_read_state(path: &str) -> Arc<Mutex<AuthReadState>> {
	static SHARED: OnceLock<Mutex<Option<(String, Arc<Mutex<AuthReadState>>)>>> = OnceLock::new();
	let shared = SHARED.get_or_init(Default::default);
	let mut slot = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
	match slot.as_ref() {
		Some((cached_path, state)) if cached_path == path => Arc::clone(state),
		_ => {
			let state = Arc::new(Mutex::new(AuthReadState::default()));
			*slot = Some((path.to_string(), Arc::clone(&state)));
			state
		}
	}
}

/// TS `AuthStorage` — credential storage backed by the backend, with
/// revision-cached reads (G8: sync; the async reload machinery collapses —
/// see module docs).
///
/// The raw-`Value` methods (`read_raw`/`modify_raw`/`delete_raw`/
/// `list_raw`) are the exact TS port (unchecked `AuthStorageData` casts
/// included); the [`CredentialStore`] impl layers typed credentials on top.
pub struct AuthStorage<'a, B: AuthStorageBackend = FileAuthStorageBackend> {
	storage: B,
	auth_path: Option<String>,
	read_state: Arc<Mutex<AuthReadState>>,
	revision_fn: RevisionFn,
	resolver: RefCell<ConfigValueResolver<'a>>,
	ambient_env: Env,
}

impl<'a> AuthStorage<'a, FileAuthStorageBackend> {
	/// TS `create(authPath)`.
	pub fn create(
		auth_path: impl Into<String>,
		resolver: ConfigValueResolver<'a>,
		ambient_env: Env,
	) -> AuthStorage<'a, FileAuthStorageBackend> {
		let path = auth_path.into();
		let storage = FileAuthStorageBackend::new(path.clone());
		AuthStorage::with_backend(storage, Some(path), resolver, ambient_env)
	}
}

impl<'a, B: AuthStorageBackend> AuthStorage<'a, B> {
	/// TS `fromStorage(storage)`.
	pub fn from_storage(
		storage: B,
		resolver: ConfigValueResolver<'a>,
		ambient_env: Env,
	) -> AuthStorage<'a, B> {
		AuthStorage::with_backend(storage, None, resolver, ambient_env)
	}

	/// TS `inMemory(data = {})` — seeds an in-memory backend with the
	/// pretty-serialized data.
	pub fn in_memory(
		data: Map<String, Value>,
		resolver: ConfigValueResolver<'a>,
		ambient_env: Env,
	) -> AuthStorage<'a, InMemoryAuthStorageBackend> {
		let storage = InMemoryAuthStorageBackend::new();
		let seeded = js_stringify_pretty(&Value::Object(data));
		let _ = storage.with_lock::<()>(
			&mut |_| {
				Ok(LockResult { result: (), next: Some(seeded.clone()) })
			},
			None,
		);
		AuthStorage::from_storage(storage, resolver, ambient_env)
	}

	fn with_backend(
		storage: B,
		auth_path: Option<String>,
		resolver: ConfigValueResolver<'a>,
		ambient_env: Env,
	) -> AuthStorage<'a, B> {
		let read_state = match &auth_path {
			Some(path) => shared_auth_read_state(path),
			None => Arc::new(Mutex::new(AuthReadState::default())),
		};
		let this = AuthStorage {
			storage,
			auth_path,
			read_state,
			revision_fn: Box::new(pi_tools::node_fs::file_revision),
			resolver: RefCell::new(resolver),
			ambient_env,
		};
		// TS constructor: skip the reload when the revision is unchanged.
		if let Some(path) = &this.auth_path {
			let revision = (this.revision_fn)(path);
			let unchanged = {
				let state = this.read_state.lock().unwrap_or_else(|p| p.into_inner());
				revision.as_deref().is_some_and(|r| state.revision.as_deref() == Some(r))
			};
			if unchanged {
				return this;
			}
		}
		this.reload();
		this
	}

	/// Override the revision function (test injection; must be set before
	/// the first read — the constructor's skip check uses the default).
	pub fn with_revision_fn(mut self, f: RevisionFn) -> AuthStorage<'a, B> {
		self.revision_fn = f;
		self
	}

	/// The backing backend (harness/test accessor).
	pub fn storage_ref(&self) -> &B {
		&self.storage
	}

	fn snapshot(&self) -> Value {
		self.read_state.lock().unwrap_or_else(|p| p.into_inner()).data.clone()
	}

	fn update_read_state(&self, data: Value, revision: Option<String>) {
		let mut state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
		state.data = data;
		state.revision = revision;
	}

	/// TS `reload` — reload from storage, preserving the last valid
	/// in-memory snapshot on any error.
	pub fn reload(&self) {
		let _ = self.reload_from_storage();
	}

	/// TS `reloadFromStorageAsync` (sync): read + parse under the lock,
	/// recording the recomputed revision.
	fn reload_from_storage(&self) -> Result<Value, JsError> {
		self.storage.with_lock(
			&mut |content| {
				let data = parse_storage_data(content)?;
				let revision =
					self.auth_path.as_ref().and_then(|path| (self.revision_fn)(path));
				let mut state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
				state.data = data.clone();
				state.revision = revision;
				Ok(LockResult { result: data, next: None })
			},
			None,
		)
	}

	/// TS `readLatestData` (sync): serve the cached snapshot while the
	/// revision is unchanged; otherwise reload, falling back to the last
	/// snapshot when the reload fails (the TS
	/// `.catch(() => this.readState.data)` on the no-signal path).
	fn read_latest_data(&self) -> Result<Value, JsError> {
		if let Some(path) = &self.auth_path {
			let revision = (self.revision_fn)(path);
			if let Some(revision) = revision {
				let state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
				if state.revision.as_deref() == Some(revision.as_str()) {
					return Ok(state.data.clone());
				}
			}
		}
		match self.reload_from_storage() {
			Ok(data) => Ok(data),
			Err(_) => Ok(self.snapshot()),
		}
	}

	/// TS `read` — exact raw-value semantics (parity surface). `api_key`
	/// credentials with a present `key` are resolved via
	/// `resolveConfigValue(key, credential.env)` (commands included, unlike
	/// the read-only store); an `undefined` resolution drops the key.
	pub fn read_raw(
		&self,
		provider: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Value>, JsError> {
		let data = self.read_latest_data()?;
		check_aborted(options)?;
		let credential = js_get(&data, provider).cloned();
		let Some(credential) = credential else { return Ok(None) };
		// TS `if (credential?.type !== "api_key") return credential`.
		let is_api_key =
			credential.as_object().and_then(|o| o.get("type")).and_then(Value::as_str)
				== Some("api_key");
		if !is_api_key {
			return Ok(Some(credential));
		}
		// TS `if (credential.key === undefined) return credential`.
		let Some(key) = credential.get("key").cloned() else { return Ok(Some(credential)) };
		let Some(key_str) = key.as_str() else {
			// Non-string key: TS throws a TypeError from `startsWith`;
			// pass through (documented divergence — unreachable via
			// validated inputs).
			return Ok(Some(credential));
		};
		let env = resolution_env(&credential, &self.ambient_env);
		let resolved = self.resolver.borrow_mut().resolve_config_value(key_str, Some(&env));
		let mut out = credential;
		let obj = out.as_object_mut().expect("type === \"api_key\" implies an object");
		match resolved {
			Some(v) => {
				obj.insert("key".to_string(), Value::String(v));
			}
			None => {
				obj.shift_remove("key"); // JS delete preserves order (serde_json remove = swap_remove)
			}
		}
		Ok(Some(out))
	}

	/// TS `modify` — exact raw-value semantics (parity surface). The merge
	/// is `{ ...currentData, [provider]: next }` (existing keys keep their
	/// position) serialized with `JSON.stringify(merged, null, 2)`;
	/// `f` returning `None` leaves storage untouched and resolves with the
	/// current credential.
	pub fn modify_raw(
		&self,
		provider: &str,
		f: &mut dyn FnMut(Option<&Value>) -> Result<Option<Value>, JsError>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Value>, JsError> {
		let mut latest_data = self.snapshot();
		let mut revision: Option<String> = None;
		let result = self.storage.with_lock(
			&mut |content| {
				let current_data = parse_storage_data(content)?;
				let current_cred = js_get(&current_data, provider).cloned();
				let next = f(current_cred.as_ref())?;
				match next {
					None => {
						latest_data = current_data.clone();
						if let Some(path) = &self.auth_path {
							revision = (self.revision_fn)(path);
						}
						Ok(LockResult { result: js_get(&current_data, provider).cloned(), next: None })
					}
					Some(next) => {
						let mut merged = spread_to_record(&current_data);
						merged.insert(provider.to_string(), next.clone());
						latest_data = Value::Object(merged.clone());
						let serialized = js_stringify_pretty(&Value::Object(merged));
						Ok(LockResult { result: Some(next), next: Some(serialized) })
					}
				}
			},
			options,
		)?;
		// TS updateReadState(latestData, revision) — the revision is only
		// set on the untouched branch, so a write clears it (the next read
		// reloads once).
		self.update_read_state(latest_data, revision);
		Ok(result)
	}

	/// TS `delete` — exact raw-value semantics (parity surface).
	pub fn delete_raw(
		&self,
		provider: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<(), JsError> {
		let mut latest_data = self.snapshot();
		self.storage.with_lock(
			&mut |content| {
				let current = parse_storage_data(content)?;
				let mut next_value = current.clone();
				// TS `delete currentData[provider]`.
				match &mut next_value {
					Value::Object(map) => {
						map.shift_remove(provider); // JS delete preserves order
					}
					Value::Array(items) => {
						if let Some(i) = js_array_index(provider) {
							if i < items.len() {
								// A JS hole; `null` is JSON.stringify-identical.
								items[i] = Value::Null;
							}
						}
					}
					_ => {}
				}
				latest_data = next_value.clone();
				let next = js_stringify_pretty(&next_value);
				Ok(LockResult { result: (), next: Some(next) })
			},
			options,
		)?;
		// TS updateReadState(latestData) — revision cleared.
		self.update_read_state(latest_data, None);
		Ok(())
	}

	/// TS `list` — exact raw-value semantics (parity surface):
	/// `Object.entries(data)` mapped to `[{ providerId, type }]` with
	/// `type: null` for non-object entries / missing types.
	pub fn list_raw(
		&self,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<(String, Value)>, JsError> {
		let data = self.read_latest_data()?;
		check_aborted(options)?;
		Ok(object_entries(&data)
			.into_iter()
			.map(|(provider_id, credential)| {
				let ty = credential
					.as_object()
					.and_then(|o| o.get("type"))
					.cloned()
					.unwrap_or(Value::Null);
				(provider_id, ty)
			})
			.collect())
	}
}

impl<'a, B: AuthStorageBackend> CredentialStore for AuthStorage<'a, B> {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Credential>, JsError> {
		let raw = self.read_raw(provider_id, options)?;
		Ok(raw
			.and_then(|v| v.as_object().cloned())
			.and_then(|m| Credential::from_map(m).ok()))
	}

	fn list(&self, options: Option<&AuthOperationOptions<'_>>) -> Result<Vec<CredentialInfo>, JsError> {
		Ok(self
			.list_raw(options)?
			.into_iter()
			.map(|(provider_id, ty)| CredentialInfo {
				provider_id,
				// Raw data is unchecked; a missing type defaults to
				// "api_key" (chat) — documented divergence.
				credential_type: if ty.as_str() == Some("oauth") { "oauth" } else { "api_key" },
			})
			.collect())
	}

	fn modify(
		&mut self,
		provider_id: &str,
		f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<Credential>, JsError> {
		let result = self.modify_raw(
			provider_id,
			&mut |current: Option<&Value>| {
				let current_cred = current
					.and_then(|v| v.as_object())
					.and_then(|m| Credential::from_map(m.clone()).ok());
				let next = f(current_cred.as_ref())?;
				Ok(next.map(|c| c.to_value()))
			},
			options,
		)?;
		Ok(result
			.and_then(|v| v.as_object().cloned())
			.and_then(|m| Credential::from_map(m).ok()))
	}

	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<(), JsError> {
		self.delete_raw(provider_id, options)
	}
}

// ============================================================================
// readStoredCredential
// ============================================================================

/// TS `readStoredCredential(providerId, authPath)` — one-off synchronous
/// read of a stored credential, without instantiating a store or resolving
/// configured key values. Any error (missing file, invalid JSON) → `None`.
/// The raw entry is returned as-is (the TS cast is unchecked).
pub fn read_stored_credential(provider_id: &str, auth_path: &str) -> Option<Value> {
	let content = std::fs::read_to_string(auth_path).ok()?;
	let data = js_json_parse(strip_bom(&content)).ok()?;
	js_get(&data, provider_id).cloned()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use serde_json::json;
	use std::io::Write;

	/// Scripted command runner (never used by these tests — resolution is
	/// env-only here).
	struct NoCommands;
	impl CommandRunner for NoCommands {
		fn run(&self, _command: &str) -> Option<String> {
			None
		}
	}

	fn no_env() -> Env {
		Env::new()
	}

	fn read_only(content: Option<&str>) -> ReadOnlyAuthStorage<'static> {
		// Unique directory per call: the tests run in parallel and share a
		// process, so a per-process path would race on the same auth.json.
		static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
		let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		let dir =
			std::env::temp_dir().join(format!("pi-auth-storage-ro-{}-{}", std::process::id(), id));
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("auth.json");
		match content {
			Some(content) => std::fs::write(&path, content).unwrap(),
			None => {
				let _ = std::fs::remove_file(&path);
			}
		}
		ReadOnlyAuthStorage::new(
			path.to_string_lossy().into_owned(),
			ConfigValueResolver::new(&NoCommands),
			no_env(),
		)
	}

	pub(crate) fn in_memory(data: Value) -> AuthStorage<'static, InMemoryAuthStorageBackend> {
		let map = data.as_object().cloned().unwrap_or_default();
		AuthStorage::<InMemoryAuthStorageBackend>::in_memory(
			map,
			ConfigValueResolver::new(&NoCommands),
			no_env(),
		)
	}

	fn backend_content(storage: &AuthStorage<'_, InMemoryAuthStorageBackend>) -> Option<String> {
		// The final content is observable through the state machine: a
		// fresh store over the same backend reloads it.
		storage.storage_ref().value()
	}

	#[test]
	fn read_only_error_strings() {
		// JSON parse error (Node 24 V8 message, wrapped).
		let err = read_only(Some("{\"a\":}")).load_data().unwrap_err();
		assert_eq!(
			err.message,
			"Failed to read auth.json: Unexpected token '}', \"{\"a\":}\" is not valid JSON"
		);
		// Non-object roots.
		for content in ["[1, 2]", "null", "5", "\"x\""] {
			assert_eq!(
				read_only(Some(content)).load_data().unwrap_err().message,
				"Invalid auth.json: expected an object"
			);
		}
		// Credential validation branches.
		let invalid_entry = |content: &str| {
			read_only(Some(content)).load_data().unwrap_err().message
		};
		assert_eq!(
			invalid_entry("{\"p\": 5}"),
			"Invalid auth.json credential for provider \"p\""
		);
		assert_eq!(
			invalid_entry("{\"p\": null}"),
			"Invalid auth.json credential for provider \"p\""
		);
		assert_eq!(
			invalid_entry("{\"p\": []}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// No type at all.
		assert_eq!(
			invalid_entry("{\"p\": {\"key\": \"k\"}}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// Unknown type.
		assert_eq!(
			invalid_entry("{\"p\": {\"type\": \"weird\"}}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// api_key with non-string key.
		assert_eq!(
			invalid_entry("{\"p\": {\"type\": \"api_key\", \"key\": 5}}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// api_key with non-object env.
		assert_eq!(
			invalid_entry("{\"p\": {\"type\": \"api_key\", \"env\": \"x\"}}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// api_key with non-string env value.
		assert_eq!(
			invalid_entry("{\"p\": {\"type\": \"api_key\", \"env\": {\"a\": 5}}}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// oauth with non-number expires.
		assert_eq!(
			invalid_entry(
				"{\"p\": {\"type\": \"oauth\", \"access\": \"a\", \"refresh\": \"r\", \"expires\": \"x\"}}"
			),
			"Invalid auth.json credential for provider \"p\""
		);
		// oauth missing access.
		assert_eq!(
			invalid_entry("{\"p\": {\"type\": \"oauth\", \"refresh\": \"r\", \"expires\": 1}}"),
			"Invalid auth.json credential for provider \"p\""
		);
		// Valid entries load.
		let data = read_only(Some(
			"{\"a\": {\"type\": \"api_key\", \"key\": \"k\"}, \
			  \"b\": {\"type\": \"api_key\", \"env\": {\"x\": \"y\"}}, \
			  \"c\": {\"type\": \"oauth\", \"access\": \"a\", \"refresh\": \"r\", \"expires\": 1.5}}",
		))
		.load_data()
		.unwrap();
		assert_eq!(data.len(), 3);
		// Missing file → empty (cached).
		assert_eq!(read_only(None).load_data().unwrap().len(), 0);
	}

	#[test]
	fn read_only_write_errors() {
		let mut storage = read_only(Some("{}"));
		let err = storage.modify("p", &mut |_| Ok(None), None).unwrap_err();
		assert_eq!(err.message, "Read-only credential storage cannot modify auth.json");
		let err = storage.delete("p", None).unwrap_err();
		assert_eq!(err.message, "Read-only credential storage cannot modify auth.json");
	}

	#[test]
	fn read_only_read_semantics() {
		let storage = read_only(Some(
			// The "env" entry is a valid api_key credential with only an
			// env map (no key) — the TS validator rejects typeless entries.
			"{\"env\": {\"type\": \"api_key\", \"env\": {\"FOO\": \"from-cred\"}}, \"p\": {\"type\": \"api_key\", \"key\": \"$FOO\", \"env\": {\"FOO\": \"from-cred\"}}, \
			  \"q\": {\"type\": \"api_key\", \"key\": \"$MISSING\"}, \"r\": {\"type\": \"api_key\", \"key\": \"\"}, \
			  \"cmd\": {\"type\": \"api_key\", \"key\": \"!echo hi\"}, \"o\": {\"type\": \"oauth\", \"access\": \"a\", \"refresh\": \"r\", \"expires\": 1}}",
		));
		// Resolves via the credential-scoped env.
		assert_eq!(
			storage.read_raw("p", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "from-cred", "env": {"FOO": "from-cred"}})
		);
		// Missing env → key dropped (undefined).
		assert_eq!(storage.read_raw("q", None).unwrap().unwrap(), json!({"type": "api_key"}));
		// Empty key and command keys are NOT resolved by the read-only store.
		assert_eq!(
			storage.read_raw("r", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": ""})
		);
		assert_eq!(
			storage.read_raw("cmd", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "!echo hi"})
		);
		// Non-api_key passes through (parsed numbers are f64); missing → None.
		assert_eq!(
			storage.read_raw("o", None).unwrap().unwrap(),
			json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 1.0})
		);
		assert_eq!(storage.read_raw("nope", None).unwrap(), None);
		// List metadata.
		assert_eq!(
			storage.list_raw(None).unwrap(),
			vec![
				("env".to_string(), json!("api_key")),
				("p".to_string(), json!("api_key")),
				("q".to_string(), json!("api_key")),
				("r".to_string(), json!("api_key")),
				("cmd".to_string(), json!("api_key")),
				("o".to_string(), json!("oauth")),
			]
		);
	}

	#[test]
	fn in_memory_sequence_merge_order_and_delete() {
		let storage = in_memory(json!({
			"b": {"type": "api_key", "key": "kb"},
			"a": {"type": "oauth", "access": "A", "refresh": "R", "expires": 5}
		}));
		// modify returning None resolves with the current credential
		// (parsed values carry JS f64 numbers).
		assert_eq!(
			storage
				.modify_raw("a", &mut |_| Ok(None), None)
				.unwrap()
				.unwrap(),
			json!({"type": "oauth", "access": "A", "refresh": "R", "expires": 5.0})
		);
		// New keys append; existing keys keep their position.
		assert_eq!(
			storage
				.modify_raw("zz", &mut |_| Ok(Some(json!({"type": "api_key", "key": "kz"}))), None)
				.unwrap()
				.unwrap(),
			json!({"type": "api_key", "key": "kz"})
		);
		assert_eq!(
			storage.read_raw("b", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "kb"})
		);
		assert_eq!(
			storage.list_raw(None).unwrap(),
			vec![
				("b".to_string(), json!("api_key")),
				("a".to_string(), json!("oauth")),
				("zz".to_string(), json!("api_key")),
			]
		);
		storage.delete_raw("b", None).unwrap();
		assert_eq!(
			storage.list_raw(None).unwrap(),
			vec![
				("a".to_string(), json!("oauth")),
				("zz".to_string(), json!("api_key")),
			]
		);
		// Overwriting an existing key keeps its position.
		storage
			.modify_raw("a", &mut |_| Ok(Some(json!({"type": "api_key", "key": "na"}))), None)
			.unwrap();
		assert_eq!(
			storage.list_raw(None).unwrap(),
			vec![
				("a".to_string(), json!("api_key")),
				("zz".to_string(), json!("api_key")),
			]
		);
		// The serialized content is 2-space pretty JSON.
		let content = backend_content(&storage).unwrap();
		assert_eq!(content, "{\n  \"a\": {\n    \"type\": \"api_key\",\n    \"key\": \"na\"\n  },\n  \"zz\": {\n    \"type\": \"api_key\",\n    \"key\": \"kz\"\n  }\n}");
	}

	#[test]
	fn non_object_roots() {
		// Array root: index keys in list, element reads, spread merge.
		let backend = InMemoryAuthStorageBackend::new();
		let _ = backend.with_lock::<()>(
			&mut |_| {
				Ok(LockResult {
					result: (),
					next: Some("[{\"type\": \"api_key\", \"key\": \"arr0\"}, 7]".to_string()),
				})
			},
			None,
		);
		let storage =
			AuthStorage::from_storage(backend, ConfigValueResolver::new(&NoCommands), no_env());
		assert_eq!(
			storage.list_raw(None).unwrap(),
			vec![("0".to_string(), json!("api_key")), ("1".to_string(), Value::Null)]
		);
		assert_eq!(
			storage.read_raw("0", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "arr0"})
		);
		assert_eq!(storage.read_raw("1", None).unwrap().unwrap(), json!(7.0));
		assert_eq!(
			storage
				.modify_raw("0", &mut |_| Ok(Some(json!({"type": "api_key", "key": "new"}))), None)
				.unwrap()
				.unwrap(),
			json!({"type": "api_key", "key": "new"})
		);
		assert_eq!(
			storage.list_raw(None).unwrap(),
			vec![("0".to_string(), json!("api_key")), ("1".to_string(), Value::Null)]
		);
		// Number root: empty list, undefined reads, spread → {}.
		let backend = InMemoryAuthStorageBackend::new();
		let _ = backend.with_lock::<()>(
			&mut |_| Ok(LockResult { result: (), next: Some("5".to_string()) }),
			None,
		);
		let storage =
			AuthStorage::from_storage(backend, ConfigValueResolver::new(&NoCommands), no_env());
		assert!(storage.list_raw(None).unwrap().is_empty());
		assert_eq!(storage.read_raw("x", None).unwrap(), None);
		assert_eq!(
			storage
				.modify_raw(
					"x",
					&mut |_| Ok(Some(json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 1}))),
					None
				)
				.unwrap()
				.unwrap(),
			json!({"type": "oauth", "access": "a", "refresh": "r", "expires": 1})
		);
		// Empty-string content parses as {}.
		let backend = InMemoryAuthStorageBackend::new();
		let _ = backend.with_lock::<()>(
			&mut |_| Ok(LockResult { result: (), next: Some(String::new()) }),
			None,
		);
		let storage =
			AuthStorage::from_storage(backend, ConfigValueResolver::new(&NoCommands), no_env());
		assert!(storage.list_raw(None).unwrap().is_empty());
	}

	#[test]
	fn corrupted_reload_preserves_snapshot() {
		let backend = InMemoryAuthStorageBackend::new();
		let _ = backend.with_lock::<()>(
			&mut |_| {
				Ok(LockResult {
					result: (),
					next: Some("{\"p\": {\"type\": \"api_key\", \"key\": \"k\"}}".to_string()),
				})
			},
			None,
		);
		let storage =
			AuthStorage::from_storage(backend, ConfigValueResolver::new(&NoCommands), no_env());
		assert_eq!(
			storage.read_raw("p", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "k"})
		);
		// Corrupt the backend; reads keep serving the last snapshot.
		let _ = storage.storage_ref().with_lock::<()>(
			&mut |_| Ok(LockResult { result: (), next: Some("{".to_string()) }),
			None,
		);
		assert_eq!(
			storage.read_raw("p", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "k"})
		);
	}

	#[test]
	fn read_stored_credential_semantics() {
		let dir = std::env::temp_dir().join(format!("pi-auth-storage-rsc-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("auth.json");
		let mut file = std::fs::File::create(&path).unwrap();
		write!(
			file,
			"{}",
			"{\"p\": {\"type\": \"api_key\", \"key\": \"k\"}, \"weird\": {\"type\": \"nope\"}}"
		)
		.unwrap();
		drop(file);
		let path = path.to_string_lossy().into_owned();
		// Raw entries pass through unchecked; unknown provider → None.
		assert_eq!(
			read_stored_credential("p", &path),
			Some(json!({"type": "api_key", "key": "k"}))
		);
		assert_eq!(
			read_stored_credential("weird", &path),
			Some(json!({"type": "nope"}))
		);
		assert_eq!(read_stored_credential("x", &path), None);
		// Missing file / invalid JSON → None.
		assert_eq!(read_stored_credential("p", "/nonexistent/pi-auth.json"), None);
		let bad = dir.join("bad.json");
		std::fs::write(&bad, "{").unwrap();
		assert_eq!(read_stored_credential("p", bad.to_string_lossy().as_ref()), None);
	}

	#[test]
	fn file_backend_round_trip_and_bom() {
		let dir = std::env::temp_dir().join(format!("pi-auth-storage-file-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		let path = dir.join("auth.json");
		let storage = AuthStorage::create(
			path.to_string_lossy().into_owned(),
			ConfigValueResolver::new(&NoCommands),
			no_env(),
		);
		assert!(storage.list_raw(None).unwrap().is_empty());
		storage
			.modify_raw("p", &mut |_| Ok(Some(json!({"type": "api_key", "key": "k"}))), None)
			.unwrap();
		// A second store for the same path sees the written file (BOM
		// tolerated by the parse layer).
		std::fs::write(&path, format!("\u{feff}{}", std::fs::read_to_string(&path).unwrap())).unwrap();
		let storage2 = AuthStorage::create(
			path.to_string_lossy().into_owned(),
			ConfigValueResolver::new(&NoCommands),
			no_env(),
		);
		assert_eq!(
			storage2.read_raw("p", None).unwrap().unwrap(),
			json!({"type": "api_key", "key": "k"})
		);
		let _ = std::fs::remove_dir_all(&dir);
	}
}
