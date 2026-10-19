//! CLI argument parsing and help display.
//!
//! 1:1 port of `src/cli/args.ts`: [`Args`], [`parse_args`], [`print_help`],
//! [`normalize_session_name`], [`is_valid_thinking_level`], plus the
//! `getToolListError` validation used by `--tools` (from
//! `core/settings-manager.ts`).
//!
//! Quirks that MUST match the TS byte-for-byte (verified against the TS
//! source; see the `quirk_matrix` test):
//! - flags like `--provider` consume the next argv entry when one exists,
//!   even if it starts with `-`; with no following entry they fall through
//!   to the unknown-flag capture (`--provider` alone → `unknownFlags
//!   {"provider": true}`);
//! - `--mode`/`--use-theme`/`--tui-mode` diagnostics do NOT consume the
//!   following `-`-prefixed arg (it is then reported as an unknown option);
//! - `-p` grabs the next argv entry as a message unless it starts with `@`
//!   or with `-` (except `---`);
//! - `--list-models` grabs the next arg as a search pattern unless it starts
//!   with `-` or `@`;
//! - unknown `--flag=v` stores the raw remainder; `--flag value` stores the
//!   value when it does not start with `-`/`@`, else `true`;
//! - `--` ends option parsing; `@`-prefixed positionals become `fileArgs`.
//!
//! Help text: the env-var block lines (`ENV_AGENT_DIR.padEnd(32)` etc.) are
//! computed at runtime like the TS; the remainder is the verbatim TS output
//! (chalk writes plain text when stdout is piped — the parity case). The
//! `expected_help_*` test constants were captured from the TS `printHelp`.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::js_trim;
use crate::config::{CONFIG_DIR_NAME, ENV_AGENT_DIR, ENV_SESSION_DIR};
use pi_core::js_string::js_length;

// __HELP_CONSTANTS__

// =============================================================================
// Types
// =============================================================================

/// `Mode` — `"text" | "json" | "rpc"` (kept as a string like the TS union).
pub type Mode = String;

/// `TuiMode` — `"regular" | "fullscreen"`.
pub type TuiMode = String;

/// `--list-models [search]` value: `string | true`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ListModels {
    Search(String),
    Flag(bool),
}

/// `Args` interface (args.ts ~13). Field declaration order matches the TS
/// `JSON.stringify` output order: the four initializer keys first, then the
/// assignment-order fields in interface order.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Args {
    pub messages: Vec<String>,
    pub file_args: Vec<String>,
    /// Unknown flags (potentially extension flags) — insertion-ordered map
    /// of flag name to `true` or the string value.
    pub unknown_flags: Map<String, Value>,
    pub diagnostics: Vec<Diagnostic>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_session: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_builtin_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_extensions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_mcp: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_skills: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_templates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_prompt_templates: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub themes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_themes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_context_files: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_models: Option<ListModels>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tui_mode: Option<TuiMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbose: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_trust_override: Option<bool>,
}

/// Diagnostic entry: `{ type: "warning" | "error"; message: string }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        kind: "error".to_string(),
        message: message.into(),
    }
}

fn warning(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        kind: "warning".to_string(),
        message: message.into(),
    }
}

/// `ExtensionFlag` (core/extensions/types.ts ~2086) — the subset
/// `printHelp` consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionFlag {
    pub name: String,
    pub description: Option<String>,
    /// `"boolean" | "string"`.
    pub flag_type: &'static str,
    pub extension_path: String,
}

// =============================================================================
// Validation helpers
// =============================================================================

const VALID_THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `isValidThinkingLevel`.
pub fn is_valid_thinking_level(level: &str) -> bool {
    VALID_THINKING_LEVELS.contains(&level)
}

