//! TS `AgentSession` prompt pipeline (`agent-session.ts` ~2058–2430):
//! `prompt`, `steer`/`followUp`/`_queueUserInput`, `sendUserMessage`,
//! extension-command dispatch, skill/template expansion, image
//! normalization seam, and the reduced `_startRun`/`_prepareRun` run
//! driver the prompt paths need (the full run orchestration —
//! `before_agent_start` handlers, compaction checks, retry, the
//! before-settle boundary — is tranche 11+ and marked TODO).
//!
//! **Event-pump divergence (documented):** TS subscribes
//! `_handleAgentEvent` directly to the agent, so session processing of a
//! loop event completes before the loop continues. The Rust port queues
//! agent events (the subscription only pushes) and pumps them at run
//! checkpoints (`agent.prompt`/`agent.continue` resolution), preserving
//! emission order. Observably equivalent unless session state is read
//! mid-run (the prompt harness never does).

use serde_json::{json, Map, Value};

use pi_agent::AgentEvent;

use super::events::SessionAgentEvent;
use super::{AgentSession, DeferredSettledAction, MsgId};

/// TS `preflightResult`.
pub type PreflightResult = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// TS `PromptOptions`.
#[derive(Default)]
pub struct PromptOptions {
    /// TS `expandPromptTemplates` (default `true`).
    pub expand_prompt_templates: Option<bool>,
    /// TS `images`.
    pub images: Option<Vec<Value>>,
    /// TS `streamingBehavior` (`"steer"` | `"followUp"`).
    pub streaming_behavior: Option<String>,
    /// TS `source` (default `"interactive"`).
    pub source: Option<String>,
    /// TS `preflightResult`.
    pub preflight_result: Option<PreflightResult>,
}

/// TS `_startRun`'s `BeforeAgentStartTrigger` — the slice the prompt
/// pipeline produces.
pub enum RunTrigger {
    /// `{ messageType: "user", prompt, images }`.
    User { prompt: String, images: Option<Vec<Value>> },
    /// `{ messageType: "custom", message }`.
    Custom { message: Value },
}

/// TS `QueuedInputDisposition`.
pub const DISPOSITION_HANDLED: &str = "handled";
pub const DISPOSITION_QUEUED: &str = "queued";

/// TS `InputEventResult` (the fake extension runner's answer).
pub enum InputAction {
    /// `{ action: "continue" }`.
    Continue,
    /// `{ action: "handled" }`.
    Handled,
    /// `{ action: "transform", text?, images? }`.
    Transform { text: Option<String>, images: Option<Vec<Value>> },
}

/// A skill as the prompt slice sees it (TS `resourceLoader.getSkills()`;
/// the real loader is Phase 4 — the harness injects specs and the file
/// content directly, `read_fails` standing in for a failed
/// `readFileSync`).
#[derive(Clone)]
pub struct SkillSpec {
    pub name: String,
    pub file_path: String,
    pub base_dir: String,
    pub content: String,
    pub read_fails: bool,
}

/// TS `stripFrontmatter` (utils/frontmatter.ts) — body of the frontmatter
/// block, newlines normalized and BOM stripped.
fn strip_frontmatter(content: &str) -> String {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.strip_prefix('\u{feff}').unwrap_or(&normalized);
    if !normalized.starts_with("---") {
        return normalized.to_string();
    }
    match normalized[3..].find("\n---") {
        None => normalized.to_string(),
        Some(_) => {
            // `endIndex` counts from index 3 in TS (`indexOf("\n---", 3)`),
            // body starts 4 past it.
            let rest = &normalized[3..];
            let end = rest.find("\n---").expect("checked");
            rest[end + 4..].trim().to_string()
        }
    }
}

/// Split `/name args` → (name, args) (TS `indexOf(" ")` slicing).
fn split_command(text: &str) -> (String, String) {
    match text.find(' ') {
        None => (text[1..].to_string(), String::new()),
        Some(space) => (text[1..space].to_string(), text[space + 1..].to_string()),
    }
}

