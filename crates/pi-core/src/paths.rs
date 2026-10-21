//! Path utilities.
//!
//! Port of `src/utils/paths.ts` and `src/core/tools/path-utils.ts`.
//! Mirrors Node.js `path` module semantics (unix flavor) so that resolved
//! paths and tool output match the original implementation.

use std::path::Path;

use crate::normalize::nfd_str;

const NARROW_NO_BREAK_SPACE: char = '\u{202F}';

#[derive(Debug, Clone, Default)]
pub struct PathInputOptions {
    /// Trim leading/trailing whitespace before normalization.
    pub trim: bool,
    /// Expand leading `~` to a home directory. Defaults to true.
    pub expand_tilde: Option<bool>,
    /// Home directory used for `~` expansion. Defaults to $HOME.
    pub home_dir: Option<String>,
    /// Strip a leading `@`, used for CLI @file paths.
    pub strip_at_prefix: bool,
    /// Normalize unicode space variants to regular spaces.
    pub normalize_unicode_spaces: bool,
}

fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// Percent-decode a URI component (enough for file:// URLs).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 1 && i + 2 <= bytes.len() - 1 + 1 {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if i + 2 < bytes.len() {
                if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(hi * 16 + lo);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Node's `fileURLToPath` for unix file: URLs.
fn file_url_to_path(url: &str) -> String {
    // file:///path or file://localhost/path
    let rest = url.strip_prefix("file://").unwrap_or(url);
    let rest = if let Some(after) = rest.strip_prefix("localhost") {
        after
    } else {
        rest
    };
    percent_decode(rest)
}

/// Port of `normalizePath` (unix branch; see normalizeWindowsShellPath for the
/// win32-specific path munging, which is inert on unix).
pub fn normalize_path(input: &str, options: &PathInputOptions) -> String {
    let mut normalized = if options.trim {
        input.trim().to_string()
    } else {
        input.to_string()
    };

    if options.normalize_unicode_spaces {
        normalized = normalized
            .chars()
            .map(|c| if is_unicode_space(c) { ' ' } else { c })
            .collect();
    }
    if options.strip_at_prefix {
        if let Some(rest) = normalized.strip_prefix('@') {
            normalized = rest.to_string();
        }
    }

    if options.expand_tilde.unwrap_or(true) {
        let home = options.home_dir.clone().unwrap_or_else(home_dir);
        if normalized == "~" {
            return home;
        }
        if let Some(rest) = normalized.strip_prefix("~/") {
            return node_join_2(&home, rest);
        }
    }

    if normalized.starts_with("file://") {
        return file_url_to_path(&normalized);
    }

    normalized
}

/// Node `path.join` for two unix segments.
fn node_join_2(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    let a = a.trim_end_matches('/');
    let b = b.trim_start_matches('/');
    format!("{}/{}", a, b)
}

/// Node `path.resolve(...)` normalization: collapse `.`/`..` and duplicate
/// separators in an absolute unix path.
fn normalize_absolute(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    format!("/{}", stack.join("/"))
}

/// Port of `resolvePath`. Resolves `input` against `base_dir` with Node
/// `path.resolve` semantics.
pub fn resolve_path(input: &str, base_dir: &str, options: &PathInputOptions) -> String {
    let normalized = normalize_path(input, options);
    let normalized_base_dir = normalize_path(base_dir, &PathInputOptions::default());
    if normalized.starts_with('/') {
        normalize_absolute(&normalized)
    } else {
        normalize_absolute(&node_join_2(&normalized_base_dir, &normalized))
    }
}

/// Port of `expandPath` (path-utils.ts).
pub fn expand_path(file_path: &str) -> String {
    normalize_path(
        file_path,
        &PathInputOptions {
            normalize_unicode_spaces: true,
            strip_at_prefix: true,
            ..Default::default()
        },
    )
}

/// Port of `resolveToCwd` (path-utils.ts).
pub fn resolve_to_cwd(file_path: &str, cwd: &str) -> String {
    resolve_path(
        file_path,
        cwd,
        &PathInputOptions {
            normalize_unicode_spaces: true,
            strip_at_prefix: true,
            ..Default::default()
        },
    )
}

