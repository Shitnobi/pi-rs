//! Tool registry & loadout slice of `AgentSession` (TS
//! `agent-session.ts`: `_refreshToolRegistry` (L3525), `_applyToolLoadout`
//! (L1580), exposure/allowlist helpers (L1542–1578), `_rebuildSystemPrompt`
//! glue (L1705), `_restoreToolsFromTranscript` (L1815), the
//! hidden-declarations + forced-prompt projections (L1772–1813), and the
//! constructor's `_buildRuntime` slice (L3636)).
//!
//! Divergences from the TS (documented per item):
//! - The real `ExtensionRunner` is Phase 4+; [`super::ExtensionHost`] is the
//!   seam. It gains `get_all_registered_tools` (TS
//!   `runner.getAllRegisteredTools()`); the harness fake swaps in extension
//!   tools exactly where the TS constructor's real runner sees none.
//! - `prepareLoadout` hooks are TS closures; until the runner is real the
//!   definition carries a data spec ([`PrepareLoadoutSpec`]) that the
//!   harness injects from JSON. The hook's `loadout` argument is unused by
//!   the spec (the TS hook receives the full [`ToolLoadout`]; a host trait
//!   replaces the spec in Phase 4+). Hook errors emit `{extensionPath,
//!   event, error}` — the TS also carries `error.stack`, which the parity
//!   collector drops on both sides (V8 stacks are nondeterministic).
//! - `createAllToolDefinitions` (the eight builtin tool definitions) is not
//!   ported (Phase 4+): [`super::AgentSessionConfig::base_tools_override`]
//!   must carry the base tools; without it the base registry is empty and
//!   the default active names fall back to the TS list (they activate
//!   nothing). The settings the default definitions read
//!   (`getImageAutoResize`/`getShellCommandPrefix`/`getShellPath`) are
//!   therefore unread here.
//! - `_rebuildSystemPrompt` reads the resource loader
//!   (`getSystemPrompt`/`getAppendSystemPrompt`/`getSkills`/
//!   `getAgentsFiles`); the loader seam arrives with tranche 10, so the
//!   tools slice builds options over empty resources (the parity harness
//!   runs `DefaultResourceLoader` with `noExtensions` over a nonexistent
//!   cwd, which yields exactly those empties).
//! - The two request projections are installed in TS as chained
//!   `agent.transformContext` hooks; the Rust port keeps the state they
//!   read and applies them in installation order in
//!   [`AgentSession::transform_context`] (earlier hooks land with tranches
//!   10–11 and will chain ahead of these, like the TS).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use pi_agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture};
use pi_core::ai::{get_current_system_message, js_number};

use super::system_prompt::{build_system_prompt_sections, diff_system_prompt_sections};
use pi_core::js_string::is_js_whitespace_or_terminator;
use pi_core::mcp_servers::{create_tool_name_matcher, is_mcp_tool_name, ToolNameMatcher};
use pi_core::prompt_templates::SourceInfo;
use pi_core::settings_manager::apply_tool_modifiers;

use super::{AgentSession, AgentSessionConfig};

/// TS `ToolExposure` (extensions/types.ts L516).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Direct,
    ModelOnly,
    Codemode,
    Deferred,
    Hidden,
}

impl ToolExposure {
    fn from_str(s: &str) -> Option<ToolExposure> {
        Some(match s {
            "direct" => ToolExposure::Direct,
            "model-only" => ToolExposure::ModelOnly,
            "codemode" => ToolExposure::Codemode,
            "deferred" => ToolExposure::Deferred,
            "hidden" => ToolExposure::Hidden,
            _ => return None,
        })
    }

    fn as_str(&self) -> &'static str {
        match self {
            ToolExposure::Direct => "direct",
            ToolExposure::ModelOnly => "model-only",
            ToolExposure::Codemode => "codemode",
            ToolExposure::Deferred => "deferred",
            ToolExposure::Hidden => "hidden",
        }
    }
}

/// TS `prepareLoadout` hook — a data stand-in for the closure until the
/// extension runner is real (see module docs).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PrepareLoadoutSpec {
    /// Returns `{ descriptions, hiddenDeclarations }`.
    Changes {
        #[serde(default)]
        descriptions: Map<String, Value>,
        #[serde(default, rename = "hiddenDeclarations")]
        hidden_declarations: Vec<String>,
    },
    /// Throws `new Error(message)`.
    Throw { message: String },
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// TS `ToolDefinition` (extensions/types.ts L579) — the fields the tools
/// slice consumes. `execute` is unreachable here (see [`SessionAgentTool`]).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    pub description: String,
    #[serde(default = "empty_object")]
    pub parameters: Value,
    #[serde(default)]
    pub prompt_snippet: Option<String>,
    /// Raw `promptGuidelines` (getAllTools projects it verbatim; the
    /// registry normalizes a separate copy).
    #[serde(default)]
    pub prompt_guidelines: Option<Vec<String>>,
    #[serde(default)]
    pub exposure: Option<ToolExposureValue>,
    #[serde(default)]
    pub namespace: Option<Value>,
    #[serde(default)]
    pub annotations: Option<Value>,
    #[serde(default)]
    pub reaches_tools: Option<Value>,
    #[serde(default)]
    pub default_active: Option<bool>,
    #[serde(default)]
    pub prepare_loadout: Option<PrepareLoadoutSpec>,
    /// The wrapped AgentTool this definition came from
    /// (`createToolDefinitionFromAgentTool` keeps execution reachable).
    #[serde(skip, default)]
    pub agent_tool: Option<Arc<dyn AgentTool>>,
}

