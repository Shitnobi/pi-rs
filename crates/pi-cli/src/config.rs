//! App identity, filesystem layout, and install-method detection.
//!
//! 1:1 port of `src/config.ts` (pi-coding-agent). `config.ts` is not the user
//! settings file: it is app identity + filesystem layout + package-manager
//! detection. In the TS original the identity constants are read from the
//! package's own `package.json` at import time; the published package sets
//! `piConfig.configDir = ".pi"` (no `name` override), so the constants below
//! are the resolved values. The resolution logic itself is ported as
//! [`app_config_from_package_json`] for parity.
//!
//! All env / process / fs access goes through the [`EnvProvider`] /
//! [`FsProvider`] traits so config resolution is testable with fakes
//! (no `std::env` in library functions; see docs/decisions/G8-async-runtime.md
//! for the all-sync convention — errors are plain `String`).
//!
//! Skipped from the TS file (per docs/plans/phase3-cli.md §2.3): the Bun /
//! bundled-node detection consts (`isBunBinary` is effectively `false` here)
//! and the QuickJS / codemode-worker plumbing.




use pi_core::paths::{normalize_path, PathInputOptions};
use pi_core::text::strip_bom;
use pi_session::config::{node_dirname, node_join};
use serde::Serialize;
use serde_json::Value;

// =============================================================================
// App Config (from package.json piConfig)
// =============================================================================

/// `PACKAGE_NAME` — `pkg.name || "@earendil-works/pi-coding-agent"` (config.ts ~445).
pub const PACKAGE_NAME: &str = "@earendil-works/pi-coding-agent";
/// `APP_NAME` — `pkg.piConfig?.name || "pi"` (config.ts ~446).
pub const APP_NAME: &str = "pi";
/// `APP_TITLE` — `piConfigName ? APP_NAME : "π"` (config.ts ~447).
pub const APP_TITLE: &str = "π";
/// `CONFIG_DIR_NAME` — `pkg.piConfig?.configDir || ".pi"` (config.ts ~448).
pub const CONFIG_DIR_NAME: &str = ".pi";
/// `VERSION` — `pkg.version || "0.0.0"` (config.ts ~449). Kept in parity with
/// the TS package version.
pub const VERSION: &str = "1.3.0";

/// `ENV_AGENT_DIR` — e.g. `PI_CODING_AGENT_DIR` or `TAU_CODING_AGENT_DIR`.
pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";
/// `ENV_SESSION_DIR` — e.g. `PI_CODING_AGENT_SESSION_DIR`.
pub const ENV_SESSION_DIR: &str = "PI_CODING_AGENT_SESSION_DIR";
/// Literal env var read by `getPackageDir()` (Nix/Guix store-path override).
pub const ENV_PACKAGE_DIR: &str = "PI_PACKAGE_DIR";
/// Literal env var read by `getShareViewerUrl()`.
pub const ENV_SHARE_VIEWER_URL: &str = "PI_SHARE_VIEWER_URL";

/// The identity constants resolved from a package.json `piConfig` block
/// (TS config.ts ~437–453, evaluated once at module load).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppConfig {
    pub package_name: String,
    pub app_name: String,
    pub app_title: String,
    pub config_dir_name: String,
    pub version: String,
}

/// Resolve the app identity from a parsed `package.json` value. Mirrors the
/// TS defaults: missing `name` falls back to the npm package name, missing
/// `piConfig.name` falls back to `"pi"` (and the title to `"π"`), missing
/// `configDir` to `".pi"`, missing `version` to `"0.0.0"`.
pub fn app_config_from_package_json(pkg: &Value) -> AppConfig {
    // JS truthiness: an empty-string `piConfig.name` is falsy, so `piConfigName || "pi"`
    // falls back (and `piConfigName ? APP_NAME : "π"` picks the π title).
    let pi_config_name: Option<&str> = pkg
        .get("piConfig")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty());
    let config_dir = pkg
        .get("piConfig")
        .and_then(|c| c.get("configDir"))
        .and_then(|n| n.as_str());
    AppConfig {
        package_name: pkg
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(PACKAGE_NAME)
            .to_string(),
        app_name: pi_config_name.unwrap_or("pi").to_string(),
        app_title: match pi_config_name {
            Some(_) => pi_config_name.unwrap().to_string(),
            None => "π".to_string(),
        },
        config_dir_name: config_dir.filter(|s| !s.is_empty()).unwrap_or(".pi").to_string(),
        version: pkg
            .get("version")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("0.0.0")
            .to_string(),
    }
}

// =============================================================================
// Injected environment / filesystem
// =============================================================================

/// Process state accessor (env vars, home dir, cwd, executable dir).
///
/// `var` returns `Some("")` for a variable that is set to the empty string —
/// callers must apply JS truthiness (empty string is falsy) exactly like
/// `process.env[NAME]` reads in the TS original.
pub trait EnvProvider {
    /// `process.env[name]` — `None` when unset.
    fn var(&self, name: &str) -> Option<String>;
    /// `os.homedir()` (unix flavor: `$HOME`).
    fn home_dir(&self) -> String;
    /// `process.cwd()`.
    fn cwd(&self) -> String;
    /// Directory of `process.execPath` (stand-in for the TS `__dirname` in
    /// package-dir detection; the Rust binary has no JS module dir).
    fn current_exe_dir(&self) -> String;
}

/// Real process environment.
pub struct SystemEnv;

impl EnvProvider for SystemEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn home_dir(&self) -> String {
        std::env::var("HOME").unwrap_or_default()
    }

    fn cwd(&self) -> String {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn current_exe_dir(&self) -> String {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
            .unwrap_or_else(|| ".".to_string())
    }
}

/// Filesystem accessor. Errors are plain `String`s in Node.js style — they
/// start with the `errno` code (e.g. `"ENOENT: no such file or directory,
/// open '/x'"`) so callers can distinguish failure modes like the TS
/// `e.code === "ENOENT"` checks.
pub trait FsProvider {
    /// `existsSync(path)`.
    fn exists(&self, path: &str) -> bool;
    /// `readFileSync(path, "utf-8")`.
    fn read_file(&self, path: &str) -> Result<String, String>;
    /// `writeFileSync(path, content[, { mode }])` — `mode` applies on create
    /// only, like Node. Default flags `"w"`: create + truncate.
    fn write_file(&self, path: &str, content: &str, mode: Option<u32>) -> Result<(), String>;
    /// `renameSync(from, to)` (overwrites an existing file target).
    fn rename(&self, from: &str, to: &str) -> Result<(), String>;
    /// `mkdirSync(path, { recursive: true })`.
    fn mkdir_recursive(&self, path: &str) -> Result<(), String>;
    /// `readdirSync(path)` — entry names (files and directories), unsorted.
    fn read_dir(&self, path: &str) -> Result<Vec<String>, String>;
    /// `rmSync(path, { force: true })` — missing target is not an error.
    fn remove_file(&self, path: &str) -> Result<(), String>;
}

/// Real filesystem. Error strings are Node-style (`"ENOENT: no such file or
/// directory, open '/x'"`) so errno-code checks behave like the TS `e.code`
/// comparisons.
pub struct SystemFs;

fn errno_code_and_desc(e: &std::io::Error) -> (&'static str, &'static str) {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => ("ENOENT", "no such file or directory"),
        PermissionDenied => ("EACCES", "permission denied"),
        AlreadyExists => ("EEXIST", "file already exists"),
        _ => ("EIO", "i/o error"),
    }
}

