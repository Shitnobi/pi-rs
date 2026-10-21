//! Port of `pi-ai` `src/utils/validation.ts` `validateToolArguments` and the
//! typebox 1.3.27 schema-engine subset it needs.
//!
//! The TS helper runs `Compile(parameters).Check/Errors(value)` plus the
//! coercion fallbacks (`normalizeOptionalNulls`, `Value.Convert`,
//! `coerceWithJsonSchema`). Typebox's engine dispatches on JSON Schema
//! keywords (`type`, `properties`, `required`, `items`, …), so plain JSON
//! schemas — everything that crosses the parity-harness JSON boundary —
//! validate through this walker exactly like the TS side.
//!
//! `Value.Convert` is a no-op here: its dispatch reads the non-JSON `~kind`
//! symbol emitted by the `Type.*` builders, which `JSON.parse` can never
//! produce, so for harness-exchanged schemas the TS call returns its input
//! unchanged (probed against typebox 1.3.27: `Convert(plainSchema, args) ===
//! args`). The `TYPEBOX_KIND` guard around `coerceWithJsonSchema` is
//! therefore always taken on the TS side for plain schemas; the Rust port
//! applies the coercion unconditionally.
//!
//! Supported keywords (byte-exact error strings from typebox's `en_US`
//! locale): boolean schemas, `type` (string and array forms), `required`,
//! `properties`, `additionalProperties`, `min/maxProperties`, `items`
//! (single and tuple), `min/maxItems`, `uniqueItems`, `min/maxLength`,
//! `pattern`, `minimum`/`maximum`/`exclusiveMinimum`/`exclusiveMaximum`,
//! `const`, `enum`, `allOf`/`anyOf`/`oneOf`. Unsupported keywords
//! (`$ref`/`$defs`, `format`, `if`/`then`/`else`, `not`, `contains`,
//! `prefixItems`, `patternProperties`, `propertyNames`, `dependencies*`,
//! `multipleOf`, `unevaluated*`) are ignored — the parity scenarios for
//! this tranche use plain object schemas.

use serde_json::{Map, Value};

use pi_providers::pi_ai::js::{js_number_to_string, js_stringify_pretty, js_to_number};

/// typebox `Settings.maxErrors` default (probed; same value the
/// settings-schema port observed).
const MAX_ERRORS: usize = 8;

// ============================================================================
// JS value guards
// ============================================================================

fn js_length(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn is_object_not_array(value: &Value) -> bool {
    value.is_object()
}

fn is_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|n| n.fract() == 0.0)
}

/// JS structural deep equality (object key order insensitive).
fn deep_equal(a: &Value, b: &Value) -> bool {
    a == b
}

// ============================================================================
// Schema keyword predicates (typebox `Schema.Is*`)
// ============================================================================

fn has_key(schema: &Value, key: &str) -> bool {
    schema.as_object().is_some_and(|m| m.contains_key(key))
}

fn is_schema(value: &Value) -> bool {
    value.is_object() || value.is_boolean()
}

fn is_type(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(_)) => true,
        Some(Value::Array(items)) => items.iter().all(Value::is_string),
        _ => false,
    }
}

fn is_required(schema: &Value) -> bool {
    matches!(schema.get("required"), Some(Value::Array(items)) if items.iter().all(Value::is_string))
}

fn is_properties(schema: &Value) -> bool {
    matches!(schema.get("properties"), Some(props) if props.is_object()
        && props.as_object().unwrap().values().all(is_schema))
}

fn is_items(schema: &Value) -> bool {
    match schema.get("items") {
        Some(v) if is_schema(v) => true,
        Some(Value::Array(items)) => items.iter().all(is_schema),
        _ => false,
    }
}

fn is_items_sized(schema: &Value) -> bool {
    schema.get("items").is_some_and(Value::is_array)
}

fn is_additional_properties(schema: &Value) -> bool {
    has_key(schema, "additionalProperties") && is_schema(schema.get("additionalProperties").unwrap())
}

fn is_enum(schema: &Value) -> bool {
    has_key(schema, "enum") && schema.get("enum").unwrap().is_array()
}

fn is_const(schema: &Value) -> bool {
    has_key(schema, "const")
}

fn is_any_of(schema: &Value) -> bool {
    has_key(schema, "anyOf")
}

fn is_one_of(schema: &Value) -> bool {
    has_key(schema, "oneOf")
}

fn is_all_of(schema: &Value) -> bool {
    has_key(schema, "allOf")
}

fn is_unique_items(schema: &Value) -> bool {
    has_key(schema, "uniqueItems")
}

fn is_number_keyword(schema: &Value, key: &str) -> bool {
    has_key(schema, key) && schema.get(key).unwrap().is_number()
}

// ============================================================================
// typebox en_US locale
// ============================================================================