/// The `processImage` seam (utils/image-process.ts) — no real decoding:
/// supported mime types pass through byte-identical, unsupported types
/// fail conversion. Divergence: the real converter (Phase 4) may succeed
/// on decodable non-inline formats, and `autoResize` cannot run (the
/// harness always pins `images.autoResize: false`).
fn process_image_seam(data: &str, mime_type: &str, auto_resize: bool) -> Result<(String, String), String> {
    let base = mime_type.split(';').next().unwrap_or("").trim().to_lowercase();
    let normalized = match base.as_str() {
        "image/png" => "image/png",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => {
            return Err(
                "[Image omitted: could not be converted to a supported inline image format.]".to_string(),
            )
        }
    };
    if auto_resize {
        return Err("[Image omitted: could not be resized below the inline image size limit.]".to_string());
    }
    Ok((data.to_string(), normalized.to_string()))
}

impl AgentSession {
    /// TS `prompt(text, options?)`.
    pub async fn prompt(&mut self, text: &str, options: PromptOptions) -> Result<(), String> {
        if self.state.is_emitting_agent_settled {
            self.state.deferred_settled_actions.lock().expect("deferred actions").push(DeferredSettledAction::Prompt {
                text: text.to_string(),
                options,
            });
            return Ok(());
        }
        let expand = options.expand_prompt_templates.unwrap_or(true);
        let preflight = |session: &AgentSession, disposition: &str| {
            if let Some(callback) = &options.preflight_result {
                callback(disposition);
            }
            let _ = session;
        };
        // Extension commands first — executed immediately, even while
        // streaming; they manage their own LLM interaction.
        if expand && text.starts_with('/') && self.try_execute_extension_command(text).await {
            preflight(self, DISPOSITION_HANDLED);
            return Ok(());
        }
        // TODO(tranche 13): compaction-in-progress rejection
        // (`_compactionAbortController !== undefined`).

        // Input interception before skill/template expansion.
        let streaming_behavior =
            if self.is_streaming() { options.streaming_behavior.as_deref() } else { None };
        let processed = self
            .run_input_handlers(
                text,
                options.images.as_deref(),
                options.source.as_deref().unwrap_or("interactive"),
                streaming_behavior,
            )
            .await?;
        let Some((current_text, current_images)) = processed else {
            preflight(self, DISPOSITION_HANDLED);
            return Ok(());
        };

        // Expand skill commands (/skill:name args) and prompt templates.
        let mut expanded_text = current_text.clone();
        if expand {
            expanded_text = self.expand_skill_command(&current_text);
            expanded_text =
                pi_core::prompt_templates::expand_prompt_template(&expanded_text, &self.prompt_templates);
        }

        // While streaming, queue via steer()/followUp().
        if self.is_streaming() {
            let Some(behavior) = options.streaming_behavior.as_deref() else {
                return Err("Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message.".to_string());
            };
            if behavior == "followUp" {
                self.queue_follow_up_internal(&expanded_text, current_images.as_deref());
            } else {
                self.queue_steer_internal(&expanded_text, current_images.as_deref());
            }
            preflight(self, DISPOSITION_QUEUED);
            return Ok(());
        }

        // Validate model + auth before sending.
        let model = self.agent.model();
        if model.is_null() {
            return Err(self.no_model_selected_message());
        }
        let provider =
            model.get("provider").and_then(Value::as_str).unwrap_or_default().to_string();
        let has_auth = match &self.model_auth {
            Some(store) => {
                store.has_configured_auth(&provider) || store.check_auth(&provider)
            }
            None => true,
        };
        if !has_auth {
            if let Some(store) = &self.model_auth {
                if store.is_using_oauth(&provider) {
                    return Err(format!(
                        "Authentication failed for \"{}\". Credentials may have expired or network is unavailable. Run '/login {}' to re-authenticate.",
                        provider, provider
                    ));
                }
                return Err(store.no_api_key_message(&provider));
            }
            return Err(format!("No API key found for {}.", provider));
        }

        let on_started = options.preflight_result.as_ref().map(|callback| {
            let callback = std::sync::Arc::clone(callback);
            std::sync::Arc::new(move || callback("started"))
                as std::sync::Arc<dyn Fn() + Send + Sync>
        });
        self.start_run(
            RunTrigger::User { prompt: expanded_text, images: current_images },
            on_started,
        )
        .await
    }

