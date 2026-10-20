//! Port of `packages/coding-agent/src/core/slash-commands.ts`.
//!
//! `SourceInfo` (type-only import from `./source-info.ts` in the TS file) is
//! defined here as the struct subset `SlashCommandInfo` needs; a dedicated
//! source-info module can absorb it later.
//!
//! `APP_NAME` (from `../config.ts`, `pkg.piConfig?.name || "pi"`) is injected
//! as a parameter to `builtin_slash_commands` — this crate never reads
//! package.json or the environment.

use serde::{Deserialize, Serialize};

/// Port of `SourceScope` from `src/core/source-info.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
	User,
	Project,
	Temporary,
}

/// Port of `SourceOrigin` from `src/core/source-info.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceOrigin {
	Package,
	#[serde(rename = "top-level")]
	TopLevel,
}

/// Port of `SourceInfo` from `src/core/source-info.ts`.
/// `baseDir` is optional: `undefined` is omitted when serializing, never `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
	pub path: String,
	pub source: String,
	pub scope: SourceScope,
	pub origin: SourceOrigin,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub base_dir: Option<String>,
}

/// Port of `SlashCommandSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlashCommandSource {
	Extension,
	Prompt,
	Skill,
}

/// Port of `SlashCommandInfo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommandInfo {
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	pub source: SlashCommandSource,
	pub source_info: SourceInfo,
}

/// Port of `BuiltinSlashCommand`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinSlashCommand {
	pub name: String,
	pub description: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub argument_hint: Option<String>,
}

