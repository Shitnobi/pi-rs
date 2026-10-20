//! Port of `packages/coding-agent/src/core/auth-guidance.ts` (25 LOC).
//!
//! All four TS exports ported; the TS calls `getDocsPath()`
//! (`resolve(join(getPackageDir(), "docs"))` — a process-global lookup),
//! which the Rust port takes as an injected `docs_path: &str` parameter per
//! plan §5 (harness design).
//!
//! `join(getDocsPath(), "providers.md")` is replicated by
//! [`node_path_join`] — a faithful two-segment `node:path` (posix) join:
//! empty segments are skipped, then the concatenation is normalized
//! (`.`/`..` resolution, `//` collapsing, trailing-slash preservation).
//! pi-core's `paths::node_join` is private and does not resolve `..`, so a
//! local implementation lives here.
//!
//! TS source: /tmp/pi-repo/packages/coding-agent/src/core/auth-guidance.ts

/// TS `UNKNOWN_PROVIDER = "unknown"`.
const UNKNOWN_PROVIDER: &str = "unknown";

// ============================================================================
// node:path join (two segments, posix)
// ============================================================================

/// `node:path` posix `normalize` — the normalizer `join` applies to its
/// concatenation. Behavior probed against node:
/// `"a/.."` → `"."`, `"/.."` → `"/"`, `"../"` → `"../"`, `"a/b/"` → `"a/b/"`,
/// `"a//b/"` → `"a/b/"`, `"a/./b"` → `"a/b"`, `""` → `"."`.
fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with('/');
    let trailing_separator = path.ends_with('/');

    let mut stack: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => match stack.last() {
                // Relative paths keep leading ".."s (".." can't be on the
                // stack of an absolute path — it is dropped at the root).
                Some(&"..") if !absolute => stack.push(".."),
                Some(_) => {
                    stack.pop();
                }
                None => {
                    if !absolute {
                        stack.push("..");
                    }
                }
            },
            s => stack.push(s),
        }
    }

    let body = stack.join("/");
    let mut result = if absolute {
        format!("/{body}")
    } else if body.is_empty() {
        ".".to_string()
    } else {
        body
    };
    if trailing_separator && !result.ends_with('/') {
        result.push('/');
    }
    result
}

/// `node:path` posix `join(a, b)` for exactly two segments — skip empty
/// arguments, join the rest with `/`, then [`posix_normalize`]; all-empty
/// input yields `"."`. Probed against node:
/// `("/docs", "providers.md")` → `"/docs/providers.md"`,
/// `("docs/", "p.md")` → `"docs/p.md"`, `("", "x")` → `"x"`,
/// `("a", "")` → `"a"`, `("a", "/x")` → `"a/x"` (join does *not* reset on an
/// absolute second segment — that is `resolve`), `("a", "./x")` → `"a/x"`,
/// `("a", "../x")` → `"x"`, `("/a/b", "../c")` → `"/a/c"`,
/// `("a//b", "c")` → `"a/b/c"`, `("a", "..")` → `"."`.
fn node_path_join(a: &str, b: &str) -> String {
    let mut joined: Option<String> = None;
    for arg in [a, b] {
        if !arg.is_empty() {
            match &mut joined {
                None => joined = Some(arg.to_string()),
                Some(j) => {
                    j.push('/');
                    j.push_str(arg);
                }
            }
        }
    }
    match joined {
        None => ".".to_string(),
        Some(j) => posix_normalize(&j),
    }
}

// ============================================================================
// auth-guidance exports
// ============================================================================

/// Port of `getProviderLoginHelp(): string`.
pub fn get_provider_login_help(docs_path: &str) -> String {
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_string(),
        format!("  {}", node_path_join(docs_path, "providers.md")),
        format!("  {}", node_path_join(docs_path, "models.md")),
    ]
    .join("\n")
}

/// Port of `formatNoModelsAvailableMessage(): string`.
pub fn format_no_models_available_message(docs_path: &str) -> String {
    format!("No models available. {}", get_provider_login_help(docs_path))
}

/// Port of `formatNoModelSelectedMessage(): string`.
pub fn format_no_model_selected_message(docs_path: &str) -> String {
    format!(
        "No model selected.\n\n{}\n\nThen use /model to select a model.",
        get_provider_login_help(docs_path)
    )
}