    pub(super) fn no_model_selected_message(&self) -> String {
        match &self.model_auth {
            Some(store) => store.no_model_selected_message(),
            None => "No model selected.".to_string(),
        }
    }

    /// TS `_tryExecuteExtensionCommand(text)` — `true` when a command was
    /// found (and executed or failed; failures surface via `emitError`).
    async fn try_execute_extension_command(&mut self, text: &str) -> bool {
        let (name, args) = split_command(text);
        if !self.extension_host.get_command(&name) {
            return false;
        }
        if let Err(error) = self.extension_host.execute_command(&name, &args).await {
            self.extension_host.emit_error(json!({
                "extensionPath": format!("command:{}", name),
                "event": "command",
                "error": error,
            }));
        }
        true
    }

    /// TS `_expandSkillCommand(text)` — expand `/skill:name args` to the
    /// skill block; unknown skills and read failures pass through (the
    /// failure is emitted like an extension error).
    fn expand_skill_command(&mut self, text: &str) -> String {
        if !text.starts_with("/skill:") {
            return text.to_string();
        }
        // TS: name is `text.slice(7, spaceIndex)` — skip the "/skill:" prefix.
        let (name, args) = match text.find(' ') {
            Some(space) => (text[7..space].to_string(), text[space + 1..].trim().to_string()),
            None => (text[7..].to_string(), String::new()),
        };
        let Some(skill) = self.skills.iter().find(|skill| skill.name == name).cloned() else {
            return text.to_string(); // Unknown skill, pass through.
        };
        if skill.read_fails {
            // Node readFileSync ENOENT message (byte-exact).
            self.extension_host.emit_error(json!({
                "extensionPath": skill.file_path,
                "event": "skill_expansion",
                "error": format!("ENOENT: no such file or directory, open '{}'", skill.file_path),
            }));
            return text.to_string();
        }
        let body = strip_frontmatter(&skill.content).trim().to_string();
        let skill_block = format!(
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
            skill.name, skill.file_path, skill.base_dir, body
        );
        if args.is_empty() { skill_block } else { format!("{}\n\n{}", skill_block, args) }
    }

    /// TS `_runInputHandlers` — `None` when handled.
    async fn run_input_handlers(
        &mut self,
        text: &str,
        images: Option<&[Value]>,
        source: &str,
        streaming_behavior: Option<&str>,
    ) -> Result<Option<(String, Option<Vec<Value>>)>, String> {
        if !self.extension_host.has_handlers("input") {
            return Ok(Some((text.to_string(), images.map(|images| images.to_vec()))));
        }
        let mut event = Map::new();
        event.insert("type".to_string(), json!("input"));
        event.insert("text".to_string(), json!(text));
        if let Some(images) = images {
            event.insert("images".to_string(), json!(images));
        }
        event.insert("source".to_string(), json!(source));
        if let Some(behavior) = streaming_behavior {
            event.insert("streamingBehavior".to_string(), json!(behavior));
        }
        match self.extension_host.emit_input(Value::Object(event)).await {
            InputAction::Handled => Ok(None),
            InputAction::Continue => {
                Ok(Some((text.to_string(), images.map(|images| images.to_vec()))))
            }
            InputAction::Transform { text: new_text, images: new_images } => Ok(Some((
                new_text.unwrap_or_else(|| text.to_string()),
                new_images.or_else(|| images.map(|images| images.to_vec())),
            ))),
        }
    }

