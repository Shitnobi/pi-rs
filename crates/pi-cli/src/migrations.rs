//! One-time startup migrations over `~/.pi/agent` and project `.pi/`.
//!
//! 1:1 port of `src/migrations.ts`. Filesystem access goes through the
//! [`FsProvider`] trait and env access through [`EnvProvider`] (both defined
//! in [`crate::config`]); `console.log` output is captured in a caller-supplied
//! sink so the migration messages are testable. Chalk colors are stripped
//! (chalk writes plain text when stdout is piped — the parity-harness case).
//!
//! JS semantics to watch here (all ported):
//! - `JSON.stringify(x, null, 2)` pretty printing incl. JS number formatting
//!   (integral floats without `.0`, `1e21` → `1e+21`, `-0` → `0`, non-finite
//!   → `null`) — see [`js_stringify_pretty`].
//! - `Object.entries` iterates arrays by index keys; spread of a non-object
//!   contributes nothing.
//! - Partial mutation on caught errors: entries already inserted into the
//!   outer `migrated` map survive a later `renameSync` failure.

use pi_core::text::strip_bom;
use serde_json::{Map, Value};

use crate::cli::js_trim;
use crate::config::{get_agent_dir, get_bin_dir, CONFIG_DIR_NAME, EnvProvider, FsProvider};
use pi_session::config::{node_dirname, node_join};
use serde::Serialize;

pub const MIGRATION_GUIDE_URL: &str =
    "https://github.com/earendil-works/pi/blob/main/packages/coding-agent/CHANGELOG.md#extensions-migration";
pub const EXTENSIONS_DOC_URL: &str =
    "https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md";

// ============================================================================
// JSON.stringify(x, null, 2)
// ============================================================================

/// JS `Number.prototype.toString()` for a finite f64 (JSON.stringify path).
/// Fixed notation on `[1e-6, 1e21)`, exponent form outside it (with the `+`
/// sign on positive exponents, like JS).
fn js_f64_to_string(v: f64) -> String {
    if !v.is_finite() {
        // JSON.stringify maps non-finite numbers to null.
        return "null".to_string();
    }
    if v == 0.0 {
        // JSON.stringify(-0) === "0".
        return "0".to_string();
    }
    let a = v.abs();
    if (1e-6..1e21).contains(&a) {
        // Rust's f64 Display prints shortest round-trip digits and integral
        // values without a decimal point — same as JS in this range.
        format!("{}", v)
    } else {
        let mut s = format!("{:e}", v);
        if let Some(pos) = s.find('e') {
            if !s[pos + 1..].starts_with('-') {
                s.insert(pos + 1, '+');
            }
        }
        s
    }
}

/// `JSON.stringify(value, null, 2)` for a parsed-JSON value — byte-identical
/// output for objects, arrays, strings, booleans, null and JS numbers.
pub fn js_stringify_pretty(value: &Value) -> String {
    fn go(value: &Value, depth: usize) -> String {
        let indent = "  ".repeat(depth);
        let inner = "  ".repeat(depth + 1);
        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    return "{}".to_string();
                }
                let mut s = String::from("{\n");
                let n = map.len();
                for (i, (k, v)) in map.iter().enumerate() {
                    s.push_str(&inner);
                    s.push_str(&serde_json::to_string(&Value::String(k.clone())).unwrap());
                    s.push_str(": ");
                    s.push_str(&go(v, depth + 1));
                    if i + 1 < n {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&indent);
                s.push('}');
                s
            }
            Value::Array(items) => {
                if items.is_empty() {
                    return "[]".to_string();
                }
                let mut s = String::from("[\n");
                let n = items.len();
                for (i, v) in items.iter().enumerate() {
                    s.push_str(&inner);
                    s.push_str(&go(v, depth + 1));
                    if i + 1 < n {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&indent);
                s.push(']');
                s
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.to_string()
                } else if let Some(u) = n.as_u64() {
                    u.to_string()
                } else {
                    js_f64_to_string(n.as_f64().unwrap_or(0.0))
                }
            }
            other => serde_json::to_string(other).unwrap_or_default(),
        }
    }
    go(value, 0)
}

