//! Port of `coding-agent/src/core/system-prompt.ts` (the tranche-11 slice
//! `_preparePromptAndToolLoadout` needs): `normalizeBuildSystemPromptOptions`,
//! `buildSystemPromptSections`, and `diffSystemPromptSections`.
//!
//! **Documented divergence:** the default `docs` section embeds the host's
//! readme/docs/examples paths (TS `getReadmePath()`/`getDocsPath()`/
//! `getExamplesPath()`); library code cannot read the environment, so they
//! arrive via [`DocPaths`] on the session config. Parity cases always pin a
//! `customPrompt` (the custom preamble path skips the default sections), so
//! the placeholder defaults are unobservable there.
//!
//! Options are raw `serde_json` Values (camelCase, `undefined`-omitted),
//! mirroring the TS objects extensions can mutate.

use serde_json::{json, Map, Value};

use pi_core::skills::{FileReadTool, Skill};

/// TS `SYSTEM_PROMPT_SECTION_NAME`.
fn valid_section_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && name != "preamble"
}

/// The host paths the default docs section renders (see module docs).
#[derive(Clone, Default)]
pub struct DocPaths {
    pub readme: String,
    pub docs: String,
    pub examples: String,
}

/// TS `normalizeBuildSystemPromptOptions(input)` over a raw Value — the
/// shallow normalization `_rebuildSystemPrompt` stores and handlers mutate.
pub fn normalize_build_system_prompt_options(input: &Value, cwd: &str) -> Value {
    let get = |key: &str| input.get(key).filter(|v| !v.is_null());
    let mut out = Map::new();
    if let Some(custom) = get("customPrompt") {
        out.insert("customPrompt".to_string(), custom.clone());
    }
    if let Some(forced) = get("forceSystemPrompt") {
        out.insert("forceSystemPrompt".to_string(), forced.clone());
    }
    out.insert(
        "selectedTools".to_string(),
        match get("selectedTools") {
            Some(Value::Array(tools)) => Value::Array(tools.clone()),
            _ => json!(["read", "bash", "edit", "write"]),
        },
    );
    out.insert("hiddenTools".to_string(), get("hiddenTools").cloned().unwrap_or(json!([])));
    out.insert("toolSnippets".to_string(), get("toolSnippets").cloned().unwrap_or(json!({})));
    out.insert("toolGuidelines".to_string(), get("toolGuidelines").cloned().unwrap_or(json!({})));
    out.insert("promptGuidelines".to_string(), get("promptGuidelines").cloned().unwrap_or(json!([])));
    out.insert(
        "appendSystemPrompt".to_string(),
        get("appendSystemPrompt").and_then(Value::as_str).unwrap_or("").to_string().into(),
    );
    out.insert("sections".to_string(), get("sections").cloned().unwrap_or(json!({})));
    out.insert("cwd".to_string(), json!(cwd));
    out.insert("contextFiles".to_string(), get("contextFiles").cloned().unwrap_or(json!([])));
    out.insert("skills".to_string(), get("skills").cloned().unwrap_or(json!([])));
    Value::Object(out)
}

/// TS `renderProjectContext`.
fn render_project_context(context_files: &[Value]) -> String {
    let mut parts = vec!["Project-specific instructions and guidelines:".to_string()];
    for file in context_files {
        parts.push(format!(
            "<project_instructions path=\"{}\">\n{}\n</project_instructions>",
            file.get("path").and_then(Value::as_str).unwrap_or_default(),
            file.get("content").and_then(Value::as_str).unwrap_or_default()
        ));
    }
    parts.join("\n\n")
}