/// The `en_US` message for one localized error (params mirror the typebox
/// `params` objects).
fn locale_message(keyword: &str, params: &Value) -> String {
    let p = |k: &str| params.get(k).cloned().unwrap_or(Value::Null);
    match keyword {
        "additionalProperties" => "must not have additional properties".to_string(),
        "anyOf" => "must match a schema in anyOf".to_string(),
        "boolean" => "schema is false".to_string(),
        "const" => "must be equal to constant".to_string(),
        "enum" => "must be equal to one of the allowed values".to_string(),
        "exclusiveMaximum" => format!("must be < {}", number_to_string(&p("limit"))),
        "exclusiveMinimum" => format!("must be > {}", number_to_string(&p("limit"))),
        "maxItems" => format!("must not have more than {} items", number_to_string(&p("limit"))),
        "maxLength" => format!("must not have more than {} characters", number_to_string(&p("limit"))),
        "maxProperties" => format!("must not have more than {} properties", number_to_string(&p("limit"))),
        "maximum" => format!("must be <= {}", number_to_string(&p("limit"))),
        "minItems" => format!("must not have fewer than {} items", number_to_string(&p("limit"))),
        "minLength" => format!("must not have fewer than {} characters", number_to_string(&p("limit"))),
        "minProperties" => format!("must not have fewer than {} properties", number_to_string(&p("limit"))),
        "minimum" => format!("must be >= {}", number_to_string(&p("limit"))),
        "pattern" => format!("must match pattern \"{}\"", p("pattern").as_str().unwrap_or("")),
        "required" => {
            let names: Vec<String> = p("requiredProperties")
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            format!("must have required properties {}", names.join(", "))
        }
        "type" => match p("type") {
            Value::String(s) => format!("must be {s}"),
            Value::Array(types) => {
                let names: Vec<String> =
                    types.iter().filter_map(Value::as_str).map(str::to_string).collect();
                format!("must be either {}", names.join(" or "))
            }
            _ => "must be either".to_string(),
        },
        "oneOf" => "must match exactly one schema in oneOf".to_string(),
        "uniqueItems" => "must not have duplicate items".to_string(),
        _ => "an unknown validation error occurred".to_string(),
    }
}

/// `String(number)` for a JSON number value.
fn number_to_string(value: &Value) -> String {
    match value {
        Value::Number(n) => js_number_to_string(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        _ => js_number_to_string(0.0),
    }
}

// ============================================================================
// Error context
// ============================================================================

/// One localized validation error (typebox `TLocalizedValidationError`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedError {
    pub keyword: String,
    pub schema_path: String,
    pub instance_path: String,
    pub params: Value,
}

/// typebox `ErrorContext`: capped error list.
#[derive(Default)]
struct ErrorContext {
    errors: Vec<LocalizedError>,
}

impl ErrorContext {
    fn at_capacity(&self) -> bool {
        self.errors.len() >= MAX_ERRORS
    }

    /// `AddError` returns false so callers report failure.
    fn add_error(&mut self, keyword: &str, schema_path: &str, instance_path: &str, params: Value) -> bool {
        if !self.at_capacity() {
            self.errors.push(LocalizedError {
                keyword: keyword.to_string(),
                schema_path: schema_path.to_string(),
                instance_path: instance_path.to_string(),
                params,
            });
        }
        false
    }
}

// ============================================================================
// Check (typebox `CheckSchema`)
// ============================================================================

fn check_type_name(type_name: &str, value: &Value) -> bool {
    match type_name {
        "object" => is_object_not_array(value),
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "integer" => is_integer(value),
        "number" => value.is_number(),
        "null" => value.is_null(),
        "string" => value.is_string(),
        _ => true,
    }
}

fn check_type(schema: &Value, value: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(s)) => check_type_name(s, value),
        Some(Value::Array(types)) => {
            types.iter().filter_map(Value::as_str).any(|t| check_type_name(t, value))
        }
        _ => true,
    }
}

/// `Compile(schema).Check(value)` — the typebox keyword walker.
pub fn check_schema(schema: &Value, value: &Value) -> bool {
    if schema.is_boolean() {
        return schema == &Value::Bool(true);
    }
    let is_object = is_object_not_array(value);
    let is_array = value.is_array();
    let is_string = value.is_string();
    let is_number = value.is_number();

    (!is_type(schema) || check_type(schema, value))
        && (!is_object
            || ((!is_required(schema) || check_required(schema, value))
                && (!is_additional_properties(schema) || check_additional_properties(schema, value))
                && (!is_properties(schema) || check_properties(schema, value))
                && (!is_number_keyword(schema, "minProperties")
                    || check_min_properties(schema, value))
                && (!is_number_keyword(schema, "maxProperties")
                    || check_max_properties(schema, value))))
        && (!is_array
            || ((!is_items(schema) || check_items(schema, value))
                && (!is_number_keyword(schema, "minItems") || check_min_items(schema, value))
                && (!is_number_keyword(schema, "maxItems") || check_max_items(schema, value))
                && (!is_unique_items(schema) || check_unique_items(schema, value))))
        && (!is_string
            || ((!is_number_keyword(schema, "maxLength") || check_max_length(schema, value))
                && (!is_number_keyword(schema, "minLength") || check_min_length(schema, value))
                && (!has_key(schema, "pattern") || check_pattern(schema, value))))
        && (!is_number
            || ((!is_number_keyword(schema, "exclusiveMaximum")
                || check_exclusive_maximum(schema, value))
                && (!is_number_keyword(schema, "exclusiveMinimum")
                    || check_exclusive_minimum(schema, value))
                && (!is_number_keyword(schema, "maximum") || check_maximum(schema, value))
                && (!is_number_keyword(schema, "minimum") || check_minimum(schema, value))))
        && (!is_const(schema) || check_const(schema, value))
        && (!is_enum(schema) || check_enum(schema, value))
        && (!is_all_of(schema) || check_all_of(schema, value))
        && (!is_any_of(schema) || check_any_of(schema, value))
        && (!is_one_of(schema) || check_one_of(schema, value))
}

