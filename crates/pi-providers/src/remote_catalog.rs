//! Remote catalog — port of `coding-agent`
//! `src/core/remote-catalog-provider.ts`.
//!
//! `withRemoteCatalog` / `withPersonalizedCatalog` overlay a persisted
//! pi.dev catalog on a static built-in provider. The catalog parse/merge/
//! refresh semantics (the observable behavior) are ported exactly, over the
//! **raw JSON model form** (the TS casts catalog entries to `AnyModel`
//! unchecked, so arbitrary objects flow through the merge); the
//! `Provider`-closure wrapper itself lands with the facade wave, which
//! needs a `getModels` override seam in `pi_ai::provider` (foundation-owned
//! module — do-not-edit per lib.rs module ownership).
//!
//! Network calls go through an injected fetch closure (`fetchWithRetry`
//! seam) — no real network, no `std::env`. `Date.now()` is an injected
//! `now` parameter; `signal.aborted` checks collapse in the sync port
//! (single-threaded refresh).
//!
//! Divergences (documented):
//! - `Date.parse` is a subset: only the RFC 7231 IMF-fixdate
//!   `Day, DD Mon YYYY HH:MM:SS GMT` (UTC). Anything else → `None` → 0,
//!   which matches the TS NaN → 0 mapping for the header formats the
//!   parity cases use.
//! - A stored entry whose `models` is not an array reads as empty (the TS
//!   unchecked cast would throw on `.filter`).
//! - Model ids that are not strings/numbers are keyed by their JS
//!   stringification (`[object Object]`, `undefined`).

use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::error::JsError;

/// TS `REMOTE_CATALOG_REFRESH_INTERVAL_MS` (4 hours, ms).
pub const REMOTE_CATALOG_REFRESH_INTERVAL_MS: f64 = 4.0 * 60.0 * 60.0 * 1000.0;

/// TS `REMOTE_CATALOG_MODEL_TYPES` — model types this client consumes,
/// sent as `?types=`.
pub const REMOTE_CATALOG_MODEL_TYPES: [&str; 3] = ["chat", "image", "classifier"];

/// TS `DEFAULT_CATALOG_BASE_URL`.
pub const DEFAULT_CATALOG_BASE_URL: &str = "https://pi.dev";

// ============================================================================
// Request/response seam (fetchWithRetry injection)
// ============================================================================

/// The request `fetchWithRetry` would issue (harness-observable).
#[derive(Debug, Clone)]
pub struct CatalogRequest {
	pub url: String,
	/// Header name/value pairs in construction order.
	pub headers: Vec<(String, String)>,
}

/// A successful-or-failed HTTP response, as the TS `Response` surface the
/// module consumes (`status`, `ok`, `last-modified`, `etag`, `.json()`).
#[derive(Debug, Clone)]
pub struct CatalogResponse {
	pub status: u16,
	pub ok: bool,
	pub last_modified: Option<String>,
	pub etag: Option<String>,
	pub body: Value,
}

/// TS `requestCatalog` — builds the catalog URL and headers.
/// `validator` is the `if-none-match` value (only sent when a cached body
/// backs it).
pub fn request_catalog(
	provider_id: &str,
	catalog_base_url: &str,
	validator: Option<&str>,
) -> CatalogRequest {
	let mut url = format!(
		"{}/api/models/providers/{}",
		catalog_base_url.trim_end_matches('/'),
		js_encode_uri_component(provider_id)
	);
	url.push_str("?types=");
	url.push_str(&REMOTE_CATALOG_MODEL_TYPES.join(","));
	let mut headers = vec![
		("accept".to_string(), "application/json".to_string()),
		// The real User-Agent is `getPiUserAgent(VERSION)`; the harness
		// normalizes it to `<user-agent>`.
		("User-Agent".to_string(), "<user-agent>".to_string()),
	];
	if let Some(validator) = validator {
		headers.push(("if-none-match".to_string(), validator.to_string()));
	}
	CatalogRequest { url, headers }
}