/// Deserialization helper so `exposure` rejects unknown values like TS
/// would at the type level.
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct ToolExposureValue(String);

impl ToolExposureValue {
    fn get(&self) -> Option<ToolExposure> {
        ToolExposure::from_str(&self.0)
    }
}

impl ToolDefinition {
    /// TS `definition.exposure ?? "direct"` — an unrecognized exposure
    /// string never reaches here (the harness validates it up front).
    pub fn exposure(&self) -> ToolExposure {
        self.exposure.as_ref().and_then(|v| v.get()).unwrap_or(ToolExposure::Direct)
    }
}

/// TS `ToolDefinitionEntry` (agent-session.ts L362).
#[derive(Clone)]
pub struct ToolDefinitionEntry {
    pub definition: ToolDefinition,
    pub source_info: SourceInfo,
}

/// TS `RegisteredTool` (extensions/types.ts) — what
/// `ExtensionHost::get_all_registered_tools` returns.
#[derive(Clone)]
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    pub source_info: SourceInfo,
}

/// TS `BUILTIN_PATH_PREFIX` (source-info.ts).
pub const BUILTIN_PATH_PREFIX: &str = "builtin:";

/// TS `createSyntheticSourceInfo(path, {source, scope?, origin?, baseDir?})`
/// (core/source-info.ts L41).
pub fn create_synthetic_source_info(path: &str, source: &str) -> SourceInfo {
    SourceInfo {
        path: path.to_string(),
        source: source.to_string(),
        scope: pi_core::prompt_templates::SourceScope::Temporary,
        origin: pi_core::prompt_templates::SourceOrigin::TopLevel,
        base_dir: None,
    }
}

/// TS `normalizeBuildSystemPromptOptions` (system-prompt.ts L60) — the
/// shallow normalization `_rebuildSystemPrompt` stores in
/// `_baseSystemPromptOptions`. `undefined` fields are skipped on
/// serialization (JSON drops them, like the TS).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSystemPromptOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force_system_prompt: Option<String>,
    pub selected_tools: Vec<String>,
    pub hidden_tools: Vec<String>,
    pub tool_snippets: Map<String, Value>,
    pub tool_guidelines: Map<String, Value>,
    pub prompt_guidelines: Vec<String>,
    pub append_system_prompt: String,
    pub sections: Map<String, Value>,
    pub cwd: String,
    pub context_files: Vec<Value>,
    pub skills: Vec<Value>,
}

/// The wrapped `AgentTool` the registry holds (TS
/// `wrapRegisteredTools` → `wrapToolDefinition`). Execution delegates to
/// the originating tool when the definition came from one; harness-built
/// definitions never execute.
pub struct SessionAgentTool {
    name: String,
    label: Option<String>,
    description: String,
    parameters: Value,
    inner: Option<Arc<dyn AgentTool>>,
}

impl SessionAgentTool {
    /// TS `wrapToolDefinition(definition, ctxFactory)`.
    pub fn from_definition(definition: &ToolDefinition) -> Arc<SessionAgentTool> {
        Arc::new(SessionAgentTool {
            name: definition.name.clone(),
            label: definition.label.clone(),
            description: definition.description.clone(),
            parameters: definition.parameters.clone(),
            inner: definition.agent_tool.clone(),
        })
    }

    /// TS `{ ...tool, description }` (the `prepareLoadout` override).
    pub fn with_description(&self, description: &str) -> Arc<SessionAgentTool> {
        Arc::new(SessionAgentTool {
            name: self.name.clone(),
            label: self.label.clone(),
            description: description.to_string(),
            parameters: self.parameters.clone(),
            inner: self.inner.clone(),
        })
    }
}

impl AgentTool for SessionAgentTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
    fn parameters(&self) -> &Value {
        &self.parameters
    }
    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        args: &'a Value,
        signal: Option<&'a pi_agent::host::AbortHandle>,
        on_update: Option<&'a AgentToolUpdateCallback<'a>>,
    ) -> BoxFuture<'a, Result<AgentToolResult, String>> {
        match &self.inner {
            Some(tool) => tool.execute(tool_call_id, args, signal, on_update),
            None => Box::pin(async {
                Err("agent-session tool execution is unreachable in this slice".to_string())
            }),
        }
    }
}

/// TS `wrapRegisteredTools(registeredTools, runner)` (extensions/wrapper.ts
/// L27) — the runner context factory only matters at execute time.
pub fn wrap_registered_tools(registered: &[RegisteredTool]) -> Vec<Arc<SessionAgentTool>> {
    registered.iter().map(|t| SessionAgentTool::from_definition(&t.definition)).collect()
}