fn check_required(schema: &Value, value: &Value) -> bool {
    let Some(keys) = schema.get("required").and_then(Value::as_array) else { return true };
    let obj = value.as_object().expect("required only checked for objects");
    keys.iter().filter_map(Value::as_str).all(|k| obj.contains_key(k))
}

/// The `properties` + `patternProperties` key matcher (patternProperties is
/// unsupported, so this is the escaped literal property keys; no keys → the
/// typebox never-matching `(?!)`).
fn properties_pattern(schema: &Value) -> String {
    let mut patterns: Vec<String> = Vec::new();
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for key in props.keys() {
            let escaped: String = key
                .chars()
                .map(|c| {
                    if ".*+?^${}()|[]\\".contains(c) {
                        format!("\\{c}")
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            patterns.push(format!("^{escaped}$"));
        }
    }
    if patterns.is_empty() {
        "(?!)".to_string()
    } else {
        format!("({})", patterns.join("|"))
    }
}

fn regex_test(pattern: &str, value: &str) -> bool {
    match fancy_regex::Regex::new(pattern) {
        Ok(re) => re.is_match(value).unwrap_or(false),
        Err(_) => false,
    }
}

fn check_additional_properties(schema: &Value, value: &Value) -> bool {
    let pattern = properties_pattern(schema);
    let ap = schema.get("additionalProperties").unwrap();
    let Some(obj) = value.as_object() else { return true };
    obj.iter().all(|(key, item)| {
        regex_test(&pattern, key)
            || (check_schema(ap, item))
    })
}

fn check_properties(schema: &Value, value: &Value) -> bool {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return true };
    let obj = value.as_object().expect("properties only checked for objects");
    props.iter().all(|(key, prop_schema)| {
        match obj.get(key) {
            None => true,
            Some(item) => check_schema(prop_schema, item),
        }
    })
}

fn check_min_properties(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("minProperties").and_then(Value::as_f64).unwrap_or(0.0);
    let count = value.as_object().map_or(0.0, |o| o.len() as f64);
    count >= limit
}

fn check_max_properties(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("maxProperties").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    let count = value.as_object().map_or(0.0, |o| o.len() as f64);
    count <= limit
}

fn check_items(schema: &Value, value: &Value) -> bool {
    let items = schema.get("items").unwrap();
    let Some(arr) = value.as_array() else { return true };
    if is_items_sized(schema) {
        let Some(schemas) = items.as_array() else { return true };
        arr.iter().enumerate().all(|(index, element)| {
            match schemas.get(index) {
                None => true,
                Some(item_schema) => index >= arr.len() || check_schema(item_schema, element),
            }
        })
    } else {
        arr.iter().all(|element| check_schema(items, element))
    }
}

fn check_min_items(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("minItems").and_then(Value::as_f64).unwrap_or(0.0);
    (value.as_array().map_or(0, Vec::len) as f64) >= limit
}

fn check_max_items(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("maxItems").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    (value.as_array().map_or(0, Vec::len) as f64) <= limit
}

fn check_unique_items(_schema: &Value, value: &Value) -> bool {
    let Some(arr) = value.as_array() else { return true };
    for (i, a) in arr.iter().enumerate() {
        for b in arr.iter().skip(i + 1) {
            if deep_equal(a, b) {
                return false;
            }
        }
    }
    true
}

fn check_max_length(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("maxLength").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    (js_length(value.as_str().unwrap_or("")) as f64) <= limit
}

fn check_min_length(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("minLength").and_then(Value::as_f64).unwrap_or(0.0);
    (js_length(value.as_str().unwrap_or("")) as f64) >= limit
}

fn check_pattern(schema: &Value, value: &Value) -> bool {
    let Some(pattern) = schema.get("pattern").and_then(Value::as_str) else { return true };
    regex_test(pattern, value.as_str().unwrap_or(""))
}

fn check_exclusive_maximum(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("exclusiveMaximum").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    value.as_f64().unwrap_or(0.0) < limit
}

fn check_exclusive_minimum(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("exclusiveMinimum").and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY);
    value.as_f64().unwrap_or(0.0) > limit
}

fn check_maximum(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("maximum").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    value.as_f64().unwrap_or(0.0) <= limit
}

fn check_minimum(schema: &Value, value: &Value) -> bool {
    let limit = schema.get("minimum").and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY);
    value.as_f64().unwrap_or(0.0) >= limit
}

fn check_const(schema: &Value, value: &Value) -> bool {
    deep_equal(schema.get("const").unwrap_or(&Value::Null), value)
}

fn check_enum(schema: &Value, value: &Value) -> bool {
    let Some(options) = schema.get("enum").and_then(Value::as_array) else { return true };
    options.iter().any(|option| deep_equal(option, value))
}

fn check_all_of(schema: &Value, value: &Value) -> bool {
    let Some(branches) = schema.get("allOf").and_then(Value::as_array) else { return true };
    branches.iter().all(|branch| check_schema(branch, value))
}

fn check_any_of(schema: &Value, value: &Value) -> bool {
    let Some(branches) = schema.get("anyOf").and_then(Value::as_array) else { return true };
    branches.iter().any(|branch| check_schema(branch, value))
}

fn check_one_of(schema: &Value, value: &Value) -> bool {
    let Some(branches) = schema.get("oneOf").and_then(Value::as_array) else { return true };
    branches.iter().filter(|branch| check_schema(branch, value)).count() == 1
}

