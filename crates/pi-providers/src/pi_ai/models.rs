//! Models — port of `pi-ai` `src/models.ts` collection interfaces and
//! `src/utils/model-operations.ts` pure functions.
//!
//! The `Models`/`MutableModels` traits are the collection contract the
//! facade wave (`model_runtime`) implements; this module declares the
//! surface (G8: sync) plus the pure model helpers (`get_model_type`,
//! `has_api`, `calculate_cost`, thinking-level selection). All
//! `calculateCost`/thinking-level semantics are differentially verified
//! against the TS dist (see docs/plans/impl-notes-providers-foundation.md).

use indexmap::IndexMap;

use crate::error::JsError;
use crate::pi_ai::credential::{
	AuthCheck, AuthOperationOptions, AuthResult, AuthType, Credential,
};
use crate::pi_ai::model::{
	AnyModel, ImageModel, Model, ModelThinkingLevel, ModelType, ProviderEnv,
};
use crate::pi_ai::models_store::ModelsStoreEntry;
use crate::pi_ai::provider::{
	AssistantImages, AssistantMessage, AssistantMessageEventStream, ClassifierContext,
	ClassifierOptions, ClassifierResult, DeferredCancelOptions, DeferredFetchOptions,
	DeferredHandle, ImagesContext, ImagesOptions, Provider, SimpleStreamOptions,
	StreamOptions, TranscriptContext,
};
use pi_tools::shell::AbortSignal;

// ============================================================================
// Model type operations (utils/model-operations.ts)
// ============================================================================

/// The type of a model. Models without `type` are chat models.
///
/// Reads the raw `type` field (not the enum variant) so an oddly-tagged
/// entry behaves like the TS `model.type ?? "chat"`.
pub fn get_model_type(model: &AnyModel) -> ModelType {
	match model.type_field() {
		None => ModelType::Chat,
		Some("chat") => ModelType::Chat,
		Some("image") => ModelType::Image,
		Some("classifier") => ModelType::Classifier,
		Some(other) => ModelType::Unknown(other.to_string()),
	}
}

/// Runtime-checked model type narrowing, including legacy chat models
/// without `type`.
pub fn is_model_type(model: &AnyModel, ty: &ModelType) -> bool {
	&get_model_type(model) == ty
}

/// Runtime-checked API narrowing for dynamically looked-up chat models
/// (TS `hasApi`). Non-chat models never match, even when their api id
/// equals `api`.
pub fn has_api(model: &AnyModel, api: &str) -> bool {
	is_model_type(model, &ModelType::Chat) && model.api() == api
}

/// Check if two models are equal by comparing their type, id, and
/// provider. Returns false if either model is absent.
pub fn models_are_equal(a: Option<&AnyModel>, b: Option<&AnyModel>) -> bool {
	match (a, b) {
		(Some(a), Some(b)) => {
			get_model_type(a) == get_model_type(b) && a.id() == b.id() && a.provider() == b.provider()
		}
		_ => false,
	}
}

// ============================================================================
// Usage / calculateCost (models.ts l1206)
// ============================================================================

/// TS `Usage["cost"]` — USD cost breakdown of one response.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageCost {
	pub input: f64,
	pub output: f64,
	#[serde(rename = "cacheRead")]
	pub cache_read: f64,
	#[serde(rename = "cacheWrite")]
	pub cache_write: f64,
	pub total: f64,
}

/// TS `Usage` (types.ts l451). Token counts are JS numbers; `cacheWrite1h`
/// and `reasoning` are optional. Serialized form (camelCase, as part of
/// assistant messages) lands with the agent-session tranches.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
	pub input: f64,
	pub output: f64,
	#[serde(rename = "cacheRead")]
	pub cache_read: f64,
	#[serde(rename = "cacheWrite")]
	pub cache_write: f64,
	/// Subset of `cache_write` written with 1h retention.
	#[serde(rename = "cacheWrite1h", default)]
	pub cache_write_1h: Option<f64>,
	/// Reasoning tokens (subset of `output`).
	#[serde(default)]
	pub reasoning: Option<f64>,
	#[serde(rename = "totalTokens", default)]
	pub total_tokens: f64,
	#[serde(default)]
	pub cost: UsageCost,
}