/// `normalizeSessionName` — trim; `None` when the result is empty.
pub fn normalize_session_name(value: &str) -> Option<String> {
    let name = js_trim(value);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// `isToolModifier` (core/settings-manager.ts ~88).
fn is_tool_modifier(entry: &str) -> bool {
    entry.starts_with('+') || entry.starts_with('-')
}

/// `getToolListError` (core/settings-manager.ts ~97).
pub fn get_tool_list_error(entries: &[String]) -> Option<String> {
    let modifiers: Vec<&String> = entries.iter().filter(|e| is_tool_modifier(e)).collect();
    if modifiers.is_empty() {
        return None;
    }
    if modifiers.len() < entries.len() {
        return Some("tool names cannot be mixed with +name or -name entries".to_string());
    }
    if let Some(pattern) = modifiers.iter().find(|e| e.contains('*')) {
        return Some(format!(
            "+name and -name entries take exact tool names, not patterns: {}",
            pattern
        ));
    }
    None
}

// =============================================================================
// parseArgs
// =============================================================================

fn split_tool_list(spec: &str) -> Vec<String> {
    spec.split(',')
        .map(js_trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// `parseArgs(args)` (args.ts ~69).
#[allow(clippy::too_many_lines)]
pub fn parse_args(args: &[String]) -> Args {
    let mut result = Args::default();
    let mut i = 0usize;

    while i < args.len() {
        let arg = args[i].as_str();

        if arg == "--" {
            for positional_arg in &args[i + 1..] {
                if positional_arg.starts_with('@') {
                    result.file_args.push(positional_arg[1..].to_string());
                } else {
                    result.messages.push(positional_arg.clone());
                }
            }
            break;
        } else if arg == "--help" || arg == "-h" {
            result.help = Some(true);
        } else if arg == "--version" || arg == "-v" {
            result.version = Some(true);
        } else if arg == "--mode" {
            let mode = args.get(i + 1).map(|s| s.as_str());
            if mode.is_none() || mode.unwrap().starts_with('-') {
                result.diagnostics.push(error("--mode requires text, json, or rpc"));
            } else {
                i += 1;
                let mode = mode.unwrap();
                if mode != "text" && mode != "json" && mode != "rpc" {
                    result
                        .diagnostics
                        .push(error(format!("Invalid mode \"{}\". Valid values: text, json, rpc", mode)));
                } else {
                    result.mode = Some(mode.to_string());
                }
            }
        } else if arg == "--continue" || arg == "-c" {
            result.continue_ = Some(true);
        } else if arg == "--resume" || arg == "-r" {
            result.resume = Some(true);
        } else if arg == "--provider" && i + 1 < args.len() {
            i += 1;
            result.provider = Some(args[i].clone());
        } else if arg == "--model" && i + 1 < args.len() {
            i += 1;
            result.model = Some(args[i].clone());
        } else if arg == "--api-key" && i + 1 < args.len() {
            i += 1;
            result.api_key = Some(args[i].clone());
        } else if arg == "--system-prompt" && i + 1 < args.len() {
            i += 1;
            result.system_prompt = Some(args[i].clone());
        } else if arg == "--append-system-prompt" && i + 1 < args.len() {
            i += 1;
            let list = result.append_system_prompt.get_or_insert_with(Vec::new);
            list.push(args[i].clone());
        } else if arg == "--name" || arg == "-n" {
            if i + 1 < args.len() {
                i += 1;
                result.name = Some(args[i].clone());
            } else {
                result.diagnostics.push(error("--name requires a value"));
            }
        } else if arg == "--no-session" {
            result.no_session = Some(true);
        } else if arg == "--session" && i + 1 < args.len() {
            i += 1;
            result.session = Some(args[i].clone());
        } else if arg == "--session-id" && i + 1 < args.len() {
            i += 1;
            result.session_id = Some(args[i].clone());
        } else if arg == "--fork" && i + 1 < args.len() {
            i += 1;
            result.fork = Some(args[i].clone());
        } else if arg == "--session-dir" && i + 1 < args.len() {
            i += 1;
            result.session_dir = Some(args[i].clone());
        } else if arg == "--models" && i + 1 < args.len() {
            i += 1;
            result.models = Some(split_tool_list(&args[i]));
        } else if arg == "--no-tools" || arg == "-nt" {
            result.no_tools = Some(true);
        } else if arg == "--no-builtin-tools" || arg == "-nbt" {
            result.no_builtin_tools = Some(true);
        } else if (arg == "--tools" || arg == "-t") && i + 1 < args.len() {
            i += 1;
            let tools = split_tool_list(&args[i]);
            if let Some(tool_error) = get_tool_list_error(&tools) {
                result.diagnostics.push(error(format!("{}: {}", arg, tool_error)));
            } else {
                result.tools = Some(tools);
            }
        } else if (arg == "--exclude-tools" || arg == "-xt") && i + 1 < args.len() {
            i += 1;
            result.exclude_tools = Some(split_tool_list(&args[i]));
        } else if arg == "--thinking" && i + 1 < args.len() {
            i += 1;
            let level = &args[i];
            if is_valid_thinking_level(level) {
                result.thinking = Some(level.clone());
            } else {
                result.diagnostics.push(warning(format!(
                    "Invalid thinking level \"{}\". Valid values: {}",
                    level,
                    VALID_THINKING_LEVELS.join(", ")
                )));
            }
        } else if arg == "--print" || arg == "-p" {
            result.print = Some(true);
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with('@') && (!next.starts_with('-') || next.starts_with("---")) {
                    result.messages.push(next.clone());
                    i += 1;
                }
            }
        } else if arg == "--export" && i + 1 < args.len() {
            i += 1;
            result.export = Some(args[i].clone());
        } else if (arg == "--extension" || arg == "-e") && i + 1 < args.len() {
            i += 1;
            let list = result.extensions.get_or_insert_with(Vec::new);
            list.push(args[i].clone());
        } else if arg == "--no-extensions" || arg == "-ne" {
            result.no_extensions = Some(true);
        } else if arg == "--no-mcp" {
            result.no_mcp = Some(true);
        } else if arg == "--skill" && i + 1 < args.len() {
            i += 1;
            let list = result.skills.get_or_insert_with(Vec::new);
            list.push(args[i].clone());
        } else if arg == "--prompt-template" && i + 1 < args.len() {
            i += 1;
            let list = result.prompt_templates.get_or_insert_with(Vec::new);
            list.push(args[i].clone());
        } else if arg == "--theme" && i + 1 < args.len() {
            i += 1;
            let list = result.themes.get_or_insert_with(Vec::new);
            list.push(args[i].clone());
        } else if arg == "--use-theme" {
            let theme_name = args.get(i + 1).map(|s| s.as_str());
            if theme_name.is_none() || theme_name.unwrap().starts_with('-') {
                result.diagnostics.push(error("--use-theme requires a theme name"));
            } else {
                result.use_theme = Some(theme_name.unwrap().to_string());
                i += 1;
            }
        } else if arg == "--no-skills" || arg == "-ns" {
            result.no_skills = Some(true);
        } else if arg == "--no-prompt-templates" || arg == "-np" {
            result.no_prompt_templates = Some(true);
        } else if arg == "--no-themes" {
            result.no_themes = Some(true);
        } else if arg == "--no-context-files" || arg == "-nc" {
            result.no_context_files = Some(true);
        } else if arg == "--list-models" {
            // Check if next arg is a search pattern (not a flag or file arg)
            match args.get(i + 1) {
                Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                    i += 1;
                    result.list_models = Some(ListModels::Search(next.clone()));
                }
                _ => {
                    result.list_models = Some(ListModels::Flag(true));
                }
            }
        } else if arg == "--tui-mode" {
            let mode = args.get(i + 1).map(|s| s.as_str());
            if mode == Some("regular") || mode == Some("fullscreen") {
                result.tui_mode = Some(mode.unwrap().to_string());
                i += 1;
            } else if mode.is_none() || mode.unwrap().starts_with('-') {
                result
                    .diagnostics
                    .push(error("--tui-mode requires regular or fullscreen"));
            } else {
                i += 1;
                result.diagnostics.push(error(format!(
                    "Invalid TUI mode \"{}\". Valid values: regular, fullscreen",
                    mode.unwrap()
                )));
            }
        } else if arg == "--verbose" {
            result.verbose = Some(true);
        } else if arg == "--approve" || arg == "-a" {
            result.project_trust_override = Some(true);
        } else if arg == "--no-approve" || arg == "-na" {
            result.project_trust_override = Some(false);
        } else if arg == "--offline" {
            result.offline = Some(true);
        } else if arg.starts_with('@') {
            result.file_args.push(arg[1..].to_string()); // Remove @ prefix
        } else if arg.starts_with("--") {
            if let Some(eq_index) = arg.find('=') {
                result
                    .unknown_flags
                    .insert(arg[2..eq_index].to_string(), Value::String(arg[eq_index + 1..].to_string()));
            } else {
                let flag_name = arg[2..].to_string();
                match args.get(i + 1) {
                    Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                        result.unknown_flags.insert(flag_name, Value::String(next.clone()));
                        i += 1;
                    }
                    _ => {
                        result.unknown_flags.insert(flag_name, Value::Bool(true));
                    }
                }
            }
        } else if arg.starts_with('-') {
            result.diagnostics.push(error(format!("Unknown option: {}", arg)));
        } else {
            result.messages.push(arg.to_string());
        }

        i += 1;
    }

    result
}