// ============================================================================
// Errors (typebox `ErrorSchema`)
// ============================================================================

/// `Compile(schema).Errors(value)` — the ordered localized error list.
pub fn schema_errors(schema: &Value, value: &Value) -> Vec<LocalizedError> {
    let mut context = ErrorContext::default();
    error_schema(schema, value, "#", "", &mut context);
    context.errors
}

/// One keyword step of the typebox `ErrorSchema` traversal. Returns whether
/// the value is valid per this schema; every applicable keyword check runs
/// (the TS `&` chain is non-short-circuiting) so error order is exact.
fn error_schema(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    if context.at_capacity() {
        return false;
    }
    if schema.is_boolean() {
        return if schema == &Value::Bool(true) {
            true
        } else {
            context.add_error("boolean", schema_path, instance_path, serde_json::json!({}))
        };
    }

    // The TS traversal chains keyword checks with non-short-circuiting `&`,
    // so every applicable check runs and errors append in keyword order.
    let mut result = true;
    let record = |ok: bool, result: &mut bool| {
        *result = *result && ok;
    };

    if is_type(schema) {
        let ok = check_type(schema, value);
        if !ok {
            context.add_error(
                "type",
                schema_path,
                instance_path,
                serde_json::json!({ "type": schema.get("type").cloned().unwrap_or(Value::Null) }),
            );
        }
        record(ok, &mut result);
    }

    if is_object_not_array(value) {
        if is_required(schema) {
            let ok = check_required(schema, value);
            if !ok {
                let missing: Vec<Value> = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|keys| {
                        keys.iter()
                            .filter(|k| !value.as_object().unwrap().contains_key(k.as_str().unwrap_or("")))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                context.add_error(
                    "required",
                    schema_path,
                    instance_path,
                    serde_json::json!({ "requiredProperties": missing }),
                );
            }
            record(ok, &mut result);
        }

        if is_additional_properties(schema) {
            let ok = error_additional_properties(schema, value, schema_path, instance_path, context);
            record(ok, &mut result);
        }

        if is_properties(schema) {
            let ok = error_properties(schema, value, schema_path, instance_path, context);
            record(ok, &mut result);
        }

        for keyword in ["minProperties", "maxProperties"] {
            if is_number_keyword(schema, keyword) {
                let ok = match keyword {
                    "minProperties" => check_min_properties(schema, value),
                    _ => check_max_properties(schema, value),
                };
                if !ok {
                    context.add_error(
                        keyword,
                        schema_path,
                        instance_path,
                        serde_json::json!({ "limit": schema.get(keyword).cloned().unwrap_or(Value::Null) }),
                    );
                }
                record(ok, &mut result);
            }
        }
    }

    if value.is_array() {
        if is_items(schema) {
            let ok = error_items(schema, value, schema_path, instance_path, context);
            record(ok, &mut result);
        }

        for keyword in ["minItems", "maxItems"] {
            if is_number_keyword(schema, keyword) {
                let ok = match keyword {
                    "minItems" => check_min_items(schema, value),
                    _ => check_max_items(schema, value),
                };
                if !ok {
                    context.add_error(
                        keyword,
                        schema_path,
                        instance_path,
                        serde_json::json!({ "limit": schema.get(keyword).cloned().unwrap_or(Value::Null) }),
                    );
                }
                record(ok, &mut result);
            }
        }

        if is_unique_items(schema) && schema.get("uniqueItems") == Some(&Value::Bool(true)) {
            let ok = check_unique_items(schema, value);
            if !ok {
                context.add_error("uniqueItems", schema_path, instance_path, serde_json::json!({}));
            }
            record(ok, &mut result);
        }
    }

    if value.is_string() {
        for keyword in ["maxLength", "minLength"] {
            if is_number_keyword(schema, keyword) {
                let ok = match keyword {
                    "maxLength" => check_max_length(schema, value),
                    _ => check_min_length(schema, value),
                };
                if !ok {
                    context.add_error(
                        keyword,
                        schema_path,
                        instance_path,
                        serde_json::json!({ "limit": schema.get(keyword).cloned().unwrap_or(Value::Null) }),
                    );
                }
                record(ok, &mut result);
            }
        }

        if has_key(schema, "pattern") {
            let ok = check_pattern(schema, value);
            if !ok {
                context.add_error(
                    "pattern",
                    schema_path,
                    instance_path,
                    serde_json::json!({ "pattern": schema.get("pattern").cloned().unwrap_or(Value::Null) }),
                );
            }
            record(ok, &mut result);
        }
    }

    if value.is_number() {
        for (keyword, comparison) in [
            ("exclusiveMaximum", "<"),
            ("exclusiveMinimum", ">"),
            ("maximum", "<="),
            ("minimum", ">="),
        ] {
            if is_number_keyword(schema, keyword) {
                let ok = match keyword {
                    "exclusiveMaximum" => check_exclusive_maximum(schema, value),
                    "exclusiveMinimum" => check_exclusive_minimum(schema, value),
                    "maximum" => check_maximum(schema, value),
                    _ => check_minimum(schema, value),
                };
                if !ok {
                    context.add_error(
                        keyword,
                        schema_path,
                        instance_path,
                        serde_json::json!({ "comparison": comparison, "limit": schema.get(keyword).cloned().unwrap_or(Value::Null) }),
                    );
                }
                record(ok, &mut result);
            }
        }
    }

    if is_const(schema) {
        let ok = check_const(schema, value);
        if !ok {
            context.add_error(
                "const",
                schema_path,
                instance_path,
                serde_json::json!({ "allowedValue": schema.get("const").cloned().unwrap_or(Value::Null) }),
            );
        }
        record(ok, &mut result);
    }

    if is_enum(schema) {
        let ok = check_enum(schema, value);
        if !ok {
            context.add_error(
                "enum",
                schema_path,
                instance_path,
                serde_json::json!({ "allowedValues": schema.get("enum").cloned().unwrap_or(Value::Null) }),
            );
        }
        record(ok, &mut result);
    }

    if is_all_of(schema) {
        let ok = error_all_of(schema, value, schema_path, instance_path, context);
        record(ok, &mut result);
    }

    if is_any_of(schema) {
        let ok = error_any_of(schema, value, schema_path, instance_path, context);
        record(ok, &mut result);
    }

    if is_one_of(schema) {
        let ok = error_one_of(schema, value, schema_path, instance_path, context);
        record(ok, &mut result);
    }

    result
}

fn error_properties(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return true };
    let obj = value.as_object().expect("properties only checked for objects");
    let mut result = true;
    for (key, prop_schema) in props {
        if let Some(item) = obj.get(key) {
            let next_schema_path = format!("{schema_path}/properties/{key}");
            let next_instance_path = format!("{instance_path}/{key}");
            let ok = error_schema(prop_schema, item, &next_schema_path, &next_instance_path, context);
            result = result && ok;
        }
    }
    result
}

