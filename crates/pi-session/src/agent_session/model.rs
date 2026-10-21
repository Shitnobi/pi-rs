//! TS `AgentSession` model & thinking management (`agent-session.ts`
//! ~2480–2710) plus the full `agent.prepareRequest` projection
//! (`_installAgentRequestProjection`, ~788) with virtual-model routing
//! (`_modelRuntime.resolveModel`).
//!
//! **Projection bridge (documented divergence):** TS installs
//! `agent.prepareRequest` as a closure over `this`, so the hook reads and
//! mutates the live session. The Rust hook is `'static` and the session is
//! exclusively borrowed by the run driver, so the hook forwards each
//! session access over an mpsc channel to
//! [`AgentSession::run_with_bridge`], which services it while the agent
//! run future is parked at the hook's await point. Behavior is 1:1: each
//! forwarded operation happens at exactly the TS point in the request
//! sequence. (The run's abort `signal` is not threaded to the routing
//! seam — TS passes it, but the parity fakes ignore it.)
//!
//! **Mid-run pump:** servicing `Canonical` first drains the agent event
//! queue, so the projection sees every message of earlier in-run requests
//! (TS: `_handleAgentEvent` runs synchronously per event). A turn_end
//! boundary `continue` observed there is satisfied by the loop continuing
//! on its own, so the mid-run pump clears the continuation flag (the
//! post-run dispatch would otherwise resume a second time).

use std::sync::Arc;

use serde_json::{json, Map, Value};

use pi_agent::types::BoxFuture;
use pi_agent::{AgentContext, AgentRequestUpdate};
use pi_core::defaults::{THINKING_LEVEL_OPTIONS, DEFAULT_THINKING_LEVEL};
use pi_core::messages::convert_to_llm;
use pi_providers::virtual_models::{
    get_virtual_model_state, is_virtual_model, VIRTUAL_MODEL_STATE_ENTRY,
};

use super::AgentSession;

/// TS `scopedModels` config entries (`{ model, thinkingLevel? }`).
#[derive(Clone)]
pub struct ScopedModel {
    pub model: Value,
    pub thinking_level: Option<String>,
}

/// TS `ModelMutationOptions`.
#[derive(Default)]
pub struct ModelMutationOptions {
    pub persist: bool,
}

/// TS `ModelCycleResult`.
#[derive(Clone)]
pub struct ModelCycleResult {
    pub model: Value,
    pub thinking_level: String,
    pub is_scoped: bool,
}

/// The session-facing `ModelRouteRequest` slice (`{ reason, thinkingLevel,
/// failed, state }`); `messages` are passed alongside like the TS call.
pub struct RouteRequest {
    pub reason: String,
    pub thinking_level: String,
    pub failed: Option<Value>,
    pub state: Option<Value>,
}

/// TS `ModelRoute`.
pub struct RouteOutcome {
    pub model: Value,
    pub thinking_level: String,
    pub state: Option<Value>,
}

/// TS `_modelRuntime` routing surface (the model-mutation + virtual-model
/// slice; auth lives on [`super::ModelAuthStore`]).
pub trait ModelRouteStore: Send + Sync {
    /// TS `getAvailableSnapshot()`.
    fn available_snapshot(&self) -> Vec<Value>;
    /// TS `resolveModel(model, messages, request)` — `Err` models a router
    /// throw, which rejects the run.
    fn resolve_model(&self, model: &Value, messages: &[Value], request: &RouteRequest)
        -> Result<RouteOutcome, String>;
}

// ============================================================================
// Pure helpers (TS models.ts over raw JSON model values)
// ============================================================================

/// TS `modelsAreEqual` — same provider + id.
pub fn models_are_equal_value(a: &Value, b: &Value) -> bool {
    fn field<'a>(model: &'a Value, key: &str) -> &'a str {
        model.get(key).and_then(Value::as_str).unwrap_or("")
    }
    !a.is_null() && !b.is_null() && field(a, "provider") == field(b, "provider") && field(a, "id") == field(b, "id")
}

