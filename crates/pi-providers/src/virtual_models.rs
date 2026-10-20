//! Virtual models — port of `coding-agent` `src/core/virtual-models.ts`.
//!
//! Virtual models are catalog entries that route each request to a
//! physical model. Everything below the routing step only sees physical
//! models: providers stream them and assistant messages record them.
//!
//! Session entries and messages arrive as raw JSON values (the shapes the
//! pi-session crate persists); the typed agent-message port lands with the
//! agent-session tranches. `withVirtualModels` composes providers; the
//! unrouted-stream error is embedded in the placeholder stream value
//! instead of thrown lazily (TS defers the throw to first event read — a
//! documented divergence that the real stream port replaces).

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::error::JsError;
use crate::pi_ai::credential::{
	ApiKeyAuth, AuthCheck, AuthContext, AuthResult, ModelAuth, ProviderAuth,
};
use crate::pi_ai::model::{Model, ModelInputModality, ModelThinkingLevel};
use crate::pi_ai::models::EXTENDED_THINKING_LEVELS;
use crate::pi_ai::provider::{AssistantMessageEventStream, Provider};
use pi_tools::shell::AbortSignal;

/// API id of virtual catalog entries. Requests for it fail unless routed
/// first.
pub const VIRTUAL_MODEL_API: &str = "pi-virtual";

/// Custom entry type that stores router state on the session branch.
pub const VIRTUAL_MODEL_STATE_ENTRY: &str = "pi.virtual-model-state";

/// Data of a `pi.virtual-model-state` custom entry
/// (`{ provider, modelId, state }`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VirtualModelStateData {
	pub provider: String,
	#[serde(rename = "modelId")]
	pub model_id: String,
	pub state: Value,
}

/// The full thinking-level scale (TS `THINKING_LEVELS`; identical to the
/// models module's `EXTENDED_THINKING_LEVELS`).
pub const THINKING_LEVELS: [ModelThinkingLevel; 7] = EXTENDED_THINKING_LEVELS;

/// Why a request is being routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRouteReason {
	/// First request after a message the user wrote.
	User,
	/// Any other request in the agent loop (after tool results, …).
	Continuation,
	/// Automatic retry after a failed request.
	Retry,
	/// A request outside the agent loop (compaction summary, extension
	/// call).
	Direct,
}

impl ModelRouteReason {
	pub fn as_str(self) -> &'static str {
		match self {
			ModelRouteReason::User => "user",
			ModelRouteReason::Continuation => "continuation",
			ModelRouteReason::Retry => "retry",
			ModelRouteReason::Direct => "direct",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"user" => Some(ModelRouteReason::User),
			"continuation" => Some(ModelRouteReason::Continuation),
			"retry" => Some(ModelRouteReason::Retry),
			"direct" => Some(ModelRouteReason::Direct),
			_ => None,
		}
	}
}

/// The failed request carried by a `retry` (TS `ModelRouteRequest.failed`).
#[derive(Debug, Clone)]
pub struct FailedRouteRequest {
	pub model: Model,
	pub thinking_level: Option<ModelThinkingLevel>,
	/// The failed assistant message (`stopReason`/`errorMessage` live here).
	pub message: Value,
}

/// TS `ModelRouteRequest` — what a router receives for one request.
/// `messages` (and `failed.message`) are raw JSON message values until the
/// typed agent-message port.
#[derive(Clone)]
pub struct ModelRouteRequest {
	/// The selected virtual model.
	pub model: Model,
	/// The selected thinking level; its meaning is up to the router.
	pub thinking_level: ModelThinkingLevel,
	pub reason: ModelRouteReason,
	/// Physical model and thinking level of the latest successful response.
	pub previous: Option<(Model, Option<ModelThinkingLevel>)>,
	/// For `retry`: the failed request, which `messages` no longer contains.
	pub failed: Option<FailedRouteRequest>,
	/// Router state last returned on this session branch (`None` before the
	/// first state and for `direct` requests).
	pub state: Option<Value>,
	/// Conversation for this request, including system messages.
	pub messages: Vec<Value>,
	pub signal: Option<AbortSignal>,
}