fn error_additional_properties(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    let pattern = properties_pattern(schema);
    let ap = schema.get("additionalProperties").unwrap();
    let Some(obj) = value.as_object() else { return true };
    let mut additional: Vec<Value> = Vec::new();
    let mut result = true;
    for (key, item) in obj {
        let next_schema_path = format!("{schema_path}/additionalProperties");
        let next_instance_path = format!("{instance_path}/{key}");
        let ok = regex_test(&pattern, key)
            || error_schema(ap, item, &next_schema_path, &next_instance_path, context);
        if !ok {
            additional.push(Value::String(key.clone()));
        }
        result = result && ok;
    }
    if !result {
        return context.add_error(
            "additionalProperties",
            schema_path,
            instance_path,
            serde_json::json!({ "additionalProperties": additional }),
        );
    }
    result
}

fn error_items(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    let items = schema.get("items").unwrap();
    let Some(arr) = value.as_array() else { return true };
    let mut result = true;
    if is_items_sized(schema) {
        let Some(schemas) = items.as_array() else { return true };
        for (index, item_schema) in schemas.iter().enumerate() {
            if index >= arr.len() {
                continue;
            }
            let next_schema_path = format!("{schema_path}/items/{index}");
            let next_instance_path = format!("{instance_path}/{index}");
            let ok = error_schema(item_schema, &arr[index], &next_schema_path, &next_instance_path, context);
            result = result && ok;
        }
    } else {
        for (index, element) in arr.iter().enumerate() {
            let next_schema_path = format!("{schema_path}/items");
            let next_instance_path = format!("{instance_path}/{index}");
            let ok = error_schema(items, element, &next_schema_path, &next_instance_path, context);
            result = result && ok;
        }
    }
    result
}

fn error_all_of(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    let Some(branches) = schema.get("allOf").and_then(Value::as_array) else { return true };
    let mut failed: Vec<ErrorContext> = Vec::new();
    let mut passing = 0usize;
    for (index, branch) in branches.iter().enumerate() {
        let next_schema_path = format!("{schema_path}/allOf/{index}");
        let mut next_context = ErrorContext::default();
        let ok = error_schema(branch, value, &next_schema_path, instance_path, &mut next_context);
        if ok {
            passing += 1;
        } else {
            failed.push(next_context);
        }
    }
    let is_all_of = passing == branches.len();
    if !is_all_of {
        for failed_context in failed {
            for error in failed_context.errors {
                context.add_error(&error.keyword, &error.schema_path, &error.instance_path, error.params);
            }
        }
    }
    is_all_of
}

fn error_any_of(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    let Some(branches) = schema.get("anyOf").and_then(Value::as_array) else { return true };
    let mut failed: Vec<ErrorContext> = Vec::new();
    let mut passing = 0usize;
    for (index, branch) in branches.iter().enumerate() {
        let next_schema_path = format!("{schema_path}/anyOf/{index}");
        let mut next_context = ErrorContext::default();
        let ok = error_schema(branch, value, &next_schema_path, instance_path, &mut next_context);
        if ok {
            passing += 1;
        } else {
            failed.push(next_context);
        }
    }
    let is_any_of = passing > 0;
    if !is_any_of {
        for failed_context in failed {
            for error in failed_context.errors {
                context.add_error(&error.keyword, &error.schema_path, &error.instance_path, error.params);
            }
        }
        return context.add_error("anyOf", schema_path, instance_path, serde_json::json!({}));
    }
    is_any_of
}