/// Port of `BUILTIN_SLASH_COMMANDS` (order preserved). The `quit` description
/// embeds `APP_NAME` (`Quit ${APP_NAME}`); pass the app name explicitly
/// (the shipped package uses `"pi"`).
pub fn builtin_slash_commands(app_name: &str) -> Vec<BuiltinSlashCommand> {
	fn cmd(name: &str, description: &str) -> BuiltinSlashCommand {
		BuiltinSlashCommand { name: name.to_string(), description: description.to_string(), argument_hint: None }
	}
	fn cmd_with_hint(name: &str, description: &str, hint: &str) -> BuiltinSlashCommand {
		BuiltinSlashCommand {
			name: name.to_string(),
			description: description.to_string(),
			argument_hint: Some(hint.to_string()),
		}
	}

	vec![
		cmd("settings", "Open settings menu"),
		cmd_with_hint("model", "Select model (opens selector UI)", "<provider/model>"),
		cmd("tree", "Navigate session tree (switch branches)"),
		cmd_with_hint("thinking", "Set thinking level", "<level>"),
		cmd("scoped-models", "Enable/disable models for Ctrl+P cycling"),
		cmd("export", "Export session (HTML default, or specify path: .html/.jsonl)"),
		cmd("import", "Import and resume a session from a JSONL file"),
		cmd("share", "Share session via a Radius artifact or a GitHub gist"),
		cmd_with_hint("bug", "Report a bug to the Pi developers", "<description>"),
		cmd("copy", "Copy last agent message to clipboard"),
		cmd("name", "Set session display name"),
		cmd("session", "Show session info and stats"),
		cmd("changelog", "Show changelog entries"),
		cmd("hotkeys", "Show all keyboard shortcuts"),
		cmd("fork", "Create a new fork from a previous user message"),
		cmd("clone", "Duplicate the current session at the current position"),
		cmd("trust", "Save project trust decision for future sessions"),
		cmd_with_hint("login", "Configure provider authentication", "<provider>"),
		cmd("logout", "Remove provider authentication"),
		cmd("new", "Start a new session"),
		cmd("compact", "Manually compact the session context"),
		cmd("resume", "Resume a different session"),
		cmd("reload", "Reload keybindings, extensions, skills, prompts, themes, and context files"),
		cmd("quit", &format!("Quit {app_name}")),
	]
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn builtin_command_count_and_order() {
		let commands = builtin_slash_commands("pi");
		assert_eq!(commands.len(), 24);
		let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
		assert_eq!(
			names,
			vec![
				"settings", "model", "tree", "thinking", "scoped-models", "export", "import",
				"share", "bug", "copy", "name", "session", "changelog", "hotkeys", "fork",
				"clone", "trust", "login", "logout", "new", "compact", "resume", "reload", "quit",
			]
		);
	}

	#[test]
	fn quit_embeds_app_name() {
		assert_eq!(builtin_slash_commands("pi").last().unwrap().description, "Quit pi");
		assert_eq!(builtin_slash_commands("tau").last().unwrap().description, "Quit tau");
	}

	#[test]
	fn argument_hints_match_ts() {
		let commands = builtin_slash_commands("pi");
		let with_hints: Vec<(&str, &str)> = commands
			.iter()
			.filter_map(|c| c.argument_hint.as_deref().map(|h| (c.name.as_str(), h)))
			.collect();
		assert_eq!(
			with_hints,
			vec![
				("model", "<provider/model>"),
				("thinking", "<level>"),
				("bug", "<description>"),
				("login", "<provider>"),
			]
		);
	}

	#[test]
	fn spot_check_descriptions() {
		let commands = builtin_slash_commands("pi");
		let find = |name: &str| commands.iter().find(|c| c.name == name).unwrap();
		assert_eq!(find("settings").description, "Open settings menu");
		assert_eq!(
			find("reload").description,
			"Reload keybindings, extensions, skills, prompts, themes, and context files"
		);
		assert_eq!(find("share").description, "Share session via a Radius artifact or a GitHub gist");
	}

	#[test]
	fn slash_command_info_serialization_omits_undefined() {
		let info = SlashCommandInfo {
			name: "review".to_string(),
			description: None,
			source: SlashCommandSource::Skill,
			source_info: SourceInfo {
				path: "/skills/review/skill.json".to_string(),
				source: "skills".to_string(),
				scope: SourceScope::Project,
				origin: SourceOrigin::TopLevel,
				base_dir: None,
			},
		};
		let json = serde_json::to_value(&info).unwrap();
		assert_eq!(
			json,
			serde_json::json!({
				"name": "review",
				"source": "skill",
				"sourceInfo": {
					"path": "/skills/review/skill.json",
					"source": "skills",
					"scope": "project",
					"origin": "top-level"
				}
			})
		);
		// Round-trips, and optional fields deserialize when present.
		let back: SlashCommandInfo = serde_json::from_value(json).unwrap();
		assert_eq!(back, info);
		let with_all: SlashCommandInfo = serde_json::from_value(serde_json::json!({
			"name": "ext",
			"description": "from extension",
			"source": "extension",
			"sourceInfo": {
				"path": "builtin:ext",
				"source": "builtin",
				"scope": "temporary",
				"origin": "package",
				"baseDir": "/x"
			}
		}))
		.unwrap();
		assert_eq!(with_all.description.as_deref(), Some("from extension"));
		assert_eq!(with_all.source_info.base_dir.as_deref(), Some("/x"));
		assert_eq!(with_all.source, SlashCommandSource::Extension);
		assert_eq!(with_all.source_info.origin, SourceOrigin::Package);
	}

	#[test]
	fn builtin_command_serialization_omits_missing_hint() {
		let settings = &builtin_slash_commands("pi")[0];
		let json = serde_json::to_value(settings).unwrap();
		assert_eq!(
			json,
			serde_json::json!({
				"name": "settings",
				"description": "Open settings menu"
			})
		);
		let model = &builtin_slash_commands("pi")[1];
		assert_eq!(
			serde_json::to_value(model).unwrap(),
			serde_json::json!({
				"name": "model",
				"description": "Select model (opens selector UI)",
				"argumentHint": "<provider/model>"
			})
		);
	}
}