/// TS `getSupportedThinkingLevels(model)`.
pub fn get_supported_thinking_levels_value(model: &Value) -> Vec<String> {
    if model.get("reasoning").and_then(Value::as_bool) != Some(true) {
        return vec!["off".to_string()];
    }
    let map = model.get("thinkingLevelMap");
    THINKING_LEVEL_OPTIONS
        .iter()
        .filter(|level| {
            let mapped = map.and_then(|m| m.get(level.as_str()));
            if mapped == Some(&Value::Null) {
                return false;
            }
            if level.as_str() == "xhigh" || level.as_str() == "max" {
                return mapped.is_some();
            }
            true
        })
        .map(|level| level.as_str().to_string())
        .collect()
}

/// TS `clampThinkingLevel(model, level)`.
pub fn clamp_thinking_level_value(model: &Value, level: &str) -> String {
    let available = get_supported_thinking_levels_value(model);
    if available.iter().any(|candidate| candidate == level) {
        return level.to_string();
    }
    let all: Vec<&str> = THINKING_LEVEL_OPTIONS.iter().map(|l| l.as_str()).collect();
    let requested_index = all.iter().position(|candidate| *candidate == level);
    match requested_index {
        None => available.first().cloned().unwrap_or_else(|| "off".to_string()),
        Some(index) => {
            for candidate in &all[index..] {
                if available.iter().any(|a| a == candidate) {
                    return candidate.to_string();
                }
            }
            for candidate in all[..index].iter().rev() {
                if available.iter().any(|a| a == candidate) {
                    return candidate.to_string();
                }
            }
            "off".to_string()
        }
    }
}

// ============================================================================
// Projection bridge
// ============================================================================

/// One hook → run-driver call (the reply closes over a oneshot sender).
pub(crate) type ProjectionCall = (ProjectionRequest, tokio::sync::oneshot::Sender<ProjectionReply>);

pub(crate) enum ProjectionRequest {
    /// TS `const failed = this._failedResponse; this._failedResponse = undefined;`
    TakeFailed,
    /// `prepare()` — the canonical context (projection messages + LLM
    /// form) and the branch's router state for the current model.
    Canonical { provider: String, model_id: String },
    /// TS `appendCustomEntry(VIRTUAL_MODEL_STATE_ENTRY, data)` + emit.
    AppendVirtualModelState { data: Value },
    /// TS `_exceedsCompactionThreshold(route.model, projection)`.
    ExceedsThreshold { model: Value },
    /// TS `await this._runAutoCompaction("threshold", false)`.
    RunThresholdCompaction,
}

pub(crate) enum ProjectionReply {
    Failed(Option<Value>),
    Canonical { messages: Vec<Value>, llm_messages: Vec<Value>, state: Option<Value> },
    Done,
    Exceeds(bool),
}

/// The hook's request helper (send + await reply).
async fn bridge_call(tx: &tokio::sync::mpsc::UnboundedSender<ProjectionCall>, request: ProjectionRequest) -> ProjectionReply {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if tx.send((request, reply_tx)).is_err() {
        return ProjectionReply::Done;
    }
    reply_rx.await.unwrap_or(ProjectionReply::Done)
}

