//! Runtime credentials overlay — port of `coding-agent` `src/core/runtime-credentials.ts`.
//!
//! The TS class is an async `CredentialStore` overlay for non-persistent
//! runtime API keys; per the G8 async-runtime decision this port is fully
//! synchronous. Observable semantics preserved: `read`/`delete` throw-if-
//! aborted before anything else, `list` checks the signal again *after* the
//! backing store resolves, runtime overrides shadow the backing store on
//! `read`, `list` merges override entries (`type: "api_key"`) on top of the
//! backing entries (override wins, backing order first, TS `Map` insertion
//! order kept via `IndexMap`), `modify` is pure delegation, and `delete`
//! only drops the override if the backing delete succeeded.

use indexmap::IndexMap;

use crate::error::JsError;
use crate::pi_ai::credential::{
	check_aborted, ApiKeyCredential, AuthOperationOptions, Credential, CredentialInfo,
	CredentialStore,
};

/// Build the `{ type: "api_key", key }` credential a runtime override reads
/// as (TS returns the plain literal `{ type: "api_key", key: override }`).
fn runtime_credential(key: &str) -> Credential {
	let mut map = serde_json::Map::new();
	map.insert(
		"key".to_string(),
		serde_json::Value::String(key.to_string()),
	);
	ApiKeyCredential::from_map(map)
		.map(Credential::ApiKey)
		.expect("well-formed api_key credential")
}

/// Async credential store overlay for non-persistent runtime API keys.
///
/// Wraps an owned backing `CredentialStore`; `overrides` keeps insertion
/// order (TS `Map`) so `list` merges in the same order TS would.
pub struct RuntimeCredentials {
	store: Box<dyn CredentialStore>,
	overrides: IndexMap<String, String>,
}

impl RuntimeCredentials {
	pub fn new(store: Box<dyn CredentialStore>) -> RuntimeCredentials {
		RuntimeCredentials {
			store,
			overrides: IndexMap::new(),
		}
	}

	pub fn set_runtime_api_key(&mut self, provider_id: &str, api_key: &str) {
		self.overrides
			.insert(provider_id.to_string(), api_key.to_string());
	}

	pub fn remove_runtime_api_key(&mut self, provider_id: &str) {
		self.overrides.shift_remove(provider_id);
	}

	pub fn has_runtime_api_key(&self, provider_id: &str) -> bool {
		self.overrides.contains_key(provider_id)
	}
}

impl CredentialStore for RuntimeCredentials {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError> {
		check_aborted(options)?;
		// TS truthiness: an empty-string override (`override ? … : …`)
		// falls through to the backing store.
		match self.overrides.get(provider_id).filter(|key| !key.is_empty()) {
			Some(override_key) => Ok(Some(runtime_credential(override_key))),
			None => self.store.read(provider_id, options),
		}
	}

	fn list(&self, options: Option<&AuthOperationOptions>) -> Result<Vec<CredentialInfo>, JsError> {
		let mut entries: IndexMap<String, CredentialInfo> = IndexMap::new();
		for entry in self.store.list(options)? {
			entries.insert(entry.provider_id.clone(), entry);
		}
		// TS checks the signal again after `await this.store.list(options)`.
		check_aborted(options)?;
		for provider_id in self.overrides.keys() {
			entries.insert(
				provider_id.clone(),
				CredentialInfo {
					provider_id: provider_id.clone(),
					credential_type: "api_key",
				},
			);
		}
		Ok(entries.into_values().collect())
	}