/// Physical model and thinking level for one request (TS `ModelRoute`).
#[derive(Debug, Clone)]
pub struct ModelRoute {
	pub model: Model,
	pub thinking_level: ModelThinkingLevel,
	/// New router state, stored on the session branch unless it is
	/// `request.state` itself. Must be JSON-serializable; ignored for
	/// `direct` requests.
	pub state: Option<Value>,
}

/// TS `VirtualModelDefinition` — `route` picks the physical model for one
/// request.
pub struct VirtualModelDefinition {
	/// Provider the virtual model is listed under.
	pub provider: String,
	/// Model id — must not be the id of a physical model of `provider`.
	pub id: String,
	pub name: String,
	/// Thinking levels offered for selection (defaults to `["off"]`).
	pub thinking_levels: Option<Vec<ModelThinkingLevel>>,
	/// Limits shown before the first response (unset = unknown, 0).
	pub context_window: Option<f64>,
	pub max_tokens: Option<f64>,
	/// Input types accepted for selection (defaults to text and images).
	pub input: Option<Vec<ModelInputModality>>,
	/// Pick the physical model and thinking level for one request
	/// (G8: sync).
	pub route: RouteFn,
}

pub type RouteFn = Box<dyn Fn(&ModelRouteRequest) -> Result<ModelRoute, JsError> + Send + Sync>;

// ============================================================================
// Pure functions (over raw session JSON)
// ============================================================================

/// Whether a model or message names a virtual model. Failed routing leaves
/// the virtual model on its message.
pub fn is_virtual_model(api: &str) -> bool {
	api == VIRTUAL_MODEL_API
}

/// [`is_virtual_model`] for a chat model.
pub fn model_is_virtual(model: &Model) -> bool {
	is_virtual_model(&model.api)
}

/// Latest successful response (last assistant message whose `stopReason`
/// is not `error`/`aborted`). Its model is physical: failed or aborted
/// requests, including failed routing, are skipped.
pub fn find_latest_response(messages: &[Value]) -> Option<&Value> {
	messages.iter().rev().find(|message| {
		message.get("role").and_then(Value::as_str) == Some("assistant")
			&& !matches!(
				message.get("stopReason").and_then(Value::as_str),
				Some("error") | Some("aborted")
			)
	})
}

/// The model selection a session branch records. A virtual `model_change`
/// holds until the next `model_change`, because responses name the
/// physical models it routed to; otherwise the latest physical response
/// wins. A virtual model that is no longer registered does not hold, so
/// the selection falls back to the physical model that answered last.
///
/// Only the last `model_change` can hold, so this looks up at most one
/// model in the catalog (`get_model`).
pub fn get_branch_selection(
	branch: &[Value],
	get_model: impl Fn(&str, &str) -> Option<Model>,
) -> Option<(String, String)> {
	for i in (0..branch.len()).rev() {
		let entry = &branch[i];
		match entry.get("type").and_then(Value::as_str) {
			Some("model_change") => {
				return Some((
					entry.get("provider")?.as_str()?.to_string(),
					entry.get("modelId")?.as_str()?.to_string(),
				));
			}
			Some("message") => {
				let message = entry.get("message")?;
				let is_assistant =
					message.get("role").and_then(Value::as_str) == Some("assistant");
				let virtual_message = is_virtual_model(
					message.get("api").and_then(Value::as_str).unwrap_or_default(),
				);
				if !is_assistant || virtual_message {
					continue;
				}
				let response = (
					message.get("provider")?.as_str()?.to_string(),
					message.get("model")?.as_str()?.to_string(),
				);
				let change = find_last_model_change(branch, i);
				// TS: `change && getModel(...) && isVirtualModel(model) ? change : response`
				return match change {
					Some(change) => match get_model(&change.0, &change.1) {
						Some(model) if model_is_virtual(&model) => Some(change),
						_ => Some(response),
					},
					None => Some(response),
				};
			}
			_ => {}
		}
	}
	None
}

fn find_last_model_change(branch: &[Value], before: usize) -> Option<(String, String)> {
	for entry in branch[..before].iter().rev() {
		if entry.get("type").and_then(Value::as_str) == Some("model_change") {
			return Some((
				entry.get("provider")?.as_str()?.to_string(),
				entry.get("modelId")?.as_str()?.to_string(),
			));
		}
	}
	None
}

