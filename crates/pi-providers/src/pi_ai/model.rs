//! pi-ai model catalog types.
//!
//! Port of `packages/ai/src/types.ts` (model types, l~700-898) and
//! `packages/ai/src/providers/model-schema.ts` (thinking levels, cost,
//! input limits).
//!
//! ## Round-trip design
//!
//! TS model objects cross JSON constantly (catalog fixtures, models.json,
//! parity harness output) and their **key order is observable** (file writes
//! with `JSON.stringify(x, null, 2)` compare byte-for-byte in the parity
//! harness). The generated catalog data uses 57 distinct key orders, so a
//! plain `#[derive(Serialize)]` struct cannot round-trip losslessly. Instead
//! every model struct records the original key order and custom
//! (de)serialization walks it:
//!
//! - known fields are typed; unknown fields land in `extra` (ordered map);
//! - serialization emits keys in the original order, so
//!   `parse → serialize` is lossless (JS number semantics, key order, extra
//!   fields);
//! - `set(key, value)` mirrors TS `{...model, key: value}`: existing keys keep
//!   their position, new keys are appended;
//! - a known field set to `None`/absent is **dropped** on serialization,
//!   matching `JSON.stringify` dropping `undefined`.
//!
//! Known limitation: a JSON `null` on an optional typed field is treated as
//! absent (TS `undefined`-like) rather than preserved; the TS schemas never
//! produce `null` for these fields. Numbers are JS doubles: integral values
//! print without a decimal point; integral magnitudes above 2^53 serialize in
//! ryu shortest form, which differs from V8's plain-digit form only above
//! ~9e15..1e21 (catalog data never reaches this).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};


// ============================================================================
// Open string unions (TS `KnownApi | (string & {})`)
// ============================================================================

/// `KnownApi` from `types.ts`. Open union: `Api` accepts any string.
pub const KNOWN_APIS: &[&str] = &[
	"openai-completions",
	"mistral-conversations",
	"openai-responses",
	"azure-openai-responses",
	"openai-codex-responses",
	"anthropic-messages",
	"bedrock-converse-stream",
	"google-generative-ai",
	"google-vertex",
	"pi-messages",
];

/// `Api` — open union, plain `String` (TS `KnownApi | (string & {})`).
pub type Api = String;

pub const KNOWN_IMAGE_APIS: &[&str] = &["openrouter-images"];
/// `ImageApi` — open union.
pub type ImageApi = String;

pub const KNOWN_CLASSIFIER_APIS: &[&str] = &[
	"typesafe-system-one",
	"cloudflare-workers-ai-system-one",
	"llama-cpp-classify",
	"openai-decisions",
];
/// `ClassifierApi` — open union.
pub type ClassifierApi = String;

/// `KnownProvider` from `types.ts` (insertion order preserved).
pub const KNOWN_PROVIDERS: &[&str] = &[
	"amazon-bedrock",
	"ant-ling",
	"anthropic",
	"google",
	"google-vertex",
	"openai",
	"azure",
	"openai-codex",
	"radius",
	"typesafe",
	"nvidia",
	"deepseek",
	"github-copilot",
	"xai",
	"groq",
	"cerebras",
	"openrouter",
	"vercel-ai-gateway",
	"zai",
	"zai-coding-cn",
	"mistral",
	"minimax",
	"minimax-cn",
	"moonshotai",
	"moonshotai-cn",
	"huggingface",
	"fireworks",
	"together",
	"baseten",
	"opencode",
	"opencode-go",
	"kimi-coding",
	"meta",
	"cloudflare-workers-ai",
	"cloudflare-ai-gateway",
	"qwen-token-plan",
	"qwen-token-plan-cn",
	"qwen-token-plan-individual",
	"xiaomi",
	"xiaomi-token-plan-cn",
	"xiaomi-token-plan-ams",
	"xiaomi-token-plan-sgp",
];

/// `ProviderId` — open union (`KnownProvider | string`).
pub type ProviderId = String;

// ============================================================================
// Provider env / headers
// ============================================================================

/// `ProviderEnv` — provider-scoped environment overrides, insertion-ordered.
pub type ProviderEnv = IndexMap<String, String>;

/// `ProviderHeaders = Record<string, string | null>` — ordered. A `None`
/// (JSON `null`) value suppresses a provider/API default header with the same
/// name; absent key means no override.
pub type ProviderHeaders = IndexMap<String, Option<String>>;