/// `Object.entries(value)` — objects by key, arrays by index key, strings by
/// (char) index, everything else empty.
fn object_entries(v: &Value) -> Vec<(String, Value)> {
    match v {
        Value::Object(m) => m.iter().map(|(k, val)| (k.clone(), val.clone())).collect(),
        Value::Array(a) => a.iter().enumerate().map(|(i, val)| (i.to_string(), val.clone())).collect(),
        // Object.entries("ab") → [["0","a"],["1","b"]] (UTF-16 indices; char
        // indices are equivalent for BMP-only content, which covers any
        // realistic credential file).
        Value::String(s) => s
            .chars()
            .enumerate()
            .map(|(i, c)| (i.to_string(), Value::String(c.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

/// Object spread `{ ...value }` — own enumerable properties of objects,
/// arrays (index keys) and strings; primitives contribute nothing.
fn spread_into(map: &mut Map<String, Value>, v: &Value) {
    for (k, val) in object_entries(v) {
        map.insert(k, val);
    }
}

// ============================================================================
// migrateAuthToAuthJson (migrations.ts ~19)
// ============================================================================

/// `migrateAuthToAuthJson()` — migrate legacy `oauth.json` and
/// `settings.json` `apiKeys` to `auth.json` (written with mode 0600).
///
/// Returns the provider names that were migrated. Like the TS, the final
/// `mkdirSync`/`writeFileSync` of `auth.json` are *not* guarded — an error
/// there propagates to the caller (`runMigrations` in the TS can throw).
pub fn migrate_auth_to_auth_json(
    env: &dyn EnvProvider,
    fs: &dyn FsProvider,
) -> Result<Vec<String>, String> {
    let agent_dir = get_agent_dir(env);
    let auth_path = node_join(&agent_dir, &["auth.json"]);
    let oauth_path = node_join(&agent_dir, &["oauth.json"]);
    let settings_path = node_join(&agent_dir, &["settings.json"]);

    // Skip if auth.json already exists.
    if fs.exists(&auth_path) {
        return Ok(vec![]);
    }

    let mut migrated: Map<String, Value> = Map::new();
    let mut providers: Vec<String> = Vec::new();

    // Migrate oauth.json
    if fs.exists(&oauth_path) {
        let attempt: Result<(), String> = (|| {
            let content = fs.read_file(&oauth_path)?;
            let oauth: Value = serde_json::from_str(strip_bom(&content)).map_err(|e| e.to_string())?;
            for (provider, cred) in object_entries(&oauth) {
                let mut entry = Map::new();
                entry.insert("type".to_string(), Value::String("oauth".to_string()));
                spread_into(&mut entry, &cred);
                migrated.insert(provider.clone(), Value::Object(entry));
                providers.push(provider);
            }
            fs.rename(&oauth_path, &format!("{}.migrated", oauth_path))?;
            Ok(())
        })();
        // Skip on error (but keep partial mutations, like the TS try/catch).
        attempt.ok();
    }

    // Migrate settings.json apiKeys
    if fs.exists(&settings_path) {
        let attempt: Result<(), String> = (|| {
            let content = fs.read_file(&settings_path)?;
            let mut settings: Value =
                serde_json::from_str(strip_bom(&content)).map_err(|e| e.to_string())?;
            // `settings.apiKeys && typeof settings.apiKeys === "object"` —
            // truthy AND typeof object (objects and arrays; null is falsy).
            let api_keys_truthy_object = match settings.get("apiKeys") {
                Some(Value::Object(_)) | Some(Value::Array(_)) => true,
                _ => false,
            };
            if api_keys_truthy_object {
                for (provider, key) in object_entries(settings.get("apiKeys").unwrap()) {
                    if !migrated.contains_key(&provider) && key.is_string() {
                        let mut entry = Map::new();
                        entry.insert("type".to_string(), Value::String("api_key".to_string()));
                        entry.insert("key".to_string(), key);
                        migrated.insert(provider.clone(), Value::Object(entry));
                        providers.push(provider);
                    }
                }
                if let Some(obj) = settings.as_object_mut() {
                    obj.remove("apiKeys");
                }
                fs.write_file(&settings_path, &js_stringify_pretty(&settings), None)?;
            }
            Ok(())
        })();
        attempt.ok();
    }

    if !migrated.is_empty() {
        fs.mkdir_recursive(&node_dirname(&auth_path))?;
        fs.write_file(&auth_path, &js_stringify_pretty(&Value::Object(migrated)), Some(0o600))?;
    }

    Ok(providers)
}

// ============================================================================
// migrateSessionsFromAgentRoot (migrations.ts ~87)
// ============================================================================

/// `migrateSessionsFromAgentRoot()` — move v0.30.0 stray `*.jsonl` session
/// files from the agent root into `sessions/<encoded-cwd>/` based on the cwd
/// in each file's session header. Per-file errors are swallowed.
pub fn migrate_sessions_from_agent_root(env: &dyn EnvProvider, fs: &dyn FsProvider) {
    let agent_dir = get_agent_dir(env);

    // Find all .jsonl files directly in agentDir (not in subdirectories).
    let files: Vec<String> = match fs.read_dir(&agent_dir) {
        Ok(names) => names
            .iter()
            .filter(|f| f.ends_with(".jsonl"))
            .map(|f| node_join(&agent_dir, &[f]))
            .collect(),
        Err(_) => return,
    };

    if files.is_empty() {
        return;
    }

    for file in &files {
        let attempt: Result<(), String> = (|| {
            // Read first line to get session header.
            let content = fs.read_file(file)?;
            let first_line = content.split('\n').next().unwrap_or("");
            if js_trim(first_line).is_empty() {
                return Ok(());
            }

            let header: Value =
                serde_json::from_str(first_line).map_err(|e| e.to_string())?;
            let header_type = header.get("type").and_then(|t| t.as_str());
            let header_cwd = header.get("cwd").and_then(|c| c.as_str());
            // `header.type !== "session" || !header.cwd` — an empty-string cwd is
            // falsy and skips; a truthy non-string cwd would throw on `.replace`
            // in the TS and skip the file, which is observationally identical.
            if header_type != Some("session") || header_cwd.map_or(true, |c| c.is_empty()) {
                return Ok(());
            }
            let cwd = header_cwd.unwrap();

            // Compute the correct session directory (same encoding as
            // session-manager.ts): strip ONE leading / or \, then replace
            // each of / \ : with '-'.
            let stripped = cwd
                .strip_prefix('/')
                .or_else(|| cwd.strip_prefix('\\'))
                .unwrap_or(cwd);
            let safe_path = format!(
                "--{}--",
                stripped
                    .chars()
                    .map(|c| if c == '/' || c == '\\' || c == ':' { '-' } else { c })
                    .collect::<String>()
            );
            let correct_dir = node_join(&agent_dir, &["sessions", &safe_path]);

            // Create directory if needed.
            if !fs.exists(&correct_dir) {
                fs.mkdir_recursive(&correct_dir)?;
            }

            // Move the file (`file.split("/").pop()`).
            let file_name = file.rsplit('/').next().unwrap_or(file);
            let new_path = node_join(&correct_dir, &[file_name]);

            if fs.exists(&new_path) {
                return Ok(()); // Skip if target exists
            }

            fs.rename(file, &new_path)?;
            Ok(())
        })();
        attempt.ok(); // Skip files that can't be migrated.
    }
}

// ============================================================================
// migrateCommandsToPrompts (migrations.ts ~135)
// ============================================================================

/// `migrateCommandsToPrompts(baseDir, label)` — rename `commands/` to
/// `prompts/` when prompts does not exist yet.
fn migrate_commands_to_prompts(
    base_dir: &str,
    label: &str,
    fs: &dyn FsProvider,
    log: &mut dyn FnMut(&str),
) -> bool {
    let commands_dir = node_join(base_dir, &["commands"]);
    let prompts_dir = node_join(base_dir, &["prompts"]);

    if fs.exists(&commands_dir) && !fs.exists(&prompts_dir) {
        match fs.rename(&commands_dir, &prompts_dir) {
            Ok(()) => {
                log(&format!("Migrated {} commands/ → prompts/", label));
                return true;
            }
            Err(err) => {
                log(&format!(
                    "Warning: Could not migrate {} commands/ to prompts/: {}",
                    label, err
                ));
            }
        }
    }
    false
}

// ============================================================================
// migrateKeybindingsConfigFile (migrations.ts ~158)
// ============================================================================

/// `KEYBINDING_NAME_MIGRATIONS` (core/keybindings.ts ~200) — legacy flat
/// names → current dotted names.
const KEYBINDING_NAME_MIGRATIONS: &[(&str, &str)] = &[
    ("cursorUp", "tui.editor.cursorUp"),
    ("cursorDown", "tui.editor.cursorDown"),
    ("cursorLeft", "tui.editor.cursorLeft"),
    ("cursorRight", "tui.editor.cursorRight"),
    ("cursorWordLeft", "tui.editor.cursorWordLeft"),
    ("cursorWordRight", "tui.editor.cursorWordRight"),
    ("cursorLineStart", "tui.editor.cursorLineStart"),
    ("cursorLineEnd", "tui.editor.cursorLineEnd"),
    ("jumpForward", "tui.editor.jumpForward"),
    ("jumpBackward", "tui.editor.jumpBackward"),
    ("pageUp", "tui.editor.pageUp"),
    ("pageDown", "tui.editor.pageDown"),
    ("deleteCharBackward", "tui.editor.deleteCharBackward"),
    ("deleteCharForward", "tui.editor.deleteCharForward"),
    ("deleteWordBackward", "tui.editor.deleteWordBackward"),
    ("deleteWordForward", "tui.editor.deleteWordForward"),
    ("deleteToLineStart", "tui.editor.deleteToLineStart"),
    ("deleteToLineEnd", "tui.editor.deleteToLineEnd"),
    ("yank", "tui.editor.yank"),
    ("yankPop", "tui.editor.yankPop"),
    ("undo", "tui.editor.undo"),
    ("newLine", "tui.input.newLine"),
    ("submit", "tui.input.submit"),
    ("tab", "tui.input.tab"),
    ("copy", "tui.input.copy"),
    ("selectUp", "tui.select.up"),
    ("selectDown", "tui.select.down"),
    ("selectPageUp", "tui.select.pageUp"),
    ("selectPageDown", "tui.select.pageDown"),
    ("selectConfirm", "tui.select.confirm"),
    ("selectCancel", "tui.select.cancel"),
    ("interrupt", "app.interrupt"),
    ("clear", "app.clear"),
    ("exit", "app.exit"),
    ("suspend", "app.suspend"),
    ("cycleThinkingLevel", "app.thinking.cycle"),
    ("cycleModelForward", "app.model.cycleForward"),
    ("cycleModelBackward", "app.model.cycleBackward"),
    ("selectModel", "app.model.select"),
    ("expandTools", "app.tools.expand"),
    ("toggleThinking", "app.thinking.toggle"),
    ("toggleSessionNamedFilter", "app.session.toggleNamedFilter"),
    ("externalEditor", "app.editor.external"),
    ("followUp", "app.message.followUp"),
    ("dequeue", "app.message.dequeue"),
    ("pasteImage", "app.clipboard.pasteImage"),
    ("newSession", "app.session.new"),
    ("tree", "app.session.tree"),
    ("fork", "app.session.fork"),
    ("resume", "app.session.resume"),
    ("treeFoldOrUp", "app.tree.foldOrUp"),
    ("treeUnfoldOrDown", "app.tree.unfoldOrDown"),
    ("treeEditLabel", "app.tree.editLabel"),
    ("treeToggleLabelTimestamp", "app.tree.toggleLabelTimestamp"),
    ("toggleSessionPath", "app.session.togglePath"),
    ("toggleSessionSort", "app.session.toggleSort"),
    ("renameSession", "app.session.rename"),
    ("deleteSession", "app.session.delete"),
    ("deleteSessionNoninvasive", "app.session.deleteNoninvasive"),
];

/// `APP_KEYBINDINGS` key order (core/keybindings.ts ~25–172) — the tail of
/// `Object.keys(KEYBINDINGS)` after the `TUI_KEYBINDINGS` keys.
const APP_KEYBINDINGS: &[&str] = &[
    "app.interrupt",
    "app.clear",
    "app.exit",
    "app.suspend",
    "app.thinking.cycle",
    "app.thinking.save",
    "app.model.cycleForward",
    "app.model.cycleBackward",
    "app.model.select",
    "app.tools.expand",
    "app.thinking.toggle",
    "app.session.toggleNamedFilter",
    "app.editor.external",
    "app.message.copy",
    "app.message.followUp",
    "app.message.dequeue",
    "app.clipboard.pasteImage",
    "app.session.new",
    "app.session.tree",
    "app.session.fork",
    "app.session.resume",
    "app.tree.foldOrUp",
    "app.tree.unfoldOrDown",
    "app.tree.editLabel",
    "app.tree.toggleLabelTimestamp",
    "app.session.togglePath",
    "app.session.toggleSort",
    "app.session.rename",
    "app.session.delete",
    "app.session.deleteNoninvasive",
    "app.models.save",
    "app.models.enableAll",
    "app.models.clearAll",
    "app.models.toggleProvider",
    "app.models.reorderUp",
    "app.models.reorderDown",
    "app.tree.filter.default",
    "app.tree.filter.noTools",
    "app.tree.filter.userOnly",
    "app.tree.filter.labeledOnly",
    "app.tree.filter.all",
    "app.tree.filter.cycleForward",
    "app.tree.filter.cycleBackward",
];

/// `Object.keys(KEYBINDINGS)` order: TUI keys (the four overridden keys keep
/// their original positions when re-assigned by spread) then app keys.
fn keybindings_order() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = pi_tui::keybindings::TUI_KEYBINDINGS.iter().map(|k| k.id).collect();
    v.extend_from_slice(APP_KEYBINDINGS);
    v
}

fn legacy_keybinding_target(key: &str) -> Option<&'static str> {
    KEYBINDING_NAME_MIGRATIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
}

/// `orderKeybindingsConfig` (core/keybindings.ts ~300) — known keys in
/// KEYBINDINGS order, then remaining keys sorted.
fn order_keybindings_config(config: Map<String, Value>) -> Map<String, Value> {
    let mut ordered = Map::new();
    for keybinding in keybindings_order() {
        if let Some(v) = config.get(keybinding) {
            ordered.insert(keybinding.to_string(), v.clone());
        }
    }
    let mut extras: Vec<&String> = config.keys().filter(|k| !ordered.contains_key(k.as_str())).collect();
    extras.sort();
    for k in extras {
        ordered.insert(k.clone(), config[k].clone());
    }
    ordered
}

/// `migrateKeybindingsConfig` (core/keybindings.ts ~278).
pub fn migrate_keybindings_config(raw: &Map<String, Value>) -> (Map<String, Value>, bool) {
    let mut config: Map<String, Value> = Map::new();
    let mut migrated = false;

    for (key, value) in raw {
        let next_key = legacy_keybinding_target(key).unwrap_or(key.as_str());
        if next_key != key {
            migrated = true;
        }
        if key.as_str() != next_key && raw.contains_key(next_key) {
            migrated = true;
            continue;
        }
        config.insert(next_key.to_string(), value.clone());
    }

    (order_keybindings_config(config), migrated)
}

/// `migrateKeybindingsConfigFile()` (migrations.ts ~158) — rewrite
/// `~/.pi/agent/keybindings.json` when the name migration changes it.
/// Malformed files are ignored.
fn migrate_keybindings_config_file(env: &dyn EnvProvider, fs: &dyn FsProvider) {
    let config_path = node_join(&get_agent_dir(env), &["keybindings.json"]);
    if !fs.exists(&config_path) {
        return;
    }

    let attempt: Result<(), String> = (|| {
        let content = fs.read_file(&config_path)?;
        let parsed: Value = serde_json::from_str(strip_bom(&content)).map_err(|e| e.to_string())?;
        // typeof object, not null, not array.
        let map = match parsed {
            Value::Object(m) => m,
            _ => return Ok(()),
        };
        let (config, migrated) = migrate_keybindings_config(&map);
        if !migrated {
            return Ok(());
        }
        fs.write_file(&config_path, &format!("{}\n", js_stringify_pretty(&Value::Object(config))), None)?;
        Ok(())
    })();
    attempt.ok(); // Ignore malformed files during migration.
}

// ============================================================================
// migrateToolsToBin (migrations.ts ~173)
// ============================================================================

/// `migrateToolsToBin()` — move fd/rg binaries from `tools/` to `bin/`.
/// The unguarded `mkdirSync` propagates errors (as in the TS).
fn migrate_tools_to_bin(env: &dyn EnvProvider, fs: &dyn FsProvider, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    let agent_dir = get_agent_dir(env);
    let tools_dir = node_join(&agent_dir, &["tools"]);
    let bin_dir = get_bin_dir(env);

    if !fs.exists(&tools_dir) {
        return Ok(());
    }

    let binaries = ["fd", "rg", "fd.exe", "rg.exe"];
    let mut moved_any = false;

    for bin in binaries {
        let old_path = node_join(&tools_dir, &[bin]);
        let new_path = node_join(&bin_dir, &[bin]);

        if fs.exists(&old_path) {
            if !fs.exists(&bin_dir) {
                fs.mkdir_recursive(&bin_dir)?;
            }
            if !fs.exists(&new_path) {
                if fs.rename(&old_path, &new_path).is_ok() {
                    moved_any = true;
                }
            } else {
                // Target exists, just delete the old one.
                let _ = fs.remove_file(&old_path);
            }
        }
    }

    if moved_any {
        log("Migrated managed binaries tools/ → bin/");
    }
    Ok(())
}

// ============================================================================
// checkDeprecatedExtensionDirs / migrateExtensionSystem (migrations.ts ~205, ~243)
// ============================================================================

/// `checkDeprecatedExtensionDirs(baseDir, label)` — collect warnings about
/// `hooks/` and custom `tools/` content.
fn check_deprecated_extension_dirs(base_dir: &str, label: &str, fs: &dyn FsProvider) -> Vec<String> {
    let hooks_dir = node_join(base_dir, &["hooks"]);
    let tools_dir = node_join(base_dir, &["tools"]);
    let mut warnings: Vec<String> = Vec::new();

    if fs.exists(&hooks_dir) {
        warnings.push(format!(
            "{} hooks/ directory found. Hooks have been renamed to extensions.",
            label
        ));
    }

    if fs.exists(&tools_dir) {
        // Check if tools/ contains anything other than fd/rg (auto-extracted).
        if let Ok(entries) = fs.read_dir(&tools_dir) {
            let custom_tools = entries.iter().any(|e| {
                let lower = e.to_lowercase();
                lower != "fd"
                    && lower != "rg"
                    && lower != "fd.exe"
                    && lower != "rg.exe"
                    && !e.starts_with('.') // Ignore .DS_Store and other hidden files
            });
            if custom_tools {
                warnings.push(format!(
                    "{} tools/ directory contains custom tools. Custom tools have been merged into extensions.",
                    label
                ));
            }
        }
    }

    warnings
}

/// `migrateExtensionSystem(cwd)` — commands→prompts migrations + deprecated
/// directory warnings.
fn migrate_extension_system(
    cwd: &str,
    env: &dyn EnvProvider,
    fs: &dyn FsProvider,
    log: &mut dyn FnMut(&str),
) -> Vec<String> {
    let agent_dir = get_agent_dir(env);
    let project_dir = node_join(cwd, &[CONFIG_DIR_NAME]);

    migrate_commands_to_prompts(&agent_dir, "Global", fs, log);
    migrate_commands_to_prompts(&project_dir, "Project", fs, log);

    let mut warnings = check_deprecated_extension_dirs(&agent_dir, "Global", fs);
    warnings.extend(check_deprecated_extension_dirs(&project_dir, "Project", fs));

    warnings
}

// ============================================================================
// showDeprecationWarnings (migrations.ts ~260)
// ============================================================================

/// The interactive half of `showDeprecationWarnings`: TS puts stdin in raw
/// mode and waits for one keypress.
pub trait DeprecationInput {
    fn wait_for_keypress(&self);
}

/// Waits for one line of stdin (raw mode is a TTY concern; the parity
/// harness pipes input, where a single newline arrives).
pub struct SystemDeprecationInput;

impl DeprecationInput for SystemDeprecationInput {
    fn wait_for_keypress(&self) {
        use std::io::BufRead;
        let _ = std::io::stdin().lock().lines().next();
    }
}

/// `showDeprecationWarnings(warnings)` — print warnings and wait for a
/// keypress. Output lines match the TS console.log calls with chalk
/// disabled (piped stdout); each line is passed to `log` without its
/// trailing newline.
pub fn show_deprecation_warnings(
    warnings: &[String],
    log: &mut dyn FnMut(&str),
    input: &dyn DeprecationInput,
) {
    if warnings.is_empty() {
        return;
    }

    for warning in warnings {
        log(&format!("Warning: {}", warning));
    }
    log("\nMove your extensions to the extensions/ directory.");
    log(&format!("Migration guide: {}", MIGRATION_GUIDE_URL));
    log(&format!("Documentation: {}", EXTENSIONS_DOC_URL));
    log("\nPress any key to continue...");
    input.wait_for_keypress();
    log("");
}

// ============================================================================
// runMigrations (migrations.ts ~297)
// ============================================================================

/// Result of `runMigrations(cwd)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MigrationResults {
    pub migrated_auth_providers: Vec<String>,
    pub deprecation_warnings: Vec<String>,
}

/// `runMigrations(cwd)` — run all migrations, called once on startup.
/// Can propagate errors from the unguarded `auth.json` write and the
/// `bin/` mkdir (both unguarded in the TS too).
pub fn run_migrations(
    cwd: &str,
    env: &dyn EnvProvider,
    fs: &dyn FsProvider,
    log: &mut dyn FnMut(&str),
) -> Result<MigrationResults, String> {
    let migrated_auth_providers = migrate_auth_to_auth_json(env, fs)?;
    migrate_sessions_from_agent_root(env, fs);
    migrate_tools_to_bin(env, fs, log)?;
    migrate_keybindings_config_file(env, fs);
    let deprecation_warnings = migrate_extension_system(cwd, env, fs, log);
    Ok(MigrationResults {
        migrated_auth_providers,
        deprecation_warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fakes::{FakeEnv, FakeFs};
    use serde_json::json;

    fn agent_fs() -> FakeFs {
        FakeFs::new().dir("/home/t/.pi/agent")
    }

    fn env() -> FakeEnv {
        FakeEnv::new("/home/t")
    }

    // -- js_stringify_pretty ---------------------------------------------------

    #[test]
    fn js_stringify_pretty_matches_node() {
        // Expected values produced by node: JSON.stringify(x, null, 2).
        assert_eq!(js_stringify_pretty(&json!({})), "{}");
        assert_eq!(js_stringify_pretty(&json!([])), "[]");
        assert_eq!(
            js_stringify_pretty(&json!({"a": 1})),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(
            js_stringify_pretty(&json!({"a": {"b": [1, 2]}})),
            "{\n  \"a\": {\n    \"b\": [\n      1,\n      2\n    ]\n  }\n}"
        );
        assert_eq!(
            js_stringify_pretty(&json!({"e": 0.5, "f": 1e21, "g": -0.0, "h": "x", "i": 1.0})),
            "{\n  \"e\": 0.5,\n  \"f\": 1e+21,\n  \"g\": 0,\n  \"h\": \"x\",\n  \"i\": 1\n}"
        );
    }

    // -- migrateAuthToAuthJson ---------------------------------------------------

    #[test]
    fn auth_migration_from_oauth_json() {
        let fs = agent_fs()
            .file(
                "/home/t/.pi/agent/oauth.json",
                r#"{"openai-codex": {"refresh_token": "r", "access_token": "a"}}"#,
            );
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["openai-codex"]);
        assert_eq!(
            fs.content("/home/t/.pi/agent/auth.json").unwrap(),
            "{\n  \"openai-codex\": {\n    \"type\": \"oauth\",\n    \"refresh_token\": \"r\",\n    \"access_token\": \"a\"\n  }\n}"
        );
        assert_eq!(fs.mode("/home/t/.pi/agent/auth.json"), Some(0o600));
        // oauth.json renamed to oauth.json.migrated with identical content.
        assert!(fs.exists("/home/t/.pi/agent/oauth.json.migrated"));
        assert!(!fs.exists("/home/t/.pi/agent/oauth.json"));
    }

    #[test]
    fn auth_migration_skips_when_auth_json_exists() {
        let fs = agent_fs()
            .file("/home/t/.pi/agent/oauth.json", r#"{"p": {}}"#)
            .file("/home/t/.pi/agent/auth.json", "{}");
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert!(providers.is_empty());
        // Nothing touched.
        assert_eq!(fs.content("/home/t/.pi/agent/oauth.json").unwrap(), r#"{"p": {}}"#);
        assert_eq!(fs.content("/home/t/.pi/agent/auth.json").unwrap(), "{}");
    }

    #[test]
    fn auth_migration_from_settings_api_keys() {
        let fs = agent_fs().file(
            "/home/t/.pi/agent/settings.json",
            r#"{"theme": "dark", "apiKeys": {"openai": "sk-1", "anthropic": "sk-2"}}"#,
        );
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["openai", "anthropic"]);
        // apiKeys removed from settings.json; everything else kept in order.
        assert_eq!(
            fs.content("/home/t/.pi/agent/settings.json").unwrap(),
            "{\n  \"theme\": \"dark\"\n}"
        );
        assert_eq!(
            fs.content("/home/t/.pi/agent/auth.json").unwrap(),
            "{\n  \"openai\": {\n    \"type\": \"api_key\",\n    \"key\": \"sk-1\"\n  },\n  \"anthropic\": {\n    \"type\": \"api_key\",\n    \"key\": \"sk-2\"\n  }\n}"
        );
    }

    #[test]
    fn auth_migration_oauth_wins_over_settings() {
        let fs = agent_fs()
            .file("/home/t/.pi/agent/oauth.json", r#"{"x": {"t": 1}}"#)
            .file(
                "/home/t/.pi/agent/settings.json",
                r#"{"apiKeys": {"x": "sk", "y": "other"}}"#,
            );
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["x", "y"]);
        assert_eq!(
            fs.content("/home/t/.pi/agent/auth.json").unwrap(),
            "{\n  \"x\": {\n    \"type\": \"oauth\",\n    \"t\": 1\n  },\n  \"y\": {\n    \"type\": \"api_key\",\n    \"key\": \"other\"\n  }\n}"
        );
        // settings.json still rewritten (apiKeys removed).
        assert_eq!(fs.content("/home/t/.pi/agent/settings.json").unwrap(), "{}");
    }

    #[test]
    fn auth_migration_ignores_non_string_and_non_object_api_keys() {
        let fs = agent_fs().file(
            "/home/t/.pi/agent/settings.json",
            r#"{"apiKeys": {"ok": "sk", "num": 5, "obj": {}, "arr": []}}"#,
        );
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["ok"]);
        // apiKeys was still an object → deleted + rewritten.
        assert_eq!(fs.content("/home/t/.pi/agent/settings.json").unwrap(), "{}");
        assert_eq!(
            fs.content("/home/t/.pi/agent/auth.json").unwrap(),
            "{\n  \"ok\": {\n    \"type\": \"api_key\",\n    \"key\": \"sk\"\n  }\n}"
        );

        // A non-object apiKeys value → no rewrite at all.
        let fs = agent_fs().file(
            "/home/t/.pi/agent/settings.json",
            r#"{"apiKeys": "nope"}"#,
        );
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert!(providers.is_empty());
        assert!(!fs.exists("/home/t/.pi/agent/auth.json"));
        assert_eq!(
            fs.content("/home/t/.pi/agent/settings.json").unwrap(),
            r#"{"apiKeys": "nope"}"#
        );
    }

    #[test]
    fn auth_migration_malformed_oauth_is_skipped() {
        let fs = agent_fs()
            .file("/home/t/.pi/agent/oauth.json", "{invalid json")
            .file("/home/t/.pi/agent/settings.json", r#"{"apiKeys": {"a": "k"}}"#);
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["a"]);
        assert!(fs.exists("/home/t/.pi/agent/oauth.json")); // not renamed
        assert_eq!(
            fs.content("/home/t/.pi/agent/auth.json").unwrap(),
            "{\n  \"a\": {\n    \"type\": \"api_key\",\n    \"key\": \"k\"\n  }\n}"
        );
    }

    #[test]
    fn auth_migration_bom_is_stripped() {
        let fs = agent_fs().file(
            "/home/t/.pi/agent/oauth.json",
            "\u{feff}{\"p\": {\"k\": 1}}",
        );
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["p"]);
    }

    #[test]
    fn auth_migration_creates_agent_dir_when_missing() {
        let fs = FakeFs::new().file("/home/t/.pi/agent/oauth.json", r#"{"p": {}}"#);
        // (file() creates the parent dirs implicitly in the fake, but the
        // mkdir_recursive call is still exercised.)
        let providers = migrate_auth_to_auth_json(&env(), &fs).unwrap();
        assert_eq!(providers, vec!["p"]);
        assert!(fs.exists("/home/t/.pi/agent/auth.json"));
    }

    #[test]
    fn auth_migration_write_failure_propagates() {
        let mut fs = agent_fs().file("/home/t/.pi/agent/oauth.json", r#"{"p": {}}"#);
        fs.fail_writes = Some("EACCES");
        let err = migrate_auth_to_auth_json(&env(), &fs).unwrap_err();
        assert!(err.starts_with("EACCES"));
    }

    // -- migrateSessionsFromAgentRoot ---------------------------------------------

    fn session_file(header: &str) -> String {
        format!("{}\n{{\"type\":\"message\"}}\n", header)
    }

    #[test]
    fn sessions_migration_moves_stray_jsonl() {
        let fs = agent_fs()
            .file(
                "/home/t/.pi/agent/2024-01-01_a.jsonl",
                &session_file(r#"{"type":"session","id":"a","cwd":"/home/t/proj"}"#),
            )
            .file("/home/t/.pi/agent/keep.txt", "x");
        migrate_sessions_from_agent_root(&env(), &fs);
        assert!(!fs.exists("/home/t/.pi/agent/2024-01-01_a.jsonl"));
        assert_eq!(
            fs.content("/home/t/.pi/agent/sessions/--home-t-proj--/2024-01-01_a.jsonl")
                .unwrap(),
            session_file(r#"{"type":"session","id":"a","cwd":"/home/t/proj"}"#)
        );
        // Non-jsonl untouched.
        assert_eq!(fs.content("/home/t/.pi/agent/keep.txt").unwrap(), "x");
    }

    #[test]
    fn sessions_migration_encodes_backslashes_and_colons() {
        let fs = agent_fs().file(
            "/home/t/.pi/agent/s.jsonl",
            &session_file(r#"{"type":"session","cwd":"\\srv\\share:dir"}"#),
        );
        migrate_sessions_from_agent_root(&env(), &fs);
        // Strip ONE leading backslash, then replace \ and : with '-'.
        assert!(fs.exists("/home/t/.pi/agent/sessions/--srv-share-dir--/s.jsonl"));
    }

    #[test]
    fn sessions_migration_skips_bad_headers() {
        let cases: Vec<(&str, String)> = vec![
            ("no_header.jsonl", "\n".to_string()),
            ("badjson.jsonl", "not json\n".to_string()),
            ("not_session.jsonl", session_file(r#"{"type":"message","cwd":"/x"}"#)),
            ("no_cwd.jsonl", session_file(r#"{"type":"session"}"#)),
            ("empty_cwd.jsonl", session_file(r#"{"type":"session","cwd":""}"#)),
        ];
        let mut fs = agent_fs();
        for (name, content) in &cases {
            fs = fs.file(&format!("/home/t/.pi/agent/{}", name), content);
        }
        migrate_sessions_from_agent_root(&env(), &fs);
        for (name, _) in &cases {
            assert!(
                fs.exists(&format!("/home/t/.pi/agent/{}", name)),
                "{} should not have moved",
                name
            );
        }
    }

    #[test]
    fn sessions_migration_skips_when_target_exists() {
        let header = session_file(r#"{"type":"session","cwd":"/p"}"#);
        let fs = agent_fs()
            .file("/home/t/.pi/agent/a.jsonl", &header)
            .file("/home/t/.pi/agent/sessions/--p--/a.jsonl", "existing");
        migrate_sessions_from_agent_root(&env(), &fs);
        assert_eq!(fs.content("/home/t/.pi/agent/a.jsonl").unwrap(), header);
        assert_eq!(
            fs.content("/home/t/.pi/agent/sessions/--p--/a.jsonl").unwrap(),
            "existing"
        );
    }

    #[test]
    fn sessions_migration_noop_without_agent_dir() {
        let fs = FakeFs::new();
        migrate_sessions_from_agent_root(&env(), &fs); // must not panic
    }

    // -- migrateCommandsToPrompts ---------------------------------------------------

    #[test]
    fn commands_to_prompts_renames_and_logs() {
        let fs = FakeFs::new()
            .dir("/base/commands")
            .file("/base/commands/x.md", "hi");
        let mut logs: Vec<String> = Vec::new();
        assert!(migrate_commands_to_prompts("/base", "Global", &fs, &mut |l| logs.push(l.to_string())));
        assert_eq!(logs, vec!["Migrated Global commands/ → prompts/"]);
        assert_eq!(fs.content("/base/prompts/x.md").unwrap(), "hi");
        assert!(!fs.exists("/base/commands"));
    }

    #[test]
    fn commands_to_prompts_noop_when_prompts_exists() {
        let fs = FakeFs::new().dir("/base/commands").dir("/base/prompts");
        let mut logs: Vec<String> = Vec::new();
        assert!(!migrate_commands_to_prompts("/base", "Project", &fs, &mut |l| logs.push(l.to_string())));
        assert!(logs.is_empty());
        assert!(fs.exists("/base/commands"));
    }

    // -- keybindings migration ---------------------------------------------------

    #[test]
    fn keybindings_migration_renames_legacy_keys() {
        let raw: Map<String, Value> = serde_json::from_str(r#"{"undo": ["ctrl+z"], "app.exit": ["ctrl+d"]}"#)
            .unwrap();
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(migrated);
        // Known keys ordered by KEYBINDINGS order: tui.editor.undo comes
        // before app.exit.
        assert_eq!(
            serde_json::to_string(&Value::Object(config)).unwrap(),
            r#"{"tui.editor.undo":["ctrl+z"],"app.exit":["ctrl+d"]}"#
        );
    }

    #[test]
    fn keybindings_migration_skips_when_target_key_present() {
        // Raw has both the legacy name and the current name → legacy skipped.
        let raw: Map<String, Value> =
            serde_json::from_str(r#"{"undo": ["a"], "tui.editor.undo": ["b"]}"#).unwrap();
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(migrated);
        assert_eq!(
            serde_json::to_string(&Value::Object(config)).unwrap(),
            r#"{"tui.editor.undo":["b"]}"#
        );
    }

    #[test]
    fn keybindings_migration_orders_extras_last_sorted() {
        let raw: Map<String, Value> =
            serde_json::from_str(r#"{"zebra": 1, "$schema": "x", "app.clear": 2, "alpha": 3}"#)
                .unwrap();
        let (config, migrated) = migrate_keybindings_config(&raw);
        assert!(!migrated);
        let keys: Vec<&String> = config.keys().collect();
        // app.clear in KEYBINDINGS order, then extras sorted ($ before letters).
        assert_eq!(keys, vec!["app.clear", "$schema", "alpha", "zebra"]);
    }

    #[test]
    fn keybindings_migration_no_change_when_clean() {
        let raw: Map<String, Value> = serde_json::from_str(r#"{"app.exit": ["ctrl+d"]}"#).unwrap();
        let (_, migrated) = migrate_keybindings_config(&raw);
        assert!(!migrated);
    }

    #[test]
    fn keybindings_config_file_rewritten_with_trailing_newline() {
        let fs = agent_fs().file(
            "/home/t/.pi/agent/keybindings.json",
            r#"{"undo": ["ctrl+z"]}"#,
        );
        migrate_keybindings_config_file(&env(), &fs);
        assert_eq!(
            fs.content("/home/t/.pi/agent/keybindings.json").unwrap(),
            "{\n  \"tui.editor.undo\": [\n    \"ctrl+z\"\n  ]\n}\n"
        );
    }

    #[test]
    fn keybindings_config_file_ignores_non_object_and_malformed() {
        for content in ["[1, 2]", "\"str\"", "{bad", "null"] {
            let fs = agent_fs().file("/home/t/.pi/agent/keybindings.json", content);
            migrate_keybindings_config_file(&env(), &fs);
            assert_eq!(fs.content("/home/t/.pi/agent/keybindings.json").unwrap(), content);
        }
        // Missing file → no-op.
        let fs = agent_fs();
        migrate_keybindings_config_file(&env(), &fs);
    }

    #[test]
    fn keybindings_config_file_not_rewritten_when_not_migrated() {
        let fs = agent_fs().file(
            "/home/t/.pi/agent/keybindings.json",
            r#"{"app.exit": ["ctrl+d"]}"#,
        );
        migrate_keybindings_config_file(&env(), &fs);
        assert_eq!(
            fs.content("/home/t/.pi/agent/keybindings.json").unwrap(),
            r#"{"app.exit": ["ctrl+d"]}"#
        );
    }

    // -- migrateToolsToBin -----------------------------------------------------------

    #[test]
    fn tools_to_bin_moves_binaries() {
        let fs = agent_fs()
            .dir("/home/t/.pi/agent/tools")
            .file("/home/t/.pi/agent/tools/fd", "bin-fd")
            .file("/home/t/.pi/agent/tools/rg", "bin-rg");
        let mut logs: Vec<String> = Vec::new();
        migrate_tools_to_bin(&env(), &fs, &mut |l| logs.push(l.to_string())).unwrap();
        assert_eq!(logs, vec!["Migrated managed binaries tools/ → bin/"]);
        assert_eq!(fs.content("/home/t/.pi/agent/bin/fd").unwrap(), "bin-fd");
        assert_eq!(fs.content("/home/t/.pi/agent/bin/rg").unwrap(), "bin-rg");
        assert!(!fs.exists("/home/t/.pi/agent/tools/fd"));
    }

    #[test]
    fn tools_to_bin_removes_old_when_target_exists() {
        let fs = agent_fs()
            .dir("/home/t/.pi/agent/tools")
            .dir("/home/t/.pi/agent/bin")
            .file("/home/t/.pi/agent/tools/fd", "old")
            .file("/home/t/.pi/agent/bin/fd", "new");
        let mut logs: Vec<String> = Vec::new();
        migrate_tools_to_bin(&env(), &fs, &mut |l| logs.push(l.to_string())).unwrap();
        assert!(logs.is_empty()); // nothing moved
        assert!(!fs.exists("/home/t/.pi/agent/tools/fd"));
        assert_eq!(fs.content("/home/t/.pi/agent/bin/fd").unwrap(), "new");
    }

    #[test]
    fn tools_to_bin_noop_without_tools_dir() {
        let fs = agent_fs();
        let mut logs: Vec<String> = Vec::new();
        migrate_tools_to_bin(&env(), &fs, &mut |l| logs.push(l.to_string())).unwrap();
        assert!(logs.is_empty());
    }

    // -- checkDeprecatedExtensionDirs --------------------------------------------------

    #[test]
    fn deprecated_dirs_warnings() {
        let fs = FakeFs::new().dir("/b/hooks");
        assert_eq!(
            check_deprecated_extension_dirs("/b", "Global", &fs),
            vec!["Global hooks/ directory found. Hooks have been renamed to extensions."]
        );
        // tools/ with only fd/rg + hidden files → no warning.
        let fs = FakeFs::new()
            .dir("/b/tools")
            .file("/b/tools/fd", "x")
            .file("/b/tools/.DS_Store", "y");
        assert!(check_deprecated_extension_dirs("/b", "Project", &fs).is_empty());
        // tools/ with custom content → warning.
        let fs = FakeFs::new().dir("/b/tools").file("/b/tools/custom.sh", "x");
        assert_eq!(
            check_deprecated_extension_dirs("/b", "Project", &fs),
            vec!["Project tools/ directory contains custom tools. Custom tools have been merged into extensions."]
        );
        // Both.
        let fs = FakeFs::new().dir("/b/hooks").dir("/b/tools").file("/b/tools/c", "x");
        assert_eq!(check_deprecated_extension_dirs("/b", "Global", &fs).len(), 2);
        // Nothing.
        assert!(check_deprecated_extension_dirs("/empty", "Global", &FakeFs::new()).is_empty());
    }

    // -- showDeprecationWarnings -----------------------------------------------------

    struct NoopInput;
    impl DeprecationInput for NoopInput {
        fn wait_for_keypress(&self) {}
    }

    #[test]
    fn deprecation_warning_output_lines() {
        let mut logs: Vec<String> = Vec::new();
        show_deprecation_warnings(
            &["Global hooks/ directory found. Hooks have been renamed to extensions.".to_string()],
            &mut |l| logs.push(l.to_string()),
            &NoopInput,
        );
        assert_eq!(
            logs,
            vec![
                "Warning: Global hooks/ directory found. Hooks have been renamed to extensions.".to_string(),
                "\nMove your extensions to the extensions/ directory.".to_string(),
                format!("Migration guide: {}", MIGRATION_GUIDE_URL),
                format!("Documentation: {}", EXTENSIONS_DOC_URL),
                "\nPress any key to continue...".to_string(),
                String::new(),
            ]
        );
    }

    #[test]
    fn deprecation_warnings_empty_is_noop() {
        let mut logs: Vec<String> = Vec::new();
        show_deprecation_warnings(&[], &mut |l| logs.push(l.to_string()), &NoopInput);
        assert!(logs.is_empty());
    }

    // -- runMigrations ----------------------------------------------------------------

    #[test]
    fn run_migrations_full_scenario() {
        let fs = agent_fs()
            // auth migration inputs
            .file(
                "/home/t/.pi/agent/oauth.json",
                r#"{"prov-a": {"refresh": "r"}}"#,
            )
            .file(
                "/home/t/.pi/agent/settings.json",
                r#"{"apiKeys": {"prov-b": "sk"}, "theme": "x"}"#,
            )
            // stray session file
            .file(
                "/home/t/.pi/agent/old.jsonl",
                &session_file(r#"{"type":"session","cwd":"/work"}"#),
            )
            // tools → bin
            .file("/home/t/.pi/agent/tools/fd", "fd")
            // keybindings migration
            .file("/home/t/.pi/agent/keybindings.json", r#"{"undo": ["ctrl+z"]}"#)
            // extension dirs: global commands + project hooks
            .dir("/home/t/.pi/agent/commands")
            .file("/home/t/.pi/agent/commands/p.md", "p")
            .dir("/work/.pi/hooks")
            .dir("/work");
        let mut logs: Vec<String> = Vec::new();
        let results = run_migrations("/work", &env(), &fs, &mut |l| logs.push(l.to_string())).unwrap();

        assert_eq!(results.migrated_auth_providers, vec!["prov-a", "prov-b"]);
        assert_eq!(
            results.deprecation_warnings,
            vec!["Project hooks/ directory found. Hooks have been renamed to extensions."]
        );
        assert!(logs.contains(&"Migrated Global commands/ → prompts/".to_string()));
        assert!(logs.contains(&"Migrated managed binaries tools/ → bin/".to_string()));
        // File tree assertions.
        assert!(fs.exists("/home/t/.pi/agent/auth.json"));
        assert_eq!(fs.mode("/home/t/.pi/agent/auth.json"), Some(0o600));
        assert!(fs.exists("/home/t/.pi/agent/oauth.json.migrated"));
        assert_eq!(
            fs.content("/home/t/.pi/agent/settings.json").unwrap(),
            "{\n  \"theme\": \"x\"\n}"
        );
        assert!(fs.exists("/home/t/.pi/agent/sessions/--work--/old.jsonl"));
        assert!(fs.exists("/home/t/.pi/agent/prompts/p.md"));
        assert!(fs.exists("/home/t/.pi/agent/bin/fd"));
        assert_eq!(
            fs.content("/home/t/.pi/agent/keybindings.json").unwrap(),
            "{\n  \"tui.editor.undo\": [\n    \"ctrl+z\"\n  ]\n}\n"
        );
    }

    #[test]
    fn run_migrations_is_idempotent() {
        let fs = agent_fs()
            .file(
                "/home/t/.pi/agent/oauth.json",
                r#"{"prov-a": {"refresh": "r"}}"#,
            )
            .file("/home/t/.pi/agent/settings.json", r#"{"apiKeys": {"prov-b": "sk"}}"#)
            .file(
                "/home/t/.pi/agent/old.jsonl",
                &session_file(r#"{"type":"session","cwd":"/work"}"#),
            );
        let mut logs: Vec<String> = Vec::new();
        let first = run_migrations("/work", &env(), &fs, &mut |l| logs.push(l.to_string())).unwrap();
        let tree_after_first = fs.tree();
        let mut logs2: Vec<String> = Vec::new();
        let second = run_migrations("/work", &env(), &fs, &mut |l| logs2.push(l.to_string())).unwrap();

        assert_eq!(first.migrated_auth_providers, vec!["prov-a", "prov-b"]);
        assert!(second.migrated_auth_providers.is_empty());
        assert!(second.deprecation_warnings.is_empty());
        assert!(logs2.is_empty());
        assert_eq!(fs.tree(), tree_after_first);
    }
}