/// Accumulated request cost in USD, using the model's rates (with
/// request-wide tier selection) and this usage. Mutates and returns
/// `usage.cost`, like the TS `calculateCost`.
///
/// Tiers: the highest `inputTokensAbove` strictly below the request's
/// total input tokens (`input + cacheRead + cacheWrite`) wins; the
/// request qualifies only when its token count is strictly above the
/// threshold. Anthropic charges 2× base input for 1h cache writes.
pub fn calculate_cost<'a>(model: &AnyModel, usage: &'a mut Usage) -> &'a UsageCost {
	let input_tokens = usage.input + usage.cache_read + usage.cache_write;
	let cost = model.cost();
	// `let rates: ModelCostRates = model.cost` — the base cost is the
	// zeroth tier; `ModelCost` is a `ModelCostRates` plus `tiers`.
	let mut rates = [
		cost.input.get(),
		cost.output.get(),
		cost.cache_read.get(),
		cost.cache_write.get(),
	];
	let mut matched_threshold = -1.0;
	if let Some(tiers) = &cost.tiers {
		for tier in tiers {
			let above = tier.input_tokens_above.get();
			if input_tokens > above && above > matched_threshold {
				rates = [
					tier.input.get(),
					tier.output.get(),
					tier.cache_read.get(),
					tier.cache_write.get(),
				];
				matched_threshold = above;
			}
		}
	}

	// Anthropic charges 2x base input for 1h cache writes.
	let long_write = usage.cache_write_1h.unwrap_or(0.0);
	let short_write = usage.cache_write - long_write;
	usage.cost.input = (rates[0] / 1000000.0) * usage.input;
	usage.cost.output = (rates[1] / 1000000.0) * usage.output;
	usage.cost.cache_read = (rates[2] / 1000000.0) * usage.cache_read;
	usage.cost.cache_write = (rates[3] * short_write + rates[0] * 2.0 * long_write) / 1000000.0;
	usage.cost.total =
		usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
	&usage.cost
}

// ============================================================================
// Thinking levels (models.ts l1230/l1241)
// ============================================================================

/// `EXTENDED_THINKING_LEVELS` — the full ordered scale; `xhigh`/`max` are
/// the extended entries (they need an explicit map entry to be offered).
pub const EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
	ModelThinkingLevel::Off,
	ModelThinkingLevel::Minimal,
	ModelThinkingLevel::Low,
	ModelThinkingLevel::Medium,
	ModelThinkingLevel::High,
	ModelThinkingLevel::Xhigh,
	ModelThinkingLevel::Max,
];

/// Thinking levels the model supports:
/// - non-reasoning models support only `off`;
/// - a `null` map entry excludes the level;
/// - `xhigh`/`max` require an explicit (non-null) map entry;
/// - every other level is supported whenever the model reasons.
pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
	if !model.reasoning {
		return vec![ModelThinkingLevel::Off];
	}
	EXTENDED_THINKING_LEVELS
		.iter()
		.copied()
		.filter(|&level| {
			let mapped = model
				.thinking_level_map
				.as_ref()
				.and_then(|map| map.get(level.as_str()));
			match mapped {
				// TS `mapped === null` → excluded.
				Some(None) => false,
				// TS `mapped === undefined` → only xhigh/max are excluded.
				None => level != ModelThinkingLevel::Xhigh && level != ModelThinkingLevel::Max,
				Some(Some(_)) => true,
			}
		})
		.collect()
}

/// Clamp a requested level to one the model supports: the nearest
/// supported level at or above the request, else the nearest below, else
/// the first supported level.
pub fn clamp_thinking_level(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
	let available_levels = get_supported_thinking_levels(model);
	if available_levels.contains(&level) {
		return level;
	}

	// A valid `ModelThinkingLevel` always has an index in the extended
	// scale; the TS `-1` case (unknown level string) cannot occur here and
	// falls through to the final fallback below.
	let requested_index = level.index();
	for candidate in EXTENDED_THINKING_LEVELS.iter().skip(requested_index) {
		if available_levels.contains(candidate) {
			return *candidate;
		}
	}
	for candidate in EXTENDED_THINKING_LEVELS.iter().take(requested_index).rev() {
		if available_levels.contains(candidate) {
			return *candidate;
		}
	}
	available_levels.first().copied().unwrap_or(ModelThinkingLevel::Off)
}