	#[allow(clippy::type_complexity)] // faithful to the TS callback shape
	fn modify(
		&mut self,
		provider_id: &str,
		f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
		options: Option<&AuthOperationOptions>,
	) -> Result<Option<Credential>, JsError> {
		// Pure delegation — no abort check of its own (backing store checks).
		self.store.modify(provider_id, f, options)
	}

	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions>,
	) -> Result<(), JsError> {
		check_aborted(options)?;
		self.store.delete(provider_id, options)?;
		// Only reached if the backing delete succeeded (TS awaits it first).
		self.overrides.shift_remove(provider_id);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use pi_tools::shell::AbortSignal;
	use serde_json::json;

	fn cred(key: &str) -> Credential {
		Credential::from_map(
			json!({"type": "api_key", "key": key})
				.as_object()
				.unwrap()
				.clone(),
		)
		.unwrap()
	}

	fn read_key(credential: &Credential) -> &str {
		match credential {
			Credential::ApiKey(c) => c.key.as_deref().unwrap(),
			Credential::OAuth(_) => panic!("expected api_key credential"),
		}
	}

	/// Order-preserving fake backing store (never checks abort itself, so
	/// the overlay's own check points are what the abort tests exercise).
	#[derive(Default)]
	struct FakeStore {
		entries: Vec<(String, Credential)>,
		fail_delete_on: Option<String>,
	}

	impl FakeStore {
		fn with(entries: &[(&str, &str)]) -> FakeStore {
			FakeStore {
				entries: entries
					.iter()
					.map(|(id, key)| (id.to_string(), cred(key)))
					.collect(),
				fail_delete_on: None,
			}
		}
	}

	impl CredentialStore for FakeStore {
		fn read(
			&self,
			provider_id: &str,
			_options: Option<&AuthOperationOptions>,
		) -> Result<Option<Credential>, JsError> {
			Ok(self
				.entries
				.iter()
				.find(|(id, _)| id == provider_id)
				.map(|(_, c)| c.clone()))
		}

		fn list(
			&self,
			_options: Option<&AuthOperationOptions>,
		) -> Result<Vec<CredentialInfo>, JsError> {
			Ok(self
				.entries
				.iter()
				.map(|(id, c)| CredentialInfo {
					provider_id: id.clone(),
					credential_type: c.credential_type(),
				})
				.collect())
		}

		fn modify(
			&mut self,
			provider_id: &str,
			f: &mut dyn FnMut(Option<&Credential>) -> Result<Option<Credential>, JsError>,
			_options: Option<&AuthOperationOptions>,
		) -> Result<Option<Credential>, JsError> {
			let current = self
				.entries
				.iter()
				.find(|(id, _)| id == provider_id)
				.map(|(_, c)| c);
			let next = f(current)?;
			let result = match next {
				Some(next) => {
					match self.entries.iter_mut().find(|(id, _)| id == provider_id) {
						Some(slot) => slot.1 = next.clone(),
						None => self.entries.push((provider_id.to_string(), next.clone())),
					}
					Some(next)
				}
				None => current.cloned(),
			};
			Ok(result)
		}

		fn delete(
			&mut self,
			provider_id: &str,
			_options: Option<&AuthOperationOptions>,
		) -> Result<(), JsError> {
			if self.fail_delete_on.as_deref() == Some(provider_id) {
				return Err(JsError::new("delete failed"));
			}
			self.entries.retain(|(id, _)| id != provider_id);
			Ok(())
		}
	}

	fn aborted_signal() -> AbortSignal {
		let signal = AbortSignal::new();
		signal.abort();
		signal
	}

	#[test]
	fn override_wins_on_read() {
		let mut rc =
			RuntimeCredentials::new(Box::new(FakeStore::with(&[("anthropic", "sk-backing")])));
		assert_eq!(
			read_key(&rc.read("anthropic", None).unwrap().unwrap()),
			"sk-backing"
		);
		rc.set_runtime_api_key("anthropic", "sk-runtime");
		let credential = rc.read("anthropic", None).unwrap().unwrap();
		assert_eq!(read_key(&credential), "sk-runtime");
		// Override applies even when the backing store has nothing.
		rc.set_runtime_api_key("openai", "sk-openai");
		assert_eq!(
			read_key(&rc.read("openai", None).unwrap().unwrap()),
			"sk-openai"
		);
	}

	#[test]
	fn read_delegates_when_no_override() {
		let rc = RuntimeCredentials::new(Box::new(FakeStore::with(&[("anthropic", "sk-backing")])));
		assert_eq!(
			read_key(&rc.read("anthropic", None).unwrap().unwrap()),
			"sk-backing"
		);
		assert_eq!(rc.read("missing", None).unwrap(), None);
	}

	#[test]
	fn list_merges_overrides_on_top() {
		let mut rc =
			RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "k-a"), ("b", "k-b")])));
		rc.set_runtime_api_key("b", "k-runtime");
		rc.set_runtime_api_key("c", "k-c");
		let list = rc.list(None).unwrap();
		let ids: Vec<(&str, &str)> = list
			.iter()
			.map(|e| (e.provider_id.as_str(), e.credential_type))
			.collect();
		// Backing order first, overridden entry replaced in place, new
		// override appended after (TS Map insertion order).
		assert_eq!(
			ids,
			vec![("a", "api_key"), ("b", "api_key"), ("c", "api_key")]
		);
	}

	#[test]
	fn delete_clears_override_only_on_success() {
		let mut rc = RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "k-a")])));
		rc.set_runtime_api_key("a", "k-runtime");
		rc.delete("a", None).unwrap();
		assert!(!rc.has_runtime_api_key("a"));
		assert_eq!(rc.read("a", None).unwrap(), None);

		let mut backing = FakeStore::with(&[("b", "k-b")]);
		backing.fail_delete_on = Some("b".to_string());
		let mut rc = RuntimeCredentials::new(Box::new(backing));
		rc.set_runtime_api_key("b", "k-runtime");
		assert!(rc.delete("b", None).is_err());
		// Backing delete failed → override must survive (TS awaits first).
		assert!(rc.has_runtime_api_key("b"));
	}

	#[test]
	fn aborted_operations_reject() {
		let mut rc = RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "k-a")])));
		rc.set_runtime_api_key("a", "k-runtime");
		let signal = aborted_signal();
		let opts = AuthOperationOptions {
			signal: Some(&signal),
		};
		assert_eq!(
			rc.read("a", Some(&opts)).unwrap_err().message,
			"This operation was aborted"
		);
		drop(signal);

		// FakeStore.list does not check abort itself — the overlay's second
		// throwIfAborted (after the backing await) is what rejects here.
		let rc = RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "k-a")])));
		let signal = aborted_signal();
		let opts = AuthOperationOptions {
			signal: Some(&signal),
		};
		assert_eq!(
			rc.list(Some(&opts)).unwrap_err().message,
			"This operation was aborted"
		);

		let mut rc = RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "k-a")])));
		rc.set_runtime_api_key("a", "k-runtime");
		let signal = aborted_signal();
		let opts = AuthOperationOptions {
			signal: Some(&signal),
		};
		assert_eq!(
			rc.delete("a", Some(&opts)).unwrap_err().message,
			"This operation was aborted"
		);
		assert!(
			rc.has_runtime_api_key("a"),
			"aborted delete must not clear the override"
		);
	}

	#[test]
	fn empty_string_override_falls_through_to_backing() {
		// TS: `override ? {type:"api_key",key} : this.store.read(...)` — the
		// empty string is falsy, so an empty override delegates.
		let mut rc =
			RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "sk-backing")])));
		rc.set_runtime_api_key("a", "");
		assert_eq!(
			read_key(&rc.read("a", None).unwrap().unwrap()),
			"sk-backing"
		);
	}

	#[test]
	fn has_and_remove_runtime_api_key() {
		let mut rc = RuntimeCredentials::new(Box::new(FakeStore::default()));
		assert!(!rc.has_runtime_api_key("a"));
		rc.set_runtime_api_key("a", "k-1");
		rc.set_runtime_api_key("a", "k-2");
		assert!(rc.has_runtime_api_key("a"));
		assert_eq!(read_key(&rc.read("a", None).unwrap().unwrap()), "k-2");
		rc.remove_runtime_api_key("a");
		assert!(!rc.has_runtime_api_key("a"));
		assert_eq!(rc.read("a", None).unwrap(), None);
		// Removing a missing key is a no-op.
		rc.remove_runtime_api_key("missing");
	}

	#[test]
	fn modify_is_pure_delegation() {
		let mut rc = RuntimeCredentials::new(Box::new(FakeStore::with(&[("a", "k-a")])));
		rc.set_runtime_api_key("a", "k-runtime");
		let result = rc
			.modify("a", &mut |_| Ok(Some(cred("k-new"))), None)
			.unwrap();
		assert_eq!(read_key(result.as_ref().unwrap()), "k-new");
		// Override is untouched by modify; read still prefers it.
		assert_eq!(read_key(&rc.read("a", None).unwrap().unwrap()), "k-runtime");
	}
}