// ============================================================================
// Thinking levels
// ============================================================================

/// `THINKING_LEVELS` from `providers/model-schema.ts`.
pub const THINKING_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];

/// `MODEL_THINKING_LEVELS` from `providers/model-schema.ts`.
pub const MODEL_THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `ModelThinkingLevel` — `"off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelThinkingLevel {
	Off,
	Minimal,
	Low,
	Medium,
	High,
	Xhigh,
	Max,
}

impl ModelThinkingLevel {
	pub fn as_str(self) -> &'static str {
		match self {
			ModelThinkingLevel::Off => "off",
			ModelThinkingLevel::Minimal => "minimal",
			ModelThinkingLevel::Low => "low",
			ModelThinkingLevel::Medium => "medium",
			ModelThinkingLevel::High => "high",
			ModelThinkingLevel::Xhigh => "xhigh",
			ModelThinkingLevel::Max => "max",
		}
	}

	/// Parse a level string; unknown strings yield `None` (TS union has no
	/// fallback).
	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"off" => Some(ModelThinkingLevel::Off),
			"minimal" => Some(ModelThinkingLevel::Minimal),
			"low" => Some(ModelThinkingLevel::Low),
			"medium" => Some(ModelThinkingLevel::Medium),
			"high" => Some(ModelThinkingLevel::High),
			"xhigh" => Some(ModelThinkingLevel::Xhigh),
			"max" => Some(ModelThinkingLevel::Max),
			_ => None,
		}
	}

	/// Index into `MODEL_THINKING_LEVELS` order.
	pub fn index(self) -> usize {
		match self {
			ModelThinkingLevel::Off => 0,
			ModelThinkingLevel::Minimal => 1,
			ModelThinkingLevel::Low => 2,
			ModelThinkingLevel::Medium => 3,
			ModelThinkingLevel::High => 4,
			ModelThinkingLevel::Xhigh => 5,
			ModelThinkingLevel::Max => 6,
		}
	}
}

/// `ThinkingLevel` — like `ModelThinkingLevel` without `"off"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThinkingLevel {
	Minimal,
	Low,
	Medium,
	High,
	Xhigh,
	Max,
}

impl ThinkingLevel {
	pub fn as_str(self) -> &'static str {
		match self {
			ThinkingLevel::Minimal => "minimal",
			ThinkingLevel::Low => "low",
			ThinkingLevel::Medium => "medium",
			ThinkingLevel::High => "high",
			ThinkingLevel::Xhigh => "xhigh",
			ThinkingLevel::Max => "max",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"minimal" => Some(ThinkingLevel::Minimal),
			"low" => Some(ThinkingLevel::Low),
			"medium" => Some(ThinkingLevel::Medium),
			"high" => Some(ThinkingLevel::High),
			"xhigh" => Some(ThinkingLevel::Xhigh),
			"max" => Some(ThinkingLevel::Max),
			_ => None,
		}
	}
}

// ============================================================================
// JS number
// ============================================================================

/// A JSON number with JS semantics: always an f64 (like `JSON.parse` output),
/// serialized so integral values print without a decimal point
/// (`JSON.stringify(3.0)` → `3`).
///
/// Integral magnitudes ≤ 2^53 serialize exactly; larger integral values fall
/// back to ryu shortest form (V8 prints plain digits up to 1e21 — a divergence
/// only for magnitudes above ~9e15, which catalog/model data never reaches).
/// Non-finite values serialize as `null`, like `JSON.stringify(NaN)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct JsNumber(pub f64);

impl JsNumber {
	pub fn new(v: f64) -> Self {
		JsNumber(v)
	}

	pub fn get(self) -> f64 {
		self.0
	}

	/// JSON value form, honoring the JS number semantics of `Serialize`
	/// (integral → plain number, non-finite → null).
	pub fn into_value(self) -> Value {
		serde_json::to_value(self).expect("JsNumber serializes to a JSON value")
	}
}

impl From<f64> for JsNumber {
	fn from(v: f64) -> Self {
		JsNumber(v)
	}
}

impl From<i64> for JsNumber {
	fn from(v: i64) -> Self {
		JsNumber(v as f64)
	}
}

impl From<u64> for JsNumber {
	fn from(v: u64) -> Self {
		JsNumber(v as f64)
	}
}

