//! Model reference resolution, scoping, and initial selection.
//!
//! Faithful port of `packages/coding-agent/src/core/model-resolver.ts`
//! (everything except the `modelRuntime`-taking wrappers
//! `resolveModelScopeWithDiagnostics`/`resolveModelScope`, which print to
//! the console and are superseded by the facade wave).
//!
//! JS-semantics notes:
//! - `toLowerCase()` is Rust `str::to_lowercase` (identical for the ASCII
//!   ids the parity catalog uses; JS full lowercase differs only for
//!   non-ASCII edge cases, which the catalog avoids).
//! - `localeCompare` under en-US agrees with codepoint order for every
//!   string in the parity catalog (probed: 400 pairs, 0 divergences);
//!   `js_locale_compare` implements that comparator (see `pi_ai::js`).
//! - `Array.prototype.sort` is stable in V8; Rust `sort_by` is stable.
//! - Set/Map iteration order = insertion order: provider dedup uses
//!   order-preserving Vecs, never HashSet.

use serde_json::{json, Value};

use crate::minimatch::minimatch;
use crate::pi_ai::js::{is_valid_thinking_level, js_locale_compare, js_trim, DEFAULT_THINKING_LEVEL};
use crate::pi_ai::model::Model;

/// `defaultModelPerProvider` — default chat model IDs for providers with
/// built-in chat models. Insertion order is behavior (`Object.keys`
/// iteration in `find_initial_model` / `restore_model_from_session`).
static DEFAULT_MODELS_PER_PROVIDER: &[(&str, &str)] = &[
		("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1"),
		("ant-ling", "Ring-2.6-1T"),
		("anthropic", "claude-opus-4-8"),
		("openai", "gpt-5.5"),
		("azure", "gpt-5.4"),
		("openai-codex", "gpt-6.1-sol"),
		("radius", "balanced"),
		("nvidia", "nvidia/nemotron-3-ultra-550b-a55b"),
		("deepseek", "deepseek-v4-pro"),
		("google", "gemini-3.1-pro-preview"),
		("google-vertex", "gemini-3.1-pro-preview"),
		("github-copilot", "gpt-5.4"),
		("openrouter", "moonshotai/kimi-k2.6"),
		("vercel-ai-gateway", "zai/glm-5.1"),
		("xai", "grok-4.7"),
		("groq", "openai/gpt-oss-120b"),
		("cerebras", "gpt-oss-120b"),
		("zai", "glm-5.3"),
		("zai-coding-cn", "glm-5.3"),
		("mistral", "devstral-medium-latest"),
		("minimax", "MiniMax-M2.7"),
		("minimax-cn", "MiniMax-M2.7"),
		("moonshotai", "kimi-k2.6"),
		("moonshotai-cn", "kimi-k2.6"),
		("huggingface", "moonshotai/Kimi-K2.6"),
		("fireworks", "accounts/fireworks/models/kimi-k3"),
		("together", "moonshotai/Kimi-K3"),
		("baseten", "zai-org/GLM-5.2"),
		("opencode", "kimi-k2.6"),
		("opencode-go", "kimi-k3"),
		("kimi-coding", "kimi-for-coding"),
		("meta", "muse-spark-1.3"),
		("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
		("cloudflare-ai-gateway", "workers-ai/@cf/moonshotai/kimi-k2.6"),
		("qwen-token-plan", "qwen3.7-max"),
		("qwen-token-plan-cn", "qwen3.7-max"),
		("qwen-token-plan-individual", "qwen3.8-max"),
		("xiaomi", "mimo-v2.6-pro"),
		("xiaomi-token-plan-cn", "mimo-v2.6-pro"),
		("xiaomi-token-plan-ams", "mimo-v2.6-pro"),
		("xiaomi-token-plan-sgp", "mimo-v2.6-pro"),
	];

pub fn default_model_per_provider(provider: &str) -> Option<&'static str> {
	DEFAULT_MODELS_PER_PROVIDER
		.iter()
		.find(|(p, _)| *p == provider)
		.map(|(_, id)| *id)
}

/// All provider ids in `defaultModelPerProvider`, in insertion order.
pub fn default_model_provider_order() -> Vec<&'static str> {
	DEFAULT_MODELS_PER_PROVIDER.iter().map(|(p, _)| *p).collect()
}

// ============================================================================
// Request / result types
// ============================================================================

/// How a trailing `:<suffix>` is treated (`ModelReferenceRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingSuffix {
	None,
	Strict,
	Lenient,
}

/// `ModelReferenceRequest` (text + constraints).
#[derive(Debug, Clone)]
pub struct ModelReferenceRequest {
	/// Bare model id or `provider/id`, optionally followed by `:<thinking>`.
	pub text: String,
	/// Only consider models from this provider.
	pub provider: Option<String>,
	pub thinking_suffix: ThinkingSuffix,
	/// Match id and name substrings when no exact match exists.
	pub fuzzy: bool,
}

/// `ModelReferenceTarget` — not-found payload for custom model ids.
#[derive(Debug, Clone)]
pub struct ModelReferenceTarget {
	pub provider: String,
	pub provider_models: Vec<Model>,
	/// Text after the provider prefix as typed, including any `:<suffix>`.
	pub model_text: String,
	/// `model_text` without a valid thinking suffix (unless `none`).
	pub model_id: String,
	pub thinking_level: Option<String>,
}

