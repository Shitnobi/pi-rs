//! `AgentSessionRuntime` (TS `core/agent-session-runtime.ts`, 447 LOC):
//! owns the current `AgentSession` plus its cwd-bound services; session
//! replacement (`switchSession` / `newSession` / `fork` /
//! `importFromJsonl`) tears down the current runtime first, then creates
//! and applies the next one.

use std::sync::Arc;

use serde_json::Value;

use pi_agent::types::BoxFuture;
use pi_core::session_manager::{Generators, SessionManager};

/// TS `join(dir, name)` (POSIX harness paths).
fn join_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{}{}", dir, name)
    } else {
        format!("{}/{}", dir, name)
    }
}

use super::services::{AgentSessionRuntimeDiagnostic, AgentSessionServices};
use super::AgentSession;

/// TS `CreateAgentSessionRuntimeResult`.
pub struct CreateAgentSessionRuntimeResult {
    pub session: AgentSession,
    pub services: AgentSessionServices,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    pub model_fallback_message: Option<String>,
}

/// TS `CreateAgentSessionRuntimeFactory` options.
pub struct CreateRuntimeOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub session_manager: SessionManager,
    pub session_start_event: Option<Value>,
    pub project_trust_context: Option<Value>,
}

/// TS `CreateAgentSessionRuntimeFactory`.
pub type CreateRuntimeFactory = Arc<
    dyn Fn(CreateRuntimeOptions) -> BoxFuture<'static, Result<CreateAgentSessionRuntimeResult, String>>
        + Send
        + Sync,
>;

/// TS `ReplacedSessionContext` — the command-context properties plus the
/// session-bound `sendMessage`/`sendUserMessage` (the bound methods are
/// host-side closures in TS; the port records the command context and the
/// harness treats the methods as present).
pub struct ReplacedSessionContext {
    pub command_context: Value,
}

/// TS `withSession` callback.
pub type WithSessionFn = Arc<dyn Fn(&ReplacedSessionContext) -> BoxFuture<'static, ()> + Send + Sync>;

/// TS `rebindSession` callback (TS receives the new session; the port
/// notifies — the harness records the rebinding only).
pub type RebindSessionFn = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// TS `setBeforeSessionInvalidate` callback.
pub type BeforeInvalidateFn = Arc<dyn Fn() + Send + Sync>;

/// TS `newSession` `setup` callback.
pub type NewSessionSetupFn = Arc<dyn Fn(&mut SessionManager) -> BoxFuture<'static, ()> + Send + Sync>;

/// TS `projectTrustContextFactory`.
pub type ProjectTrustContextFactory = Arc<dyn Fn(&str) -> Value + Send + Sync>;

/// TS `extractUserMessageText`.
fn extract_user_message_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|part| {
                    part.get("type").and_then(Value::as_str) == Some("text")
                        && part.get("text").map(Value::is_string).unwrap_or(false)
                })
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// TS `getMissingSessionCwdIssue` → `(sessionFile, sessionCwd, fallbackCwd)`.
fn get_missing_session_cwd_issue(
    session_manager: &SessionManager,
    fallback_cwd: &str,
) -> Option<(String, String, String)> {
    let session_file = session_manager.get_session_file()?.to_string();
    let session_cwd = session_manager.get_cwd().to_string();
    if session_cwd.is_empty() || std::path::Path::new(&session_cwd).exists() {
        return None;
    }
    Some((session_file, session_cwd, fallback_cwd.to_string()))
}

/// TS `formatMissingSessionCwdError` (byte-exact).
fn format_missing_session_cwd_error(issue: &(String, String, String)) -> String {
    let session_file = if !issue.0.is_empty() {
        format!("\nSession file: {}", issue.0)
    } else {
        String::new()
    };
    format!(
        "Stored session working directory does not exist: {}{}\nCurrent working directory: {}",
        issue.1, session_file, issue.2
    )
}

