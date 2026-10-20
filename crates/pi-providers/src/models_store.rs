//! Persistent model catalogs — port of `coding-agent`
//! `src/core/models-store.ts`.
//!
//! `InMemoryCodingAgentModelsStore` mirrors the pi-ai in-memory store
//! (kept as its own type, like the TS). `FileModelsStore` is the locked
//! JSON-backed storage for dynamically refreshed provider catalogs: it
//! reuses the auth-storage backend seam (`with_lock` read-modify-write),
//! keeps a module-level shared read state per path (the TS
//! `sharedModelsFileReadState` — one entry for the common path), and
//! skips re-parsing while the file revision is unchanged.
//!
//! The TS async machinery collapses under the G8 sync decision: the
//! single-flight reload (`AbortController` + reader counting) becomes a
//! direct synchronous reload, which is equivalent single-threaded.
//!
//! Divergences (documented in docs/plans/impl-notes-providers-foundation.md):
//! - the TS `parse` casts any JSON to `StoredModels` unchecked; the port
//!   requires a JSON object and errors otherwise;
//! - the default path (`join(getAgentDir(), "models-store.json")`) is the
//!   caller's responsibility — `get_agent_dir` lives in pi-session, which
//!   this crate does not depend on (the facade computes it).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value};

use crate::auth_storage::{AuthStorageBackend, FileAuthStorageBackend, LockResult};
use crate::error::JsError;
use crate::pi_ai::credential::check_aborted;
use crate::pi_ai::js::{js_json_parse, js_stringify_pretty};
use crate::pi_ai::models_store::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use pi_core::text::strip_bom;

/// Revisions are injectable for tests (TS parity harness injects
/// `getFileRevision`).
pub type RevisionFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// TS `InMemoryCodingAgentModelsStore`.
#[derive(Default)]
pub struct InMemoryCodingAgentModelsStore {
	entries: BTreeMap<String, ModelsStoreEntry>,
}

impl InMemoryCodingAgentModelsStore {
	pub fn new() -> InMemoryCodingAgentModelsStore {
		InMemoryCodingAgentModelsStore { entries: BTreeMap::new() }
	}
}

impl ModelsStore for InMemoryCodingAgentModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<Option<ModelsStoreEntry>, JsError> {
		check_aborted(options)?;
		Ok(self.entries.get(provider_id).cloned())
	}

	fn write(
		&mut self,
		provider_id: &str,
		entry: &ModelsStoreEntry,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<(), JsError> {
		check_aborted(options)?;
		self.entries.insert(provider_id.to_string(), entry.clone());
		Ok(())
	}

	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<(), JsError> {
		check_aborted(options)?;
		self.entries.remove(provider_id); // BTreeMap (sorted) — pre-existing design
		Ok(())
	}
}

/// TS `ModelsFileReadState` (data + revision).
#[derive(Default)]
struct ReadState {
	data: Map<String, Value>,
	revision: Option<String>,
}

/// Module-level shared read state (TS `sharedModelsFileReadState` l23):
/// one entry for the last path, so repeated construction of a
/// `FileModelsStore` for the common path reuses the cache.
fn shared_read_state(path: &str) -> Arc<Mutex<ReadState>> {
	static SHARED: OnceLock<Mutex<Option<(String, Arc<Mutex<ReadState>>)>>> = OnceLock::new();
	let shared = SHARED.get_or_init(Default::default);
	let mut slot = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
	match slot.as_ref() {
		Some((cached_path, state)) if cached_path == path => Arc::clone(state),
		_ => {
			let state = Arc::new(Mutex::new(ReadState::default()));
			*slot = Some((path.to_string(), Arc::clone(&state)));
			state
		}
	}
}

/// Locked JSON-backed storage for dynamically refreshed provider catalogs
/// (TS `FileModelsStore`). Generic over the backend so the parity harness
/// can run it over a fake; the default is the real file backend.
pub struct FileModelsStore<B: AuthStorageBackend = FileAuthStorageBackend> {
	storage: B,
	path: String,
	revision_fn: RevisionFn,
	read_state: Arc<Mutex<ReadState>>,
}

