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


use pi_core::skills::FileReadTool;

// ============================================================================
// JS value-model helpers (V8 semantics, probe-pinned — plan-doc Addendum A
// + the sp1–sp5 probe rounds)
// ============================================================================

/// JS truthiness.
pub fn js_truthy(v: &Value) -> bool {
	match v {
		Value::Null => false,
		Value::Bool(b) => *b,
		Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
		Value::String(s) => !s.is_empty(),
		Value::Array(_) => true,
		Value::Object(_) => true,
	}
}

/// JS `String(v)` (V8 template / `+` coercion).
pub fn js_to_string(v: &Value) -> String {
	match v {
		Value::Null => "null".into(),
		Value::Bool(b) => b.to_string(),
		Value::Number(n) => n.to_string(),
		Value::String(s) => s.clone(),
		Value::Array(a) => a.iter().map(js_to_string).collect::<Vec<_>>().join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// `v.x` where v must be a non-null object: null → the exact V8
/// TypeError; primitives tolerate (property read → undefined = the
/// `??`-default path). Missing key = undefined (Value::Null sentinel is
/// NOT used for undefined; callers distinguish via key presence).
fn prop_or_err(v: &Value, key: &str) -> Result<Value, String> {
	match v {
		Value::Null => Err(format!("Cannot read properties of null (reading '{}')", key)),
		Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => Ok(Value::Null),
		Value::Object(m) => Ok(m.get(key).cloned().unwrap_or(Value::Null)),
	}
}

/// `[...v]` (iterable spread): non-iterables throw V8's
/// `<expr> is not iterable` (probe-pinned: the full compiled expression
/// text for `??`-defaults, the bare variable for `...guidelines`; no
/// Symbol-iterator suffix on this node build); strings spread into
/// chars; arrays clone.
fn spread_iterable(v: &Value, expr: &str) -> Result<Vec<Value>, String> {
	match v {
		Value::Array(a) => Ok(a.clone()),
		Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
		_ => Err(format!("{} is not iterable", expr)),
	}
}

/// `{...v}` (object spread): objects clone; **strings spread to
/// char-keys** (`{..."ab"}` → `{"0":"a","1":"b"}`); **arrays spread to
/// index-keys** (`{...["a"]}` → `{"0":"a"}`); other primitives → `{}`
/// (never throws). All probe-pinned.
fn spread_object(v: &Value) -> Map<String, Value> {
	match v {
		Value::Object(m) => m.clone(),
		Value::String(str) => str
			.char_indices()
			.map(|(i, c)| (i.to_string(), Value::String(c.to_string())))
			.collect(),
		Value::Array(a) => a
			.iter()
			.enumerate()
			.map(|(i, e)| (i.to_string(), e.clone()))
			.collect(),
		_ => Map::new(),
	}
}

/// `v.replace(/x/g, y)` on a non-string → the exact V8 error
/// (`undefined`/`null` variants resolved by the caller).
fn replace_or_err(v: &Value, name: &str) -> Result<String, String> {
	match v {
		Value::String(s) => Ok(s.clone()),
		Value::Null => Err("Cannot read properties of null (reading 'replace')".to_string()),
		_ => Err(format!("{}.replace is not a function", name)),
	}
}

// ============================================================================
// normalizeBuildSystemPromptOptions — the raw (V8-observable) port
// ============================================================================

/// TS `normalizeBuildSystemPromptOptions(input)` — shallow normalization
/// with the full V8 error surface (the plan-doc Addendum A catalog; all
/// cells probe-pinned):
///
/// - `input` null/undefined → the first property read throws
///   (`Cannot read properties of null (reading 'customPrompt')`;
///   undefined → `of undefined`). Primitives (5/"s"/true/[1,2])
///   tolerate → defaults.
/// - `customPrompt` / `forceSystemPrompt` / `cwd`: **raw `input.x`
///   reads** — present (any value incl. null) → kept raw; missing
///   (undefined) → key ABSENT (jstr drops it). `n(5)` jstrs 9 keys.
/// - `selectedTools`: `[...(input.selectedTools ?? ["read", "bash",
///   "edit", "write"])]` — non-iterable non-nullish → throw
///   `(input.selectedTools ?? ["read", "bash", "edit", "write"]) is not
///   iterable`; `"ab"` → `["a","b"]`; elements kept raw.
/// - `hiddenTools` / `promptGuidelines`: `[...(input.x ?? [])]` (same
///   throw shape with their own `?? []` expression text).
/// - `toolSnippets` / `sections`: `{...(input.x ?? {})}` (primitives →
///   `{}`; **never throws**).
/// - `toolGuidelines`:
///   `fromEntries(entries(?? {}).map(([n, g]) => [n, ...g]))` —
///   primitives → `{}`; per-VALUE spread: non-iterable g (5 / null /
///   `{a:1}` / undefined) → throw `guidelines is not iterable`; `g:
///   "ab"` → `["a","b"]`; arrays clone.
/// - `appendSystemPrompt`: `input.x ?? ""` (0/false/5 kept; null → "").
/// - `contextFiles` / `skills`: `(input.x ?? []).map(x => ({...x}))` —
///   non-array non-nullish → throw `(input.x ?? []).map is not a
///   function`; elements spread (primitives → `{}`, strings →
///   char-keys).
/// - Output key order (probe-pinned): customPrompt, forceSystemPrompt,
///   selectedTools, hiddenTools, toolSnippets, toolGuidelines,
///   promptGuidelines, appendSystemPrompt, sections, cwd, contextFiles,
///   skills.
///
/// **Rust pitfall (divergence-guard):** `Value::Null.get("x")` = None ≠
/// a throw — null input is matched explicitly (missing key ≠ present
/// null: present null stays, missing = undefined).
pub fn normalize_options_raw(input: &Value) -> Result<Value, String> {
	// const customPrompt = input.customPrompt — the FIRST read: a
	// null/undefined input throws here (probe: n(null) → customPrompt).
	let _custom_prompt = prop_or_err(input, "customPrompt")?;
	let _force_system_prompt = prop_or_err(input, "forceSystemPrompt")?;

	let selected_tools = match prop_or_err(input, "selectedTools")? {
		Value::Null => vec![json!("read"), json!("bash"), json!("edit"), json!("write")],
		v => spread_iterable(&v, "(input.selectedTools ?? [\"read\", \"bash\", \"edit\", \"write\"])")?,
	};
	let hidden_tools = match prop_or_err(input, "hiddenTools")? {
		Value::Null => Vec::new(),
		v => spread_iterable(&v, "(input.hiddenTools ?? [])")?,
	};
	let tool_snippets = match prop_or_err(input, "toolSnippets")? {
		Value::Null => Map::new(),
		v => spread_object(&v),
	};
	let tool_guidelines: Map<String, Value> = {
		let source = match prop_or_err(input, "toolGuidelines")? {
			Value::Null => Map::new(),
			v => spread_object(&v),
		};
		let mut out = Map::new();
		for (name, guidelines) in source {
			out.insert(name, Value::Array(spread_iterable(&guidelines, "guidelines")?));
		}
		out
	};
	let prompt_guidelines = match prop_or_err(input, "promptGuidelines")? {
		Value::Null => Vec::new(),
		v => spread_iterable(&v, "(input.promptGuidelines ?? [])")?,
	};
	let append_system_prompt = match prop_or_err(input, "appendSystemPrompt")? {
		Value::Null => json!(""),
		v => v,
	};
	let sections = match prop_or_err(input, "sections")? {
		Value::Null => Map::new(),
		v => spread_object(&v),
	};
	let _cwd = prop_or_err(input, "cwd")?;
	let context_files: Vec<Value> = {
		let source = match prop_or_err(input, "contextFiles")? {
			Value::Null => Vec::new(),
			v => match v {
				Value::Array(a) => a.clone(),
				_ => return Err("(input.contextFiles ?? []).map is not a function".to_string()),
			},
		};
		source.iter().map(|f| Value::Object(spread_object(f))).collect()
	};
	let skills: Vec<Value> = {
		let source = match prop_or_err(input, "skills")? {
			Value::Null => Vec::new(),
			v => match v {
				Value::Array(a) => a.clone(),
				_ => return Err("(input.skills ?? []).map is not a function".to_string()),
			},
		};
		source.iter().map(|s| Value::Object(spread_object(s))).collect()
	};

	let mut out = Map::new();
	if let Value::Object(m) = input {
		for k in ["customPrompt", "forceSystemPrompt", "cwd"] {
			if let Some(v) = m.get(k) {
				out.insert(k.to_string(), v.clone());
			}
		}
	}
	out.insert("selectedTools".to_string(), Value::Array(selected_tools));
	out.insert("hiddenTools".to_string(), Value::Array(hidden_tools));
	out.insert("toolSnippets".to_string(), Value::Object(tool_snippets));
	out.insert("toolGuidelines".to_string(), Value::Object(tool_guidelines));
	out.insert("promptGuidelines".to_string(), Value::Array(prompt_guidelines));
	out.insert("appendSystemPrompt".to_string(), append_system_prompt);
	out.insert("sections".to_string(), Value::Object(sections));
	out.insert("contextFiles".to_string(), Value::Array(context_files));
	out.insert("skills".to_string(), Value::Array(skills));
	Ok(Value::Object(out))
}

// ============================================================================
// buildSystemPromptSections — the raw (V8-observable) port
// ============================================================================

/// The host paths the default docs section renders (see module docs).
#[derive(Clone, Default)]
pub struct DocPaths {
	pub readme: String,
	pub docs: String,
	pub examples: String,
}

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

/// `escapeXml(str)`: `str.replace(/&/g, "&").replace(/</g,
/// "<").replace(/>/g, ">").replace(/"/g, """).replace(/'/g,
/// "'")` — non-string / null / missing throw the exact V8 errors
/// (probe-pinned: `{name: 5}` → `str.replace is not a function`; a
/// missing `filePath` → `Cannot read properties of undefined (reading
/// 'replace')`, null → `of null`).
fn escape_xml_value(v: Option<&Value>, name: &str) -> Result<String, String> {
	match v {
		None | Some(Value::Null) => {
			// `undefined`/`null` property reads inside `.replace` calls.
			Err(match v {
				None => "Cannot read properties of undefined (reading 'replace')".to_string(),
				_ => "Cannot read properties of null (reading 'replace')".to_string(),
			})
		}
		Some(Value::String(s)) => Ok(s
			.replace('&', "&amp;")
			.replace('<', "&lt;")
			.replace('>', "&gt;")
			.replace('"', "&quot;")
			.replace('\'', "&apos;")),
		Some(_) => Err(format!("{}.replace is not a function", name)),
	}
}

/// A raw (unvalidated) skill map element — the escape throw sites live in
/// `format_skills_prompt_raw`.
struct RawSkill {
	name: Option<Value>,
	description: Option<Value>,
	file_path: Option<Value>,
	/// `!s.disableModelInvocation` (undefined/null/0 → kept).
	visible: bool,
}

fn parse_skills_raw(skills: &[Value]) -> Vec<RawSkill> {
	skills
		.iter()
		.map(|skill| RawSkill {
			name: skill.get("name").cloned(),
			description: skill.get("description").cloned(),
			file_path: skill.get("filePath").cloned(),
			visible: !skill.get("disableModelInvocation").map(js_truthy).unwrap_or(false),
		})
		.collect()
}

/// TS `formatSkillsForPrompt(skills, fileReadTool)` (verbatim text; the
/// `escapeXml` sites throw on non-string name/description/filePath).
fn format_skills_prompt_raw(
	skills: &[RawSkill],
	file_read_tool: FileReadTool,
) -> Result<String, String> {
	let visible: Vec<&RawSkill> = skills.iter().filter(|s| s.visible).collect();
	if visible.is_empty() {
		return Ok(String::new());
	}
	let mut lines: Vec<String> = vec![
		"\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
		match file_read_tool {
			FileReadTool::Read => "Use the read tool to load a skill's file when the task matches its description.",
			FileReadTool::Bash => "Use bash to load a skill's file when the task matches its description.",
			FileReadTool::Indirect => "Load a skill's file when the task matches its description.",
		}
		.to_string(),
		"When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
		String::new(),
		"<available_skills>".to_string(),
	];
	for skill in visible {
		lines.push("  <skill>".to_string());
		lines.push(format!("    <name>{}</name>", escape_xml_value(skill.name.as_ref(), "str")?));
		lines.push(format!(
			"    <description>{}</description>",
			escape_xml_value(skill.description.as_ref(), "str")?
		));
		lines.push(format!(
			"    <location>{}</location>",
			escape_xml_value(skill.file_path.as_ref(), "str")?
		));
		lines.push("  </skill>".to_string());
	}
	lines.push("</available_skills>".to_string());
	Ok(lines.join("\n"))
}

/// TS `renderProjectContext` — `({path, content})` destructured raw:
/// missing → `undefined` (template "undefined"), null → "null", 5 → "5"
/// (probe-pinned).
fn render_project_context_raw(context_files: &[Value]) -> String {
	let mut parts = vec!["Project-specific instructions and guidelines:".to_string()];
	for file in context_files {
		// `${path}` raw templates: missing → "undefined", null → "null",
		// 5 → "5" (probe sp8 A–F).
		let field = |key: &str| match file.get(key) {
			None => "undefined".to_string(),
			Some(v) => js_to_string(v),
		};
		parts.push(format!(
			"<project_instructions path=\"{}\">\n{}\n</project_instructions>",
			field("path"),
			field("content")
		));
	}
	parts.join("\n\n")
}

/// TS `buildRules` — non-string rules throw (`rule.trim is not a
/// function` / null → `'trim'`-of-null; probe-pinned). `selectedTools`
/// elements are raw (strict-eq vs "bash"/…); `toolGuidelines[name]`
/// looks up `String(name)`; `rule.trim()` results are strings.
fn build_rules_raw(
	selected_tools: &[Value],
	tool_guidelines: &Map<String, Value>,
	prompt_guidelines: &[Value],
) -> Result<String, String> {
	let mut rules: Vec<String> = Vec::new();
	let mut seen: Vec<String> = Vec::new();
	fn trim_rule(rule: &Value) -> Result<String, String> {
		match rule {
			Value::String(s) => Ok(s.trim().to_string()),
			Value::Null => Err("Cannot read properties of null (reading 'trim')".to_string()),
			_ => Err("rule.trim is not a function".to_string()),
		}
	}
	let add_rule = |rule: &Value, rules: &mut Vec<String>, seen: &mut Vec<String>| -> Result<(), String> {
		let normalized = trim_rule(rule)?;
		if normalized.is_empty() || seen.iter().any(|s| s == &normalized) {
			return Ok(());
		}
		seen.push(normalized.clone());
		rules.push(normalized);
		Ok(())
	};
	let has = |name: &str| selected_tools.iter().any(|t| t == &json!(name));
	let (has_bash, has_powershell, has_grep, has_find, has_ls) =
		(has("bash"), has("powershell"), has("grep"), has("find"), has("ls"));
	if (has_bash || has_powershell) && !has_grep && !has_find && !has_ls {
		if has_bash && has_powershell {
			add_rule(&json!("Use bash or PowerShell for file operations like listing, searching, and finding files"), &mut rules, &mut seen)?;
		} else if has_powershell {
			add_rule(&json!("Use PowerShell for file operations like listing, searching, and finding files"), &mut rules, &mut seen)?;
		} else {
			add_rule(&json!("Use bash for file operations like ls, rg, find"), &mut rules, &mut seen)?;
		}
	}
	for name in selected_tools {
		// for (const rule of toolGuidelines[name] ?? []) — property key
		// String(name), post-normalize arrays.
		let guidelines: Vec<Value> = match tool_guidelines.get(&js_to_string(name)) {
			Some(Value::Array(a)) => a.clone(),
			_ => Vec::new(),
		};
		for rule in &guidelines {
			add_rule(rule, &mut rules, &mut seen)?;
		}
	}
	for rule in prompt_guidelines {
		add_rule(rule, &mut rules, &mut seen)?;
	}
	add_rule(&json!("Be concise in your responses"), &mut rules, &mut seen)?;
	add_rule(&json!("Show file paths clearly when working with files"), &mut rules, &mut seen)?;
	Ok(rules.into_iter().map(|rule| format!("- {}", rule)).collect::<Vec<_>>().join("\n"))
}

/// TS `buildSystemPromptSections(input)` — the **raw** (V8-observable)
/// port. `input` null/undefined/primitives take the
/// `normalizeBuildSystemPromptOptions` error paths
/// (`normalize_options_raw` runs first); post-normalize raw-kept values
/// (customPrompt/forceSystemPrompt/cwd/appendSystemPrompt/sections
/// values/selectedTools elements) keep their V8 semantics:
/// - name validation first: `/^[a-z][a-z0-9_-]*$/` + `!== "preamble"`
///   → `Invalid system prompt section name: ${name}`.
/// - `customPrompt` **truthy** → `promptSections.preamble = customPrompt`
///   **raw** (non-strings survive to the map; the gsmt length-filter
///   drops them) — skipping the default preamble+tools+rules+docs
///   cluster. **Falsy (incl. null/0/"") → defaults** (probe-pinned).
/// - `visibleTools = declaredTools.filter(name => !!toolSnippets[name])`
///   — `toolSnippets[String(name)]` truthy; `- ${name}: ${snip}` raw
///   templates (probe: `{read: 5}` → `- read: 5`; `{read: []}` → kept
///   empty; 0/""/null → `(none)`).
/// - `buildRules` throws (`rule.trim`).
/// - `appendSystemPrompt` truthy (raw 5 kept) → addendum.
/// - `contextFiles.length > 0` → `renderProjectContext` (raw templates).
/// - `skillFileReadTool` = declared read/bash ?? (selected read/bash ?
///   "indirect" : undefined); `if (tool && skills.length > 0)` →
///   `formatSkillsForPrompt(skills).trim()` — escapeXml throws
///   (`str.replace` / `'replace'`-of-undefined/null);
///   `if (skillsPrompt)` non-empty.
/// - `promptSections.cwd = cwd.replace(/\\/g, "/")` — non-string cwd
///   throws (`cwd.replace is not a function` / of-null / **of-undefined
///   — missing cwd!**).
/// - custom sections **last**, `if (content)` truthy-kept **raw**
///   (overwrite built; template-coerced in the wrap) — `"preamble"`
///   rejected, invalid names throw.
/// - `sections = {preamble: raw}` + others `<name>\n${content}\n</name>`
///   (template-coerced).
pub fn build_system_prompt_sections(
	input: &Value,
	paths: &DocPaths,
) -> Result<Map<String, Value>, String> {
	let options = normalize_options_raw(input)?;

	// destructure (post-normalize: arrays / maps; raw-keeps keep V8
	// values).
	let obj_arr = |key: &str| -> Vec<Value> {
		match options.get(key) {
			Some(Value::Array(a)) => a.clone(),
			_ => Vec::new(),
		}
	};
	let obj_map = |key: &str| -> Map<String, Value> {
		match options.get(key) {
			Some(Value::Object(m)) => m.clone(),
			_ => Map::new(),
		}
	};
	let custom_prompt = options.get("customPrompt").cloned();
	let selected_tools: Vec<Value> = obj_arr("selectedTools");
	let hidden_tools: Vec<Value> = obj_arr("hiddenTools");
	let tool_snippets: Map<String, Value> = obj_map("toolSnippets");
	let tool_guidelines: Map<String, Value> = obj_map("toolGuidelines");
	let prompt_guidelines: Vec<Value> = obj_arr("promptGuidelines");
	let append_system_prompt = options.get("appendSystemPrompt").cloned();
	let custom_sections: Map<String, Value> = obj_map("sections");
	let cwd = options.get("cwd").cloned();
	let context_files: Vec<Value> = obj_arr("contextFiles");
	let skills: Vec<Value> = obj_arr("skills");

	// 1. Name validation first.
	for name in custom_sections.keys() {
		if !valid_section_name(name) {
			return Err(format!("Invalid system prompt section name: {}", name));
		}
	}

	// 2. declaredTools = selectedTools.filter(!hiddenTools.includes(name))
	//    — strict-eq on raw values.
	let declared_tools: Vec<&Value> = selected_tools
		.iter()
		.filter(|name| !hidden_tools.iter().any(|h| h == *name))
		.collect();

	let mut prompt_sections: Map<String, Value> = Map::new();
	match custom_prompt.as_ref().map(js_truthy) {
		Some(true) => {
			// raw (non-coerced) — the gsmt length-filter drops non-strings.
			prompt_sections.insert("preamble".to_string(), custom_prompt.clone().unwrap());
		}
		_ => {
			prompt_sections.insert(
				"preamble".to_string(),
				json!("You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files."),
			);
			// visibleTools = declaredTools.filter(!!toolSnippets[name])
			let visible_tools: Vec<&&Value> = declared_tools
				.iter()
				.filter(|name| match tool_snippets.get(&js_to_string(name)) {
					None => false,
					Some(v) => js_truthy(v),
				})
				.collect();
			let tools = if !visible_tools.is_empty() {
				visible_tools
					.iter()
					.map(|name| {
						format!(
							"- {}: {}",
							js_to_string(name),
							js_to_string(&tool_snippets.get(&js_to_string(name)).cloned().unwrap_or(Value::Null))
						)
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
			let declared: Vec<Value> = declared_tools.iter().map(|v| (*v).clone()).collect();
			prompt_sections.insert(
				"rules".to_string(),
				json!(build_rules_raw(&declared, &tool_guidelines, &prompt_guidelines)?),
			);
			prompt_sections.insert("docs".to_string(), json!(format!(
				"Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):\n- Main documentation: {}\n- Additional docs: {}\n- Examples: {} (extensions, custom tools, SDK)\n- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory\n- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md), environment variables (docs/environment-variables.md), MCP servers (docs/mcp.md), codemode scripts and non-LLM models such as classifiers and image models (docs/codemode.md)\n- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing\n- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)",
				paths.readme, paths.docs, paths.examples
			)));
		}
	}

	// 3. if (appendSystemPrompt) — truthy raw (5 kept).
	if append_system_prompt.as_ref().map(js_truthy).unwrap_or(false) {
		prompt_sections.insert("addendum".to_string(), append_system_prompt.clone().unwrap());
	}
	// 4. if (contextFiles.length > 0)
	if !context_files.is_empty() {
		prompt_sections.insert("project_context".to_string(), json!(render_project_context_raw(&context_files)));
	}
	// 5. skillFileReadTool (declared read/bash ?? selected read/bash ?
	//    "indirect" : undefined) — truthy string.
	let skill_file_read_tool: Option<FileReadTool> = {
		let readers = ["read", "bash"];
		// `readers.find(tool => declaredTools.includes(tool))`
		let find_declared = readers
			.iter()
			.find(|tool| declared_tools.iter().any(|name| *name == &json!(*tool)))
			.map(|tool| match *tool {
				"bash" => FileReadTool::Bash,
				_ => FileReadTool::Read,
			});
		match find_declared {
			Some(tool) => Some(tool),
			None => {
				if readers.iter().any(|tool| selected_tools.iter().any(|name| name == &json!(*tool))) {
					Some(FileReadTool::Indirect)
				} else {
					None
				}
			}
		}
	};
	if let Some(tool) = skill_file_read_tool {
		if !skills.is_empty() {
			let parsed = parse_skills_raw(&skills);
			let skills_prompt =
				format_skills_prompt_raw(&parsed, tool)?.trim().to_string();
			if js_truthy(&json!(skills_prompt)) {
				prompt_sections.insert("skills".to_string(), json!(skills_prompt));
			}
		}
	}
	// 6. promptSections.cwd = cwd.replace(/\/g, "/") — non-strings throw;
	//    MISSING cwd (undefined) → 'of undefined', present-null → 'of null'
	//    (probe-pinned: bp(n({})) / st(msgs) → 'replace' of undefined).
	match &cwd {
		None => return Err("Cannot read properties of undefined (reading 'replace')".to_string()),
		Some(v) => prompt_sections.insert(
			"cwd".to_string(),
			json!(replace_or_err(v, "cwd")?.replace('\\', "/")),
		),
	};
	// 7. custom sections last — truthy-kept raw (overwrite built).
	for (name, content) in &custom_sections {
		if js_truthy(content) {
			prompt_sections.insert(name.clone(), content.clone());
		}
	}

	// 8. sections = {preamble: raw} + others templated.
	let mut sections: Map<String, Value> = Map::new();
	sections.insert("preamble".to_string(), prompt_sections.get("preamble").cloned().unwrap_or(Value::Null));
	for (name, content) in prompt_sections {
		if name != "preamble" {
			sections.insert(name.clone(), json!(format!("<{}>\n{}\n</{}>", name, js_to_string(&content), name)));
		}
	}
	Ok(sections)
}

// ============================================================================
// buildSystemPromptState / buildSystemPrompt / getSystemMessageText /
// contentText (utils/text.js) — the raw ports
// ============================================================================

/// TS `contentText(content, separator = "\n")` (pi-ai utils/text.js) —
/// **the raw port**: string → itself; array →
/// `.filter(b => b.type === "text").map(b => b.text).join(sep)` (the
/// `b.type` read on null throws `'type'`-of-null; non-objects drop;
/// `b.text` missing = undefined → join-coerced `""`); null → `null.filter`
/// throw; other primitives → `content.filter is not a function`.
pub fn content_text_raw(content: &Value, separator: &str) -> Result<String, String> {
	match content {
		Value::String(s) => Ok(s.clone()),
		Value::Array(blocks) => {
			let mut parts: Vec<String> = Vec::new();
			for b in blocks {
				// b.type === "text" — null throws, non-objects drop.
				let ty = prop_or_err(b, "type")?;
				if ty == json!("text") {
					// b.text — join-coerced (null/undefined → "").
					parts.push(match b.get("text") {
						Some(Value::String(s)) => s.clone(),
						Some(v) if v.is_null() => String::new(),
						Some(v) => js_to_string(v),
						None => String::new(),
					});
				}
			}
			Ok(parts.join(separator))
		}
		Value::Null => Err("Cannot read properties of null (reading 'filter')".to_string()),
		_ => Err("content.filter is not a function".to_string()),
	}
}

/// TS `getSystemMessageText(message)` —
/// `[contentText(message.content)] + Object.values(message.sections ??
/// {}).filter(v => v !== null) → filter(p => p.length > 0) → "\n\n".join`.
/// - `message.content` missing → `undefined.filter` throw (but the
///   forced-check in `buildSystemPromptState` throws first).
/// - non-null section values push **raw**; `p.length > 0` drops
///   non-strings (raw `preamble`) and empty strings.
/// - forced `content: ["x"]` → ct drops non-text-blocks → "" (probe).
pub fn get_system_message_text_raw(message: &Value) -> Result<String, String> {
	let mut parts: Vec<Value> = vec![json!(content_text_raw(
		&message.get("content").cloned().unwrap_or(Value::Null),
		"\n"
	)?)];
	if let Some(Value::Object(sections)) = message.get("sections") {
		// `sections ?? {}` — null (missing) → no parts; null VALUES are
		// filtered before the push.
		for value in sections.values() {
			if !value.is_null() {
				parts.push(value.clone());
			}
		}
	}
	// filter(p => p.length > 0) — strings: len; others: dropped.
	let joined: Vec<String> = parts
		.iter()
		.filter(|p| p.as_str().map(|s| !s.is_empty()).unwrap_or(false))
		.map(|p| p.as_str().unwrap().to_string())
		.collect();
	Ok(joined.join("\n\n"))
}

/// TS `buildSystemPromptState(input)` —
/// `input.forceSystemPrompt !== undefined` (KEY-PRESENT — null/0/false
/// all force!) → `{content: fsp}` (sections absent); else
/// `{content: "", sections: buildSystemPromptSections(input)}`.
/// `input` null → `null.forceSystemPrompt` throw (probe-pinned).
pub fn build_system_prompt_state(input: &Value, paths: &DocPaths) -> Result<(Value, Option<Map<String, Value>>), String> {
	let _fsp = prop_or_err(input, "forceSystemPrompt")?;
	if let Value::Object(m) = input {
		if m.contains_key("forceSystemPrompt") {
			return Ok((m.get("forceSystemPrompt").cloned().unwrap(), None));
		}
	}
	Ok((json!(""), Some(build_system_prompt_sections(input, paths)?)))
}

/// TS `buildSystemPrompt(input)` =
/// `getSystemMessageText({role: "system", ...buildSystemPromptState(input), timestamp: 0})`
/// — forced fsp: null → `'filter'`-of-null, 0/5 → `content.filter`, `["x"]`
/// → "" (probe-pinned); unforced → the sections render (all throws of
/// `buildSystemPromptSections` + the gsmt length-filter).
pub fn build_system_prompt(input: &Value, paths: &DocPaths) -> Result<String, String> {
	let (content, sections) = build_system_prompt_state(input, paths)?;
	let mut message = Map::new();
	message.insert("role".to_string(), json!("system"));
	message.insert("content".to_string(), content);
	if let Some(sections) = sections {
		message.insert("sections".to_string(), Value::Object(sections));
	}
	message.insert("timestamp".to_string(), json!(0));
	get_system_message_text_raw(&Value::Object(message))
}

/// TS `diffSystemPromptSections(previous, current)` — a
/// `SystemMessage.sections` patch (`null` removes), or `None` when
/// nothing changed.
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

// ============================================================================
// Probe-parity tests — every cell asserted against the compiled dist
// (node v24; the sp1–sp7 probe rounds; baselines embedded verbatim).
// ============================================================================

#[cfg(test)]
mod raw_probes {
	use super::*;

	const README: &str = "/home/shinobi/AI/pi-repo/packages/coding-agent/README.md";
	const DOCS: &str = "/home/shinobi/AI/pi-repo/packages/coding-agent/docs";
	const EXAMPLES: &str = "/home/shinobi/AI/pi-repo/packages/coding-agent/examples";

	fn paths() -> DocPaths {
		DocPaths { readme: README.to_string(), docs: DOCS.to_string(), examples: EXAMPLES.to_string() }
	}

	fn n(v: Value) -> Value {
		normalize_options_raw(&v).expect("normalize ok")
	}

	fn bp(v: Value) -> Result<String, String> {
		build_system_prompt(&v, &paths())
	}

	fn bp_n(v: Value) -> Result<String, String> {
		bp(n(v))
	}

	fn err_of(r: Result<String, String>) -> String {
		match r {
			Err(e) => e,
			Ok(v) => panic!("expected Err, got Ok({:?})", v),
		}
	}

	// ---- normalizeBuildSystemPromptOptions (sp1/sp2) ------------------

	#[test]
	fn normalize_null_throws() {
		assert_eq!(
			normalize_options_raw(&Value::Null).unwrap_err(),
			"Cannot read properties of null (reading 'customPrompt')"
		);
	}

	#[test]
	fn normalize_undefined_throws() {
		assert_eq!(
			normalize_options_raw(&json!({ "customPrompt": 5 })).map(|_| ()).is_ok(),
			true
		);
		// undefined input = JSON absent = Value::Null sentinel? No — the
		// harness passes Value::Null for an explicit null input only;
		// undefined = absent key. Both null input and undefined input
		// throw (probe: n(undefined) → of undefined) — modeled by the
		// runner's JSVal; raw null is the only reachable one.
	}

	#[test]
	fn normalize_primitives_tolerate_to_defaults() {
		let d = |v: Value| {
			let o = n(v);
			assert_eq!(o.get("selectedTools").unwrap(), &json!(["read", "bash", "edit", "write"]));
			assert_eq!(o.get("hiddenTools").unwrap(), &json!([]));
			assert_eq!(o.get("toolSnippets").unwrap(), &json!({}));
			assert_eq!(o.get("toolGuidelines").unwrap(), &json!({}));
			assert_eq!(o.get("promptGuidelines").unwrap(), &json!([]));
			assert_eq!(o.get("appendSystemPrompt").unwrap(), &json!(""));
			assert_eq!(o.get("sections").unwrap(), &json!({}));
			assert_eq!(o.get("contextFiles").unwrap(), &json!([]));
			assert_eq!(o.get("skills").unwrap(), &json!([]));
			assert!(o.get("cwd").is_none());
		};
		d(json!(5));
		d(json!(0));
		d(json!("s"));
		d(json!(true));
		d(json!([1, 2]));
	}

	#[test]
	fn normalize_raw_keeps() {
		// customPrompt / forceSystemPrompt / cwd = raw input.x.
		assert_eq!(n(json!({"cwd": 5})).get("cwd"), Some(&json!(5)));
		assert_eq!(n(json!({"cwd": 0})).get("cwd"), Some(&json!(0)));
		assert_eq!(n(json!({"cwd": null})).get("cwd"), Some(&json!(null)));
		assert_eq!(n(json!({"cwd": ""})).get("cwd"), Some(&json!("")));
		assert_eq!(n(json!({"cwd": "x"})).get("cwd"), Some(&json!("x")));
		assert_eq!(n(json!({"customPrompt": 0})).get("customPrompt"), Some(&json!(0)));
		assert_eq!(n(json!({"customPrompt": null})).get("customPrompt"), Some(&json!(null)));
		assert_eq!(n(json!({"forceSystemPrompt": null})).get("forceSystemPrompt"), Some(&json!(null)));
		assert_eq!(n(json!({"forceSystemPrompt": 0})).get("forceSystemPrompt"), Some(&json!(0)));
		// present keys (undefined = absent): n({customPrompt:'a',
		// forceSystemPrompt:false}) keeps 12 keys (probe).
		let keys: Vec<String> = n(json!({"customPrompt": "a", "forceSystemPrompt": false}))
			.as_object().unwrap().keys().cloned().collect();
		// TS Object.keys sees 12 (undefined-valued cwd present); jstr drops
		// undefined keys → 11 (the observable model).
		assert_eq!(keys.len(), 11);
		assert_eq!(keys[0], "customPrompt");
		assert_eq!(keys[1], "forceSystemPrompt");
	}

	#[test]
	fn normalize_append_system_prompt() {
		assert_eq!(n(json!({"appendSystemPrompt": 0})).get("appendSystemPrompt"), Some(&json!(0)));
		assert_eq!(n(json!({"appendSystemPrompt": false})).get("appendSystemPrompt"), Some(&json!(false)));
		assert_eq!(n(json!({"appendSystemPrompt": null})).get("appendSystemPrompt"), Some(&json!("")));
		assert_eq!(n(json!({"appendSystemPrompt": 5})).get("appendSystemPrompt"), Some(&json!(5)));
		assert_eq!(n(json!({"appendSystemPrompt": "x"})).get("appendSystemPrompt"), Some(&json!("x")));
	}

	#[test]
	fn normalize_iterable_spreads() {
		assert_eq!(n(json!({"selectedTools": "ab"})).get("selectedTools"), Some(&json!(["a", "b"])));
		assert_eq!(n(json!({"selectedTools": null})).get("selectedTools"), Some(&json!(["read", "bash", "edit", "write"])));
		assert_eq!(n(json!({"selectedTools": [1, 2]})).get("selectedTools"), Some(&json!([1, 2])));
		assert_eq!(n(json!({"hiddenTools": "ab"})).get("hiddenTools"), Some(&json!(["a", "b"])));
		assert_eq!(n(json!({"hiddenTools": null})).get("hiddenTools"), Some(&json!([])));
		assert_eq!(n(json!({"promptGuidelines": "ab"})).get("promptGuidelines"), Some(&json!(["a", "b"])));
		assert_eq!(n(json!({"promptGuidelines": null})).get("promptGuidelines"), Some(&json!([])));
		// non-iterable throws (full compiled expression text).
		assert_eq!(
			normalize_options_raw(&json!({"selectedTools": 5})).unwrap_err(),
			"(input.selectedTools ?? [\"read\", \"bash\", \"edit\", \"write\"]) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"selectedTools": {}})).unwrap_err(),
			"(input.selectedTools ?? [\"read\", \"bash\", \"edit\", \"write\"]) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"selectedTools": true})).unwrap_err(),
			"(input.selectedTools ?? [\"read\", \"bash\", \"edit\", \"write\"]) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"hiddenTools": 5})).unwrap_err(),
			"(input.hiddenTools ?? []) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"promptGuidelines": 5})).unwrap_err(),
			"(input.promptGuidelines ?? []) is not iterable"
		);
		assert_eq!(
			normalize_options_raw(&json!({"promptGuidelines": {}})).unwrap_err(),
			"(input.promptGuidelines ?? []) is not iterable"
		);
	}

	#[test]
	fn normalize_tool_guidelines() {
		assert_eq!(n(json!({"toolGuidelines": 5})).get("toolGuidelines"), Some(&json!({})));
		assert_eq!(n(json!({"toolGuidelines": null})).get("toolGuidelines"), Some(&json!({})));
		assert_eq!(n(json!({"toolGuidelines": {}})).get("toolGuidelines"), Some(&json!({})));
		assert_eq!(
			n(json!({"toolGuidelines": {"x": "ab"}})).get("toolGuidelines"),
			Some(&json!({"x": ["a", "b"]}))
		);
		assert_eq!(n(json!({"toolGuidelines": {"x": [1]}})).get("toolGuidelines"), Some(&json!({"x": [1]})));
		for bad in [json!(5), json!(null), json!({"a": 1})] {
			assert_eq!(
				normalize_options_raw(&json!({"toolGuidelines": {"x": bad}})).unwrap_err(),
				"guidelines is not iterable"
			);
		}
	}

	#[test]
	fn normalize_object_spreads_never_throw() {
		assert_eq!(n(json!({"toolSnippets": 5})).get("toolSnippets"), Some(&json!({})));
		assert_eq!(n(json!({"toolSnippets": null})).get("toolSnippets"), Some(&json!({})));
		assert_eq!(n(json!({"toolSnippets": "ab"})).get("toolSnippets"), Some(&json!({"0": "a", "1": "b"})));
		assert_eq!(n(json!({"sections": 5})).get("sections"), Some(&json!({})));
		assert_eq!(n(json!({"sections": null})).get("sections"), Some(&json!({})));
		assert_eq!(n(json!({"sections": {"a": 5}})).get("sections"), Some(&json!({"a": 5})));
		assert_eq!(n(json!({"sections": ["a"]})).get("sections"), Some(&json!({"0": "a"})));
	}

	#[test]
	fn normalize_map_spreads() {
		assert_eq!(
			normalize_options_raw(&json!({"contextFiles": 5})).unwrap_err(),
			"(input.contextFiles ?? []).map is not a function"
		);
		assert_eq!(
			normalize_options_raw(&json!({"skills": "ab"})).unwrap_err(),
			"(input.skills ?? []).map is not a function"
		);
		assert_eq!(n(json!({"contextFiles": null})).get("contextFiles"), Some(&json!([])));
		assert_eq!(n(json!({"skills": null})).get("skills"), Some(&json!([])));
		assert_eq!(
			n(json!({"contextFiles": [5, null, "ab", []]})).get("contextFiles"),
			Some(&json!([{}, {}, {"0": "a", "1": "b"}, {}]))
		);
		assert_eq!(n(json!({"skills": [5, null]})).get("skills"), Some(&json!([{}, {}])));
		assert_eq!(n(json!({"skills": [[1]]})).get("skills"), Some(&json!([{"0": 1}])));
		assert_eq!(n(json!({"skills": [{"name": 5}]})).get("skills"), Some(&json!([{"name": 5}])));
	}

	// ---- buildSystemPromptSections / bp (sp4/sp5/sp7) ------------------

	#[test]
	fn bp_baselines_exact() {
		let b1 = include_str!("raw_fixtures/sp_base1.txt");
		assert_eq!(bp_n(json!({"cwd": "/p"})).unwrap(), b1);
		let b2 = include_str!("raw_fixtures/sp_base2.txt");
		assert_eq!(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": "R"}})).unwrap(), b2);
		let b3 = include_str!("raw_fixtures/sp_base3.txt");
		assert_eq!(bp_n(json!({"cwd": "/p", "selectedTools": ["read"]})).unwrap(), b3);
	}

	#[test]
	fn bp_custom_prompt_and_forced() {
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP"})).unwrap(), "CP\n\n<cwd>\n/p\n</cwd>");
		assert_eq!(bp_n(json!({"cwd": "/p", "forceSystemPrompt": "F"})).unwrap(), "F");
		// fsp null → contentText(null) → 'filter' of null.
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "forceSystemPrompt": null}))),
			"Cannot read properties of null (reading 'filter')"
		);
		// fsp 0 → contentText(0) → content.filter.
		assert_eq!(err_of(bp_n(json!({"cwd": "/p", "forceSystemPrompt": 0}))), "content.filter is not a function");
		// fsp ["x"] → ct drops non-text-blocks → "".
		assert_eq!(bp_n(json!({"cwd": "/p", "forceSystemPrompt": ["x"]})).unwrap(), "");
		// cp falsy (0 / null) → default sections.
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": 0})).unwrap(), include_str!("raw_fixtures/sp_base1.txt"));
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": null})).unwrap(), include_str!("raw_fixtures/sp_base1.txt"));
	}

	#[test]
	fn bp_cwd_throws() {
		assert_eq!(err_of(bp_n(json!({"cwd": 5}))), "cwd.replace is not a function");
		assert_eq!(
			err_of(bp_n(json!({"cwd": null}))),
			"Cannot read properties of null (reading 'replace')"
		);
		// missing cwd → undefined → 'replace' of undefined.
		assert_eq!(
			err_of(bp_n(json!({}))),
			"Cannot read properties of undefined (reading 'replace')"
		);
		// backslash replaced.
		let r = bp_n(json!({"cwd": "a\\b"})).unwrap();
		assert_eq!(&r[r.len()-14..], "wd>\na/b\n</cwd>");
	}

	#[test]
	fn bp_tool_snippets_truthy_raw() {
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": 5}})).unwrap();
		assert!(r.contains("- read: 5\n"), "ts read:5 - backtick - read: 5");
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": []}})).unwrap();
		assert!(r.contains("- read: \n"), "ts read:[] - empty-array truthy");
		for falsy in [json!(0), json!(""), json!(null)] {
			let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolSnippets": {"read": falsy}})).unwrap();
			assert!(r.contains("(none)"), "ts falsy {:?} - (none)", falsy);
		}
		// non-declared snippet not listed.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read", "x"], "toolSnippets": {"read": "R", "x": "X", "y": "Y"}})).unwrap();
		let tools = extract(&r, "tools");
		assert_eq!(tools, "\n- read: R\n- x: X\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n");
	}

	#[test]
	fn bp_rules_throws_and_dedup() {
		assert_eq!(err_of(bp_n(json!({"cwd": "/p", "toolGuidelines": {"read": [5]}}))), "rule.trim is not a function");
		assert_eq!(err_of(bp_n(json!({"cwd": "/p", "promptGuidelines": [5]}))), "rule.trim is not a function");
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "toolGuidelines": {"read": ["R1", "R2"]}, "promptGuidelines": ["G", "G"]})).unwrap();
		let rules = extract(&r, "rules");
		assert_eq!(rules, "\n- R1\n- R2\n- G\n- Be concise in your responses\n- Show file paths clearly when working with files\n");
		// hidden tools: bash hidden → no bash rule, snippet gone.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read", "bash"], "hiddenTools": ["bash"]})).unwrap();
		assert!(r.contains("(none)"));
	}

	#[test]
	fn bp_append_and_custom_sections() {
		// appendSystemPrompt: 5 truthy → template-coerced "5"; 0 → skipped.
		let r = bp_n(json!({"cwd": "/p", "appendSystemPrompt": 5})).unwrap();
		assert!(r.contains("<addendum>\n5\n</addendum>"));
		let r = bp_n(json!({"cwd": "/p", "appendSystemPrompt": 0})).unwrap();
		assert!(!r.contains("addendum"));
		// custom sections: truthy-kept raw (5 → "5"), falsy skipped,
		// null-0-''-dropped; custom runs last + overwrites built.
		let r = bp_n(json!({"cwd": "/p", "sections": {"a": 5, "b": "S", "c": null, "d": 0}})).unwrap();
		assert!(r.contains("<a>\n5\n</a>"));
		assert!(r.contains("<b>\nS\n</b>"));
		assert!(!r.contains("<c>"));
		assert!(!r.contains("<d>"));
		let r = bp_n(json!({"cwd": "/p", "sections": {"rules": "XX"}})).unwrap();
		let rules = extract(&r, "rules");
		assert_eq!(rules, "\nXX\n");
		assert_eq!(r.matches("<rules>").count(), 1);
		// cp-gate: customPrompt truthy → only the default cluster is
		// skipped (addendum/project_context/skills/cwd/custom run).
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "sections": {"rules": "XX"}})).unwrap(), "CP\n\n<cwd>\n/p\n</cwd>\n\n<rules>\nXX\n</rules>");
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "appendSystemPrompt": "A"})).unwrap(), "CP\n\n<addendum>\nA\n</addendum>\n\n<cwd>\n/p\n</cwd>");
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "contextFiles": [{"path": 1, "content": 2}]})).unwrap(), "CP\n\n<project_context>\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"1\">\n2\n</project_instructions>\n</project_context>\n\n<cwd>\n/p\n</cwd>");
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "sections": {"z": "Z"}})).unwrap(), "CP\n\n<cwd>\n/p\n</cwd>\n\n<z>\nZ\n</z>");
	}

	#[test]
	fn bp_skills() {
		// cp + full skill: the 506-char baseline (embedded verbatim).
		let b7 = bp_n(json!({"cwd": "/p", "customPrompt": "CP", "skills": [{"name": "S", "description": "D", "filePath": "F"}]})).unwrap();
		assert_eq!(b7.len(), 506);
		assert_eq!(b7, "CP\n\n<skills>\nThe following skills provide specialized instructions for specific tasks.\nUse the read tool to load a skill's file when the task matches its description.\nWhen a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\n<available_skills>\n  <skill>\n    <name>S</name>\n    <description>D</description>\n    <location>F</location>\n  </skill>\n</available_skills>\n</skills>\n\n<cwd>\n/p\n</cwd>");
		// missing filePath → 'replace' of undefined.
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": "S", "description": "D"}]}))),
			"Cannot read properties of undefined (reading 'replace')"
		);
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{}]}))),
			"Cannot read properties of undefined (reading 'replace')"
		);
		// non-string name/description → str.replace.
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": 5, "description": "D", "filePath": "F"}]}))),
			"str.replace is not a function"
		);
		assert_eq!(
			err_of(bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": "S", "description": 5, "filePath": "F"}]}))),
			"str.replace is not a function"
		);
		// dmi → no section (equal to the no-skills baseline).
		let dmi = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": [{"name": "S", "description": "D", "filePath": "F", "disableModelInvocation": true}]})).unwrap();
		assert_eq!(dmi, include_str!("raw_fixtures/sp_base3.txt"));
		// empty skills + no reader → no section.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["read"], "skills": []})).unwrap();
		// NB: the docs section contains the word "skills" — assert the tag.
		assert!(!r.contains("<skills>"));
		// NO selectedTools → the default [read, bash, ...] → the reader is
		// found (Read) → the section renders.
		let r = bp_n(json!({"cwd": "/p", "skills": [{"name": "S", "description": "D", "filePath": "F"}]})).unwrap();
		assert!(r.contains("<skills>"));
		// skills with NO reader (selectedTools: ["x"]) → no section.
		let r = bp_n(json!({"cwd": "/p", "selectedTools": ["x"], "skills": [{"name": "S", "description": "D", "filePath": "F"}]})).unwrap();
		assert!(!r.contains("<skills>"));
	}

	#[test]
	fn bp_section_name_validation() {
		for name in ["A", "0", "preamble", "Bad-Name"] {
			let mut sections = serde_json::Map::new();
			sections.insert(name.to_string(), json!("x"));
			assert_eq!(
				err_of(bp(json!({"cwd": "/p", "sections": Value::Object(sections)}))),
				format!("Invalid system prompt section name: {}", name)
			);
		}
		// valid names.
		let mut sections = serde_json::Map::new();
		sections.insert("a-b_9".to_string(), json!("x"));
		assert!(bp(json!({"cwd": "/p", "sections": Value::Object(sections)})).is_ok());
	}

	#[test]
	fn bp_project_context_raw_templates() {
		// probe sp8 (A–F): raw templates — missing → "undefined", null →
		// "null", 5 → "5", 0 → "0" (undefined ≠ null!).
		let pc = |v: Value| -> String {
			let r = bp_n(json!({"cwd": "/p", "customPrompt": "CP", "contextFiles": v})).unwrap();
			let i = r.find("<project_context>").expect("pc open") + 17;
			let j = r[i..].find("</project_context>").expect("pc close") + i;
			r[i..j].to_string()
		};
		let head = "\nProject-specific instructions and guidelines:\n\n";
		assert_eq!(pc(json!([{"a": 1}])), format!("{}<project_instructions path=\"undefined\">\nundefined\n</project_instructions>\n", head));
		assert_eq!(pc(json!([5])), format!("{}<project_instructions path=\"undefined\">\nundefined\n</project_instructions>\n", head));
		assert_eq!(pc(json!([{"path": 5, "content": null}])), format!("{}<project_instructions path=\"5\">\nnull\n</project_instructions>\n", head));
		assert_eq!(pc(json!([{"path": null}])), format!("{}<project_instructions path=\"null\">\nundefined\n</project_instructions>\n", head));
		assert_eq!(pc(json!([{"path": null, "content": 0}])), format!("{}<project_instructions path=\"null\">\n0\n</project_instructions>\n", head));
		assert_eq!(pc(json!([{"content": "c"}])), format!("{}<project_instructions path=\"undefined\">\nc\n</project_instructions>\n", head));
		assert_eq!(pc(json!([{"path": 5, "content": 6}])), format!("{}<project_instructions path=\"5\">\n6\n</project_instructions>\n", head));
		// B6 verbatim baseline.
		assert_eq!(bp_n(json!({"cwd": "/p", "customPrompt": "CP", "contextFiles": [{"path": 1, "content": 2}]})).unwrap(), "CP\n\n<project_context>\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"1\">\n2\n</project_instructions>\n</project_context>\n\n<cwd>\n/p\n</cwd>");
	}

	// ---- buildSystemPromptState / getSystemMessageText (sp4) ----------

	#[test]
	fn state_forced_and_default() {
		let (content, sections) = build_system_prompt_state(&n(json!({"cwd": "/p", "forceSystemPrompt": "F"})), &paths()).unwrap();
		assert_eq!(content, json!("F"));
		assert!(sections.is_none());
		let (content, sections) = build_system_prompt_state(&n(json!({"cwd": "/p"})), &paths()).unwrap();
		assert_eq!(content, json!(""));
		let sections = sections.unwrap();
		let keys: Vec<&String> = sections.keys().collect();
		assert_eq!(keys, ["preamble", "tools", "rules", "docs", "cwd"]);
		// forced falsy values still force (contains_key).
		for f in [json!(null), json!(0), json!(false), json!("")] {
			let (content, sections) =
				build_system_prompt_state(&n(json!({"cwd": "/p", "forceSystemPrompt": f})), &paths()).unwrap();
			assert_eq!(content, f);
			assert!(sections.is_none(), "forced {:?} → no sections", f);
		}
	}

	#[test]
	fn state_null_throws() {
		assert_eq!(
			build_system_prompt_state(&Value::Null, &paths()).unwrap_err(),
			"Cannot read properties of null (reading 'forceSystemPrompt')"
		);
	}

	#[test]
	fn gsmt_content_text() {
		// ct(null) → 'filter' of null; ct(0) → content.filter.
		assert_eq!(content_text_raw(&Value::Null, "\n").unwrap_err(), "Cannot read properties of null (reading 'filter')");
		assert_eq!(content_text_raw(&json!(0), "\n").unwrap_err(), "content.filter is not a function");
		// ct(["a", 5]) → non-text-blocks dropped → "".
		assert_eq!(content_text_raw(&json!(["a", 5]), "\n").unwrap(), "");
		// ct([{type:"text",text:"a"},{type:"x"}]) → "a".
		assert_eq!(content_text_raw(&json!([{"type": "text", "text": "a"}, {"type": "x"}]), "\n").unwrap(), "a");
		// ct([null]) → 'type' of null.
		assert_eq!(content_text_raw(&json!([null]), "\n").unwrap_err(), "Cannot read properties of null (reading 'type')");
		// gsmt: sections ?? {} (null/missing → no parts); null section
		// values dropped; empty parts dropped.
		assert_eq!(get_system_message_text_raw(&json!({"content": "", "sections": {"a": null, "b": "x"}})).unwrap(), "x");
		assert_eq!(get_system_message_text_raw(&json!({"content": "c"})).unwrap(), "c");
	}

	// ---- helpers -------------------------------------------------------

	// the TS regex capture: between <name> and </name> (INCLUDING the
	// template's leading/trailing \n's).
	fn extract(r: &str, section: &str) -> String {
		let open = format!("<{}>", section);
		let close = format!("</{}>", section);
		let i = r.find(&open).expect("section open") + open.len();
		let j = r[i..].find(&close).expect("section close") + i;
		r[i..j].to_string()
	}
}
