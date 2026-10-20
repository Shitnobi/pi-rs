//! Immutable, credential-blind models.json snapshot: port of
//! `packages/coding-agent/src/core/model-config.ts`.
//!
//! `ModelConfig::from_load` mirrors `ModelConfig.load(modelsJsonPath)`
//! with the file read injected by the caller (the parity harness owns fs
//! concerns; the ported logic itself never touches env/fs/network):
//! - no path → empty config, no error;
//! - content → `JSON.parse(stripJsonComments(stripBom(content)))` (V8
//!   parse-error strings via `pi_ai::js::js_json_parse`);
//! - an io error → the TS non-ENOENT `readFile` failure message;
//! - neither → ENOENT → empty config, no error.
//!
//! Schema failures reproduce typebox v1.3.27 `Compile(...).Errors()`
//! output exactly for the `ModelsConfigSchema` tree: message strings,
//! traversal order (objects in schema property order, records/arrays in
//! instance order, anyOf branches in order plus a trailing anyOf error),
//! `formatValidationPath` paths, and the global `maxErrors: 8` cap (each
//! `anyOf` branch validates against a fresh context; branch errors merge
//! back through the cap, so a failing anyOf near the cap can lose its
//! trailing error). See the schema IR below.

use indexmap::IndexMap;
use serde_json::Value;

use pi_core::text::{strip_bom, strip_json_comments};

use crate::pi_ai::js::js_json_parse;

// ============================================================================
// ModelConfig
// ============================================================================

/// One immutable load of models.json. `providers` keeps the RAW parsed
/// provider objects (`deepFreeze(structuredClone(x))` is JSON-identical)
/// in parsed key order; later waves add typed `ModelsJsonProvider` views
/// on top.
pub struct ModelConfig {
	providers: IndexMap<String, Value>,
	error: Option<String>,
}

impl ModelConfig {
	/// `ModelConfig.load(modelsJsonPath)` with the file read injected:
	/// `content` is the file text when available, `io_error` stands in for
	/// a non-ENOENT `readFile` failure, and `None`/`None` means ENOENT
	/// (empty config, no error).
	pub fn from_load(path: Option<&str>, content: Option<&str>, io_error: Option<&str>) -> ModelConfig {
		let Some(path) = path else {
			return ModelConfig::empty();
		};
		if let Some(content) = content {
			return ModelConfig::from_content(path, content);
		}
		if let Some(io_error) = io_error {
			return ModelConfig::with_error(format!(
				"Failed to load models.json: {}\n\nFile: {}",
				io_error, path
			));
		}
		// readFile ENOENT → empty config, no error.
		ModelConfig::empty()
	}

	fn from_content(path: &str, content: &str) -> ModelConfig {
		let parsed = match js_json_parse(&strip_json_comments(strip_bom(content))) {
			Ok(parsed) => parsed,
			Err(message) => {
				return ModelConfig::with_error(format!(
					"Failed to parse models.json: {}\n\nFile: {}",
					message, path
				))
			}
		};
		let errors = validate_models_config(&parsed);
		if !errors.is_empty() {
			let lines = errors
				.iter()
				.map(|error| format!("  - {}: {}", format_validation_path(error), error.message))
				.collect::<Vec<_>>()
				.join("\n");
			// Unreachable in practice (a failed Check always yields errors),
			// but the TS falls back to this literal when the list is empty.
			let lines = if lines.is_empty() { "Unknown schema error" } else { &lines };
			return ModelConfig::with_error(format!(
				"Invalid models.json schema:\n{}\n\nFile: {}",
				lines, path
			));
		}
		let mut providers = IndexMap::new();
		if let Some(map) = parsed.get("providers").and_then(Value::as_object) {
			for (provider_id, provider) in map {
				providers.insert(provider_id.clone(), provider.clone());
			}
		}
		ModelConfig { providers, error: None }
	}

	fn empty() -> ModelConfig {
		ModelConfig { providers: IndexMap::new(), error: None }
	}

	fn with_error(error: String) -> ModelConfig {
		ModelConfig { providers: IndexMap::new(), error: Some(error) }
	}

	/// `getProvider` — the raw provider object.
	pub fn get_provider(&self, provider_id: &str) -> Option<&Value> {
		self.providers.get(provider_id)
	}

	/// `getProviderIds` — ids in parsed key order.
	pub fn get_provider_ids(&self) -> Vec<&str> {
		self.providers.keys().map(String::as_str).collect()
	}

	/// `getError`.
	pub fn get_error(&self) -> Option<&str> {
		self.error.as_deref()
	}
}

// ============================================================================
// Typebox-subset validator
// ============================================================================

/// One localized validation error, shaped like the typebox
/// `TLocalizedValidationError` fields this port observes.
pub(crate) struct ValidationError {
	/// `instancePath`, e.g. `/providers/a/models/0/id` (empty at the root).
	instance_path: String,
	/// The typebox error message, verbatim.
	message: String,
	/// `params.requiredProperties[0]` for `required` errors.
	first_missing_property: Option<String>,
}

/// `formatValidationPath` (module-private in the TS, replicated verbatim):
/// a `required` error points at `basePath + "." + firstMissing`; anything
/// else points at the base path itself, or `root` when empty.
fn format_validation_path(error: &ValidationError) -> String {
	if let Some(first_missing) = &error.first_missing_property {
		let base = instance_base_path(&error.instance_path);
		return if base.is_empty() {
			first_missing.clone()
		} else {
			format!("{}.{}", base, first_missing)
		};
	}
	let path = instance_base_path(&error.instance_path);
	if path.is_empty() {
		"root".to_string()
	} else {
		path
	}
}