fn error_one_of(schema: &Value, value: &Value, schema_path: &str, instance_path: &str, context: &mut ErrorContext) -> bool {
    let Some(branches) = schema.get("oneOf").and_then(Value::as_array) else { return true };
    let mut failed: Vec<ErrorContext> = Vec::new();
    let mut passing_schemas: Vec<Value> = Vec::new();
    for (index, branch) in branches.iter().enumerate() {
        let next_schema_path = format!("{schema_path}/oneOf/{index}");
        let mut next_context = ErrorContext::default();
        let ok = error_schema(branch, value, &next_schema_path, instance_path, &mut next_context);
        if ok {
            passing_schemas.push(serde_json::json!(index));
        } else {
            failed.push(next_context);
        }
    }
    let is_one_of = passing_schemas.len() == 1;
    if !is_one_of && passing_schemas.is_empty() {
        for failed_context in failed {
            for error in failed_context.errors {
                context.add_error(&error.keyword, &error.schema_path, &error.instance_path, error.params);
            }
        }
    }
    is_one_of
        || context.add_error(
            "oneOf",
            schema_path,
            instance_path,
            serde_json::json!({ "passingSchemas": passing_schemas }),
        )
}

// ============================================================================
// validation.ts helpers
// ============================================================================

/// `formatValidationPath` — dotted instance path; `required` errors point at
/// the first missing property; empty base renders as `root`.
fn format_validation_path(error: &LocalizedError) -> String {
    let base = error
        .instance_path
        .strip_prefix('/')
        .unwrap_or(&error.instance_path)
        .replace('/', ".");
    if error.keyword == "required" {
        let first = error
            .params
            .get("requiredProperties")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return if base.is_empty() { first } else { format!("{base}.{first}") };
    }
    if base.is_empty() { "root".to_string() } else { base }
}

/// `normalizeOptionalNulls` — drop null values for optional properties whose
/// schema rejects null.
fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    if let Value::Array(items) = value {
        if let Some(Value::Array(item_schemas)) = schema.get("items") {
            for (index, item) in items.iter_mut().enumerate() {
                if let Some(item_schema) = item_schemas.get(index) {
                    normalize_optional_nulls(item, item_schema);
                }
            }
        } else if let Some(item_schema) = schema.get("items") {
            for item in items.iter_mut() {
                normalize_optional_nulls(item, item_schema);
            }
        }
        return;
    }
    let Some(obj) = value.as_object_mut() else { return };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else { return };
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    for (key, property_schema) in properties {
        let Some(item) = obj.get_mut(key) else { continue };
        let is_null = item.is_null();
        let rejects_null = check_schema(property_schema, &Value::Null) == false;
        let has_ref = property_schema.get("$ref").is_some_and(Value::is_string);
        if is_null && !required.contains(key) && !has_ref && rejects_null {
            obj.shift_remove(key);
        } else {
            normalize_optional_nulls(item, property_schema);
        }
    }
}

/// `getSchemaTypes` — the string forms of the `type` keyword.
fn get_schema_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(types)) => {
            types.iter().filter_map(Value::as_str).map(str::to_string).collect()
        }
        _ => Vec::new(),
    }
}

/// `matchesJsonType`.
fn matches_json_type(value: &Value, type_name: &str) -> bool {
    match type_name {
        "number" => value.is_number(),
        "integer" => value.is_number() && is_integer(value),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

/// `coercePrimitiveByType`.
fn coerce_primitive_by_type(value: &Value, type_name: &str) -> Value {
    match type_name {
        "number" => {
            if value.is_null() {
                return serde_json::json!(0);
            }
            if let Value::String(s) = value {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    let parsed = js_to_number(trimmed);
                    if parsed.is_finite() {
                        return pi_core::ai::js_number(parsed);
                    }
                }
            }
            if let Value::Bool(b) = value {
                return serde_json::json!(if *b { 1 } else { 0 });
            }
            value.clone()
        }
        "integer" => {
            if value.is_null() {
                return serde_json::json!(0);
            }
            if let Value::String(s) = value {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    let parsed = js_to_number(trimmed);
                    if parsed.is_finite() && parsed.fract() == 0.0 {
                        return pi_core::ai::js_number(parsed);
                    }
                }
            }
            if let Value::Bool(b) = value {
                return serde_json::json!(if *b { 1 } else { 0 });
            }
            value.clone()
        }
        "boolean" => {
            if value.is_null() {
                return serde_json::json!(false);
            }
            if let Value::String(s) = value {
                if s == "true" {
                    return serde_json::json!(true);
                }
                if s == "false" {
                    return serde_json::json!(false);
                }
            }
            if let Some(n) = value.as_f64() {
                if n == 1.0 {
                    return serde_json::json!(true);
                }
                if n == 0.0 {
                    return serde_json::json!(false);
                }
            }
            value.clone()
        }
        "string" => {
            if value.is_null() {
                return serde_json::json!("");
            }
            match value {
                Value::Number(n) => Value::String(js_number_to_string(n.as_f64().unwrap_or(0.0))),
                Value::Bool(b) => Value::String(if *b { "true" } else { "false" }.to_string()),
                _ => value.clone(),
            }
        }
        "null" => {
            if value == &Value::String(String::new()) || value == &serde_json::json!(0) || value == &Value::Bool(false) {
                return Value::Null;
            }
            value.clone()
        }
        _ => value.clone(),
    }
}

/// `coerceWithUnionSchema` — returns a replacement when a coerced clone
/// validates against a branch (a different reference in TS).
fn coerce_with_union_schema(value: &mut Value, schemas: &[Value]) -> Option<Value> {
    for branch in schemas {
        if check_schema(branch, value) {
            return None;
        }
    }
    for branch in schemas {
        let mut candidate = value.clone();
        let coerced = match coerce_with_json_schema(&mut candidate, branch) {
            Some(replacement) => replacement,
            None => candidate,
        };
        if check_schema(branch, &coerced) {
            return Some(coerced);
        }
    }
    None
}