impl FileModelsStore<FileAuthStorageBackend> {
	/// The TS constructor (path required — see module docs about the
	/// `getAgentDir` default).
	pub fn new(path: impl Into<String>) -> FileModelsStore<FileAuthStorageBackend> {
		let path = path.into();
		let storage = FileAuthStorageBackend::new(path.clone());
		FileModelsStore::with_backend(storage, path)
	}
}

impl<B: AuthStorageBackend> FileModelsStore<B> {
	pub fn with_backend(storage: B, path: impl Into<String>) -> FileModelsStore<B> {
		let path = path.into();
		let read_state = shared_read_state(&path);
		FileModelsStore {
			storage,
			path,
			revision_fn: Box::new(pi_tools::node_fs::file_revision),
			read_state,
		}
	}

	/// Override the revision function (parity-harness injection).
	pub fn with_revision_fn(mut self, f: RevisionFn) -> FileModelsStore<B> {
		self.revision_fn = f;
		self
	}

	pub fn path(&self) -> &str {
		&self.path
	}

	/// TS `parse`: `JSON.parse(stripBom(content))` with no schema
	/// validation. Parse errors carry the V8 message (via `js_json_parse`).
	/// `None` (missing file) parses as empty.
	fn parse_stored(content: Option<&str>) -> Result<Map<String, Value>, JsError> {
		let Some(content) = content else {
			return Ok(Map::new());
		};
		let value = js_json_parse(strip_bom(content)).map_err(JsError::new)?;
		match value {
			Value::Object(map) => Ok(map),
			// Divergence: TS casts any JSON to StoredModels unchecked.
			_ => Err(JsError::new("Invalid models-store.json: expected a JSON object")),
		}
	}

	/// TS `readLatest`: serve the cached snapshot while the file revision
	/// is unchanged, else reload under the storage lock. (The TS
	/// single-flight reload collapses to a direct sync reload.)
	fn read_latest(
		&self,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<Map<String, Value>, JsError> {
		check_aborted(options)?;
		let revision = (self.revision_fn)(&self.path);
		{
			let state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
			if let Some(revision) = revision.as_deref() {
				if state.revision.as_deref() == Some(revision) {
					return Ok(state.data.clone());
				}
			}
		}
		self.storage.with_lock(
			&mut |content| {
				let data = Self::parse_stored(content)?;
				// TS reloadFromStorage recomputes the revision under the
				// lock and records it.
				let revision = (self.revision_fn)(&self.path);
				let mut state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
				state.data = data.clone();
				state.revision = revision;
				Ok(LockResult { result: data, next: None })
			},
			options,
		)
	}
}

impl<B: AuthStorageBackend> ModelsStore for FileModelsStore<B> {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<Option<ModelsStoreEntry>, JsError> {
		let data = self.read_latest(options)?;
		let entry = data.get(provider_id).cloned();
		// TS checks the signal again after the read settles.
		check_aborted(options)?;
		Ok(entry.and_then(|value| ModelsStoreEntry::from_value(&value)))
	}