impl Serialize for JsNumber {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		let v = self.0;
		if !v.is_finite() {
			// JSON.stringify(NaN / Infinity) → null
			return serializer.serialize_unit();
		}
		if v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_992.0 {
			if v >= 0.0 {
				return serializer.serialize_u64(v as u64);
			}
			return serializer.serialize_i64(v as i64);
		}
		serializer.serialize_f64(v)
	}
}

impl<'de> Deserialize<'de> for JsNumber {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		struct V;
		impl<'v> serde::de::Visitor<'v> for V {
			type Value = JsNumber;
			fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("a JSON number")
			}
			fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<JsNumber, E> {
				Ok(JsNumber(v))
			}
			fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<JsNumber, E> {
				Ok(JsNumber(v as f64))
			}
			fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<JsNumber, E> {
				Ok(JsNumber(v as f64))
			}
			// JSON `null` coerces to 0 like JS `null` arithmetic.
			fn visit_unit<E: serde::de::Error>(self) -> Result<JsNumber, E> {
				Ok(JsNumber(0.0))
			}
		}
		deserializer.deserialize_any(V)
	}
}

// ============================================================================
// Input modalities
// ============================================================================

/// `ModelInputModality` — `"text" | "image"`. Also used for image model
/// `output` ("text" | "image").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelInputModality {
	#[serde(rename = "text")]
	Text,
	#[serde(rename = "image")]
	Image,
}

// ============================================================================
// Order-preserving struct machinery
// ============================================================================
// `js_object!` and friends live in `macros.rs` (shared with `credential.rs`).


// ============================================================================
// Cost / limits (providers/model-schema.ts)
// ============================================================================

js_object! {
	/// `ModelCostTier` — request-wide pricing tier.
	pub struct ModelCostTier {
		input_tokens_above req "inputTokensAbove" : JsNumber,
		input req "input" : JsNumber,
		output req "output" : JsNumber,
		cache_read req "cacheRead" : JsNumber,
		cache_write req "cacheWrite" : JsNumber,
	}
}

js_object! {
	/// `ModelCost` — USD per million tokens, plus optional request-wide tiers.
	pub struct ModelCost {
		input req "input" : JsNumber,
		output req "output" : JsNumber,
		cache_read req "cacheRead" : JsNumber,
		cache_write req "cacheWrite" : JsNumber,
		tiers opt "tiers" : Option<Vec<ModelCostTier>>,
	}
}

js_object! {
	/// `ModelImageResizeOptions`.
	pub struct ModelImageResizeOptions {
		max_width opt "maxWidth" : Option<JsNumber>,
		max_height opt "maxHeight" : Option<JsNumber>,
		max_bytes opt "maxBytes" : Option<JsNumber>,
		jpeg_quality opt "jpegQuality" : Option<JsNumber>,
	}
}

js_object! {
	/// `ModelImageInputLimits`.
	pub struct ModelImageInputLimits {
		resize opt "resize" : Option<ModelImageResizeOptions>,
		max_per_message opt "maxPerMessage" : Option<JsNumber>,
		max_per_request opt "maxPerRequest" : Option<JsNumber>,
	}
}

js_object! {
	/// `ModelInputLimits`.
	pub struct ModelInputLimits {
		max_request_bytes opt "maxRequestBytes" : Option<JsNumber>,
		images opt "images" : Option<ModelImageInputLimits>,
	}
}

/// `ThinkingBudgets` — token budgets per thinking level (token-based
/// providers only). Option values, not order-sensitive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgets {
	pub minimal: Option<JsNumber>,
	pub low: Option<JsNumber>,
	pub medium: Option<JsNumber>,
	pub high: Option<JsNumber>,
}

/// `ThinkingLevelMap` — pi thinking level → provider value. `null` marks a
/// level as unsupported. Unknown level keys are preserved.
pub type ThinkingLevelMap = IndexMap<String, Option<String>>;

/// `ModelPromptCache` — best-effort cache lifetime in seconds per retention
/// tier ("short"/"long").
pub type ModelPromptCache = IndexMap<String, JsNumber>;

/// `SamplingParams = Record<string, unknown>` — ordered.
pub type SamplingParams = Map<String, Value>;

/// `SamplingParamsByThinkingLevel` — per-level overrides. Ordered; unknown
/// level keys preserved.
pub type SamplingParamsByThinkingLevel = IndexMap<String, SamplingParams>;