/// `instancePath.replace(/^\//, "").replace(/\//g, ".")`.
fn instance_base_path(instance_path: &str) -> String {
	instance_path
		.strip_prefix('/')
		.unwrap_or(instance_path)
		.replace('/', ".")
}

/// Schema IR for the typebox subset used by `ModelsConfigSchema`.
enum Schema {
	/// `Type.Object` — properties in SCHEMA order. Non-optional properties
	/// feed the single `required` error (missing names listed in schema
	/// property order); present properties are then checked in schema
	/// order. Unknown instance properties are ignored
	/// (`additionalProperties` is never `false`).
	Object(Vec<Property>),
	/// `Type.Record(Type.String(), X)` (`patternProperties: {"^.*$": X}`):
	/// instance entries in insertion order, each value checked against `X`.
	Record(Box<Schema>),
	/// `Type.Array(X)` with optional `maxItems`; items in instance order.
	Array(Box<Schema>, Option<usize>),
	/// `Type.String({ minLength })`.
	String(Option<u64>),
	/// `Type.Number` / `Type.Integer` with numeric constraints.
	Number(NumberConstraints),
	/// `Type.Boolean`.
	Boolean,
	/// `Type.Null`.
	Null,
	/// `Type.Literal(x)` — a string constant.
	Literal(&'static str),
	/// `{ enum: [...] }` — a fixed set of string constants.
	Enum(&'static [&'static str]),
	/// `Type.AnyOf([...])` — branches in order.
	AnyOf(Vec<Schema>),
	/// `Type.Unknown()` — any value passes.
	Unknown,
}

struct Property {
	name: &'static str,
	schema: Schema,
	required: bool,
}

struct NumberConstraints {
	integer: bool,
	exclusive_minimum: Option<f64>,
	minimum: Option<f64>,
	maximum: Option<f64>,
}

/// `Compile(ModelsConfigSchema).Errors(value)` — the ordered error list.
pub(crate) fn validate_models_config(value: &Value) -> Vec<ValidationError> {
	let mut context = ErrorContext::default();
	check(&models_config_schema(), value, "", &mut context);
	context.errors
}

/// typebox `ErrorContext`: one error list per context, capped at
/// `Settings.maxErrors` (default 8 — probed: a document with 10 errors
/// yields only the first 8). `anyOf` branches get fresh contexts; their
/// errors are merged back through this cap, so a failing anyOf near the
/// cap can lose its trailing `must match a schema in anyOf` error.
#[derive(Default)]
struct ErrorContext {
	errors: Vec<ValidationError>,
}

/// typebox `Settings.maxErrors` default.
const MAX_ERRORS: usize = 8;

impl ErrorContext {
	/// `ErrorContext.AddError`: appends while below capacity, always
	/// reports the node as failed.
	fn add_error(&mut self, error: ValidationError) {
		if self.errors.len() < MAX_ERRORS {
			self.errors.push(error);
		}
	}

	fn at_capacity(&self) -> bool {
		self.errors.len() >= MAX_ERRORS
	}
}

fn error(path: &str, message: &str) -> ValidationError {
	ValidationError {
		instance_path: path.to_string(),
		message: message.to_string(),
		first_missing_property: None,
	}
}

/// Walk `value` against `schema`, appending errors in typebox traversal
/// order. `path` is the `instancePath` ("" at the root, "/a/b" below).
/// Mirrors `ErrorSchema`, which terminates immediately when the context
/// is at capacity.
fn check(schema: &Schema, value: &Value, path: &str, context: &mut ErrorContext) {
	if context.at_capacity() {
		return;
	}
	match schema {
		Schema::Unknown => {}
		Schema::Boolean => {
			if !value.is_boolean() {
				context.add_error(error(path, "must be boolean"));
			}
		}
		Schema::Null => {
			if !value.is_null() {
				context.add_error(error(path, "must be null"));
			}
		}
		Schema::String(min_length) => match value.as_str() {
			None => context.add_error(error(path, "must be string")),
			Some(text) => {
				if let Some(min) = min_length {
					// JS string length counts UTF-16 code units.
					if (text.encode_utf16().count() as u64) < *min {
						context.add_error(error(
							path,
							&format!("must not have fewer than {} characters", min),
						));
					}
				}
			}
		},
		Schema::Number(constraints) => check_number(constraints, value, path, context),
		Schema::Literal(expected) => {
			// `{ type: "string", const: x }`: a non-string fails BOTH the
			// type keyword and the const keyword (probed:
			// `thinkingFormat: 5` → `must be string` then `must be equal
			// to constant` per branch).
			if value.as_str().is_none() {
				context.add_error(error(path, "must be string"));
			}
			if value.as_str() != Some(*expected) {
				context.add_error(error(path, "must be equal to constant"));
			}
		}
		Schema::Enum(allowed) => {
			let matches = value
				.as_str()
				.is_some_and(|text| allowed.contains(&text));
			if !matches {
				context.add_error(error(path, "must be equal to one of the allowed values"));
			}
		}
		Schema::Object(properties) => check_object(properties, value, path, context),
		Schema::Record(value_schema) => match value.as_object() {
			None => context.add_error(error(path, "must be object")),
			Some(map) => {
				for (key, entry) in map {
					check(value_schema, entry, &format!("{}/{}", path, key), context);
				}
			}
		},
		Schema::Array(item_schema, max_items) => match value.as_array() {
			None => context.add_error(error(path, "must be array")),
			Some(items) => {
				// Items are checked before the `maxItems` keyword (probed
				// error order: item errors, then the maxItems error).
				for (index, item) in items.iter().enumerate() {
					check(item_schema, item, &format!("{}/{}", path, index), context);
				}
				if let Some(max) = max_items {
					if items.len() > *max {
						context.add_error(
							error(path, &format!("must not have more than {} items", max)),
						);
					}
				}
			}
		},
		Schema::AnyOf(branches) => {
			// Each branch is validated against a FRESH error context
			// (own capacity); when every branch fails, the failed
			// contexts' errors are merged back into this context through
			// its capacity cap, followed by the anyOf error itself.
			let mut failed_branches = Vec::new();
			for branch in branches {
				let mut branch_context = ErrorContext::default();
				check(branch, value, path, &mut branch_context);
				if branch_context.errors.is_empty() {
					// A branch matched — the anyOf passes, no errors.
					return;
				}
				failed_branches.push(branch_context);
			}
			for branch_context in failed_branches {
				for branch_error in branch_context.errors {
					context.add_error(branch_error);
				}
			}
			context.add_error(error(path, "must match a schema in anyOf"));
		}
	}
}