impl AgentSession {
    /// TS `_installAgentRequestProjection()` — installed once at
    /// construction; chains over the agent's existing `prepareRequest`.
    pub(crate) fn install_agent_request_projection(&mut self) {
        let previous = self.agent.prepare_request_hook();
        let agent = self.agent.clone();
        let tx = self.state.projection_bridge.clone();
        let routes = self.model_routes.clone();
        let hook: Arc<pi_agent::PrepareRequestHook> = Arc::new(
            move |context: &AgentContext, _loop_model: &Value, _loop_thinking: &str| {
                let tx = tx.clone();
                let agent = agent.clone();
                let previous = previous.clone();
                let routes = routes.clone();
                let _context_messages = context.messages.clone();
                Box::pin(async move {
                    let failed = match bridge_call(&tx, ProjectionRequest::TakeFailed).await {
                        ProjectionReply::Failed(failed) => failed,
                        _ => None,
                    };
                    loop {
                        let model = agent.model();
                        let thinking_level = agent.thinking_level();
                        let provider =
                            model.get("provider").and_then(Value::as_str).unwrap_or("").to_string();
                        let model_id = model.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let canonical = match bridge_call(
                            &tx,
                            ProjectionRequest::Canonical { provider: provider.clone(), model_id: model_id.clone() },
                        )
                        .await
                        {
                            ProjectionReply::Canonical { messages, llm_messages, state } => {
                                (messages, llm_messages, state)
                            }
                            _ => return Ok(None),
                        };
                        let canonical_context = AgentContext {
                            messages: canonical.0,
                            tools: Some(agent.tools()),
                        };
                        let previous_update = match &previous {
                            Some(hook) => hook(&canonical_context, &model, &thinking_level).await?,
                            None => None,
                        };
                        let context = previous_update
                            .as_ref()
                            .and_then(|update| update.context.clone())
                            .unwrap_or(canonical_context.clone());
                        let mut model = agent.model();
                        let mut thinking_level = agent.thinking_level();
                        if let Some(update) = previous_update {
                            if let Some(next_model) = update.model {
                                model = next_model;
                            }
                            if let Some(next_level) = update.thinking_level {
                                thinking_level = next_level;
                            }
                        }
                        if !is_virtual_model(model.get("api").and_then(Value::as_str).unwrap_or("")) {
                            return Ok(Some(AgentRequestUpdate {
                                context: Some(context),
                                model: Some(model),
                                thinking_level: Some(thinking_level),
                            }));
                        }
                        let messages = &context.messages;
                        let last_response = messages
                            .iter()
                            .rposition(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
                            .map(|index| index as isize)
                            .unwrap_or(-1);
                        let user_turn = messages[(last_response + 1) as usize..]
                            .iter()
                            .any(|message| message.get("role").and_then(Value::as_str) == Some("user"));
                        let state = canonical.2.clone();
                        let reason = if failed.is_some() {
                            "retry"
                        } else if user_turn {
                            "user"
                        } else {
                            "continuation"
                        };
                        let Some(routes) = routes.clone() else {
                            return Ok(Some(AgentRequestUpdate {
                                context: Some(context),
                                model: Some(model),
                                thinking_level: Some(thinking_level),
                            }));
                        };
                        let route = routes.resolve_model(
                            &model,
                            &canonical.1,
                            &RouteRequest {
                                reason: reason.to_string(),
                                thinking_level: thinking_level.clone(),
                                failed: failed.clone(),
                                state: state.clone(),
                            },
                        )?;
                        if route.state.is_some() && route.state != state {
                            let data = json!({
                                "provider": provider,
                                "modelId": model_id,
                                "state": route.state,
                            });
                            bridge_call(&tx, ProjectionRequest::AppendVirtualModelState { data }).await;
                        }
                        let exceeds = matches!(
                            bridge_call(&tx, ProjectionRequest::ExceedsThreshold { model: route.model.clone() }).await,
                            ProjectionReply::Exceeds(true)
                        );
                        if exceeds {
                            bridge_call(&tx, ProjectionRequest::RunThresholdCompaction).await;
                            continue; // re-prepare against the compacted session
                        }
                        model = route.model;
                        thinking_level = route.thinking_level;
                        return Ok(Some(AgentRequestUpdate {
                            context: Some(context),
                            model: Some(model),
                            thinking_level: Some(thinking_level),
                        }));
                    }
                }) as BoxFuture<'static, Result<Option<AgentRequestUpdate>, String>>
            },
        );
        self.agent.set_prepare_request(Some(hook));
    }

    /// Service one forwarded hook operation (the run future is parked).
    pub(crate) async fn service_projection(&mut self, request: ProjectionRequest) -> ProjectionReply {
        match request {
            ProjectionRequest::TakeFailed => ProjectionReply::Failed(
                self.state.failed_response.take().map(|id| self.state.arena[id].clone()),
            ),
            ProjectionRequest::Canonical { provider, model_id } => {
                // Mid-run pump: TS processes each agent event synchronously,
                // so the projection already contains earlier in-run
                // messages. The loop continuing on its own satisfies any
                // turn_end continuation observed here.
                self.pump_agent_events().await;
                self.state.turn_end_boundary_continue = false;
                let projection = self.session_manager.build_session_projection();
                let messages = projection
                    .messages
                    .iter()
                    .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
                    .collect();
                let llm_messages = convert_to_llm(&projection.messages);
                let branch = self.session_manager.get_branch(None);
                let state = get_virtual_model_state(&branch, &provider, &model_id);
                ProjectionReply::Canonical { messages, llm_messages, state }
            }
            ProjectionRequest::AppendVirtualModelState { data } => {
                let id = self
                    .session_manager
                    .append_custom_entry(VIRTUAL_MODEL_STATE_ENTRY, Some(data));
                if let Some(entry) = self.session_manager.get_entry(&id) {
                    // Through the agent event queue: emitted at the TS point
                    // relative to the run's other events (pumped in order).
                    let event = json!({ "type": "entry_appended", "entry": entry });
                    self.agent_event_queue
                        .lock()
                        .expect("agent event queue")
                        .push(pi_agent::AgentEvent::SessionEvent(event));
                }
                ProjectionReply::Done
            }
            ProjectionRequest::ExceedsThreshold { model } => ProjectionReply::Exceeds(
                self.exceeds_compaction_threshold(&model, &self.session_manager.build_session_projection()),
            ),
            ProjectionRequest::RunThresholdCompaction => {
                self.run_auto_compaction("threshold", false).await;
                ProjectionReply::Done
            }
        }
    }

    // ========================================================================
    // Model selection (TS ~2480–2640)
    // ========================================================================

    /// TS `get scopedModels`.
    pub fn scoped_models(&self) -> &[ScopedModel] {
        &self.state.scoped_models
    }

    /// TS `setScopedModels(scopedModels)`.
    pub fn set_scoped_models(&mut self, scoped_models: Vec<ScopedModel>) {
        self.state.scoped_models = scoped_models;
    }

    /// TS `_emitModelSelect(nextModel, previousModel, source)`.
    async fn emit_model_select(&mut self, next_model: &Value, previous_model: &Value, source: &str) {
        if models_are_equal_value(previous_model, next_model) {
            return;
        }
        let mut event = Map::new();
        event.insert("type".to_string(), json!("model_select"));
        event.insert("model".to_string(), next_model.clone());
        if !previous_model.is_null() {
            event.insert("previousModel".to_string(), previous_model.clone());
        }
        event.insert("source".to_string(), json!(source));
        self.extension_host.emit(Value::Object(event)).await;
    }

    /// TS `setModel(model, options)`.
    pub async fn set_model(&mut self, model: &Value, options: ModelMutationOptions) -> Result<(), String> {
        let provider = model.get("provider").and_then(Value::as_str).unwrap_or("").to_string();
        let id = model.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let authorized = match &self.model_auth {
            Some(store) => store.check_auth(&provider),
            None => true,
        };
        if !authorized {
            return Err(format!("No API key for {}/{}", provider, id));
        }
        let previous_model = self.agent.model();
        let thinking_level = self.thinking_level_for_model_switch(model, None);
        self.agent.set_model(model.clone());
        self.session_manager.append_model_change(&provider, &id);
        if options.persist {
            self.settings.set_default_model_and_provider(&provider, &id);
            self.add_persisted_default_to_non_empty_scope(model);
        }
        self.set_thinking_level(&thinking_level, ModelMutationOptions::default()).await;
        self.emit_model_select(model, &previous_model, "set").await;
        Ok(())
    }

    /// TS `_addPersistedDefaultToNonEmptyScope(model)`.
    fn add_persisted_default_to_non_empty_scope(&mut self, model: &Value) {
        if self.state.scoped_models.is_empty() {
            return;
        }
        if self.state.scoped_models.iter().any(|scoped| models_are_equal_value(&scoped.model, model)) {
            return;
        }
        self.state.scoped_models.push(ScopedModel { model: model.clone(), thinking_level: None });
        let enabled_models = match self.settings.get_enabled_models() {
            Some(models) if !models.is_empty() => models,
            _ => return,
        };
        let provider = model.get("provider").and_then(Value::as_str).unwrap_or("");
        let id = model.get("id").and_then(Value::as_str).unwrap_or("");
        let model_reference = format!("{}/{}", provider, id);
        if enabled_models
            .iter()
            .any(|pattern| pattern.to_lowercase() == model_reference.to_lowercase())
        {
            return;
        }
        let mut next = enabled_models;
        next.push(model_reference);
        self.settings.set_enabled_models(next);
    }

    /// TS `cycleModel(direction, options)`.
    pub async fn cycle_model(
        &mut self,
        direction: &str,
        options: ModelMutationOptions,
    ) -> Option<ModelCycleResult> {
        if !self.state.scoped_models.is_empty() {
            return self.cycle_scoped_model(direction, options).await;
        }
        self.cycle_available_model(direction, options).await
    }

    /// TS `_cycleScopedModel(direction, options)`.
    async fn cycle_scoped_model(
        &mut self,
        direction: &str,
        options: ModelMutationOptions,
    ) -> Option<ModelCycleResult> {
        let snapshot = match &self.model_routes {
            Some(routes) => routes.available_snapshot(),
            None => Vec::new(),
        };
        let available_ids: Vec<String> = snapshot
            .iter()
            .map(|model| {
                format!(
                    "{}\0{}",
                    model.get("provider").and_then(Value::as_str).unwrap_or(""),
                    model.get("id").and_then(Value::as_str).unwrap_or("")
                )
            })
            .collect();
        let scoped_models: Vec<ScopedModel> = self
            .state
            .scoped_models
            .iter()
            .filter(|scoped| {
                available_ids.contains(&format!(
                    "{}\0{}",
                    scoped.model.get("provider").and_then(Value::as_str).unwrap_or(""),
                    scoped.model.get("id").and_then(Value::as_str).unwrap_or("")
                ))
            })
            .cloned()
            .collect();
        if scoped_models.len() <= 1 {
            return None;
        }
        let current_model = self.agent.model();
        let current_index = scoped_models
            .iter()
            .position(|scoped| models_are_equal_value(&scoped.model, &current_model))
            .unwrap_or(0);
        let len = scoped_models.len();
        let next_index = if direction == "backward" {
            (current_index + len - 1) % len
        } else {
            (current_index + 1) % len
        };
        let next = scoped_models[next_index].clone();
        let thinking_level =
            self.thinking_level_for_model_switch(&next.model, next.thinking_level.clone());
        self.agent.set_model(next.model.clone());
        let provider = next.model.get("provider").and_then(Value::as_str).unwrap_or("").to_string();
        let id = next.model.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        self.session_manager.append_model_change(&provider, &id);
        if options.persist {
            self.settings.set_default_model_and_provider(&provider, &id);
            self.add_persisted_default_to_non_empty_scope(&next.model);
        }
        self.set_thinking_level(&thinking_level, ModelMutationOptions::default()).await;
        self.emit_model_select(&next.model, &current_model, "cycle").await;
        Some(ModelCycleResult {
            model: next.model,
            thinking_level: self.thinking_level(),
            is_scoped: true,
        })
    }

    /// TS `_cycleAvailableModel(direction, options)`.
    async fn cycle_available_model(
        &mut self,
        direction: &str,
        options: ModelMutationOptions,
    ) -> Option<ModelCycleResult> {
        let available_models = match &self.model_routes {
            Some(routes) => routes.available_snapshot(),
            None => Vec::new(),
        };
        if available_models.len() <= 1 {
            return None;
        }
        let current_model = self.agent.model();
        let current_index = available_models
            .iter()
            .position(|model| models_are_equal_value(model, &current_model))
            .unwrap_or(0);
        let len = available_models.len();
        let next_index = if direction == "backward" {
            (current_index + len - 1) % len
        } else {
            (current_index + 1) % len
        };
        let next_model = available_models[next_index].clone();
        let thinking_level = self.thinking_level_for_model_switch(&next_model, None);
        self.agent.set_model(next_model.clone());
        let provider = next_model.get("provider").and_then(Value::as_str).unwrap_or("").to_string();
        let id = next_model.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        self.session_manager.append_model_change(&provider, &id);
        if options.persist {
            self.settings.set_default_model_and_provider(&provider, &id);
            self.add_persisted_default_to_non_empty_scope(&next_model);
        }
        self.set_thinking_level(&thinking_level, ModelMutationOptions::default()).await;
        self.emit_model_select(&next_model, &current_model, "cycle").await;
        Some(ModelCycleResult {
            model: next_model,
            thinking_level: self.thinking_level(),
            is_scoped: false,
        })
    }

    // ========================================================================
    // Thinking level (TS ~2642–2710)
    // ========================================================================

    /// TS `get thinkingLevel` (`agent.state.thinkingLevel`).
    pub fn thinking_level(&self) -> String {
        self.agent.thinking_level()
    }

    /// TS `setThinkingLevel(level, options)` (async here so the
    /// fire-and-forget extension emission still completes before the
    /// caller proceeds — the recording fakes finish on first poll, so
    /// ordering matches the TS synchronous emit).
    pub async fn set_thinking_level(&mut self, level: &str, options: ModelMutationOptions) {
        let available_levels = self.get_available_thinking_levels();
        let effective_level = if available_levels.iter().any(|candidate| candidate == level) {
            level.to_string()
        } else {
            self.clamp_thinking_level(level)
        };
        let previous_level = self.agent.thinking_level();
        let is_changing = effective_level != previous_level;
        self.agent.set_thinking_level(&effective_level);
        if options.persist {
            self.settings.set_default_thinking_level(level);
        }
        if is_changing {
            self.session_manager.append_thinking_level_change(&effective_level);
            self.emit(json!({ "type": "thinking_level_changed", "level": effective_level }));
            // TS `void this._extensionRunner.emit(...)` — fire-and-forget;
            // the recording fakes complete synchronously, so awaiting here
            // is observably identical.
            let mut event = Map::new();
            event.insert("type".to_string(), json!("thinking_level_select"));
            event.insert("level".to_string(), json!(effective_level));
            event.insert("previousLevel".to_string(), json!(previous_level));
            let host = Arc::clone(&self.extension_host);
            let event = Value::Object(event);
            host.emit(event).await;
        }
    }

    /// TS `cycleThinkingLevel(options)`.
    pub async fn cycle_thinking_level(&mut self, options: ModelMutationOptions) -> Option<String> {
        if !self.supports_thinking() {
            return None;
        }
        let levels = self.get_available_thinking_levels();
        let current = self.thinking_level();
        let current_index = levels.iter().position(|level| *level == current).unwrap_or(0);
        let next_index = (current_index + 1) % levels.len();
        let next_level = levels[next_index].clone();
        self.set_thinking_level(&next_level, options).await;
        Some(next_level)
    }

    /// TS `getAvailableThinkingLevels()`.
    pub fn get_available_thinking_levels(&self) -> Vec<String> {
        let model = self.agent.model();
        if model.is_null() {
            return THINKING_LEVEL_OPTIONS.iter().map(|level| level.as_str().to_string()).collect();
        }
        get_supported_thinking_levels_value(&model)
    }

    /// TS `supportsThinking()`.
    pub fn supports_thinking(&self) -> bool {
        self.agent.model().get("reasoning").and_then(Value::as_bool) == Some(true)
    }

    /// TS `_getThinkingLevelForModelSwitch(targetModel?, explicitLevel?)`.
    fn thinking_level_for_model_switch(&self, target_model: &Value, explicit_level: Option<String>) -> String {
        if let Some(level) = explicit_level {
            return level;
        }
        if !target_model.is_null() {
            let provider = target_model.get("provider").and_then(Value::as_str).unwrap_or("");
            let id = target_model.get("id").and_then(Value::as_str).unwrap_or("");
            if let Some(per_model) = self.settings.get_model_thinking_level(provider, id) {
                return per_model;
            }
        }
        self.settings
            .get_default_thinking_level()
            .unwrap_or_else(|| self.thinking_level_if_any().unwrap_or(DEFAULT_THINKING_LEVEL.as_str().to_string()))
    }

    /// TS `this.thinkingLevel ?? DEFAULT_THINKING_LEVEL` — the agent state
    /// level is always a string in this port (`"off"` default), so this is
    /// the agent value directly.
    fn thinking_level_if_any(&self) -> Option<String> {
        Some(self.agent.thinking_level())
    }

    /// TS `_clampThinkingLevel(level, _availableLevels)`.
    fn clamp_thinking_level(&self, level: &str) -> String {
        let model = self.agent.model();
        if model.is_null() {
            "off".to_string()
        } else {
            clamp_thinking_level_value(&model, level)
        }
    }
}