// ============================================================================
// Model structs (types.ts l~700-898)
// ============================================================================

js_object! {
	/// `Model<TApi>` — chat model. Models without `type` are chat models;
	/// narrow mixed lists with `is_model_type` instead of comparing `type`.
	pub struct Model {
		id req "id" : String,
		name req "name" : String,
		api req "api" : String,
		provider req "provider" : String,
		base_url req "baseUrl" : String,
		input req "input" : Vec<ModelInputModality>,
		input_limits opt "inputLimits" : Option<ModelInputLimits>,
		cost req "cost" : ModelCost,
		headers opt "headers" : Option<ProviderEnv>,
		model_type opt "type" : Option<String>,
		reasoning req "reasoning" : bool,
		thinking_level_map opt "thinkingLevelMap" : Option<ThinkingLevelMap>,
		prompt_cache opt "promptCache" : Option<ModelPromptCache>,
		context_window req "contextWindow" : JsNumber,
		max_tokens req "maxTokens" : JsNumber,
		sampling_params opt "samplingParams" : Option<SamplingParams>,
		sampling_params_by_thinking_level opt "samplingParamsByThinkingLevel" : Option<SamplingParamsByThinkingLevel>,
		compat opt "compat" : Option<Value>,
	}
}

js_object! {
	/// `ImageModel<TApi>` — image-generation model (usable with
	/// `generate_images` only).
	pub struct ImageModel {
		id req "id" : String,
		name req "name" : String,
		api req "api" : String,
		provider req "provider" : String,
		base_url req "baseUrl" : String,
		input req "input" : Vec<ModelInputModality>,
		input_limits opt "inputLimits" : Option<ModelInputLimits>,
		cost req "cost" : ModelCost,
		headers opt "headers" : Option<ProviderEnv>,
		model_type opt "type" : Option<String>,
		output req "output" : Vec<ModelInputModality>,
	}
}

js_object! {
	/// `ClassifierModel<TApi>` — structured classifier model (usable with
	/// `classify` only).
	pub struct ClassifierModel {
		id req "id" : String,
		name req "name" : String,
		api req "api" : String,
		provider req "provider" : String,
		base_url req "baseUrl" : String,
		input req "input" : Vec<ModelInputModality>,
		input_limits opt "inputLimits" : Option<ModelInputLimits>,
		cost req "cost" : ModelCost,
		headers opt "headers" : Option<ProviderEnv>,
		model_type opt "type" : Option<String>,
		context_window req "contextWindow" : JsNumber,
	}
}

// ============================================================================
// ModelType / AnyModel
// ============================================================================

/// `ModelType` — `"chat" | "image" | "classifier"`. Data from stores and
/// remote sources may carry types only newer versions know; those surface as
/// `Unknown` (TS keeps the raw string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelType {
	Chat,
	Image,
	Classifier,
	Unknown(String),
}

impl ModelType {
	pub fn as_str(&self) -> &str {
		match self {
			ModelType::Chat => "chat",
			ModelType::Image => "image",
			ModelType::Classifier => "classifier",
			ModelType::Unknown(s) => s,
		}
	}
}

impl Serialize for ModelType {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(self.as_str())
	}
}

impl<'de> Deserialize<'de> for ModelType {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let s = String::deserialize(deserializer)?;
		Ok(match s.as_str() {
			"chat" => ModelType::Chat,
			"image" => ModelType::Image,
			"classifier" => ModelType::Classifier,
			_ => ModelType::Unknown(s),
		})
	}
}

/// `AnyModel` — anything a provider can list. Dispatch on the `type` field:
/// `"image"` → image, `"classifier"` → classifier, anything else (including
/// absent) → chat.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // faithful to the TS union; models are cloned wholesale
pub enum AnyModel {
	Chat(Model),
	Image(ImageModel),
	Classifier(ClassifierModel),
}

impl AnyModel {
	pub fn id(&self) -> &str {
		match self {
			AnyModel::Chat(m) => &m.id,
			AnyModel::Image(m) => &m.id,
			AnyModel::Classifier(m) => &m.id,
		}
	}

	pub fn provider(&self) -> &str {
		match self {
			AnyModel::Chat(m) => &m.provider,
			AnyModel::Image(m) => &m.provider,
			AnyModel::Classifier(m) => &m.provider,
		}
	}