/// JS `encodeURIComponent` (unreserved set per the spec).
fn js_encode_uri_component(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for byte in s.bytes() {
		let c = byte as char;
		if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')') {
			out.push(c);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
	out
}

// ============================================================================
// Catalog parsing / merging (raw JSON model form)
// ============================================================================

/// pi-ai `getModelType` over the raw JSON form: the `type` field,
/// defaulting to `"chat"` when absent or null.
pub fn get_model_type_value(model: &Value) -> String {
	model
		.get("type")
		.and_then(Value::as_str)
		.unwrap_or("chat")
		.to_string()
}

/// pi-ai `isModelType(model, ty)` over the raw JSON form.
pub fn is_model_type_value(model: &Value, ty: &str) -> bool {
	get_model_type_value(model) == ty
}

/// JS template-string stringification of `model.id` (merge key component).
fn model_id_key(model: &Value) -> String {
	match model.get("id") {
		Some(Value::String(s)) => s.clone(),
		Some(Value::Number(n)) => n.to_string(),
		Some(Value::Bool(b)) => b.to_string(),
		Some(Value::Null) => "null".to_string(),
		None => "undefined".to_string(),
		_ => "[object Object]".to_string(),
	}
}

/// TS `isSupportedModelType`: `type` absent, or a string in
/// [`REMOTE_CATALOG_MODEL_TYPES`] (JSON `null` is NOT `undefined` →
/// unsupported).
pub fn is_supported_model_type(model: &Value) -> bool {
	match model.get("type") {
		None => true,
		Some(Value::String(s)) => REMOTE_CATALOG_MODEL_TYPES.contains(&s.as_str()),
		Some(_) => false,
	}
}

/// TS `mergeModels(baseline, dynamic)` — a Map keyed by
/// `` `${getModelType(model)}\0${model.id}` ``: later entries replace in
/// place, order is first-insertion.
pub fn merge_models(baseline: &[Value], dynamic: &[Value]) -> Vec<Value> {
	let mut merged: IndexMap<String, Value> = IndexMap::new();
	for model in baseline.iter().chain(dynamic.iter()) {
		let key = format!("{}\0{}", get_model_type_value(model), model_id_key(model));
		merged.insert(key, model.clone());
	}
	merged.into_values().collect()
}

/// TS `parseCatalog(providerId, value)` — accepts an array, an object with
/// a `models` array, or any object (whose values become the entries);
/// anything else throws `Invalid model catalog for provider "<id>"`.
/// Entries must be objects with an `id` and a supported type; each gets
/// `provider` overwritten (position preserved when already present).
pub fn parse_catalog(provider_id: &str, value: &Value) -> Result<Vec<Value>, JsError> {
	let entries: Vec<Value> = match value {
		Value::Array(items) => items.clone(),
		Value::Object(map) => match map.get("models") {
			Some(Value::Array(items)) => items.clone(),
			_ => map.values().cloned().collect(),
		},
		_ => {
			return Err(JsError::new(format!(
				"Invalid model catalog for provider \"{provider_id}\""
			)))
		}
	};
	Ok(entries
		.into_iter()
		.filter(|entry| entry.as_object().is_some_and(|o| o.contains_key("id")))
		.filter(is_supported_model_type)
		.map(|mut model| {
			model
				.as_object_mut()
				.expect("filtered to objects")
				.insert("provider".to_string(), Value::String(provider_id.to_string()));
			model
		})
		.collect())
}

// ============================================================================
// readCatalog (Date.parse subset)
// ============================================================================

/// V8 `Date.parse` subset: the RFC 7231 IMF-fixdate
/// `Day, DD Mon YYYY HH:MM:SS GMT` (UTC only). `None` for anything else
/// (the TS maps NaN → 0 in `readCatalog`).
pub fn js_date_parse_http(s: &str) -> Option<f64> {
	let s = s.trim();
	let (_, rest) = s.split_once(", ")?;
	let mut parts = rest.split(' ');
	let day: i64 = parts.next()?.parse().ok()?;
	let month = match parts.next()? {
		"Jan" => 1,
		"Feb" => 2,
		"Mar" => 3,
		"Apr" => 4,
		"May" => 5,
		"Jun" => 6,
		"Jul" => 7,
		"Aug" => 8,
		"Sep" => 9,
		"Oct" => 10,
		"Nov" => 11,
		"Dec" => 12,
		_ => return None,
	};
	let year: i64 = parts.next()?.parse().ok()?;
	let mut time = parts.next()?.split(':');
	let hour: i64 = time.next()?.parse().ok()?;
	let minute: i64 = time.next()?.parse().ok()?;
	let second: i64 = time.next()?.parse().ok()?;
	match parts.next()? {
		"GMT" | "UTC" => {}
		_ => return None,
	}
	if parts.next().is_some() {
		return None;
	}
	let days = days_from_civil(year, month, day);
	Some((((days * 24 + hour) * 60 + minute) * 60 + second) as f64 * 1000.0)
}

/// Howard Hinnant's `days_from_civil` (proleptic Gregorian, Unix epoch).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
	let y = if m <= 2 { y - 1 } else { y };
	let era = if y >= 0 { y } else { y - 399 } / 400;
	let yoe = y - era * 400;
	let mp = (m + 9) % 12;
	let doy = (153 * mp + 2) / 5 + d - 1;
	let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
	era * 146097 + doe - 719468
}