/// `applySchemaObjectCoercion` — in-place property coercion.
fn apply_schema_object_coercion(value: &mut Map<String, Value>, schema: &Value) {
    let properties = schema.get("properties").and_then(Value::as_object);
    let defined_keys: std::collections::HashSet<&String> = properties
        .map(|p| p.keys().collect())
        .unwrap_or_default();
    if let Some(properties) = properties {
        for (key, property_schema) in properties {
            if !value.contains_key(key) {
                continue;
            }
            if let Some(item) = value.get_mut(key) {
                let coerced = match coerce_with_json_schema(item, property_schema) {
                    Some(replacement) => replacement,
                    None => item.clone(),
                };
                *item = coerced;
            }
        }
    }
    if let Some(ap_schema) = schema.get("additionalProperties") {
        if ap_schema.is_object() {
            for (key, item) in value.iter_mut() {
                if defined_keys.contains(key) {
                    continue;
                }
                let coerced = match coerce_with_json_schema(item, ap_schema) {
                    Some(replacement) => replacement,
                    None => item.clone(),
                };
                *item = coerced;
            }
        }
    }
}

/// `applySchemaArrayCoercion` — in-place item coercion.
fn apply_schema_array_coercion(value: &mut [Value], schema: &Value) {
    if let Some(Value::Array(item_schemas)) = schema.get("items") {
        for (index, item) in value.iter_mut().enumerate() {
            if let Some(item_schema) = item_schemas.get(index) {
                let coerced = match coerce_with_json_schema(item, item_schema) {
                    Some(replacement) => replacement,
                    None => item.clone(),
                };
                *item = coerced;
            }
        }
        return;
    }
    if let Some(item_schema) = schema.get("items") {
        if is_schema(item_schema) && !item_schema.is_array() {
            for item in value.iter_mut() {
                let coerced = match coerce_with_json_schema(item, item_schema) {
                    Some(replacement) => replacement,
                    None => item.clone(),
                };
                *item = coerced;
            }
        }
    }
}

/// `coerceWithJsonSchema`. Mutates `value` in place (object/array forms keep
/// the same reference in TS); returns `Some(replacement)` when the TS call
/// would return a different reference than its input (primitive replacement
/// or a union-coerced clone).
fn coerce_with_json_schema(value: &mut Value, schema: &Value) -> Option<Value> {
    let mut next_value = value.clone();
    let mut replaced = false;

    if let Some(Value::Array(nested)) = schema.get("allOf") {
        for branch in nested {
            if let Some(replacement) = coerce_with_json_schema(&mut next_value, branch) {
                next_value = replacement;
                replaced = true;
            }
        }
    }

    if let Some(Value::Array(branches)) = schema.get("anyOf") {
        if let Some(coerced) = coerce_with_union_schema(&mut next_value, branches) {
            next_value = coerced;
            replaced = true;
        }
    }

    if let Some(Value::Array(branches)) = schema.get("oneOf") {
        if let Some(coerced) = coerce_with_union_schema(&mut next_value, branches) {
            next_value = coerced;
            replaced = true;
        }
    }

    let schema_types = get_schema_types(schema);
    let matches_union_member = schema_types.len() > 1
        && schema_types.iter().any(|t| matches_json_type(&next_value, t));
    if !schema_types.is_empty() && !matches_union_member {
        for schema_type in &schema_types {
            let candidate = coerce_primitive_by_type(&next_value, schema_type);
            if candidate != next_value {
                next_value = candidate;
                replaced = true;
                break;
            }
        }
    }

    if schema_types.iter().any(|t| t == "object") && next_value.is_object() {
        if let Value::Object(map) = &mut next_value {
            apply_schema_object_coercion(map, schema);
        }
    }

    if schema_types.iter().any(|t| t == "array") && next_value.is_array() {
        if let Value::Array(items) = &mut next_value {
            apply_schema_array_coercion(items, schema);
        }
    }

    *value = next_value;
    if replaced { Some(value.clone()) } else { None }
}

// ============================================================================
// validateToolArguments
// ============================================================================

