//! Persistent model catalogs — port of `pi-ai` `src/models-store.ts`.
//!
//! TS interface is Promise-based; sync port per the G8 decision (this layer
//! stays synchronous). `structuredClone` → `Clone` (JSON data only).

use crate::error::JsError;
use crate::pi_ai::credential::{check_aborted, AuthOperationOptions};
use crate::pi_ai::model::AnyModel;

/// Persisted models of one provider (TS `ModelsStoreEntry`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelsStoreEntry {
	/// Persisted models of every type.
	pub models: Vec<AnyModel>,
	/// Unix timestamp (ms) from the remote catalog's Last-Modified header.
	pub last_modified: Option<f64>,
	/// Unix timestamp (ms) of the last completed remote check.
	pub checked_at: Option<f64>,
	/// Opaque validator from the remote catalog's ETag header, stored
	/// verbatim (quotes included) and echoed back as If-None-Match.
	pub etag: Option<String>,
}

impl ModelsStoreEntry {
	/// Ordered JSON object form (key order matches the TS interface:
	/// models, lastModified?, checkedAt?, etag?).
	pub fn to_value(&self) -> serde_json::Value {
		let mut map = serde_json::Map::new();
		map.insert(
			"models".to_string(),
			serde_json::Value::Array(self.models.iter().map(|m| m.to_value()).collect()),
		);
		if let Some(v) = self.last_modified {
			map.insert("lastModified".to_string(), crate::pi_ai::model::JsNumber(v).into_value());
		}
		if let Some(v) = self.checked_at {
			map.insert("checkedAt".to_string(), crate::pi_ai::model::JsNumber(v).into_value());
		}
		if let Some(v) = &self.etag {
			map.insert("etag".to_string(), serde_json::Value::String(v.clone()));
		}
		serde_json::Value::Object(map)
	}

	/// Parse from the stored JSON form (unknown keys ignored, like the TS
	/// cast `as ModelsStoreEntry`).
	pub fn from_value(v: &serde_json::Value) -> Option<ModelsStoreEntry> {
		let obj = v.as_object()?;
		Some(ModelsStoreEntry {
			models: obj
				.get("models")?
				.as_array()?
				.iter()
				.map(|m| serde_json::from_value(m.clone()).ok())
				.collect::<Option<Vec<AnyModel>>>()?,
			last_modified: obj.get("lastModified").and_then(|x| x.as_f64()),
			checked_at: obj.get("checkedAt").and_then(|x| x.as_f64()),
			etag: obj.get("etag").and_then(|x| x.as_str()).map(str::to_string),
		})
	}
}

/// `ModelsStoreOperationOptions` — same shape as `AuthOperationOptions`.
pub type ModelsStoreOperationOptions<'a> = AuthOperationOptions<'a>;

/// Persistent model catalogs keyed by provider ID (TS `ModelsStore`,
/// sync port). Methods fail only on storage errors.
pub trait ModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions>,
	) -> Result<Option<ModelsStoreEntry>, JsError>;
	fn write(
		&mut self,
		provider_id: &str,
		entry: &ModelsStoreEntry,
		options: Option<&ModelsStoreOperationOptions>,
	) -> Result<(), JsError>;
	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions>,
	) -> Result<(), JsError>;
}

/// TS `InMemoryModelsStore`. Entries are cloned in and out
/// (`structuredClone` in TS).
#[derive(Default)]
pub struct InMemoryModelsStore {
	entries: std::collections::BTreeMap<String, ModelsStoreEntry>,
}

impl InMemoryModelsStore {
	pub fn new() -> InMemoryModelsStore {
		InMemoryModelsStore { entries: std::collections::BTreeMap::new() }
	}
}

impl ModelsStore for InMemoryModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions>,
	) -> Result<Option<ModelsStoreEntry>, JsError> {
		check_aborted(options)?;
		Ok(self.entries.get(provider_id).cloned())
	}

	fn write(
		&mut self,
		provider_id: &str,
		entry: &ModelsStoreEntry,
		options: Option<&ModelsStoreOperationOptions>,
	) -> Result<(), JsError> {
		check_aborted(options)?;
		self.entries.insert(provider_id.to_string(), entry.clone());
		Ok(())
	}

	fn delete(
		&mut self,
		provider_id: &str,
		options: Option<&ModelsStoreOperationOptions>,
	) -> Result<(), JsError> {
		check_aborted(options)?;
		self.entries.remove(provider_id);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn entry_round_trip() {
		let model = json!({"id": "m", "name": "M", "api": "openai-completions", "provider": "p", "baseUrl": "https://x", "input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}, "reasoning": false, "contextWindow": 128000, "maxTokens": 16384});
		let entry = ModelsStoreEntry {
			models: vec![serde_json::from_value(model.clone()).unwrap()],
			last_modified: Some(123.5),
			checked_at: None,
			etag: Some("\"abc\"".to_string()),
		};
		let v = entry.to_value();
		assert_eq!(v["lastModified"], 123.5);
		assert_eq!(v["etag"], "\"abc\"");
		let back = ModelsStoreEntry::from_value(&v).unwrap();
		assert_eq!(back, entry);
	}

	#[test]
	fn in_memory_store() {
		let mut store = InMemoryModelsStore::new();
		let entry = ModelsStoreEntry { models: vec![], last_modified: None, checked_at: Some(1.0), etag: None };
		assert_eq!(store.read("p", None).unwrap(), None);
		store.write("p", &entry, None).unwrap();
		assert_eq!(store.read("p", None).unwrap(), Some(entry.clone()));
		store.delete("p", None).unwrap();
		assert_eq!(store.read("p", None).unwrap(), None);
	}

	#[test]
	fn aborted_rejects() {
		use pi_tools::shell::AbortSignal;
		let signal = AbortSignal::new();
		signal.abort();
		let opts = ModelsStoreOperationOptions { signal: Some(&signal) };
		let store = InMemoryModelsStore::new();
		assert_eq!(store.read("p", Some(&opts)).unwrap_err().message, "This operation was aborted");
	}
}
