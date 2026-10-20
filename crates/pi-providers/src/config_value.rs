//! Port of `packages/coding-agent/src/core/resolve-config-value.ts`.
//!
//! Resolve configuration values that may be shell commands, environment
//! variables, or literals. Used by auth-storage.ts and model-registry.ts
//! (auth_storage / model_config waves).
//!
//! Parity design (plan §3.1/§3.2):
//! - **env is injected** (the `Env` map parameter), never `std::env`. The TS
//!   reads `env?.[name] || process.env[name] || undefined` — a *truthy-only*
//!   lookup (empty strings count as unset); the ambient `process.env`
//!   fallback belongs to the real (non-parity) implementation and the
//!   injected map plays its role here.
//! - **Command execution is behind the [`CommandRunner`] trait** (sync). The
//!   TS `executeWithConfiguredShell` (spawnSync with `getShellConfig()` —
//!   shell + args + argv/stdin transport, 10 000 ms timeout, `windowsHide`)
//!   and `executeWithDefaultShell` (execSync, 10 000 ms timeout) live in the
//!   real impl, including the observable stdout semantics: **trimmed**, and
//!   empty output → `None`. The Windows-only configured-shell-then-fallback
//!   chain (`process.platform === "win32"`) is also a real-impl concern; on
//!   the parity platform (Linux) the TS always takes the default-shell path.
//! - The TS module-level `commandResultCache` (persists for process
//!   lifetime, leaking between batched harness cases — plan §2 item 22) is
//!   ported as the owned cache inside [`ConfigValueResolver`], so parity
//!   cases construct and control it explicitly. TS `clearConfigValueCache()`
//!   (exported for testing) becomes [`ConfigValueResolver::clear_cache`].
//!
//! Template grammar (ported exactly — note there is **no** `$VAR:-default`
//! syntax in the TS; `"$NOPE:-fallback"` parses as the env var `NOPE`
//! followed by the literal `":-fallback"`):
//! - values starting with `!` are shell commands (`"!cmd"` — the `!` is part
//!   of the cached key, stripped before execution)
//! - `$ENV_VAR` and `${ENV_VAR}` interpolate the named environment variable
//!   (name must match `^[A-Za-z_][A-Za-z0-9_]*$` — hand-written scanner, not
//!   the regex crate, plan §2 item 19)
//! - in non-command values, `$$` escapes a literal `$` and `$!` escapes a
//!   literal `!`; an unmatched `$` stays literal
//! - anything else is a literal (an empty value resolves to `Some("")`)
//!
//! TS source: /tmp/pi-repo/packages/coding-agent/src/core/resolve-config-value.ts

use std::collections::{BTreeMap, HashMap};
use std::fmt;

// ============================================================================
// Injected environment
// ============================================================================

/// Injected environment map (replaces `process.env` reads — plan §2 item 16).
/// `BTreeMap` for deterministic lookups; iteration order is never observable
/// through this module (only point lookups).
pub type Env = BTreeMap<String, String>;

// ============================================================================
// CommandRunner trait
// ============================================================================

/// Shell-command execution seam for `!command` config values
/// (plan §3.2: "same shape as the existing BashOperations seam").
///
/// `run` receives the command with its leading `!` stripped (TS
/// `executeCommandUncached`'s `commandConfig.slice(1)`) and must reproduce
/// the TS `executeWithDefaultShell` / `executeWithConfiguredShell`
/// observable contract:
///
/// - execute via the configured shell (`getShellConfig()` — `bash -c` on the
///   parity platform, argv transport) or the default shell
/// - 10 000 ms timeout; spawn errors and nonzero exit status → `None`
/// - stdout is **trimmed** (JS `String.prototype.trim`)
/// - empty/whitespace-only stdout → `None` (TS `output.trim() || undefined`)
///
/// Parity fakes script the return values directly (including `None` for the
/// empty-stdout and nonzero-exit cases); the timeout and shell-config
/// machinery belong to the real implementation, not the parity path.
pub trait CommandRunner {
    fn run(&self, command: &str) -> Option<String>;
}

// ============================================================================
// Error type
// ============================================================================