// =============================================================================
// printHelp
// =============================================================================

/// JS `String.prototype.padEnd(width)` with UTF-16 length semantics.
fn pad_end(s: &str, width: usize) -> String {
    let len = js_length(s);
    if len >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - len))
    }
}

// Generated from src/cli/args.ts printHelp template (chalk stripped, APP_NAME="pi",
// CONFIG_DIR_NAME=".pi" resolved). HEAD ends at ${extensionFlagsText}; MID runs from
// there to the ENV_AGENT_DIR line prefix (the 2-space prefix + padded var names are
// rendered at runtime); TAIL resumes after the ENV_SESSION_DIR var name.
const HELP_HEAD: &str = r#"pi - AI coding assistant with read, bash, edit, write tools

Usage:
  pi [options] [--] [@files...] [messages...]

Commands:
  pi install <source> [-l]     Install extension source and add to settings
  pi remove <source> [-l]      Remove extension source from settings
  pi uninstall <source> [-l]   Alias for remove
  pi update [source|self|pi]   Update pi, extensions, or model catalogs
  pi list                      List installed extensions from settings
  pi config [-l]               Open TUI to enable/disable package resources (Tab switches scope)
  pi auth <command>            Print credentials or check provider readiness
  pi mcp <command>             Check MCP servers, sign in to or out of OAuth servers
  pi <command> --help          Show help for install/remove/uninstall/update/list/config/auth/mcp