impl FsProvider for SystemFs {
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Result<String, String> {
        std::fs::read(path)
            .map_err(|e| {
                let (code, desc) = errno_code_and_desc(&e);
                format!("{}: {}, open '{}'", code, desc, path)
            })
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    fn write_file(&self, path: &str, content: &str, mode: Option<u32>) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        if let Some(mode) = mode {
            opts.mode(mode);
        }
        let file = opts
            .open(path)
            .map_err(|e| {
                let (code, desc) = errno_code_and_desc(&e);
                format!("{}: {}, open '{}'", code, desc, path)
            })?;
        let mut file = file;
        file.write_all(content.as_bytes()).map_err(|e| {
            let (code, desc) = errno_code_and_desc(&e);
            format!("{}: {}, write '{}'", code, desc, path)
        })
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|e| {
            let (code, desc) = errno_code_and_desc(&e);
            format!("{}: {}, rename '{}' -> '{}'", code, desc, from, to)
        })
    }

    fn mkdir_recursive(&self, path: &str) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|e| {
            let (code, desc) = errno_code_and_desc(&e);
            format!("{}: {}, mkdir '{}'", code, desc, path)
        })
    }

    fn read_dir(&self, path: &str) -> Result<Vec<String>, String> {
        std::fs::read_dir(path)
            .map_err(|e| {
                let (code, desc) = errno_code_and_desc(&e);
                format!("{}: {}, scandir '{}'", code, desc, path)
            })?
            .map(|entry| {
                entry
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .map_err(|e| {
                        let (code, desc) = errno_code_and_desc(&e);
                        format!("{}: {}, scandir '{}'", code, desc, path)
                    })
            })
            .collect()
    }

    fn remove_file(&self, path: &str) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                let (code, desc) = errno_code_and_desc(&e);
                Err(format!("{}: {}, unlink '{}'", code, desc, path))
            }
        }
    }
}

/// `path.basename` (posix flavor).
pub fn node_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// `path.normalize` (posix flavor): collapse `//`, resolve `.` and `..`.
pub fn node_normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if let Some(last) = out.last() {
                    if *last != ".." {
                        out.pop();
                    }
                } else if !absolute {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if absolute {
        format!("/{}", joined)
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// `path.resolve(path)` — resolve against the process cwd when relative.
pub fn node_resolve(env: &dyn EnvProvider, path: &str) -> String {
    if path.starts_with('/') {
        node_normalize(path)
    } else {
        node_normalize(&format!("{}/{}", env.cwd(), path))
    }
}

// =============================================================================
// User Config Paths (~/.pi/agent/*)
// =============================================================================

/// `expandTildePath` — `normalizePath(path)` with the injected home dir.
pub fn expand_tilde_path(env: &dyn EnvProvider, path: &str) -> String {
    normalize_path(
        path,
        &PathInputOptions {
            home_dir: Some(env.home_dir()),
            ..Default::default()
        },
    )
}

/// `getAgentDir()` — e.g. `~/.pi/agent/`. Precedence (config.ts ~628):
/// 1. `PI_CODING_AGENT_DIR` env override (empty string is falsy → ignored),
///    tilde-expanded via `normalizePath`;
/// 2. `join(homedir(), CONFIG_DIR_NAME, "agent")`.
pub fn get_agent_dir(env: &dyn EnvProvider) -> String {
    if let Some(env_dir) = env.var(ENV_AGENT_DIR).filter(|s| !s.is_empty()) {
        return expand_tilde_path(env, &env_dir);
    }
    node_join(&env.home_dir(), &[CONFIG_DIR_NAME, "agent"])
}

/// `getCustomThemesDir()` — `~/.pi/agent/themes`.
pub fn get_custom_themes_dir(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["themes"])
}

/// `getModelsPath()` — `~/.pi/agent/models.json`.
pub fn get_models_path(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["models.json"])
}

/// `getAuthPath()` — `~/.pi/agent/auth.json`.
pub fn get_auth_path(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["auth.json"])
}

/// `getSettingsPath()` — `~/.pi/agent/settings.json`.
pub fn get_settings_path(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["settings.json"])
}

/// `getToolsDir()` — `~/.pi/agent/tools`.
pub fn get_tools_dir(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["tools"])
}

/// `getBinDir()` — `~/.pi/agent/bin` (managed fd/rg binaries).
pub fn get_bin_dir(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["bin"])
}

/// `getPromptsDir()` — `~/.pi/agent/prompts`.
pub fn get_prompts_dir(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["prompts"])
}

/// `getSessionsDir()` — `~/.pi/agent/sessions`.
pub fn get_sessions_dir(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &["sessions"])
}

/// `getDebugLogPath()` — `~/.pi/agent/pi-debug.log` (`${APP_NAME}-debug.log`).
pub fn get_debug_log_path(env: &dyn EnvProvider) -> String {
    node_join(&get_agent_dir(env), &[&format!("{}-debug.log", APP_NAME)])
}

// =============================================================================
// Share viewer
// =============================================================================

const DEFAULT_SHARE_VIEWER_URL: &str = "https://pi.dev/session/";

/// `getShareViewerUrl(gistId)` — `${PI_SHARE_VIEWER_URL || default}#${gistId}`.
pub fn get_share_viewer_url(env: &dyn EnvProvider, gist_id: &str) -> String {
    let base_url = env
        .var(ENV_SHARE_VIEWER_URL)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SHARE_VIEWER_URL.to_string());
    format!("{}#{}", base_url, gist_id)
}

// =============================================================================
// Package Asset Paths (shipped with executable)
// =============================================================================

/// `findNodePackageDir(startDir)` — walk up to the nearest directory
/// containing `package.json`; a `dist/` hit returns its parent when that also
/// has a `package.json` (build:binary places Bun metadata inside `dist/`).
pub fn find_node_package_dir(start_dir: &str, fs: &dyn FsProvider) -> String {
    let mut dir = start_dir.to_string();
    while dir != node_dirname(&dir) {
        if fs.exists(&node_join(&dir, &["package.json"])) {
            let parent = node_dirname(&dir);
            if node_basename(&dir) == "dist" && fs.exists(&node_join(&parent, &["package.json"])) {
                return parent;
            }
            return dir;
        }
        dir = node_dirname(&dir);
    }
    start_dir.to_string()
}

/// `getPackageDir()` — `PI_PACKAGE_DIR` override (normalized), else the
/// package root found from the executable directory. The Bun-binary branch
/// is unreachable in the Rust port (`isBunBinary` is always false).
pub fn get_package_dir(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    if let Some(env_dir) = env.var(ENV_PACKAGE_DIR).filter(|s| !s.is_empty()) {
        return normalize_path(
            &env_dir,
            &PathInputOptions {
                home_dir: Some(env.home_dir()),
                ..Default::default()
            },
        );
    }
    find_node_package_dir(&env.current_exe_dir(), fs)
}

/// `src` vs `dist` segment shared by the asset-dir accessors.
fn src_or_dist(package_dir: &str, fs: &dyn FsProvider) -> &'static str {
    if fs.exists(&node_join(package_dir, &["src"])) {
        "src"
    } else {
        "dist"
    }
}

/// `getThemesDir()` — `<pkg>/(src|dist)/modes/interactive/theme`.
pub fn get_themes_dir(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    let package_dir = get_package_dir(env, fs);
    node_join(&package_dir, &[src_or_dist(&package_dir, fs), "modes", "interactive", "theme"])
}

/// `getExportTemplateDir()` — `<pkg>/(src|dist)/core/export-html`.
pub fn get_export_template_dir(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    let package_dir = get_package_dir(env, fs);
    node_join(&package_dir, &[src_or_dist(&package_dir, fs), "core", "export-html"])
}

/// `getPackageJsonPath()`.
pub fn get_package_json_path(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    node_join(&get_package_dir(env, fs), &["package.json"])
}

/// `getReadmePath()` — `resolve(join(pkg, "README.md"))`.
pub fn get_readme_path(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    node_resolve(env, &node_join(&get_package_dir(env, fs), &["README.md"]))
}

/// `getDocsPath()`.
pub fn get_docs_path(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    node_resolve(env, &node_join(&get_package_dir(env, fs), &["docs"]))
}

/// `getExamplesPath()`.
pub fn get_examples_path(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    node_resolve(env, &node_join(&get_package_dir(env, fs), &["examples"]))
}

/// `getChangelogPath()`.
pub fn get_changelog_path(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    node_resolve(env, &node_join(&get_package_dir(env, fs), &["CHANGELOG.md"]))
}

/// `getInteractiveAssetsDir()` — `<pkg>/(src|dist)/modes/interactive/assets`.
pub fn get_interactive_assets_dir(env: &dyn EnvProvider, fs: &dyn FsProvider) -> String {
    let package_dir = get_package_dir(env, fs);
    node_join(&package_dir, &[src_or_dist(&package_dir, fs), "modes", "interactive", "assets"])
}

/// `getBundledInteractiveAssetPath(name)`.
pub fn get_bundled_interactive_asset_path(
    env: &dyn EnvProvider,
    fs: &dyn FsProvider,
    name: &str,
) -> String {
    node_join(&get_interactive_assets_dir(env, fs), &[name])
}