// ============================================================================
// Refresh types (models.ts l73/l80/l103/l112)
// ============================================================================

/// `ModelsPublication.persist` — TS `persist?: ModelsStoreEntry | null`:
/// omit to leave storage unchanged, `null` deletes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Persist {
	/// Omitted — leave storage unchanged.
	Unchanged,
	/// Write this entry.
	Write(ModelsStoreEntry),
	/// `null` — delete the stored entry.
	Delete,
}

/// Provider-selected persistence plus an optional synchronous in-memory
/// catalog update (TS `ModelsPublication`).
pub struct ModelsPublication {
	pub persist: Persist,
	pub update: Option<Box<dyn FnOnce() + Send>>,
}

impl Default for ModelsPublication {
	fn default() -> ModelsPublication {
		ModelsPublication { persist: Persist::Unchanged, update: None }
	}
}

/// `RefreshModelsContext` — what a dynamic provider's `refreshModels`
/// receives (G8: sync). `publish` is generation-checked: it reports
/// whether the publication was accepted.
pub struct RefreshModelsContext<'a> {
	/// Effective configured credential (OAuth refreshed before network).
	pub credential: Option<&'a Credential>,
	/// Immutable provider-scoped catalog snapshot from before this phase.
	pub stored: Option<&'a ModelsStoreEntry>,
	/// `publish(publication) -> accepted` (TS `Promise<boolean>`).
	pub publish: &'a mut dyn FnMut(ModelsPublication) -> Result<bool, JsError>,
	/// False during offline/cache-only initialization.
	pub allow_network: bool,
	/// Bypass freshness checks and fetch immediately.
	pub force: bool,
	/// Always present (the shared abort signal of the refresh operation).
	pub signal: Option<&'a AbortSignal>,
	/// Base URL of the registered provider, including overrides applied
	/// by wrappers outside the provider.
	pub base_url: Option<&'a str>,
}

/// `ModelsRefreshOptions` (TS `allowNetwork` defaults to `true` in
/// `refresh`).
#[derive(Default)]
pub struct ModelsRefreshOptions<'a> {
	pub allow_network: Option<bool>,
	/// Restrict refresh to these provider IDs (unknown/static ignored).
	pub providers: Option<Vec<String>>,
	pub force: bool,
	pub signal: Option<&'a AbortSignal>,
}

/// `ModelsRefreshResult` — provider errors keyed by provider id, in
/// refresh order (TS `ReadonlyMap`, insertion-ordered).
#[derive(Debug, Default)]
pub struct ModelsRefreshResult {
	pub aborted: bool,
	pub errors: IndexMap<String, JsError>,
}

// ============================================================================
// Auth interaction surface (auth/types.ts, as consumed by Models)
// ============================================================================

/// TS `AuthResolutionOverrides` (auth/resolve.ts l19).
pub struct AuthResolutionOverrides<'a> {
	pub api_key: Option<String>,
	pub env: Option<ProviderEnv>,
	/// Require this much remaining OAuth-token validity (default 5 min).
	pub min_oauth_validity_ms: Option<f64>,
	pub signal: Option<&'a AbortSignal>,
}

/// TS `AuthPrompt` — placeholder JSON form until the typed auth-ui port.
pub type AuthPrompt = serde_json::Value;
/// TS `AuthEvent` — placeholder JSON form.
pub type AuthEvent = serde_json::Value;

/// TS `AuthInteraction` — app-supplied prompts/notifications for login
/// flows (G8: sync).
pub trait AuthInteraction {
	fn prompt(&mut self, prompt: &AuthPrompt) -> Result<String, JsError>;
	fn notify(&mut self, event: &AuthEvent);
}