impl ModelReferenceTarget {
	fn to_value(&self) -> Value {
		let mut map = serde_json::Map::new();
		map.insert("provider".into(), json!(self.provider));
		map.insert(
			"providerModels".into(),
			Value::Array(self.provider_models.iter().map(Model::to_value).collect()),
		);
		map.insert("modelText".into(), json!(self.model_text));
		map.insert("modelId".into(), json!(self.model_id));
		if let Some(l) = &self.thinking_level {
			map.insert("thinkingLevel".into(), json!(l));
		}
		Value::Object(map)
	}
}

/// `ModelReferenceResult` — the discriminated union. Serialized exactly as
/// the TS union (undefined fields omitted).
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // Resolved carries a Model; boxing would obscure the port
pub enum ModelReferenceResult {
	Resolved {
		model: Model,
		thinking_level: Option<String>,
		warning: Option<String>,
	},
	Ambiguous {
		candidates: Vec<Model>,
		usable_count: usize,
		thinking_level: Option<String>,
		warning: Option<String>,
	},
	NotFound {
		target: Option<ModelReferenceTarget>,
	},
}

impl ModelReferenceResult {
	pub fn to_value(&self) -> Value {
		// Verbatim TS union shapes; undefined fields drop out of JSON.stringify.
		let mut map = serde_json::Map::new();
		match self {
			ModelReferenceResult::Resolved { model, thinking_level, warning } => {
				map.insert("status".into(), json!("resolved"));
				map.insert("model".into(), model.to_value());
				if let Some(l) = thinking_level {
					map.insert("thinkingLevel".into(), json!(l));
				}
				if let Some(w) = warning {
					map.insert("warning".into(), json!(w));
				}
			}
			ModelReferenceResult::Ambiguous { candidates, usable_count, thinking_level, warning } => {
				map.insert("status".into(), json!("ambiguous"));
				map.insert(
					"candidates".into(),
					Value::Array(candidates.iter().map(Model::to_value).collect()),
				);
				map.insert("usableCount".into(), json!(usable_count));
				if let Some(l) = thinking_level {
					map.insert("thinkingLevel".into(), json!(l));
				}
				if let Some(w) = warning {
					map.insert("warning".into(), json!(w));
				}
			}
			ModelReferenceResult::NotFound { target } => {
				map.insert("status".into(), json!("not-found"));
				if let Some(t) = target {
					map.insert("target".into(), t.to_value());
				}
			}
		}
		Value::Object(map)
	}

	fn status_is_not_found(&self) -> bool {
		matches!(self, ModelReferenceResult::NotFound { .. })
	}
}

/// `ScopedModel`.
#[derive(Debug, Clone)]
pub struct ScopedModel {
	pub model: Model,
	/// Thinking level if explicitly specified in pattern, undefined otherwise.
	pub thinking_level: Option<String>,
}

/// `ModelScopeDiagnostic`.
#[derive(Debug, Clone)]
pub struct ModelScopeDiagnostic {
	pub code: &'static str,
	pub message: String,
	pub pattern: String,
}

/// `ResolveModelScopeResult`.
#[derive(Debug, Clone)]
pub struct ResolveModelScopeResult {
	pub scoped_models: Vec<ScopedModel>,
	pub diagnostics: Vec<ModelScopeDiagnostic>,
}

impl ResolveModelScopeResult {
	pub fn to_value(&self) -> Value {
		let scoped: Vec<Value> = self
			.scoped_models
			.iter()
			.map(|s| {
				let mut m = serde_json::Map::new();
				m.insert("model".into(), s.model.to_value());
				if let Some(l) = &s.thinking_level {
					m.insert("thinkingLevel".into(), json!(l));
				}
				Value::Object(m)
			})
			.collect();
		let diagnostics: Vec<Value> = self
			.diagnostics
			.iter()
			.map(|d| {
				json!({
					"type": "warning",
					"code": d.code,
					"message": d.message,
					"pattern": d.pattern,
				})
			})
			.collect();
		json!({
			"scopedModels": scoped,
			"diagnostics": diagnostics,
		})
	}
}

/// `ResolveCliModelResult`.
#[derive(Debug, Clone)]
pub struct ResolveCliModelResult {
	pub model: Option<Model>,
	pub thinking_level: Option<String>,
	pub warning: Option<String>,
	pub error: Option<String>,
}

impl ResolveCliModelResult {
	pub fn to_value(&self) -> Value {
		// Verbatim TS return; undefined fields drop out of JSON.stringify.
		let mut map = serde_json::Map::new();
		if let Some(m) = &self.model {
			map.insert("model".into(), m.to_value());
		}
		if let Some(l) = &self.thinking_level {
			map.insert("thinkingLevel".into(), json!(l));
		}
		if let Some(w) = &self.warning {
			map.insert("warning".into(), json!(w));
		}
		if let Some(e) = &self.error {
			map.insert("error".into(), json!(e));
		}
		Value::Object(map)
	}
}