/// Latest router state a session branch stores for a virtual model.
/// `data.state` may be `null`; a missing `state` field behaves like no
/// state (TS `undefined`).
pub fn get_virtual_model_state(branch: &[Value], provider: &str, model_id: &str) -> Option<Value> {
	for entry in branch.iter().rev() {
		if entry.get("type").and_then(Value::as_str) != Some("custom") {
			continue;
		}
		if entry.get("customType").and_then(Value::as_str) != Some(VIRTUAL_MODEL_STATE_ENTRY) {
			continue;
		}
		if let Some(data) = entry.get("data") {
			if data.get("provider").and_then(Value::as_str) == Some(provider)
				&& data.get("modelId").and_then(Value::as_str) == Some(model_id)
			{
				return data.get("state").cloned();
			}
		}
	}
	None
}

/// Build the catalog entry of a virtual model (TS `createVirtualModel`).
/// Key order matches the TS object literal: id, name, api, provider,
/// baseUrl, reasoning, thinkingLevelMap, input, cost, contextWindow,
/// maxTokens.
pub fn create_virtual_model(definition: &VirtualModelDefinition) -> Model {
	let levels = definition.thinking_levels.clone().unwrap_or_else(|| vec![ModelThinkingLevel::Off]);
	let mut map = serde_json::Map::new();
	for level in THINKING_LEVELS {
		let mapped = if levels.contains(&level) {
			Value::String(level.as_str().to_string())
		} else {
			Value::Null
		};
		map.insert(level.as_str().to_string(), mapped);
	}

	let mut obj = serde_json::Map::new();
	obj.insert("id".into(), json!(definition.id));
	obj.insert("name".into(), json!(definition.name));
	obj.insert("api".into(), json!(VIRTUAL_MODEL_API));
	obj.insert("provider".into(), json!(definition.provider));
	obj.insert("baseUrl".into(), json!(""));
	obj.insert(
		"reasoning".into(),
		json!(levels.iter().any(|l| *l != ModelThinkingLevel::Off)),
	);
	obj.insert("thinkingLevelMap".into(), Value::Object(map));
	obj.insert(
		"input".into(),
		json!(definition
			.input
			.clone()
			.unwrap_or_else(|| vec![ModelInputModality::Text, ModelInputModality::Image])),
	);
	obj.insert(
		"cost".into(),
		json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}),
	);
	obj.insert("contextWindow".into(), json!(definition.context_window.unwrap_or(0.0)));
	obj.insert("maxTokens".into(), json!(definition.max_tokens.unwrap_or(0.0)));
	Model::from_map(obj).expect("create_virtual_model builds a valid model")
}

// ============================================================================
// withVirtualModels
// ============================================================================

/// Keyless auth of a provider that only lists virtual models (TS:
/// `apiKey: { name: "Virtual model", resolve: async () => ({ auth: {},
/// source: "virtual" }) }`).
struct VirtualModelAuth;

impl ApiKeyAuth for VirtualModelAuth {
	fn name(&self) -> &str {
		"Virtual model"
	}

	fn check(
		&self,
		_ctx: &dyn AuthContext,
		_credential: Option<&crate::pi_ai::credential::ApiKeyCredential>,
		_signal: Option<&AbortSignal>,
	) -> Result<Option<AuthCheck>, JsError> {
		Ok(None)
	}

	fn resolve(
		&self,
		_ctx: &dyn AuthContext,
		_credential: Option<&crate::pi_ai::credential::ApiKeyCredential>,
		_signal: Option<&AbortSignal>,
	) -> Result<Option<AuthResult>, JsError> {
		let auth = ModelAuth::from_map(serde_json::Map::new())
			.expect("empty ModelAuth is well-formed");
		Ok(Some(AuthResult { auth, env: None, source: Some("virtual".to_string()) }))
	}
}

/// Placeholder for the TS `unroutedStream` (lazy throw on first event
/// read). The error is embedded in the stream value; the real stream port
/// replaces this with a lazy error stream.
fn unrouted_stream(model: &Model) -> AssistantMessageEventStream {
	AssistantMessageEventStream(json!({
		"error": format!(
			"Virtual model {}/{} must be routed before streaming",
			model.provider, model.id
		)
	}))
}