/// TS `readCatalog` output.
#[derive(Debug, Clone)]
pub struct CatalogData {
	pub models: Vec<Value>,
	/// Unix ms from the Last-Modified header; missing/invalid → 0.
	pub last_modified: f64,
	pub etag: Option<String>,
}

/// TS `readCatalog(providerId, response)`.
pub fn read_catalog(provider_id: &str, response: &CatalogResponse) -> Result<CatalogData, JsError> {
	let last_modified = response
		.last_modified
		.as_deref()
		.and_then(js_date_parse_http)
		.unwrap_or(0.0);
	Ok(CatalogData {
		models: parse_catalog(provider_id, &response.body)?,
		last_modified,
		etag: response.etag.clone(),
	})
}

// ============================================================================
// Stored entries / freshness
// ============================================================================

/// A persisted catalog entry (TS `ModelsStoreEntry`) in raw JSON form.
#[derive(Debug, Clone, Default)]
pub struct StoredCatalog {
	/// The original object form (spread source for persist payloads).
	pub raw: Value,
	pub models: Vec<Value>,
	pub last_modified: Option<f64>,
	pub checked_at: Option<f64>,
	pub etag: Option<String>,
}

impl StoredCatalog {
	pub fn from_value(value: &Value) -> StoredCatalog {
		let obj = value.as_object().cloned().unwrap_or_default();
		StoredCatalog {
			raw: value.clone(),
			models: obj.get("models").and_then(Value::as_array).cloned().unwrap_or_default(),
			last_modified: obj.get("lastModified").and_then(Value::as_f64),
			checked_at: obj.get("checkedAt").and_then(Value::as_f64),
			etag: obj.get("etag").and_then(Value::as_str).map(str::to_string),
		}
	}

	fn base(&self) -> Map<String, Value> {
		match &self.raw {
			Value::Object(map) => map.clone(),
			_ => {
				let mut map = Map::new();
				map.insert("models".to_string(), json!([]));
				map
			}
		}
	}

	/// `{ ...stored, checkedAt }` (TS spread; existing keys keep position).
	fn with_checked_at(&self, checked_at: f64) -> Value {
		let mut map = self.base();
		map.insert("checkedAt".to_string(), number_value(checked_at));
		Value::Object(map)
	}

	/// `{ ...(stored ?? { models: [] }), checkedAt, lastModified: 0,
	/// etag: undefined }` — the 404/501 payload (`etag` dropped).
	fn cleared(&self, checked_at: f64) -> Value {
		let mut map = self.with_checked_at(checked_at);
		let obj = map.as_object_mut().expect("constructed object");
		obj.insert("lastModified".to_string(), json!(0));
		obj.shift_remove("etag"); // JS delete preserves order
		map
	}
}

fn number_value(x: f64) -> Value {
	serde_json::from_str(&crate::pi_ai::js::js_number_to_string(x)).unwrap_or(json!(0))
}

/// TS `isNewer(lastModified, localGeneratedAt)`.
pub fn is_newer(last_modified: Option<f64>, local_generated_at: Option<f64>) -> bool {
	match local_generated_at {
		None => true,
		Some(local) => matches!(last_modified, Some(last) if last > local),
	}
}