// =============================================================================
// Install Method Detection & Self-Update
// =============================================================================

/// `InstallMethod` (config.ts ~20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InstallMethod {
    #[serde(rename = "bun-binary")]
    BunBinary,
    #[serde(rename = "npm")]
    Npm,
    #[serde(rename = "pnpm")]
    Pnpm,
    #[serde(rename = "yarn")]
    Yarn,
    #[serde(rename = "bun")]
    Bun,
    #[serde(rename = "unknown")]
    Unknown,
}

impl InstallMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstallMethod::BunBinary => "bun-binary",
            InstallMethod::Npm => "npm",
            InstallMethod::Pnpm => "pnpm",
            InstallMethod::Yarn => "yarn",
            InstallMethod::Bun => "bun",
            InstallMethod::Unknown => "unknown",
        }
    }
}

/// `detectInstallMethod()` — pure path-heuristic core. `isBunBinary` and
/// `isBunRuntime` are always false in the Rust port, so the Bun branches
/// drop out. `module_dir` stands in for the TS `__dirname` and `exec_path`
/// for `process.execPath`.
pub fn detect_install_method_for_paths(module_dir: &str, exec_path: &str) -> InstallMethod {
    let resolved_path = format!("{}\0{}", module_dir, exec_path)
        .to_lowercase()
        .replace('\\', "/");
    if resolved_path.contains("/pnpm/") || resolved_path.contains("/.pnpm/") {
        return InstallMethod::Pnpm;
    }
    if resolved_path.contains("/yarn/") || resolved_path.contains("/.yarn/") {
        return InstallMethod::Yarn;
    }
    if resolved_path.contains("/install/global/node_modules/") {
        return InstallMethod::Bun;
    }
    if resolved_path.contains("/npm/") || resolved_path.contains("/node_modules/") {
        return InstallMethod::Npm;
    }
    InstallMethod::Unknown
}

/// `detectInstallMethod()` against the real process: the executable
/// directory stands in for both `__dirname` and `process.execPath` (the Rust
/// binary has no JS module dir). A cargo-installed binary has no
/// `node_modules` path components, so this returns `Unknown`.
pub fn detect_install_method(env: &dyn EnvProvider) -> InstallMethod {
    let exe_dir = env.current_exe_dir();
    detect_install_method_for_paths(&exe_dir, &exe_dir)
}

/// `SelfUpdateCommandStep` (config.ts ~24).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelfUpdateCommandStep {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
}

/// `SelfUpdateCommand` (config.ts ~29).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelfUpdateCommand {
    pub command: String,
    pub args: Vec<String>,
    pub display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<SelfUpdateCommandStep>>,
}

/// `SelfUpdatePackageTarget` — `string | { packageName, installSpec? }`.
#[derive(Debug, Clone, PartialEq)]
pub enum SelfUpdatePackageTarget {
    Spec(String),
    Package {
        package_name: String,
        install_spec: Option<String>,
    },
}

/// Result of `normalizeSelfUpdatePackageTarget`.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedTarget {
    pub package_name: String,
    pub install_spec: String,
}

/// `normalizeSelfUpdatePackageTarget` (config.ts ~36).
pub fn normalize_self_update_package_target(target: &SelfUpdatePackageTarget) -> NormalizedTarget {
    match target {
        SelfUpdatePackageTarget::Spec(s) => NormalizedTarget {
            package_name: s.clone(),
            install_spec: s.clone(),
        },
        SelfUpdatePackageTarget::Package {
            package_name,
            install_spec,
        } => NormalizedTarget {
            package_name: package_name.clone(),
            install_spec: install_spec.clone().unwrap_or_else(|| package_name.clone()),
        },
    }
}

/// `makeSelfUpdateCommand` (config.ts ~50).
fn make_self_update_command(
    install_step: SelfUpdateCommandStep,
    uninstall_step: Option<SelfUpdateCommandStep>,
) -> SelfUpdateCommand {
    match uninstall_step {
        None => SelfUpdateCommand {
            command: install_step.command,
            args: install_step.args,
            display: install_step.display,
            steps: None,
        },
        Some(uninstall) => SelfUpdateCommand {
            display: format!("{} && {}", uninstall.display, install_step.display),
            steps: Some(vec![uninstall, install_step.clone()]),
            command: install_step.command,
            args: install_step.args,
        },
    }
}