    /// TS `_queueUserInput` — the steer/followUp entry point.
    async fn queue_user_input(
        &mut self,
        text: &str,
        images: Option<&[Value]>,
        behavior: &str,
        source: &str,
    ) -> Result<&'static str, String> {
        if text.starts_with('/') {
            self.throw_if_extension_command(text)?;
        }
        let streaming_behavior = if self.is_streaming() { Some(behavior) } else { None };
        let processed = self
            .run_input_handlers(text, images, source, streaming_behavior)
            .await?;
        let Some((text, images)) = processed else {
            return Ok(DISPOSITION_HANDLED);
        };
        let expanded = self.expand_skill_command(&text);
        let expanded =
            pi_core::prompt_templates::expand_prompt_template(&expanded, &self.prompt_templates);
        if behavior == "steer" {
            self.queue_steer_internal(&expanded, images.as_deref());
        } else {
            self.queue_follow_up_internal(&expanded, images.as_deref());
        }
        Ok(DISPOSITION_QUEUED)
    }

    /// TS `steer(text, images?, options?)`.
    pub async fn steer(
        &mut self,
        text: &str,
        images: Option<&[Value]>,
        source: Option<&str>,
    ) -> Result<&'static str, String> {
        self.queue_user_input(text, images, "steer", source.unwrap_or("interactive")).await
    }

    /// TS `followUp(text, images?, options?)`.
    pub async fn follow_up(
        &mut self,
        text: &str,
        images: Option<&[Value]>,
        source: Option<&str>,
    ) -> Result<&'static str, String> {
        self.queue_user_input(text, images, "followUp", source.unwrap_or("interactive")).await
    }

    /// TS `_throwIfExtensionCommand(text)`.
    fn throw_if_extension_command(&mut self, text: &str) -> Result<(), String> {
        let (name, _) = split_command(text);
        if self.extension_host.get_command(&name) {
            return Err(format!(
                "Extension command \"/{}\" cannot be queued. Use prompt() or execute the command when not streaming.",
                name
            ));
        }
        Ok(())
    }

    /// TS `sendUserMessage(content, options?)` — always triggers a turn
    /// (or queues via `deliverAs` while streaming); template expansion is
    /// off by default.
    pub async fn send_user_message(
        &mut self,
        content: &Value,
        deliver_as: Option<&str>,
        expand_prompt_templates: Option<bool>,
    ) -> Result<(), String> {
        let (text, images) = match content {
            Value::String(text) => (text.clone(), None),
            Value::Array(parts) => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut images: Vec<Value> = Vec::new();
                for part in parts {
                    if part.get("type").and_then(Value::as_str) == Some("text") {
                        text_parts.push(
                            part.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
                        );
                    } else {
                        images.push(part.clone());
                    }
                }
                let images = if images.is_empty() { None } else { Some(images) };
                (text_parts.join("\n"), images)
            }
            _ => return Err("invalid content".to_string()),
        };
        let options = PromptOptions {
            expand_prompt_templates: Some(expand_prompt_templates.unwrap_or(false)),
            images,
            streaming_behavior: deliver_as.map(str::to_string),
            source: Some("extension".to_string()),
            preflight_result: None,
        };
        self.prompt(&text, options).await
    }

    /// TS `_normalizePromptImages` — the processImage seam.
    pub(super) async fn normalize_prompt_images(&self, images: Option<&[Value]>) -> (Vec<Value>, Vec<String>) {
        let Some(images) = images else { return (Vec::new(), Vec::new()) };
        let auto_resize = self.settings.image_auto_resize();
        let mut normalized_images = Vec::new();
        let mut hints = Vec::new();
        for image in images {
            let data = image.get("data").and_then(Value::as_str).unwrap_or_default();
            let mime_type = image.get("mimeType").and_then(Value::as_str).unwrap_or_default();
            match process_image_seam(data, mime_type, auto_resize) {
                Ok((data, mime_type)) => {
                    normalized_images.push(json!({ "type": "image", "data": data, "mimeType": mime_type }));
                }
                Err(message) => hints.push(message),
            }
        }
        (normalized_images, hints)
    }

    // Run driver (`_startRun` and friends) lives in [`super::run`].

    // ========================================================================
    // Agent-event pump (the `_unsubscribeAgent` subscription)
    // ========================================================================

    /// Drain the queued agent events through [`AgentSession::handle_agent_event`].
    pub async fn pump_agent_events(&mut self) {
        loop {
            // FIFO drain: events are pushed in emission order, so pop from
            // the front (TS listeners run in emission order).
            let event = {
                let mut queue = self.agent_event_queue.lock().expect("agent event queue");
                if queue.is_empty() { None } else { Some(queue.remove(0)) }
            };
            let Some(event) = event else { break };
            self.handle_agent_value_event(event).await;
        }
    }

    /// Bridge an [`AgentEvent`] (loop emission, owned message values) onto
    /// the arena-keyed [`SessionAgentEvent`]. Streaming messages map to one
    /// arena slot (TS: one mutated object); turn_end/agent_end messages are
    /// matched back by value (content-equal snapshots of the same object).
    async fn handle_agent_value_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.state.open_message = None;
                self.state.run_message_values.clear();
                self.state.run_message_ids.clear();
                self.handle_agent_event(&SessionAgentEvent::AgentStart).await;
            }
            AgentEvent::MessageStart { message } => {
                let id = self.register_message(message);
                self.state.open_message = Some(id);
                self.handle_agent_event(&SessionAgentEvent::MessageStart { message: id }).await;
            }
            AgentEvent::MessageUpdate { message, assistant_message_event } => {
                let id = match self.state.open_message {
                    Some(id) => id,
                    None => {
                        let id = self.register_message(message.clone());
                        self.state.open_message = Some(id);
                        id
                    }
                };
                self.state.arena[id] = message;
                self.handle_agent_event(&SessionAgentEvent::MessageUpdate {
                    message: id,
                    assistant_message_event,
                })
                .await;
            }
            AgentEvent::MessageEnd { message } => {
                let id = self.state.open_message.take().unwrap_or_else(|| {
                    let value = message.clone();
                    self.register_message(value)
                });
                // TS in-place mutation: session-side merges performed at
                // message_start (nested calls) stay visible at message_end.
                let merged = if self.state.arena[id].get("nestedCalls").is_some() {
                    let previous = self.state.arena[id].clone();
                    let mut merged = message;
                    if let Some(object) = merged.as_object_mut() {
                        object.insert("nestedCalls".to_string(), previous["nestedCalls"].clone());
                        if let Some(usage) = previous.get("usage") {
                            object.insert("usage".to_string(), usage.clone());
                        }
                    }
                    merged
                } else {
                    message
                };
                self.state.arena[id] = merged.clone();
                self.state.run_message_values.push(merged);
                self.state.run_message_ids.push(id);
                // The agent already pushed this message into its state
                // (processEvents runs before listeners); keep the id list
                // in step so in-place replacements sync both.
                self.state.state_message_ids.push(id);
                self.handle_agent_event(&SessionAgentEvent::MessageEnd { message: id }).await;
            }
            AgentEvent::TurnEnd { message, tool_results } => {
                let message = self.match_run_message(message);
                let tool_results =
                    tool_results.into_iter().map(|result| self.match_run_message(result)).collect();
                self.handle_agent_event(&SessionAgentEvent::TurnEnd { message, tool_results }).await;
            }
            AgentEvent::AgentEnd { messages } => {
                let messages =
                    messages.into_iter().map(|message| self.match_run_message(message)).collect();
                self.handle_agent_event(&SessionAgentEvent::AgentEnd { messages }).await;
            }
            AgentEvent::TurnStart => {
                self.handle_agent_event(&SessionAgentEvent::TurnStart).await;
            }
            AgentEvent::SessionEvent(event) => {
                // A session-emitted event forwarded through the queue (the
                // virtual-model projection's `entry_appended`).
                self.emit(event);
            }
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                self.handle_agent_event(&SessionAgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    args,
                })
                .await;
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, args, partial_result } => {
                self.handle_agent_event(&SessionAgentEvent::ToolExecutionUpdate {
                    tool_call_id,
                    tool_name,
                    args,
                    partial_result,
                })
                .await;
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error, duration_ms } => {
                self.handle_agent_event(&SessionAgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    tool_name,
                    result,
                    is_error,
                    duration_ms,
                })
                .await;
            }
        }
    }

    /// Value → arena id: the most recent content-equal message of this run
    /// (snapshots of the same TS object are content-equal at completion).
    fn match_run_message(&mut self, value: Value) -> MsgId {
        for index in (0..self.state.run_message_values.len()).rev() {
            if self.state.run_message_values[index] == value {
                return self.state.run_message_ids[index];
            }
        }
        self.register_message(value)
    }
}