/// TS `remoteModels(entry, localGeneratedAt)` — the stored overlay when it
/// is newer than the shipped model data, else empty.
pub fn remote_models(stored: Option<&StoredCatalog>, local_generated_at: Option<f64>) -> Vec<Value> {
	match stored {
		Some(entry) if is_newer(entry.last_modified, local_generated_at) => entry.models.clone(),
		_ => Vec::new(),
	}
}

// ============================================================================
// Refresh drivers (withRemoteCatalog / withPersonalizedCatalog)
// ============================================================================

/// One `context.publish` call, as observed.
#[derive(Debug, Clone)]
pub struct PublishRecord {
	/// The `persist` payload (entry JSON), or `None` when omitted.
	pub persist: Option<Value>,
	/// Whether the publication carried an `update`.
	pub update: bool,
	pub accepted: bool,
}

fn same_provider(model: &Value, provider_id: &str) -> bool {
	model.get("provider").and_then(Value::as_str) == Some(provider_id)
}

/// TS `withRemoteCatalog`'s `refreshModels` (sync driver). `dynamic_models`
/// is the overlay state (in/out); `publishes` records each
/// `context.publish`; `accept` is the generation check. Returns the error
/// the TS refresh would reject with, if any.
#[allow(clippy::too_many_arguments)] // faithful to the TS closure captures
pub fn refresh_with_remote_catalog(
	dynamic_models: &mut Vec<Value>,
	publishes: &mut Vec<PublishRecord>,
	provider_id: &str,
	catalog_base_url: &str,
	local_generated_at: Option<f64>,
	stored: Option<&StoredCatalog>,
	allow_network: bool,
	force: bool,
	now: f64,
	fetch: &dyn Fn(&CatalogRequest) -> Result<CatalogResponse, JsError>,
	accept: &mut dyn FnMut() -> bool,
) -> Result<(), JsError> {
	// Restore the stored overlay.
	let restored: Vec<Value> = remote_models(stored, local_generated_at)
		.into_iter()
		.filter(|m| same_provider(m, provider_id))
		.collect();
	let accepted = accept();
	publishes.push(PublishRecord { persist: None, update: true, accepted });
	if !accepted {
		return Ok(());
	}
	*dynamic_models = restored;
	if !allow_network {
		return Ok(());
	}
	// Skip the request when pi.dev was checked less than
	// REMOTE_CATALOG_REFRESH_INTERVAL_MS ago (an entry without
	// lastModified is always refetched).
	let skip = !force
		&& stored.is_some_and(|s| s.checked_at.is_some() && s.last_modified.is_some())
		&& now - stored.unwrap().checked_at.unwrap() < REMOTE_CATALOG_REFRESH_INTERVAL_MS;
	if skip {
		return Ok(());
	}
	// Only revalidate when a cached body backs the validator.
	let validator = stored
		.filter(|s| !s.models.is_empty())
		.and_then(|s| s.etag.clone());
	let response = fetch(&request_catalog(provider_id, catalog_base_url, validator.as_deref()))?;
	let checked_at = now;
	// 304: the overlay already holds the stored body.
	if response.status == 304 {
		if let Some(stored) = stored {
			let persist = stored.with_checked_at(checked_at);
			let accepted = accept();
			publishes.push(PublishRecord { persist: Some(persist), update: false, accepted });
			return Ok(());
		}
	}
	if response.status == 404 || response.status == 501 {
		let empty = StoredCatalog::from_value(&json!({ "models": [] }));
		let persist = stored.unwrap_or(&empty).cleared(checked_at);
		let accepted = accept();
		publishes.push(PublishRecord { persist: Some(persist), update: false, accepted });
		return Ok(());
	}
	if !response.ok {
		// Transient failure: keep the cached body and its validator.
		let empty = StoredCatalog::from_value(&json!({ "models": [] }));
		let persist = stored.unwrap_or(&empty).with_checked_at(checked_at);
		let accepted = accept();
		publishes.push(PublishRecord { persist: Some(persist), update: false, accepted });
		return Err(JsError::new(format!(
			"Model catalog request failed for {provider_id}: {}",
			response.status
		)));
	}
	let catalog = read_catalog(provider_id, &response)?;
	let entry = StoredCatalog {
		raw: {
			let mut map = Map::new();
			map.insert("models".to_string(), Value::Array(catalog.models.clone()));
			map.insert("lastModified".to_string(), number_value(catalog.last_modified));
			if let Some(etag) = &catalog.etag {
				map.insert("etag".to_string(), Value::String(etag.clone()));
			}
			map.insert("checkedAt".to_string(), number_value(checked_at));
			Value::Object(map)
		},
		models: catalog.models,
		last_modified: Some(catalog.last_modified),
		checked_at: Some(checked_at),
		etag: catalog.etag,
	};
	let published = remote_models(Some(&entry), local_generated_at);
	let persist = entry.raw.clone();
	let accepted = accept();
	publishes.push(PublishRecord { persist: Some(persist), update: true, accepted });
	if accepted {
		*dynamic_models = published;
	}
	Ok(())
}