/// `makeSelfUpdateCommandStep` (config.ts ~64) — quotes args containing JS
/// `\s` whitespace when building the display string.
fn make_self_update_command_step(command: &str, args: &[&str]) -> SelfUpdateCommandStep {
    let display = std::iter::once(command.to_string())
        .chain(args.iter().map(|a| a.to_string()))
        .map(|arg| {
            if arg.chars().any(pi_core::js_string::is_js_whitespace_or_terminator) {
                format!("\"{}\"", arg)
            } else {
                arg
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    SelfUpdateCommandStep {
        command: command.to_string(),
        args: args.iter().map(|a| a.to_string().to_string()).collect(),
        display,
    }
}

/// `getInferredNpmInstall` (config.ts ~92) — pure path-shape logic
/// (unix flavor; the win32 branch of the TS is not ported).
/// Returns `(root, prefix)`.
pub fn get_inferred_npm_install(package_dir: &str) -> Option<(String, String)> {
    let parent = node_dirname(package_dir);
    let root: Option<String> = if node_basename(&parent).starts_with('@')
        && node_basename(&node_dirname(&parent)) == "node_modules"
    {
        Some(node_dirname(&parent))
    } else if node_basename(&parent) == "node_modules" {
        Some(parent.clone())
    } else {
        None
    };
    let root = root?;
    let root_parent = node_dirname(&root);
    if node_basename(&root_parent) == "lib" {
        Some((root, node_dirname(&root_parent)))
    } else {
        None
    }
}

/// `spawnProcessSync` + trim behind a trait (testable; no network of its
/// own — commands like `npm root -g` are still user-machine queries).
/// `Some` only when the process exits 0 with non-empty trimmed stdout.
pub trait CommandRunner {
    fn read_command_output(&self, command: &str, args: &[&str]) -> Option<String>;
}

/// Real subprocess runner.
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn read_command_output(&self, command: &str, args: &[&str]) -> Option<String> {
        use std::io::Read;
        let child = std::process::Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }
        let mut stdout = String::new();
        output
            .stdout
            .as_slice()
            .read_to_string(&mut stdout)
            .ok()?;
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

/// Dependencies + stubbed facts for the self-update helpers.
///
/// In the TS these functions call `detectInstallMethod`,
/// `isManagedByGlobalPackageManager` and `isSelfUpdatePathWritable`
/// internally. The npm/pnpm/yarn/bun global-root heuristics spawn package
/// managers and walk install paths that are meaningless for a Rust binary
/// distribution, so `method`, `is_managed` and `path_writable` are injected
/// here (see docs/plans/phase3-cli.md §2.3 — port the message formatting,
/// stub the process-dependent detection).
pub struct InstallContext<'a> {
    pub env: &'a dyn EnvProvider,
    pub runner: &'a dyn CommandRunner,
    pub package_dir: String,
    pub method: InstallMethod,
    /// Stubbed `isManagedByGlobalPackageManager`.
    pub is_managed: bool,
    /// Stubbed `isSelfUpdatePathWritable`.
    pub path_writable: bool,
}

/// Hand-rolled port of the pnpm global-root regex
/// `/^(.*[\/\\]global[\/\\][^\/\\]+)[\/\\]\.pnpm[\/\\]/` (greedy `.*` — the
/// last candidate wins). Returns capture group 1.
fn pnpm_global_root_match(package_dir: &str) -> Option<String> {
    let chars: Vec<char> = package_dir.chars().collect();
    let is_sep = |c: char| c == '/' || c == '\\';
    // Last ".pnpm" occurrence with separators on both sides.
    let mut candidate: Option<usize> = None;
    for i in 1..chars.len().saturating_sub(5) {
        // ".pnpm" is 5 chars; needs a separator before (i-1) and after (i+5).
        if chars[i..].starts_with(&['.', 'p', 'n', 'p', 'm'][..])
            && is_sep(chars[i - 1])
            && chars.get(i + 5).map(|&c| is_sep(c)).unwrap_or(false)
        {
            candidate = Some(i);
        }
    }
    let dot_pnpm = candidate?;
    // Walk the segment ([^\/\\]+) directly before the ".pnpm" separator.
    let seg_end = dot_pnpm - 1; // index of the separator before .pnpm
    let mut seg_start = seg_end;
    while seg_start > 0 && !is_sep(chars[seg_start - 1]) {
        seg_start -= 1;
    }
    if seg_start == seg_end {
        return None; // empty segment — regex needs [^\/\\]+ (one or more)
    }
    // "global" occupies the 6 chars immediately before the separator that
    // precedes the segment, and must itself be preceded by a separator.
    let global: Vec<char> = "global".chars().collect();
    if seg_start < global.len() + 2 {
        return None;
    }
    let g_start = seg_start - 1 - global.len();
    if chars[g_start..seg_start - 1] != global[..] || !is_sep(chars[g_start - 1]) {
        return None;
    }
    let prefix: String = chars[..seg_end].iter().collect();
    Some(prefix)
}

/// `getSelfUpdateCommandForMethod` (config.ts ~114) — takes an already
/// normalized target.
fn self_update_command_for_method(
    ctx: &InstallContext,
    installed_package_name: &str,
    target: &NormalizedTarget,
    npm_command: Option<&[String]>,
) -> Option<SelfUpdateCommand> {
    match ctx.method {
        InstallMethod::BunBinary | InstallMethod::Unknown => None,
        InstallMethod::Pnpm => {
            let match_root = if ctx
                .runner
                .read_command_output("pnpm", &["root", "-g"])
                .is_some()
            {
                None
            } else {
                pnpm_global_root_match(&ctx.package_dir)
            };
            let bin_dir_args: Vec<String> = match &match_root {
                Some(m) => {
                    let bin_dir = ctx
                        .env
                        .var("PNPM_HOME")
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| node_dirname(&node_dirname(m)));
                    vec![format!("--config.global-bin-dir={}", bin_dir)]
                }
                None => vec![],
            };
            let bin_dir_refs: Vec<&str> = bin_dir_args.iter().map(|s| s.as_str()).collect();
            let mut install_args: Vec<&str> =
                vec!["install", "-g", "--ignore-scripts", "--config.minimumReleaseAge=0"];
            install_args.extend(bin_dir_refs.iter().copied());
            install_args.push(&target.install_spec);
            let uninstall_step = if target.package_name == installed_package_name {
                None
            } else {
                let mut args: Vec<&str> = vec!["remove", "-g"];
                args.extend(bin_dir_refs.iter().copied());
                args.push(installed_package_name);
                Some(make_self_update_command_step("pnpm", &args))
            };
            Some(make_self_update_command(
                make_self_update_command_step("pnpm", &install_args),
                uninstall_step,
            ))
        }
        InstallMethod::Yarn => {
            let install =
                make_self_update_command_step("yarn", &["global", "add", "--ignore-scripts", &target.install_spec]);
            let uninstall = if target.package_name == installed_package_name {
                None
            } else {
                Some(make_self_update_command_step("yarn", &["global", "remove", installed_package_name]))
            };
            Some(make_self_update_command(install, uninstall))
        }
        InstallMethod::Bun => {
            let install = make_self_update_command_step(
                "bun",
                &["install", "-g", "--ignore-scripts", "--minimum-release-age=0", &target.install_spec],
            );
            let uninstall = if target.package_name == installed_package_name {
                None
            } else {
                Some(make_self_update_command_step("bun", &["uninstall", "-g", installed_package_name]))
            };
            Some(make_self_update_command(install, uninstall))
        }
        InstallMethod::Npm => {
            let configured = npm_command.map(|c| !c.is_empty()).unwrap_or(false);
            let (command, npm_args): (String, Vec<String>) = match npm_command {
                Some(cmds) if !cmds.is_empty() => (cmds[0].clone(), cmds[1..].to_vec()),
                _ => ("npm".to_string(), vec![]),
            };
            let inferred = if configured {
                None
            } else {
                get_inferred_npm_install(&ctx.package_dir)
            };
            let mut prefix_args: Vec<String> = npm_args.clone();
            if let Some((_, prefix)) = &inferred {
                prefix_args.push("--prefix".to_string());
                prefix_args.push(prefix.clone());
            }
            let prefix_refs: Vec<&str> = prefix_args.iter().map(|s| s.as_str()).collect();
            let mut install_args: Vec<&str> = prefix_refs.iter().copied().collect();
            install_args.extend(["install", "-g", "--ignore-scripts", "--min-release-age=0"]);
            install_args.push(&target.install_spec);
            let install_step =
                make_self_update_command_step(&command, &install_args);
            let uninstall_step = if target.package_name == installed_package_name {
                None
            } else {
                let mut args: Vec<&str> = prefix_refs.iter().copied().collect();
                args.extend(["uninstall", "-g"]);
                args.push(installed_package_name);
                Some(make_self_update_command_step(&command, &args))
            };
            Some(make_self_update_command(install_step, uninstall_step))
        }
    }
}

/// `getSelfUpdateCommandForMethod` (public surface — normalizes the target).
pub fn get_self_update_command_for_method(
    ctx: &InstallContext,
    installed_package_name: &str,
    update_package_target: &SelfUpdatePackageTarget,
    npm_command: Option<&[String]>,
) -> Option<SelfUpdateCommand> {
    let target = normalize_self_update_package_target(update_package_target);
    self_update_command_for_method(ctx, installed_package_name, &target, npm_command)
}

/// `getSelfUpdateCommand` (config.ts ~253). The managed/writable checks are
/// injected via [`InstallContext`] (stubbed detection, see its docs).
pub fn get_self_update_command(
    ctx: &InstallContext,
    package_name: &str,
    npm_command: Option<&[String]>,
    update_package_target: &SelfUpdatePackageTarget,
) -> Option<SelfUpdateCommand> {
    let command = self_update_command_for_method(
        ctx,
        package_name,
        &normalize_self_update_package_target(update_package_target),
        npm_command,
    )?;
    if !ctx.is_managed || !ctx.path_writable {
        return None;
    }
    Some(command)
}

/// `getSelfUpdateUnavailableInstruction` (config.ts ~265).
pub fn get_self_update_unavailable_instruction(
    ctx: &InstallContext,
    package_name: &str,
    npm_command: Option<&[String]>,
    update_package_target: &SelfUpdatePackageTarget,
) -> String {
    if ctx.method == InstallMethod::BunBinary {
        return "Download from: https://github.com/earendil-works/pi/releases/latest".to_string();
    }
    let target = normalize_self_update_package_target(update_package_target);
    if let Some(command) = self_update_command_for_method(ctx, package_name, &target, npm_command) {
        if ctx.is_managed && !ctx.path_writable {
            return format!(
                "This installation is managed by a global {} install, but the install path is not writable. Update it yourself with: {}",
                ctx.method.as_str(),
                command.display
            );
        }
        return format!(
            "This installation is not managed by a global {} install. Update it with the package manager, wrapper, or source checkout that provides it.",
            ctx.method.as_str()
        );
    }
    format!(
        "Update {} using the package manager, wrapper, or source checkout that provides this installation.",
        target.install_spec
    )
}

/// `getUpdateInstruction` (config.ts ~289).
pub fn get_update_instruction(ctx: &InstallContext, package_name: &str) -> String {
    let command = self_update_command_for_method(
        ctx,
        package_name,
        &normalize_self_update_package_target(&SelfUpdatePackageTarget::Spec(package_name.to_string())),
        None,
    );
    if let Some(command) = command {
        return format!("Run: {}", command.display);
    }
    get_self_update_unavailable_instruction(
        ctx,
        package_name,
        None,
        &SelfUpdatePackageTarget::Spec(package_name.to_string()),
    )
}

// =============================================================================
// Install change detection
// =============================================================================

/// `InstallChange` (config.ts ~375).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum InstallChange {
    #[serde(rename = "updated")]
    Updated { version: Value },
    #[serde(rename = "removed")]
    Removed,
}