/// TS `buildRules`.
fn build_rules(selected_tools: &[String], tool_guidelines: &Map<String, Value>, prompt_guidelines: &[Value]) -> String {
    let mut rules: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let add_rule = |rule: &str, rules: &mut Vec<String>, seen: &mut Vec<String>| {
        let normalized = rule.trim();
        if normalized.is_empty() || seen.iter().any(|s| s == normalized) {
            return;
        }
        seen.push(normalized.to_string());
        rules.push(normalized.to_string());
    };
    let has = |name: &str| selected_tools.iter().any(|t| t == name);
    let (has_bash, has_powershell, has_grep, has_find, has_ls) =
        (has("bash"), has("powershell"), has("grep"), has("find"), has("ls"));
    if (has_bash || has_powershell) && !has_grep && !has_find && !has_ls {
        if has_bash && has_powershell {
            add_rule("Use bash or PowerShell for file operations like listing, searching, and finding files", &mut rules, &mut seen);
        } else if has_powershell {
            add_rule("Use PowerShell for file operations like listing, searching, and finding files", &mut rules, &mut seen);
        } else {
            add_rule("Use bash for file operations like ls, rg, find", &mut rules, &mut seen);
        }
    }
    for name in selected_tools {
        if let Some(Value::Array(guidelines)) = tool_guidelines.get(name) {
            for rule in guidelines {
                if let Some(rule) = rule.as_str() {
                    add_rule(rule, &mut rules, &mut seen);
                }
            }
        }
    }
    for rule in prompt_guidelines {
        if let Some(rule) = rule.as_str() {
            add_rule(rule, &mut rules, &mut seen);
        }
    }
    add_rule("Be concise in your responses", &mut rules, &mut seen);
    add_rule("Show file paths clearly when working with files", &mut rules, &mut seen);
    rules.into_iter().map(|rule| format!("- {}", rule)).collect::<Vec<_>>().join("\n")
}