/// TS `LoginOptions`.
#[derive(Default)]
pub struct LoginOptions {
	/// Stable app-installation id (created on first use).
	pub get_device_id: Option<Box<dyn Fn() -> Result<String, JsError> + Send + Sync>>,
	/// Name the app introduces itself with during login.
	pub agent_name: Option<String>,
}

// ============================================================================
// Models / MutableModels (models.ts l255/l366)
// ============================================================================

/// Runtime collection of providers plus auth application and request
/// convenience (G8: sync). Read accessors return owned `Vec`s of clones —
/// TS hands out frozen references; the Rust collections keep providers
/// behind the trait so clones are bounded by list size.
///
/// `getAuth` is split by overload into `get_auth_for_provider` /
/// `get_auth_for_model`. Error channels (`Result<_, JsError>`) stand in
/// for the TS `ModelsError` rejections (code "oauth"/"auth"); the typed
/// error code surface lands with the auth wave.
pub trait Models {
	fn get_providers(&self) -> Vec<&Provider>;
	fn get_provider(&self, id: &str) -> Option<&Provider>;

	/// Sync read of last-known chat models from one provider or all.
	fn get_models(&self, provider: Option<&str>) -> Vec<Model>;

	/// Sync runtime chat model lookup against last-known lists.
	fn get_model(&self, provider: &str, id: &str) -> Option<Model>;

	/// Sync read of last-known models of one type from one/all providers.
	fn get_models_of_type(&self, ty: &ModelType, provider: Option<&str>) -> Vec<AnyModel>;

	/// Sync runtime lookup of a model of one type.
	fn get_model_of_type(&self, ty: &ModelType, provider: &str, id: &str) -> Option<AnyModel>;

	/// Sync read of last-known models of every type.
	fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel>;

	/// Refresh selected configured dynamic providers. Provider errors and
	/// cancellation are returned in the result, never rejected.
	fn refresh(&mut self, options: Option<&ModelsRefreshOptions<'_>>) -> ModelsRefreshResult;

	/// Whether a provider has complete auth configuration, without
	/// refreshing OAuth.
	fn check_auth(
		&self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Option<AuthCheck>, JsError>;

	/// Chat models whose providers have complete auth configuration.
	fn get_available(
		&self,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<Model>, JsError>;

	/// Models of one type whose providers have complete auth.
	fn get_available_of_type(
		&self,
		ty: &ModelType,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<AnyModel>, JsError>;

	/// Models of every type whose providers have complete auth.
	fn get_all_available(
		&self,
		provider_id: Option<&str>,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<Vec<AnyModel>, JsError>;

	/// Resolve provider-scoped auth by provider id (TS `getAuth`
	/// overload). `Ok(None)` when the provider is unknown/unconfigured.
	fn get_auth_for_provider(
		&self,
		provider_id: &str,
		overrides: Option<&AuthResolutionOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError>;

	/// Resolve provider auth plus static model headers (TS `getAuth`
	/// overload taking a model).
	fn get_auth_for_model(
		&self,
		model: &AnyModel,
		overrides: Option<&AuthResolutionOverrides<'_>>,
	) -> Result<Option<AuthResult>, JsError>;

	/// Run a provider-owned login flow and persist its credential.
	fn login(
		&mut self,
		provider_id: &str,
		ty: AuthType,
		interaction: &mut dyn AuthInteraction,
		options: Option<&LoginOptions>,
	) -> Result<Credential, JsError>;

	/// Remove the stored credential for a provider.
	fn logout(
		&mut self,
		provider_id: &str,
		options: Option<&AuthOperationOptions<'_>>,
	) -> Result<(), JsError>;

	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&StreamOptions>,
	) -> AssistantMessageEventStream;

	fn complete(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&StreamOptions>,
	) -> Result<AssistantMessage, JsError>;

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&SimpleStreamOptions>,
	) -> AssistantMessageEventStream;

	fn complete_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<&SimpleStreamOptions>,
	) -> Result<AssistantMessage, JsError>;

	fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredFetchOptions>,
	) -> AssistantMessageEventStream;

	fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredFetchOptions>,
	) -> Result<AssistantMessage, JsError>;

	fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<&DeferredCancelOptions>,
	) -> Result<(), JsError>;

