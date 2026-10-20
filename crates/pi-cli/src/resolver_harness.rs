//! Parity-harness entries for the config/resolution wave tool:
//! `model-resolver`.
//!
//! Wire protocol is documented in `docs/plans/phase3-providers.md` §5 and
//! `/home/shinobi/wave2-spec.md` (tool 3). The TS side reduces the
//! `ModelRuntime` to `models` / `availableModels` / `configuredAuth` args
//! (deterministic, no env); this harness does the same.

use serde_json::Value;

use pi_providers::model_resolver::{
	find_exact_model_reference_match, find_initial_model, resolve_cli_model,
	resolve_model_reference, resolve_model_scope_from_models, restore_model_from_session,
	FindInitialModelOptions, ModelReferenceRequest, ResolveCliModelOptions, RestoreModelOptions,
	ScopedModel, ThinkingSuffix,
};
use pi_providers::pi_ai::model::Model;

/// Parse the full chat-model JSON (`msModel` shape) into a `Model`.
/// Round-trips key order via `js_object!`.
fn parse_model(v: &Value) -> Result<Model, String> {
	let map = v
		.as_object()
		.ok_or_else(|| format!("model entry is not an object: {v}"))?
		.clone();
	Model::from_map(map).map_err(|e| format!("invalid model entry: {e}"))
}

fn parse_models(input: &Value) -> Result<Vec<Model>, String> {
	input
		.get("models")
		.and_then(Value::as_array)
		.ok_or("missing models array")?
		.iter()
		.map(parse_model)
		.collect()
}

fn parse_scoped_models(input: &Value, key: &str) -> Result<Vec<ScopedModel>, String> {
	input
		.get(key)
		.and_then(Value::as_array)
		.ok_or_else(|| format!("missing {key} array"))?
		.iter()
		.map(|entry| {
			let model = parse_model(entry.get("model").ok_or("scopedModel missing model")?)?;
			let thinking_level = entry
				.get("thinkingLevel")
				.and_then(Value::as_str)
				.map(|s| s.to_string());
			Ok(ScopedModel { model, thinking_level })
		})
		.collect()
}

fn parse_thinking_suffix(v: Option<&Value>) -> Result<ThinkingSuffix, String> {
	match v.and_then(Value::as_str) {
		Some("none") => Ok(ThinkingSuffix::None),
		Some("strict") => Ok(ThinkingSuffix::Strict),
		Some("lenient") => Ok(ThinkingSuffix::Lenient),
		other => Err(format!("invalid thinkingSuffix: {other:?}")),
	}
}

fn opt_str<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
	input.get(key).and_then(Value::as_str)
}

fn str_list(input: &Value, key: &str) -> Vec<String> {
	input
		.get(key)
		.and_then(Value::as_array)
		.map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect())
		.unwrap_or_default()
}

/// Dispatch a model-resolver harness tool.
pub fn run(tool: &str, input: &Value) -> Result<Value, String> {
	if tool != "model-resolver" {
		return Err(format!("resolver_harness: unknown tool {tool:?}"));
	}
	let args = input;
	let fn_name = args.get("fn").and_then(Value::as_str).ok_or("missing fn")?;
	let models = parse_models(args)?;
	let configured_auth = str_list(args, "configuredAuth");
	match fn_name {
		"resolveModelReference" => {
			let request = ModelReferenceRequest {
				text: args
					.get("text")
					.and_then(Value::as_str)
					.ok_or("missing text")?
					.to_string(),
				provider: opt_str(args, "provider").map(|s| s.to_string()),
				thinking_suffix: parse_thinking_suffix(args.get("thinkingSuffix"))?,
				fuzzy: args.get("fuzzy").and_then(Value::as_bool).unwrap_or(false),
			};
			let usable: Vec<String> = str_list(args, "usable");
			let result = resolve_model_reference(&request, &models, &|p| usable.iter().any(|u| u == p));
			Ok(json!({ "result": result.to_value() }))
		}
		"findExactModelReferenceMatch" => {
			let model_reference = args
				.get("modelReference")
				.and_then(Value::as_str)
				.ok_or("missing modelReference")?;
			let result = find_exact_model_reference_match(model_reference, &models);
			Ok(json!({ "result": result.as_ref().map(Model::to_value).unwrap_or(Value::Null) }))
		}
		"resolveModelScopeFromModels" => {
			let patterns: Vec<String> = args
				.get("patterns")
				.and_then(Value::as_array)
				.ok_or("missing patterns array")?
				.iter()
				.map(|p| {
					p.as_str()
						.map(|s| s.to_string())
						.ok_or_else(|| format!("pattern is not a string: {p}"))
				})
				.collect::<Result<_, _>>()?;
			let result = resolve_model_scope_from_models(&patterns, &models);
			Ok(json!({ "result": result.to_value() }))
		}
		"resolveCliModel" => {
			let options = ResolveCliModelOptions {
				cli_provider: opt_str(args, "cliProvider"),
				cli_model: opt_str(args, "cliModel"),
				cli_thinking: opt_str(args, "cliThinking"),
				inline_api_key: args.get("inlineApiKey").and_then(Value::as_bool).unwrap_or(false),
				models: &models,
				has_configured_auth: &|p| configured_auth.iter().any(|c| c == p),
			};
			let result = resolve_cli_model(&options);
			Ok(json!({ "result": result.to_value() }))
		}
		"findInitialModel" => {
			let scoped_models = parse_scoped_models(args, "scopedModels")?;
			let model_thinking_levels_map: std::collections::HashMap<String, String> = args
				.get("modelThinkingLevels")
				.and_then(Value::as_object)
				.map(|o| {
					o.iter()
						.filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
						.collect()
				})
				.unwrap_or_default();
			let lookup = |key: &str| model_thinking_levels_map.get(key).cloned();
			let available_models: Vec<Model> = args
				.get("availableModels")
				.and_then(Value::as_array)
				.ok_or("missing availableModels array")?
				.iter()
				.map(parse_model)
				.collect::<Result<_, _>>()?;
			let options = FindInitialModelOptions {
				scoped_models: &scoped_models,
				is_continuing: args.get("isContinuing").and_then(Value::as_bool).unwrap_or(false),
				default_provider: opt_str(args, "defaultProvider"),
				default_model_id: opt_str(args, "defaultModelId"),
				default_thinking_level: opt_str(args, "defaultThinkingLevel"),
				model_thinking_levels: Some(&lookup),
				models: &models,
				available_models: &available_models,
				configured_auth: &configured_auth,
			};
			let result = find_initial_model(&options);
			Ok(json!({ "result": result.to_value() }))
		}
		"restoreModelFromSession" => {
			let current_model = match args.get("currentModel") {
				None | Some(Value::Null) => None,
				Some(v) => Some(parse_model(v)?),
			};
			let available_models: Vec<Model> = args
				.get("availableModels")
				.and_then(Value::as_array)
				.ok_or("missing availableModels array")?
				.iter()
				.map(parse_model)
				.collect::<Result<_, _>>()?;
			let options = RestoreModelOptions {
				saved_provider: args
					.get("savedProvider")
					.and_then(Value::as_str)
					.ok_or("missing savedProvider")?,
				saved_model_id: args
					.get("savedModelId")
					.and_then(Value::as_str)
					.ok_or("missing savedModelId")?,
				current_model: current_model.as_ref(),
				models: &models,
				available_models: &available_models,
				configured_auth: &configured_auth,
			};
			let result = restore_model_from_session(&options);
			Ok(json!({ "result": result.to_value() }))
		}
		other => Err(format!("unknown model-resolver fn: {other}")),
	}
}

use serde_json::json;
