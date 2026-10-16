//! Config-path helpers.
//!
//! Port of the user-config path section of `src/config.ts`. The original
//! reads `piConfig` from the package's `package.json`; the published pi
//! package sets only `configDir: ".pi"` (no `name` override), so the app
//! name is the `"pi"` fallback and the config dir is `~/.pi`.

use pi_core::paths::{normalize_path, PathInputOptions};

/// `APP_NAME` — `pkg.piConfig?.name || "pi"`.
pub const APP_NAME: &str = "pi";

/// `CONFIG_DIR_NAME` — `pkg.piConfig?.configDir || ".pi"`.
pub const CONFIG_DIR_NAME: &str = ".pi";

/// Name of the environment variable overriding the agent directory
/// (`${APP_NAME.toUpperCase()}_CODING_AGENT_DIR`).
pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";

/// Node `path.join` (posix flavor) for already-normalized absolute
/// components. Absolute later parts reset the result, like Node.
pub fn node_join(base: &str, parts: &[&str]) -> String {
    let mut result = base.trim_end_matches('/').to_string();
    for part in parts {
        if part.starts_with('/') {
            result = part.trim_end_matches('/').to_string();
        } else if !part.is_empty() {
            result.push('/');
            result.push_str(part);
        }
    }
    result
}

/// Node `path.dirname` for posix paths.
pub fn node_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => trimmed[..idx].to_string(),
        None => ".".to_string(),
    }
}

/// `expandTildePath` — path normalization only in the original.
pub fn expand_tilde_path(path: &str) -> String {
    normalize_path(path, &PathInputOptions::default())
}

/// `os.homedir()` (linux flavor: `$HOME`).
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// `getAgentDir()` — e.g. `~/.pi/agent/`, overridable via
/// `PI_CODING_AGENT_DIR`.
pub fn get_agent_dir() -> String {
    if let Ok(env_dir) = std::env::var(ENV_AGENT_DIR) {
        if !env_dir.is_empty() {
            return expand_tilde_path(&env_dir);
        }
    }
    node_join(&home_dir(), &[CONFIG_DIR_NAME, "agent"])
}

/// `getSessionsDir()` — `~/.pi/agent/sessions/`.
pub fn get_sessions_dir() -> String {
    node_join(&get_agent_dir(), &["sessions"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_join_semantics() {
        assert_eq!(node_join("/a/b", &["c", "d"]), "/a/b/c/d");
        assert_eq!(node_join("/a/b/", &["c"]), "/a/b/c");
        assert_eq!(node_join("/a", &["/abs", "c"]), "/abs/c");
        assert_eq!(node_dirname("/a/b/c.jsonl"), "/a/b");
        assert_eq!(node_dirname("/x"), "/");
    }
}