/// The `Error` thrown by `resolveConfigValueOrThrow` / `resolveHeadersOrThrow`
/// (message strings verbatim from the TS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValueError {
    pub message: String,
}

impl ConfigValueError {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for ConfigValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigValueError {}

// ============================================================================
// Template parsing (TS parseConfigValueTemplate / parseConfigValueReference)
// ============================================================================

/// TS `TemplatePart`: `{ type: "literal"; value } | { type: "env"; name }`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplatePart {
    Literal(String),
    Env(String),
}

/// TS `ConfigValueReference`:
/// `{ type: "command"; config } | { type: "template"; parts }`.
/// `Command` keeps the full value *including* the leading `!` (the TS caches
/// by that full string).
enum ConfigValueReference {
    Command(String),
    Template(Vec<TemplatePart>),
}

/// TS `appendLiteral` — empty values are dropped; adjacent literals merge.
fn append_literal(parts: &mut Vec<TemplatePart>, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(TemplatePart::Literal(existing)) = parts.last_mut() {
        existing.push_str(value);
        return;
    }
    parts.push(TemplatePart::Literal(value.to_string()));
}

/// Longest prefix of `s` matching `^[A-Za-z_][A-Za-z0-9_]*` (TS
/// `ENV_VAR_NAME_PREFIX_RE`). ASCII-only classes, so byte scanning is
/// UTF-8-safe (continuation bytes never match ASCII).
fn env_var_name_prefix_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let first = *bytes.first()?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut len = 1;
    while len < bytes.len() && (bytes[len].is_ascii_alphanumeric() || bytes[len] == b'_') {
        len += 1;
    }
    Some(len)
}

/// TS `ENV_VAR_NAME_RE` (`^[A-Za-z_][A-Za-z0-9_]*$`) — full match.
fn is_env_var_name(s: &str) -> bool {
    env_var_name_prefix_len(s) == Some(s.len())
}

/// TS `parseConfigValueTemplate` — `$VAR`, `${VAR}`, `$$`, `$!` escapes.
fn parse_config_value_template(config: &str) -> Vec<TemplatePart> {
    let mut parts: Vec<TemplatePart> = Vec::new();
    let mut index = 0;

    while index < config.len() {
        let dollar_index = match config[index..].find('$') {
            Some(rel) => index + rel,
            None => {
                append_literal(&mut parts, &config[index..]);
                break;
            }
        };

        append_literal(&mut parts, &config[index..dollar_index]);
        let next_char = config.as_bytes().get(dollar_index + 1).copied();

        if next_char == Some(b'$') || next_char == Some(b'!') {
            // `$$` escapes a literal `$`; `$!` escapes a literal `!`.
            append_literal(&mut parts, &config[dollar_index + 1..dollar_index + 2]);
            index = dollar_index + 2;
            continue;
        }

        if next_char == Some(b'{') {
            match config[dollar_index + 2..].find('}') {
                None => {
                    append_literal(&mut parts, "$");
                    index = dollar_index + 1;
                    continue;
                }
                Some(rel) => {
                    let end_index = dollar_index + 2 + rel;
                    let name = &config[dollar_index + 2..end_index];
                    if is_env_var_name(name) {
                        parts.push(TemplatePart::Env(name.to_string()));
                    } else {
                        append_literal(&mut parts, &config[dollar_index..end_index + 1]);
                    }
                    index = end_index + 1;
                    continue;
                }
            }
        }

        let rest = &config[dollar_index + 1..];
        if let Some(prefix_len) = env_var_name_prefix_len(rest) {
            parts.push(TemplatePart::Env(rest[..prefix_len].to_string()));
            index = dollar_index + 1 + prefix_len;
            continue;
        }

        append_literal(&mut parts, "$");
        index = dollar_index + 1;
    }

    parts
}

/// TS `parseConfigValueReference` — `config.startsWith("!")` → command.
fn parse_config_value_reference(config: &str) -> ConfigValueReference {
    if config.starts_with('!') {
        return ConfigValueReference::Command(config.to_string());
    }
    ConfigValueReference::Template(parse_config_value_template(config))
}

// ============================================================================
// Env resolution / templates
// ============================================================================

