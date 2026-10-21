// Minimal pi-ai type surface (docs/plans/phase3-providers.md §6).
//
// Ports the parts of `@earendil-works/pi-ai` (TS: /tmp/pi-repo/packages/ai)
// that the provider layer consumes: model shapes, credentials, provider and
// Models interfaces, plus the JS-semantics helpers in `js`.
//
// The builtin provider catalog is vendored data (`builtin-catalog.json`,
// generated from the TS `models.generated.ts` dist output; structure:
// `{ generatedAt, schemaVersion, providers: { <provider>: { <api>:
// { "chat:<id>" | "image:<id>" | "classifier:<id>": <model> } } } }` — the
// type-prefixed keys keep chat/classifier entries of the same id apart).

use std::sync::OnceLock;

use serde_json::Value;

// `macro_rules!` are textually scoped: `macros` must be declared before the
// modules that use `js_object!`.
#[macro_use]
pub mod macros;

pub mod compat;
pub mod credential;
pub mod js;
pub mod lazy;
pub mod model;
pub mod model_operations;
pub mod models;
pub mod models_error;
pub mod models_impl;
pub mod models_store;
pub mod transcript;
pub mod provider;

/// The vendored builtin provider catalog (parsed once, shared).
///
/// Mirrors the TS `providers/all.ts` model lists: `MODELS`, `IMAGE_MODELS`
/// and `CLASSIFIER_MODELS` from `models.generated.ts`, grouped by provider
/// and api with type-prefixed model keys. Regenerate with
/// scripts/gen-builtin-catalog.mjs against the TS dist (1731 models across
/// 42 providers as of 2026-10-20).
pub fn builtin_catalog() -> Value {
	static CATALOG: OnceLock<Value> = OnceLock::new();
	CATALOG
		.get_or_init(|| {
			serde_json::from_str(include_str!("builtin-catalog.json"))
				.expect("builtin-catalog.json must be valid JSON")
		})
		.clone()
}