	fn write(
		&mut self,
		provider_id: &str,
		entry: &ModelsStoreEntry,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<(), JsError> {
		let entry_value = entry.to_value();
		let latest = self.storage.with_lock(
			&mut |content| {
				let mut current = Self::parse_stored(content)?;
				current.insert(provider_id.to_string(), entry_value.clone());
				let latest = current.clone();
				let next = js_stringify_pretty(&Value::Object(current));
				Ok(LockResult { result: Some(latest), next: Some(next) })
			},
			options,
		)?;
		// TS updateReadState(readState, latest) — the revision is left
		// unset, so the next read reloads once.
		if let Some(latest) = latest {
			let mut state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
			state.data = latest;
			state.revision = None;
		}
		Ok(())
	}

	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions<'_>>,
	) -> Result<(), JsError> {
		let latest = self.storage.with_lock(
			&mut |content| {
				let mut current = Self::parse_stored(content)?;
				current.shift_remove(provider_id); // JS delete preserves order
				let latest = current.clone();
				let next = js_stringify_pretty(&Value::Object(current));
				Ok(LockResult { result: Some(latest), next: Some(next) })
			},
			options,
		)?;
		if let Some(latest) = latest {
			let mut state = self.read_state.lock().unwrap_or_else(|p| p.into_inner());
			state.data = latest;
			state.revision = None;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::credential::AuthOperationOptions;
	use pi_tools::shell::AbortSignal;
	use serde_json::json;
	use std::cell::RefCell;

	fn entry(model_id: &str) -> ModelsStoreEntry {
		let model = json!({"id": model_id, "name": model_id, "api": "a", "provider": "p",
			"baseUrl": "https://x", "input": ["text"],
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"reasoning": false, "contextWindow": 128000, "maxTokens": 16384});
		ModelsStoreEntry {
			models: vec![serde_json::from_value(model).unwrap()],
			last_modified: Some(123.0),
			checked_at: None,
			etag: Some("\"e\"".to_string()),
		}
	}

	/// Fake backend: in-memory content string, counts lock invocations.
	struct FakeBackend {
		content: RefCell<Option<String>>,
		locks: std::cell::Cell<usize>,
	}

	impl FakeBackend {
		fn new(content: Option<&str>) -> FakeBackend {
			FakeBackend { content: RefCell::new(content.map(str::to_string)), locks: std::cell::Cell::new(0) }
		}
	}

	impl AuthStorageBackend for FakeBackend {
		fn with_lock<T>(
			&self,
			f: &mut dyn FnMut(Option<&str>) -> Result<LockResult<T>, JsError>,
			options: Option<&AuthOperationOptions<'_>>,
		) -> Result<T, JsError> {
			crate::pi_ai::credential::check_aborted(options)?;
			self.locks.set(self.locks.get() + 1);
			let content = self.content.borrow().clone();
			let outcome = f(content.as_deref())?;
			if let Some(next) = &outcome.next {
				*self.content.borrow_mut() = Some(next.clone());
			}
			Ok(outcome.result)
		}
	}

	#[test]
	fn in_memory_round_trip() {
		let mut store = InMemoryCodingAgentModelsStore::new();
		assert_eq!(store.read("p", None).unwrap(), None);
		store.write("p", &entry("m"), None).unwrap();
		assert_eq!(store.read("p", None).unwrap().unwrap().models.len(), 1);
		store.delete("p", None).unwrap();
		assert_eq!(store.read("p", None).unwrap(), None);
	}

	fn store_with(content: Option<&str>, path: &str) -> (FileModelsStore<FakeBackendHandle>, std::rc::Rc<FakeBackend>) {
		let backend = std::rc::Rc::new(FakeBackend::new(content));
		let store = FileModelsStore::with_backend(
			FakeBackendHandle(std::rc::Rc::clone(&backend)),
			path,
		)
		.with_revision_fn(Box::new(|_| None));
		(store, backend)
	}

	// Rc-sharing handle: the backend trait must be object-safe per-instance
	// but the test needs to inspect the same content the store locks.
	struct FakeBackendHandle(std::rc::Rc<FakeBackend>);

	impl AuthStorageBackend for FakeBackendHandle {
		fn with_lock<T>(
			&self,
			f: &mut dyn FnMut(Option<&str>) -> Result<LockResult<T>, JsError>,
			options: Option<&AuthOperationOptions<'_>>,
		) -> Result<T, JsError> {
			self.0.with_lock(f, options)
		}
	}

	fn unique_path(name: &str) -> String {
		format!("/tmp/pi-models-store-test/{}/{}", std::process::id(), name)
	}

	#[test]
	fn file_store_read_write_delete() {
		let path = unique_path("rwd");
		let (mut store, backend) = store_with(None, &path);
		assert_eq!(store.read("p", None).unwrap(), None);

		store.write("p", &entry("m1"), None).unwrap();
		let read = store.read("p", None).unwrap().unwrap();
		assert_eq!(read.models.len(), 1);
		assert_eq!(read.last_modified, Some(123.0));
		assert_eq!(read.etag.as_deref(), Some("\"e\""));

		// The written content is 2-space pretty JSON.
		let content = backend.content.borrow().clone().unwrap();
		assert!(content.contains("\n  \"p\": {"));
		assert!(content.starts_with('{'));

		store.delete("p", None).unwrap();
		assert_eq!(store.read("p", None).unwrap(), None);
	}

	#[test]
	fn file_store_strips_bom_and_reports_v8_parse_errors() {
		// node: JSON.parse("{") → Expected property name or '}' in JSON at
		// position 1 (line 1 column 2)
		let path = unique_path("parse-err");
		let (store, _) = store_with(Some("{"), &path);
		let err = store.read("p", None).unwrap_err();
		assert_eq!(err.message, "Expected property name or '}' in JSON at position 1 (line 1 column 2)");

		// BOM is stripped before parsing.
		let path = unique_path("bom");
		let (store, _) = store_with(Some("\u{feff}{\"p\": {\"models\": []}}"), &path);
		assert_eq!(store.read("p", None).unwrap().unwrap().models, vec![]);
	}

	#[test]
	fn file_store_requires_object_payload() {
		let path = unique_path("non-object");
		let (store, _) = store_with(Some("[1, 2]"), &path);
		assert_eq!(
			store.read("p", None).unwrap_err().message,
			"Invalid models-store.json: expected a JSON object"
		);
	}

	#[test]
	fn revision_unchanged_serves_cache() {
		let path = unique_path("revision-cache");
		let (store, backend) = store_with(Some("{\"p\": {\"models\": []}}"), &path);
		let store = store.with_revision_fn({
			// A fixed revision after the first read — the second read must
			// NOT take the storage lock again.
			let first = std::sync::atomic::AtomicBool::new(true);
			Box::new(move |_| {
				if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
					None // first read: no revision → reload
				} else {
					Some("rev-1".to_string())
				}
			})
		});
		_ = store.read("p", None);
		let locks_before = backend.locks.get();
		_ = store.read("p", None);
		assert_eq!(backend.locks.get(), locks_before, "unchanged revision must serve the cache");
	}

	#[test]
	fn write_resets_revision_so_next_read_reloads() {
		use std::sync::atomic::{AtomicBool, Ordering};
		let path = unique_path("revision-reset");
		let (mut store, backend) = store_with(Some("{\"p\": {\"models\": []}}"), &path);
		// Armed → fixed revision; unarmed → no revision (always reload).
		let armed = std::sync::Arc::new(AtomicBool::new(false));
		let armed_fn = std::sync::Arc::clone(&armed);
		store.revision_fn = Box::new(move |_| {
			if armed_fn.load(Ordering::SeqCst) {
				Some("rev".to_string())
			} else {
				None
			}
		});
		_ = store.read("p", None).unwrap(); // reload, cached revision None
		armed.store(true, Ordering::SeqCst);
		_ = store.read("p", None).unwrap(); // reload, cached revision Some("rev")
		let locks_cached = backend.locks.get();
		_ = store.read("p", None).unwrap(); // served from cache
		assert_eq!(backend.locks.get(), locks_cached, "unchanged revision serves the cache");

		// Writing resets the cached revision (TS leaves it undefined)…
		store.write("q", &entry("m"), None).unwrap();
		// …so the next read reloads from storage.
		_ = store.read("q", None).unwrap();
		assert!(backend.locks.get() > locks_cached, "read after write must reload");
	}

	#[test]
	fn cache_serves_across_store_instances_for_same_path() {
		let path = unique_path("shared");
		let (_store_a, backend) = store_with(Some("{\"p\": {\"models\": []}}"), &path);
		// A second store for the same path with a stable revision serves
		// the cache without locking (it shares the per-path read state or
		// primes its own — either way the second read must not lock).
		let store_b = FileModelsStore::with_backend(FakeBackendHandle(std::rc::Rc::clone(&backend)), path)
			.with_revision_fn(Box::new(|_| Some("same-rev".to_string())));
		_ = store_b.read("p", None).unwrap(); // prime
		let locks_before = backend.locks.get();
		let result = store_b.read("p", None).unwrap();
		assert!(result.is_some());
		assert_eq!(backend.locks.get(), locks_before, "stable revision serves the cache");
	}

	#[test]
	fn abort_rejects() {
		let path = unique_path("abort");
		let (mut store, _) = store_with(None, &path);
		let signal = AbortSignal::new();
		signal.abort();
		let opts = AuthOperationOptions { signal: Some(&signal) };
		assert_eq!(
			store.read("p", Some(&opts)).unwrap_err().message,
			"This operation was aborted"
		);
		assert_eq!(
			store.write("p", &entry("m"), Some(&opts)).unwrap_err().message,
			"This operation was aborted"
		);
	}
}
