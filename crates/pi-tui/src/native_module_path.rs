//! Port of `native-module-path.ts` — native helper module path candidates.
//!
//! Computes the deduplicated list of filesystem locations a platform native
//! module (`.node`) may live at: beside the installed TUI package, beside and
//! inside this module's directory, and beside the running executable.
//! Standalone binaries have no installed TUI package, so package resolution
//! is optional — a failed resolution simply drops that candidate (the TS
//! version lets `require.resolve` throw and swallows it).
//!
//! Port notes:
//! * `resolvePackage` is an injectable closure returning `Option<String>`
//!   (`None` models the throwing resolver) so tests can stub resolution.
//! * Like upstream `path.join`, paths are joined textually without
//!   normalizing `..` components; the `Set` dedup is a string-equality
//!   dedup preserving the first occurrence.

use std::path::{Path, PathBuf};

/// The npm package name of the TUI library (upstream constant).
pub const TUI_PACKAGE_NAME: &str = "@earendil-works/pi-tui";

/// Resolves a package specifier to its entry file, or `None` when the
/// package is not installed.
pub type ResolvePackage<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Injectable inputs of [`get_native_module_candidates`].
#[derive(Default)]
pub struct NativeModuleCandidateOptions<'a> {
    /// Module location whose directory is searched (TS: `moduleUrl`, a
    /// `file://` URL). Defaults to this source file's directory.
    pub module_url: Option<&'a str>,
    /// Executable whose directory is searched (TS: `process.execPath`).
    /// Defaults to `std::env::current_exe()`.
    pub exec_path: Option<&'a str>,
    /// Package resolution hook (TS: `moduleRequire.resolve`). Defaults to
    /// "not installed": Rust builds behave like standalone binaries, which
    /// upstream also supports by catching the resolution failure.
    pub resolve_package: Option<ResolvePackage<'a>>,
}

/// Port of `fileURLToPath` for the subset we accept: `file://` URLs (with an
/// optional `localhost` authority) and plain paths.
fn file_url_to_path(url: &str) -> &str {
    match url.strip_prefix("file://") {
        Some(rest) => rest.strip_prefix("localhost").unwrap_or(rest),
        None => url,
    }
}

