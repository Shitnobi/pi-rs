//! Settings JSON schema: port of
//! `packages/coding-agent/src/core/settings-schema.ts`.
//!
//! The TS module's runtime artifact is the `SettingsSchema` typebox
//! object (the `Static`-derived type surface is compile-time only and
//! lands with the settings-manager wave); the schema is published as
//! JSON Schema by `scripts/generate-schemas.ts` and validatable via
//! `Compile(SettingsSchema)`. This port reproduces the validation half
//! byte-exactly: `validate_settings` mirrors typebox v1.3.27
//! `Compile(SettingsSchema).Errors(value)` — message strings, traversal
//! order (objects in schema property order with the `required` error
//! first, records/arrays in instance order, anyOf branches in order
//! against fresh error contexts merged back through the global cap),
//! `instancePath` format, and the `Settings.maxErrors: 8` cap. Every
//! error shape in the tests was probed against the compiled TS schema
//! (Node 24, typebox 1.3.27).
//!
//! `default`/`description`/`deprecated` schema options are
//! validation-invisible and omitted from the IR. `additionalProperties`
//! is permissive throughout (explicitly `true` at the root; probed at
//! `compaction`, `thinkingBudgets` and package objects). The schema
//! never reads env, fs, or network; `undefined` never crosses the JSON
//! boundary.
//!
//! The walker below is the model-config typebox-subset validator
//! (`model_config.rs`) vendored and extended: its `Schema::Literal` is
//! string-only, while settings-schema needs `Type.Literal(false)`
//! (`terminal.images`) and `Type.Literal(0)`/`Type.Literal(1)`
//! (`outputPad`), and its IR is module-private. Keep the two walkers in
//! sync; a later refactor can hoist both into a shared typebox module.

use serde_json::Value;

// ============================================================================
// Public API
// ============================================================================

/// One localized validation error, shaped like the typebox
/// `TLocalizedValidationError` fields this port observes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsValidationError {
	/// `instancePath`, e.g. `/compaction/modelOverrides/a/b/reserveTokens`
	/// (empty at the root).
	instance_path: String,
	/// The typebox error message, verbatim.
	message: String,
	/// `params.requiredProperties[0]` for `required` errors.
	first_missing_property: Option<String>,
}

impl SettingsValidationError {
	/// `instancePath` (empty at the root).
	pub fn instance_path(&self) -> &str {
		&self.instance_path
	}

	/// The typebox error message, verbatim.
	pub fn message(&self) -> &str {
		&self.message
	}

	/// `params.requiredProperties[0]` for `required` errors.
	pub fn first_missing_property(&self) -> Option<&str> {
		self.first_missing_property.as_deref()
	}

	/// The `formatValidationPath` rendering shared with model-config
	/// (`a.b.c` dotted; a `required` error points at the first missing
	/// property; an empty base renders as `root`).
	pub fn format_path(&self) -> String {
		if let Some(first_missing) = &self.first_missing_property {
			let base = instance_base_path(&self.instance_path);
			return if base.is_empty() {
				first_missing.clone()
			} else {
				format!("{}.{}", base, first_missing)
			};
		}
		let path = instance_base_path(&self.instance_path);
		if path.is_empty() {
			"root".to_string()
		} else {
			path
		}
	}
}

/// `Compile(SettingsSchema).Errors(value)` — the ordered error list.
pub fn validate_settings(value: &Value) -> Vec<SettingsValidationError> {
	let mut context = ErrorContext::default();
	check(&settings_schema(), value, "", &mut context);
	context.errors
}

// ============================================================================
// Typebox-subset validator (mirrors model_config.rs; see module docs)
// ============================================================================

/// typebox `Settings.maxErrors` default (probed: see model_config).
const MAX_ERRORS: usize = 8;

/// JS `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// typebox `ErrorContext`: one error list per context, capped at
/// `Settings.maxErrors`. `anyOf` branches get fresh contexts; their
/// errors are merged back through this cap, so a failing anyOf near the
/// cap can lose its trailing `must match a schema in anyOf` error.
#[derive(Default)]
struct ErrorContext {
	errors: Vec<SettingsValidationError>,
}

impl ErrorContext {
	/// `ErrorContext.AddError`: appends while below capacity, always
	/// reports the node as failed.
	fn add_error(&mut self, error: SettingsValidationError) {
		if self.errors.len() < MAX_ERRORS {
			self.errors.push(error);
		}
	}

	fn at_capacity(&self) -> bool {
		self.errors.len() >= MAX_ERRORS
	}
}

fn error(path: &str, message: &str) -> SettingsValidationError {
	SettingsValidationError {
		instance_path: path.to_string(),
		message: message.to_string(),
		first_missing_property: None,
	}
}

/// `instancePath.replace(/^\//, "").replace(/\//g, ".")`.
fn instance_base_path(instance_path: &str) -> String {
	instance_path
		.strip_prefix('/')
		.unwrap_or(instance_path)
		.replace('/', ".")
}

/// Schema IR for the typebox subset used by `SettingsSchema`.
enum Schema {
	/// `Type.Object` — properties in SCHEMA order. Non-optional properties
	/// feed the single `required` error (missing names listed in schema
	/// property order); present properties are then checked in schema
	/// property order. Unknown instance properties are ignored
	/// (`additionalProperties` is permissive throughout — probed).
	Object(Vec<Property>),
	/// `Type.Record(Type.String(), X)` (`patternProperties: {"^.*$": X}`):
	/// instance entries in insertion order, each value checked against `X`.
	/// Keys are embedded raw, so a `"provider/modelId"` key yields a path
	/// like `/modelThinkingLevels/anthropic/claude`.
	Record(Box<Schema>),
	/// `Type.Array(X)` — items in instance order (no `maxItems` in this
	/// tree).
	Array(Box<Schema>),
	/// `Type.String()` (no length constraints in this tree).
	String,
	/// `Type.Number` / `Type.Integer` with numeric constraints.
	Number(NumberConstraints),
	/// `Type.Boolean`.
	Boolean,
	/// `Type.Literal(x)` — a string, boolean or number constant
	/// (`{ type, const }`).
	Literal(LiteralValue),
	/// `Type.Enum([...])` (`{ enum: [...] }`) — string constants.
	Enum(&'static [&'static str]),
	/// `Type.Union([...])` — branches in order.
	AnyOf(Vec<Schema>),
}