/// `InitialModelResult`.
#[derive(Debug, Clone)]
pub struct InitialModelResult {
	pub model: Option<Model>,
	pub thinking_level: String,
	pub fallback_message: Option<String>,
}

impl InitialModelResult {
	pub fn to_value(&self) -> Value {
		json!({
			"model": self.model.as_ref().map(Model::to_value),
			"thinkingLevel": self.thinking_level,
			"fallbackMessage": self.fallback_message,
		})
	}
}

/// Result of `restore_model_from_session`.
#[derive(Debug, Clone)]
pub struct RestoreModelResult {
	pub model: Option<Model>,
	pub fallback_message: Option<String>,
}

impl RestoreModelResult {
	pub fn to_value(&self) -> Value {
		json!({
			"model": self.model.as_ref().map(Model::to_value),
			"fallbackMessage": self.fallback_message,
		})
	}
}

// ============================================================================
// Splitting helpers
// ============================================================================

/// `splitThinkingSuffix` — `model:high` → (model, high) when the suffix is
/// a valid thinking level.
fn split_thinking_suffix(text: &str) -> Option<(String, String)> {
	let colon_index = text.rfind(':')?;
	let suffix = &text[colon_index + 1..];
	if !is_valid_thinking_level(suffix) {
		return None;
	}
	Some((text[..colon_index].to_string(), suffix.to_string()))
}

/// `splitProviderPrefix` — `provider/rest` when `provider` names a model's
/// provider (case-insensitively); returns the canonical provider spelling.
fn split_provider_prefix(text: &str, models: &[Model]) -> Option<(String, String)> {
	let slash_index = text.find('/')?;
	let prefix = js_trim(&text[..slash_index]).to_lowercase();
	let model_text = js_trim(&text[slash_index + 1..]).to_string();
	if prefix.is_empty() || model_text.is_empty() {
		return None;
	}
	models
		.iter()
		.find(|m| m.provider.to_lowercase() == prefix)
		.map(|m| (m.provider.clone(), model_text))
}

// ============================================================================
// Reference resolution
// ============================================================================

/// `resolveModelReference`. `usable` only breaks ties between providers, so
/// a unique match resolves even without credentials.
pub fn resolve_model_reference(
	request: &ModelReferenceRequest,
	models: &[Model],
	usable: &dyn Fn(&str) -> bool,
) -> ModelReferenceResult {
	let text = js_trim(&request.text).to_string();
	if text.is_empty() {
		return ModelReferenceResult::NotFound { target: None };
	}
	let constraint = request.provider.as_ref().map(|p| p.to_lowercase());
	let pool: Vec<usize> = match &constraint {
		Some(c) => models
			.iter()
			.enumerate()
			.filter(|(_, m)| m.provider.to_lowercase() == *c)
			.map(|(i, _)| i)
			.collect(),
		None => (0..models.len()).collect(),
	};
	let result = resolve_text(&text, request, models, &pool, usable);
	if !result.status_is_not_found() {
		return result;
	}

	let split = split_provider_prefix(&text, models);
	let anchor: Option<usize> = match &constraint {
		Some(_) => pool.first().copied(),
		None => split.as_ref().and_then(|(provider, _)| {
			models.iter().position(|m| m.provider == *provider)
		}),
	};
	let anchor = match anchor {
		Some(a) => a,
		None => return result,
	};
	let model_text = split.as_ref().map(|(_, t)| t.clone()).unwrap_or(text.clone());
	let suffix = if request.thinking_suffix == ThinkingSuffix::None {
		None
	} else {
		split_thinking_suffix(&model_text)
	};
	let mut provider_models = vec![anchor];
	for &i in &pool {
		if i != anchor && models[i].provider == models[anchor].provider {
			provider_models.push(i);
		}
	}
	ModelReferenceResult::NotFound {
		target: Some(ModelReferenceTarget {
			provider: models[anchor].provider.clone(),
			provider_models: provider_models.into_iter().map(|i| models[i].clone()).collect(),
			model_id: suffix.as_ref().map(|(b, _)| b.clone()).unwrap_or(model_text.clone()),
			model_text,
			thinking_level: suffix.map(|(_, l)| l),
		}),
	}
}

/// `findExactModelReferenceMatch` — exact `provider/id` or bare id lookup
/// without thinking suffixes or fuzzy matching.
pub fn find_exact_model_reference_match(model_reference: &str, models: &[Model]) -> Option<Model> {
	let result = resolve_model_reference(
		&ModelReferenceRequest {
			text: model_reference.to_string(),
			provider: None,
			thinking_suffix: ThinkingSuffix::None,
			fuzzy: false,
		},
		models,
		&|_| true,
	);
	match result {
		ModelReferenceResult::Resolved { model, .. } => Some(model),
		_ => None,
	}
}