/// TS `createToolDefinitionFromAgentTool` (tools/tool-definition-wrapper.ts)
/// — keeps the tool reachable for execution; carries no prompt metadata.
pub fn create_tool_definition_from_agent_tool(tool: &Arc<dyn AgentTool>) -> ToolDefinition {
    ToolDefinition {
        name: tool.name().to_string(),
        label: Some(tool.label().to_string()),
        description: tool.description().to_string(),
        parameters: tool.parameters().clone(),
        prompt_snippet: None,
        prompt_guidelines: None,
        exposure: None,
        namespace: None,
        annotations: None,
        reaches_tools: None,
        default_active: None,
        prepare_loadout: None,
        agent_tool: Some(Arc::clone(tool)),
    }
}

/// The tools half of `SessionState` (TS private fields; see module docs for
/// the ordered-map stands for TS `Map` insertion order).
pub(crate) struct ToolsState {
    /// TS `_baseToolsOverride` (config; `_buildRuntime` re-reads it).
    pub base_tools_override: Option<Vec<(String, Arc<dyn AgentTool>)>>,
    /// TS `_customTools`.
    pub custom_tools: Vec<ToolDefinition>,
    /// TS `_usesDefaultTools`.
    pub uses_default_tools: bool,
    /// TS `_defaultToolModifiers`.
    pub default_tool_modifiers: Vec<String>,
    /// TS `_allowedTools`.
    pub allowed_tools: Option<ToolNameMatcher>,
    /// TS `_allowlistFiltersMcp`.
    pub allowlist_filters_mcp: bool,
    /// TS `_excludedTools`.
    pub excluded_tools: Option<ToolNameMatcher>,
    /// TS `_baseToolDefinitions` (ordered map key → definition).
    pub base_tool_definitions: Vec<(String, ToolDefinition)>,
    /// TS `_pendingToolNames` (insertion-ordered set).
    pub pending_tool_names: Vec<String>,
    /// TS `_toolRegistry` (ordered map name → wrapped tool).
    pub tool_registry: Vec<(String, Arc<SessionAgentTool>)>,
    /// TS `_hiddenDeclarations` (insertion-ordered set).
    pub hidden_declarations: Vec<String>,
    /// TS `_toolDefinitions` (ordered map).
    pub tool_definitions: Vec<(String, ToolDefinitionEntry)>,
    /// TS `_toolPromptSnippets` (ordered map).
    pub tool_prompt_snippets: Vec<(String, String)>,
    /// TS `_toolPromptGuidelines` (ordered map).
    pub tool_prompt_guidelines: Vec<(String, Vec<String>)>,
    /// TS `_baseSystemPromptOptions`.
    pub base_system_prompt_options: NormalizedSystemPromptOptions,
    /// TS `_runSystemPromptOptions` — a raw Value (camelCase) once the
    /// run pipeline (tranche 11) assigns handler-edited options.
    pub run_system_prompt_options: Option<Value>,
    /// TS `resourceLoader.getSystemPrompt()` (config seam).
    pub resource_custom_prompt: Option<String>,
}

impl ToolsState {
    pub fn new(config: &AgentSessionConfig) -> ToolsState {
        let allowed_tools = config
            .allowed_tool_names
            .as_ref()
            .map(|names| create_tool_name_matcher(names));
        // TS: the flag is set only when `config.allowedToolNames` is truthy
        // (an empty array is truthy in JS).
        let allowlist_filters_mcp = match &config.allowed_tool_names {
            Some(names) => names.is_empty() || names.iter().any(|n| n.starts_with("mcp__")),
            None => false,
        };
        let excluded_tools = config
            .excluded_tool_names
            .as_ref()
            .map(|names| create_tool_name_matcher(names));
        ToolsState {
            base_tools_override: config.base_tools_override.clone(),
            custom_tools: config.custom_tools.clone(),
            uses_default_tools: config.uses_default_tools,
            default_tool_modifiers: config.default_tool_modifiers.clone(),
            allowed_tools,
            allowlist_filters_mcp,
            excluded_tools,
            base_tool_definitions: Vec::new(),
            pending_tool_names: Vec::new(),
            tool_registry: Vec::new(),
            hidden_declarations: Vec::new(),
            tool_definitions: Vec::new(),
            tool_prompt_snippets: Vec::new(),
            tool_prompt_guidelines: Vec::new(),
            base_system_prompt_options: NormalizedSystemPromptOptions {
                custom_prompt: config.system_prompt.clone(),
                force_system_prompt: None,
                selected_tools: Vec::new(),
                hidden_tools: Vec::new(),
                tool_snippets: Map::new(),
                tool_guidelines: Map::new(),
                prompt_guidelines: Vec::new(),
                append_system_prompt: String::new(),
                sections: Map::new(),
                cwd: config.cwd.clone(),
                context_files: Vec::new(),
                skills: Vec::new(),
            },
            run_system_prompt_options: None,
            resource_custom_prompt: config.system_prompt.clone(),
        }
    }