/// Object traversal: the single required-missing error (missing names in
/// schema property order), then each schema-defined property PRESENT in
/// the instance, in schema property order.
fn check_object(
	properties: &[Property],
	value: &Value,
	path: &str,
	context: &mut ErrorContext,
) {
	let Some(map) = value.as_object() else {
		context.add_error(error(path, "must be object"));
		return;
	};
	let missing: Vec<&str> = properties
		.iter()
		.filter(|property| property.required && !map.contains_key(property.name))
		.map(|property| property.name)
		.collect();
	if !missing.is_empty() {
		context.add_error(ValidationError {
			instance_path: path.to_string(),
			message: format!("must have required properties {}", missing.join(", ")),
			first_missing_property: Some(missing[0].to_string()),
		});
	}
	for property in properties {
		if let Some(entry) = map.get(property.name) {
			check(&property.schema, entry, &format!("{}/{}", path, property.name), context);
		}
	}
}

fn check_number(
	constraints: &NumberConstraints,
	value: &Value,
	path: &str,
	context: &mut ErrorContext,
) {
	let number = value.as_f64();
	if constraints.integer {
		// `Type.Integer`: non-numbers and non-integral values both fail
		// the type guard (probed: a string yields `must be integer`,
		// unlike `Type.Number`'s `must be number`). Numeric constraints
		// still apply to non-integral numbers (probed: 0.5 against
		// minimum 1 yields both `must be integer` and `must be >= 1`).
		match number {
			Some(n) if n.fract() == 0.0 => {}
			_ => context.add_error(error(path, "must be integer")),
		}
	} else if number.is_none() {
		context.add_error(error(path, "must be number"));
	}
	// The numeric keyword group only runs when the value is a number.
	if let Some(n) = number {
		if let Some(bound) = constraints.exclusive_minimum {
			if n <= bound {
				context.add_error(error(path, &format!("must be > {}", format_bound(bound))));
			}
		}
		if let Some(bound) = constraints.minimum {
			if n < bound {
				context.add_error(error(path, &format!("must be >= {}", format_bound(bound))));
			}
		}
		if let Some(bound) = constraints.maximum {
			if n > bound {
				context.add_error(error(path, &format!("must be <= {}", format_bound(bound))));
			}
		}
	}
}

/// JS number formatting for constraint bounds in messages (all bounds in
/// this schema tree are small integers).
fn format_bound(bound: f64) -> String {
	if bound.fract() == 0.0 {
		format!("{}", bound as i64)
	} else {
		format!("{}", bound)
	}
}

// ============================================================================
// ModelsConfigSchema tree (property ORDER matters — replicate exactly)
// ============================================================================

fn prop(name: &'static str, schema: Schema) -> Property {
	Property { name, schema, required: false }
}

fn req_prop(name: &'static str, schema: Schema) -> Property {
	Property { name, schema, required: true }
}

fn string_schema() -> Schema {
	Schema::String(None)
}

fn string_min_1() -> Schema {
	Schema::String(Some(1))
}

fn boolean_schema() -> Schema {
	Schema::Boolean
}

fn number_schema() -> Schema {
	Schema::Number(NumberConstraints {
		integer: false,
		exclusive_minimum: None,
		minimum: None,
		maximum: None,
	})
}

/// `PositiveTokenCountSchema` — `Type.Number({ exclusiveMinimum: 0 })`.
fn positive_number() -> Schema {
	Schema::Number(NumberConstraints {
		integer: false,
		exclusive_minimum: Some(0.0),
		minimum: None,
		maximum: None,
	})
}

fn integer_min(bound: f64) -> Schema {
	Schema::Number(NumberConstraints {
		integer: true,
		exclusive_minimum: None,
		minimum: Some(bound),
		maximum: None,
	})
}

fn literal(value: &'static str) -> Schema {
	Schema::Literal(value)
}

fn any_of_literals(values: &'static [&'static str]) -> Schema {
	Schema::AnyOf(values.iter().map(|value| Schema::Literal(value)).collect())
}

fn string_array() -> Schema {
	Schema::Array(Box::new(string_schema()), None)
}

fn record_strings() -> Schema {
	Schema::Record(Box::new(string_schema()))
}

/// `SamplingParamsSchema` — `Type.Record(Type.String(), Type.Unknown())`.
fn sampling_params() -> Schema {
	Schema::Record(Box::new(Schema::Unknown))
}

/// `SamplingParamsByThinkingLevelSchema`.
fn sampling_params_by_thinking_level() -> Schema {
	Schema::Object(vec![
		prop("off", sampling_params()),
		prop("minimal", sampling_params()),
		prop("low", sampling_params()),
		prop("medium", sampling_params()),
		prop("high", sampling_params()),
		prop("xhigh", sampling_params()),
		prop("max", sampling_params()),
	])
}