/// TS `validateToolArguments(tool, toolCall)` — validate (and coerce) tool
/// call arguments against the tool's schema. `Err` carries the byte-exact
/// thrown error message.
pub fn validate_tool_arguments(tool_name: &str, parameters: &Value, tool_call: &Value) -> Result<Value, String> {
    // `if (toolCall.argumentsError)` — truthy check.
    if let Some(arguments_error) = tool_call.get("argumentsError") {
        let falsy = arguments_error.is_null()
            || arguments_error == &Value::Bool(false)
            || arguments_error == &Value::String(String::new())
            || arguments_error == &serde_json::json!(0)
            || arguments_error == &serde_json::json!(0.0);
        if !falsy {
            let message = arguments_error.as_str().map(str::to_string).unwrap_or_else(|| {
                js_number_to_string(arguments_error.as_f64().unwrap_or(f64::NAN))
            });
            return Err(format!("Invalid arguments for tool \"{tool_name}\": {message}"));
        }
    }

    let mut args = tool_call.get("arguments").cloned().unwrap_or(Value::Null);
    normalize_optional_nulls(&mut args, parameters);
    // Value.Convert: no-op for plain JSON schemas (see module docs).

    // Plain schemas (no TypeBox `~kind` symbol) take the coercion fallback.
    if let Some(coerced) = coerce_with_json_schema(&mut args, parameters) {
        if !(args.is_object() && coerced.is_object()) {
            return if check_schema(parameters, &coerced) {
                Ok(coerced)
            } else {
                Ok(args)
            };
        }
        // Both objects: replace args' contents with coerced's (key order
        // from coerced).
        args = coerced;
    }

    if check_schema(parameters, &args) {
        return Ok(args);
    }

    let errors = schema_errors(parameters, &args)
        .iter()
        .map(|error| format!("  - {}: {}", format_validation_path(error), locale_message(&error.keyword, &error.params)))
        .collect::<Vec<_>>()
        .join("\n");
    let errors = if errors.is_empty() { "Unknown validation error".to_string() } else { errors };

    let received = js_stringify_pretty(tool_call.get("arguments").unwrap_or(&Value::Null));
    Err(format!(
        "Validation failed for tool \"{tool_name}\":\n{errors}\n\nReceived arguments:\n{received}"
    ))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn check_required_and_type() {
        let schema = json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] });
        assert!(!check_schema(&schema, &json!({})));
        let errors = schema_errors(&schema, &json!({}));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].keyword, "required");
        assert_eq!(errors[0].params["requiredProperties"], json!(["path"]));
        assert_eq!(locale_message("required", &errors[0].params), "must have required properties path");

        assert!(!check_schema(&schema, &json!({ "path": 5 })));
        let errors = schema_errors(&schema, &json!({ "path": 5 }));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].instance_path, "/path");
        assert_eq!(locale_message("type", &errors[0].params), "must be string");
        assert_eq!(format_validation_path(&errors[0]), "path");
    }

    #[test]
    fn required_error_path_points_at_first_missing_property() {
        let schema = json!({ "type": "object", "required": ["a", "b"] });
        let errors = schema_errors(&schema, &json!({}));
        assert_eq!(errors.len(), 1);
        assert_eq!(format_validation_path(&errors[0]), "a");
        assert_eq!(locale_message("required", &errors[0].params), "must have required properties a, b");
    }

    #[test]
    fn validate_error_message_is_byte_exact() {
        let schema = json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] });
        // {path: 5} is coerced to "5" (validation.ts coercePrimitiveByType string case) and validates.
        let ok = validate_tool_arguments("read", &schema, &json!({ "id": "c1", "name": "read", "arguments": { "path": 5 } })).unwrap();
        assert_eq!(ok, json!({ "path": "5" }));
        // No coercion path applies to an array where an object is required.
        let err = validate_tool_arguments("read", &schema, &json!({ "id": "c1", "name": "read", "arguments": [] })).unwrap_err();
        assert_eq!(
            err,
            "Validation failed for tool \"read\":\n  - root: must be object\n\nReceived arguments:\n[]"
        );
    }

    #[test]
    fn arguments_error_short_circuits() {
        let tool_call = json!({ "name": "read", "arguments": {}, "argumentsError": "SyntaxError: Unexpected end of JSON input" });
        let err = validate_tool_arguments("read", &json!({ "type": "object" }), &tool_call).unwrap_err();
        assert_eq!(err, "Invalid arguments for tool \"read\": SyntaxError: Unexpected end of JSON input");
    }

    #[test]
    fn nested_error_paths() {
        let schema = json!({
            "type": "object",
            "properties": { "items": { "type": "array", "items": { "type": "string" } } }
        });
        let errors = schema_errors(&schema, &json!({ "items": ["ok", 3] }));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].instance_path, "/items/1");
        assert_eq!(format_validation_path(&errors[0]), "items.1");
    }

    #[test]
    fn normalize_optional_nulls_drops_nulls_for_non_null_schemas() {
        let schema = json!({
            "type": "object",
            "properties": { "a": { "type": "string" }, "b": { "type": ["string", "null"] } },
            "required": ["b"]
        });
        let mut value = json!({ "a": null, "b": null });
        normalize_optional_nulls(&mut value, &schema);
        assert_eq!(value, json!({ "b": null }));
    }

    #[test]
    fn coercion_converts_primitives() {
        let schema = json!({
            "type": "object",
            "properties": { "count": { "type": "integer" }, "flag": { "type": "boolean" } }
        });
        let tool_call = json!({ "name": "t", "arguments": { "count": "7", "flag": "true" } });
        let args = validate_tool_arguments("t", &schema, &tool_call).unwrap();
        assert_eq!(args, json!({ "count": 7, "flag": true }));
    }

    #[test]
    fn empty_object_schema_accepts_any_object() {
        let tool_call = json!({ "name": "t", "arguments": { "x": 1 } });
        assert!(validate_tool_arguments("t", &json!({ "type": "object" }), &tool_call).is_ok());
    }

    #[test]
    fn any_of_errors_merge_branch_errors() {
        let schema = json!({
            "anyOf": [ { "type": "object", "properties": { "a": { "type": "string" } }, "required": ["a"] },
                       { "type": "object", "properties": { "b": { "type": "string" } }, "required": ["b"] } ]
        });
        let errors = schema_errors(&schema, &json!({}));
        // Both branch errors merge, then the anyOf error.
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[2].keyword, "anyOf");
        assert_eq!(locale_message("anyOf", &errors[2].params), "must match a schema in anyOf");
    }
}