/// Port of `formatNoApiKeyFoundMessage(provider: string): string` — the
/// `"unknown"` provider displays as "the selected model".
pub fn format_no_api_key_found_message(docs_path: &str, provider: &str) -> String {
    let provider_display = if provider == UNKNOWN_PROVIDER {
        "the selected model"
    } else {
        provider
    };
    format!(
        "No API key found for {}.\n\n{}",
        provider_display,
        get_provider_login_help(docs_path)
    )
}

// ============================================================================
// Tests (path-join vectors and message text probed against node / the TS
// dist)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_path_join_matches_node() {
        // probed with `node -e 'require("path").join(a, b)'`
        let cases: &[(&str, &str, &str)] = &[
            ("/docs", "providers.md", "/docs/providers.md"),
            ("docs/", "p.md", "docs/p.md"),
            ("", "x", "x"),
            ("a", "", "a"),
            ("a", "/x", "a/x"),
            ("a", "./x", "a/x"),
            ("a", "../x", "x"),
            ("/a/b", "../c", "/a/c"),
            ("a//b", "c", "a/b/c"),
            ("a/..", "x", "x"),
            (".", "x", "x"),
            ("..", "x", "../x"),
            ("a/b/", "../c.md", "a/c.md"),
            ("a/..", "", "."),
            ("", "/x", "/x"),
            ("/", "x", "/x"),
            ("..", "..", "../.."),
            ("a", "b/", "a/b/"),
            (".", "", "."),
            (".", ".", "."),
            ("a/", "", "a/"),
            ("", "", "."),
            ("/", "", "/"),
            ("a", "..", "."),
            ("/a", "../..", "/"),
            ("a/b", "../../c", "c"),
        ];
        for (a, b, expected) in cases {
            assert_eq!(node_path_join(a, b), *expected, "join({a:?}, {b:?})");
        }
    }

    #[test]
    fn posix_normalize_matches_node() {
        // probed with `node -e 'require("path").posix.normalize(p)'`
        let cases: &[(&str, &str)] = &[
            ("a/..", "."),
            ("/..", "/"),
            ("../", "../"),
            ("a/b/", "a/b/"),
            ("/", "/"),
            ("..", ".."),
            ("a//b/", "a/b/"),
            ("a/./b", "a/b"),
        ];
        for (p, expected) in cases {
            assert_eq!(posix_normalize(p), *expected, "normalize({p:?})");
        }
    }

    #[test]
    fn login_help_message() {
        assert_eq!(
            get_provider_login_help("/usr/lib/pi/docs"),
            "Use /login to log into a provider via OAuth or API key. See:\n  \
             /usr/lib/pi/docs/providers.md\n  /usr/lib/pi/docs/models.md"
        );
        // trailing slash on docs_path joins without doubling
        assert_eq!(
            get_provider_login_help("docs/"),
            "Use /login to log into a provider via OAuth or API key. See:\n  \
             docs/providers.md\n  docs/models.md"
        );
    }

    #[test]
    fn no_models_available_message() {
        assert_eq!(
            format_no_models_available_message("/d"),
            format!("No models available. {}", get_provider_login_help("/d"))
        );
        assert_eq!(
            format_no_models_available_message("/d"),
            "No models available. Use /login to log into a provider via OAuth or API key. See:\n  \
             /d/providers.md\n  /d/models.md"
        );
    }

    #[test]
    fn no_model_selected_message() {
        assert_eq!(
            format_no_model_selected_message("/d"),
            "No model selected.\n\nUse /login to log into a provider via OAuth or API key. \
             See:\n  /d/providers.md\n  /d/models.md\n\nThen use /model to select a model."
        );
    }

    #[test]
    fn no_api_key_found_message() {
        assert_eq!(
            format_no_api_key_found_message("/d", "anthropic"),
            "No API key found for anthropic.\n\nUse /login to log into a provider via OAuth or \
             API key. See:\n  /d/providers.md\n  /d/models.md"
        );
        // "unknown" → "the selected model" special case
        assert_eq!(
            format_no_api_key_found_message("/d", "unknown"),
            "No API key found for the selected model.\n\nUse /login to log into a provider via \
             OAuth or API key. See:\n  /d/providers.md\n  /d/models.md"
        );
    }
}