/// TS `withPersonalizedCatalog`'s `refreshModels` (sync driver).
/// `personalize` receives the merged catalog (built-ins + newer remote
/// catalog); its result is persisted and replaces the model list. The
/// refresh interval and etag revalidation are skipped, per the TS.
#[allow(clippy::too_many_arguments)] // faithful to the TS closure captures
pub fn refresh_with_personalized_catalog(
	personal_models: &mut Vec<Value>,
	publishes: &mut Vec<PublishRecord>,
	provider_id: &str,
	catalog_base_url: &str,
	builtin_models: &[Value],
	local_generated_at: Option<f64>,
	stored: Option<&StoredCatalog>,
	allow_network: bool,
	now: f64,
	fetch: &dyn Fn(&CatalogRequest) -> Result<CatalogResponse, JsError>,
	personalize: &dyn Fn(&[Value]) -> Result<Vec<Value>, JsError>,
	accept: &mut dyn FnMut() -> bool,
) -> Result<(), JsError> {
	// Restore the saved personalized list (provider-filtered); an empty
	// list falls back to the built-ins via the accessors.
	let restored: Vec<Value> = stored
		.map(|s| s.models.iter().filter(|m| same_provider(m, provider_id)).cloned().collect())
		.unwrap_or_default();
	let accepted = accept();
	publishes.push(PublishRecord { persist: None, update: true, accepted });
	if !accepted {
		return Ok(());
	}
	*personal_models = restored;
	if !allow_network {
		return Ok(());
	}
	let response = fetch(&request_catalog(provider_id, catalog_base_url, None))?;
	let mut remote: Vec<Value> = Vec::new();
	if response.status != 404 && response.status != 501 {
		if !response.ok {
			return Err(JsError::new(format!(
				"Model catalog request failed for {provider_id}: {}",
				response.status
			)));
		}
		let catalog = read_catalog(provider_id, &response)?;
		if is_newer(Some(catalog.last_modified), local_generated_at) {
			remote = catalog.models;
		}
	}
	let personalized = personalize(&merge_models(builtin_models, &remote))?;
	let persist = {
		let mut map = Map::new();
		map.insert("models".to_string(), Value::Array(personalized.clone()));
		map.insert("checkedAt".to_string(), number_value(now));
		Value::Object(map)
	};
	let accepted = accept();
	publishes.push(PublishRecord { persist: Some(persist), update: true, accepted });
	if accepted {
		*personal_models = personalized;
	}
	Ok(())
}

// ============================================================================
// Post-refresh model accessors (the wrapper's getModels/getAllModels)
// ============================================================================

/// TS `withRemoteCatalog` `getModels`: the built-in chat models merged with
/// the chat-typed overlay entries.
pub fn remote_catalog_get_models(builtin_chat_models: &[Value], dynamic_models: &[Value]) -> Vec<Value> {
	let chat_overlay: Vec<Value> = dynamic_models
		.iter()
		.filter(|m| is_model_type_value(m, "chat"))
		.cloned()
		.collect();
	merge_models(builtin_chat_models, &chat_overlay)
}

/// TS `withRemoteCatalog` `getAllModels`.
pub fn remote_catalog_get_all_models(
	builtin_all_models: &[Value],
	dynamic_models: &[Value],
) -> Vec<Value> {
	merge_models(builtin_all_models, dynamic_models)
}