/// TS `resolveEnvConfigValue`: `env?.[name] || process.env[name] || undefined`
/// — truthy-only lookup (an empty-string value counts as unset). The ambient
/// `process.env` fallback layer is intentionally absent: env is injected and
/// never read from `std::env` (plan §2 item 16); the injected map plays the
/// role of the ambient environment for parity cases.
fn resolve_env_config_value<'a>(name: &str, env: Option<&'a Env>) -> Option<&'a str> {
    env?.get(name).map(String::as_str).filter(|v| !v.is_empty())
}

/// TS `getTemplateEnvVarNames` — unique env-var names, insertion order.
fn get_template_env_var_names(parts: &[TemplatePart]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in parts {
        if let TemplatePart::Env(name) = part {
            if !names.iter().any(|n| n == name) {
                names.push(name.clone());
            }
        }
    }
    names
}

/// TS `resolveTemplate` — any missing env var aborts with `undefined`.
fn resolve_template(parts: &[TemplatePart], env: Option<&Env>) -> Option<String> {
    let mut resolved = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(value) => resolved.push_str(value),
            TemplatePart::Env(name) => {
                let env_value = resolve_env_config_value(name, env)?;
                resolved.push_str(env_value);
            }
        }
    }
    Some(resolved)
}

// ============================================================================
// Pure query functions
// ============================================================================

/// Port of `getConfigValueEnvVarName(config): string | undefined` — the name
/// iff the value is exactly one env-var reference (`"$FOO"` / `"${FOO}"`).
pub fn get_config_value_env_var_name(config: &str) -> Option<String> {
    if let ConfigValueReference::Template(parts) = parse_config_value_reference(config) {
        if parts.len() == 1 {
            if let Some(TemplatePart::Env(name)) = parts.into_iter().next() {
                return Some(name);
            }
        }
    }
    None
}

/// Port of `getConfigValueEnvVarNames(config): string[]` — unique env-var
/// names referenced by a template value (commands → empty).
pub fn get_config_value_env_var_names(config: &str) -> Vec<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Template(parts) => get_template_env_var_names(&parts),
        ConfigValueReference::Command(_) => Vec::new(),
    }
}

/// Port of `getMissingConfigValueEnvVarNames(config, env): string[]`.
pub fn get_missing_config_value_env_var_names(config: &str, env: Option<&Env>) -> Vec<String> {
    get_config_value_env_var_names(config)
        .into_iter()
        .filter(|name| resolve_env_config_value(name, env).is_none())
        .collect()
}

/// Port of `isCommandConfigValue(config): boolean` — `config.startsWith("!")`.
pub fn is_command_config_value(config: &str) -> bool {
    matches!(parse_config_value_reference(config), ConfigValueReference::Command(_))
}

/// Port of `isConfigValueConfigured(config, env): boolean` — no missing env
/// vars (command values count as configured).
pub fn is_config_value_configured(config: &str, env: Option<&Env>) -> bool {
    get_missing_config_value_env_var_names(config, env).is_empty()
}

// ============================================================================
// Resolution
// ============================================================================