/// Add virtual models to a provider's catalog. Without a provider, the
/// result is a keyless provider that only lists the virtual models. A
/// virtual model hides a physical chat model with the same id, which a
/// catalog refresh can add after registration. Availability follows the
/// provider's auth.
pub fn with_virtual_models(
	provider_id: &str,
	provider: Option<Provider>,
	virtual_models: Vec<Model>,
) -> Provider {
	with_virtual_models_shared(provider_id, provider.map(Arc::new), virtual_models)
}

/// Shared-base variant of [`with_virtual_models`] for callers (e.g.
/// `ModelRuntime`) that keep their providers behind `Arc`. Added for the
/// model-runtime.ts port; behavior is identical to the by-value entry point.
pub fn with_virtual_models_shared(
	provider_id: &str,
	provider: Option<Arc<Provider>>,
	virtual_models: Vec<Model>,
) -> Provider {
	let virtual_models = Arc::new(virtual_models);
	match provider {
		None => Provider::new(
			provider_id.to_string(),
			provider_id.to_string(),
			ProviderAuth { api_key: Some(Arc::new(VirtualModelAuth)), oauth: None },
			{
				let virtual_models = Arc::clone(&virtual_models);
				Box::new(move || virtual_models.as_ref().clone())
			},
			Box::new(|model, _ctx, _opts| unrouted_stream(model)),
			Box::new(|model, _ctx, _opts| unrouted_stream(model)),
		),
		Some(provider) => {
			let ids: HashSet<String> =
				virtual_models.iter().map(|m| m.id.clone()).collect();

			// `physical` — hide virtual models and physical chat models
			// shadowed by a virtual id.
			let physical_chat = {
				let ids = ids.clone();
				move |models: &[Model]| -> Vec<Model> {
					models
						.iter()
						.filter(|m| !model_is_virtual(m) && !ids.contains(&m.id))
						.cloned()
						.collect()
				}
			};
			let physical_any = {
				let ids = ids.clone();
				move |models: &[crate::pi_ai::model::AnyModel]| -> Vec<crate::pi_ai::model::AnyModel> {
					models
						.iter()
						.filter(|m| {
							!is_virtual_model(m.api())
								&& !(matches!(m, crate::pi_ai::model::AnyModel::Chat(_)) && ids.contains(m.id()))
						})
						.cloned()
						.collect()
				}
			};
			let virtual_any =
				|models: &[crate::pi_ai::model::AnyModel]| -> Vec<crate::pi_ai::model::AnyModel> {
					models
						.iter()
						.filter(|m| is_virtual_model(m.api()))
						.cloned()
						.collect()
				};

			let mut composed = Provider::new(
				provider.id.clone(),
				provider.name.clone(),
				provider.auth.clone(),
				{
					let provider = Arc::clone(&provider);
					let physical_chat = physical_chat.clone();
					let virtual_models = Arc::clone(&virtual_models);
					Box::new(move || {
						let mut models = physical_chat(&provider.get_models());
						models.extend(virtual_models.as_ref().iter().cloned());
						models
					})
				},
				{
					let provider = Arc::clone(&provider);
					Box::new(move |model, context, options| {
						if model_is_virtual(model) {
							unrouted_stream(model)
						} else {
							provider.stream(model, context, options)
						}
					})
				},
				{
					let provider = Arc::clone(&provider);
					Box::new(move |model, context, options| {
						if model_is_virtual(model) {
							unrouted_stream(model)
						} else {
							provider.stream_simple(model, context, options)
						}
					})
				},
			)
			.with_base_url(provider.base_url.clone())
			.with_headers(provider.headers.clone())
			.with_get_all_models({
				let provider = Arc::clone(&provider);
				let physical_any = physical_any.clone();
				let virtual_models = Arc::clone(&virtual_models);
				Box::new(move || {
					let mut models = physical_any(&provider.get_all_models());
					models.extend(virtual_models.as_ref().iter().cloned().map(crate::pi_ai::model::AnyModel::Chat));
					models
				})
			});

			// filterModels: apply the original filter to the physical
			// models, then re-append the incoming virtual ones.
			composed = composed.with_filter_models({
				let provider = Arc::clone(&provider);
				Box::new(move |models, credential| {
					let real = physical_chat(models);
					let mut filtered = provider.filter_models(&real, credential);
					filtered.extend(
						models
							.iter()
							.filter(|m| model_is_virtual(m))
							.cloned()
							.collect::<Vec<_>>(),
					);
					filtered
				})
			});

			// filterAllModels: only composed when the original had one.
			if provider.has_filter_all_models() {
				composed = composed.with_filter_all_models({
					let provider = Arc::clone(&provider);
					Box::new(move |models, credential| {
						let real = physical_any(models);
						let mut filtered = provider.filter_all_models(&real, credential);
						filtered.extend(virtual_any(models));
						filtered
					})
				});
			}

			composed
		}
	}
}