/// `resolveText` — reference selection with optional fuzzy fallback and
/// thinking-suffix recursion.
fn resolve_text(
	text: &str,
	request: &ModelReferenceRequest,
	models: &[Model],
	pool: &[usize],
	usable: &dyn Fn(&str) -> bool,
) -> ModelReferenceResult {
	if let Some(result) = select_reference(text, models, pool, request.fuzzy, usable) {
		return result;
	}

	if request.fuzzy {
		let matches = fuzzy_matches(text, models, pool);
		if let Some(result) = select_fuzzy(&matches, models, usable) {
			return result;
		}
	}

	if request.thinking_suffix == ThinkingSuffix::None {
		return ModelReferenceResult::NotFound { target: None };
	}
	if let Some((base, thinking_level)) = split_thinking_suffix(text) {
		let inner = resolve_text(&base, request, models, pool, usable);
		if inner.status_is_not_found() {
			return inner;
		}
		// Only use this thinking level if no warning from inner recursion.
		let level = if has_warning(&inner) { None } else { Some(thinking_level) };
		return override_thinking(inner, level, None);
	}
	// Keep an invalid suffix as part of the id and fail, rather than resolve
	// a different model.
	if request.thinking_suffix == ThinkingSuffix::Strict {
		return ModelReferenceResult::NotFound { target: None };
	}
	let colon_index = match text.rfind(':') {
		Some(i) => i,
		None => return ModelReferenceResult::NotFound { target: None },
	};

	let suffix = text[colon_index + 1..].to_string();
	let inner = resolve_text(&text[..colon_index], request, models, pool, usable);
	if inner.status_is_not_found() {
		return inner;
	}
	override_thinking(
		inner,
		None,
		Some(format!(
			"Invalid thinking level \"{suffix}\" in pattern \"{text}\". Using default instead."
		)),
	)
}

/// `{ ...inner, thinkingLevel, warning }` spread on resolved/ambiguous.
fn override_thinking(
	result: ModelReferenceResult,
	thinking_level: Option<String>,
	warning: Option<String>,
) -> ModelReferenceResult {
	match result {
		ModelReferenceResult::Resolved { model, .. } => {
			ModelReferenceResult::Resolved { model, thinking_level, warning }
		}
		ModelReferenceResult::Ambiguous { candidates, usable_count, .. } => {
			ModelReferenceResult::Ambiguous { candidates, usable_count, thinking_level, warning }
		}
		not_found => not_found,
	}
}

fn has_warning(result: &ModelReferenceResult) -> bool {
	match result {
		ModelReferenceResult::Resolved { warning, .. } | ModelReferenceResult::Ambiguous { warning, .. } => {
			warning.is_some()
		}
		ModelReferenceResult::NotFound { .. } => false,
	}
}

/// `selectReference` — `provider/rest` is only a hint; see the TS doc
/// comment. The named provider wins when usable; otherwise a sole usable
/// raw match wins; an unusable named match is still preferred over
/// unusable raw matches.
fn select_reference(
	text: &str,
	models: &[Model],
	pool: &[usize],
	fuzzy: bool,
	usable: &dyn Fn(&str) -> bool,
) -> Option<ModelReferenceResult> {
	let lower_text = text.to_lowercase();
	let raw: Vec<usize> = pool
		.iter()
		.copied()
		.filter(|&i| models[i].id.to_lowercase() == lower_text)
		.collect();
	let split = split_provider_prefix(text, models);
	let mut named: Option<usize> = None;
	if let Some((provider, model_text)) = &split {
		let provider_models: Vec<usize> = pool
			.iter()
			.copied()
			.filter(|&i| &models[i].provider == provider)
			.collect();
		let lower_model_text = model_text.to_lowercase();
		// The provider's own literal id (OpenRouter's `openrouter/free`)
		// beats its fuzzy matches.
		named = provider_models
			.iter()
			.copied()
			.find(|&i| models[i].id.to_lowercase() == lower_model_text)
			.or_else(|| raw.iter().copied().find(|&i| &models[i].provider == provider))
			.or_else(|| {
				if fuzzy {
					let matches = fuzzy_matches(model_text, models, &provider_models);
					best_fuzzy_match(&matches, models)
				} else {
					None
				}
			});
	}

	let others: Vec<usize> = raw.iter().copied().filter(|&i| Some(i) != named).collect();
	let mut candidates: Vec<usize> = Vec::with_capacity(others.len() + 1);
	if let Some(n) = named {
		candidates.push(n);
	}
	candidates.extend(others.iter().copied());
	if candidates.is_empty() {
		return None;
	}
	if unique_provider_count(&candidates, models) == 1 {
		return Some(ModelReferenceResult::Resolved {
			model: models[candidates[0]].clone(),
			thinking_level: None,
			warning: None,
		});
	}
	if let Some(n) = named {
		if usable(&models[n].provider) {
			return Some(ModelReferenceResult::Resolved {
				model: models[n].clone(),
				thinking_level: None,
				warning: None,
			});
		}
	}
	let usable_others: Vec<usize> = others
		.iter()
		.copied()
		.filter(|&i| usable(&models[i].provider))
		.collect();
	if usable_others.len() == 1 {
		return Some(ModelReferenceResult::Resolved {
			model: models[usable_others[0]].clone(),
			thinking_level: None,
			warning: None,
		});
	}
	if usable_others.is_empty() {
		if let Some(n) = named {
			return Some(ModelReferenceResult::Resolved {
				model: models[n].clone(),
				thinking_level: None,
				warning: None,
			});
		}
	}
	Some(ModelReferenceResult::Ambiguous {
		candidates: candidates.iter().map(|&i| models[i].clone()).collect(),
		usable_count: usable_others.len(),
		thinking_level: None,
		warning: None,
	})
}