Options:
  --provider <name>              Provider to search for --model (requires --model)
  --model <pattern>              Model pattern or ID (supports "provider/id" and optional ":<thinking>")
  --api-key <key>                API key (defaults to env vars)
  --system-prompt <text>         System prompt (default: coding assistant prompt)
  --append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)
  --mode <mode>                  Output mode: text (default), json, or rpc
  --print, -p                    Non-interactive mode: process prompt and exit
  --continue, -c                 Continue previous session
  --resume, -r                   Select a session to resume
  --session <path|id>            Use specific session file or partial UUID
  --session-id <id>              Use exact project session ID, creating it if missing
  --fork <path|id>               Fork specific session file or partial UUID into a new session
  --session-dir <dir>            Directory for session storage and lookup
  --no-session                   Don't save session (ephemeral)
  --name, -n <name>              Set session display name
  --models <patterns>            Comma-separated model patterns for Ctrl+P cycling
                                 Supports globs (anthropic/*, *sonnet*) and fuzzy matching
  --no-tools, -nt                Disable all tools by default (built-in and extension)
  --no-builtin-tools, -nbt       Disable built-in tools by default but keep extension/custom tools enabled
  --tools, -t <tools>            Comma-separated allowlist of tool names or patterns (*) to enable
                                 Keeps MCP tools unless an entry starts with mcp__
                                 Only +name/-name entries add to or remove from the defaults
  --exclude-tools, -xt <tools>   Comma-separated denylist of tool names or patterns (*) to disable
                                 Applies to all tools, MCP tools included
  --thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh, max
  --extension, -e <path>         Load an extension file or builtin:<name> (can be used multiple times)
  --no-extensions, -ne           Disable extension discovery and built-in extensions (explicit -e paths still work)
  --no-mcp                       Disable built-in MCP support: no servers connect and no MCP tools
  --skill <path>                 Load a skill file or directory (can be used multiple times)
  --no-skills, -ns               Disable skills discovery and loading
  --prompt-template <path>       Load a prompt template file or directory (can be used multiple times)
  --no-prompt-templates, -np     Disable prompt template discovery and loading
  --theme <path>                 Load a theme file or directory (can be used multiple times)
  --use-theme <name[/name]>      Set the initial interactive theme for this run
  --no-themes                    Disable theme discovery and loading
  --no-context-files, -nc        Disable AGENTS.md and CLAUDE.md discovery and loading
  --export <file>                Export session file to HTML and exit
  --list-models [search]         List available models (with optional fuzzy search)
  --verbose                      Force verbose startup (overrides quietStartup setting)
  --tui-mode <mode>              TUI mode: fullscreen (default) or regular
  --approve, -a                  Trust project-local files for this run
  --no-approve, -na              Ignore project-local files for this run
  --offline                      Disable startup network operations (same as PI_OFFLINE=1)
  --                             End option parsing; treat remaining arguments as messages/files
  --help, -h                     Show this help
  --version, -v                  Show version number

Extensions can register additional flags (e.g., --plan from plan-mode extension)."#;

const HELP_MID: &str = r#"

Examples:
  # Print a provider API key for an external client
  pi auth print-api-key --provider openai

  # Print an OAuth bearer token for an external client (refreshes if expired)
  pi auth print-bearer-token --provider openai-codex

  # Interactive mode
  pi

  # Interactive mode with initial prompt
  pi "List all .ts files in src/"

  # Include files in initial message
  pi @prompt.md @image.png "What color is the sky?"

  # Non-interactive mode (process and exit)
  pi -p "List all .ts files in src/"

  # Prompt beginning with a dash
  pi -p -- "- Summarize these points"

  # Multiple messages (interactive)
  pi "Read package.json" "What dependencies do we have?"

  # Continue previous session
  pi --continue "What did we discuss?"

  # Start a named session
  pi --name "Refactor auth module"

  # Use different model
  pi --provider openai --model gpt-4o-mini "Help me refactor this code"

  # Use model with provider prefix (no --provider needed)
  pi --model openai/gpt-4o "Help me refactor this code"

  # Use model with thinking level shorthand
  pi --model sonnet:high "Solve this complex problem"

  # Limit model cycling to specific models
  pi --models claude-sonnet,claude-haiku,gpt-4o

  # Limit to a specific provider with glob pattern
  pi --models "github-copilot/*"

  # Cycle models with fixed thinking levels
  pi --models sonnet:high,haiku:low

  # Start with a specific thinking level
  pi --thinking high "Solve this complex problem"

  # Read-only mode (no file modifications possible)
  pi --tools read,grep,find,ls -p "Review the code in src/"

  # Add codemode to the default tools
  pi --tools +codemode

  # Codemode with only the tools of one MCP server
  pi --tools read,bash,codemode,'mcp__radius__*'

  # Disable one tool while keeping the rest available
  pi --exclude-tools ask_question

  # Export a session file to HTML
  pi --export ~/.pi/agent/sessions/--path--/session.jsonl
  pi --export session.jsonl output.html

Environment Variables:
  ANTHROPIC_AUTH_TOKEN             - Anthropic bearer auth token
  ANTHROPIC_API_KEY                - Anthropic Claude API key
  ANTHROPIC_OAUTH_TOKEN            - Anthropic OAuth token (alternative to API key)
  ANT_LING_API_KEY                 - Ant Ling API key
  OPENAI_API_KEY                   - OpenAI GPT API key
  AZURE_OPENAI_API_KEY             - Azure OpenAI API key
  AZURE_OPENAI_BASE_URL            - Azure OpenAI/Cognitive Services base URL (e.g. https://{resource}.openai.azure.com)
  AZURE_OPENAI_RESOURCE_NAME       - Azure OpenAI resource name (alternative to base URL)
  AZURE_OPENAI_API_VERSION         - Azure OpenAI API version (default: v1)
  AZURE_OPENAI_DEPLOYMENT_NAME_MAP - Azure OpenAI model=deployment map (comma-separated)
  DEEPSEEK_API_KEY                 - DeepSeek API key
  NVIDIA_API_KEY                   - NVIDIA NIM API key
  GEMINI_API_KEY                   - Google Gemini API key
  GROQ_API_KEY                     - Groq API key
  CEREBRAS_API_KEY                 - Cerebras API key
  XAI_API_KEY                      - xAI Grok API key
  FIREWORKS_API_KEY                - Fireworks API key
  TOGETHER_API_KEY                 - Together AI API key
  BASETEN_API_KEY                  - Baseten API key
  OPENROUTER_API_KEY               - OpenRouter API key
  AI_GATEWAY_API_KEY               - Vercel AI Gateway API key
  ZAI_API_KEY                      - ZAI Coding Plan API key (Global)
  ZAI_CODING_CN_API_KEY            - ZAI Coding Plan API key (China)
  MISTRAL_API_KEY                  - Mistral API key
  MINIMAX_API_KEY                  - MiniMax API key
  MOONSHOT_API_KEY                 - Moonshot AI API key
  OPENCODE_API_KEY                 - OpenCode Zen/OpenCode Go API key
  KIMI_API_KEY                     - Kimi For Coding API key
  META_API_KEY                     - Meta Model API key
  CLOUDFLARE_API_KEY               - Cloudflare API token (Workers AI and AI Gateway)
  CLOUDFLARE_ACCOUNT_ID            - Cloudflare account id (required for both)
  CLOUDFLARE_GATEWAY_ID            - Cloudflare AI Gateway slug (required for AI Gateway)
  QWEN_TOKEN_PLAN_API_KEY          - Qwen Token Plan API key (international region)
  QWEN_TOKEN_PLAN_CN_API_KEY       - Qwen Token Plan API key (China region)
  XIAOMI_API_KEY                   - Xiaomi MiMo API key (api.xiaomimimo.com billing)
  XIAOMI_TOKEN_PLAN_CN_API_KEY     - Xiaomi MiMo Token Plan API key (China region)
  XIAOMI_TOKEN_PLAN_AMS_API_KEY    - Xiaomi MiMo Token Plan API key (Amsterdam region)
  XIAOMI_TOKEN_PLAN_SGP_API_KEY    - Xiaomi MiMo Token Plan API key (Singapore region)
  AWS_PROFILE                      - AWS profile for Amazon Bedrock
  AWS_ACCESS_KEY_ID                - AWS access key for Amazon Bedrock
  AWS_SECRET_ACCESS_KEY            - AWS secret key for Amazon Bedrock
  AWS_BEARER_TOKEN_BEDROCK         - Bedrock API key (bearer token)
  AWS_REGION                       - AWS region for Amazon Bedrock (e.g., us-east-1)
"#;

const HELP_TAIL: &str = r#" - Session storage directory (overridden by --session-dir)
  PI_PACKAGE_DIR                   - Override package directory (for Nix/Guix store paths)
  PI_OFFLINE                       - Disable startup network operations when set to 1/true/yes
  PI_TELEMETRY                     - Override install telemetry when set to 1/true/yes or 0/false/no
  PI_SHARE_VIEWER_URL              - Base URL for /share command (default: https://pi.dev/session/)

Built-in Tool Names:
  read       - Read file contents
  bash       - Execute bash commands
  powershell - Execute PowerShell commands on Windows
  edit       - Edit files with find/replace
  write      - Write files (creates/overwrites)
  grep       - Search file contents (read-only, off by default)
  find       - Find files by glob pattern (read-only, off by default)
  ls         - List directory contents (read-only, off by default)
"#;

/// `printHelp(extensionFlags?)` (args.ts ~301) — returns the text that the
/// TS writes with a single `console.log` call (including its trailing
/// newline). Chalk is plain-text (piped stdout).
pub fn print_help(extension_flags: Option<&[ExtensionFlag]>) -> String {
    // The two env-var lines use runtime padding, exactly like the TS:
    //   ${ENV_AGENT_DIR.padEnd(32)} - Config directory (default: ~/${CONFIG_DIR_NAME}/agent)
    //   ${ENV_SESSION_DIR.padEnd(32)} - Session storage directory (overridden by --session-dir)
    // HELP_TAIL resumes mid-session-line (right after the padded var name).
    let env_lines = format!(
        "  {} - Config directory (default: ~/{}/agent)\n  {}",
        pad_end(ENV_AGENT_DIR, 32),
        CONFIG_DIR_NAME,
        pad_end(ENV_SESSION_DIR, 32),
    );

    let extension_flags_text = match extension_flags {
        Some(flags) if !flags.is_empty() => {
            let entries: Vec<String> = flags
                .iter()
                .map(|flag| {
                    let value = if flag.flag_type == "string" { " <value>" } else { "" };
                    let description = flag
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("Registered by {}", flag.extension_path));
                    format!("{}{}", pad_end(&format!("  --{}{}", flag.name, value), 30), description)
                })
                .collect();
            format!("\nExtension CLI Flags:\n{}\n", entries.join("\n"))
        }
        _ => String::new(),
    };

    format!(
        "{}{}{}{}{}",
        HELP_HEAD, extension_flags_text, HELP_MID, env_lines, HELP_TAIL
    )
}

// __HELP_TESTS__

// __HELP_SNAPSHOT_TEST__

#[cfg(test)]
mod help_snapshot {
    use super::*;

    /// Byte-for-byte snapshot of the TS `printHelp()` output (chalk disabled —
    /// piped stdout), captured from the original dist. `console.log` emits the
    /// template plus one trailing newline; `print_help` returns the same bytes.
    #[test]
    fn print_help_matches_ts_snapshot() {
        let expected = include_str!("help-snapshot.txt");
        assert_eq!(print_help(None), expected);
    }

    #[test]
    fn print_help_extension_flags_shape() {
        let flags = [ExtensionFlag {
            name: "plan".to_string(),
            flag_type: "boolean",
            description: Some("Enable plan mode".to_string()),
            extension_path: "/x/ext.js".to_string(),
        }];
        let out = print_help(Some(&flags));
        assert!(out.contains("Extension CLI Flags:\n  --plan                      Enable plan mode\n"));
        // Unknown description falls back to "Registered by <path>".
        let flags = [ExtensionFlag {
            name: "verbose2".to_string(),
            flag_type: "string",
            description: None,
            extension_path: "/y/e.js".to_string(),
        }];
        let out = print_help(Some(&flags));
        assert!(out.contains("--verbose2 <value>"));
        assert!(out.contains("Registered by /y/e.js"));
    }
}