/// JS truthiness for a JSON value (`installed.version && ...`).
fn json_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `detectInstallChange` (config.ts ~383). The Bun-binary short-circuit is
/// unreachable here (embedded code never changes); `package_json_path`
/// stands in for the TS module-load-captured `startupPackageJsonPath`.
/// A read error whose errno code is `ENOENT` yields `Removed`; any other
/// read or parse error yields `None` (the TS `catch` returns `undefined`).
pub fn detect_install_change(fs: &dyn FsProvider, package_json_path: Option<&str>) -> Option<InstallChange> {
    let path = package_json_path?;
    let content = match fs.read_file(path) {
        Ok(content) => content,
        Err(e) => {
            return if e.starts_with("ENOENT") {
                Some(InstallChange::Removed)
            } else {
                None
            }
        }
    };
    let installed: Value = serde_json::from_str(strip_bom(&content)).ok()?;
    let version = installed.get("version");
    if !json_truthy(version) {
        return None;
    }
    if version != Some(&Value::String(VERSION.to_string())) {
        return Some(InstallChange::Updated {
            version: version.cloned()?,
        });
    }
    None
}

// =============================================================================
// Test fakes (in-memory env + fs)
// =============================================================================

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    /// In-memory [`EnvProvider`].
    #[derive(Debug, Clone)]
    pub(crate) struct FakeEnv {
        pub vars: HashMap<String, String>,
        pub home: String,
        pub cwd: String,
        pub exe_dir: String,
    }

    impl FakeEnv {
        pub(crate) fn new(home: &str) -> Self {
            FakeEnv {
                vars: HashMap::new(),
                home: home.to_string(),
                cwd: "/".to_string(),
                exe_dir: "/usr/local/bin".to_string(),
            }
        }

        pub(crate) fn with_var(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.to_string(), value.to_string());
            self
        }
    }

    impl EnvProvider for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }

        fn home_dir(&self) -> String {
            self.home.clone()
        }

        fn cwd(&self) -> String {
            self.cwd.clone()
        }

        fn current_exe_dir(&self) -> String {
            self.exe_dir.clone()
        }
    }

    /// Node-style error strings.
    fn errno_description(code: &str) -> &'static str {
        match code {
            "ENOENT" => "no such file or directory",
            "EEXIST" => "file already exists",
            "EISDIR" => "illegal operation on a directory",
            "ENOTDIR" => "not a directory",
            "EACCES" | "EPERM" => "permission denied",
            _ => "unknown error",
        }
    }

    fn node_error(code: &str, syscall: &str, path: &str) -> String {
        format!("{}: {}, {} '{}'", code, errno_description(code), syscall, path)
    }

    fn node_rename_error(code: &str, from: &str, to: &str) -> String {
        format!(
            "{}: {}, rename '{}' -> '{}'",
            code,
            errno_description(code),
            from,
            to
        )
    }

    /// In-memory [`FsProvider`] — flat path-keyed file map plus a directory
    /// set, behind `RefCell` (the trait is `&self`). `read_dir` returns
    /// sorted names for determinism (the real fs returns OS order, like
    /// Node's `readdirSync`).
    #[derive(Debug, Clone, Default)]
    pub(crate) struct FakeFs {
        files: RefCell<HashMap<String, String>>,
        dirs: RefCell<HashSet<String>>,
        modes: RefCell<HashMap<String, u32>>,
        /// When set, every `write_file` fails with this errno code.
        pub fail_writes: Option<&'static str>,
    }

    impl FakeFs {
        pub(crate) fn new() -> Self {
            FakeFs::default()
        }

        pub(crate) fn dir(self, path: &str) -> Self {
            self.add_dir_rec(path);
            self
        }

        pub(crate) fn file(self, path: &str, content: &str) -> Self {
            self.add_dir_rec(node_dirname(path).trim_end_matches('/'));
            self.files
                .borrow_mut()
                .insert(path.to_string(), content.to_string());
            self
        }

        pub(crate) fn add_dir_rec(&self, path: &str) {
            let mut dir = path.trim_end_matches('/').to_string();
            if dir.is_empty() {
                return;
            }
            loop {
                self.dirs.borrow_mut().insert(dir.clone());
                let parent = node_dirname(&dir);
                if parent == dir || parent == "." {
                    break;
                }
                dir = parent;
            }
            if path.starts_with('/') {
                self.dirs.borrow_mut().insert("/".to_string());
            }
        }

        fn parent_exists(&self, path: &str) -> bool {
            let parent = node_dirname(path);
            parent == "." || parent == "/" || self.dirs.borrow().contains(&parent) || self.has_children(&parent)
        }

        fn has_children(&self, dir: &str) -> bool {
            let prefix = format!("{}/", dir.trim_end_matches('/'));
            self.files.borrow().keys().any(|p| p.starts_with(&prefix))
                || self.dirs.borrow().iter().any(|p| p.starts_with(&prefix))
        }

        /// File content snapshot (for assertions).
        pub(crate) fn content(&self, path: &str) -> Option<String> {
            self.files.borrow().get(path).cloned()
        }

        /// Recorded creation mode for a file (for 0600 assertions).
        pub(crate) fn mode(&self, path: &str) -> Option<u32> {
            self.modes.borrow().get(path).copied()
        }

        /// Files+dirs snapshot (sorted) — for tree-comparison assertions.
        pub(crate) fn tree(&self) -> Vec<String> {
            let files = self.files.borrow();
            let mut out: Vec<String> = files
                .keys()
                .map(|p| format!("{} = {}", p, files[p]))
                .chain(self.dirs.borrow().iter().cloned())
                .collect();
            out.sort();
            out
        }
    }

    impl FsProvider for FakeFs {
        fn exists(&self, path: &str) -> bool {
            self.files.borrow().contains_key(path) || self.dirs.borrow().contains(path)
        }

        fn read_file(&self, path: &str) -> Result<String, String> {
            match self.files.borrow().get(path) {
                Some(content) => Ok(content.clone()),
                None => {
                    if self.dirs.borrow().contains(path) {
                        Err(node_error("EISDIR", "read", path))
                    } else {
                        Err(node_error("ENOENT", "open", path))
                    }
                }
            }
        }

        fn write_file(&self, path: &str, content: &str, mode: Option<u32>) -> Result<(), String> {
            if let Some(code) = self.fail_writes {
                return Err(node_error(code, "open", path));
            }
            if self.dirs.borrow().contains(path) {
                return Err(node_error("EISDIR", "open", path));
            }
            if !self.parent_exists(path) {
                return Err(node_error("ENOENT", "open", path));
            }
            self.files
                .borrow_mut()
                .insert(path.to_string(), content.to_string());
            if let Some(mode) = mode {
                self.modes.borrow_mut().insert(path.to_string(), mode);
            }
            Ok(())
        }

        fn rename(&self, from: &str, to: &str) -> Result<(), String> {
            if !self.exists(from) {
                return Err(node_rename_error("ENOENT", from, to));
            }
            if self.dirs.borrow().contains(to) {
                return Err(node_rename_error("EPERM", from, to));
            }
            if self.dirs.borrow().contains(from) {
                // Directory rename: move every child path.
                let prefix = format!("{}/", from.trim_end_matches('/'));
                let child_files: Vec<String> = self
                    .files
                    .borrow()
                    .keys()
                    .filter(|p| p.starts_with(&prefix))
                    .cloned()
                    .collect();
                for child in child_files {
                    let new_path = format!("{}{}", to, &child[prefix.len() - 1..]);
                    let content = self.files.borrow()[&child].clone();
                    self.files.borrow_mut().insert(new_path, content);
                    self.files.borrow_mut().remove(&child);
                }
                let child_dirs: Vec<String> = self
                    .dirs
                    .borrow()
                    .iter()
                    .filter(|p| p.starts_with(&prefix))
                    .cloned()
                    .collect();
                for child in child_dirs {
                    let new_path = format!("{}{}", to, &child[prefix.len() - 1..]);
                    self.dirs.borrow_mut().insert(new_path);
                    self.dirs.borrow_mut().remove(&child);
                }
                self.dirs.borrow_mut().insert(to.to_string());
                self.dirs.borrow_mut().remove(from);
            } else {
                let content = self.files.borrow().get(from).cloned();
                if let Some(content) = content {
                    self.files.borrow_mut().insert(to.to_string(), content);
                    self.files.borrow_mut().remove(from);
                    if let Some(mode) = self.modes.borrow_mut().remove(from) {
                        self.modes.borrow_mut().insert(to.to_string(), mode);
                    }
                }
            }
            Ok(())
        }

        fn mkdir_recursive(&self, path: &str) -> Result<(), String> {
            self.add_dir_rec(path);
            Ok(())
        }

        fn read_dir(&self, path: &str) -> Result<Vec<String>, String> {
            if !self.dirs.borrow().contains(path) && !self.has_children(path) {
                return Err(node_error("ENOENT", "scandir", path));
            }
            let prefix = format!("{}/", path.trim_end_matches('/'));
            let files = self.files.borrow();
            let dirs = self.dirs.borrow();
            let mut names: Vec<String> = files
                .keys()
                .filter(|p| p.starts_with(&prefix) && !p[prefix.len()..].contains('/'))
                .map(|p| p[prefix.len()..].to_string())
                .chain(
                    dirs.iter()
                        .filter(|p| p.starts_with(&prefix) && !p[prefix.len()..].contains('/'))
                        .map(|p| p[prefix.len()..].to_string()),
                )
                .collect();
            names.sort();
            names.dedup();
            Ok(names)
        }

        fn remove_file(&self, path: &str) -> Result<(), String> {
            if self.dirs.borrow().contains(path) {
                return Err(node_error("EISDIR", "unlink", path));
            }
            self.files.borrow_mut().remove(path);
            self.modes.borrow_mut().remove(path);
            Ok(())
        }
    }

    /// [`CommandRunner`] fake with canned outputs.
    #[derive(Default)]
    pub(crate) struct FakeRunner {
        pub outputs: HashMap<String, Option<String>>,
    }

    impl FakeRunner {
        pub(crate) fn set(&mut self, command: &str, args: &[&str], output: Option<&str>) {
            let key = format!("{} {}", command, args.join(" "));
            self.outputs
                .insert(key, output.map(|s| s.to_string()));
        }
    }

    impl CommandRunner for FakeRunner {
        fn read_command_output(&self, command: &str, args: &[&str]) -> Option<String> {
            let key = format!("{} {}", command, args.join(" "));
            self.outputs.get(&key).cloned().flatten()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::{FakeEnv, FakeFs, FakeRunner};
    use super::*;

    // -- app config ---------------------------------------------------------

    #[test]
    fn app_config_from_published_package_json() {
        let pkg: Value = serde_json::from_str(
            r#"{"name":"@earendil-works/pi-coding-agent","version":"1.3.0","piConfig":{"configDir":".pi"}}"#,
        )
        .unwrap();
        let cfg = app_config_from_package_json(&pkg);
        assert_eq!(cfg.package_name, "@earendil-works/pi-coding-agent");
        assert_eq!(cfg.app_name, "pi");
        assert_eq!(cfg.app_title, "π");
        assert_eq!(cfg.config_dir_name, ".pi");
        assert_eq!(cfg.version, "1.3.0");
    }

    #[test]
    fn app_config_defaults_on_empty_package_json() {
        let cfg = app_config_from_package_json(&serde_json::json!({}));
        assert_eq!(cfg.package_name, PACKAGE_NAME);
        assert_eq!(cfg.app_name, "pi");
        assert_eq!(cfg.app_title, "π");
        assert_eq!(cfg.config_dir_name, ".pi");
        assert_eq!(cfg.version, "0.0.0");
    }

    #[test]
    fn app_config_pi_config_name_override() {
        let cfg = app_config_from_package_json(&serde_json::json!({
            "name": "x", "version": "9.9.9",
            "piConfig": {"name": "tau", "configDir": ".tau"}
        }));
        assert_eq!(cfg.app_name, "tau");
        assert_eq!(cfg.app_title, "tau");
        assert_eq!(cfg.config_dir_name, ".tau");
    }

    #[test]
    fn app_config_empty_strings_fall_back_to_defaults() {
        // JS `||` treats "" as missing.
        let cfg = app_config_from_package_json(&serde_json::json!({
            "name": "", "version": "", "piConfig": {"name": "", "configDir": ""}
        }));
        assert_eq!(cfg.package_name, PACKAGE_NAME);
        assert_eq!(cfg.app_name, "pi");
        assert_eq!(cfg.config_dir_name, ".pi");
        assert_eq!(cfg.version, "0.0.0");
    }

    // -- agent dir resolution ------------------------------------------------

    #[test]
    fn get_agent_dir_default() {
        let env = FakeEnv::new("/home/tester");
        assert_eq!(get_agent_dir(&env), "/home/tester/.pi/agent");
    }

    #[test]
    fn get_agent_dir_env_override() {
        let env = FakeEnv::new("/home/tester").with_var(ENV_AGENT_DIR, "/custom/dir");
        assert_eq!(get_agent_dir(&env), "/custom/dir");
    }

    #[test]
    fn get_agent_dir_env_override_with_trailing_slash_is_normalized() {
        // join in later steps handles it, but expandTildePath/normalizePath
        // keeps the trailing slash (Node normalizePath does not strip it).
        let env = FakeEnv::new("/home/tester").with_var(ENV_AGENT_DIR, "/custom/dir/");
        assert_eq!(get_agent_dir(&env), "/custom/dir/");
    }

    #[test]
    fn get_agent_dir_env_override_tilde() {
        let env = FakeEnv::new("/home/tester").with_var(ENV_AGENT_DIR, "~/agentdir");
        assert_eq!(get_agent_dir(&env), "/home/tester/agentdir");
    }

    #[test]
    fn get_agent_dir_empty_env_var_is_falsy() {
        let env = FakeEnv::new("/home/tester").with_var(ENV_AGENT_DIR, "");
        assert_eq!(get_agent_dir(&env), "/home/tester/.pi/agent");
    }

    #[test]
    fn derived_paths() {
        let env = FakeEnv::new("/home/tester");
        assert_eq!(get_custom_themes_dir(&env), "/home/tester/.pi/agent/themes");
        assert_eq!(get_models_path(&env), "/home/tester/.pi/agent/models.json");
        assert_eq!(get_auth_path(&env), "/home/tester/.pi/agent/auth.json");
        assert_eq!(get_settings_path(&env), "/home/tester/.pi/agent/settings.json");
        assert_eq!(get_tools_dir(&env), "/home/tester/.pi/agent/tools");
        assert_eq!(get_bin_dir(&env), "/home/tester/.pi/agent/bin");
        assert_eq!(get_prompts_dir(&env), "/home/tester/.pi/agent/prompts");
        assert_eq!(get_sessions_dir(&env), "/home/tester/.pi/agent/sessions");
        assert_eq!(get_debug_log_path(&env), "/home/tester/.pi/agent/pi-debug.log");
    }

    #[test]
    fn expand_tilde_path_cases() {
        let env = FakeEnv::new("/home/tester");
        assert_eq!(expand_tilde_path(&env, "~"), "/home/tester");
        assert_eq!(expand_tilde_path(&env, "~/x"), "/home/tester/x");
        assert_eq!(expand_tilde_path(&env, "/abs/path"), "/abs/path");
        assert_eq!(expand_tilde_path(&env, "relative"), "relative");
        assert_eq!(expand_tilde_path(&env, "~notilde"), "~notilde");
    }

    // -- share viewer ---------------------------------------------------------

    #[test]
    fn share_viewer_url_default_and_override() {
        let env = FakeEnv::new("/h");
        assert_eq!(
            get_share_viewer_url(&env, "abc123"),
            "https://pi.dev/session/#abc123"
        );
        let env = FakeEnv::new("/h").with_var(ENV_SHARE_VIEWER_URL, "http://localhost:8080/v/");
        assert_eq!(
            get_share_viewer_url(&env, "abc123"),
            "http://localhost:8080/v/#abc123"
        );
        let env = FakeEnv::new("/h").with_var(ENV_SHARE_VIEWER_URL, "");
        assert_eq!(
            get_share_viewer_url(&env, "abc123"),
            "https://pi.dev/session/#abc123"
        );
    }

    // -- package dir + assets --------------------------------------------------

    #[test]
    fn find_node_package_dir_walks_up() {
        let fs = FakeFs::new().dir("/work/app/src").dir("/work/app");
        // No package.json anywhere: returns start dir.
        assert_eq!(find_node_package_dir("/work/app/src", &fs), "/work/app/src");
        let fs = fs.file("/work/package.json", "{}");
        assert_eq!(find_node_package_dir("/work/app/src", &fs), "/work");
    }

    #[test]
    fn find_node_package_dir_dist_prefers_parent() {
        let fs = FakeFs::new()
            .file("/pkg/dist/package.json", "{}")
            .file("/pkg/package.json", "{}");
        assert_eq!(find_node_package_dir("/pkg/dist", &fs), "/pkg");
        // Without a parent package.json the dist dir itself wins.
        let fs = FakeFs::new().file("/pkg/dist/package.json", "{}");
        assert_eq!(find_node_package_dir("/pkg/dist", &fs), "/pkg/dist");
    }

    #[test]
    fn get_package_dir_env_override() {
        let env = FakeEnv::new("/home/tester")
            .with_var(ENV_PACKAGE_DIR, "/nix/store/pi-pkg")
            .clone();
        let fs = FakeFs::new();
        assert_eq!(get_package_dir(&env, &fs), "/nix/store/pi-pkg");
        // Tilde expansion applies to the override.
        let env = FakeEnv::new("/home/tester").with_var(ENV_PACKAGE_DIR, "~/pkg");
        assert_eq!(get_package_dir(&env, &fs), "/home/tester/pkg");
        // Empty override is falsy → exe-dir walk.
        let mut env = FakeEnv::new("/home/tester");
        env.vars.insert(ENV_PACKAGE_DIR.to_string(), String::new());
        let fs = FakeFs::new().file("/usr/local/package.json", "{}");
        assert_eq!(get_package_dir(&env, &fs), "/usr/local");
    }

    #[test]
    fn asset_paths_use_src_or_dist() {
        // Walk up from the exe dir to the package root; src/ present → src.
        let mut env = FakeEnv::new("/h");
        env.exe_dir = "/pkg/bin".to_string();
        let fs = FakeFs::new().file("/pkg/package.json", "{}").dir("/pkg/src");
        assert_eq!(get_themes_dir(&env, &fs), "/pkg/src/modes/interactive/theme");
        assert_eq!(
            get_export_template_dir(&env, &fs),
            "/pkg/src/core/export-html"
        );
        assert_eq!(
            get_interactive_assets_dir(&env, &fs),
            "/pkg/src/modes/interactive/assets"
        );
        assert_eq!(
            get_bundled_interactive_asset_path(&env, &fs, "logo.png"),
            "/pkg/src/modes/interactive/assets/logo.png"
        );
        // No src/ → dist/.
        let fs = FakeFs::new().file("/pkg/package.json", "{}");
        assert_eq!(get_themes_dir(&env, &fs), "/pkg/dist/modes/interactive/theme");
        assert_eq!(get_package_json_path(&env, &fs), "/pkg/package.json");
        assert_eq!(get_readme_path(&env, &fs), "/pkg/README.md");
        assert_eq!(get_docs_path(&env, &fs), "/pkg/docs");
        assert_eq!(get_examples_path(&env, &fs), "/pkg/examples");
        assert_eq!(get_changelog_path(&env, &fs), "/pkg/CHANGELOG.md");
    }

    // -- node path helpers ------------------------------------------------------

    #[test]
    fn node_basename_and_normalize() {
        assert_eq!(node_basename("/a/b/c.txt"), "c.txt");
        assert_eq!(node_basename("/a/b/"), "b");
        assert_eq!(node_basename("/"), "/");
        assert_eq!(node_normalize("/a/./b/../c"), "/a/c");
        assert_eq!(node_normalize("a//b"), "a/b");
        assert_eq!(node_normalize("a/.."), ".");
        assert_eq!(node_normalize("/.."), "/");
    }

    #[test]
    fn node_resolve_relative_uses_cwd() {
        let mut env = FakeEnv::new("/home/tester");
        env.cwd = "/work".to_string();
        assert_eq!(node_resolve(&env, "x/y"), "/work/x/y");
        assert_eq!(node_resolve(&env, "/abs"), "/abs");
    }

    // -- install method -----------------------------------------------------------

    #[test]
    fn detect_install_method_matrix() {
        use InstallMethod::*;
        assert_eq!(detect_install_method_for_paths("/u/.pnpm/pi@1/node_modules/pi", "/x"), Pnpm);
        assert_eq!(detect_install_method_for_paths("/u/pnpm/global", "/x"), Pnpm);
        assert_eq!(detect_install_method_for_paths("/u/.yarn/global", "/x"), Yarn);
        assert_eq!(detect_install_method_for_paths("/u/yarn/link", "/x"), Yarn);
        assert_eq!(
            detect_install_method_for_paths("/x", "/home/u/.bun/install/global/node_modules/pi"),
            Bun
        );
        assert_eq!(detect_install_method_for_paths("/usr/lib/node_modules/pi", "/x"), Npm);
        assert_eq!(detect_install_method_for_paths("/npm/global", "/x"), Npm);
        assert_eq!(detect_install_method_for_paths("/usr/local/bin", "/usr/local/bin"), Unknown);
        // Backslashes are normalized to slashes first.
        assert_eq!(detect_install_method_for_paths("C:\\pnpm\\global", "C:\\x"), Pnpm);
        // Case-insensitive.
        assert_eq!(detect_install_method_for_paths("/PNPM/global", "/x"), Pnpm);
    }

    #[test]
    fn make_self_update_command_step_quotes_whitespace_args() {
        let step = make_self_update_command_step("pnpm", &["install", "a b", "plain"]);
        assert_eq!(step.args, vec!["install", "a b", "plain"]);
        assert_eq!(step.display, "pnpm install \"a b\" plain");
        let step = make_self_update_command_step("npm", &[]);
        assert_eq!(step.display, "npm");
    }

    #[test]
    fn make_self_update_command_combines_steps() {
        let install = make_self_update_command_step("yarn", &["global", "add", "pi"]);
        let uninstall = make_self_update_command_step("yarn", &["global", "remove", "old"]);
        let combined = make_self_update_command(install.clone(), Some(uninstall.clone()));
        assert_eq!(combined.display, "yarn global remove old && yarn global add pi");
        assert_eq!(combined.steps, Some(vec![uninstall, install]));
        let single = make_self_update_command(
            make_self_update_command_step("yarn", &["global", "add", "pi"]),
            None,
        );
        assert_eq!(single.display, "yarn global add pi");
        assert!(single.steps.is_none());
    }

    #[test]
    fn get_inferred_npm_install_cases() {
        // Scoped package: <prefix>/lib/node_modules/@scope/pkg → root is the
        // node_modules dir (dirname of the @scope dir), like the TS.
        assert_eq!(
            get_inferred_npm_install("/usr/lib/node_modules/@scope/pi"),
            Some(("/usr/lib/node_modules".to_string(), "/usr".to_string()))
        );
        assert_eq!(
            get_inferred_npm_install("/usr/lib/node_modules/pi"),
            Some(("/usr/lib/node_modules".to_string(), "/usr".to_string()))
        );
        // No node_modules parent → None.
        assert_eq!(get_inferred_npm_install("/usr/local/pi"), None);
        // node_modules but no lib grandparent → None.
        assert_eq!(get_inferred_npm_install("/data/node_modules/pi"), None);
    }

    #[test]
    fn pnpm_global_root_match_cases() {
        assert_eq!(
            pnpm_global_root_match("/home/u/.local/share/pnpm/global/abc123/.pnpm/pi@1.0/node_modules/pi"),
            Some("/home/u/.local/share/pnpm/global/abc123".to_string())
        );
        assert_eq!(pnpm_global_root_match("/home/u/.pnpm/pi"), None);
        assert_eq!(pnpm_global_root_match("/x/global/.pnpm/y"), None); // empty segment
        assert_eq!(pnpm_global_root_match("/x/other/dir/.pnpm/y"), None);
    }

    fn ctx<'a>(
        env: &'a FakeEnv,
        runner: &'a FakeRunner,
        package_dir: &str,
        method: InstallMethod,
    ) -> InstallContext<'a> {
        InstallContext {
            env,
            runner,
            package_dir: package_dir.to_string(),
            method,
            is_managed: true,
            path_writable: true,
        }
    }

    #[test]
    fn self_update_command_yarn() {
        let env = FakeEnv::new("/h");
        let runner = FakeRunner::default();
        let c = ctx(&env, &runner, "/yarn/global", InstallMethod::Yarn);
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            None,
        )
        .unwrap();
        assert_eq!(cmd.command, "yarn");
        assert_eq!(cmd.args, vec!["global", "add", "--ignore-scripts", "pi"]);
        assert_eq!(cmd.display, "yarn global add --ignore-scripts pi");
        // Different installed package name → uninstall step first
        // (TS: `target.packageName === installedPackageName ? undefined : remove`).
        let cmd = get_self_update_command_for_method(
            &c,
            "@earendil-works/pi-coding-agent",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            cmd.display,
            "yarn global remove @earendil-works/pi-coding-agent && yarn global add --ignore-scripts pi"
        );
    }

    #[test]
    fn self_update_command_npm_with_prefix() {
        let env = FakeEnv::new("/h");
        let runner = FakeRunner::default();
        let c = ctx(&env, &runner, "/usr/lib/node_modules/pi", InstallMethod::Npm);
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            cmd.args,
            vec!["--prefix", "/usr", "install", "-g", "--ignore-scripts", "--min-release-age=0", "pi"]
        );
        // Configured npmCommand suppresses the inference.
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            Some(&["npm".to_string(), "--registry=x".to_string()]),
        )
        .unwrap();
        assert_eq!(
            cmd.args,
            vec!["--registry=x", "install", "-g", "--ignore-scripts", "--min-release-age=0", "pi"]
        );
        // bun-as-npm wrapper uses the first element as the command.
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            Some(&["bun".to_string()]),
        )
        .unwrap();
        assert_eq!(cmd.command, "bun");
        assert_eq!(cmd.args[0], "install");
    }

    #[test]
    fn self_update_command_pnpm_bin_dir_from_env() {
        let env = FakeEnv::new("/h").with_var("PNPM_HOME", "/home/pnpm-bin");
        let mut runner = FakeRunner::default();
        // `pnpm root -g` failing (None) falls through to the path heuristic.
        runner.set("pnpm", &["root", "-g"], None);
        let c = ctx(
            &env,
            &runner,
            "/home/u/.local/share/pnpm/global/abc/.pnpm/pi@1/node_modules/pi",
            InstallMethod::Pnpm,
        );
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            cmd.args,
            vec![
                "install",
                "-g",
                "--ignore-scripts",
                "--config.minimumReleaseAge=0",
                "--config.global-bin-dir=/home/pnpm-bin",
                "pi"
            ]
        );
        // Without PNPM_HOME the bin dir is derived from the match.
        let env = FakeEnv::new("/h");
        let c = ctx(
            &env,
            &runner,
            "/home/u/.local/share/pnpm/global/abc/.pnpm/pi@1/node_modules/pi",
            InstallMethod::Pnpm,
        );
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            None,
        )
        .unwrap();
        assert!(cmd.args.contains(&"--config.global-bin-dir=/home/u/.local/share/pnpm".to_string()));
        // `pnpm root -g` succeeding → no bin-dir args.
        let mut runner = FakeRunner::default();
        runner.set("pnpm", &["root", "-g"], Some("/some/root"));
        let c = ctx(&env, &runner, "/x", InstallMethod::Pnpm);
        let cmd = get_self_update_command_for_method(
            &c,
            "pi",
            &SelfUpdatePackageTarget::Spec("pi".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            cmd.args,
            vec!["install", "-g", "--ignore-scripts", "--config.minimumReleaseAge=0", "pi"]
        );
    }

    #[test]
    fn self_update_command_unknown_and_bun_binary_are_none() {
        let env = FakeEnv::new("/h");
        let runner = FakeRunner::default();
        for method in [InstallMethod::Unknown, InstallMethod::BunBinary] {
            let c = ctx(&env, &runner, "/x", method);
            assert!(get_self_update_command_for_method(
                &c,
                "pi",
                &SelfUpdatePackageTarget::Spec("pi".into()),
                None
            )
            .is_none());
        }
    }

    #[test]
    fn get_self_update_command_gates_on_managed_and_writable() {
        let env = FakeEnv::new("/h");
        let runner = FakeRunner::default();
        let mut c = ctx(&env, &runner, "/yarn/global", InstallMethod::Yarn);
        assert!(get_self_update_command(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi".into())).is_some());
        c.is_managed = false;
        assert!(get_self_update_command(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi".into())).is_none());
        c.is_managed = true;
        c.path_writable = false;
        assert!(get_self_update_command(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi".into())).is_none());
    }

    #[test]
    fn unavailable_instruction_strings() {
        let env = FakeEnv::new("/h");
        let runner = FakeRunner::default();
        // bun-binary branch.
        let c = ctx(&env, &runner, "/x", InstallMethod::BunBinary);
        assert_eq!(
            get_self_update_unavailable_instruction(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi".into())),
            "Download from: https://github.com/earendil-works/pi/releases/latest"
        );
        // Managed but not writable.
        let mut c = ctx(&env, &runner, "/yarn/global", InstallMethod::Yarn);
        c.path_writable = false;
        assert_eq!(
            get_self_update_unavailable_instruction(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi".into())),
            "This installation is managed by a global yarn install, but the install path is not writable. Update it yourself with: yarn global add --ignore-scripts pi"
        );
        // Not managed.
        c.is_managed = false;
        c.path_writable = true;
        assert_eq!(
            get_self_update_unavailable_instruction(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi".into())),
            "This installation is not managed by a global yarn install. Update it with the package manager, wrapper, or source checkout that provides it."
        );
        // No command for method (unknown).
        let c = ctx(&env, &runner, "/x", InstallMethod::Unknown);
        assert_eq!(
            get_self_update_unavailable_instruction(&c, "pi", None, &SelfUpdatePackageTarget::Spec("pi-pkg".into())),
            "Update pi-pkg using the package manager, wrapper, or source checkout that provides this installation."
        );
    }

    #[test]
    fn update_instruction_strings() {
        let env = FakeEnv::new("/h");
        let runner = FakeRunner::default();
        let c = ctx(&env, &runner, "/yarn/global", InstallMethod::Yarn);
        assert_eq!(get_update_instruction(&c, "pi"), "Run: yarn global add --ignore-scripts pi");
        let c = ctx(&env, &runner, "/x", InstallMethod::Unknown);
        assert_eq!(
            get_update_instruction(&c, "pi"),
            "Update pi using the package manager, wrapper, or source checkout that provides this installation."
        );
    }

    // -- install change -----------------------------------------------------------

    #[test]
    fn detect_install_change_matrix() {
        let fs = FakeFs::new().file("/pkg/package.json", r#"{"version":"9.9.9"}"#);
        assert_eq!(
            detect_install_change(&fs, Some("/pkg/package.json")),
            Some(InstallChange::Updated {
                version: Value::String("9.9.9".into())
            })
        );
        let fs = FakeFs::new().file("/pkg/package.json", r#"{"version":"1.3.0"}"#);
        assert_eq!(detect_install_change(&fs, Some("/pkg/package.json")), None);
        // Missing → Removed; no path → None.
        let fs = FakeFs::new();
        assert_eq!(
            detect_install_change(&fs, Some("/pkg/package.json")),
            Some(InstallChange::Removed)
        );
        assert_eq!(detect_install_change(&fs, None), None);
        // Malformed JSON → None.
        let fs = FakeFs::new().file("/pkg/package.json", "{oops");
        assert_eq!(detect_install_change(&fs, Some("/pkg/package.json")), None);
        // BOM is stripped before parsing.
        let fs = FakeFs::new().file("/pkg/package.json", "\u{feff}{\"version\":\"2.0.0\"}");
        assert_eq!(
            detect_install_change(&fs, Some("/pkg/package.json")),
            Some(InstallChange::Updated {
                version: Value::String("2.0.0".into())
            })
        );
        // Missing/falsy version → None.
        let fs = FakeFs::new().file("/pkg/package.json", r#"{"name":"x"}"#);
        assert_eq!(detect_install_change(&fs, Some("/pkg/package.json")), None);
        let fs = FakeFs::new().file("/pkg/package.json", r#"{"version":""}"#);
        assert_eq!(detect_install_change(&fs, Some("/pkg/package.json")), None);
        // Non-string truthy version compares unequal → updated with the raw value.
        let fs = FakeFs::new().file("/pkg/package.json", r#"{"version":5}"#);
        assert_eq!(
            detect_install_change(&fs, Some("/pkg/package.json")),
            Some(InstallChange::Updated {
                version: Value::from(5)
            })
        );
    }

    #[test]
    fn detect_install_method_system_returns_unknown_for_plain_paths() {
        // exe dir without node_modules components → unknown.
        let env = FakeEnv::new("/h");
        assert_eq!(detect_install_method(&env), InstallMethod::Unknown);
    }
}