/// `fuzzyMatches` — id or name substring matches (case-insensitive).
fn fuzzy_matches(text: &str, models: &[Model], pool: &[usize]) -> Vec<usize> {
	let lower_text = text.to_lowercase();
	pool.iter()
		.copied()
		.filter(|&i| {
			models[i].id.to_lowercase().contains(&lower_text)
				|| models[i].name.to_lowercase().contains(&lower_text)
		})
		.collect()
}

/// `selectFuzzy` — best fuzzy match; a same-id tie across providers is
/// ambiguous rather than decided by catalog order.
fn select_fuzzy(
	matches: &[usize],
	models: &[Model],
	usable: &dyn Fn(&str) -> bool,
) -> Option<ModelReferenceResult> {
	if matches.is_empty() {
		return None;
	}
	let multiple_providers = unique_provider_count(matches, models) > 1;
	let usable_matches: Vec<usize> = if multiple_providers {
		matches.iter().copied().filter(|&i| usable(&models[i].provider)).collect()
	} else {
		Vec::new()
	};
	let pool: &[usize] = if !usable_matches.is_empty() { &usable_matches } else { matches };
	let best = best_fuzzy_match(pool, models)?;
	// The same id on several providers would otherwise be decided by
	// catalog order.
	let tied: Vec<usize> = pool.iter().copied().filter(|&i| models[i].id == models[best].id).collect();
	if unique_provider_count(&tied, models) == 1 {
		return Some(ModelReferenceResult::Resolved {
			model: models[best].clone(),
			thinking_level: None,
			warning: None,
		});
	}
	Some(ModelReferenceResult::Ambiguous {
		candidates: tied.iter().map(|&i| models[i].clone()).collect(),
		usable_count: if !usable_matches.is_empty() { tied.len() } else { 0 },
		thinking_level: None,
		warning: None,
	})
}

/// `bestFuzzyMatch` — prefer aliases over dated versions, then the latest
/// (descending `localeCompare` on id, stable sort).
fn best_fuzzy_match(matches: &[usize], models: &[Model]) -> Option<usize> {
	if matches.is_empty() {
		return None;
	}
	let aliases: Vec<usize> = matches.iter().copied().filter(|&i| is_alias(&models[i].id)).collect();
	let list: &[usize] = if !aliases.is_empty() { &aliases } else { matches };
	let mut sorted: Vec<usize> = list.to_vec();
	sorted.sort_by(|&a, &b| js_locale_compare(&models[b].id, &models[a].id));
	sorted.first().copied()
}

/// `isAlias` — `id` is an alias when it ends in `-latest` or does not end
/// in an 8-digit date (`-\d{8}$`).
fn is_alias(id: &str) -> bool {
	if id.ends_with("-latest") {
		return true;
	}
	// TS: `!/-\d{8}$/.test(id)` — NOT a dated version.
	let bytes = id.as_bytes();
	if bytes.len() < 9 || bytes[bytes.len() - 9] != b'-' {
		return true;
	}
	!bytes[bytes.len() - 8..].iter().all(|b| b.is_ascii_digit())
}

/// Number of distinct providers among `indices` (insertion order kept).
fn unique_provider_count(indices: &[usize], models: &[Model]) -> usize {
	let mut seen: Vec<&str> = Vec::new();
	for &i in indices {
		let p = models[i].provider.as_str();
		if !seen.contains(&p) {
			seen.push(p);
		}
	}
	seen.len()
}

/// `buildFallbackModel` — base model with `id`/`name` replaced in place.
fn build_fallback_model(provider_models: &[Model], model_id: &str) -> Model {
	let default_id = default_model_per_provider(&provider_models[0].provider);
	let base = match default_id {
		Some(id) => provider_models
			.iter()
			.find(|m| m.id == id)
			.unwrap_or(&provider_models[0]),
		None => &provider_models[0],
	};
	let mut model = base.clone();
	let _ = model.set("id", json!(model_id));
	let _ = model.set("name", json!(model_id));
	model
}

// ============================================================================
// Scope resolution
// ============================================================================