/// TS `withPersonalizedCatalog` `getModels`/`getAllModels`: the personal
/// list when non-empty, else the built-ins; `getModels` filters to chat.
pub fn personalized_get_all_models(
	builtin_all_models: &[Value],
	personal_models: &[Value],
) -> Vec<Value> {
	if personal_models.is_empty() {
		builtin_all_models.to_vec()
	} else {
		personal_models.to_vec()
	}
}

/// TS `withPersonalizedCatalog` `getModels`.
pub fn personalized_get_models(
	builtin_all_models: &[Value],
	personal_models: &[Value],
) -> Vec<Value> {
	personalized_get_all_models(builtin_all_models, personal_models)
		.into_iter()
		.filter(|m| is_model_type_value(m, "chat"))
		.collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;

	fn response(status: u16, body: Value, last_modified: Option<&str>) -> CatalogResponse {
		CatalogResponse {
			status,
			ok: (200..300).contains(&status),
			last_modified: last_modified.map(str::to_string),
			etag: Some("\"e\"".to_string()),
			body,
		}
	}

	#[test]
	fn constants() {
		assert_eq!(REMOTE_CATALOG_REFRESH_INTERVAL_MS, 14_400_000.0);
		assert_eq!(REMOTE_CATALOG_MODEL_TYPES, ["chat", "image", "classifier"]);
	}

	#[test]
	fn parse_catalog_shapes() {
		let m = |id: &str, ty: Option<&str>| match ty {
			Some(ty) => json!({"id": id, "type": ty, "name": id}),
			None => json!({"id": id, "name": id}),
		};
		// Array form.
		let models = parse_catalog("p", &json!([m("a", None), m("b", Some("image"))])).unwrap();
		assert_eq!(models.len(), 2);
		assert_eq!(models[0]["provider"], "p");
		// provider keeps its position when already present.
		let models = parse_catalog("p", &json!([{"id": "a", "provider": "x"}])).unwrap();
		let keys: Vec<&str> = models[0].as_object().unwrap().keys().map(String::as_str).collect();
		assert_eq!(keys, vec!["id", "provider"]);
		assert_eq!(models[0]["provider"], "p");
		// { models: [...] } form.
		assert_eq!(
			parse_catalog("p", &json!({"models": [m("a", None)]})).unwrap().len(),
			1
		);
		// Object form → Object.values.
		assert_eq!(
			parse_catalog("p", &json!({"x": m("a", None), "y": m("b", None)})).unwrap().len(),
			2
		);
		// models key present but not an array → Object.values (whose
		// non-object members, like the `models: 5` value itself, are then
		// filtered out by the entry filter).
		assert_eq!(
			parse_catalog("p", &json!({"models": 5, "x": m("a", None)})).unwrap().len(),
			1
		);
		// Invalid shapes.
		for value in [json!(5), json!("x"), json!(null)] {
			assert_eq!(
				parse_catalog("p", &value).unwrap_err().message,
				"Invalid model catalog for provider \"p\""
			);
		}
		// Filters: no id, unsupported type, null type.
		let models = parse_catalog(
			"p",
			&json!([
				{"name": "no-id"},
				m("ok", None),
				m("chat", Some("chat")),
				m("img", Some("image")),
				m("cls", Some("classifier")),
				m("future", Some("future")),
				{"id": "null-type", "type": null},
				["array", "entry"],
			]),
		)
		.unwrap();
		let ids: Vec<&str> = models.iter().map(|m| m["id"].as_str().unwrap()).collect();
		assert_eq!(ids, vec!["ok", "chat", "img", "cls"]);
	}

	#[test]
	fn merge_models_semantics() {
		let a = json!({"id": "m", "provider": "p"});
		let b = json!({"id": "m", "provider": "p", "tag": "dyn"});
		let img = json!({"id": "m", "type": "image"});
		// Later wins, position preserved.
		let merged = merge_models(&[a.clone()], &[b.clone()]);
		assert_eq!(merged.len(), 1);
		assert_eq!(merged[0]["tag"], "dyn");
		// Different types are different keys.
		let merged = merge_models(&[a.clone(), img.clone()], &[b.clone()]);
		assert_eq!(merged.len(), 2);
		assert_eq!(merged[0]["id"], "m");
		assert_eq!(merged[1]["type"], "image");
		// Untyped and "chat" collide (both key "chat").
		let chat = json!({"id": "m", "type": "chat"});
		assert_eq!(merge_models(&[a.clone()], &[chat]).len(), 1);
	}

	#[test]
	fn date_parse_subset() {
		assert_eq!(js_date_parse_http("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0.0));
		assert_eq!(js_date_parse_http("Fri, 02 Jan 1970 00:00:00 GMT"), Some(86_400_000.0));
		assert_eq!(js_date_parse_http("Mon, 19 Oct 2026 00:00:00 GMT").is_some(), true);
		assert_eq!(js_date_parse_http("nonsense"), None);
		assert_eq!(js_date_parse_http(""), None);
	}

	#[test]
	fn read_catalog_last_modified() {
		let r = response(200, json!({"models": []}), Some("Thu, 01 Jan 1970 00:00:00 GMT"));
		let catalog = read_catalog("p", &r).unwrap();
		assert_eq!(catalog.last_modified, 0.0);
		assert_eq!(catalog.etag.as_deref(), Some("\"e\""));
		// Missing header → 0.
		let r = response(200, json!([]), None);
		assert_eq!(read_catalog("p", &r).unwrap().last_modified, 0.0);
		// Invalid header → 0.
		let r = response(200, json!([]), Some("nope"));
		assert_eq!(read_catalog("p", &r).unwrap().last_modified, 0.0);
	}

	fn stored(models: Value, last_modified: Option<f64>, checked_at: Option<f64>) -> StoredCatalog {
		let mut raw = Map::new();
		raw.insert("models".to_string(), models);
		if let Some(lm) = last_modified {
			raw.insert("lastModified".to_string(), json!(lm));
		}
		if let Some(ca) = checked_at {
			raw.insert("checkedAt".to_string(), json!(ca));
		}
		// The TS sends if-none-match only when a cached body backs the
		// validator: `stored && stored.models.length > 0 ? stored.etag :
		// undefined`. The fixture models are non-empty, so an etag is
		// needed for the revalidation header to be sent.
		raw.insert("etag".to_string(), json!("\"e\""));
		StoredCatalog::from_value(&Value::Object(raw))
	}

	#[test]
	fn remote_models_filters_by_newer() {
		let m = json!({"id": "m", "provider": "p"});
		let entry = stored(json!([m.clone()]), Some(100.0), Some(50.0));
		assert_eq!(remote_models(Some(&entry), None).len(), 1);
		assert_eq!(remote_models(Some(&entry), Some(200.0)).len(), 0);
		assert_eq!(remote_models(Some(&entry), Some(100.0)).len(), 0); // not >
		assert_eq!(remote_models(None, None).len(), 0);
	}

	#[test]
	fn refresh_skip_interval_and_304() {
		let m = json!({"id": "m", "provider": "p"});
		let entry = stored(json!([m.clone()]), Some(100.0), Some(1_000_000.0));
		let mut dynamic = Vec::new();
		let mut publishes = Vec::new();
		let fetched = std::cell::Cell::new(0usize);
		let fetch = |_: &CatalogRequest| {
			fetched.set(fetched.get() + 1);
			Ok(response(200, json!([m.clone()]), Some("Fri, 02 Jan 1970 00:00:00 GMT")))
		};
		// now - checkedAt (14_000_000) < interval → no fetch.
		refresh_with_remote_catalog(
			&mut dynamic, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, None,
			Some(&entry), true, false, 15_000_000.0, &fetch, &mut || true,
		)
		.unwrap();
		assert_eq!(fetched.get(), 0);
		assert_eq!(dynamic.len(), 1);
		// Beyond the interval → fetch, merge, persist.
		publishes.clear();
		refresh_with_remote_catalog(
			&mut dynamic, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, None,
			Some(&entry), true, false, 15_400_000.0, &fetch, &mut || true,
		)
		.unwrap();
		assert_eq!(fetched.get(), 1);
		let last = publishes.last().unwrap();
		assert!(last.persist.as_ref().unwrap().get("checkedAt").is_some());
		// force bypasses the interval.
		publishes.clear();
		refresh_with_remote_catalog(
			&mut dynamic, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, None,
			Some(&entry), true, true, 15_000_000.0, &fetch, &mut || true,
		)
		.unwrap();
		assert_eq!(fetched.get(), 2);
	}

	#[test]
	fn refresh_status_branches() {
		let m = json!({"id": "m", "provider": "p"});
		let entry = stored(json!([m.clone()]), Some(100.0), Some(1_000.0));
		for (status, expect_error) in [(304u16, false), (404, false), (501, false), (500, true)] {
			let mut dynamic = Vec::new();
			let mut publishes = Vec::new();
			let models = m.clone();
			let fetch = move |req: &CatalogRequest| {
				// Validator only sent when a cached body backs it.
				assert!(req.headers.iter().any(|(k, _)| k == "if-none-match"));
				Ok(response(status, json!([models.clone()]), Some("Fri, 02 Jan 1970 00:00:00 GMT")))
			};
			let result = refresh_with_remote_catalog(
				&mut dynamic, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, None,
				Some(&entry), true, true, 0.0, &fetch, &mut || true,
			);
			assert_eq!(result.is_err(), expect_error, "status {status}");
			if expect_error {
				assert_eq!(
					result.unwrap_err().message,
					format!("Model catalog request failed for p: {status}")
				);
			}
			if status == 404 || status == 501 {
				let persist = publishes.last().unwrap().persist.clone().unwrap();
				assert_eq!(persist["lastModified"], 0);
				assert!(persist.get("etag").is_none());
			}
		}
		// No stored entry + 304 → falls through to the ok check (304 is
		// not ok) → transient error.
		let mut dynamic = Vec::new();
		let mut publishes = Vec::new();
		let fetch = |_: &CatalogRequest| Ok(response(304, json!([]), None));
		let err = refresh_with_remote_catalog(
			&mut dynamic, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, None,
			None, true, true, 0.0, &fetch, &mut || true,
		)
		.unwrap_err();
		assert_eq!(err.message, "Model catalog request failed for p: 304");
	}

	#[test]
	fn request_catalog_shape() {
		let req = request_catalog("prov id", "https://pi.dev", Some("\"v\""));
		assert_eq!(req.url, "https://pi.dev/api/models/providers/prov%20id?types=chat,image,classifier");
		let names: Vec<&str> = req.headers.iter().map(|(k, _)| k.as_str()).collect();
		assert_eq!(names, vec!["accept", "User-Agent", "if-none-match"]);
		let req = request_catalog("p", "https://pi.dev", None);
		assert!(!req.headers.iter().any(|(k, _)| k == "if-none-match"));
	}

	#[test]
	fn personalized_refresh() {
		let builtin = json!({"id": "b", "provider": "p"});
		let remote = json!({"id": "r", "provider": "p"});
		let mut personal = Vec::new();
		let mut publishes = Vec::new();
		let fetch = |_: &CatalogRequest| {
			Ok(response(200, json!([remote.clone()]), Some("Fri, 02 Jan 1970 00:00:00 GMT")))
		};
		let personalize = |catalog: &[Value]| Ok(catalog.to_vec());
		refresh_with_personalized_catalog(
			&mut personal, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, &[builtin.clone()],
			None, None, true, 42.0, &fetch, &personalize, &mut || true,
		)
		.unwrap();
		assert_eq!(personal.len(), 2);
		let persist = publishes.last().unwrap().persist.clone().unwrap();
		assert_eq!(persist["checkedAt"], 42.0);
		assert_eq!(persist["models"].as_array().unwrap().len(), 2);
		// 404 → personalized over built-ins only.
		let mut personal = Vec::new();
		let mut publishes = Vec::new();
		let fetch = |_: &CatalogRequest| Ok(response(404, json!(null), None));
		refresh_with_personalized_catalog(
			&mut personal, &mut publishes, "p", DEFAULT_CATALOG_BASE_URL, &[builtin.clone()],
			None, None, true, 42.0, &fetch, &personalize, &mut || true,
		)
		.unwrap();
		assert_eq!(personal.len(), 1);
		// Empty personal list falls back to built-ins in the accessors.
		assert_eq!(personalized_get_all_models(&[builtin.clone()], &[]).len(), 1);
		assert_eq!(personalized_get_models(&[builtin.clone()], &[]).len(), 1);
	}
}