fn try_macos_screenshot_path(file_path: &str) -> String {
    // Replace / (AM|PM)\./gi with narrow-no-break-space + $1 + "."
    let chars: Vec<char> = file_path.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' ' && i + 3 < chars.len() {
            let c1 = chars[i + 1].to_ascii_lowercase();
            let c2 = chars[i + 2].to_ascii_lowercase();
            let c3 = chars[i + 3];
            if (c1 == 'a' || c1 == 'p') && c2 == 'm' && c3 == '.' {
                out.push(NARROW_NO_BREAK_SPACE);
                out.push(chars[i + 1]);
                out.push(chars[i + 2]);
                out.push('.');
                i += 4; // space + AM/PM + '.'
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn try_nfd_variant(file_path: &str) -> String {
    nfd_str(file_path)
}

fn try_curly_quote_variant(file_path: &str) -> String {
    file_path.replace('\'', "\u{2019}")
}

fn file_exists(file_path: &str) -> bool {
    Path::new(file_path).exists()
}

/// Port of `resolveReadPath`: resolve a path, then try macOS screenshot-name
/// variants (narrow no-break space before AM/PM, NFD normalization, curly
/// apostrophes) when the direct path does not exist.
pub fn resolve_read_path(file_path: &str, cwd: &str) -> String {
    let resolved = resolve_to_cwd(file_path, cwd);

    if file_exists(&resolved) {
        return resolved;
    }

    let am_pm_variant = try_macos_screenshot_path(&resolved);
    if am_pm_variant != resolved && file_exists(&am_pm_variant) {
        return am_pm_variant;
    }

    let nfd_variant = try_nfd_variant(&resolved);
    if nfd_variant != resolved && file_exists(&nfd_variant) {
        return nfd_variant;
    }

    let curly_variant = try_curly_quote_variant(&resolved);
    if curly_variant != resolved && file_exists(&curly_variant) {
        return curly_variant;
    }

    let nfd_curly_variant = try_curly_quote_variant(&nfd_variant);
    if nfd_curly_variant != resolved && file_exists(&nfd_curly_variant) {
        return nfd_curly_variant;
    }

    resolved
}

/// Port of `canonicalizePath`: realpath with fallback to the raw path.
pub fn canonicalize_path(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

/// Node `path.relative(from, to)` for unix paths.
pub(crate) fn node_relative(from: &str, to: &str) -> String {
    let from = normalize_absolute(from);
    let to = normalize_absolute(to);
    if from == to {
        return String::new();
    }
    let from_segs: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_segs: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let mut common = 0;
    while common < from_segs.len() && common < to_segs.len() && from_segs[common] == to_segs[common] {
        common += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_segs.len() {
        parts.push("..".to_string());
    }
    for seg in &to_segs[common..] {
        parts.push(seg.to_string());
    }
    parts.join("/")
}

/// Port of `getCwdRelativePath`: relative path if inside cwd, else None.
pub fn get_cwd_relative_path(file_path: &str, cwd: &str) -> Option<String> {
    let resolved_cwd = resolve_path(cwd, ".", &PathInputOptions::default());
    let resolved_path = resolve_path(file_path, &resolved_cwd, &PathInputOptions::default());
    let relative_path = node_relative(&resolved_cwd, &resolved_path);
    let is_inside_cwd = relative_path.is_empty()
        || (relative_path != ".." && !relative_path.starts_with("../") && !relative_path.starts_with('/'));

    if is_inside_cwd {
        Some(if relative_path.is_empty() {
            ".".to_string()
        } else {
            relative_path
        })
    } else {
        None
    }
}

/// Port of `formatPathRelativeToCwdOrAbsolute`.
pub fn format_path_relative_to_cwd_or_absolute(file_path: &str, cwd: &str) -> String {
    let absolute_path = resolve_path(file_path, cwd, &PathInputOptions::default());
    get_cwd_relative_path(&absolute_path, cwd)
        .unwrap_or(absolute_path)
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// Port of `isLocalPath`: true when the value is not a package source
/// (npm:, git:, ...), built-in extension, or remote URL protocol.
pub fn is_local_path(value: &str) -> bool {
    let trimmed = value.trim();
    let prefixes = [
        "npm:", "git:", "github:", "http:", "https:", "ssh:", "builtin:",
    ];
    !prefixes.iter().any(|p| trimmed.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_semantics() {
        assert_eq!(resolve_path("a.txt", "/tmp/x", &PathInputOptions::default()), "/tmp/x/a.txt");
        assert_eq!(resolve_path("/abs/a.txt", "/tmp/x", &PathInputOptions::default()), "/abs/a.txt");
        assert_eq!(
            resolve_path("../a.txt", "/tmp/x/y", &PathInputOptions::default()),
            "/tmp/x/a.txt"
        );
        assert_eq!(
            resolve_path("./a/../b", "/tmp/x", &PathInputOptions::default()),
            "/tmp/x/b"
        );
        assert_eq!(
            resolve_path("~/f.txt", "/tmp", &PathInputOptions { home_dir: Some("/home/u".into()), ..Default::default() }),
            "/home/u/f.txt"
        );
        assert_eq!(
            resolve_path("file:///tmp/a%20b.txt", "/tmp", &PathInputOptions::default()),
            "/tmp/a b.txt"
        );
    }

    #[test]
    fn resolve_to_cwd_strips_at_and_unicode_spaces() {
        assert_eq!(resolve_to_cwd("@a.txt", "/tmp"), "/tmp/a.txt");
        assert_eq!(resolve_to_cwd("a\u{00A0}b.txt", "/tmp"), "/tmp/a b.txt");
    }

    #[test]
    fn relative_paths() {
        assert_eq!(get_cwd_relative_path("/tmp/x/a.txt", "/tmp/x").as_deref(), Some("a.txt"));
        assert_eq!(get_cwd_relative_path("/tmp/x", "/tmp/x").as_deref(), Some("."));
        assert_eq!(get_cwd_relative_path("/etc/passwd", "/tmp/x"), None);
        assert_eq!(format_path_relative_to_cwd_or_absolute("/tmp/x/a/b.txt", "/tmp/x"), "a/b.txt");
        assert_eq!(format_path_relative_to_cwd_or_absolute("/etc/hosts", "/tmp/x"), "/etc/hosts");
    }

    #[test]
    fn local_path_detection() {
        assert!(is_local_path("./foo"));
        assert!(is_local_path("file://x"));
        assert!(!is_local_path("npm:pkg"));
        assert!(!is_local_path("https://x"));
    }

    #[test]
    fn macos_screenshot_ampm_variant() {
        assert_eq!(
            try_macos_screenshot_path("/x/Screenshot at 12.03.45 PM.png"),
            "/x/Screenshot at 12.03.45\u{202F}PM.png"
        );
        assert_eq!(try_macos_screenshot_path("/x/AM. plain.txt"), "/x/AM. plain.txt");
    }
}

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