/// `ThinkingLevelMapSchema` — `AnyOf[Str, Null]` per level, all optional.
fn thinking_level_map() -> Schema {
	let level = || Schema::AnyOf(vec![string_schema(), Schema::Null]);
	Schema::Object(vec![
		prop("off", level()),
		prop("minimal", level()),
		prop("low", level()),
		prop("medium", level()),
		prop("high", level()),
		prop("xhigh", level()),
		prop("max", level()),
	])
}

/// `ModelInputModalitySchema` — enum `["text", "image"]`.
fn input_modality() -> Schema {
	Schema::Enum(&["text", "image"])
}

/// `ModelInputLimitsSchema`.
fn input_limits() -> Schema {
	Schema::Object(vec![
		prop("maxRequestBytes", integer_min(1.0)),
		prop(
			"images",
			Schema::Object(vec![
				prop(
					"resize",
					Schema::Object(vec![
						prop("maxWidth", integer_min(1.0)),
						prop("maxHeight", integer_min(1.0)),
						prop("maxBytes", integer_min(1.0)),
						prop(
							"jpegQuality",
							Schema::Number(NumberConstraints {
								integer: true,
								exclusive_minimum: None,
								minimum: Some(1.0),
								maximum: Some(100.0),
							}),
						),
					]),
				),
				prop("maxPerMessage", integer_min(1.0)),
				prop("maxPerRequest", integer_min(1.0)),
			]),
		),
	])
}

/// A cost-tier object: all five number props required (shared by
/// `ModelCostSchema`, its `Partial` form and the fallback-model cost).
fn cost_tier() -> Schema {
	Schema::Object(vec![
		req_prop("inputTokensAbove", number_schema()),
		req_prop("input", number_schema()),
		req_prop("output", number_schema()),
		req_prop("cacheRead", number_schema()),
		req_prop("cacheWrite", number_schema()),
	])
}

/// `ModelCostSchema` — the four cost props required, no minimums
/// (0/-1 pass), optional tiers.
fn model_cost() -> Schema {
	Schema::Object(vec![
		req_prop("input", number_schema()),
		req_prop("output", number_schema()),
		req_prop("cacheRead", number_schema()),
		req_prop("cacheWrite", number_schema()),
		prop("tiers", Schema::Array(Box::new(cost_tier()), None)),
	])
}

/// `Type.Partial(ModelCostSchema)` — same props, none required (tier
/// items keep their own required list).
fn partial_model_cost() -> Schema {
	Schema::Object(vec![
		prop("input", number_schema()),
		prop("output", number_schema()),
		prop("cacheRead", number_schema()),
		prop("cacheWrite", number_schema()),
		prop("tiers", Schema::Array(Box::new(cost_tier()), None)),
	])
}

/// `ModelPromptCacheSchema`.
fn prompt_cache() -> Schema {
	Schema::Object(vec![prop("short", positive_number()), prop("long", positive_number())])
}

/// The `$var` chat-template value object.
fn chat_template_var() -> Schema {
	Schema::Object(vec![
		req_prop(
			"$var",
			any_of_literals(&["thinking.enabled", "thinking.effort", "thinking.budget"]),
		),
		prop("omitWhenOff", boolean_schema()),
	])
}

/// `chatTemplateKwargs` / `chatTemplateArgs` record values.
fn chat_template_map() -> Schema {
	Schema::Record(Box::new(Schema::AnyOf(vec![
		string_schema(),
		number_schema(),
		boolean_schema(),
		Schema::Null,
		chat_template_var(),
	])))
}

/// OpenRouter `sort`-style `preferred_*` metric: number or percentiles.
fn throughput_metric() -> Schema {
	Schema::AnyOf(vec![
		number_schema(),
		Schema::Object(vec![
			prop("p50", number_schema()),
			prop("p75", number_schema()),
			prop("p90", number_schema()),
			prop("p99", number_schema()),
		]),
	])
}

fn number_or_string() -> Schema {
	Schema::AnyOf(vec![number_schema(), string_schema()])
}

/// `openRouterRouting` (property order from the probed schema JSON).
fn open_router_routing() -> Schema {
	Schema::Object(vec![
		prop("allow_fallbacks", boolean_schema()),
		prop("require_parameters", boolean_schema()),
		prop("data_collection", any_of_literals(&["deny", "allow"])),
		prop("zdr", boolean_schema()),
		prop("enforce_distillable_text", boolean_schema()),
		prop("order", string_array()),
		prop("only", string_array()),
		prop("ignore", string_array()),
		prop("quantizations", string_array()),
		prop(
			"sort",
			Schema::AnyOf(vec![
				string_schema(),
				Schema::Object(vec![
					prop("by", string_schema()),
					prop("partition", Schema::AnyOf(vec![string_schema(), Schema::Null])),
				]),
			]),
		),
		prop(
			"max_price",
			Schema::Object(vec![
				prop("prompt", number_or_string()),
				prop("completion", number_or_string()),
				prop("image", number_or_string()),
				prop("audio", number_or_string()),
				prop("request", number_or_string()),
			]),
		),
		prop("preferred_min_throughput", throughput_metric()),
		prop("preferred_max_latency", throughput_metric()),
	])
}

/// `vercelGatewayRouting`.
fn vercel_gateway_routing() -> Schema {
	Schema::Object(vec![
		prop("only", string_array()),
		prop("order", string_array()),
	])
}

/// An allowed fallback model entry (with local pricing metadata).
fn allowed_fallback_model() -> Schema {
	Schema::Object(vec![
		req_prop("provider", string_min_1()),
		req_prop("model", string_min_1()),
		req_prop("cost", model_cost()),
	])
}