	pub fn api(&self) -> &str {
		match self {
			AnyModel::Chat(m) => &m.api,
			AnyModel::Image(m) => &m.api,
			AnyModel::Classifier(m) => &m.api,
		}
	}

	pub fn name(&self) -> &str {
		match self {
			AnyModel::Chat(m) => &m.name,
			AnyModel::Image(m) => &m.name,
			AnyModel::Classifier(m) => &m.name,
		}
	}

	pub fn cost(&self) -> &ModelCost {
		match self {
			AnyModel::Chat(m) => &m.cost,
			AnyModel::Image(m) => &m.cost,
			AnyModel::Classifier(m) => &m.cost,
		}
	}

	/// The raw `type` field (`None` when absent — chat models may omit it).
	pub fn type_field(&self) -> Option<&str> {
		match self {
			AnyModel::Chat(m) => m.model_type.as_deref(),
			AnyModel::Image(m) => m.model_type.as_deref(),
			AnyModel::Classifier(m) => m.model_type.as_deref(),
		}
	}

	pub fn to_value(&self) -> Value {
		match self {
			AnyModel::Chat(m) => m.to_value(),
			AnyModel::Image(m) => m.to_value(),
			AnyModel::Classifier(m) => m.to_value(),
		}
	}
}

impl Serialize for AnyModel {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			AnyModel::Chat(m) => m.serialize(serializer),
			AnyModel::Image(m) => m.serialize(serializer),
			AnyModel::Classifier(m) => m.serialize(serializer),
		}
	}
}