    fn registry_has(&self, name: &str) -> bool {
        self.tool_registry.iter().any(|(n, _)| n == name)
    }

    fn registry_get(&self, name: &str) -> Option<Arc<SessionAgentTool>> {
        self.tool_registry.iter().find(|(n, _)| n == name).map(|(_, t)| Arc::clone(t))
    }

    fn definitions_get(&self, name: &str) -> Option<&ToolDefinitionEntry> {
        self.tool_definitions.iter().find(|(n, _)| n == name).map(|(_, e)| e)
    }

    fn pending_push(&mut self, name: &str) {
        if !self.pending_tool_names.iter().any(|n| n == name) {
            self.pending_tool_names.push(name.to_string());
        }
    }
}

/// TS `Map.set`: replace an existing key in place, else append.
fn set_ordered<T>(map: &mut Vec<(String, T)>, key: String, value: T) {
    match map.iter_mut().find(|(n, _)| *n == key) {
        Some(slot) => slot.1 = value,
        None => map.push((key, value)),
    }
}

/// JS truthiness for the JSON-carried definition fields.
fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// TS `_normalizePromptSnippet` (agent-session.ts): collapse runs of
/// `\s`-class chars (JS `\s` = WhiteSpace + LineTerminator) to one space
/// and trim (JS `String.prototype.trim` strips the same set).
fn normalize_prompt_snippet(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if is_js_whitespace_or_terminator(c) {
            in_space = true;
        } else {
            if in_space && !out.is_empty() {
                out.push(' ');
            }
            in_space = false;
            out.push(c);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// TS `_normalizePromptGuidelines`: trim each entry, drop empties, dedupe
/// preserving first occurrence.
fn normalize_prompt_guidelines(guidelines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in guidelines {
        let trimmed: String = g
            .trim_matches(is_js_whitespace_or_terminator)
            .to_string();
        if !trimmed.is_empty() && !out.contains(&trimmed) {
            out.push(trimmed);
        }
    }
    out
}

impl AgentSession {
    // ========================================================================
    // Public surface (TS getters/methods)
    // ========================================================================

    /// TS `getActiveToolNames()`.
    pub fn get_active_tool_names(&self) -> Vec<String> {
        self.agent.tools().iter().map(|t| t.name().to_string()).collect()
    }

    /// TS `getAllTools()` — the `ToolInfo` projection.
    pub fn get_all_tools(&self) -> Vec<Value> {
        self.state
            .tools
            .tool_definitions
            .iter()
            .map(|(_, entry)| {
                let d = &entry.definition;
                let mut o = Map::new();
                o.insert("name".into(), json!(d.name));
                o.insert("description".into(), json!(d.description));
                o.insert("parameters".into(), d.parameters.clone());
                if let Some(g) = &d.prompt_guidelines {
                    o.insert("promptGuidelines".into(), json!(g));
                }
                o.insert(
                    "exposure".into(),
                    json!(self.get_tool_exposure(&d.name).as_str()),
                );
                if let Some(n) = &d.namespace {
                    if js_truthy(n) {
                        o.insert("namespace".into(), n.clone());
                    }
                }
                if let Some(a) = &d.annotations {
                    if js_truthy(a) {
                        o.insert("annotations".into(), a.clone());
                    }
                }
                if let Some(r) = &d.reaches_tools {
                    if js_truthy(r) {
                        o.insert("reachesTools".into(), r.clone());
                    }
                }
                o.insert("sourceInfo".into(), serde_json::to_value(&entry.source_info).unwrap());
                Value::Object(o)
            })
            .collect()
    }

    /// TS `getCallableToolNames()`.
    pub fn get_callable_tool_names(&self) -> Vec<String> {
        self.get_callable_tools().iter().map(|t| t.name().to_string()).collect()
    }

    /// TS `setActiveToolsByName(toolNames)`.
    pub fn set_active_tools_by_name(&mut self, tool_names: Vec<String>) {
        let previous = self.get_active_tool_names();
        self.set_active_tools_internal(&tool_names);
        // A loadout that deactivates a tool replaces the restored one, whose
        // pending tools are dropped (TS).
        let active = self.get_active_tool_names();
        if previous.iter().any(|name| !active.contains(name)) {
            self.state.tools.pending_tool_names.clear();
        }
    }

    // ========================================================================
    // Exposure / allowlist algebra (TS L1542–1578)
    // ========================================================================

    /// TS `_isAllowedTool(name)`.
    fn is_allowed_tool(&self, name: &str) -> bool {
        if let Some(excluded) = &self.state.tools.excluded_tools {
            if excluded.matches(name) {
                return false;
            }
        }
        if let Some(allowed) = &self.state.tools.allowed_tools {
            if !allowed.matches(name) {
                return !self.state.tools.allowlist_filters_mcp && is_mcp_tool_name(name);
            }
        }
        true
    }

    /// TS `_isActivatable(name)`.
    fn is_activatable(&self, name: &str) -> bool {
        if self.state.tools.allowed_tools.as_ref().is_none_or(|a| a.matches(name)) {
            return true;
        }
        if !is_mcp_tool_name(name) {
            return true;
        }
        self.get_tool_exposure(name) != ToolExposure::Direct
            && self.state.tools.registry_has("tool_search")
    }

    /// TS `_getToolExposure(name)`.
    pub fn get_tool_exposure(&self, name: &str) -> ToolExposure {
        self.state
            .tools
            .definitions_get(name)
            .map(|e| e.definition.exposure())
            .unwrap_or(ToolExposure::Direct)
    }

    /// TS `_getCallableTools(active)`.
    pub fn get_callable_tools(&self) -> Vec<Arc<SessionAgentTool>> {
        let active: Vec<String> = self.get_active_tool_names();
        self.state
            .tools
            .tool_registry
            .iter()
            .filter(|(_, tool)| {
                let exposure = self.get_tool_exposure(tool.name());
                exposure == ToolExposure::Codemode
                    || exposure == ToolExposure::Deferred
                    || (exposure == ToolExposure::Direct && active.contains(&tool.name().to_string()))
            })
            .map(|(_, t)| Arc::clone(t))
            .collect()
    }

    /// TS `_isDeclarable(name)`.
    fn is_declarable(&self, name: &str) -> bool {
        let exposure = self.get_tool_exposure(name);
        exposure == ToolExposure::Direct || exposure == ToolExposure::ModelOnly
    }

    /// TS `_isActivatedOnRegistration(name)`.
    fn is_activated_on_registration(&self, name: &str) -> bool {
        self.is_declarable(name)
            && self.state.tools.definitions_get(name).map(|e| e.definition.default_active) != Some(Some(false))
    }

    // ========================================================================
    // Loadout (TS `_applyToolLoadout` / `_setActiveTools`)
    // ========================================================================

    /// TS `_setActiveTools(toolNames)`.
    fn set_active_tools_internal(&mut self, tool_names: &[String]) {
        let tools = self.apply_tool_loadout(tool_names);
        for tool in &tools {
            let name = tool.name().to_string();
            self.state.tools.pending_tool_names.retain(|n| *n != name);
        }
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        self.rebuild_system_prompt(&names);
    }

    /// TS `_applyToolLoadout(toolNames)` — sets `agent.state.tools` and
    /// returns the declared tools.
    pub fn apply_tool_loadout(&mut self, tool_names: &[String]) -> Vec<Arc<SessionAgentTool>> {
        let mut seen: Vec<String> = Vec::new();
        let mut tools: Vec<Arc<SessionAgentTool>> = Vec::new();
        for name in tool_names {
            if seen.iter().any(|n| n == name) {
                continue;
            }
            seen.push(name.clone());
            if let Some(tool) = self.state.tools.registry_get(name) {
                if self.get_tool_exposure(name) != ToolExposure::Hidden && self.is_activatable(name) {
                    tools.push(tool);
                }
            }
        }
        let hooks: Vec<ToolDefinitionEntry> = tools
            .iter()
            .filter_map(|tool| {
                let entry = self.state.tools.definitions_get(tool.name())?;
                if entry.definition.prepare_loadout.is_some() {
                    Some(entry.clone())
                } else {
                    None
                }
            })
            .collect();
        let mut hidden: Vec<String> = Vec::new();
        let mut declared = tools.clone();
        if !hooks.is_empty() {
            // TS builds the `ToolLoadout` the closures receive; the data
            // spec ignores it (module docs).
            let mut descriptions: Vec<(String, String)> = Vec::new();
            for entry in &hooks {
                match &entry.definition.prepare_loadout {
                    Some(PrepareLoadoutSpec::Changes {
                        descriptions: desc,
                        hidden_declarations,
                    }) => {
                        for (name, description) in desc {
                            if let Some(text) = description.as_str() {
                                descriptions.retain(|(n, _)| n != name);
                                descriptions.push((name.clone(), text.to_string()));
                            }
                        }
                        for name in hidden_declarations {
                            if !hidden.contains(name) {
                                hidden.push(name.clone());
                            }
                        }
                    }
                    Some(PrepareLoadoutSpec::Throw { message }) => {
                        self.extension_host.emit_error(json!({
                            "extensionPath": entry.source_info.path,
                            "event": "prepare_loadout",
                            "error": message,
                        }));
                    }
                    None => {}
                }
            }
            declared = tools
                .iter()
                .map(|tool| {
                    match descriptions.iter().find(|(n, _)| n == tool.name()) {
                        Some((_, description)) => tool.with_description(description),
                        None => Arc::clone(tool),
                    }
                })
                .collect();
        }
        self.state.tools.hidden_declarations = hidden;
        let declared_dyn: Vec<Arc<dyn AgentTool>> =
            declared.iter().map(|t| Arc::clone(t) as Arc<dyn AgentTool>).collect();
        self.agent.set_tools(declared_dyn);
        declared
    }

    // ========================================================================
    // System prompt glue (TS `_rebuildSystemPrompt` L1705)
    // ========================================================================

    /// TS `_rebuildSystemPrompt(toolNames)`.
    fn rebuild_system_prompt(&mut self, tool_names: &[String]) {
        let valid_tool_names: Vec<String> = tool_names
            .iter()
            .filter(|name| self.state.tools.registry_has(name))
            .cloned()
            .collect();
        let mut tool_snippets = Map::new();
        for (name, _) in &self.state.tools.tool_registry {
            if let Some((_, snippet)) =
                self.state.tools.tool_prompt_snippets.iter().find(|(n, _)| n == name)
            {
                tool_snippets.insert(name.clone(), json!(snippet));
            }
        }
        let mut tool_guidelines = Map::new();
        for (name, guidelines) in &self.state.tools.tool_prompt_guidelines {
            tool_guidelines.insert(name.clone(), json!(guidelines));
        }
        // Resource-loader reads (empty here; module docs).
        self.state.tools.base_system_prompt_options = NormalizedSystemPromptOptions {
            custom_prompt: self.state.tools.resource_custom_prompt.clone(),
            force_system_prompt: None,
            selected_tools: valid_tool_names,
            hidden_tools: self.state.tools.hidden_declarations.clone(),
            tool_snippets,
            tool_guidelines,
            prompt_guidelines: Vec::new(),
            append_system_prompt: String::new(),
            sections: Map::new(),
            cwd: self.cwd.clone(),
            context_files: Vec::new(),
            skills: Vec::new(),
        };
    }

    // ========================================================================
    // Transcript restore (TS `_restoreToolsFromTranscript` L1815)
    // ========================================================================

    /// TS `_restoreToolsFromTranscript(modifiers)`.
    pub fn restore_tools_from_transcript(&mut self, modifiers: &[String]) {
        self.state.tools.pending_tool_names.clear();
        let context = self.session_manager.build_session_context();
        let messages: Vec<Value> = context
            .messages
            .iter()
            .map(|m| serde_json::to_value(m).expect("message serializes"))
            .collect();
        let Some(current) = get_current_system_message(&messages) else { return };
        let names: Vec<String> = current
            .get("toolsAdded")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let names = apply_tool_modifiers(&names, modifiers);
        for name in &names {
            if self.is_allowed_tool(name) {
                self.state.tools.pending_push(name);
            }
        }
        self.set_active_tools_internal(&names);
    }

    // ========================================================================
    // Registry refresh (TS `_refreshToolRegistry` L3525)
    // ========================================================================

    /// TS `_refreshToolRegistry({activeToolNames?, includeAllExtensionTools?})`.
    pub fn refresh_tool_registry(
        &mut self,
        active_tool_names: Option<Vec<String>>,
        include_all_extension_tools: bool,
    ) {
        let previous_activated_on_registration: Vec<String> = self
            .state
            .tools
            .tool_registry
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|name| self.is_activated_on_registration(name))
            .collect();
        let previous_active_tool_names = self.get_active_tool_names();
        let has_allowed_tools = self.state.tools.allowed_tools.is_some();

        let registered_tools = self.extension_host.get_all_registered_tools();
        let mut all_custom_tools: Vec<RegisteredTool> = registered_tools;
        for definition in self.state.tools.custom_tools.clone() {
            all_custom_tools.push(RegisteredTool {
                source_info: create_synthetic_source_info(
                    &format!("<sdk:{}>", definition.name),
                    "sdk",
                ),
                definition,
            });
        }
        let all_custom_tools: Vec<RegisteredTool> = all_custom_tools
            .into_iter()
            .filter(|t| self.is_allowed_tool(&t.definition.name))
            .collect();

        let mut definition_registry: Vec<(String, ToolDefinitionEntry)> = Vec::new();
        for (name, definition) in self.state.tools.base_tool_definitions.clone() {
            if self.is_allowed_tool(&name) {
                definition_registry.push((
                    name.clone(),
                    ToolDefinitionEntry {
                        definition,
                        source_info: create_synthetic_source_info(
                            &format!("{}{}", BUILTIN_PATH_PREFIX, name),
                            "builtin",
                        ),
                    },
                ));
            }
        }
        for tool in &all_custom_tools {
            // TS `Map.set` replaces an existing key in place (insertion
            // order is preserved).
            let entry = ToolDefinitionEntry {
                definition: tool.definition.clone(),
                source_info: tool.source_info.clone(),
            };
            match definition_registry.iter_mut().find(|(n, _)| *n == tool.definition.name) {
                Some(slot) => slot.1 = entry,
                None => definition_registry.push((tool.definition.name.clone(), entry)),
            }
        }
        self.state.tools.tool_definitions = definition_registry;

        let entries: Vec<ToolDefinitionEntry> = self
            .state
            .tools
            .tool_definitions
            .iter()
            .map(|(_, e)| e.clone())
            .collect();
        self.state.tools.tool_prompt_snippets = entries
            .iter()
            .filter_map(|e| {
                normalize_prompt_snippet(e.definition.prompt_snippet.as_deref()?)
                    .map(|s| (e.definition.name.clone(), s))
            })
            .collect();
        self.state.tools.tool_prompt_guidelines = entries
            .iter()
            .filter_map(|e| {
                let g = normalize_prompt_guidelines(e.definition.prompt_guidelines.as_deref()?);
                if g.is_empty() {
                    None
                } else {
                    Some((e.definition.name.clone(), g))
                }
            })
            .collect();

        let wrapped_extension_tools = wrap_registered_tools(&all_custom_tools);
        let wrapped_builtin_tools: Vec<Arc<SessionAgentTool>> = self
            .state
            .tools
            .base_tool_definitions
            .iter()
            .filter(|(_, definition)| self.is_allowed_tool(&definition.name))
            .map(|(_, definition)| SessionAgentTool::from_definition(definition))
            .collect();

        let mut tool_registry: Vec<(String, Arc<SessionAgentTool>)> = Vec::new();
        for tool in &wrapped_builtin_tools {
            set_ordered(&mut tool_registry, tool.name().to_string(), Arc::clone(tool));
        }
        for tool in &wrapped_extension_tools {
            set_ordered(&mut tool_registry, tool.name().to_string(), Arc::clone(tool));
        }
        self.state.tools.tool_registry = tool_registry;

        let mut next_active_tool_names: Vec<String> = match &active_tool_names {
            Some(names) => names.clone(),
            None => previous_active_tool_names.clone(),
        };
        next_active_tool_names.retain(|name| self.is_allowed_tool(name));

        if has_allowed_tools {
            for (tool_name, _) in &self.state.tools.tool_registry {
                // Naming or matching a tool activates it even when it is not
                // active by default; MCP tools kept registered without being
                // named stay inactive (TS).
                if self.state.tools.allowed_tools.as_ref().is_some_and(|a| a.matches(tool_name))
                    && self.is_declarable(tool_name)
                {
                    next_active_tool_names.push(tool_name.clone());
                }
            }
        } else if include_all_extension_tools {
            for tool in &wrapped_extension_tools {
                if self.is_activated_on_registration(tool.name()) {
                    next_active_tool_names.push(tool.name().to_string());
                }
            }
        } else if active_tool_names.is_none() {
            for (tool_name, _) in &self.state.tools.tool_registry {
                if !previous_activated_on_registration.contains(tool_name)
                    && self.is_activated_on_registration(tool_name)
                {
                    next_active_tool_names.push(tool_name.clone());
                }
            }
        }
        // Pending tools that are registered now become active (TS).
        for name in &self.state.tools.pending_tool_names {
            next_active_tool_names.push(name.clone());
        }

        self.set_active_tools_internal(&next_active_tool_names);
    }

    // ========================================================================
    // Runtime build (TS `_buildRuntime` L3636, constructor slice)
    // ========================================================================

    /// TS `_buildRuntime({activeToolNames?, includeAllExtensionTools?})` —
    /// the tools slice (extension-runner construction and core binding are
    /// the injected [`super::ExtensionHost`]; module docs).
    pub fn build_runtime(
        &mut self,
        active_tool_names: Option<Vec<String>>,
        include_all_extension_tools: bool,
    ) {
        let base_tool_definitions: Vec<(String, ToolDefinition)> =
            match &self.state.tools.base_tools_override {
                Some(entries) => entries
                    .iter()
                    .map(|(name, tool)| (name.clone(), create_tool_definition_from_agent_tool(tool)))
                    .collect(),
                // No `createAllToolDefinitions` port yet (module docs): the
                // base registry is empty and the default names below
                // activate nothing.
                None => Vec::new(),
            };
        let default_active_tool_names: Vec<String> = match &self.state.tools.base_tools_override {
            Some(entries) => entries.iter().map(|(name, _)| name.clone()).collect(),
            None => vec![
                "read".to_string(),
                "bash".to_string(),
                "edit".to_string(),
                "write".to_string(),
            ],
        };
        self.state.tools.base_tool_definitions = base_tool_definitions;
        let base_active_tool_names = active_tool_names.unwrap_or(default_active_tool_names);
        self.refresh_tool_registry(Some(base_active_tool_names), include_all_extension_tools);
    }

    // ========================================================================
    // Request projections (TS L1772–1813)
    // ========================================================================

    /// The chained `agent.transformContext` the constructor installs
    /// (`_installHiddenDeclarationsProjection`, then
    /// `_installAgentForcedPromptProjection`).
    pub async fn transform_context(&self, messages: Vec<Value>) -> Vec<Value> {
        // Hidden declarations: filter toolsAdded/toolsRemoved on system
        // messages across the whole transcript.
        let hidden = self.state.tools.hidden_declarations.clone();
        let mut transformed = messages;
        if !hidden.is_empty() {
            transformed = transformed
                .into_iter()
                .map(|message| {
                    let mut message = message;
                    if message.get("role").and_then(Value::as_str) != Some("system") {
                        return message;
                    }
                    let obj = message.as_object_mut().expect("system message is an object");
                    let has_added = obj.get("toolsAdded").is_some_and(js_truthy);
                    let has_removed = obj.get("toolsRemoved").is_some_and(js_truthy);
                    if !has_added && !has_removed {
                        return message;
                    }
                    let added: Vec<Value> = obj
                        .get("toolsAdded")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter(|t| {
                                    !t.get("name").and_then(Value::as_str).is_some_and(|n| hidden.iter().any(|h| h == n))
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    let removed: Vec<Value> = obj
                        .get("toolsRemoved")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter(|t| {
                                    !t.get("name").and_then(Value::as_str).is_some_and(|n| hidden.iter().any(|h| h == n))
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    obj.remove("toolsAdded");
                    obj.remove("toolsRemoved");
                    if !added.is_empty() {
                        obj.insert("toolsAdded".to_string(), Value::Array(added));
                    }
                    if !removed.is_empty() {
                        obj.insert("toolsRemoved".to_string(), Value::Array(removed));
                    }
                    message
                })
                .collect();
        }
        // Forced prompt: collapse the system messages into one head holding
        // the forced text and the current tools.
        let forced = self
            .state
            .tools
            .run_system_prompt_options
            .as_ref()
            .and_then(|o| o.get("forceSystemPrompt"))
            .filter(|v| !v.is_null())
            .and_then(Value::as_str)
            .map(String::from);
        let Some(forced) = forced else { return transformed };
        let current = get_current_system_message(&transformed);
        let mut head = Map::new();
        head.insert("role".to_string(), json!("system"));
        head.insert("content".to_string(), json!(forced));
        if let Some(tools_added) = current.as_ref().and_then(|c| c.get("toolsAdded")) {
            if js_truthy(tools_added) {
                head.insert("toolsAdded".to_string(), tools_added.clone());
            }
        }
        let timestamp = current
            .as_ref()
            .and_then(|c| c.get("timestamp").cloned())
            .unwrap_or_else(|| js_number(self.clock.date_now_ms()));
        head.insert("timestamp".to_string(), timestamp);
        let mut out = vec![Value::Object(head)];
        out.extend(transformed.into_iter().filter(|m| m.get("role").and_then(Value::as_str) != Some("system")));
        out
    }

    // ========================================================================
    // Harness seams (TS parity harness reaches privates via any-cast)
    // ========================================================================

    /// Harness seam: TS any-cast `_runSystemPromptOptions = {forceSystemPrompt}`
    /// / `undefined`.
    pub fn set_forced_prompt(&mut self, text: Option<String>) {
        self.state.tools.run_system_prompt_options = text.map(|force_system_prompt| {
            let mut options = self.base_system_prompt_options_value();
            options
                .as_object_mut()
                .expect("base options are an object")
                .insert("forceSystemPrompt".to_string(), json!(force_system_prompt));
            options
        });
    }

    /// TS `_hiddenDeclarations` (insertion order).
    pub fn hidden_declarations(&self) -> &[String] {
        &self.state.tools.hidden_declarations
    }

    /// TS `_baseSystemPromptOptions`.
    pub fn system_prompt_options(&self) -> Value {
        serde_json::to_value(&self.state.tools.base_system_prompt_options).expect("options serialize")
    }

    /// TS `_pendingToolNames` (insertion order).
    pub fn pending_tool_names(&self) -> &[String] {
        &self.state.tools.pending_tool_names
    }
}

impl AgentSession {
    /// `_baseSystemPromptOptions` as the raw JSON the run slice and
    /// handlers pass around.
    pub(crate) fn base_system_prompt_options_value(&self) -> Value {
        serde_json::to_value(&self.state.tools.base_system_prompt_options)
            .expect("base options serialize")
    }

    /// TS `_preparePromptAndToolLoadout(options, messages?)` — applies the
    /// loadout, syncs `hiddenTools`, and diffs the system-prompt sections
    /// against the current system message. Mutates `options` in place
    /// (selectedTools/hiddenTools), returning the system patch message.
    pub(crate) fn prepare_prompt_and_tool_loadout(
        &mut self,
        options: &mut Value,
    ) -> Option<Value> {
        let selected: Vec<String> = options
            .get("selectedTools")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default();
        let applied = self.apply_tool_loadout(&selected);
        let names: Vec<Value> = applied.iter().map(|t| json!(t.name())).collect();
        if let Some(object) = options.as_object_mut() {
            object.insert("selectedTools".to_string(), Value::Array(names));
            object.insert(
                "hiddenTools".to_string(),
                json!(self.state.tools.hidden_declarations.clone()),
            );
        }
        let messages = self.agent.messages();
        let previous = get_current_system_message(&messages)
            .and_then(|m| m.get("sections").and_then(Value::as_object).cloned())
            .unwrap_or_default();
        let current = build_system_prompt_sections(options, &self.doc_paths).ok()?;
        let sections = diff_system_prompt_sections(&previous, &current)?;
        Some(json!({
            "role": "system",
            "content": "",
            "sections": sections,
            "timestamp": js_number(self.clock.date_now_ms()),
        }))
    }
}