/// `ProviderCompatSchema` — all optional, `additionalProperties: true`
/// (property order from the probed schema JSON).
fn provider_compat() -> Schema {
	Schema::Object(vec![
		prop("supportsStore", boolean_schema()),
		prop("supportsDeveloperRole", boolean_schema()),
		prop("supportsReasoningEffort", boolean_schema()),
		prop("supportsUsageInStreaming", boolean_schema()),
		prop("supportsFinishReason", boolean_schema()),
		prop("maxTokensField", any_of_literals(&["max_completion_tokens", "max_tokens"])),
		prop("requiresToolResultName", boolean_schema()),
		prop("requiresAssistantAfterToolResult", boolean_schema()),
		prop("requiresThinkingAsText", boolean_schema()),
		prop("requiresReasoningContentOnAssistantMessages", boolean_schema()),
		prop(
			"thinkingFormat",
			any_of_literals(&[
				"openai",
				"openrouter",
				"deepseek",
				"together",
				"baseten",
				"zai",
				"qwen",
				"chat-template",
				"qwen-chat-template",
				"string-thinking",
				"ant-ling",
			]),
		),
		prop("chatTemplateKwargs", chat_template_map()),
		prop("chatTemplateArgs", chat_template_map()),
		prop("openRouterRouting", open_router_routing()),
		prop("vercelGatewayRouting", vercel_gateway_routing()),
		prop("zaiToolStream", boolean_schema()),
		prop(
			"thinkingTokenBudgetField",
			any_of_literals(&["thinking_token_budget", "thinking_budget", "thinking_budget_tokens"]),
		),
		prop("supportsThinkingTokenBudget", boolean_schema()),
		prop("supportsOpenAIGrammarTools", boolean_schema()),
		prop("supportsMidConvoSystemMessages", boolean_schema()),
		prop("supportsMidConvoToolAdditions", boolean_schema()),
		prop("supportsStrictMode", boolean_schema()),
		prop("cacheControlFormat", literal("anthropic")),
		prop("sendSessionAffinityHeaders", boolean_schema()),
		prop(
			"sessionAffinityFormat",
			any_of_literals(&["openai", "openai-nosession", "openrouter"]),
		),
		prop("supportsLongCacheRetention", boolean_schema()),
		prop("vllmPriority", number_schema()),
		prop("supportsAdditionalTools", boolean_schema()),
		prop("supportsToolSearch", boolean_schema()),
		prop("supportsExplicitPromptCacheMode", boolean_schema()),
		prop("supportsMidConvoEffort", boolean_schema()),
		prop("supportsMaxOutputTokens", boolean_schema()),
		prop("supportsEagerToolInputStreaming", boolean_schema()),
		prop("supportsCacheControlOnTools", boolean_schema()),
		prop("supportsTemperature", boolean_schema()),
		prop("forceAdaptiveThinking", boolean_schema()),
		prop("allowEmptySignature", boolean_schema()),
		prop("supportsStrictTools", boolean_schema()),
		prop("supportsMidConvoToolChanges", boolean_schema()),
		prop(
			"allowedFallbackModels",
			Schema::Array(Box::new(allowed_fallback_model()), Some(3)),
		),
	])
}

/// `ModelDefinitionSchema`.
fn model_definition() -> Schema {
	Schema::Object(vec![
		req_prop("id", string_min_1()),
		prop("name", string_min_1()),
		prop("api", string_min_1()),
		prop("baseUrl", string_min_1()),
		prop("reasoning", boolean_schema()),
		prop("thinkingLevelMap", thinking_level_map()),
		prop("input", Schema::Array(Box::new(input_modality()), None)),
		prop("inputLimits", input_limits()),
		prop("cost", model_cost()),
		prop("promptCache", prompt_cache()),
		prop("contextWindow", positive_number()),
		prop("maxTokens", positive_number()),
		prop("samplingParams", sampling_params()),
		prop("samplingParamsByThinkingLevel", sampling_params_by_thinking_level()),
		prop("headers", record_strings()),
		prop("compat", provider_compat()),
	])
}

/// `ModelOverrideSchema` — the definition schema minus
/// id/api/baseUrl, with a `Partial` cost.
fn model_override() -> Schema {
	Schema::Object(vec![
		prop("name", string_min_1()),
		prop("reasoning", boolean_schema()),
		prop("thinkingLevelMap", thinking_level_map()),
		prop("input", Schema::Array(Box::new(input_modality()), None)),
		prop("inputLimits", input_limits()),
		prop("cost", partial_model_cost()),
		prop("promptCache", prompt_cache()),
		prop("contextWindow", positive_number()),
		prop("maxTokens", positive_number()),
		prop("samplingParams", sampling_params()),
		prop("samplingParamsByThinkingLevel", sampling_params_by_thinking_level()),
		prop("headers", record_strings()),
		prop("compat", provider_compat()),
	])
}

/// `ProviderConfigSchema`.
fn provider_config() -> Schema {
	Schema::Object(vec![
		prop("name", string_min_1()),
		prop("baseUrl", string_min_1()),
		prop("apiKey", string_min_1()),
		prop("api", string_min_1()),
		prop("oauth", literal("radius")),
		prop("headers", record_strings()),
		prop("compat", provider_compat()),
		prop("authHeader", boolean_schema()),
		prop("models", Schema::Array(Box::new(model_definition()), None)),
		prop(
			"modelOverrides",
			Schema::Record(Box::new(model_override())),
		),
	])
}