/// Map a raw skills array onto [`Skill`] (fields the prompt renders; the
/// loader-populated `sourceInfo` defaults when absent).
fn parse_skills(skills: &[Value]) -> Vec<Skill> {
    skills
        .iter()
        .map(|skill| Skill {
            name: skill.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            description: skill.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
            file_path: skill.get("filePath").and_then(Value::as_str).unwrap_or_default().to_string(),
            base_dir: skill.get("baseDir").and_then(Value::as_str).unwrap_or_default().to_string(),
            source_info: serde_json::from_value(
                skill.get("sourceInfo").cloned().unwrap_or(json!({})),
            )
            .unwrap_or_else(|_| serde_json::from_value(json!({ "path": "", "source": "" })).unwrap()),
            disable_model_invocation: skill
                .get("disableModelInvocation")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

/// TS `buildSystemPromptSections(input)` — ordered sections keyed by name;
/// `preamble` untagged, everything else `<name>`-wrapped. `Err` models the
/// invalid-section-name throw.
pub fn build_system_prompt_sections(
    input: &Value,
    paths: &DocPaths,
) -> Result<Map<String, Value>, String> {
    let get = |key: &str| input.get(key).filter(|v| !v.is_null());
    let selected_tools: Vec<String> = get("selectedTools")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let hidden_tools: Vec<String> = get("hiddenTools")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let tool_snippets = get("toolSnippets").and_then(Value::as_object).cloned().unwrap_or_default();
    let tool_guidelines = get("toolGuidelines").and_then(Value::as_object).cloned().unwrap_or_default();
    let prompt_guidelines = get("promptGuidelines").and_then(Value::as_array).cloned().unwrap_or_default();
    let custom_sections = get("sections").and_then(Value::as_object).cloned().unwrap_or_default();
    for name in custom_sections.keys() {
        if !valid_section_name(name) {
            return Err(format!("Invalid system prompt section name: {}", name));
        }
    }
    let declared_tools: Vec<&String> =
        selected_tools.iter().filter(|name| !hidden_tools.contains(name)).collect();
    let mut prompt_sections: Map<String, Value> = Map::new();
    let custom_prompt = get("customPrompt").and_then(Value::as_str);
    match custom_prompt {
        Some(custom) => {
            prompt_sections.insert("preamble".to_string(), json!(custom));
        }
        None => {
            prompt_sections.insert(
                "preamble".to_string(),
                json!("You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files."),
            );
            let visible_tools: Vec<&String> = declared_tools
                .iter()
                .copied()
                .filter(|name| tool_snippets.get(*name).and_then(Value::as_str).is_some())
                .collect();
            let tools = if !visible_tools.is_empty() {
                visible_tools
                    .iter()
                    .map(|name| {
                        format!("- {}: {}", name, tool_snippets.get(*name).and_then(Value::as_str).unwrap_or_default())
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                "(none)".to_string()
            };
            prompt_sections.insert(
                "tools".to_string(),
                json!(format!("{}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.", tools)),
            );
            let declared_names: Vec<String> =
                declared_tools.iter().map(|name| (*name).clone()).collect();
            prompt_sections.insert(
                "rules".to_string(),
                json!(build_rules(&declared_names, &tool_guidelines, &prompt_guidelines)),
            );
            prompt_sections.insert("docs".to_string(), json!(format!(
                "Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):\n- Main documentation: {}\n- Additional docs: {}\n- Examples: {} (extensions, custom tools, SDK)\n- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory\n- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md), environment variables (docs/environment-variables.md), MCP servers (docs/mcp.md), codemode scripts and non-LLM models such as classifiers and image models (docs/codemode.md)\n- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing\n- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)",
                paths.readme, paths.docs, paths.examples
            )));
        }
    }
    if let Some(append) = get("appendSystemPrompt").and_then(Value::as_str) {
        if !append.is_empty() {
            prompt_sections.insert("addendum".to_string(), json!(append));
        }
    }
    let context_files = get("contextFiles").and_then(Value::as_array).cloned().unwrap_or_default();
    if !context_files.is_empty() {
        prompt_sections.insert("project_context".to_string(), json!(render_project_context(&context_files)));
    }
    let skills = get("skills").and_then(Value::as_array).cloned().unwrap_or_default();
    let readers = ["read", "bash"];
    let skill_file_read_tool = readers
        .iter()
        .find(|tool| declared_tools.iter().any(|name| name == tool))
        .map(|tool| match *tool {
            "bash" => FileReadTool::Bash,
            _ => FileReadTool::Read,
        })
        .or_else(|| {
            if readers.iter().any(|tool| selected_tools.iter().any(|name| name == tool)) {
                Some(FileReadTool::Indirect)
            } else {
                None
            }
        });
    if let Some(tool) = skill_file_read_tool {
        if !skills.is_empty() {
            let parsed = parse_skills(&skills);
            let skills_prompt = pi_core::skills::format_skills_for_prompt(&parsed, tool).trim().to_string();
            if !skills_prompt.is_empty() {
                prompt_sections.insert("skills".to_string(), json!(skills_prompt));
            }
        }
    }
    let cwd = get("cwd").and_then(Value::as_str).unwrap_or_default().replace('\\', "/");
    prompt_sections.insert("cwd".to_string(), json!(cwd));
    for (name, content) in &custom_sections {
        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                prompt_sections.insert(name.clone(), json!(text));
            }
        }
    }
    let mut sections = Map::new();
    let preamble = prompt_sections.get("preamble").cloned().unwrap_or(Value::Null);
    sections.insert("preamble".to_string(), preamble);
    for (name, content) in prompt_sections {
        if name != "preamble" {
            // Sections carry string content; `Value`'s Display would add
            // JSON quoting (TS interpolates the raw string).
            let text = match &content {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            sections.insert(name.clone(), json!(format!("<{}>\n{}\n</{}>", name, text, name)));
        }
    }
    Ok(sections)
}

/// TS `diffSystemPromptSections(previous, current)` — a `SystemMessage.sections`
/// patch (`null` removes), or `None` when nothing changed.
pub fn diff_system_prompt_sections(
    previous: &Map<String, Value>,
    current: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut patch: Map<String, Value> = Map::new();
    for (name, text) in current {
        if previous.get(name) != Some(text) {
            patch.insert(name.clone(), text.clone());
        }
    }
    for name in previous.keys() {
        if !current.contains_key(name) {
            patch.insert(name.clone(), Value::Null);
        }
    }
    if patch.is_empty() { None } else { Some(patch) }
}