/// Port of `resolveConfigValueUncached(config, env)` — resolve without
/// consulting the command-result cache.
///
/// - `!command` values execute the rest as a shell command and use stdout
/// - `$ENV_VAR` / `${ENV_VAR}` references interpolate the named variable
/// - in non-command values, `$$` escapes a literal `$` and `$!` a literal `!`
/// - otherwise the value is a literal
pub fn resolve_config_value_uncached(
    config: &str,
    env: Option<&Env>,
    runner: &dyn CommandRunner,
) -> Option<String> {
    match parse_config_value_reference(config) {
        // TS `executeCommandUncached`: `commandConfig.slice(1)`.
        ConfigValueReference::Command(command_config) => runner.run(&command_config[1..]),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

/// Port of `resolveConfigValueOrThrow(config, description, env)` — error
/// messages verbatim from the TS:
///
/// - command: `Failed to resolve {description} from shell command: {cmd}` (`!` stripped)
/// - one missing env var: `Failed to resolve {description} from environment variable: {name}`
/// - several: `Failed to resolve {description} from environment variables: {a, b}` (", "-joined)
/// - fallback: `Failed to resolve {description}`
pub fn resolve_config_value_or_throw(
    config: &str,
    description: &str,
    env: Option<&Env>,
    runner: &dyn CommandRunner,
) -> Result<String, ConfigValueError> {
    if let Some(resolved_value) = resolve_config_value_uncached(config, env, runner) {
        return Ok(resolved_value);
    }

    match parse_config_value_reference(config) {
        ConfigValueReference::Command(command_config) => Err(ConfigValueError::new(format!(
            "Failed to resolve {description} from shell command: {}",
            &command_config[1..]
        ))),
        ConfigValueReference::Template(_) => {
            let missing_env_vars = get_missing_config_value_env_var_names(config, env);
            if missing_env_vars.len() == 1 {
                Err(ConfigValueError::new(format!(
                    "Failed to resolve {description} from environment variable: {}",
                    missing_env_vars[0]
                )))
            } else if missing_env_vars.len() > 1 {
                Err(ConfigValueError::new(format!(
                    "Failed to resolve {description} from environment variables: {}",
                    missing_env_vars.join(", ")
                )))
            } else {
                Err(ConfigValueError::new(format!(
                    "Failed to resolve {description}"
                )))
            }
        }
    }
}

// ============================================================================
// Cached resolver (TS module-level commandResultCache)
// ============================================================================

/// Owns the command-result cache (TS module-level `commandResultCache`,
/// which persists for the process lifetime and leaks between batched TS
/// harness cases — plan §2 item 22). Construct one per parity case (or share
/// deliberately) to control the cache explicitly.
pub struct ConfigValueResolver<'a> {
    runner: &'a dyn CommandRunner,
    /// TS `Map<string, string | undefined>` — keyed by the full command
    /// config value *including* the leading `!`; stores misses (`None`) too.
    command_result_cache: HashMap<String, Option<String>>,
}

impl<'a> ConfigValueResolver<'a> {
    pub fn new(runner: &'a dyn CommandRunner) -> Self {
        Self {
            runner,
            command_result_cache: HashMap::new(),
        }
    }

    /// Port of `resolveConfigValue(config, env)` — like
    /// [`resolve_config_value_uncached`], but shell-command results are cached
    /// for the lifetime of this resolver (keyed by the full `!command` value;
    /// `None` results are cached too).
    pub fn resolve_config_value(&mut self, config: &str, env: Option<&Env>) -> Option<String> {
        match parse_config_value_reference(config) {
            ConfigValueReference::Command(command_config) => {
                self.execute_command(&command_config)
            }
            ConfigValueReference::Template(parts) => resolve_template(&parts, env),
        }
    }

    /// TS `executeCommand` — cache-aware command execution.
    fn execute_command(&mut self, command_config: &str) -> Option<String> {
        if let Some(cached) = self.command_result_cache.get(command_config) {
            return cached.clone();
        }
        // TS `executeCommandUncached`: `commandConfig.slice(1)`.
        let result = self.runner.run(&command_config[1..]);
        self.command_result_cache
            .insert(command_config.to_string(), result.clone());
        result
    }

    /// Port of `clearConfigValueCache()` (exported for testing in the TS).
    pub fn clear_cache(&mut self) {
        self.command_result_cache.clear();
    }

    /// Port of `resolveHeaders(headers, env)` — resolve all header values
    /// using the same (cached) resolution logic as API keys. Entries whose
    /// value resolves to a falsy string (`undefined` or `""`) are dropped;
    /// an empty result map is `None` (as is an absent `headers`).
    pub fn resolve_headers(
        &mut self,
        headers: Option<&[(String, String)]>,
        env: Option<&Env>,
    ) -> Option<Vec<(String, String)>> {
        let headers = headers?;
        let mut resolved: Vec<(String, String)> = Vec::new();
        for (key, value) in headers {
            if let Some(resolved_value) = self.resolve_config_value(value, env) {
                if !resolved_value.is_empty() {
                    resolved.push((key.clone(), resolved_value));
                }
            }
        }
        if !resolved.is_empty() {
            Some(resolved)
        } else {
            None
        }
    }
}

/// Port of `resolveHeadersOrThrow(headers, description, env)` — every header
/// value resolved via [`resolve_config_value_or_throw`] with the description
/// `` `{description} header "{key}"` ``; an empty map is still `None`.
pub fn resolve_headers_or_throw(
    headers: Option<&[(String, String)]>,
    description: &str,
    env: Option<&Env>,
    runner: &dyn CommandRunner,
) -> Result<Option<Vec<(String, String)>>, ConfigValueError> {
    let mut resolved: Vec<(String, String)> = Vec::new();
    if let Some(headers) = headers {
        for (key, value) in headers {
            let header_description = format!("{description} header \"{key}\"");
            let resolved_value =
                resolve_config_value_or_throw(value, &header_description, env, runner)?;
            resolved.push((key.clone(), resolved_value));
        }
    }
    if !resolved.is_empty() {
        Ok(Some(resolved))
    } else {
        Ok(None)
    }
}

// ============================================================================
// Tests (fixtures probed against the TS dist with node)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripted fake: returns a canned result per command, counts calls so
    /// caching behavior is observable. `None` entries model nonzero exit /
    /// empty stdout / spawn failure.
    struct FakeRunner {
        results: HashMap<String, Option<String>>,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl FakeRunner {
        fn new(results: &[(&str, Option<&str>)]) -> Self {
            Self {
                results: results
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), v.map(|s| s.to_string())))
                    .collect(),
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn call_count(&self, command: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| c.as_str() == command)
                .count()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, command: &str) -> Option<String> {
            self.calls.borrow_mut().push(command.to_string());
            self.results.get(command).cloned().flatten()
        }
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // ------------------------------------------------------------------
    // Template parsing / resolution (probed against the TS dist)
    // ------------------------------------------------------------------

    #[test]
    fn resolves_env_references() {
        let e = env(&[("FOO", "bar"), ("A", "va")]);
        assert_eq!(
            resolve_config_value_uncached("$FOO", Some(&e), &FakeRunner::new(&[])),
            Some("bar".to_string())
        );
        assert_eq!(
            resolve_config_value_uncached("${FOO}", Some(&e), &FakeRunner::new(&[])),
            Some("bar".to_string())
        );
        assert_eq!(
            resolve_config_value_uncached("pre-${FOO}-post", Some(&e), &FakeRunner::new(&[])),
            Some("pre-bar-post".to_string())
        );
        assert_eq!(
            resolve_config_value_uncached("$A${FOO}", Some(&e), &FakeRunner::new(&[])),
            Some("vabar".to_string())
        );
        // empty value resolves to Some("") — not undefined
        assert_eq!(
            resolve_config_value_uncached("", None, &FakeRunner::new(&[])),
            Some("".to_string())
        );
    }

    #[test]
    fn missing_env_vars_resolve_to_undefined() {
        // There is no "$VAR:-default" syntax: "$NOPE:-fallback" is the env var
        // NOPE (missing) followed by the literal ":-fallback" → undefined.
        assert_eq!(
            resolve_config_value_uncached("$NOPE:-fallback", Some(&env(&[])), &FakeRunner::new(&[])),
            None
        );
        assert_eq!(
            resolve_config_value_uncached("${FOO}", Some(&env(&[])), &FakeRunner::new(&[])),
            None
        );
        // empty-string env values are falsy → treated as unset (TS `||` chain)
        assert_eq!(
            resolve_config_value_uncached("$FOO", Some(&env(&[("FOO", "")])), &FakeRunner::new(&[])),
            None
        );
        // no env injected at all
        assert_eq!(
            resolve_config_value_uncached("$FOO", None, &FakeRunner::new(&[])),
            None
        );
    }

    #[test]
    fn escapes_and_literal_dollars() {
        let runner = FakeRunner::new(&[]);
        let r = |c: &str| resolve_config_value_uncached(c, None, &runner);
        // probed against the TS dist:
        assert_eq!(r("$$"), Some("$".to_string())); // $$ escapes $
        assert_eq!(r("$!"), Some("!".to_string())); // $! escapes !
        assert_eq!(r("$"), Some("$".to_string())); // lone $ stays literal
        assert_eq!(r("$ {}"), Some("$ {}".to_string())); // space after $
        assert_eq!(r("a$"), Some("a$".to_string()));
        assert_eq!(r("pre$$post"), Some("pre$post".to_string()));
        assert_eq!(r("$${A}"), Some("${A}".to_string())); // $ then literal {A}
        assert_eq!(r("a!b"), Some("a!b".to_string())); // ! not at start
        assert_eq!(r("$!x"), Some("!x".to_string()));
        // invalid env-var shapes stay literal:
        assert_eq!(r("$1x"), Some("$1x".to_string())); // digits can't start a name
        assert_eq!(r("${1BAD}"), Some("${1BAD}".to_string()));
        assert_eq!(r("${}"), Some("${}".to_string()));
        assert_eq!(r("${A B}"), Some("${A B}".to_string()));
        // unclosed ${ → literal $, rest re-parsed
        assert_eq!(r("${UNCLOSED"), Some("${UNCLOSED".to_string()));
        assert_eq!(r("$${UNCLOSED"), Some("${UNCLOSED".to_string()));
    }

    #[test]
    fn env_var_name_queries() {
        assert_eq!(get_config_value_env_var_name("$FOO"), Some("FOO".to_string()));
        assert_eq!(get_config_value_env_var_name("${FOO}"), Some("FOO".to_string()));
        assert_eq!(get_config_value_env_var_name("a$FOO"), None);
        assert_eq!(get_config_value_env_var_name("$FOO$BAR"), None); // two parts
        assert_eq!(get_config_value_env_var_name("!cmd"), None); // commands
        assert_eq!(get_config_value_env_var_name("plain"), None);

        // dedup, insertion order preserved
        assert_eq!(
            get_config_value_env_var_names("$A ${B} $A $C"),
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
        assert_eq!(get_config_value_env_var_names("!cmd"), Vec::<String>::new());

        // "$1x" contributes no env var; "${B:-x}" is entirely literal (the
        // name "B:-x" fails ENV_VAR_NAME_RE), so only A is referenced.
        // (Probed against the TS dist: missing("$A ${B:-x} $1x", {}) = ["A"].)
        assert_eq!(
            get_missing_config_value_env_var_names("$A ${B:-x} $1x", Some(&env(&[]))),
            vec!["A".to_string()]
        );
        let e = env(&[("A", "set")]);
        assert_eq!(
            get_missing_config_value_env_var_names("$A ${B:-x} $1x", Some(&e)),
            Vec::<String>::new()
        );
        // empty-string env values count as missing
        assert_eq!(
            get_missing_config_value_env_var_names("$A", Some(&env(&[("A", "")]))),
            vec!["A".to_string()]
        );
    }

    #[test]
    fn command_detection() {
        assert!(is_command_config_value("!cmd"));
        assert!(is_command_config_value("!")); // bare "!" is a (empty) command
        assert!(!is_command_config_value("x!y"));
        assert!(!is_command_config_value("$$"));
        // commands have no env vars → configured
        assert!(is_config_value_configured("!cmd", Some(&env(&[]))));
        assert!(!is_config_value_configured("$A ${B:-x}", Some(&env(&[]))));
        assert!(is_config_value_configured("$A", Some(&env(&[("A", "v")]))));
        assert!(is_config_value_configured("plain", None));
    }

    // ------------------------------------------------------------------
    // Command execution + cache
    // ------------------------------------------------------------------

    #[test]
    fn commands_execute_without_prefix_and_cache() {
        let runner = FakeRunner::new(&[("echo hi", Some("hi")), ("exit 1", None)]);
        let mut resolver = ConfigValueResolver::new(&runner);

        // "!echo hi" → runs "echo hi"; stdout-trim and empty→None are the
        // runner impl's contract (the fake scripts them):
        assert_eq!(resolver.resolve_config_value("!echo hi", None), Some("hi".to_string()));
        assert_eq!(runner.call_count("echo hi"), 1);
        // cached: second call does not re-run
        assert_eq!(resolver.resolve_config_value("!echo hi", None), Some("hi".to_string()));
        assert_eq!(runner.call_count("echo hi"), 1);

        // failed commands are cached too (None result)
        assert_eq!(resolver.resolve_config_value("!exit 1", None), None);
        assert_eq!(resolver.resolve_config_value("!exit 1", None), None);
        assert_eq!(runner.call_count("exit 1"), 1);

        // clearCache → re-run
        resolver.clear_cache();
        assert_eq!(resolver.resolve_config_value("!echo hi", None), Some("hi".to_string()));
        assert_eq!(runner.call_count("echo hi"), 2);
    }

    #[test]
    fn uncached_resolution_reruns_commands() {
        let runner = FakeRunner::new(&[("echo hi", Some("hi"))]);
        assert_eq!(
            resolve_config_value_uncached("!echo hi", None, &runner),
            Some("hi".to_string())
        );
        assert_eq!(
            resolve_config_value_uncached("!echo hi", None, &runner),
            Some("hi".to_string())
        );
        assert_eq!(runner.call_count("echo hi"), 2);
    }

    // ------------------------------------------------------------------
    // resolveConfigValueOrThrow — error strings verbatim
    // ------------------------------------------------------------------

    #[test]
    fn or_throw_error_messages() {
        let runner = FakeRunner::new(&[("exit 1", None)]);
        let none_env = Some(&env(&[]));

        let err = resolve_config_value_or_throw("$A ${B}", "test key", none_env, &runner)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Failed to resolve test key from environment variables: A, B"
        );

        let err = resolve_config_value_or_throw("$A", "test key", none_env, &runner).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Failed to resolve test key from environment variable: A"
        );

        let err =
            resolve_config_value_or_throw("!exit 1", "test key", none_env, &runner).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Failed to resolve test key from shell command: exit 1"
        );

        // successful resolution (including empty strings) does not throw
        assert_eq!(
            resolve_config_value_or_throw("plain", "test key", none_env, &runner),
            Ok("plain".to_string())
        );
        assert_eq!(
            resolve_config_value_or_throw("", "test key", none_env, &runner),
            Ok("".to_string())
        );
    }

    // ------------------------------------------------------------------
    // resolveHeaders / resolveHeadersOrThrow (probed against the dist)
    // ------------------------------------------------------------------

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolve_headers_drops_unresolvable_and_empty() {
        let runner = FakeRunner::new(&[]);
        let mut resolver = ConfigValueResolver::new(&runner);
        let e = env(&[("A", "va")]);
        let hs = headers(&[("a", "$A"), ("b", "static"), ("c", "$MISSING")]);

        assert_eq!(
            resolver.resolve_headers(Some(&hs), Some(&e)),
            Some(vec![
                ("a".to_string(), "va".to_string()),
                ("b".to_string(), "static".to_string())
            ])
        );

        // empty map → undefined (probed); absent headers → undefined
        assert_eq!(resolver.resolve_headers(Some(&[]), Some(&e)), None);
        assert_eq!(resolver.resolve_headers(None, Some(&e)), None);
        // all-unresolvable → undefined
        let hs2 = headers(&[("c", "$MISSING")]);
        assert_eq!(resolver.resolve_headers(Some(&hs2), Some(&e)), None);
        // falsy "" resolved values are dropped too (TS `if (resolvedValue)`)
        let hs3 = headers(&[("a", ""), ("c", "$MISSING")]);
        assert_eq!(resolver.resolve_headers(Some(&hs3), Some(&e)), None);
    }

    #[test]
    fn resolve_headers_or_throw_error_message() {
        let runner = FakeRunner::new(&[]);
        let e = env(&[]);
        let hs = headers(&[("h", "$MISSING")]);

        let err = resolve_headers_or_throw(Some(&hs), "models.json", Some(&e), &runner)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Failed to resolve models.json header \"h\" from environment variable: MISSING"
        );

        // empty map is still None (not an error); absent headers → None
        assert_eq!(
            resolve_headers_or_throw(Some(&[]), "models.json", Some(&e), &runner),
            Ok(None)
        );
        assert_eq!(
            resolve_headers_or_throw(None, "models.json", Some(&e), &runner),
            Ok(None)
        );

        let e2 = env(&[("A", "va")]);
        let hs2 = headers(&[("a", "$A"), ("b", "static")]);
        assert_eq!(
            resolve_headers_or_throw(Some(&hs2), "models.json", Some(&e2), &runner),
            Ok(Some(vec![
                ("a".to_string(), "va".to_string()),
                ("b".to_string(), "static".to_string())
            ]))
        );
    }
}