/// TS `assertSessionCwdExists` — `Err` is the `MissingSessionCwdError`
/// message.
pub fn assert_session_cwd_exists(session_manager: &SessionManager, fallback_cwd: &str) -> Result<(), String> {
    match get_missing_session_cwd_issue(session_manager, fallback_cwd) {
        Some(issue) => Err(format_missing_session_cwd_error(&issue)),
        None => Ok(()),
    }
}

/// TS `class AgentSessionRuntime`.
pub struct AgentSessionRuntime {
    session: AgentSession,
    services: AgentSessionServices,
    create_runtime: CreateRuntimeFactory,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
    rebind_session: Option<RebindSessionFn>,
    before_session_invalidate: Option<BeforeInvalidateFn>,
}

/// The generators every replacement `SessionManager` is built with (TS:
/// the global `randomUUID`/`uuidv7` overrides).
pub struct RuntimeGens {
    pub gen: Generators,
}

impl AgentSessionRuntime {
    /// TS `constructor`.
    pub fn new(
        session: AgentSession,
        services: AgentSessionServices,
        create_runtime: CreateRuntimeFactory,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
        model_fallback_message: Option<String>,
    ) -> AgentSessionRuntime {
        AgentSessionRuntime {
            session,
            services,
            create_runtime,
            diagnostics,
            model_fallback_message,
            rebind_session: None,
            before_session_invalidate: None,
        }
    }

    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    pub fn cwd(&self) -> &str {
        &self.services.cwd
    }