impl<'de> Deserialize<'de> for AnyModel {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let map = Map::<String, Value>::deserialize(deserializer)?;
		match map.get("type").and_then(Value::as_str) {
			Some("image") => ImageModel::from_map(map).map(AnyModel::Image).map_err(serde::de::Error::custom),
			Some("classifier") => {
				ClassifierModel::from_map(map).map(AnyModel::Classifier).map_err(serde::de::Error::custom)
			}
			_ => Model::from_map(map).map(AnyModel::Chat).map_err(serde::de::Error::custom),
		}
	}
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::js::js_number_to_string;
	use serde_json::json;

	fn model_json() -> Value {
		json!({
			"id": "m1",
			"name": "Model One",
			"api": "anthropic-messages",
			"provider": "anthropic",
			"baseUrl": "https://api.anthropic.com",
			"reasoning": true,
			"input": ["text", "image"],
			"cost": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75},
			"contextWindow": 200000,
			"maxTokens": 16384,
			"thinkingLevelMap": {"off": null, "low": "low", "high": "high"},
			"customField": {"kept": true},
		})
	}

	#[test]
	fn round_trips_with_key_order_and_extras() {
		// Deliberately unusual key order with extras interleaved.
		let text = r#"{"maxTokens":10,"id":"x","weird":1,"name":"n","api":"a","provider":"p","baseUrl":"u","input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"reasoning":false,"contextWindow":100}"#;
		let model: Model = serde_json::from_str(text).unwrap();
		let out = serde_json::to_string(&model).unwrap();
		assert_eq!(
			out,
			r#"{"maxTokens":10,"id":"x","weird":1,"name":"n","api":"a","provider":"p","baseUrl":"u","input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"reasoning":false,"contextWindow":100}"#
		);
		assert_eq!(model.extra.get("weird"), Some(&json!(1)));
	}

	#[test]
	fn integral_numbers_print_like_json_stringify() {
		let model: Model = serde_json::from_value(model_json()).unwrap();
		let out = model.to_value();
		// 3 → "3" not "3.0"; 0.3 → "0.3".
		assert_eq!(out["cost"]["input"], json!(3));
		assert_eq!(out["cost"]["cacheRead"], json!(0.3));
		assert_eq!(out["contextWindow"], json!(200000));
		assert_eq!(serde_json::to_string(&out["cost"]["input"]).unwrap(), "3");
	}

	#[test]
	fn set_keeps_position_for_existing_keys_and_appends_new() {
		let mut model: Model = serde_json::from_value(model_json()).unwrap();
		model.set("name", json!("Renamed")).unwrap();
		model.set("samplingParams", json!({"top_p": 0.9})).unwrap();
		let keys: Vec<String> = model.ordered_map().keys().cloned().collect();
		assert_eq!(keys[1], "name"); // still second
		assert_eq!(keys[keys.len() - 1], "samplingParams"); // appended at end
		assert_eq!(model.sampling_params.as_ref().unwrap().get("top_p"), Some(&json!(0.9)));
	}

	#[test]
	fn missing_required_field_errors_with_serde_wording() {
		let err = serde_json::from_value::<Model>(json!({"id": "x"})).unwrap_err();
		assert_eq!(err.to_string(), "missing field `name`");
	}

	#[test]
	fn any_model_dispatches_on_type() {
		let chat: AnyModel = serde_json::from_value(model_json()).unwrap();
		assert!(matches!(chat, AnyModel::Chat(_)));
		let image: AnyModel = serde_json::from_value(json!({
			"type": "image", "id": "i", "name": "i", "api": "openrouter-images", "provider": "openrouter",
			"baseUrl": "", "input": ["text"], "cost": {"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
			"output": ["image"]
		})).unwrap();
		assert!(matches!(image, AnyModel::Image(_)));
		let classifier: AnyModel = serde_json::from_value(json!({
			"type": "classifier", "id": "c", "name": "c", "api": "openai-decisions", "provider": "openai",
			"baseUrl": "", "input": ["text"], "cost": {"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
			"contextWindow": 1000
		})).unwrap();
		assert!(matches!(classifier, AnyModel::Classifier(_)));
	}

	#[test]
	fn nested_cost_tiers_round_trip() {
		let v = json!({
			"id": "x", "name": "n", "api": "a", "provider": "p", "baseUrl": "u",
			"input": ["text"], "reasoning": false, "contextWindow": 1, "maxTokens": 1,
			"cost": {"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0,
				"tiers": [{"inputTokensAbove": 1000, "input": 0.5, "output": 2, "cacheRead": 0, "cacheWrite": 0}]}
		});
		let model: Model = serde_json::from_value(v.clone()).unwrap();
		assert_eq!(model.to_value(), v);
	}

	#[test]
	fn thinking_level_map_preserves_null_and_order() {
		let model: Model = serde_json::from_value(model_json()).unwrap();
		let map = model.thinking_level_map.as_ref().unwrap();
		let keys: Vec<&String> = map.keys().collect();
		assert_eq!(keys.iter().map(|s| s.as_str()).collect::<Vec<_>>(), vec!["off", "low", "high"]);
		assert_eq!(map.get("off"), Some(&None));
		assert_eq!(map.get("low"), Some(&Some("low".to_string())));
		// null round-trips as null.
		let out = model.to_value();
		assert_eq!(out["thinkingLevelMap"]["off"], Value::Null);
	}

	#[test]
	fn js_number_formatting() {
		assert_eq!(serde_json::to_string(&JsNumber(3.0)).unwrap(), "3");
		assert_eq!(serde_json::to_string(&JsNumber(-3.0)).unwrap(), "-3");
		assert_eq!(serde_json::to_string(&JsNumber(0.5)).unwrap(), "0.5");
		assert_eq!(serde_json::to_string(&JsNumber(-0.0)).unwrap(), "0");
		assert_eq!(serde_json::to_string(&JsNumber(1e15)).unwrap(), "1000000000000000");
		// JS Number::toString for reference formatting.
		assert_eq!(js_number_to_string(JsNumber(1e21).get()), "1e+21");
	}

	#[test]
	fn builtin_catalog_round_trips() {
		// Every entry in the vendored builtin catalog must deserialize into
		// the typed structs and re-serialize to the same ordered JSON.
		let catalog = crate::pi_ai::builtin_catalog();
		let providers = catalog.get("providers").and_then(Value::as_object).expect("providers");
		let mut count = 0;
		for (_prov, apis) in providers {
			for (_api, models) in apis.as_object().expect("apis") {
				for (key, model_value) in models.as_object().expect("models") {
					let model: AnyModel = serde_json::from_value(model_value.clone())
						.unwrap_or_else(|e| panic!("{key}: {e}"));
					let out = model.to_value();
					assert_eq!(
						serde_json::to_string(&out).unwrap(),
						serde_json::to_string(model_value).unwrap(),
						"round-trip mismatch for {key}"
					);
					count += 1;
				}
			}
		}
		assert!(count > 1500, "expected the full catalog, got {count} models");
	}
}