/// `resolveModelScopeFromModels` — resolve scope patterns (`--models`,
/// `enabledModels`) to models with optional thinking levels. Every model in
/// `models` is usable, so credentials cannot choose between providers
/// offering the same id; all matches are added with an ambiguous warning.
pub fn resolve_model_scope_from_models(patterns: &[String], models: &[Model]) -> ResolveModelScopeResult {
	let mut scoped_models: Vec<ScopedModel> = Vec::new();
	let mut diagnostics: Vec<ModelScopeDiagnostic> = Vec::new();
	let add = |scoped: &mut Vec<ScopedModel>, model: &Model, thinking_level: Option<String>| {
		if !scoped
			.iter()
			.any(|s| models_are_equal_value(&s.model, model))
		{
			scoped.push(ScopedModel { model: model.clone(), thinking_level });
		}
	};
	let no_match = |diagnostics: &mut Vec<ModelScopeDiagnostic>, pattern: &str| {
		diagnostics.push(ModelScopeDiagnostic {
			code: "no-match",
			message: format!("No models match pattern \"{pattern}\""),
			pattern: pattern.to_string(),
		});
	};
	let ambiguous = |diagnostics: &mut Vec<ModelScopeDiagnostic>, pattern: &str, candidates: &[Model]| {
		let mut providers: Vec<&str> = Vec::new();
		for m in candidates {
			if !providers.contains(&m.provider.as_str()) {
				providers.push(&m.provider);
			}
		}
		providers.sort_by(|a, b| js_locale_compare(a, b));
		diagnostics.push(ModelScopeDiagnostic {
			code: "ambiguous",
			message: format!(
				"Pattern \"{}\" matches several providers: {}. Use provider/id to choose one.",
				pattern,
				providers.join(", ")
			),
			pattern: pattern.to_string(),
		});
	};

	for pattern in patterns {
		// Check if pattern contains glob characters.
		if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
			// Extract optional thinking level suffix (e.g., "provider/*:high").
			let thinking = split_thinking_suffix(pattern);
			let glob_pattern = thinking.as_ref().map(|(b, _)| b.clone()).unwrap_or_else(|| pattern.clone());
			let thinking_level = thinking.as_ref().map(|(_, l)| l.clone());

			// Ids such as `bracketed-model[1m]` are not globs.
			let exact = resolve_model_reference(
				&ModelReferenceRequest {
					text: glob_pattern.clone(),
					provider: None,
					thinking_suffix: ThinkingSuffix::None,
					fuzzy: false,
				},
				models,
				&|_| true,
			);
			let matches: Vec<Model> = match exact {
				ModelReferenceResult::Resolved { model, .. } => vec![model],
				ModelReferenceResult::Ambiguous { candidates, .. } => {
					ambiguous(&mut diagnostics, pattern, &candidates);
					candidates
				}
				ModelReferenceResult::NotFound { .. } => {
					// Match against "provider/modelId" format OR just model
					// ID, so "*sonnet*" matches without "anthropic/*sonnet*".
					models
						.iter()
						.filter(|m| {
							minimatch(&format!("{}/{}", m.provider, m.id), &glob_pattern, true)
								|| minimatch(&m.id, &glob_pattern, true)
						})
						.cloned()
						.collect()
				}
			};
			if matches.is_empty() {
				no_match(&mut diagnostics, pattern);
			}
			for model in &matches {
				add(&mut scoped_models, model, thinking_level.clone());
			}
			continue;
		}

		let result = resolve_model_reference(
			&ModelReferenceRequest {
				text: pattern.clone(),
				provider: None,
				thinking_suffix: ThinkingSuffix::Lenient,
				fuzzy: true,
			},
			models,
			&|_| true,
		);
		match &result {
			ModelReferenceResult::NotFound { .. } => {
				no_match(&mut diagnostics, pattern);
				continue;
			}
			ModelReferenceResult::Resolved { warning, .. } | ModelReferenceResult::Ambiguous { warning, .. } => {
				if let Some(warning) = warning {
					diagnostics.push(ModelScopeDiagnostic {
						code: "invalid-thinking-level",
						message: warning.clone(),
						pattern: pattern.to_string(),
					});
				}
			}
		}
		let (matches, thinking_level) = match &result {
			ModelReferenceResult::Resolved { model, thinking_level, .. } => (vec![model.clone()], thinking_level.clone()),
			ModelReferenceResult::Ambiguous { candidates, thinking_level, .. } => {
				ambiguous(&mut diagnostics, pattern, candidates);
				(candidates.clone(), thinking_level.clone())
			}
			ModelReferenceResult::NotFound { .. } => unreachable!(),
		};
		for model in &matches {
			add(&mut scoped_models, model, thinking_level.clone());
		}
	}

	ResolveModelScopeResult { scoped_models, diagnostics }
}

/// `modelsAreEqual` on chat models (same provider + id).
fn models_are_equal_value(a: &Model, b: &Model) -> bool {
	a.provider == b.provider && a.id == b.id
}

// ============================================================================
// CLI model resolution
// ============================================================================

/// Options for `resolve_cli_model`; the `ModelRuntime` is reduced to a
/// model list plus a `hasConfiguredAuth` predicate.
pub struct ResolveCliModelOptions<'a> {
	pub cli_provider: Option<&'a str>,
	pub cli_model: Option<&'a str>,
	pub cli_thinking: Option<&'a str>,
	/// `--api-key` was supplied for the selected provider.
	pub inline_api_key: bool,
	pub models: &'a [Model],
	pub has_configured_auth: &'a dyn Fn(&str) -> bool,
}