/// `ModelsConfigSchema` — `$schema` optional, `providers` required.
fn models_config_schema() -> Schema {
	Schema::Object(vec![
		prop("$schema", string_schema()),
		req_prop("providers", Schema::Record(Box::new(provider_config()))),
	])
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn load(content: &str) -> ModelConfig {
		ModelConfig::from_load(Some("/m.json"), Some(content), None)
	}

	/// The schema-error body: the lines between the header and the file
	/// trailer of `Invalid models.json schema:\n<lines>\n\nFile: /m.json`.
	fn schema_error_lines(content: &str) -> Vec<String> {
		let config = load(content);
		let error = config.get_error().expect("expected a schema error");
		let body = error
			.strip_prefix("Invalid models.json schema:\n")
			.and_then(|body| body.strip_suffix("\n\nFile: /m.json"))
			.expect("expected the schema-error shape");
		body.split('\n').map(str::to_string).collect()
	}

	#[test]
	fn no_path_is_empty() {
		let config = ModelConfig::from_load(None, None, None);
		assert_eq!(config.get_error(), None);
		assert_eq!(config.get_provider_ids(), Vec::<&str>::new());
	}

	#[test]
	fn enoent_is_empty() {
		let config = ModelConfig::from_load(Some("/models.json"), None, None);
		assert_eq!(config.get_error(), None);
		assert_eq!(config.get_provider_ids(), Vec::<&str>::new());
	}

	#[test]
	fn io_error_message() {
		let config = ModelConfig::from_load(
			Some("/models.json"),
			None,
			Some("EACCES: permission denied, open '/models.json'"),
		);
		assert_eq!(
			config.get_error(),
			Some("Failed to load models.json: EACCES: permission denied, open '/models.json'\n\nFile: /models.json")
		);
	}

	#[test]
	fn parse_error_message_shape() {
		let config = load("not json");
		let error = config.get_error().expect("expected a parse error");
		assert!(error.starts_with("Failed to parse models.json: "), "{}", error);
		assert!(error.ends_with("\n\nFile: /m.json"), "{}", error);
	}

	#[test]
	fn valid_config_keeps_provider_order() {
		let config = load(r#"{"providers":{"b":{},"a":{},"c":{}}}"#);
		assert_eq!(config.get_error(), None);
		assert_eq!(config.get_provider_ids(), vec!["b", "a", "c"]);
		assert_eq!(config.get_provider("a"), Some(&json!({})));
		assert_eq!(config.get_provider("missing"), None);
	}

	#[test]
	fn valid_provider_round_trips_raw_json() {
		let config = load(
			r#"{"providers":{"anthropic":{"name":"Anthropic","baseUrl":"https://api.anthropic.com","apiKey":"sk-test"}}}"#,
		);
		assert_eq!(
			config.get_provider("anthropic"),
			Some(&json!({
				"name": "Anthropic",
				"baseUrl": "https://api.anthropic.com",
				"apiKey": "sk-test"
			}))
		);
	}

	#[test]
	fn comments_and_trailing_commas_are_stripped() {
		let config = load("{\n  // top-level comment\n  \"providers\": {\"a\": {}}, // trailing\n}");
		assert_eq!(config.get_error(), None);
		assert_eq!(config.get_provider_ids(), vec!["a"]);
	}

	#[test]
	fn bom_is_stripped() {
		let config = load("\u{FEFF}{\"providers\":{}}");
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn missing_providers() {
		assert_eq!(
			schema_error_lines("{}"),
			vec!["  - providers: must have required properties providers"]
		);
	}

	#[test]
	fn root_not_object() {
		assert_eq!(schema_error_lines("[]"), vec!["  - root: must be object"]);
	}

	#[test]
	fn providers_array() {
		assert_eq!(
			schema_error_lines(r#"{"providers":[]}"#),
			vec!["  - providers: must be object"]
		);
	}

	#[test]
	fn provider_value_number() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":1}}"#),
			vec!["  - providers.a: must be object"]
		);
	}

	#[test]
	fn model_missing_id() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{}]}}}"#),
			vec!["  - providers.a.models.0.id: must have required properties id"]
		);
	}

	#[test]
	fn model_empty_id() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":""}]}}}"#),
			vec!["  - providers.a.models.0.id: must not have fewer than 1 characters"]
		);
	}

	#[test]
	fn model_id_number() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":5}]}}}"#),
			vec!["  - providers.a.models.0.id: must be string"]
		);
	}

	#[test]
	fn context_window_zero() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","contextWindow":0}]}}}"#),
			vec!["  - providers.a.models.0.contextWindow: must be > 0"]
		);
	}

	#[test]
	fn max_tokens_negative() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","maxTokens":-1}]}}}"#),
			vec!["  - providers.a.models.0.maxTokens: must be > 0"]
		);
	}

	#[test]
	fn context_window_string() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","contextWindow":"1000"}]}}}"#),
			vec!["  - providers.a.models.0.contextWindow: must be number"]
		);
	}

	#[test]
	fn reasoning_string() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","reasoning":"yes"}]}}}"#),
			vec!["  - providers.a.models.0.reasoning: must be boolean"]
		);
	}

	#[test]
	fn oauth_wrong_literal() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"oauth":"github"}}}"#),
			vec!["  - providers.a.oauth: must be equal to constant"]
		);
	}

	#[test]
	fn oauth_radius_passes() {
		let config = load(r#"{"providers":{"r":{"oauth":"radius","name":"Radius"}}}"#);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn input_bogus_modality() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","input":["text","audio"]}]}}}"#),
			vec!["  - providers.a.models.0.input.1: must be equal to one of the allowed values"]
		);
	}

	#[test]
	fn thinking_level_map_number_yields_anyof_errors() {
		// Probed ground truth: each branch's error, then the anyOf error,
		// all at the same path.
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","thinkingLevelMap":{"high":5}}]}}}"#),
			vec![
				"  - providers.a.models.0.thinkingLevelMap.high: must be string",
				"  - providers.a.models.0.thinkingLevelMap.high: must be null",
				"  - providers.a.models.0.thinkingLevelMap.high: must match a schema in anyOf",
			]
		);
	}

	#[test]
	fn thinking_level_map_null_passes() {
		let config = load(r#"{"providers":{"a":{"models":[{"id":"m","thinkingLevelMap":{"low":null,"high":"max"}}]}}}"#);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn headers_non_string_value() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"headers":{"x":1}}}}"#),
			vec!["  - providers.a.headers.x: must be string"]
		);
	}

	#[test]
	fn cost_missing_required_lists_schema_order() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","cost":{"input":1}}]}}}"#),
			vec![
				"  - providers.a.models.0.cost.output: must have required properties output, cacheRead, cacheWrite"
			]
		);
	}

	#[test]
	fn cost_zero_and_negative_pass() {
		let config = load(
			r#"{"providers":{"a":{"models":[{"id":"m","cost":{"input":0,"output":-1,"cacheRead":0,"cacheWrite":0}}]}}}"#,
		);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn cost_tier_missing_required() {
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"a":{"models":[{"id":"m","cost":{"input":1,"output":1,"cacheRead":1,"cacheWrite":1,"tiers":[{"input":2}]}}]}}}"#
			),
			vec![
				"  - providers.a.models.0.cost.tiers.0.inputTokensAbove: must have required properties inputTokensAbove, output, cacheRead, cacheWrite"
			]
		);
	}

	#[test]
	fn prompt_cache_zero() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","promptCache":{"short":0}}]}}}"#),
			vec!["  - providers.a.models.0.promptCache.short: must be > 0"]
		);
	}

	#[test]
	fn max_request_bytes_zero() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","inputLimits":{"maxRequestBytes":0}}]}}}"#),
			vec!["  - providers.a.models.0.inputLimits.maxRequestBytes: must be >= 1"]
		);
	}

	#[test]
	fn max_request_bytes_string() {
		// Probed: a string against `Type.Integer` yields `must be integer`
		// (not `must be number`).
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","inputLimits":{"maxRequestBytes":"x"}}]}}}"#),
			vec!["  - providers.a.models.0.inputLimits.maxRequestBytes: must be integer"]
		);
	}

	#[test]
	fn jpeg_quality_above_maximum() {
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"a":{"models":[{"id":"m","inputLimits":{"images":{"resize":{"jpegQuality":101}}}}]}}}"#
			),
			vec!["  - providers.a.models.0.inputLimits.images.resize.jpegQuality: must be <= 100"]
		);
	}

	#[test]
	fn compat_max_tokens_field_bogus() {
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"compat":{"maxTokensField":"bogus"}}}}"#),
			vec![
				"  - providers.a.compat.maxTokensField: must be equal to constant",
				"  - providers.a.compat.maxTokensField: must be equal to constant",
				"  - providers.a.compat.maxTokensField: must match a schema in anyOf",
			]
		);
	}

	#[test]
	fn two_bad_props_follow_schema_order() {
		// Instance order is contextWindow-then-reasoning, but the schema
		// property order (reasoning before contextWindow) wins.
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"models":[{"id":"m","contextWindow":0,"reasoning":"x"}]}}}"#),
			vec![
				"  - providers.a.models.0.reasoning: must be boolean",
				"  - providers.a.models.0.contextWindow: must be > 0",
			]
		);
	}

	#[test]
	fn two_providers_follow_instance_order() {
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"z":{"models":[{"id":"m","contextWindow":0}]},"a":{"models":[{"id":"m","maxTokens":-1}]}}}"#
			),
			vec![
				"  - providers.z.models.0.contextWindow: must be > 0",
				"  - providers.a.models.0.maxTokens: must be > 0",
			]
		);
	}

	#[test]
	fn full_valid_model() {
		let config = load(
			r#"{"providers":{"p":{"models":[{"id":"full","name":"Full","api":"anthropic","baseUrl":"https://x.example","reasoning":true,"thinkingLevelMap":{"high":"max","low":null},"input":["text","image"],"inputLimits":{"maxRequestBytes":1000,"images":{"resize":{"maxWidth":1024,"maxHeight":768,"maxBytes":100000,"jpegQuality":80},"maxPerMessage":5,"maxPerRequest":20}},"cost":{"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75,"tiers":[{"inputTokensAbove":0,"input":3,"output":15,"cacheRead":0.3,"cacheWrite":3.75}]},"promptCache":{"short":0.5,"long":1},"contextWindow":200000,"maxTokens":64000,"samplingParams":{"temperature":0.6,"any":{"nested":[1,2]},"nul":null},"samplingParamsByThinkingLevel":{"high":{"temperature":1},"off":{}},"headers":{"x-custom":"v"},"compat":{"maxTokensField":"max_completion_tokens","thinkingFormat":"openai","thinkingTokenBudgetField":"thinking_budget","cacheControlFormat":"anthropic","sessionAffinityFormat":"openai","supportsTemperature":true,"vllmPriority":3}}]}}}"#,
		);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn model_overrides_partial_cost() {
		let config = load(
			r#"{"providers":{"p":{"modelOverrides":{"m":{"name":"Renamed","reasoning":false,"contextWindow":1000,"cost":{"input":1}}}}}}"#,
		);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn unicode_strings_pass() {
		let config =
			load(r#"{"providers":{"ünïcode":{"name":"Ünïcode ✓","models":[{"id":"mødel","name":"名前"}]}}}"#);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn compat_chat_template_kwargs_pass() {
		let config = load(
			r#"{"providers":{"p":{"compat":{"chatTemplateKwargs":{"budget":{"$var":"thinking.budget","omitWhenOff":true},"enabled":"thinking.enabled"}}}}}"#,
		);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn compat_openrouter_routing_passes() {
		let config = load(
			r#"{"providers":{"p":{"compat":{"openRouterRouting":{"allow_fallbacks":false,"data_collection":"deny","only":["a","b"],"sort":{"by":"throughput","partition":null},"max_price":{"prompt":0.5,"completion":"auto"}}}}}}"#,
		);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn compat_allowed_fallback_models_pass() {
		let config = load(
			r#"{"providers":{"p":{"compat":{"allowedFallbackModels":[{"provider":"openai","model":"gpt-5.5","cost":{"input":1,"output":2,"cacheRead":0.5,"cacheWrite":1}}]}}}}"#,
		);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn schema_key_passes() {
		let config = load(r#"{"$schema":"https://example.com/schema.json","providers":{}}"#);
		assert_eq!(config.get_error(), None);
	}

	#[test]
	fn literal_branch_with_non_string_emits_type_and_const() {
		// Probed: each `{ type: "string", const }` branch yields BOTH the
		// type error and the const error for a non-string value — and the
		// global maxErrors cap (8) truncates the 11-branch anyOf after
		// four branches, dropping the trailing anyOf error entirely.
		assert_eq!(
			schema_error_lines(r#"{"providers":{"a":{"compat":{"thinkingFormat":5}}}}"#),
			vec![
				"  - providers.a.compat.thinkingFormat: must be string",
				"  - providers.a.compat.thinkingFormat: must be equal to constant",
				"  - providers.a.compat.thinkingFormat: must be string",
				"  - providers.a.compat.thinkingFormat: must be equal to constant",
				"  - providers.a.compat.thinkingFormat: must be string",
				"  - providers.a.compat.thinkingFormat: must be equal to constant",
				"  - providers.a.compat.thinkingFormat: must be string",
				"  - providers.a.compat.thinkingFormat: must be equal to constant",
			]
		);
	}

	#[test]
	fn anyof_error_dropped_at_capacity() {
		// Probed: the outer anyOf at chatTemplateKwargs.x collects 8 branch
		// errors (four type errors plus the object branch's nested errors),
		// so its own trailing anyOf error is dropped by the cap.
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"a":{"compat":{"chatTemplateKwargs":{"x":{"$var":"bogus","omitWhenOff":"notbool"}}}}}}"#
			),
			vec![
				"  - providers.a.compat.chatTemplateKwargs.x: must be string",
				"  - providers.a.compat.chatTemplateKwargs.x: must be number",
				"  - providers.a.compat.chatTemplateKwargs.x: must be boolean",
				"  - providers.a.compat.chatTemplateKwargs.x: must be null",
				"  - providers.a.compat.chatTemplateKwargs.x.$var: must be equal to constant",
				"  - providers.a.compat.chatTemplateKwargs.x.$var: must be equal to constant",
				"  - providers.a.compat.chatTemplateKwargs.x.$var: must be equal to constant",
				"  - providers.a.compat.chatTemplateKwargs.x.$var: must match a schema in anyOf",
			]
		);
	}

	#[test]
	fn error_list_capped_at_eight() {
		// Probed: 10 failing providers yield only the first 8 errors.
		let mut content = String::from("{\"providers\":{");
		for i in 0..10 {
			if i > 0 {
				content.push(',');
			}
			content.push_str(&format!(r#""p{}": {{"name": ""}}"#, i));
		}
		content.push_str("}}");
		let lines = schema_error_lines(&content);
		assert_eq!(lines.len(), 8);
		assert_eq!(lines[0], "  - providers.p0.name: must not have fewer than 1 characters");
		assert_eq!(lines[7], "  - providers.p7.name: must not have fewer than 1 characters");
	}

	#[test]
	fn max_items_error_after_item_errors() {
		// Probed: item errors come first, then `must not have more than 3
		// items`.
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"p":{"compat":{"allowedFallbackModels":[{"provider":"p0","model":"m0","cost":{"input":1}},{"provider":"p1","model":"m1","cost":{"input":1}},{"provider":"p2","model":"m2","cost":{"input":1,"output":1,"cacheRead":1,"cacheWrite":1}},{"provider":"p3","model":"m3","cost":{"input":1,"output":1,"cacheRead":1,"cacheWrite":1}}]}}}}"#
			),
			vec![
				"  - providers.p.compat.allowedFallbackModels.0.cost.output: must have required properties output, cacheRead, cacheWrite",
				"  - providers.p.compat.allowedFallbackModels.1.cost.output: must have required properties output, cacheRead, cacheWrite",
				"  - providers.p.compat.allowedFallbackModels: must not have more than 3 items",
			]
		);
	}

	#[test]
	fn integer_non_integral_also_checks_constraints() {
		// Probed: 0.5 against `Type.Integer({ minimum: 1 })` yields both
		// the type error and the minimum error.
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"a":{"models":[{"id":"m","inputLimits":{"maxRequestBytes":0.5}}]}}}"#
			),
			vec![
				"  - providers.a.models.0.inputLimits.maxRequestBytes: must be integer",
				"  - providers.a.models.0.inputLimits.maxRequestBytes: must be >= 1",
			]
		);
	}

	#[test]
	fn integer_non_integral_within_bounds() {
		// Probed: 80.5 against `Type.Integer({ minimum: 1, maximum: 100 })`
		// yields only the type error (bounds hold).
		assert_eq!(
			schema_error_lines(
				r#"{"providers":{"a":{"models":[{"id":"m","inputLimits":{"images":{"resize":{"jpegQuality":80.5}}}}]}}}"#
			),
			vec!["  - providers.a.models.0.inputLimits.images.resize.jpegQuality: must be integer"]
		);
	}
}