/// A `Type.Literal` constant.
enum LiteralValue {
	Str(&'static str),
	Bool(bool),
	Num(f64),
}

struct Property {
	name: &'static str,
	schema: Schema,
	required: bool,
}

struct NumberConstraints {
	integer: bool,
	minimum: Option<f64>,
	maximum: Option<f64>,
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
		Schema::Boolean => {
			if !value.is_boolean() {
				context.add_error(error(path, "must be boolean"));
			}
		}
		Schema::String => {
			if !value.is_string() {
				context.add_error(error(path, "must be string"));
			}
		}
		Schema::Number(constraints) => check_number(constraints, value, path, context),
		Schema::Literal(expected) => check_literal(expected, value, path, context),
		Schema::Enum(allowed) => {
			// A non-string never matches (probed: `defaultThinkingLevel: 5`
			// yields only the enum error).
			let matches = value.as_str().is_some_and(|text| allowed.contains(&text));
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
		Schema::Array(item_schema) => match value.as_array() {
			None => context.add_error(error(path, "must be array")),
			Some(items) => {
				for (index, item) in items.iter().enumerate() {
					check(item_schema, item, &format!("{}/{}", path, index), context);
				}
			}
		},
		Schema::AnyOf(branches) => {
			// Each branch is validated against a FRESH error context (own
			// capacity); when every branch fails, the failed contexts'
			// errors are merged back into this context through its
			// capacity cap, followed by the anyOf error itself — which the
			// cap can drop (probed: `terminal.images: 5` merges eight
			// branch errors and loses the trailing anyOf error).
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

/// `{ type, const }`: a value of the wrong JSON type fails BOTH the type
/// keyword and the const keyword (probed per literal kind: string via
/// `codemode.mode: 5`, boolean via `terminal.images: 5`, number via
/// `outputPad: "1"`); a same-type mismatch fails only the const keyword.
fn check_literal(expected: &LiteralValue, value: &Value, path: &str, context: &mut ErrorContext) {
	let (type_ok, type_message, value_ok) = match *expected {
		LiteralValue::Str(text) => (value.is_string(), "must be string", value.as_str() == Some(text)),
		LiteralValue::Bool(flag) => {
			(value.is_boolean(), "must be boolean", value.as_bool() == Some(flag))
		}
		LiteralValue::Num(number) => {
			(value.is_number(), "must be number", value.as_f64() == Some(number))
		}
	};
	if !type_ok {
		context.add_error(error(path, type_message));
	}
	if !value_ok {
		context.add_error(error(path, "must be equal to constant"));
	}
}

/// Object traversal: the single required-missing error (missing names in
/// schema property order), then each schema-defined property PRESENT in
/// the instance, in schema property order.
fn check_object(properties: &[Property], value: &Value, path: &str, context: &mut ErrorContext) {
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
		context.add_error(SettingsValidationError {
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

fn check_number(constraints: &NumberConstraints, value: &Value, path: &str, context: &mut ErrorContext) {
	let number = value.as_f64();
	if constraints.integer {
		// `Type.Integer`: non-numbers and non-integral values both fail
		// the type guard (probed: a string yields `must be integer`;
		// -0.5 yields `must be integer` plus `must be >= 0`). Numeric
		// constraints still apply to non-integral numbers.
		match number {
			Some(n) if n.fract() == 0.0 => {}
			_ => context.add_error(error(path, "must be integer")),
		}
	} else if number.is_none() {
		context.add_error(error(path, "must be number"));
	}
	// The numeric keyword group only runs when the value is a number.
	if let Some(n) = number {
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
/// this schema tree are small integers plus `Number.MAX_SAFE_INTEGER`).
fn format_bound(bound: f64) -> String {
	if bound.fract() == 0.0 {
		format!("{}", bound as i64)
	} else {
		format!("{}", bound)
	}
}

// ============================================================================
// SettingsSchema tree (property ORDER matters — replicate exactly)
// ============================================================================

fn prop(name: &'static str, schema: Schema) -> Property {
	Property { name, schema, required: false }
}

fn req_prop(name: &'static str, schema: Schema) -> Property {
	Property { name, schema, required: true }
}

fn string_schema() -> Schema {
	Schema::String
}

fn boolean_schema() -> Schema {
	Schema::Boolean
}

fn number_schema() -> Schema {
	Schema::Number(NumberConstraints { integer: false, minimum: None, maximum: None })
}

fn number_min(bound: f64) -> Schema {
	Schema::Number(NumberConstraints { integer: false, minimum: Some(bound), maximum: None })
}

/// `nonNegativeSafeInteger` —
/// `Type.Integer({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER })`.
fn non_negative_safe_integer() -> Schema {
	Schema::Number(NumberConstraints {
		integer: true,
		minimum: Some(0.0),
		maximum: Some(MAX_SAFE_INTEGER),
	})
}

/// `timeoutSetting` —
/// `Type.Union([Type.Number({ minimum: 0 }), Type.Literal("disabled")])`.
fn timeout_setting() -> Schema {
	Schema::AnyOf(vec![number_min(0.0), Schema::Literal(LiteralValue::Str("disabled"))])
}

fn literal(value: &'static str) -> Schema {
	Schema::Literal(LiteralValue::Str(value))
}

fn any_of_literals(values: &'static [&'static str]) -> Schema {
	Schema::AnyOf(values.iter().map(|value| literal(value)).collect())
}

fn string_array() -> Schema {
	Schema::Array(Box::new(string_schema()))
}

/// `ModelThinkingLevelSchema` —
/// `Type.Enum(["off", ...THINKING_LEVELS])` from pi-ai.
fn model_thinking_level() -> Schema {
	Schema::Enum(&["off", "minimal", "low", "medium", "high", "xhigh", "max"])
}

/// `CompactionModelOverrideSchema`.
fn compaction_model_override() -> Schema {
	Schema::Object(vec![
		prop("reserveTokens", non_negative_safe_integer()),
		prop("keepRecentTokens", non_negative_safe_integer()),
	])
}

/// `CompactionSettingsSchema`.
fn compaction_settings() -> Schema {
	Schema::Object(vec![
		prop("enabled", boolean_schema()),
		prop("reserveTokens", non_negative_safe_integer()),
		prop("keepRecentTokens", non_negative_safe_integer()),
		prop("modelOverrides", Schema::Record(Box::new(compaction_model_override()))),
	])
}

/// `BranchSummarySettingsSchema`.
fn branch_summary_settings() -> Schema {
	Schema::Object(vec![
		prop("reserveTokens", number_schema()),
		prop("skipPrompt", boolean_schema()),
	])
}

/// `ProviderRetrySettingsSchema`.
fn provider_retry_settings() -> Schema {
	Schema::Object(vec![
		prop("timeoutMs", number_schema()),
		prop("maxRetries", number_schema()),
		prop("maxRetryDelayMs", number_schema()),
	])
}

/// `RetrySettingsSchema` — `maxDelayMs` is the deprecated legacy setting,
/// still validated (last, in schema property order).
fn retry_settings() -> Schema {
	Schema::Object(vec![
		prop("enabled", boolean_schema()),
		prop("maxRetries", number_schema()),
		prop("baseDelayMs", number_schema()),
		prop("maxAgentDelayMs", number_schema()),
		prop("provider", provider_retry_settings()),
		prop("maxDelayMs", number_schema()),
	])
}

/// `TerminalSettingsSchema`.
fn terminal_settings() -> Schema {
	Schema::Object(vec![
		prop("showImages", boolean_schema()),
		prop("imageWidthCells", number_schema()),
		prop("clearOnShrink", boolean_schema()),
		prop("showTerminalProgress", boolean_schema()),
		prop("hyperlinks", Schema::AnyOf(vec![boolean_schema(), literal("auto")])),
		prop(
			"images",
			Schema::AnyOf(vec![
				literal("kitty"),
				literal("iterm2"),
				literal("auto"),
				Schema::Literal(LiteralValue::Bool(false)),
			]),
		),
		prop("trueColor", Schema::AnyOf(vec![boolean_schema(), literal("auto")])),
	])
}

/// `ImageSettingsSchema`.
fn image_settings() -> Schema {
	Schema::Object(vec![
		prop("autoResize", boolean_schema()),
		prop("blockImages", boolean_schema()),
	])
}

/// `thinkingBudgetsSettings` (levels: minimal/low/medium/high).
fn thinking_budgets_settings() -> Schema {
	Schema::Object(vec![
		prop("minimal", number_schema()),
		prop("low", number_schema()),
		prop("medium", number_schema()),
		prop("high", number_schema()),
	])
}

/// `MarkdownSettingsSchema`.
fn markdown_settings() -> Schema {
	Schema::Object(vec![
		prop("codeBlockIndent", string_schema()),
		prop("mermaid", any_of_literals(&["off", "final", "streaming"])),
	])
}

/// `WarningSettingsSchema`.
fn warning_settings() -> Schema {
	Schema::Object(vec![prop("anthropicExtraUsage", boolean_schema())])
}

/// `codemodeMode` — `Type.Union([Type.Literal("on"), Type.Literal("only")])`.
fn codemode_mode() -> Schema {
	any_of_literals(&["on", "only"])
}

/// `CodemodeSettingsSchema`.
fn codemode_settings() -> Schema {
	Schema::Object(vec![
		prop("mode", codemode_mode()),
		prop("inlineBudget", number_min(0.0)),
	])
}

/// `PackageSourceSchema` — string form or object form (`source` is the
/// only required property in the whole tree).
fn package_source() -> Schema {
	Schema::AnyOf(vec![
		string_schema(),
		Schema::Object(vec![
			req_prop("source", string_schema()),
			prop("autoload", boolean_schema()),
			prop("extensions", string_array()),
			prop("skills", string_array()),
			prop("prompts", string_array()),
			prop("themes", string_array()),
		]),
	])
}

/// `SkillsInputSchema` — string array or the deprecated object form.
fn skills_input() -> Schema {
	Schema::AnyOf(vec![
		string_array(),
		Schema::Object(vec![
			prop("enableSkillCommands", boolean_schema()),
			prop("customDirectories", string_array()),
		]),
	])
}

/// `SettingsSchema` — all properties optional, `additionalProperties: true`.
fn settings_schema() -> Schema {
	Schema::Object(vec![
		prop("$schema", string_schema()),
		prop("lastChangelogVersion", string_schema()),
		prop("defaultProvider", string_schema()),
		prop("defaultModel", string_schema()),
		prop("defaultThinkingLevel", model_thinking_level()),
		prop("modelThinkingLevels", Schema::Record(Box::new(model_thinking_level()))),
		prop("transport", any_of_literals(&["auto", "sse", "websocket", "websocket-cached"])),
		prop("sessionShareDestination", any_of_literals(&["auto", "gist", "radius"])),
		prop("steeringMode", any_of_literals(&["all", "one-at-a-time"])),
		prop("followUpMode", any_of_literals(&["all", "one-at-a-time"])),
		prop("theme", string_schema()),
		prop("compaction", compaction_settings()),
		prop("branchSummary", branch_summary_settings()),
		prop("retry", retry_settings()),
		prop("hideThinkingBlock", boolean_schema()),
		prop("showCacheMissNotices", boolean_schema()),
		prop("externalEditor", string_schema()),
		prop("shellPath", string_schema()),
		prop("quietStartup", Schema::AnyOf(vec![boolean_schema(), literal("header")])),
		prop("defaultProjectTrust", any_of_literals(&["ask", "always", "never"])),
		prop("shellCommandPrefix", string_schema()),
		prop("npmCommand", string_array()),
		prop("collapseChangelog", boolean_schema()),
		prop("enableInstallTelemetry", boolean_schema()),
		prop("enableAnalytics", boolean_schema()),
		prop("trackingId", string_schema()),
		prop("packages", Schema::Array(Box::new(package_source()))),
		prop("extensions", string_array()),
		prop("deviceId", string_schema()),
		prop("skills", skills_input()),
		prop("prompts", string_array()),
		prop("themes", string_array()),
		prop("enableSkillCommands", boolean_schema()),
		prop("terminal", terminal_settings()),
		prop("images", image_settings()),
		prop("enabledModels", string_array()),
		prop("defaultTools", string_array()),
		prop("doubleEscapeAction", any_of_literals(&["fork", "tree", "none"])),
		prop(
			"treeFilterMode",
			any_of_literals(&["default", "no-tools", "user-only", "labeled-only", "all"]),
		),
		prop("thinkingBudgets", thinking_budgets_settings()),
		prop("editorPaddingX", number_schema()),
		prop(
			"outputPad",
			Schema::AnyOf(vec![
				Schema::Literal(LiteralValue::Num(0.0)),
				Schema::Literal(LiteralValue::Num(1.0)),
			]),
		),
		prop("autocompleteMaxVisible", number_schema()),
		prop("showHardwareCursor", boolean_schema()),
		prop("markdown", markdown_settings()),
		prop("warnings", warning_settings()),
		prop("codemode", codemode_settings()),
		prop("sessionDir", string_schema()),
		prop("httpProxy", string_schema()),
		prop("httpIdleTimeoutMs", timeout_setting()),
		prop("cacheWarming", any_of_literals(&["off", "streaming", "idle"])),
		prop("websocketConnectTimeoutMs", timeout_setting()),
		prop("tuiMode", any_of_literals(&["regular", "fullscreen"])),
		prop("fullscreenExitOutput", any_of_literals(&["transcript", "resume-hint"])),
		prop("fullscreenScrollbar", any_of_literals(&["auto", "always", "hidden"])),
		prop("fullscreenCopyOnSelect", boolean_schema()),
		prop("fullscreenWheelScrollLines", Schema::AnyOf(vec![number_schema(), literal("auto")])),
		prop("queueMode", any_of_literals(&["all", "one-at-a-time"])),
		prop("websockets", boolean_schema()),
	])
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	/// The `(instancePath, message)` pairs, the observable shape of
	/// `Compile(SettingsSchema).Errors(value)`.
	fn errors(value: Value) -> Vec<(String, String)> {
		validate_settings(&value)
			.into_iter()
			.map(|error| (error.instance_path().to_string(), error.message().to_string()))
			.collect()
	}

	/// Asserts the document produces no errors.
	fn valid(value: Value) {
		assert_eq!(errors(value), Vec::<(String, String)>::new());
	}

	#[test]
	fn root_must_be_object() {
		for root in [json!("x"), json!(42), json!(true), json!(null), json!([])] {
			assert_eq!(
				errors(root),
				vec![(String::new(), "must be object".to_string())],
			);
		}
	}

	#[test]
	fn root_error_formats_as_root() {
		let error = &validate_settings(&json!(42))[0];
		assert_eq!(error.instance_path(), "");
		assert_eq!(error.format_path(), "root");
	}

	#[test]
	fn empty_and_unknown_root_keys_pass() {
		valid(json!({}));
		valid(json!({"a": 1, "b": 2, "c": [1, 2, {"d": null}]}));
	}

	#[test]
	fn string_properties() {
		for name in [
			"$schema",
			"lastChangelogVersion",
			"defaultProvider",
			"defaultModel",
			"theme",
			"externalEditor",
			"shellPath",
			"shellCommandPrefix",
			"trackingId",
			"deviceId",
			"sessionDir",
			"httpProxy",
		] {
			assert_eq!(
				errors(json!({ name: 5 })),
				vec![(format!("/{}", name), "must be string".to_string())],
				"property {}",
				name
			);
		}
	}

	#[test]
	fn boolean_properties() {
		for name in [
			"hideThinkingBlock",
			"showCacheMissNotices",
			"collapseChangelog",
			"enableInstallTelemetry",
			"enableAnalytics",
			"enableSkillCommands",
			"showHardwareCursor",
			"fullscreenCopyOnSelect",
			"websockets",
		] {
			assert_eq!(
				errors(json!({ name: "x" })),
				vec![(format!("/{}", name), "must be boolean".to_string())],
				"property {}",
				name
			);
		}
	}

	#[test]
	fn number_properties() {
		for (name, path) in [
			("editorPaddingX", "/editorPaddingX"),
			("autocompleteMaxVisible", "/autocompleteMaxVisible"),
			("branchSummary", "/branchSummary/reserveTokens"),
			("retry", "/retry/maxDelayMs"),
		] {
			let value = match name {
				"branchSummary" => json!({"branchSummary": {"reserveTokens": "x"}}),
				"retry" => json!({"retry": {"maxDelayMs": "x"}}),
				other => json!({ other: "x" }),
			};
			assert_eq!(
				errors(value),
				vec![(path.to_string(), "must be number".to_string())],
				"property {}",
				name
			);
		}
	}

	#[test]
	fn safe_integer_bounds() {
		// Probed: -1 fails the minimum, 1.5 the type, -0.5 both, and
		// 2^53 the maximum (Number.MAX_SAFE_INTEGER + 1).
		assert_eq!(
			errors(json!({"compaction": {"reserveTokens": -1}})),
			vec![("/compaction/reserveTokens".to_string(), "must be >= 0".to_string())]
		);
		assert_eq!(
			errors(json!({"compaction": {"reserveTokens": 1.5}})),
			vec![("/compaction/reserveTokens".to_string(), "must be integer".to_string())]
		);
		assert_eq!(
			errors(json!({"compaction": {"reserveTokens": -0.5}})),
			vec![
				("/compaction/reserveTokens".to_string(), "must be integer".to_string()),
				("/compaction/reserveTokens".to_string(), "must be >= 0".to_string()),
			]
		);
		assert_eq!(
			errors(json!({"compaction": {"keepRecentTokens": 9007199254740992i64}})),
			vec![
				("/compaction/keepRecentTokens".to_string(), "must be <= 9007199254740991".to_string())
			]
		);
	}

	#[test]
	fn safe_integer_bounds_hold() {
		valid(json!({"compaction": {"reserveTokens": 0, "keepRecentTokens": 9007199254740991i64}}));
	}

	#[test]
	fn nested_object_type_errors() {
		assert_eq!(
			errors(json!({"compaction": 5})),
			vec![("/compaction".to_string(), "must be object".to_string())]
		);
		assert_eq!(
			errors(json!({"retry": {"provider": []}})),
			vec![("/retry/provider".to_string(), "must be object".to_string())]
		);
		assert_eq!(
			errors(json!({"terminal": 5})),
			vec![("/terminal".to_string(), "must be object".to_string())]
		);
	}

	#[test]
	fn compaction_valid() {
		valid(json!({
			"compaction": {
				"enabled": false,
				"reserveTokens": 1024,
				"keepRecentTokens": 20000,
				"modelOverrides": {
					"anthropic/claude-sonnet-4": {"reserveTokens": 2048},
					"openai/gpt-5.1": {"keepRecentTokens": 0}
				}
			}
		}));
	}

	#[test]
	fn compaction_model_override_errors() {
		// Record keys are embedded raw: "a/b" yields /a/b path segments.
		assert_eq!(
			errors(json!({"compaction": {"modelOverrides": {"a/b": {"reserveTokens": "x"}}}})),
			vec![(
				"/compaction/modelOverrides/a/b/reserveTokens".to_string(),
				"must be integer".to_string()
			)]
		);
	}

	#[test]
	fn branch_summary_and_retry_valid() {
		valid(json!({
			"branchSummary": {"reserveTokens": 16384, "skipPrompt": true},
			"retry": {
				"enabled": true,
				"maxRetries": 3,
				"baseDelayMs": 2000.5,
				"maxAgentDelayMs": 300000,
				"provider": {"timeoutMs": 0, "maxRetries": -1, "maxRetryDelayMs": 60000},
				"maxDelayMs": 5
			}
		}));
	}

	#[test]
	fn thinking_level_enum() {
		assert_eq!(
			errors(json!({"defaultThinkingLevel": "bogus"})),
			vec![(
				"/defaultThinkingLevel".to_string(),
				"must be equal to one of the allowed values".to_string()
			)]
		);
		// A non-string never matches the enum (no extra type error).
		assert_eq!(
			errors(json!({"defaultThinkingLevel": 5})),
			vec![(
				"/defaultThinkingLevel".to_string(),
				"must be equal to one of the allowed values".to_string()
			)]
		);
		valid(json!({"defaultThinkingLevel": "off"}));
		valid(json!({"defaultThinkingLevel": "xhigh"}));
	}

	#[test]
	fn model_thinking_levels_record() {
		assert_eq!(
			errors(json!({"modelThinkingLevels": {"anthropic/claude": 5}})),
			vec![(
				"/modelThinkingLevels/anthropic/claude".to_string(),
				"must be equal to one of the allowed values".to_string()
			)]
		);
		valid(json!({"modelThinkingLevels": {"a/b": "high", "ünï/x": "off"}}));
	}

	#[test]
	fn literal_union_properties() {
		for (name, allowed) in [
			("transport", &["auto", "sse", "websocket", "websocket-cached"][..]),
			("sessionShareDestination", &["auto", "gist", "radius"]),
			("steeringMode", &["all", "one-at-a-time"]),
			("followUpMode", &["all", "one-at-a-time"]),
			("defaultProjectTrust", &["ask", "always", "never"]),
			("doubleEscapeAction", &["fork", "tree", "none"]),
			("treeFilterMode", &["default", "no-tools", "user-only", "labeled-only", "all"]),
			("cacheWarming", &["off", "streaming", "idle"]),
			("tuiMode", &["regular", "fullscreen"]),
			("fullscreenExitOutput", &["transcript", "resume-hint"]),
			("fullscreenScrollbar", &["auto", "always", "hidden"]),
			("queueMode", &["all", "one-at-a-time"]),
		] {
			// A string value fails only the const keyword per branch,
			// then the trailing anyOf error.
			let mut expected: Vec<(String, String)> = allowed
				.iter()
				.map(|_| ("/".to_string() + name, "must be equal to constant".to_string()))
				.collect();
			let path = format!("/{}", name);
			expected.push((path.clone(), "must match a schema in anyOf".to_string()));
			assert_eq!(
				errors(json!({ name: "bogus" })),
				expected,
				"property {}",
				name
			);
			for &value in allowed {
				valid(json!({ name: value }));
			}
		}
	}

	#[test]
	fn nested_literal_unions() {
		let mut expected: Vec<(String, String)> = ["off", "final", "streaming"]
			.iter()
			.map(|_| ("/markdown/mermaid".to_string(), "must be equal to constant".to_string()))
			.collect();
		expected.push(("/markdown/mermaid".to_string(), "must match a schema in anyOf".to_string()));
		assert_eq!(errors(json!({"markdown": {"mermaid": "bogus"}})), expected);

		let mut expected: Vec<(String, String)> = ["on", "only"]
			.iter()
			.map(|_| ("/codemode/mode".to_string(), "must be equal to constant".to_string()))
			.collect();
		expected.push(("/codemode/mode".to_string(), "must match a schema in anyOf".to_string()));
		assert_eq!(errors(json!({"codemode": {"mode": "bogus"}})), expected);
	}

	#[test]
	fn literal_non_string_yields_dual_errors_per_branch() {
		// Probed: a number against string literals fails BOTH the type
		// keyword and the const keyword in each branch.
		assert_eq!(
			errors(json!({"codemode": {"mode": 5}})),
			vec![
				("/codemode/mode".to_string(), "must be string".to_string()),
				("/codemode/mode".to_string(), "must be equal to constant".to_string()),
				("/codemode/mode".to_string(), "must be string".to_string()),
				("/codemode/mode".to_string(), "must be equal to constant".to_string()),
				("/codemode/mode".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
	}

	#[test]
	fn boolean_or_auto_unions() {
		for name in ["hyperlinks", "trueColor"] {
			assert_eq!(
				errors(json!({"terminal": { name: "bogus" }})),
				vec![
					(format!("/terminal/{}", name), "must be boolean".to_string()),
					(format!("/terminal/{}", name), "must be equal to constant".to_string()),
					(format!("/terminal/{}", name), "must match a schema in anyOf".to_string()),
				],
				"property {}",
				name
			);
			valid(json!({"terminal": { name: true }}));
			valid(json!({"terminal": { name: "auto" }}));
		}
	}

	#[test]
	fn quiet_startup_union() {
		assert_eq!(
			errors(json!({"quietStartup": 5})),
			vec![
				("/quietStartup".to_string(), "must be boolean".to_string()),
				("/quietStartup".to_string(), "must be string".to_string()),
				("/quietStartup".to_string(), "must be equal to constant".to_string()),
				("/quietStartup".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		valid(json!({"quietStartup": true}));
		valid(json!({"quietStartup": "header"}));
	}

	#[test]
	fn terminal_images_string_value() {
		// A string fails the three string literals' const keyword only,
		// then the boolean literal's type and const keywords.
		assert_eq!(
			errors(json!({"terminal": {"images": "bogus"}})),
			vec![
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be boolean".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
	}

	#[test]
	fn terminal_images_boolean_true() {
		// `true` fails the string literals' type AND const keywords, and
		// only the boolean literal's const keyword.
		assert_eq!(
			errors(json!({"terminal": {"images": true}})),
			vec![
				("/terminal/images".to_string(), "must be string".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be string".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be string".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
	}

	#[test]
	fn terminal_images_number_hits_error_cap() {
		// Probed: a number yields six string-literal errors plus the
		// boolean literal's two — exactly the maxErrors cap of 8, so the
		// trailing anyOf error is dropped.
		assert_eq!(
			errors(json!({"terminal": {"images": 5}})),
			vec![
				("/terminal/images".to_string(), "must be string".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be string".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be string".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
				("/terminal/images".to_string(), "must be boolean".to_string()),
				("/terminal/images".to_string(), "must be equal to constant".to_string()),
			]
		);
	}

	#[test]
	fn terminal_images_valid_values() {
		for value in [json!("kitty"), json!("iterm2"), json!("auto"), json!(false)] {
			valid(json!({"terminal": {"images": value}}));
		}
	}

	#[test]
	fn output_pad_number_literals() {
		assert_eq!(
			errors(json!({"outputPad": 2})),
			vec![
				("/outputPad".to_string(), "must be equal to constant".to_string()),
				("/outputPad".to_string(), "must be equal to constant".to_string()),
				("/outputPad".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		// A non-number fails the type keyword and the const keyword per
		// branch (probed for both strings and booleans).
		for value in [json!("1"), json!(true)] {
			assert_eq!(
				errors(json!({"outputPad": value})),
				vec![
					("/outputPad".to_string(), "must be number".to_string()),
					("/outputPad".to_string(), "must be equal to constant".to_string()),
					("/outputPad".to_string(), "must be number".to_string()),
					("/outputPad".to_string(), "must be equal to constant".to_string()),
					("/outputPad".to_string(), "must match a schema in anyOf".to_string()),
				]
			);
		}
		valid(json!({"outputPad": 0}));
		valid(json!({"outputPad": 1}));
	}

	#[test]
	fn timeout_settings() {
		for name in ["httpIdleTimeoutMs", "websocketConnectTimeoutMs"] {
			assert_eq!(
				errors(json!({ name: -5 })),
				vec![
					(format!("/{}", name), "must be >= 0".to_string()),
					(format!("/{}", name), "must be string".to_string()),
					(format!("/{}", name), "must be equal to constant".to_string()),
					(format!("/{}", name), "must match a schema in anyOf".to_string()),
				],
				"property {}",
				name
			);
			assert_eq!(
				errors(json!({ name: "bogus" })),
				vec![
					(format!("/{}", name), "must be number".to_string()),
					(format!("/{}", name), "must be equal to constant".to_string()),
					(format!("/{}", name), "must match a schema in anyOf".to_string()),
				],
				"property {}",
				name
			);
			valid(json!({ name: 0 }));
			valid(json!({ name: 300000 }));
			valid(json!({ name: "disabled" }));
		}
	}

	#[test]
	fn fullscreen_wheel_scroll_lines() {
		assert_eq!(
			errors(json!({"fullscreenWheelScrollLines": "bogus"})),
			vec![
				("/fullscreenWheelScrollLines".to_string(), "must be number".to_string()),
				("/fullscreenWheelScrollLines".to_string(), "must be equal to constant".to_string()),
				("/fullscreenWheelScrollLines".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		valid(json!({"fullscreenWheelScrollLines": 3}));
		valid(json!({"fullscreenWheelScrollLines": "auto"}));
	}

	#[test]
	fn codemode_inline_budget_minimum() {
		assert_eq!(
			errors(json!({"codemode": {"inlineBudget": -1}})),
			vec![("/codemode/inlineBudget".to_string(), "must be >= 0".to_string())]
		);
		valid(json!({"codemode": {"mode": "only", "inlineBudget": 3000}}));
	}

	#[test]
	fn string_array_properties() {
		for name in [
			"npmCommand",
			"extensions",
			"enabledModels",
			"defaultTools",
			"prompts",
			"themes",
		] {
			assert_eq!(
				errors(json!({ name: "npm" })),
				vec![(format!("/{}", name), "must be array".to_string())],
				"property {}",
				name
			);
			assert_eq!(
				errors(json!({ name: [5] })),
				vec![(format!("/{}/0", name), "must be string".to_string())],
				"property {}",
				name
			);
			valid(json!({ name: ["mise", "exec", "node@20", "--", "npm"] }));
		}
	}

	#[test]
	fn packages_array_type() {
		assert_eq!(
			errors(json!({"packages": "notarray"})),
			vec![("/packages".to_string(), "must be array".to_string())]
		);
	}

	#[test]
	fn packages_number_item() {
		assert_eq!(
			errors(json!({"packages": [5]})),
			vec![
				("/packages/0".to_string(), "must be string".to_string()),
				("/packages/0".to_string(), "must be object".to_string()),
				("/packages/0".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
	}

	#[test]
	fn packages_object_branch() {
		// Branch 0 (string) fails first, then the object branch's nested
		// errors, then the trailing anyOf error.
		assert_eq!(
			errors(json!({"packages": [{"source": 5}]})),
			vec![
				("/packages/0".to_string(), "must be string".to_string()),
				("/packages/0/source".to_string(), "must be string".to_string()),
				("/packages/0".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
	}

	#[test]
	fn packages_required_source_is_reported_first() {
		// Probed: the `required` error precedes the property errors inside
		// the object branch.
		assert_eq!(
			errors(json!({"packages": [{"autoload": 5, "extensions": [5]}]})),
			vec![
				("/packages/0".to_string(), "must be string".to_string()),
				("/packages/0".to_string(), "must have required properties source".to_string()),
				("/packages/0/autoload".to_string(), "must be boolean".to_string()),
				("/packages/0/extensions/0".to_string(), "must be string".to_string()),
				("/packages/0".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		let error = &validate_settings(&json!({"packages": [{"autoload": 5}]}))[1];
		assert_eq!(error.first_missing_property(), Some("source"));
		assert_eq!(error.format_path(), "packages.0.source");
	}

	#[test]
	fn packages_valid_and_unknown_keys_pass() {
		valid(json!({"packages": [{"source": "x", "bogus": 1}, "plain"]}));
		valid(json!({
			"packages": [{
				"source": "npm:@example/pkg",
				"autoload": false,
				"extensions": ["a"],
				"skills": ["b"],
				"prompts": ["c"],
				"themes": ["d"]
			}]
		}));
	}

	#[test]
	fn skills_union() {
		assert_eq!(
			errors(json!({"skills": 5})),
			vec![
				("/skills".to_string(), "must be array".to_string()),
				("/skills".to_string(), "must be object".to_string()),
				("/skills".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		assert_eq!(
			errors(json!({"skills": {"customDirectories": [5]}})),
			vec![
				("/skills".to_string(), "must be array".to_string()),
				("/skills/customDirectories/0".to_string(), "must be string".to_string()),
				("/skills".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		assert_eq!(
			errors(json!({"skills": {"enableSkillCommands": "x"}})),
			vec![
				("/skills".to_string(), "must be array".to_string()),
				("/skills/enableSkillCommands".to_string(), "must be boolean".to_string()),
				("/skills".to_string(), "must match a schema in anyOf".to_string()),
			]
		);
		valid(json!({"skills": ["a", "b"]}));
		valid(json!({"skills": {"enableSkillCommands": true, "customDirectories": ["c"]}}));
	}

	#[test]
	fn thinking_budgets_and_markdown_valid() {
		valid(json!({"thinkingBudgets": {"minimal": 0, "low": 1, "medium": 2.5, "high": 1000}}));
		valid(json!({"markdown": {"codeBlockIndent": "  ", "mermaid": "streaming"}}));
		assert_eq!(
			errors(json!({"thinkingBudgets": {"high": "x"}})),
			vec![("/thinkingBudgets/high".to_string(), "must be number".to_string())]
		);
		// Unknown level keys pass (permissive objects).
		valid(json!({"thinkingBudgets": {"bogus": 1}}));
	}

	#[test]
	fn terminal_and_images_settings_valid() {
		valid(json!({
			"terminal": {
				"showImages": false,
				"imageWidthCells": 80,
				"clearOnShrink": true,
				"showTerminalProgress": true,
				"hyperlinks": true,
				"images": "kitty",
				"trueColor": "auto"
			},
			"images": {"autoResize": true, "blockImages": false}
		}));
		assert_eq!(
			errors(json!({"images": {"autoResize": "x"}})),
			vec![("/images/autoResize".to_string(), "must be boolean".to_string())]
		);
	}

	#[test]
	fn warnings_setting() {
		assert_eq!(
			errors(json!({"warnings": {"anthropicExtraUsage": "x"}})),
			vec![("/warnings/anthropicExtraUsage".to_string(), "must be boolean".to_string())]
		);
		valid(json!({"warnings": {"anthropicExtraUsage": false}}));
	}

	#[test]
	fn top_level_errors_follow_schema_order_with_cap() {
		// Probed: nine bad string properties produce the first eight in
		// SCHEMA property order (not instance order), dropping the ninth
		// (httpProxy) via the maxErrors cap.
		assert_eq!(
			errors(json!({
				"theme": 1, "externalEditor": 1, "shellPath": 1, "shellCommandPrefix": 1,
				"sessionDir": 1, "httpProxy": 1, "trackingId": 1, "deviceId": 1,
				"lastChangelogVersion": 1
			})),
			vec![
				("/lastChangelogVersion".to_string(), "must be string".to_string()),
				("/theme".to_string(), "must be string".to_string()),
				("/externalEditor".to_string(), "must be string".to_string()),
				("/shellPath".to_string(), "must be string".to_string()),
				("/shellCommandPrefix".to_string(), "must be string".to_string()),
				("/trackingId".to_string(), "must be string".to_string()),
				("/deviceId".to_string(), "must be string".to_string()),
				("/sessionDir".to_string(), "must be string".to_string()),
			]
		);
	}

	#[test]
	fn record_errors_follow_instance_order_with_cap() {
		// Probed pattern from model-config: record entries iterate in
		// instance (insertion) order; the cap keeps the first eight.
		let mut overrides = serde_json::Map::new();
		for i in 0..10 {
			overrides.insert(format!("p{}", i), json!({"reserveTokens": "x"}));
		}
		let errors = errors(json!({"compaction": {"modelOverrides": overrides}}));
		assert_eq!(errors.len(), 8);
		for (index, (path, message)) in errors.iter().enumerate() {
			assert_eq!(
				path,
				&format!("/compaction/modelOverrides/p{}/reserveTokens", index)
			);
			assert_eq!(message, "must be integer");
		}
	}

	#[test]
	fn full_valid_settings_document() {
		// Split into two halves (a single json! literal this large exceeds
		// the default macro recursion limit).
		let mut doc = json!({
			"$schema": "https://example.com/schema.json",
			"lastChangelogVersion": "1.2.3",
			"defaultProvider": "anthropic",
			"defaultModel": "claude-sonnet-4",
			"defaultThinkingLevel": "high",
			"modelThinkingLevels": {"openai/gpt-5.1": "medium"},
			"transport": "websocket-cached",
			"sessionShareDestination": "gist",
			"steeringMode": "all",
			"followUpMode": "one-at-a-time",
			"theme": "dark",
			"compaction": {
				"enabled": true,
				"reserveTokens": 16384,
				"keepRecentTokens": 20000,
				"modelOverrides": {"anthropic/claude-opus-4": {"reserveTokens": 4096}}
			},
			"branchSummary": {"reserveTokens": 16384, "skipPrompt": false},
			"retry": {
				"enabled": true,
				"maxRetries": 3,
				"baseDelayMs": 2000,
				"maxAgentDelayMs": 300000,
				"provider": {"maxRetryDelayMs": 60000},
				"maxDelayMs": 30000
			},
			"hideThinkingBlock": false,
			"showCacheMissNotices": true,
			"externalEditor": "vim",
			"shellPath": "/bin/zsh",
			"quietStartup": "header",
			"defaultProjectTrust": "ask",
			"shellCommandPrefix": "eval $(mise activate bash);",
			"npmCommand": ["mise", "exec", "node@20", "--", "npm"],
			"collapseChangelog": true,
			"enableInstallTelemetry": true,
			"enableAnalytics": false,
			"trackingId": "abc123",
			"packages": [
				"npm:@example/pkg",
				{"source": "git+https://example.com/repo", "autoload": true, "skills": ["s"]}
			],
			"extensions": ["./exts/foo.ts"],
			"deviceId": "00000000-0000-0000-0000-000000000000",
			"skills": ["./skills/bar"],
			"prompts": ["./prompts/baz.md"],
			"themes": ["./themes/qux.json"],
			"enableSkillCommands": true
		});
		doc.as_object_mut()
			.unwrap()
			.extend(json!({
				"terminal": {
					"showImages": true,
					"imageWidthCells": 60,
					"clearOnShrink": false,
					"showTerminalProgress": false,
					"hyperlinks": "auto",
					"images": "auto",
					"trueColor": true
				},
				"images": {"autoResize": true, "blockImages": false},
				"enabledModels": ["anthropic/*"],
				"defaultTools": ["+edit", "-bash"],
				"doubleEscapeAction": "tree",
				"treeFilterMode": "user-only",
				"thinkingBudgets": {"minimal": 1024, "low": 4096, "medium": 8192, "high": 16384},
				"editorPaddingX": 0,
				"outputPad": 1,
				"autocompleteMaxVisible": 5,
				"showHardwareCursor": false,
				"markdown": {"codeBlockIndent": "  ", "mermaid": "final"},
				"warnings": {"anthropicExtraUsage": true},
				"codemode": {"mode": "on", "inlineBudget": 3000},
				"sessionDir": "~/.pi/sessions",
				"httpProxy": "http://proxy.example:8080",
				"httpIdleTimeoutMs": "disabled",
				"cacheWarming": "idle",
				"websocketConnectTimeoutMs": 10000,
				"tuiMode": "fullscreen",
				"fullscreenExitOutput": "resume-hint",
				"fullscreenScrollbar": "always",
				"fullscreenCopyOnSelect": true,
				"fullscreenWheelScrollLines": "auto",
				"queueMode": "all",
				"websockets": true
			}).as_object().unwrap().clone());
		valid(doc);
	}
}