/// `resolveCliModel` — resolve a single model from CLI flags.
pub fn resolve_cli_model(options: &ResolveCliModelOptions) -> ResolveCliModelResult {
	let ResolveCliModelOptions { cli_provider, cli_model, cli_thinking, inline_api_key, models, has_configured_auth } =
		options;

	let Some(cli_model) = cli_model else {
		return ResolveCliModelResult {
			model: None,
			thinking_level: None,
			warning: None,
			error: None,
		};
	};

	// Important: use *all* models here, not just models with pre-configured
	// auth, so "--api-key" can be used for first-time setup.
	if models.is_empty() {
		return ResolveCliModelResult {
			model: None,
			thinking_level: None,
			warning: None,
			error: Some(
				"No models available. Check your installation or add models to models.json.".to_string(),
			),
		};
	}

	let provider = cli_provider
		.map(|p| {
			models
				.iter()
				.find(|m| m.provider.to_lowercase() == p.to_lowercase())
				.map(|m| m.provider.clone())
		})
		.flatten();
	if let Some(cli_provider) = cli_provider {
		if provider.is_none() {
			return ResolveCliModelResult {
				model: None,
				thinking_level: None,
				warning: None,
				error: Some(format!(
					"Unknown provider \"{cli_provider}\". Use --list-models to see available providers/models."
				)),
			};
		}
	}

	let usable: Box<dyn Fn(&str) -> bool> = if *inline_api_key {
		// An inline key goes to whichever provider is selected, so stored
		// credentials must not choose it.
		Box::new(|_: &str| false)
	} else {
		Box::new(|provider_id: &str| has_configured_auth(provider_id))
	};
	let result = resolve_model_reference(
		&ModelReferenceRequest {
			text: cli_model.to_string(),
			provider: provider.clone(),
			thinking_suffix: ThinkingSuffix::Strict,
			fuzzy: true,
		},
		models,
		&*usable,
	);
	match result {
		ModelReferenceResult::Resolved { model, thinking_level, warning, .. } => ResolveCliModelResult {
			model: Some(model),
			thinking_level,
			warning,
			error: None,
		},
		ModelReferenceResult::Ambiguous { candidates, usable_count, .. } => {
			let mut matches: Vec<String> =
				candidates.iter().map(|m| format!("{}/{}", m.provider, m.id)).collect();
			matches.sort_by(|a, b| js_locale_compare(a, b));
			let auth_hint = if *inline_api_key {
				"--api-key does not choose a provider."
			} else if usable_count == 0 {
				"No matching provider is authenticated."
			} else {
				"More than one matching provider is authenticated."
			};
			ResolveCliModelResult {
				model: None,
				thinking_level: None,
				warning: None,
				error: Some(format!(
					"Model \"{}\" is ambiguous across providers: {}. {} Use --provider or provider/model.",
					cli_model,
					matches.join(", "),
					auth_hint
				)),
			}
		}
		ModelReferenceResult::NotFound { target } => {
			let Some(target) = target else {
				return ResolveCliModelResult {
					model: None,
					thinking_level: None,
					warning: None,
					error: Some(format!(
						"Model \"{cli_model}\" not found. Use --list-models to see available models."
					)),
				};
			};

			// Use the thinking level suffix only when --thinking is not
			// explicitly provided; otherwise it stays part of the custom
			// model id.
			let model_id = if cli_thinking.is_some() { target.model_text.clone() } else { target.model_id.clone() };
			let fallback_thinking = if cli_thinking.is_some() { None } else { target.thinking_level.clone() };
			let mut fallback_model = build_fallback_model(&target.provider_models, &model_id);
			let requested_thinking = cli_thinking.map(|t| t.to_string()).or_else(|| fallback_thinking.clone());
			if requested_thinking.as_deref().is_some_and(|t| t != "off") {
				let _ = fallback_model.set("reasoning", json!(true));
			}
			ResolveCliModelResult {
				model: Some(fallback_model),
				thinking_level: fallback_thinking,
				warning: Some(format!(
					"Model \"{model_id}\" not found for provider \"{}\". Using custom model id.",
					target.provider
				)),
				error: None,
			}
		}
	}
}

// ============================================================================
// Initial model + session restore
// ============================================================================

/// `modelThinkingLevels` lookup: `"{provider}/{id}" -> level`.
pub type ModelThinkingLevels<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Options for `find_initial_model`; the `ModelRuntime` is reduced to
/// `models` (`getModel`), `available_models` (`getAvailableSnapshot`) and a
/// `configured_auth` list (`hasConfiguredAuth`).
pub struct FindInitialModelOptions<'a> {
	pub scoped_models: &'a [ScopedModel],
	pub is_continuing: bool,
	pub default_provider: Option<&'a str>,
	pub default_model_id: Option<&'a str>,
	pub default_thinking_level: Option<&'a str>,
	/// `modelThinkingLevels` keyed `"{provider}/{id}"`.
	pub model_thinking_levels: Option<ModelThinkingLevels<'a>>,
	pub models: &'a [Model],
	pub available_models: &'a [Model],
	pub configured_auth: &'a [String],
}