	/// Generate images through the owning provider with auth resolved like
	/// `stream()`. Never rejects: unknown providers, unconfigured auth, and
	/// providers without the operation return an error result value.
	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<&ImagesOptions>,
	) -> AssistantImages;

	/// Classify structured state through the owning provider. Never
	/// rejects.
	fn classify(
		&self,
		model: &crate::pi_ai::model::ClassifierModel,
		context: &ClassifierContext,
		options: Option<&ClassifierOptions>,
	) -> ClassifierResult;
}

/// `MutableModels` — upsert/delete/clear by provider id.
pub trait MutableModels: Models {
	/// Upsert/replace by `provider.id` (ids are unique).
	fn set_provider(&mut self, provider: Provider);
	fn delete_provider(&mut self, id: &str);
	fn clear_providers(&mut self);
}

// ============================================================================
// Tests — expectations verified against the TS dist with node
// (vectors in docs/plans/impl-notes-providers-foundation.md)
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn chat_model(id: &str, over: serde_json::Value) -> Model {
		let base = json!({
			"id": id, "name": id, "api": "a", "provider": "p",
			"baseUrl": "https://x", "input": ["text"],
			"cost": {"input": 1, "output": 2, "cacheRead": 0.5, "cacheWrite": 1.5},
			"reasoning": false, "contextWindow": 128000, "maxTokens": 16384
		});
		let merged = match (base.as_object().cloned(), over.as_object().cloned()) {
			(Some(mut b), Some(o)) => {
				for (k, v) in o {
					b.insert(k, v);
				}
				serde_json::Value::Object(b)
			}
			_ => base,
		};
		serde_json::from_value(merged).unwrap()
	}

	fn reasoning_model(map: serde_json::Value) -> Model {
		chat_model(
			"m",
			json!({"reasoning": true, "thinkingLevelMap": map}),
		)
	}

	fn usage(v: serde_json::Value) -> Usage {
		serde_json::from_value(v).unwrap()
	}

	/// Parse a V8 `String(number)` form back to the exact f64 (both sides
	/// produce the nearest double, so equality is bit-exact).
	fn fx(s: &str) -> f64 {
		s.parse::<f64>().unwrap()
	}

	/// Node-verified vector runner for calculateCost.
	fn check_cost(c: &UsageCost, expected: [&str; 5]) {
		assert_eq!(c.input, fx(expected[0]), "input");
		assert_eq!(c.output, fx(expected[1]), "output");
		assert_eq!(c.cache_read, fx(expected[2]), "cacheRead");
		assert_eq!(c.cache_write, fx(expected[3]), "cacheWrite");
		assert_eq!(c.total, fx(expected[4]), "total");
	}

	// -- calculateCost: all vectors node-verified (f64 bit-identical; the
	//    expected strings below are the V8 `String(number)` forms, which
	//    parse back to the same f64s). --
	#[test]
	fn calculate_cost_base_rates() {
		let mut u = usage(json!({"input": 1000, "output": 500, "cacheRead": 200, "cacheWrite": 100, "totalTokens": 1800}));
		calculate_cost(&AnyModel::Chat(chat_model("m", json!({}))), &mut u);
		check_cost(&u.cost, ["0.001", "0.001", "0.00009999999999999999", "0.00015", "0.00225"]);
	}

	#[test]
	fn calculate_cost_tier_selected() {
		let mut u = usage(json!({"input": 1000, "output": 500, "cacheRead": 200, "cacheWrite": 100}));
		let m = chat_model("m", json!({"cost": {"input": 1, "output": 2, "cacheRead": 0.5, "cacheWrite": 1.5, "tiers": [{"inputTokensAbove": 500, "input": 3, "output": 6, "cacheRead": 1, "cacheWrite": 2}]}}));
		calculate_cost(&AnyModel::Chat(m), &mut u);
		check_cost(&u.cost, ["0.003", "0.003", "0.00019999999999999998", "0.0002", "0.0063999999999999994"]);
	}

	#[test]
	fn calculate_cost_tier_boundary_is_strict() {
		// inputTokens = 1300, threshold 1300: strictly-above fails → base.
		let mut u = usage(json!({"input": 1000, "output": 500, "cacheRead": 200, "cacheWrite": 100}));
		let m = chat_model("m", json!({"cost": {"input": 1, "output": 2, "cacheRead": 0.5, "cacheWrite": 1.5, "tiers": [{"inputTokensAbove": 1300, "input": 3, "output": 6, "cacheRead": 1, "cacheWrite": 2}]}}));
		calculate_cost(&AnyModel::Chat(m), &mut u);
		check_cost(&u.cost, ["0.001", "0.001", "0.00009999999999999999", "0.00015", "0.00225"]);
	}

	#[test]
	fn calculate_cost_highest_matching_tier_wins() {
		// 500 tokens over thresholds 100 and 200 → the 200 tier wins.
		let mut u = usage(json!({"input": 500, "output": 0, "cacheRead": 0, "cacheWrite": 0}));
		let m = chat_model("m", json!({"cost": {"input": 1, "output": 2, "cacheRead": 0.5, "cacheWrite": 1.5, "tiers": [{"inputTokensAbove": 100, "input": 10, "output": 20, "cacheRead": 5, "cacheWrite": 15}, {"inputTokensAbove": 200, "input": 100, "output": 200, "cacheRead": 50, "cacheWrite": 150}]}}));
		calculate_cost(&AnyModel::Chat(m), &mut u);
		check_cost(&u.cost, ["0.05", "0", "0", "0", "0.05"]);
	}

	#[test]
	fn calculate_cost_zero_rates() {
		let mut u = usage(json!({"input": 100, "output": 100, "cacheRead": 100, "cacheWrite": 100}));
		let m = chat_model("m", json!({"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "tiers": [{"inputTokensAbove": 0, "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}]}}));
		calculate_cost(&AnyModel::Chat(m), &mut u);
		check_cost(&u.cost, ["0", "0", "0", "0", "0"]);
	}

	#[test]
	fn calculate_cost_1h_cache_write_double_charge() {
		// cacheWrite 100 with 40 written for 1h: cache_write cost uses
		// 60 * 1.5 + 40 * 2 * 1 (base input) per million.
		let mut u = usage(json!({"input": 1000, "output": 500, "cacheRead": 200, "cacheWrite": 100, "cacheWrite1h": 40}));
		calculate_cost(&AnyModel::Chat(chat_model("m", json!({}))), &mut u);
		check_cost(&u.cost, ["0.001", "0.001", "0.00009999999999999999", "0.00017", "0.00227"]);
	}

	#[test]
	fn calculate_cost_all_1h_writes() {
		let mut u = usage(json!({"input": 10, "output": 10, "cacheRead": 10, "cacheWrite": 50, "cacheWrite1h": 50}));
		calculate_cost(&AnyModel::Chat(chat_model("m", json!({}))), &mut u);
		check_cost(&u.cost, ["0.000009999999999999999", "0.000019999999999999998", "0.0000049999999999999996", "0.0001", "0.000135"]);
	}

	#[test]
	fn calculate_cost_fractional() {
		let mut u = usage(json!({"input": 0.5, "output": 0.25, "cacheRead": 0.125, "cacheWrite": 0.0625}));
		calculate_cost(&AnyModel::Chat(chat_model("m", json!({}))), &mut u);
		check_cost(&u.cost, ["5e-7", "5e-7", "6.25e-8", "9.375e-8", "0.00000115625"]);
	}

	// -- thinking levels: node-verified vectors --
	#[test]
	fn levels_no_reasoning() {
		let m = chat_model("m", json!({}));
		assert_eq!(get_supported_thinking_levels(&m), vec![ModelThinkingLevel::Off]);
	}

	fn levels(map: serde_json::Value) -> Vec<ModelThinkingLevel> {
		get_supported_thinking_levels(&reasoning_model(map))
	}

	use ModelThinkingLevel as L;

	#[test]
	fn levels_map_semantics() {
		// no map / empty map → the five base levels
		assert_eq!(levels(json!({})), vec![L::Off, L::Minimal, L::Low, L::Medium, L::High]);
		// null excludes
		assert_eq!(levels(json!({"off": null})), vec![L::Minimal, L::Low, L::Medium, L::High]);
		assert_eq!(levels(json!({"medium": null})), vec![L::Off, L::Minimal, L::Low, L::High]);
		// full map → all seven
		assert_eq!(
			levels(json!({"off": "off", "minimal": "low", "low": "low", "medium": "medium", "high": "high", "xhigh": "high", "max": "high"})),
			vec![L::Off, L::Minimal, L::Low, L::Medium, L::High, L::Xhigh, L::Max]
		);
		// xhigh set, high null
		assert_eq!(
			levels(json!({"high": null, "xhigh": "high"})),
			vec![L::Off, L::Minimal, L::Low, L::Medium, L::Xhigh]
		);
		// only max set → base five + max
		assert_eq!(
			levels(json!({"max": "max"})),
			vec![L::Off, L::Minimal, L::Low, L::Medium, L::High, L::Max]
		);
	}

	#[test]
	fn clamp_full_map_all_levels_pass() {
		let m = reasoning_model(json!({"off": "off", "minimal": "low", "low": "low", "medium": "medium", "high": "high", "xhigh": "high", "max": "high"}));
		for level in EXTENDED_THINKING_LEVELS {
			assert_eq!(clamp_thinking_level(&m, level), level);
		}
	}

	#[test]
	fn clamp_xhigh_max_fall_back_to_high() {
		// map without xhigh/max → xhigh/max clamp down to high
		let m = reasoning_model(json!({}));
		for level in [L::Xhigh, L::Max] {
			assert_eq!(clamp_thinking_level(&m, level), L::High);
		}
	}

	#[test]
	fn clamp_no_reasoning_clamps_to_off() {
		let m = chat_model("m", json!({}));
		for level in [L::Off, L::High, L::Max] {
			assert_eq!(clamp_thinking_level(&m, level), L::Off);
		}
	}

	// -- model type operations --
	#[test]
	fn model_type_operations() {
		let chat = AnyModel::Chat(chat_model("m", json!({})));
		let typed_chat = AnyModel::Chat(chat_model("m", json!({"type": "chat"})));
		let image = AnyModel::Image(serde_json::from_value(json!({
			"id": "i", "name": "i", "api": "a", "provider": "p", "baseUrl": "",
			"input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"type": "image", "output": ["image"]
		})).unwrap());

		assert_eq!(get_model_type(&chat), ModelType::Chat);
		assert_eq!(get_model_type(&typed_chat), ModelType::Chat);
		assert_eq!(get_model_type(&image), ModelType::Image);
		assert!(is_model_type(&chat, &ModelType::Chat));
		assert!(!is_model_type(&image, &ModelType::Chat));

		// hasApi: chat + api match; image never matches even with same api.
		assert!(has_api(&chat, "a"));
		assert!(!has_api(&chat, "other"));
		assert!(!has_api(&image, "a"));

		// modelsAreEqual: type + id + provider.
		let other_provider = AnyModel::Chat(chat_model("m", json!({"provider": "q"})));
		assert!(models_are_equal(Some(&chat), Some(&typed_chat)));
		assert!(!models_are_equal(Some(&chat), Some(&image)));
		assert!(!models_are_equal(Some(&chat), Some(&other_provider)));
		assert!(!models_are_equal(Some(&chat), None));
		assert!(!models_are_equal(None, Some(&chat)));
		assert!(!models_are_equal(None, None));

		// Unknown type strings narrow to Unknown and match nothing.
		let weird = AnyModel::Chat(chat_model("m", json!({"type": "future"})));
		assert_eq!(get_model_type(&weird), ModelType::Unknown("future".to_string()));
		assert!(!is_model_type(&weird, &ModelType::Chat));
		assert!(!has_api(&weird, "a"));
	}
}