    pub fn diagnostics(&self) -> &[AgentSessionRuntimeDiagnostic] {
        &self.diagnostics
    }

    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }

    /// TS `setRebindSession`.
    pub fn set_rebind_session(&mut self, rebind_session: Option<RebindSessionFn>) {
        self.rebind_session = rebind_session;
    }

    /// TS `setBeforeSessionInvalidate`.
    pub fn set_before_session_invalidate(&mut self, callback: Option<BeforeInvalidateFn>) {
        self.before_session_invalidate = callback;
    }

    /// TS `emitBeforeSwitch`.
    async fn emit_before_switch(&self, reason: &str, target_session_file: Option<&str>) -> bool {
        let runner = &self.session.extension_host_ref();
        if !runner.has_handlers("session_before_switch") {
            return false;
        }
        let mut event = serde_json::json!({ "type": "session_before_switch", "reason": reason });
        if let Some(target) = target_session_file {
            event["targetSessionFile"] = Value::String(target.to_string());
        }
        let result = runner.emit_session_before_switch(event).await;
        result.and_then(|r| r.get("cancel").and_then(Value::as_bool)).unwrap_or(false)
    }

    /// TS `emitBeforeFork`.
    async fn emit_before_fork(&self, entry_id: &str, position: &str) -> bool {
        let runner = &self.session.extension_host_ref();
        if !runner.has_handlers("session_before_fork") {
            return false;
        }
        let event = serde_json::json!({
            "type": "session_before_fork",
            "entryId": entry_id,
            "position": position,
        });
        let result = runner.emit_session_before_fork(event).await;
        result.and_then(|r| r.get("cancel").and_then(Value::as_bool)).unwrap_or(false)
    }

    /// TS `emitSessionShutdownEvent` (extensions/runner.ts).
    async fn emit_session_shutdown_event(&self, reason: &str, target_session_file: Option<&str>) {
        let runner = self.session.extension_host_ref();
        if !runner.has_handlers("session_shutdown") {
            return;
        }
        let mut event = serde_json::json!({ "type": "session_shutdown", "reason": reason });
        if let Some(target) = target_session_file {
            event["targetSessionFile"] = Value::String(target.to_string());
        }
        runner.emit(event).await;
    }

    /// TS `teardownCurrent` — settle the active response, emit
    /// `session_shutdown`, run the before-invalidate hook, dispose.
    async fn teardown_current(&mut self, reason: &str, target_session_file: Option<String>) {
        self.session.abort().await;
        self.emit_session_shutdown_event(reason, target_session_file.as_deref()).await;
        if let Some(callback) = &self.before_session_invalidate {
            callback();
        }
        self.session.dispose();
    }

    /// TS `apply`.
    fn apply(&mut self, result: CreateAgentSessionRuntimeResult) {
        self.session = result.session;
        self.services = result.services;
        self.diagnostics = result.diagnostics;
        self.model_fallback_message = result.model_fallback_message;
    }

    /// TS `finishSessionReplacement`.
    async fn finish_session_replacement(&mut self, with_session: Option<WithSessionFn>) {
        if let Some(rebind) = self.rebind_session.clone() {
            rebind().await;
        }
        if let Some(with_session) = with_session {
            let context = ReplacedSessionContext {
                command_context: self.session.create_replaced_session_context(),
            };
            with_session(&context).await;
        }
    }

    /// TS `switchSession`.
    pub async fn switch_session(
        &mut self,
        session_path: &str,
        cwd_override: Option<&str>,
        with_session: Option<WithSessionFn>,
        project_trust_context_factory: Option<ProjectTrustContextFactory>,
        gens: &Generators,
    ) -> Result<bool, String> {
        if self.emit_before_switch("resume", Some(session_path)).await {
            return Ok(true);
        }

        let previous_session_file = self.session.session_manager.get_session_file().map(str::to_string);
        let session_manager = SessionManager::open(session_path, None, cwd_override, gens)?;
        assert_session_cwd_exists(&session_manager, self.cwd())?;
        let target = session_manager.get_session_file().map(str::to_string);
        self.teardown_current("resume", target).await;
        let project_trust_context =
            project_trust_context_factory.as_ref().map(|factory| factory(session_manager.get_cwd()));
        let session_start_event = Some(serde_json::json!({
            "type": "session_start",
            "reason": "resume",
            "previousSessionFile": previous_session_file,
        }));
        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: session_manager.get_cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
            session_start_event,
            project_trust_context,
        })
        .await?;
        self.apply(result);
        self.finish_session_replacement(with_session).await;
        Ok(false)
    }

    /// TS `newSession`.
    pub async fn new_session(
        &mut self,
        parent_session: Option<&str>,
        setup: Option<NewSessionSetupFn>,
        with_session: Option<WithSessionFn>,
        gens: &Generators,
    ) -> Result<bool, String> {
        if self.emit_before_switch("new", None).await {
            return Ok(true);
        }

        let previous_session_file = self.session.session_manager.get_session_file().map(str::to_string);
        let session_dir = self.session.session_manager.get_session_dir().to_string();
        let mut session_manager = if self.session.session_manager.is_persisted() {
            SessionManager::create(self.cwd(), Some(&session_dir), None, gens)?
        } else {
            SessionManager::in_memory(self.cwd(), None, Vec::new(), gens)?
        };
        if let Some(parent) = parent_session {
            session_manager.new_session(Some(&pi_core::session_manager::NewSessionOptions {
                parent_session: Some(parent.to_string()),
                ..Default::default()
            }))?;
        }

        let target = session_manager.get_session_file().map(str::to_string);
        self.teardown_current("new", target).await;
        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: self.cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
            session_start_event: Some(serde_json::json!({
                "type": "session_start",
                "reason": "new",
                "previousSessionFile": previous_session_file,
            })),
            project_trust_context: None,
        })
        .await?;
        self.apply(result);
        if let Some(setup) = setup {
            let setup = setup.clone();
            setup(&mut self.session.session_manager).await;
            self.session.refresh_context();
        }
        self.finish_session_replacement(with_session).await;
        Ok(false)
    }

    /// TS `fork` — `Ok((cancelled, selected_text))`.
    pub async fn fork(
        &mut self,
        entry_id: &str,
        position: Option<&str>,
        with_session: Option<WithSessionFn>,
        gens: &Generators,
    ) -> Result<(bool, Option<String>), String> {
        let position = position.unwrap_or("before");
        if self.emit_before_fork(entry_id, position).await {
            return Ok((true, None));
        }
        let mut selected_text: Option<String> = None;

        let selected_entry = self
            .session
            .session_manager
            .get_entry(entry_id)
            .cloned()
            .ok_or_else(|| "Invalid entry ID for forking".to_string())?;

        let target_leaf_id: Option<String> = if position == "at" {
            selected_entry.get("id").and_then(Value::as_str).map(str::to_string)
        } else {
            if selected_entry.get("type").and_then(Value::as_str) != Some("message")
                || selected_entry
                    .pointer("/message/role")
                    .and_then(Value::as_str)
                    != Some("user")
            {
                return Err("Invalid entry ID for forking".to_string());
            }
            selected_text = Some(extract_user_message_text(
                selected_entry.get("message").and_then(|m| m.get("content")).unwrap_or(&Value::Null),
            ));
            selected_entry.get("parentId").and_then(Value::as_str).map(str::to_string)
        };

        let previous_session_file = self.session.session_manager.get_session_file().map(str::to_string);
        if self.session.session_manager.is_persisted() {
            let current_session_file = self
                .session
                .session_manager
                .get_session_file()
                .map(str::to_string)
                .ok_or_else(|| "Persisted session is missing a session file".to_string())?;
            let session_dir = self.session.session_manager.get_session_dir().to_string();
            let target = target_leaf_id.clone();
            if target.is_none() {
                let mut session_manager = SessionManager::create(self.cwd(), Some(&session_dir), None, gens)?;
                session_manager.new_session(Some(&pi_core::session_manager::NewSessionOptions {
                    parent_session: Some(current_session_file.clone()),
                    ..Default::default()
                }))?;
                let file = session_manager.get_session_file().map(str::to_string);
                self.teardown_current("fork", file).await;
                let result = (self.create_runtime)(CreateRuntimeOptions {
                    cwd: self.cwd().to_string(),
                    agent_dir: self.services.agent_dir.clone(),
                    session_manager,
                    session_start_event: Some(serde_json::json!({
                        "type": "session_start",
                        "reason": "fork",
                        "previousSessionFile": previous_session_file,
                    })),
                    project_trust_context: None,
                })
                .await?;
                self.apply(result);
                self.finish_session_replacement(with_session).await;
                return Ok((false, selected_text));
            }

            if !std::path::Path::new(&current_session_file).exists() {
                return Err(
                    "This session has not been saved yet. Send a message before cloning or forking it."
                        .to_string(),
                );
            }
            let mut session_manager = SessionManager::open(&current_session_file, Some(&session_dir), None, gens)?;
            let target = target.expect("checked above");
            let forked_session_path = session_manager.create_branched_session(&target)?;
            if forked_session_path.is_none() {
                return Err("Failed to create forked session".to_string());
            }
            let file = session_manager.get_session_file().map(str::to_string);
            self.teardown_current("fork", file).await;
            let result = (self.create_runtime)(CreateRuntimeOptions {
                cwd: session_manager.get_cwd().to_string(),
                agent_dir: self.services.agent_dir.clone(),
                session_manager,
                session_start_event: Some(serde_json::json!({
                    "type": "session_start",
                    "reason": "fork",
                    "previousSessionFile": previous_session_file,
                })),
                project_trust_context: None,
            })
            .await?;
            self.apply(result);
            self.finish_session_replacement(with_session).await;
            return Ok((false, selected_text));
        }

        let fork_cwd = self.cwd().to_string();
        let mut session_manager = std::mem::replace(
            &mut self.session.session_manager,
            SessionManager::in_memory(&fork_cwd, None, Vec::new(), gens)?,
        );
        let target_file = session_manager.get_session_file().map(str::to_string);
        self.teardown_current("fork", target_file).await;
        match &target_leaf_id {
            None => {
                session_manager.new_session(Some(&pi_core::session_manager::NewSessionOptions {
                    parent_session: previous_session_file.clone(),
                    ..Default::default()
                }))?;
            }
            Some(target) => {
                session_manager.create_branched_session(target)?;
            }
        }
        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: self.cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
            session_start_event: Some(serde_json::json!({
                "type": "session_start",
                "reason": "fork",
                "previousSessionFile": previous_session_file,
            })),
            project_trust_context: None,
        })
        .await?;
        self.apply(result);
        self.finish_session_replacement(with_session).await;
        Ok((false, selected_text))
    }

    /// TS `importFromJsonl` — `Err` carries the byte-exact error messages
    /// (`SessionImportFileNotFoundError`, `MissingSessionCwdError`).
    pub async fn import_from_jsonl(
        &mut self,
        input_path: &str,
        cwd_override: Option<&str>,
        gens: &Generators,
    ) -> Result<bool, String> {
        let resolved_path = pi_core::session_manager::resolve_ambient(input_path);
        if !std::path::Path::new(&resolved_path).exists() {
            return Err(format!("File not found: {}", resolved_path));
        }

        let session_dir = self.session.session_manager.get_session_dir().to_string();
        if !std::path::Path::new(&session_dir).exists() {
            std::fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
        }

        let base = resolved_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&resolved_path)
            .to_string();
        let mut destination_path = join_path(&session_dir, &base);
        let source_already_stored =
            pi_core::session_manager::resolve_ambient(&destination_path) == resolved_path;
        if !source_already_stored {
            let name = base.split('.').next().unwrap_or(&base).to_string();
            let ext = base.rfind('.').map(|i| base[i..].to_string()).unwrap_or_default();
            let mut suffix = 1;
            while std::path::Path::new(&destination_path).exists() {
                destination_path = join_path(&session_dir, &format!("{}-{}{}", name, suffix, ext));
                suffix += 1;
            }
        }
        if self.emit_before_switch("resume", Some(&destination_path)).await {
            return Ok(true);
        }

        let previous_session_file = self.session.session_manager.get_session_file().map(str::to_string);
        if !source_already_stored {
            std::fs::copy(&resolved_path, &destination_path).map_err(|e| e.to_string())?;
        }

        let session_manager = SessionManager::open(&destination_path, Some(&session_dir), cwd_override, gens)?;
        assert_session_cwd_exists(&session_manager, self.cwd())?;
        let target = session_manager.get_session_file().map(str::to_string);
        self.teardown_current("resume", target).await;
        let result = (self.create_runtime)(CreateRuntimeOptions {
            cwd: session_manager.get_cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
            session_start_event: Some(serde_json::json!({
                "type": "session_start",
                "reason": "resume",
                "previousSessionFile": previous_session_file,
            })),
            project_trust_context: None,
        })
        .await?;
        self.apply(result);
        self.finish_session_replacement(None).await;
        Ok(false)
    }

    /// TS `dispose`.
    pub async fn dispose(&mut self) {
        self.emit_session_shutdown_event("quit", None).await;
        if let Some(callback) = &self.before_session_invalidate {
            callback();
        }
        self.session.dispose();
    }
}

/// TS `createAgentSessionRuntime`.
pub async fn create_agent_session_runtime(
    create_runtime: CreateRuntimeFactory,
    options: CreateRuntimeOptions,
) -> Result<AgentSessionRuntime, String> {
    assert_session_cwd_exists(&options.session_manager, &options.cwd)?;
    let cwd = options.cwd.clone();
    let result = create_runtime(options).await?;
    let _ = cwd;
    Ok(AgentSessionRuntime::new(
        result.session,
        result.services,
        create_runtime,
        result.diagnostics,
        result.model_fallback_message,
    ))
}