/// `findInitialModel` — first scoped model, saved default, or first
/// available model with auth.
pub fn find_initial_model(options: &FindInitialModelOptions) -> InitialModelResult {
	let FindInitialModelOptions {
		scoped_models,
		is_continuing,
		default_provider,
		default_model_id,
		default_thinking_level,
		model_thinking_levels,
		models,
		available_models,
		configured_auth,
	} = options;

	let per_model =
		|provider: &str, id: &str| model_thinking_levels.and_then(|f| f(&format!("{provider}/{id}")));
	let has_auth = |provider: &str| configured_auth.iter().any(|p| p == provider);
	let get_model =
		|provider: &str, id: &str| models.iter().find(|m| m.provider == provider && m.id == id);

	// 1. Use first model from scoped models (skip if continuing/resuming).
	if !scoped_models.is_empty() && !*is_continuing {
		let scoped_model = &scoped_models[0];
		let per = per_model(&scoped_model.model.provider, &scoped_model.model.id);
		return InitialModelResult {
			thinking_level: scoped_model
				.thinking_level
				.clone()
				.or(per)
				.or_else(|| default_thinking_level.map(|t| t.to_string()))
				.unwrap_or_else(|| DEFAULT_THINKING_LEVEL.to_string()),
			model: Some(scoped_model.model.clone()),
			fallback_message: None,
		};
	}

	// 2. Try saved default from settings if auth is configured.
	if let (Some(provider), Some(id)) = (default_provider, default_model_id) {
		if let Some(found) = get_model(provider, id) {
			if has_auth(&found.provider) {
				let thinking_level = per_model(provider, id)
					.or_else(|| default_thinking_level.map(|t| t.to_string()))
					.unwrap_or_else(|| DEFAULT_THINKING_LEVEL.to_string());
				return InitialModelResult {
					model: Some(found.clone()),
					thinking_level,
					fallback_message: None,
				};
			}
		}
	}

	// 3. Try first available model with valid API key.
	if !available_models.is_empty() {
		// Try to find a default model from known providers.
		for provider in default_model_provider_order() {
			let Some(default_id) = default_model_per_provider(provider) else { continue };
			if let Some(found) = available_models
				.iter()
				.find(|m| m.provider == provider && m.id == default_id)
			{
				return InitialModelResult {
					model: Some(found.clone()),
					thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
					fallback_message: None,
				};
			}
		}

		// If no default found, use first available.
		return InitialModelResult {
			model: Some(available_models[0].clone()),
			thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
			fallback_message: None,
		};
	}

	// 4. No model found.
	InitialModelResult {
		model: None,
		thinking_level: DEFAULT_THINKING_LEVEL.to_string(),
		fallback_message: None,
	}
}

/// Options for `restore_model_from_session` (printing suppressed).
pub struct RestoreModelOptions<'a> {
	pub saved_provider: &'a str,
	pub saved_model_id: &'a str,
	pub current_model: Option<&'a Model>,
	pub models: &'a [Model],
	pub available_models: &'a [Model],
	pub configured_auth: &'a [String],
}

/// `restoreModelFromSession` — restore a model from a session, with
/// fallback to available models. Console printing is separated out
/// (`shouldPrintMessages: false` on the TS side).
pub fn restore_model_from_session(options: &RestoreModelOptions) -> RestoreModelResult {
	let RestoreModelOptions { saved_provider, saved_model_id, current_model, models, available_models, configured_auth } =
		options;

	let restored_model = models
		.iter()
		.find(|m| m.provider == *saved_provider && m.id == *saved_model_id);
	let has_configured_auth = restored_model
		.map(|m| configured_auth.contains(&m.provider))
		.unwrap_or(false);

	if let (Some(restored), true) = (restored_model, has_configured_auth) {
		return RestoreModelResult { model: Some(restored.clone()), fallback_message: None };
	}

	// Model not found or no API key — fall back.
	let reason = if restored_model.is_none() { "model no longer exists" } else { "no auth configured" };

	// If we already have a model, use it as fallback.
	if let Some(current) = *current_model {
		return RestoreModelResult {
			model: Some(current.clone()),
			fallback_message: Some(format!(
				"Could not restore model {}/{} ({}). Using {}/{}.",
				saved_provider, saved_model_id, reason, current.provider, current.id
			)),
		};
	}

	// Try to find any available model.
	if !available_models.is_empty() {
		// Try to find a default model from known providers.
		let mut fallback_model: Option<&Model> = None;
		for provider in default_model_provider_order() {
			let Some(default_id) = default_model_per_provider(provider) else { continue };
			if let Some(found) = available_models
				.iter()
				.find(|m| m.provider == provider && m.id == default_id)
			{
				fallback_model = Some(found);
				break;
			}
		}

		// If no default found, use first available.
		let fallback_model = fallback_model.unwrap_or(&available_models[0]);

		return RestoreModelResult {
			model: Some(fallback_model.clone()),
			fallback_message: Some(format!(
				"Could not restore model {}/{} ({}). Using {}/{}.",
				saved_provider, saved_model_id, reason, fallback_model.provider, fallback_model.id
			)),
		};
	}

	// No models available.
	RestoreModelResult { model: None, fallback_message: None }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn is_alias_matches_ts_regex() {
		// TS: id.endsWith("-latest") || !/-\d{8}$/.test(id)
		assert!(is_alias("claude-sonnet-4-5"));
		assert!(!is_alias("claude-sonnet-4-5-20250929"));
		assert!(is_alias("devstral-medium-latest"));
		assert!(is_alias("kimi-k2.6"));
		assert!(!is_alias("m-20250101"));
		assert!(!is_alias("x-12345678"));
		assert!(is_alias("x-1234567")); // 7 digits — not a date
		assert!(is_alias("x-123456789")); // 9 digits — the last 8 are digits but bytes[len-9] is a digit, not '-'
		assert!(is_alias("short"));
	}
}
