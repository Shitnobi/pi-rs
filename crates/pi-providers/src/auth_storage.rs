//! Auth storage — port of `coding-agent` `src/core/auth-storage.ts`
//! (foundation slice).
//!
//! This wave ports the storage backend seam the models-store file layer
//! reuses: the `AuthStorageBackend` trait (`with_lock` read-modify-write
//! over a file's string content) and `FileAuthStorageBackend`. The
//! TS backend uses `proper-lockfile` (cross-process lock files); the sync
//! G8 port serializes with a process-global mutex instead — cross-process
//! locking is a documented divergence (the agent runs one process per
//! user directory; see docs/plans/impl-notes-providers-foundation.md).
//!
//! The remaining auth-storage surface (lock retry policy, `ReadOnlyAuthStorage`
//! strict validation, `InMemoryAuthStorageBackend`, the `AuthStorage`
//! state machine with revision caching and credential resolution) lands
//! with the auth/composer wave per docs/plans/phase3-providers.md.

use std::sync::{Mutex, MutexGuard};

use crate::error::JsError;
use crate::pi_ai::credential::{check_aborted, AuthOperationOptions};
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

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn opts_aborted() -> AuthOperationOptions<'static> {
		use pi_tools::shell::AbortSignal;
		// A leaked, already-aborted signal (the options borrow it for the
		// test's lifetime).
		let signal: &'static AbortSignal = Box::leak(Box::new(AbortSignal::new()));
		signal.abort();
		AuthOperationOptions { signal: Some(signal) }
	}

	#[test]
	fn read_missing_file_is_empty() {
		let dir = std::env::temp_dir().join(format!("pi-auth-storage-{}", std::process::id()));
		let path = dir.join("does-not-exist.json");
		let backend = FileAuthStorageBackend::new(path.to_string_lossy().to_string());
		let result: String = backend
			.with_lock(&mut |content| Ok(LockResult { result: content.unwrap_or("empty").to_string(), next: None }), None)
			.unwrap();
		assert_eq!(result, "empty");
	}

	#[test]
	fn write_persists_and_reads_back() {
		let dir = std::env::temp_dir().join(format!("pi-auth-storage-write-{}", std::process::id()));
		let path = dir.join("nested/sub/data.json");
		let backend = FileAuthStorageBackend::new(path.to_string_lossy().to_string());
		backend
			.with_lock(&mut |_| Ok(LockResult { result: (), next: Some("{\"a\":1}".to_string()) }), None)
			.unwrap();
		let content: String = backend
			.with_lock(&mut |content| Ok(LockResult { result: content.unwrap().to_string(), next: None }), None)
			.unwrap();
		assert_eq!(content, "{\"a\":1}");
		std::fs::remove_dir_all(&dir).ok();
	}

	#[test]
	fn closure_error_skips_write() {
		let dir = std::env::temp_dir().join(format!("pi-auth-storage-err-{}", std::process::id()));
		let path = dir.join("skip.json");
		let backend = FileAuthStorageBackend::new(path.to_string_lossy().to_string());
		let err: Result<(), _> = backend.with_lock::<()>(
			&mut |_| Err(JsError::new("boom")),
			None,
		);
		assert!(err.is_err());
		assert!(!path.exists());
		std::fs::remove_dir_all(&dir).ok();
	}

	#[test]
	fn abort_rejects_before_read() {
		let backend = FileAuthStorageBackend::new("/nonexistent/pi-test.json");
		let result: Result<(), JsError> =
			backend.with_lock(&mut |_| unreachable!(), Some(&opts_aborted()));
		assert_eq!(result.unwrap_err().message, "This operation was aborted");
	}

	#[test]
	fn json_error_message_is_v8_shaped() {
		// node: JSON.parse("{") → Expected property name or '}' in JSON at
		// position 1 (line 1 column 2)
		let err = crate::pi_ai::js::js_json_parse("{").unwrap_err();
		assert_eq!(err, "Expected property name or '}' in JSON at position 1 (line 1 column 2)");
		let _ = json!({});
	}
}