fn dirname(path: &str) -> PathBuf {
    Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// The directory of this module (TS: `dirname(fileURLToPath(import.meta.url))`).
fn default_module_dir() -> PathBuf {
    dirname(file!())
}

/// Lexical normalization matching Node's `path.normalize`/`path.join`:
/// drop `.` components, resolve `..` against the parent (clamped at the
/// root for absolute paths).
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Port of `getNativeModuleCandidates`: the deduplicated candidate paths for
/// a native module, in upstream priority order.
pub fn get_native_module_candidates(
    native_path: &str,
    options: NativeModuleCandidateOptions,
) -> Vec<String> {
    fn push(candidates: &mut Vec<String>, path: PathBuf) {
        let candidate = normalize(&path).to_string_lossy().into_owned();
        // TS: Array.from(new Set(candidates)) — first occurrence wins.
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    let module_dir = match options.module_url {
        Some(url) => dirname(file_url_to_path(url)),
        None => default_module_dir(),
    };
    let mut candidates: Vec<String> = Vec::new();

    // The installed TUI package: packageEntry/../nativePath. Standalone
    // binaries do not have an installed TUI package.
    if let Some(resolve_package) = options.resolve_package {
        if let Some(package_entry) = resolve_package(TUI_PACKAGE_NAME) {
            push(&mut candidates, dirname(&package_entry).join("..").join(native_path));
        }
    }

    push(&mut candidates, module_dir.join("..").join(native_path));
    push(&mut candidates, module_dir.join(native_path));
    if let Some(exec_path) = options
        .exec_path
        .map(str::to_string)
        .or_else(|| std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned()))
    {
        push(&mut candidates, dirname(&exec_path).join(native_path));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(parts: &[&str]) -> String {
        let mut path = std::env::current_dir().unwrap();
        for part in parts {
            path.push(part);
        }
        path.to_string_lossy().into_owned()
    }

    /// Mirrors Node's `path.join`: concatenate, then normalize.
    fn join(parts: &[&str]) -> String {
        let mut path = PathBuf::from(parts[0]);
        for part in &parts[1..] {
            path.push(part);
        }
        normalize(&path).to_string_lossy().into_owned()
    }

    fn dirname_str(path: &str) -> String {
        dirname(path).to_string_lossy().into_owned()
    }

    /// Upstream: "resolves native helpers from the installed TUI package
    /// when the module is bundled elsewhere".
    #[test]
    fn resolves_native_helpers_from_the_installed_tui_package_when_bundled_elsewhere() {
        let package_root = resolve(&["virtual", "node_modules", "@earendil-works", "pi-tui"]);
        let bundled_module = resolve(&["virtual", "pi-coding-agent", "dist", "bundle", "chunks", "chunk.js"]);
        let native_path = "native/win32/prebuilds/win32-arm64/win32-platform.node";

        let resolve_package = |specifier: &str| -> Option<String> {
            assert_eq!(specifier, TUI_PACKAGE_NAME);
            Some(join(&[&package_root, "dist", "index.js"]))
        };
        let exec_path = resolve(&["virtual", "node", "node.exe"]);

        let candidates = get_native_module_candidates(
            native_path,
            NativeModuleCandidateOptions {
                module_url: Some(&bundled_module),
                exec_path: Some(&exec_path),
                resolve_package: Some(&resolve_package),
            },
        );

        assert_eq!(candidates[0], join(&[&package_root, native_path]));
        assert!(candidates.contains(&join(&[&dirname_str(&bundled_module), "..", native_path])));
    }

    /// Upstream: "keeps standalone binary fallbacks when the TUI package is
    /// unavailable".
    #[test]
    fn keeps_standalone_binary_fallbacks_when_the_tui_package_is_unavailable() {
        let bundled_module = resolve(&["virtual", "pi", "bundle", "chunks", "chunk.js"]);
        let exec_path = resolve(&["virtual", "pi", "pi.exe"]);
        let native_path = "native/darwin/prebuilds/darwin-arm64/darwin-platform.node";

        // A throwing resolver ports to None.
        let resolve_package = |_specifier: &str| -> Option<String> { None };

        let candidates = get_native_module_candidates(
            native_path,
            NativeModuleCandidateOptions {
                module_url: Some(&bundled_module),
                exec_path: Some(&exec_path),
                resolve_package: Some(&resolve_package),
            },
        );

        assert_eq!(
            candidates,
            vec![
                join(&[&dirname_str(&bundled_module), "..", native_path]),
                join(&[&dirname_str(&bundled_module), native_path]),
                join(&[&dirname_str(&exec_path), native_path]),
            ]
        );
    }

    /// Covers the upstream `Set` dedup: when the module directory equals the
    /// executable directory, those candidates collapse (textual equality;
    /// `..` components are not normalized).
    #[test]
    fn deduplicates_repeated_candidate_paths() {
        let module_url = resolve(&["virtual", "app", "chunks", "chunk.js"]);
        let exec_path = resolve(&["virtual", "app", "chunks", "run"]);
        let native_path = "native/linux/prebuilds/linux-x64/linux-platform-x11.node";

        let candidates = get_native_module_candidates(
            native_path,
            NativeModuleCandidateOptions {
                module_url: Some(&module_url),
                exec_path: Some(&exec_path),
                resolve_package: None,
            },
        );

        let module_dir = dirname_str(&module_url);
        assert_eq!(
            candidates,
            vec![
                join(&[&module_dir, "..", native_path]),
                join(&[&module_dir, native_path]),
            ]
        );
    }
}