// ============================================================================
// Tests — expectations verified against the TS dist with node
// (vectors in docs/plans/impl-notes-providers-foundation.md)
// ============================================================================

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::provider::TranscriptContext;

	fn msg(role: &str, over: Value) -> Value {
		let mut m = json!({"role": role});
		if let (Some(base), Some(o)) = (m.as_object_mut(), over.as_object()) {
			for (k, v) in o {
				base.insert(k.clone(), v.clone());
			}
		}
		m
	}

	fn mc(provider: &str, model_id: &str) -> Value {
		json!({"type": "model_change", "provider": provider, "modelId": model_id})
	}

	fn amsg(provider: &str, model: &str, api: &str) -> Value {
		json!({"type": "message", "message": msg("assistant", json!({"provider": provider, "model": model, "api": api, "stopReason": "stop"}))})
	}

	fn sel(branch: &[Value]) -> Option<(String, String)> {
		get_branch_selection(branch, |provider, id| {
			// "virt" → virtual, "missing" → not registered, else physical.
			match id {
				"virt" => Some(chat_model(provider, id, "pi-virtual")),
				"missing" => None,
				_ => Some(chat_model(provider, id, "real-api")),
			}
		})
	}

	fn chat_model(provider: &str, id: &str, api: &str) -> Model {
		serde_json::from_value(json!({
			"id": id, "name": id, "api": api, "provider": provider,
			"baseUrl": "", "input": ["text"],
			"cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"reasoning": false, "contextWindow": 0, "maxTokens": 0
		}))
		.unwrap()
	}

	#[test]
	fn is_virtual() {
		assert!(is_virtual_model("pi-virtual"));
		assert!(!is_virtual_model("openai-completions"));
		assert!(!is_virtual_model(""));
	}

	#[test]
	fn find_latest_response_vectors() {
		assert!(find_latest_response(&[]).is_none());
		assert!(find_latest_response(&[msg("user", json!({})), msg("toolResult", json!({}))]).is_none());
		assert_eq!(
			find_latest_response(&[msg("user", json!({})), msg("assistant", json!({"model": "m1", "stopReason": "stop"}))])
				.unwrap()
				.get("model")
				.unwrap(),
			"m1"
		);
		assert_eq!(
			find_latest_response(&[
				msg("assistant", json!({"model": "bad", "stopReason": "error"})),
				msg("assistant", json!({"model": "good", "stopReason": "stop"}))
			]).unwrap().get("model").unwrap(), "good"
		);
		assert_eq!(
			find_latest_response(&[
				msg("assistant", json!({"model": "bad", "stopReason": "aborted"})),
				msg("assistant", json!({"model": "good", "stopReason": "length"}))
			]).unwrap().get("model").unwrap(), "good"
		);
		// "pending" is a successful stop reason for this purpose.
		assert_eq!(
			find_latest_response(&[msg("assistant", json!({"model": "p", "stopReason": "pending"}))])
				.unwrap().get("model").unwrap(), "p"
		);
		assert_eq!(
			find_latest_response(&[
				msg("assistant", json!({"model": "a", "stopReason": "stop"})),
				msg("user", json!({})),
				msg("assistant", json!({"model": "b", "stopReason": "stop"}))
			]).unwrap().get("model").unwrap(), "b"
		);
	}

	#[test]
	fn branch_selection_vectors() {
		// (input, expected) — all node-verified.
		type SelCase<'a> = (Vec<Value>, Option<(&'a str, &'a str)>);
		let cases: Vec<SelCase> = vec![
			(vec![], None),
			(vec![mc("p1", "m1"), mc("p2", "m2")], Some(("p2", "m2"))),
			(vec![amsg("p1", "m1", "real-api")], Some(("p1", "m1"))),
			// change to virtual + physical response → change holds
			(vec![mc("p1", "virt"), amsg("p1", "phys", "real-api")], Some(("p1", "virt"))),
			// change to virtual but unregistered → falls back to response
			(vec![mc("p1", "missing"), amsg("p1", "phys", "real-api")], Some(("p1", "phys"))),
			// change to a real model → response wins
			(vec![mc("p1", "real"), amsg("p1", "phys", "real-api")], Some(("p1", "phys"))),
			// virtual assistant message → skipped, earlier model_change wins
			(vec![mc("p1", "virt"), amsg("p1", "stillvirt", "pi-virtual")], Some(("p1", "virt"))),
			// two changes: last change is real → response wins
			(vec![mc("p1", "virt"), mc("p1", "virt2"), amsg("p1", "phys", "real-api")], Some(("p1", "phys"))),
		];
		for (branch, expected) in cases {
			assert_eq!(sel(&branch), expected.map(|(p, m)| (p.to_string(), m.to_string())), "branch: {branch:?}");
		}
	}

	#[test]
	fn virtual_model_state_vectors() {
		let ce = |custom_type: &str, data: Value| json!({"type": "custom", "customType": custom_type, "data": data});
		assert_eq!(get_virtual_model_state(&[], "p", "m"), None);
		assert_eq!(
			get_virtual_model_state(&[ce("pi.virtual-model-state", json!({"provider": "p", "modelId": "m", "state": {"step": 2}}))], "p", "m"),
			Some(json!({"step": 2}))
		);
		assert_eq!(
			get_virtual_model_state(&[ce("pi.virtual-model-state", json!({"provider": "p", "modelId": "other", "state": 1}))], "p", "m"),
			None
		);
		assert_eq!(
			get_virtual_model_state(&[ce("other.type", json!({"provider": "p", "modelId": "m", "state": 1}))], "p", "m"),
			None
		);
		assert_eq!(
			get_virtual_model_state(&[
				ce("pi.virtual-model-state", json!({"provider": "p", "modelId": "m", "state": "old"})),
				ce("pi.virtual-model-state", json!({"provider": "p", "modelId": "m", "state": "new"})),
			], "p", "m"),
			Some(json!("new"))
		);
		// missing state field → undefined (None); explicit null stays null
		assert_eq!(
			get_virtual_model_state(&[ce("pi.virtual-model-state", json!({"provider": "p", "modelId": "m"}))], "p", "m"),
			None
		);
		assert_eq!(
			get_virtual_model_state(&[ce("pi.virtual-model-state", json!({"provider": "p", "modelId": "m", "state": null}))], "p", "m"),
			Some(Value::Null)
		);
	}

	fn definition(over: impl FnOnce(&mut VirtualModelDefinition)) -> VirtualModelDefinition {
		let mut d = VirtualModelDefinition {
			provider: "p".to_string(),
			id: "v".to_string(),
			name: "V".to_string(),
			thinking_levels: None,
			context_window: None,
			max_tokens: None,
			input: None,
			route: Box::new(|_| unreachable!("create_virtual_model does not call route")),
		};
		over(&mut d);
		d
	}

	#[test]
	fn create_virtual_model_defaults() {
		let m = create_virtual_model(&definition(|_| {}));
		assert_eq!(m.id, "v");
		assert_eq!(m.name, "V");
		assert_eq!(m.api, VIRTUAL_MODEL_API);
		assert_eq!(m.provider, "p");
		assert_eq!(m.base_url, "");
		assert!(!m.reasoning);
		assert_eq!(m.input, vec![ModelInputModality::Text, ModelInputModality::Image]);
		assert_eq!(m.context_window.get(), 0.0);
		assert_eq!(m.max_tokens.get(), 0.0);
		let cost = m.cost.to_value();
		assert_eq!(cost, json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}));
		// thinkingLevelMap: only "off" mapped
		assert_eq!(
			m.thinking_level_map.as_ref().unwrap().get("off"),
			Some(&Some("off".to_string()))
		);
		for level in ["minimal", "low", "medium", "high", "xhigh", "max"] {
			assert_eq!(m.thinking_level_map.as_ref().unwrap().get(level), Some(&None), "{level}");
		}
		// key order matches the TS object literal
		assert_eq!(
			m.to_value().as_object().unwrap().keys().collect::<Vec<_>>(),
			vec!["id", "name", "api", "provider", "baseUrl", "reasoning", "thinkingLevelMap", "input", "cost", "contextWindow", "maxTokens"]
		);
	}

	#[test]
	fn create_virtual_model_custom() {
		let m = create_virtual_model(&definition(|d| {
			d.thinking_levels = Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]);
			d.context_window = Some(1000.0);
			d.max_tokens = Some(50.0);
			d.input = Some(vec![ModelInputModality::Text]);
		}));
		assert!(m.reasoning);
		assert_eq!(m.input, vec![ModelInputModality::Text]);
		assert_eq!(m.context_window.get(), 1000.0);
		assert_eq!(m.max_tokens.get(), 50.0);
		let map = m.thinking_level_map.as_ref().unwrap();
		assert_eq!(map.get("low"), Some(&Some("low".to_string())));
		assert_eq!(map.get("high"), Some(&Some("high".to_string())));
		for level in ["off", "minimal", "medium", "xhigh", "max"] {
			assert_eq!(map.get(level), Some(&None), "{level}");
		}
	}

	#[test]
	fn with_virtual_models_without_provider() {
		let provider = with_virtual_models(
			"p",
			None,
			vec![create_virtual_model(&definition(|d| d.id = "v1".to_string()))],
		);
		assert_eq!(provider.id, "p");
		assert_eq!(provider.name, "p");
		assert_eq!(provider.get_models().len(), 1);
		assert_eq!(provider.get_models()[0].api, VIRTUAL_MODEL_API);
		// keyless auth resolves to the virtual source
		let auth = provider.auth.api_key.as_ref().unwrap();
		assert_eq!(auth.name(), "Virtual model");
	}

	#[test]
	fn with_virtual_models_composes() {
		let base = Provider::new(
			"p".to_string(),
			"P".to_string(),
			ProviderAuth { api_key: None, oauth: None },
			Box::new(|| {
				vec![
					chat_model("p", "phys", "real-api"),
					chat_model("p", "shadowed", "real-api"),
					chat_model("p", "vm", "pi-virtual"),
				]
			}),
			Box::new(|model, _ctx, _opts| AssistantMessageEventStream(json!({"streamed": model.id}))),
			Box::new(|model, _ctx, _opts| AssistantMessageEventStream(json!({"simple": model.id}))),
		);
		let virtual_model = create_virtual_model(&definition(|d| d.id = "shadowed".to_string()));
		let composed = with_virtual_models("p", Some(base), vec![virtual_model]);

		// physical chat models: hides "shadowed" (virtual id) and the
		// incoming virtual "vm"; virtual model appended.
		let models = composed.get_models();
		let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
		assert_eq!(ids, vec!["phys", "shadowed"]);

		// stream dispatch: virtual → unrouted error; physical → provider.
		let streamed = composed.stream(&chat_model("p", "phys", "real-api"), &TranscriptContext::default(), None);
		assert_eq!(streamed.0, json!({"streamed": "phys"}));
		let unrouted = composed.stream(&chat_model("p", "shadowed", "pi-virtual"), &TranscriptContext::default(), None);
		assert_eq!(
			unrouted.0["error"],
			"Virtual model p/shadowed must be routed before streaming"
		);

		// filterModels: physical input filtered (no original filter → all
		// physical kept), incoming virtual re-appended.
		let input = vec![
			chat_model("p", "phys", "real-api"),
			chat_model("p", "shadowed", "real-api"),
			chat_model("p", "vm", "pi-virtual"),
		];
		let filtered = composed.filter_models(&input, None);
		let filtered_ids: Vec<&str> = filtered.iter().map(|m| m.id.as_str()).collect();
		assert_eq!(filtered_ids, vec!["phys", "vm"]);
	}
}
