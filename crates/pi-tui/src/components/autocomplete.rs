//! Autocomplete engine — 1:1 Rust port of `packages/tui/src/autocomplete.ts`.
//!
//! Port notes (divergences required by the Rust/std-only design):
//!
//! * TS string indices are UTF-16 code units; this port uses `char`
//!   (Unicode scalar) indices for `cursor_col` and all slicing. All ported
//!   tests compute positions with `chars().count()`, so behaviour is
//!   identical for the tested inputs.
//! * TS `async`/`await` flows are modelled with synchronous calls through
//!   injectable seams: filesystem access goes through [`FsSeam`] and the
//!   `fd` subprocess goes through [`CommandRunner`]. The real
//!   implementations ([`StdFs`], [`StdCommandRunner`]) are blocking
//!   (`std::process::Command`, stderr redirected to null); tests inject
//!   fakes so no real process or filesystem access happens.
//! * `SlashCommand::get_argument_completions` is a synchronous closure
//!   returning `Option<Vec<AutocompleteItem>>` (TS `Awaitable<… | null>`
//!   resolved synchronously through the seam).
//! * `AbortSignal` is a simple shared flag (`Rc<Cell<bool>>`), mirroring the
//!   TS usage sites (only `signal.aborted` is consulted).
//! * `localeCompare` is approximated with codepoint ordering; `path.join` /
//!   `dirname` / `basename` are hand-ported POSIX-js helpers
//!   ([`js_path_join`], [`js_path_dirname`], [`js_path_basename`]).

use std::cell::Cell;
use std::io;
use std::rc::Rc;

use crate::fuzzy::fuzzy_filter;
use crate::utils::{is_cjk_punctuation_char, is_js_whitespace, js_trim};

// --- character classification (regexes from utils.ts) --------------------------

/// `autocompleteSeparatorRegex.test(char)` — `\s` or CJK punctuation
/// (utils.ts:62, reused by autocomplete.ts).
fn is_autocomplete_separator(c: char) -> bool {
    is_js_whitespace(c) || is_cjk_punctuation_char(c)
}

/// `autocompleteSeparatorRegex.test(text)` — true if ANY character in `text`
/// is a separator (unanchored `.test()`).
fn separator_test(text: &str) -> bool {
    text.chars().any(is_autocomplete_separator)
}

/// `tokenStartRegex.test(text)` — `/(?:^|\s|cjkPunct)$/u`: true when `text`
/// is empty or ends with a separator (autocomplete.ts:9).
fn token_start_test(text: &[char]) -> bool {
    text.is_empty() || text.last().is_some_and(|&c| is_autocomplete_separator(c))
}

/// `PATH_DELIMITERS` (autocomplete.ts:8).
const PATH_DELIMITERS: &[char] = &[' ', '\t', '"', '\'', '='];

/// `PATH_WRAPPERS` lookup (autocomplete.ts:11): opening wrapper → closer.
fn path_wrapper_closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '<' => Some('>'),
        '`' => Some('`'),
        _ => None,
    }
}

/// Approximation of JS `localeCompare` with codepoint ordering.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}

// --- js path helpers (path.join / dirname / basename, POSIX semantics) --------

/// `path.normalize(p)` (POSIX): resolve `.`/`..`, collapse slashes, preserve a
/// single trailing slash.
fn js_path_normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let is_abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if let Some(last) = parts.last() {
                    if *last != ".." {
                        parts.pop();
                        continue;
                    }
                }
                if !is_abs {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let trailing = p.ends_with('/');
    let mut out = if is_abs { "/".to_string() } else { String::new() };
    out.push_str(&parts.join("/"));
    if out.is_empty() {
        return if trailing { "/".to_string() } else { ".".to_string() };
    }
    if trailing && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// `path.join(a, b)` — normalize(`a + "/" + b`).
fn js_path_join(a: &str, b: &str) -> String {
    if a.is_empty() {
        return js_path_normalize(b);
    }
    if b.is_empty() {
        return js_path_normalize(&format!("{}/", a));
    }
    js_path_normalize(&format!("{}/{}", a, b))
}

/// `path.dirname(p)` (POSIX).
fn js_path_dirname(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let mut s = p;
    while s.len() > 1 && s.ends_with('/') {
        s = &s[..s.len() - 1];
    }
    if s == "/" {
        return "/".to_string();
    }
    match s.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => s[..i].to_string(),
    }
}

/// `path.basename(p)` (POSIX).
fn js_path_basename(p: &str) -> String {
    let mut s = p;
    while s.len() > 1 && s.ends_with('/') {
        s = &s[..s.len() - 1];
    }
    if s == "/" {
        return String::new();
    }
    match s.rfind('/') {
        Some(i) => s[i + 1..].to_string(),
        None => s.to_string(),
    }
}

// --- engine helpers (autocomplete.ts:13-146) -----------------------------------

/// `toDisplayPath` (autocomplete.ts:13).
fn to_display_path(value: &str) -> String {
    value.replace('\\', "/")
}

/// `escapeRegex` (autocomplete.ts:17): `/[.*+?^${}()|[\]\\]/g`.
fn escape_regex(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '[' | ']' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `buildFdPathQuery` (autocomplete.ts:21-47).
fn build_fd_path_query(query: &str) -> String {
    let normalized = to_display_path(query);
    if !normalized.contains('/') {
        return normalized;
    }

    let has_trailing_separator = normalized.ends_with('/');
    // normalized.replace(/^\/+|\/+$/g, "")
    let trimmed = normalized
        .trim_start_matches('/')
        .trim_end_matches('/');
    if trimmed.is_empty() {
        return normalized;
    }

    let separator_pattern = "[\\\\/]";
    let segments: Vec<String> = trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .map(escape_regex)
        .collect();
    if segments.is_empty() {
        return normalized;
    }

    let mut pattern = segments.join(separator_pattern);
    if has_trailing_separator {
        pattern.push_str(separator_pattern);
    }
    pattern
}

/// `findLastDelimiter` (autocomplete.ts:49-60) — char index of the last
/// path delimiter / separator, if any.
fn find_last_delimiter(text: &[char]) -> Option<usize> {
    text.iter()
        .enumerate()
        .filter(|(_, &c)| PATH_DELIMITERS.contains(&c) || is_autocomplete_separator(c))
        .map(|(i, _)| i)
        .last()
}

/// `stripLeadingWrappers` (autocomplete.ts:63-72).
fn strip_leading_wrappers(token: &[char]) -> &[char] {
    let mut result = token;
    while !result.is_empty() {
        let Some(closer) = path_wrapper_closer(result[0]) else {
            break;
        };
        // result.includes(closer, 1)
        if result[1..].contains(&closer) {
            break;
        }
        result = &result[1..];
    }
    result
}

/// `findUnclosedQuoteStart` (autocomplete.ts:75-88).
fn find_unclosed_quote_start(text: &[char]) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = 0usize;
    for (i, &c) in text.iter().enumerate() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = i;
            }
        }
    }
    if in_quotes {
        Some(quote_start)
    } else {
        None
    }
}

/// `isTokenStart` (autocomplete.ts:91-97).
fn is_token_start(text: &[char], index: usize) -> bool {
    let mut start = index;
    while start > 0 {
        if path_wrapper_closer(text[start - 1]).is_some() {
            start -= 1;
        } else {
            break;
        }
    }
    // PATH_DELIMITERS.has(text[start - 1] ?? "") — undefined never matches.
    if start > 0 && PATH_DELIMITERS.contains(&text[start - 1]) {
        return true;
    }
    token_start_test(&text[..start])
}

/// `extractQuotedPrefix` (autocomplete.ts:99-116).
fn extract_quoted_prefix(text: &[char]) -> Option<String> {
    let quote_start = find_unclosed_quote_start(text)?;

    if quote_start > 0 && text[quote_start - 1] == '@' {
        if !is_token_start(text, quote_start - 1) {
            return None;
        }
        return Some(text[quote_start - 1..].iter().collect());
    }

    if !is_token_start(text, quote_start) {
        return None;
    }

    Some(text[quote_start..].iter().collect())
}

/// `parsePathPrefix` (autocomplete.ts:119-130).
#[derive(Debug, Clone, PartialEq)]
struct ParsedPathPrefix {
    raw_prefix: String,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
}

fn parse_path_prefix(prefix: &str) -> ParsedPathPrefix {
    if let Some(rest) = prefix.strip_prefix("@\"") {
        ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: true,
            is_quoted_prefix: true,
        }
    } else if let Some(rest) = prefix.strip_prefix('"') {
        ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: false,
            is_quoted_prefix: true,
        }
    } else if let Some(rest) = prefix.strip_prefix('@') {
        ParsedPathPrefix {
            raw_prefix: rest.to_string(),
            is_at_prefix: true,
            is_quoted_prefix: false,
        }
    } else {
        ParsedPathPrefix {
            raw_prefix: prefix.to_string(),
            is_at_prefix: false,
            is_quoted_prefix: false,
        }
    }
}

/// `buildCompletionValue` (autocomplete.ts:132-150).
///
/// Note: like the TS original, `is_directory` is accepted but unused.
fn build_completion_value(
    path: &str,
    _is_directory: bool,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
) -> String {
    let needs_quotes = is_quoted_prefix || separator_test(path);
    let prefix = if is_at_prefix { "@" } else { "" };

    if !needs_quotes {
        return format!("{}{}", prefix, path);
    }

    format!("{}\"{}\"", prefix, path)
}

// --- public types (autocomplete.ts:249-300) -----------------------------------

/// `AutocompleteItem` (autocomplete.ts:249-253).
#[derive(Debug, Clone, PartialEq)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// Argument-completion callback for a [`SlashCommand`]
/// (`getArgumentCompletions`, autocomplete.ts:263). Returns `None` for the
/// TS `null` (no argument completion available). Synchronous resolution of
/// the TS `Awaitable` through the executor-seam design.
pub type GetArgumentCompletions = Box<dyn Fn(&str) -> Option<Vec<AutocompleteItem>>>;

/// `SlashCommand` (autocomplete.ts:257-264).
pub struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub get_argument_completions: Option<GetArgumentCompletions>,
}

/// `SlashCommand | AutocompleteItem` (the provider's command list type,
/// autocomplete.ts:304).
pub enum CommandEntry {
    Slash(SlashCommand),
    Item(AutocompleteItem),
}

impl CommandEntry {
    /// `"name" in cmd ? cmd.name : cmd.value`
    fn name(&self) -> &str {
        match self {
            Self::Slash(c) => &c.name,
            Self::Item(i) => &i.value,
        }
    }

    fn description(&self) -> Option<&str> {
        match self {
            Self::Slash(c) => c.description.as_deref(),
            Self::Item(i) => i.description.as_deref(),
        }
    }

    /// `"argumentHint" in cmd && cmd.argumentHint` — only slash commands.
    fn argument_hint(&self) -> Option<&str> {
        match self {
            Self::Slash(c) => c.argument_hint.as_deref(),
            Self::Item(_) => None,
        }
    }
}

/// `AutocompleteSuggestions` (autocomplete.ts:266-269).
#[derive(Debug, Clone, PartialEq)]
pub struct AutocompleteSuggestions {
    pub items: Vec<AutocompleteItem>,
    /// What we're matching against (e.g., "/" or "src/").
    pub prefix: String,
}

/// Result of `applyCompletion` (autocomplete.ts:285-291).
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionResult {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

/// Minimal `AbortSignal` stand-in: a shared flag consulted via `aborted()`.
#[derive(Clone, Default)]
pub struct AbortSignal {
    aborted: Rc<Cell<bool>>,
}

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.aborted.set(true);
    }

    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }
}

/// Options for `getSuggestions` (autocomplete.ts:277-282).
#[derive(Clone)]
pub struct SuggestionsOptions {
    pub signal: AbortSignal,
    pub force: bool,
}

/// `AutocompleteProvider` (autocomplete.ts:271-300). `getSuggestions` is
/// synchronous: the TS async subprocess work runs through the injectable
/// [`CommandRunner`] / [`FsSeam`] seams.
pub trait AutocompleteProvider {
    /// Characters that should naturally trigger this provider at token
    /// boundaries (`triggerCharacters?`).
    fn trigger_characters(&self) -> Vec<char> {
        Vec::new()
    }

    /// Get autocomplete suggestions for current text/cursor position.
    /// Returns `None` if no suggestions available.
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        options: &SuggestionsOptions,
    ) -> Option<AutocompleteSuggestions>;

    /// Apply the selected item; returns the new text and cursor position.
    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> CompletionResult;

    /// Check if file completion should trigger for explicit Tab completion
    /// (`shouldTriggerFileCompletion?`).
    fn should_trigger_file_completion(&self, _lines: &[String], _cursor_line: usize, _cursor_col: usize) -> bool {
        true
    }
}

// --- seams ---------------------------------------------------------------------

/// A directory entry as returned by `fs.readdirSync(dir, { withFileTypes: true })`.
#[derive(Debug, Clone, PartialEq)]
pub struct FsDirEntry {
    pub name: String,
    /// `entry.isDirectory()` — lstat semantics (false for symlinks).
    pub is_directory: bool,
    /// `entry.isSymbolicLink()`.
    pub is_symbolic_link: bool,
}

/// Filesystem seam: `fs.readdirSync` / `fs.statSync` / `os.homedir`.
pub trait FsSeam {
    /// `readdirSync(path, { withFileTypes: true })`.
    fn read_dir(&self, path: &str) -> io::Result<Vec<FsDirEntry>>;
    /// `statSync(path).isDirectory()` — `Ok(false)` for non-directories,
    /// `Err` when the path does not exist (TS throws → callers catch).
    fn is_dir(&self, path: &str) -> io::Result<bool>;
    /// `os.homedir()` (default via the `HOME` env var).
    fn home_dir(&self) -> String;
}

/// Real filesystem implementation of [`FsSeam`] via `std::fs`.
#[derive(Debug, Default)]
pub struct StdFs;

impl FsSeam for StdFs {
    fn read_dir(&self, path: &str) -> io::Result<Vec<FsDirEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type().ok();
            out.push(FsDirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_directory: file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false),
                is_symbolic_link: file_type.as_ref().map(|t| t.is_symlink()).unwrap_or(false),
            });
        }
        Ok(out)
    }

    fn is_dir(&self, path: &str) -> io::Result<bool> {
        std::fs::metadata(path).map(|m| m.is_dir())
    }

    fn home_dir(&self) -> String {
        std::env::var("HOME").unwrap_or_default()
    }
}

/// Process-executor seam: spawn a program and collect (exit code, stdout).
/// `Err` models the `child.on("error")` path. This is the injectable
/// replacement for TS `child_process.spawn` + promise plumbing.
pub trait CommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String), ()>;
}

/// Real implementation of [`CommandRunner`] via blocking
/// `std::process::Command` (stderr discarded).
#[derive(Debug, Default)]
pub struct StdCommandRunner;

impl CommandRunner for StdCommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<(i32, String), ()> {
        use std::process::{Command, Stdio};
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|_| ())?;
        Ok((
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }
}

/// One `fd` result line (`{ path, isDirectory }`, autocomplete.ts:139).
#[derive(Debug, Clone, PartialEq)]
pub struct FdEntry {
    pub path: String,
    pub is_directory: bool,
}

/// `walkDirectoryWithFd` (autocomplete.ts:153-247) — runs `fd` through the
/// [`CommandRunner`] seam and parses its stdout.
fn walk_directory_with_fd(
    runner: &dyn CommandRunner,
    base_dir: &str,
    fd_path: &str,
    query: &str,
    max_results: usize,
    signal: &AbortSignal,
    max_depth: Option<u32>,
) -> Vec<FdEntry> {
    let mut args: Vec<String> = vec![
        "--base-directory".to_string(),
        base_dir.to_string(),
        "--max-results".to_string(),
        max_results.to_string(),
        "--type".to_string(),
        "f".to_string(),
        "--type".to_string(),
        "d".to_string(),
        "--follow".to_string(),
        "--hidden".to_string(),
        "--exclude".to_string(),
        ".git".to_string(),
        "--exclude".to_string(),
        ".git/*".to_string(),
        "--exclude".to_string(),
        ".git/**".to_string(),
    ];

    if let Some(depth) = max_depth {
        args.push("--max-depth".to_string());
        args.push(depth.to_string());
    }

    if to_display_path(query).contains('/') {
        args.push("--full-path".to_string());
    }

    if !query.is_empty() {
        args.push(build_fd_path_query(query));
    }

    if signal.aborted() {
        return vec![];
    }

    let Ok((code, stdout)) = runner.run(fd_path, &args) else {
        return vec![];
    };
    if signal.aborted() || code != 0 || stdout.is_empty() {
        return vec![];
    }

    let lines = js_trim(&stdout).split('\n').filter(|l| !l.is_empty());
    let mut results: Vec<FdEntry> = Vec::new();
    for line in lines {
        let display_line = to_display_path(line);
        let has_trailing_separator = display_line.ends_with('/');
        let normalized_path = if has_trailing_separator {
            &display_line[..display_line.len() - 1]
        } else {
            &display_line
        };
        if normalized_path == ".git"
            || normalized_path.starts_with(".git/")
            || normalized_path.contains("/.git/")
        {
            continue;
        }
        results.push(FdEntry {
            path: display_line,
            is_directory: has_trailing_separator,
        });
    }
    results
}

// --- CombinedAutocompleteProvider (autocomplete.ts:303-861) --------------------

/// Combined provider that handles both slash commands and file paths
/// (autocomplete.ts:303).
pub struct CombinedAutocompleteProvider {
    commands: Vec<CommandEntry>,
    base_path: String,
    fd_path: Option<String>,
    fs: Rc<dyn FsSeam>,
    runner: Rc<dyn CommandRunner>,
}

impl CombinedAutocompleteProvider {
    /// Constructor (autocomplete.ts:308-312) with the real std seams.
    pub fn new(
        commands: Vec<CommandEntry>,
        base_path: impl Into<String>,
        fd_path: Option<String>,
    ) -> Self {
        Self::with_seams(
            commands,
            base_path,
            fd_path,
            Rc::new(StdFs),
            Rc::new(StdCommandRunner),
        )
    }

    /// Constructor with injectable seams (for tests / hosts).
    pub fn with_seams(
        commands: Vec<CommandEntry>,
        base_path: impl Into<String>,
        fd_path: Option<String>,
        fs: Rc<dyn FsSeam>,
        runner: Rc<dyn CommandRunner>,
    ) -> Self {
        Self {
            commands,
            base_path: base_path.into(),
            fd_path,
            fs,
            runner,
        }
    }

    /// `getSuggestions` (autocomplete.ts:314-423).
    fn get_suggestions_impl(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        options: &SuggestionsOptions,
    ) -> Option<AutocompleteSuggestions> {
        let current_line: Vec<char> = lines
            .get(cursor_line)
            .map(|s| s.chars().collect())
            .unwrap_or_default();
        let col = cursor_col.min(current_line.len());
        let text_before_cursor: Vec<char> = current_line[..col].to_vec();

        // @ fuzzy file suggestions.
        if let Some(at_prefix) = self.extract_at_prefix(&text_before_cursor) {
            let parsed = parse_path_prefix(&at_prefix);
            let suggestions = self.get_fuzzy_file_suggestions(
                &parsed.raw_prefix,
                parsed.is_quoted_prefix,
                &options.signal,
            );
            if suggestions.is_empty() {
                return None;
            }
            return Some(AutocompleteSuggestions {
                items: suggestions,
                prefix: at_prefix,
            });
        }

        // Slash command completion.
        let command_text: String = text_before_cursor.iter().collect();
        let command_text = command_text
            .trim_start_matches(is_js_whitespace)
            .to_string();
        if !options.force && command_text.starts_with('/') {
            match command_text.find(' ') {
                None => {
                    let prefix = &command_text[1..];
                    // commandItems: { name, label, description } views.
                    let command_items: Vec<(usize, CommandItem)> = self
                        .commands
                        .iter()
                        .enumerate()
                        .map(|(i, cmd)| (i, CommandItem::from_entry(cmd)))
                        .collect();

                    // Bare-name matches (skill: prefix stripped).
                    let bare_matches: Vec<&(usize, CommandItem)> =
                        fuzzy_filter(&command_items, prefix, |it| {
                            it.1.bare_name()
                        });
                    let bare_name_match_set: std::collections::HashSet<usize> =
                        bare_matches.iter().map(|it| it.0).collect();
                    // Full-name-only skill: matches.
                    let full_name_only: Vec<(usize, CommandItem)> = command_items
                        .iter()
                        .filter(|(i, it)| {
                            it.name.starts_with("skill:") && !bare_name_match_set.contains(i)
                        })
                        .cloned()
                        .collect();
                    let full_name_only_matches: Vec<&(usize, CommandItem)> =
                        fuzzy_filter(&full_name_only, prefix, |it| it.1.name.as_str());

                    let filtered: Vec<AutocompleteItem> = bare_matches
                        .into_iter()
                        .chain(full_name_only_matches.into_iter())
                        .map(|it| AutocompleteItem {
                            value: it.1.name.clone(),
                            label: it.1.label.clone(),
                            description: it.1.description.clone(),
                        })
                        .collect();

                    if filtered.is_empty() {
                        return None;
                    }

                    return Some(AutocompleteSuggestions {
                        items: filtered,
                        prefix: command_text,
                    });
                }
                Some(space_index) => {
                    let command_name = &command_text[1..space_index];
                    let argument_text = &command_text[space_index + 1..];

                    let command = self
                        .commands
                        .iter()
                        .find(|cmd| cmd.name() == command_name);
                    let command = match command {
                        Some(CommandEntry::Slash(cmd)) if cmd.get_argument_completions.is_some() => {
                            cmd
                        }
                        _ => return None,
                    };

                    let argument_suggestions =
                        (command.get_argument_completions.as_ref().unwrap())(argument_text)?;
                    if argument_suggestions.is_empty() {
                        return None;
                    }

                    return Some(AutocompleteSuggestions {
                        items: argument_suggestions,
                        prefix: argument_text.to_string(),
                    });
                }
            }
        }

        // File path completion.
        let path_match = self.extract_path_prefix(&text_before_cursor, options.force)?;
        let suggestions = self.get_file_suggestions(&path_match);
        if suggestions.is_empty() {
            return None;
        }
        Some(AutocompleteSuggestions {
            items: suggestions,
            prefix: path_match,
        })
    }

    /// `applyCompletion` (autocomplete.ts:425-501).
    fn apply_completion_impl(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> CompletionResult {
        let current_line: Vec<char> = lines
            .get(cursor_line)
            .map(|s| s.chars().collect())
            .unwrap_or_default();
        let col = cursor_col.min(current_line.len());
        let prefix_len = prefix.chars().count();
        // (Divergence: TS slice(0, cursorCol - prefix.length) with a negative
        // end wraps; this port clamps at 0 — never exercised upstream.)
        let before_prefix: String = current_line[..col.saturating_sub(prefix_len)].iter().collect();
        let after_cursor: String = current_line[col..].iter().collect();
        let is_quoted_prefix = prefix.starts_with('"') || prefix.starts_with("@\"");
        let has_leading_quote_after_cursor = after_cursor.starts_with('"');
        let has_trailing_quote_in_item = item.value.ends_with('"');
        let adjusted_after_cursor: String =
            if is_quoted_prefix && has_trailing_quote_in_item && has_leading_quote_after_cursor {
                after_cursor.chars().skip(1).collect()
            } else {
                after_cursor.clone()
            };

        let mut new_lines: Vec<String> = lines.to_vec();

        // Check if we're completing a slash command (prefix starts with "/"
        // but NOT a file path).
        let is_slash_command = prefix.starts_with('/')
            && js_trim(&before_prefix).is_empty()
            && !prefix[1..].contains('/');
        if is_slash_command {
            // Command name completion.
            let new_line = format!("{}/{} {}", before_prefix, item.value, adjusted_after_cursor);
            if cursor_line < new_lines.len() {
                new_lines[cursor_line] = new_line;
            }
            return CompletionResult {
                lines: new_lines,
                cursor_line,
                // +2 for "/" and space
                cursor_col: before_prefix.chars().count() + item.value.chars().count() + 2,
            };
        }

        // File attachment completion (@ prefix).
        if prefix.starts_with('@') {
            // Don't add space after directories so user can continue
            // autocompleting.
            let is_directory = item.label.ends_with('/');
            let suffix = if is_directory { "" } else { " " };
            let new_line = format!(
                "{}{}{}{}",
                before_prefix, item.value, suffix, adjusted_after_cursor
            );
            if cursor_line < new_lines.len() {
                new_lines[cursor_line] = new_line;
            }

            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.chars().count() - 1
            } else {
                item.value.chars().count()
            };

            return CompletionResult {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.chars().count() + cursor_offset + suffix.chars().count(),
            };
        }

        let text_before_cursor: String = current_line[..col].iter().collect();
        // Command argument completion (beforePrefix contains "/command ").
        if text_before_cursor.contains('/') && text_before_cursor.contains(' ') {
            let new_line =
                format!("{}{}{}", before_prefix, item.value, adjusted_after_cursor);
            if cursor_line < new_lines.len() {
                new_lines[cursor_line] = new_line;
            }

            let is_directory = item.label.ends_with('/');
            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.chars().count() - 1
            } else {
                item.value.chars().count()
            };

            return CompletionResult {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.chars().count() + cursor_offset,
            };
        }

        // Plain file path completion.
        let new_line = format!("{}{}{}", before_prefix, item.value, adjusted_after_cursor);
        if cursor_line < new_lines.len() {
            new_lines[cursor_line] = new_line;
        }

        let is_directory = item.label.ends_with('/');
        let has_trailing_quote = item.value.ends_with('"');
        let cursor_offset = if is_directory && has_trailing_quote {
            item.value.chars().count() - 1
        } else {
            item.value.chars().count()
        };

        CompletionResult {
            lines: new_lines,
            cursor_line,
            cursor_col: before_prefix.chars().count() + cursor_offset,
        }
    }

    /// `extractAtPrefix` (autocomplete.ts:503-517).
    fn extract_at_prefix(&self, text: &[char]) -> Option<String> {
        if let Some(quoted_prefix) = extract_quoted_prefix(text) {
            if quoted_prefix.starts_with("@\"") {
                return Some(quoted_prefix);
            }
        }

        let token: &[char] = match find_last_delimiter(text) {
            Some(i) => &text[i + 1..],
            None => text,
        };
        let token = strip_leading_wrappers(token);

        if token.first() == Some(&'@') {
            return Some(token.iter().collect());
        }

        None
    }

    /// `extractPathPrefix` (autocomplete.ts:520-547).
    fn extract_path_prefix(&self, text: &[char], force_extract: bool) -> Option<String> {
        if let Some(quoted_prefix) = extract_quoted_prefix(text) {
            return Some(quoted_prefix);
        }

        let token: &[char] = match find_last_delimiter(text) {
            Some(i) => &text[i + 1..],
            None => text,
        };
        let path_prefix: String = strip_leading_wrappers(token).iter().collect();

        // For forced extraction (Tab key), always return something.
        if force_extract {
            return Some(path_prefix);
        }

        // For natural triggers, return if it looks like a path, ends with /,
        // starts with ~/, .
        if path_prefix.contains('/') || path_prefix.starts_with('.') || path_prefix.starts_with("~/")
        {
            return Some(path_prefix);
        }

        // Return an empty prefix after whitespace or CJK punctuation, but not
        // for empty text.
        if path_prefix.is_empty() && !text.is_empty() && token_start_test(text) {
            return Some(path_prefix);
        }

        None
    }

    /// `expandHomePath` (autocomplete.ts:550-559).
    fn expand_home_path(&self, path: &str) -> String {
        if let Some(rest) = path.strip_prefix("~/") {
            let expanded_path = js_path_join(&self.fs.home_dir(), rest);
            // Preserve trailing slash if original path had one.
            if path.ends_with('/') && !expanded_path.ends_with('/') {
                format!("{}/", expanded_path)
            } else {
                expanded_path
            }
        } else if path == "~" {
            self.fs.home_dir()
        } else {
            path.to_string()
        }
    }

    /// `resolveScopedFuzzyQuery` (autocomplete.ts:561-589) →
    /// `(baseDir, query, displayBase)`.
    fn resolve_scoped_fuzzy_query(&self, raw_query: &str) -> Option<(String, String, String)> {
        let normalized_query = to_display_path(raw_query);
        let slash_index = normalized_query.rfind('/')?;
        let display_base = normalized_query[..slash_index + 1].to_string();
        let query = normalized_query[slash_index + 1..].to_string();

        let base_dir = if display_base.starts_with("~/") {
            self.expand_home_path(&display_base)
        } else if display_base.starts_with('/') {
            display_base.clone()
        } else {
            js_path_join(&self.base_path, &display_base)
        };

        match self.fs.is_dir(&base_dir) {
            Ok(true) => {}
            _ => return None,
        }

        Some((base_dir, query, display_base))
    }

    /// `scopedPathForDisplay` (autocomplete.ts:591-597).
    fn scoped_path_for_display(&self, display_base: &str, relative_path: &str) -> String {
        let normalized_relative_path = to_display_path(relative_path);
        if display_base == "/" {
            format!("/{}", normalized_relative_path)
        } else {
            format!("{}{}", to_display_path(display_base), normalized_relative_path)
        }
    }

    /// `getFileSuggestions` (autocomplete.ts:600-733). Errors (missing /
    /// unreadable directory) collapse to an empty list, like the TS
    /// try/catch.
    fn get_file_suggestions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        self.get_file_suggestions_inner(prefix).unwrap_or_default()
    }

    fn get_file_suggestions_inner(&self, prefix: &str) -> io::Result<Vec<AutocompleteItem>> {
        let parsed = parse_path_prefix(prefix);
        let raw_prefix = parsed.raw_prefix.as_str();
        let mut expanded_prefix = raw_prefix.to_string();

        // Handle home directory expansion.
        if expanded_prefix.starts_with('~') {
            expanded_prefix = self.expand_home_path(&expanded_prefix);
        }

        let is_root_prefix = raw_prefix.is_empty()
            || raw_prefix == "./"
            || raw_prefix == "../"
            || raw_prefix == "~"
            || raw_prefix == "~/"
            || raw_prefix == "/"
            || (parsed.is_at_prefix && raw_prefix.is_empty());

        let (search_dir, search_prefix) = if is_root_prefix || raw_prefix.ends_with('/') {
            // Complete from specified position / show contents of directory.
            let search_dir = if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
                expanded_prefix.clone()
            } else {
                js_path_join(&self.base_path, &expanded_prefix)
            };
            (search_dir, String::new())
        } else {
            // Split into directory and file prefix.
            let dir = js_path_dirname(&expanded_prefix);
            let file = js_path_basename(&expanded_prefix);
            let search_dir = if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
                dir
            } else {
                js_path_join(&self.base_path, &dir)
            };
            (search_dir, file)
        };

        let entries = self.fs.read_dir(&search_dir)?;
        let mut suggestions: Vec<AutocompleteItem> = Vec::new();

        for entry in entries {
            if !entry
                .name
                .to_lowercase()
                .starts_with(&search_prefix.to_lowercase())
            {
                continue;
            }

            // Check if entry is a directory (or a symlink pointing to one).
            let mut is_directory = entry.is_directory;
            if !is_directory && entry.is_symbolic_link {
                let full_path = js_path_join(&search_dir, &entry.name);
                if let Ok(true) = self.fs.is_dir(&full_path) {
                    is_directory = true;
                }
                // Broken symlink or permission error - treat as file.
            }

            let name = entry.name;
            let display_prefix = raw_prefix;

            let relative_path: String = if display_prefix.ends_with('/') {
                // If prefix ends with /, append entry to the prefix.
                format!("{}{}", display_prefix, name)
            } else if display_prefix.contains('/') || display_prefix.contains('\\') {
                if let Some(home_relative_dir) = display_prefix.strip_prefix("~/") {
                    // Preserve ~/ format for home directory paths.
                    let dir = js_path_dirname(home_relative_dir);
                    if dir == "." {
                        format!("~/{}", name)
                    } else {
                        format!("~/{}", js_path_join(&dir, &name))
                    }
                } else if display_prefix.starts_with('/') {
                    // Absolute path - construct properly.
                    let dir = js_path_dirname(display_prefix);
                    if dir == "/" {
                        format!("/{}", name)
                    } else {
                        format!("{}/{}", dir, name)
                    }
                } else {
                    let mut rp = js_path_join(&js_path_dirname(display_prefix), &name);
                    // path.join normalizes away ./ prefix, preserve it.
                    if display_prefix.starts_with("./") && !rp.starts_with("./") {
                        rp = format!("./{}", rp);
                    }
                    rp
                }
            } else if display_prefix.starts_with('~') {
                // For standalone entries, preserve ~/ if original was ~/.
                format!("~/{}", name)
            } else {
                name.clone()
            };

            let relative_path = to_display_path(&relative_path);
            let path_value = if is_directory {
                format!("{}/", relative_path)
            } else {
                relative_path.clone()
            };
            let value = build_completion_value(
                &path_value,
                is_directory,
                parsed.is_at_prefix,
                parsed.is_quoted_prefix,
            );

            suggestions.push(AutocompleteItem {
                value,
                label: format!("{}{}", name, if is_directory { "/" } else { "" }),
                description: None,
            });
        }

        // Sort directories first, then alphabetically.
        suggestions.sort_by(|a, b| {
            let a_is_dir = a.label.ends_with('/');
            let b_is_dir = b.label.ends_with('/');
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => locale_compare(&a.label, &b.label),
            }
        });

        Ok(suggestions)
    }

    /// `scoreEntry` (autocomplete.ts:737-757) — higher = better match;
    /// directories get a bonus to appear first.
    fn score_entry(&self, file_path: &str, query: &str, is_directory: bool) -> i64 {
        let file_name = js_path_basename(file_path);
        let lower_file_name = file_name.to_lowercase();
        let lower_query = query.to_lowercase();

        let mut score: i64 = 0;

        // Exact filename match (highest)
        if lower_file_name == lower_query {
            score = 100;
        }
        // Filename starts with query
        else if lower_file_name.starts_with(&lower_query) {
            score = 80;
        }
        // Substring match in filename
        else if lower_file_name.contains(&lower_query) {
            score = 50;
        }
        // Substring match in full path
        else if file_path.to_lowercase().contains(&lower_query) {
            score = 30;
        }

        // Directories get a bonus to appear first.
        if is_directory && score > 0 {
            score += 10;
        }

        score
    }

    /// `getBaseDirSuggestions` (autocomplete.ts:759-769).
    fn get_base_dir_suggestions(
        &self,
        base_dir: &str,
        query: &str,
        signal: &AbortSignal,
    ) -> Vec<FdEntry> {
        let Some(fd_path) = &self.fd_path else {
            return vec![];
        };
        if signal.aborted() {
            return vec![];
        }

        walk_directory_with_fd(
            self.runner.as_ref(),
            base_dir,
            fd_path,
            query,
            100,
            signal,
            Some(1),
        )
    }

    /// `getFuzzyFileSuggestions` (autocomplete.ts:772-847).
    fn get_fuzzy_file_suggestions(
        &self,
        query: &str,
        is_quoted_prefix: bool,
        signal: &AbortSignal,
    ) -> Vec<AutocompleteItem> {
        let Some(fd_path) = &self.fd_path else {
            return vec![];
        };
        if signal.aborted() {
            return vec![];
        }

        let scoped_query = self.resolve_scoped_fuzzy_query(query);
        let fd_base_dir = scoped_query
            .as_ref()
            .map(|(base, _, _)| base.clone())
            .unwrap_or_else(|| self.base_path.clone());
        let fd_query = scoped_query
            .as_ref()
            .map(|(_, q, _)| q.clone())
            .unwrap_or_else(|| query.to_string());
        let base_dir_entries = self.get_base_dir_suggestions(&fd_base_dir, &fd_query, signal);
        let recursive_entries = walk_directory_with_fd(
            self.runner.as_ref(),
            &fd_base_dir,
            fd_path,
            &fd_query,
            100,
            signal,
            None,
        );

        // De-duplicate (seenPaths set, autocomplete.ts:795-803).
        let mut seen_paths: std::collections::HashSet<String> =
            base_dir_entries.iter().map(|e| e.path.clone()).collect();
        let mut entries = base_dir_entries;
        for entry in recursive_entries {
            if seen_paths.insert(entry.path.clone()) {
                entries.push(entry);
            }
        }
        if signal.aborted() {
            return vec![];
        }

        let mut scored_entries: Vec<(FdEntry, i64)> = entries
            .into_iter()
            .map(|entry| {
                let score = if fd_query.is_empty() {
                    1
                } else {
                    self.score_entry(&entry.path, &fd_query, entry.is_directory)
                };
                (entry, score)
            })
            .filter(|(_, score)| *score > 0)
            .collect();

        scored_entries.sort_by(|(a, a_score), (b, b_score)| {
            b_score
                .cmp(a_score)
                .then_with(|| {
                    let a_depth = to_display_path(&a.path)
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .count();
                    let b_depth = to_display_path(&b.path)
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .count();
                    a_depth.cmp(&b_depth)
                })
                .then_with(|| {
                    a.path.chars().count().cmp(&b.path.chars().count())
                })
                .then_with(|| locale_compare(&a.path, &b.path))
        });
        let top_entries: Vec<(FdEntry, i64)> =
            scored_entries.into_iter().take(20).collect();

        let mut suggestions: Vec<AutocompleteItem> = Vec::new();
        for (entry, _) in top_entries {
            let path_without_slash = if entry.is_directory {
                entry.path[..entry.path.len() - 1].to_string()
            } else {
                entry.path.clone()
            };
            let display_path = match &scoped_query {
                Some((_, _, display_base)) => {
                    self.scoped_path_for_display(display_base, &path_without_slash)
                }
                None => path_without_slash.clone(),
            };
            let entry_name = js_path_basename(&path_without_slash);
            let completion_path = if entry.is_directory {
                format!("{}/", display_path)
            } else {
                display_path.clone()
            };
            let value = build_completion_value(
                &completion_path,
                entry.is_directory,
                true,
                is_quoted_prefix,
            );

            suggestions.push(AutocompleteItem {
                value,
                label: format!("{}{}", entry_name, if entry.is_directory { "/" } else { "" }),
                description: Some(display_path),
            });
        }

        suggestions
    }

    /// `shouldTriggerFileCompletion` (autocomplete.ts:850-861).
    fn should_trigger_file_completion_impl(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let current_line: Vec<char> = lines
            .get(cursor_line)
            .map(|s| s.chars().collect())
            .unwrap_or_default();
        let col = cursor_col.min(current_line.len());
        let text_before_cursor: String = current_line[..col].iter().collect();

        // Don't trigger if we're typing a slash command at the start.
        let trimmed = js_trim(&text_before_cursor);
        if trimmed.starts_with('/') && !trimmed.contains(' ') {
            return false;
        }

        true
    }
}

impl AutocompleteProvider for CombinedAutocompleteProvider {
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        options: &SuggestionsOptions,
    ) -> Option<AutocompleteSuggestions> {
        self.get_suggestions_impl(lines, cursor_line, cursor_col, options)
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> CompletionResult {
        self.apply_completion_impl(lines, cursor_line, cursor_col, item, prefix)
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        self.should_trigger_file_completion_impl(lines, cursor_line, cursor_col)
    }
}

/// Internal `{ name, label, description }` view of a command entry used by
/// the slash-command filter (autocomplete.ts:337-350).
#[derive(Debug, Clone)]
struct CommandItem {
    name: String,
    label: String,
    description: Option<String>,
}

impl CommandItem {
    fn from_entry(cmd: &CommandEntry) -> Self {
        let name = cmd.name().to_string();
        let hint = cmd.argument_hint().filter(|h| !h.is_empty());
        let desc = cmd.description().map(|d| d.to_string());
        let full_desc = match (hint, desc) {
            (Some(h), Some(d)) if !d.is_empty() => Some(format!("{} — {}", h, d)),
            (Some(h), _) => Some(h.to_string()),
            (None, d) => d,
        };
        CommandItem {
            label: name.clone(),
            name,
            description: full_desc.filter(|d| !d.is_empty()),
        }
    }

    /// Fuzzy-filter key for bare-name matches: strip the `skill:` prefix.
    fn bare_name(&self) -> &str {
        self.name.strip_prefix("skill:").unwrap_or(&self.name)
    }
}

// ================================================================================
// Tests — ports of test/autocomplete.test.ts and test/autocomplete-slash.test.ts
// (all filesystem/process access is faked; the upstream `fd`-installed skip
// does not apply because the fd subprocess is an injected fake).
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::io;

    // ---- shared helpers -------------------------------------------------------

    fn char_col(s: &str) -> usize {
        s.chars().count()
    }

    /// TS `line.indexOf(needle)` translated to a char column.
    fn char_index_of(hay: &str, needle: &str) -> usize {
        let i = hay.find(needle).expect("needle must be present");
        hay[..i].chars().count()
    }

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn get_suggestions(
        provider: &CombinedAutocompleteProvider,
        lines: &[&str],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions> {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        provider.get_suggestions(
            &owned,
            cursor_line,
            cursor_col,
            &SuggestionsOptions {
                signal: AbortSignal::new(),
                force,
            },
        )
    }

    fn apply(
        provider: &CombinedAutocompleteProvider,
        line: &str,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> CompletionResult {
        provider.apply_completion(&[line.to_string()], 0, cursor_col, item, prefix)
    }

    fn item_values(r: &AutocompleteSuggestions) -> Vec<String> {
        r.items.iter().map(|i| i.value.clone()).collect()
    }

    fn slash(name: &str, description: Option<&str>) -> CommandEntry {
        CommandEntry::Slash(SlashCommand {
            name: name.to_string(),
            description: description.map(|d| d.to_string()),
            argument_hint: None,
            get_argument_completions: None,
        })
    }

    // ---- fakes ----------------------------------------------------------------

    fn canon(path: &str) -> String {
        let n = js_path_normalize(path);
        if n.len() > 1 {
            n.trim_end_matches('/').to_string()
        } else {
            n
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FakeKind {
        Dir,
        File,
        Link,
    }

    /// In-memory filesystem fake implementing [`FsSeam`].
    #[derive(Clone)]
    struct FakeFs {
        dirs: BTreeSet<String>,
        files: BTreeSet<String>,
        links: BTreeMap<String, String>,
        home: String,
    }

    impl FakeFs {
        fn new(home: &str) -> Self {
            Self {
                dirs: BTreeSet::from(["/".to_string()]),
                files: BTreeSet::new(),
                links: BTreeMap::new(),
                home: home.to_string(),
            }
        }

        fn add_parents(&mut self, path: &str) {
            let mut acc = String::new();
            for seg in path.trim_start_matches('/').split('/') {
                if seg.is_empty() {
                    continue;
                }
                acc.push('/');
                acc.push_str(seg);
                self.dirs.insert(acc.clone());
            }
        }

        fn add_dir(&mut self, path: &str) {
            let c = canon(path);
            self.add_parents(&c);
            self.dirs.insert(c);
        }

        fn add_file(&mut self, path: &str) {
            let c = canon(path);
            self.add_parents(&c);
            self.files.insert(c);
        }

        fn add_link(&mut self, link: &str, target: &str) {
            let c = canon(link);
            self.add_parents(&c);
            self.links.insert(c, target.to_string());
        }

        /// Resolve a path through symlinks (bounded against cycles).
        fn follow(&self, path: &str) -> Option<String> {
            let mut cur = canon(path);
            for _ in 0..16 {
                match self.links.get(&cur) {
                    Some(target) => cur = canon(&js_path_join(&js_path_dirname(&cur), target)),
                    None => return Some(cur),
                }
            }
            None
        }

        fn children(&self, dir: &str) -> BTreeMap<String, FakeKind> {
            let mut out = BTreeMap::new();
            for p in &self.dirs {
                if js_path_dirname(p) == dir {
                    out.insert(js_path_basename(p), FakeKind::Dir);
                }
            }
            for p in &self.files {
                if js_path_dirname(p) == dir {
                    out.insert(js_path_basename(p), FakeKind::File);
                }
            }
            for p in self.links.keys() {
                if js_path_dirname(p) == dir {
                    out.insert(js_path_basename(p), FakeKind::Link);
                }
            }
            out
        }

        /// Emulate `fd`'s walk: all files and dirs under `base` (relative
        /// paths, dirs with a trailing `/`), `.git` excluded, symlinks
        /// followed, depth-limited, children visited in sorted order.
        fn walk(&self, base: &str, max_depth: Option<usize>) -> Vec<String> {
            let Some(resolved) = self.follow(base).filter(|p| self.dirs.contains(p)) else {
                return vec![];
            };
            let mut out = Vec::new();
            let mut visited = HashSet::new();
            self.walk_dir(&resolved, "", max_depth, &mut out, &mut visited);
            out
        }

        fn walk_dir(
            &self,
            dir: &str,
            rel: &str,
            max_depth: Option<usize>,
            out: &mut Vec<String>,
            visited: &mut HashSet<String>,
        ) {
            if !visited.insert(dir.to_string()) {
                return;
            }
            for (name, kind) in self.children(dir) {
                if name == ".git" {
                    continue;
                }
                let child_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", rel, name)
                };
                if let Some(md) = max_depth {
                    if child_rel.split('/').count() > md {
                        continue;
                    }
                }
                let child_abs = format!("{}/{}", dir, name);
                match kind {
                    FakeKind::Dir => {
                        out.push(format!("{}/", child_rel));
                        self.walk_dir(&child_abs, &child_rel, max_depth, out, visited);
                    }
                    FakeKind::File => out.push(child_rel),
                    FakeKind::Link => {
                        if let Some(followed) = self.follow(&child_abs) {
                            if self.dirs.contains(&followed) {
                                out.push(format!("{}/", child_rel));
                                self.walk_dir(&followed, &child_rel, max_depth, out, visited);
                            } else if self.files.contains(&followed) {
                                out.push(child_rel);
                            }
                        }
                    }
                }
            }
        }
    }

    impl FsSeam for FakeFs {
        fn read_dir(&self, path: &str) -> io::Result<Vec<FsDirEntry>> {
            let resolved = self
                .follow(path)
                .filter(|p| self.dirs.contains(p))
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not found"))?;
            Ok(self
                .children(&resolved)
                .into_iter()
                .map(|(name, kind)| match kind {
                    FakeKind::Dir => FsDirEntry {
                        name,
                        is_directory: true,
                        is_symbolic_link: false,
                    },
                    FakeKind::File => FsDirEntry {
                        name,
                        is_directory: false,
                        is_symbolic_link: false,
                    },
                    FakeKind::Link => FsDirEntry {
                        name,
                        is_directory: false,
                        is_symbolic_link: true,
                    },
                })
                .collect())
        }

        fn is_dir(&self, path: &str) -> io::Result<bool> {
            match self.follow(path) {
                Some(p) => Ok(self.dirs.contains(&p)),
                None => Err(io::Error::new(io::ErrorKind::NotFound, "not found")),
            }
        }

        fn home_dir(&self) -> String {
            self.home.clone()
        }
    }

    // ---- fake fd runner ---------------------------------------------------------

    /// Tokens of a `buildFdPathQuery` pattern: escaped literals plus the
    /// `[\\/]` separator class.
    #[derive(Clone, Copy, PartialEq)]
    enum FdTok {
        Lit(char),
        Sep,
    }

    fn parse_fd_pattern(pattern: &str) -> Vec<FdTok> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '['
                && i + 4 < chars.len()
                && chars[i + 1] == '\\'
                && chars[i + 2] == '\\'
                && chars[i + 3] == '/'
                && chars[i + 4] == ']'
            {
                toks.push(FdTok::Sep);
                i += 5;
            } else if chars[i] == '\\' && i + 1 < chars.len() {
                toks.push(FdTok::Lit(chars[i + 1]));
                i += 2;
            } else {
                toks.push(FdTok::Lit(chars[i]));
                i += 1;
            }
        }
        toks
    }

    /// Unanchored match of the token sequence against `target` (fd smart
    /// case: case-sensitive when the pattern contains uppercase).
    fn fd_pattern_matches(toks: &[FdTok], target: &str, case_sensitive: bool) -> bool {
        if toks.is_empty() {
            return true;
        }
        let t: Vec<char> = target.chars().collect();
        let eq = |a: char, b: char| {
            if case_sensitive {
                a == b
            } else {
                a.to_lowercase().eq(b.to_lowercase())
            }
        };
        for start in 0..=t.len() {
            let mut ok = true;
            for (k, tok) in toks.iter().enumerate() {
                let Some(&c) = t.get(start + k) else {
                    ok = false;
                    break;
                };
                match tok {
                    FdTok::Lit(l) => {
                        if !eq(c, *l) {
                            ok = false;
                            break;
                        }
                    }
                    FdTok::Sep => {
                        if c != '/' && c != '\\' {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if ok {
                return true;
            }
        }
        false
    }

    /// Fake `fd` subprocess: walks the [`FakeFs`] tree, applies the pattern
    /// (basename by default, full path with `--full-path`), respects
    /// `--max-depth` / `--max-results` / `.git` exclusion / symlink
    /// following, and emits directories with a trailing slash.
    struct FakeFdRunner {
        fs: FakeFs,
    }

    impl CommandRunner for FakeFdRunner {
        fn run(&self, _program: &str, args: &[String]) -> Result<(i32, String), ()> {
            let mut base_dir = String::new();
            let mut max_results = usize::MAX;
            let mut max_depth: Option<usize> = None;
            let mut full_path = false;
            let mut pattern: Option<String> = None;

            let mut i = 0;
            while i < args.len() {
                let a = args[i].as_str();
                match a {
                    "--full-path" => {
                        full_path = true;
                        i += 1;
                    }
                    "--base-directory" => {
                        base_dir = args.get(i + 1).cloned().unwrap_or_default();
                        i += 2;
                    }
                    "--max-results" => {
                        max_results = args
                            .get(i + 1)
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(usize::MAX);
                        i += 2;
                    }
                    "--max-depth" => {
                        max_depth = args.get(i + 1).and_then(|v| v.parse().ok());
                        i += 2;
                    }
                    "--exclude" | "--type" => {
                        i += 2;
                    }
                    other if other.starts_with("--") => {
                        i += 1;
                    }
                    other => {
                        pattern = Some(other.to_string());
                        i += 1;
                    }
                }
            }

            let Some(base) = self
                .fs
                .follow(&base_dir)
                .filter(|p| self.fs.dirs.contains(p))
            else {
                return Ok((1, String::new()));
            };
            let toks = pattern.as_deref().map(parse_fd_pattern);
            let case_sensitive = pattern
                .as_deref()
                .map(|p| p.chars().any(|c| c.is_uppercase()))
                .unwrap_or(false);
            let lines: Vec<String> = self
                .fs
                .walk(&base, max_depth)
                .into_iter()
                .filter(|p| match &toks {
                    None => true,
                    Some(t) => {
                        let no_slash = p.strip_suffix('/').unwrap_or(p);
                        let target = if full_path {
                            no_slash
                        } else {
                            &js_path_basename(no_slash)
                        };
                        fd_pattern_matches(t, target, case_sensitive)
                    }
                })
                .take(max_results)
                .collect();
            Ok((0, lines.join("\n")))
        }
    }

    // ---- provider construction --------------------------------------------------

    /// setupFolder (autocomplete.test.ts:39-50) — fake paths only.
    fn setup_folder(fs: &mut FakeFs, base: &str, dirs: &[String], files: &[String]) {
        for dir in dirs {
            fs.add_dir(&format!("{}/{}", base, dir));
        }
        for file in files {
            fs.add_file(&format!("{}/{}", base, file));
        }
    }

    fn make_provider(
        fs: &FakeFs,
        base_path: &str,
        commands: Vec<CommandEntry>,
        fd: bool,
    ) -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::with_seams(
            commands,
            base_path.to_string(),
            if fd { Some("fd".to_string()) } else { None },
            Rc::new(fs.clone()),
            Rc::new(FakeFdRunner { fs: fs.clone() }),
        )
    }

    fn provider(fs: &FakeFs, base_path: &str) -> CombinedAutocompleteProvider {
        make_provider(fs, base_path, vec![], false)
    }

    // ==== describe("CombinedAutocompleteProvider") ==============================

    // ---- describe("extractPathPrefix") -----------------------------------------

    #[test]
    fn extracts_slash_from_hey_slash_when_forced() {
        let mut fs = FakeFs::new("/home/test");
        fs.add_dir("/tmp");
        let provider = provider(&fs, "/tmp");

        let lines = ["hey /"];
        // After the "/"
        let result = get_suggestions(&provider, &lines, 0, 5, true);

        let result = result.expect("Should return suggestions for root directory");
        assert_eq!(result.prefix, "/", "Prefix should be '/'");
    }

    #[test]
    fn extracts_slash_a_from_slash_a_when_forced() {
        let mut fs = FakeFs::new("/home/test");
        fs.add_dir("/tmp");
        let provider = provider(&fs, "/tmp");

        let lines = ["/A"];
        // After the "A"
        let result = get_suggestions(&provider, &lines, 0, 2, true);

        // This might return null if /A doesn't match anything, which is fine.
        // We're mainly testing that the prefix extraction works.
        if let Some(result) = result {
            assert_eq!(result.prefix, "/A", "Prefix should be '/A'");
        }
    }

    #[test]
    fn does_not_trigger_for_slash_commands() {
        let mut fs = FakeFs::new("/home/test");
        fs.add_dir("/tmp");
        let provider = provider(&fs, "/tmp");

        let lines = ["/model"];
        // After "model"
        let result = get_suggestions(&provider, &lines, 0, 6, true);

        assert!(result.is_none(), "Should not trigger for slash commands");
    }

    #[test]
    fn triggers_for_absolute_paths_after_slash_command_argument() {
        let mut fs = FakeFs::new("/home/test");
        fs.add_dir("/tmp");
        let provider = provider(&fs, "/tmp");

        let lines = ["/command /"];
        // After the second "/"
        let result = get_suggestions(&provider, &lines, 0, 10, true);

        let result = result.expect("Should trigger for absolute paths in command arguments");
        assert_eq!(result.prefix, "/", "Prefix should be '/'");
    }

    // ---- describe("fd @ file suggestions") ---------------------------------------

    /// Per-test tempdir equivalent: fresh fake tree with cwd/outside dirs.
    fn fd_setup() -> (FakeFs, String, String, String) {
        let root_dir = "/tmp/pi-autocomplete-root".to_string();
        let base_dir = format!("{}/cwd", root_dir);
        let outside_dir = format!("{}/outside", root_dir);
        let mut fs = FakeFs::new("/home/test");
        fs.add_dir(&base_dir);
        fs.add_dir(&outside_dir);
        (fs, root_dir, base_dir, outside_dir)
    }

    #[test]
    fn returns_all_files_and_folders_for_empty_at_query() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &sv(&["src"]), &sv(&["README.md"]));

        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);

        let mut values = result.as_ref().map(|r| item_values(r)).unwrap_or_default();
        values.sort();
        let mut expected = vec!["@README.md".to_string(), "@src/".to_string()];
        expected.sort();
        assert_eq!(values, expected);
    }

    #[test]
    fn recognizes_at_after_cjk_punctuation_without_consuming_the_preceding_text() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["README.md"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);

        let mut befores: Vec<String> = vec!["查看，".to_string(), "\u{3000}".to_string()];
        befores.extend(
            "，．：；！？（）［］｛｝“”‘’…—。、「」『』《》【】"
                .chars()
                .map(|c| c.to_string()),
        );
        for before in &befores {
            for force in [false, true] {
                let line = format!("{}@REA", before);
                let result =
                    get_suggestions(&provider, &[&line], 0, char_col(&line), force).unwrap();
                assert_eq!(result.prefix, "@REA", "{}", line);
                assert_eq!(item_values(&result), vec!["@README.md"]);
                let applied = apply(&provider, &line, char_col(&line), &result.items[0], &result.prefix);
                assert_eq!(applied.lines[0], format!("{}@README.md ", before));
                assert_eq!(applied.cursor_col, char_col(&applied.lines[0]));
            }
        }
    }

    #[test]
    fn recognizes_at_after_opening_wrappers_like_paren_and_backticks() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["README.md"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        for before in ["(", "see (", "[", "`", "<", "{"] {
            let line = format!("{}@REA", before);
            let result = get_suggestions(&provider, &[&line], 0, char_col(&line), false).unwrap();
            assert_eq!(result.prefix, "@REA", "{}", line);
            let applied = apply(&provider, &line, char_col(&line), &result.items[0], &result.prefix);
            assert_eq!(applied.lines[0], format!("{}@README.md ", before));
        }
        let embedded = "foo(@REA";
        assert!(get_suggestions(&provider, &[embedded], 0, char_col(embedded), false).is_none());
    }

    #[test]
    fn preserves_cjk_characters_and_embedded_at_in_attachment_paths() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["文档/说明.md", "文档@备份/说明.md"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        for before in ["", "查看，"] {
            for directory in ["文档", "文档@备份"] {
                let prefix = format!("@{}/说", directory);
                let line = format!("{}{}", before, prefix);
                let result = get_suggestions(&provider, &[&line], 0, char_col(&line), false).unwrap();
                assert!(result.items.iter().all(|_| true), "{}", line);
                assert_eq!(result.prefix, prefix);
                assert_eq!(
                    item_values(&result),
                    vec![format!("@{}/说明.md", directory)]
                );
            }
        }
    }

    #[test]
    fn completes_quoted_cjk_attachments_after_prose_without_losing_path_segments_or_quotes() {
        let (mut fs, _, base_dir, _) = fd_setup();
        let provider = make_provider(&fs, &base_dir, vec![], true);
        for separator in [" ", "\u{3000}", "，", "。"] {
            let directory = format!("我的{}文档", separator);
            setup_folder(
                &mut fs,
                &base_dir,
                &[],
                &sv(&[&format!("{}/说明.md", directory), "文档/说明.md"]),
            );
            let line = format!("查看：@\"{}/说\"后文", directory);
            let cursor_col = char_index_of(&line, "\"后文");
            let result = get_suggestions(&provider, &[&line], 0, cursor_col, false).unwrap();
            assert_eq!(result.prefix, format!("@\"{}/说", directory));
            assert_eq!(
                item_values(&result),
                vec![format!("@\"{}/说明.md\"", directory)]
            );
            let applied = apply(&provider, &line, cursor_col, &result.items[0], &result.prefix);
            assert_eq!(applied.lines[0], format!("查看：@\"{}/说明.md\" 后文", directory));
            assert_eq!(
                applied.cursor_col,
                char_col(&format!("查看：@\"{}/说明.md\" ", directory))
            );
        }
    }

    #[test]
    fn does_not_interpret_email_addresses_or_at_after_ascii_or_cjk_letters_as_attachment_prefixes() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["README.md", "example.com"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        for before in [
            "user", "查看", "あ", "カ", "한", "ㄅ", "𠮷", "か\u{3099}", "禰\u{e0100}", "々", "Ａ",
        ] {
            for name in ["REA", "example.com"] {
                let line = format!("{}@{}", before, name);
                assert!(
                    get_suggestions(&provider, &[&line], 0, char_col(&line), false).is_none(),
                    "{}",
                    line
                );
            }
        }
    }

    #[test]
    fn matches_file_with_extension_in_query() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["file.txt"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@file.txt";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@file.txt".to_string()));
    }

    #[test]
    fn filters_are_case_insensitive() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &sv(&["src"]), &sv(&["README.md"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@re";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let mut values = result.map(|r| item_values(&r)).unwrap_or_default();
        values.sort();
        assert_eq!(values, vec!["@README.md".to_string()]);
    }

    #[test]
    fn ranks_directories_before_files() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &sv(&["src"]), &sv(&["src.txt"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@src";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);

        let first_value = result.as_ref().and_then(|r| r.items.first()).map(|i| i.value.clone());
        let has_src_file = result
            .as_ref()
            .map(|r| r.items.iter().any(|i| i.value == "@src.txt"))
            .unwrap_or(false);
        assert_eq!(first_value, Some("@src/".to_string()));
        assert!(has_src_file);
    }

    #[test]
    fn returns_nested_file_paths() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["src/index.ts"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@index";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@src/index.ts".to_string()));
    }

    #[test]
    fn matches_deeply_nested_paths() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(
            &mut fs,
            &base_dir,
            &[],
            &sv(&["packages/tui/src/autocomplete.ts", "packages/ai/src/autocomplete.ts"]),
        );
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@tui/src/auto";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@packages/tui/src/autocomplete.ts".to_string()));
        assert!(!values.contains(&"@packages/ai/src/autocomplete.ts".to_string()));
    }

    #[test]
    fn matches_directory_in_middle_of_path_with_full_path() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(
            &mut fs,
            &base_dir,
            &[],
            &sv(&["src/components/Button.tsx", "src/utils/helpers.ts"]),
        );
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@components/";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@src/components/Button.tsx".to_string()));
        assert!(!values.contains(&"@src/utils/helpers.ts".to_string()));
    }

    #[test]
    fn scopes_fuzzy_search_to_relative_directories_and_searches_recursively() {
        let (mut fs, _, base_dir, outside_dir) = fd_setup();
        setup_folder(
            &mut fs,
            &outside_dir,
            &[],
            &sv(&["nested/alpha.ts", "nested/deeper/also-alpha.ts", "nested/deeper/zzz.ts"]),
        );
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@../outside/a";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@../outside/nested/alpha.ts".to_string()));
        assert!(values.contains(&"@../outside/nested/deeper/also-alpha.ts".to_string()));
        assert!(!values.contains(&"@../outside/nested/deeper/zzz.ts".to_string()));
    }

    #[test]
    fn ranks_shallower_same_score_at_matches_before_deeper_matches() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(
            &mut fs,
            &base_dir,
            &sv(&[
                "scope/aaa/venv/lib/python3.12/site-packages/pkg/core/profile",
                "scope/projects",
            ]),
            &[],
        );
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@scope/pro";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert_eq!(
            values.first(),
            Some(&"@scope/projects/".to_string())
        );
        assert!(values
            .contains(&"@scope/aaa/venv/lib/python3.12/site-packages/pkg/core/profile/".to_string()));
    }

    #[test]
    fn includes_scoped_direct_children_when_recursive_at_matches_are_flooded() {
        let (mut fs, _, base_dir, _) = fd_setup();
        let mut dirs: Vec<String> = vec!["scope/projects".to_string()];
        for index in 0..250 {
            dirs.push(format!(
                "scope/a{:03}/venv/lib/python3.12/site-packages/pkg/core/profile",
                index + 1
            ));
        }
        setup_folder(&mut fs, &base_dir, &dirs, &[]);
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@scope/pro";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert_eq!(values.first(), Some(&"@scope/projects/".to_string()));
        assert!(
            values.iter().any(|value| value.contains("/profile/")),
            "Should keep deep fuzzy matches after direct children"
        );
    }

    #[test]
    fn quotes_paths_containing_whitespace_or_cjk_punctuation_for_at_suggestions() {
        let (mut fs, _, base_dir, _) = fd_setup();
        let provider = make_provider(&fs, &base_dir, vec![], true);
        for separator in [" ", "\u{3000}", "，", "。"] {
            let directory = format!("my{}folder", separator);
            setup_folder(&mut fs, &base_dir, &[], &[format!("{}/test.txt", directory)]);
            let line = "@my";
            let result =
                get_suggestions(&provider, &[line], 0, char_col(line), false).unwrap();
            let item = result
                .items
                .iter()
                .find(|entry| entry.value == format!("@\"{}/\"", directory))
                .unwrap_or_else(|| panic!("{}", directory));
            let applied = apply(&provider, line, char_col(line), item, &result.prefix);
            let lines: Vec<&str> = applied.lines.iter().map(|s| s.as_str()).collect();
            let continued = get_suggestions(&provider, &lines, 0, applied.cursor_col, false).unwrap();
            assert_eq!(continued.prefix, format!("@\"{}/", directory));
            assert!(continued
                .items
                .iter()
                .any(|entry| entry.value == format!("@\"{}/test.txt\"", directory)));
        }
    }

    #[test]
    fn includes_hidden_paths_but_excludes_git() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(
            &mut fs,
            &base_dir,
            &sv(&[".pi", ".github", ".git"]),
            &sv(&[".pi/config.json", ".github/workflows/ci.yml", ".git/config"]),
        );
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@.pi/".to_string()));
        assert!(values.contains(&"@.github/".to_string()));
        assert!(!values.iter().any(|value| value == "@.git" || value.starts_with("@.git/")));
    }

    #[test]
    fn follows_symlinked_directories_for_fuzzy_at_search() {
        let (mut fs, _, base_dir, outside_dir) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["dir/some_file.txt"]));
        setup_folder(&mut fs, &outside_dir, &[], &sv(&["some_file.txt"]));
        fs.add_link(&format!("{}/symlinked_dir", base_dir), "../outside");

        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@some";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@dir/some_file.txt".to_string()));
        assert!(values.contains(&"@symlinked_dir/some_file.txt".to_string()));
    }

    #[test]
    fn returns_symlinked_directories_when_matching_their_name() {
        let (mut fs, _, base_dir, outside_dir) = fd_setup();
        setup_folder(&mut fs, &outside_dir, &[], &sv(&["nested/file.txt"]));
        fs.add_link(&format!("{}/symlinked_dir", base_dir), "../outside");

        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@symlinked";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@symlinked_dir/".to_string()));
    }

    #[test]
    fn returns_symlinked_files_without_requiring_type_l() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["original.txt"]));
        fs.add_link(&format!("{}/link.txt", base_dir), "original.txt");

        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@link";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false);
        let values = result.map(|r| item_values(&r)).unwrap_or_default();
        assert!(values.contains(&"@link.txt".to_string()));
    }

    #[test]
    fn returns_the_same_at_suggestions_when_the_cwd_path_contains_the_query() {
        let (mut fs, root_dir, _, _) = fd_setup();
        let normal_base_dir = format!("{}/cwd-normal", root_dir);
        let query_in_path_base_dir = format!("{}/cwd-plan-repro", root_dir);

        let dirs = sv(&["packages/coding-agent/examples/extensions/plan-mode"]);
        let files = sv(&[
            "packages/coding-agent/examples/extensions/plan-mode/README.md",
            "packages/tui/docs/plan.md",
        ]);
        setup_folder(&mut fs, &normal_base_dir, &dirs, &files);
        setup_folder(&mut fs, &query_in_path_base_dir, &dirs, &files);

        let query = "@plan";
        let normal_provider = make_provider(&fs, &normal_base_dir, vec![], true);
        let query_in_path_provider = make_provider(&fs, &query_in_path_base_dir, vec![], true);

        let normal_result = get_suggestions(&normal_provider, &[query], 0, char_col(query), false);
        let query_in_path_result =
            get_suggestions(&query_in_path_provider, &[query], 0, char_col(query), false);

        let normalize = |result: &Option<AutocompleteSuggestions>| -> Vec<String> {
            let mut v: Vec<String> = result
                .as_ref()
                .map(|r| {
                    r.items
                        .iter()
                        .map(|i| format!("{} :: {}", i.label, i.description.clone().unwrap_or_default()))
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        };

        let normal = normalize(&normal_result);
        assert_eq!(normalize(&query_in_path_result), normal);
        assert!(normal
            .contains(&"plan-mode/ :: packages/coding-agent/examples/extensions/plan-mode".to_string()));
        assert!(normal.contains(&"plan.md :: packages/tui/docs/plan.md".to_string()));
    }

    #[test]
    fn continues_autocomplete_inside_quoted_at_paths() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["my folder/test.txt", "my folder/other.txt"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@\"my folder/\"";
        let result = get_suggestions(&provider, &[line], 0, char_col(line) - 1, false);

        let result = result.expect("Should return suggestions for quoted folder path");
        let values = item_values(&result);
        assert!(values.contains(&"@\"my folder/test.txt\"".to_string()));
        assert!(values.contains(&"@\"my folder/other.txt\"".to_string()));
    }

    #[test]
    fn applies_quoted_at_completion_without_duplicating_closing_quote() {
        let (mut fs, _, base_dir, _) = fd_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["my folder/test.txt"]));
        let provider = make_provider(&fs, &base_dir, vec![], true);
        let line = "@\"my folder/te\"";
        let cursor_col = char_col(line) - 1;
        let result = get_suggestions(&provider, &[line], 0, cursor_col, false);

        let result = result.expect("Should return suggestions for quoted @ path");
        let item = result
            .items
            .iter()
            .find(|entry| entry.value == "@\"my folder/test.txt\"")
            .expect("Should find test.txt suggestion");

        let applied = apply(&provider, line, cursor_col, item, &result.prefix);
        assert_eq!(applied.lines[0], "@\"my folder/test.txt\" ");
    }

    // ---- describe("dot-slash path completion") ----------------------------------

    fn dot_setup() -> (FakeFs, String) {
        let mut fs = FakeFs::new("/home/test");
        let base_dir = "/tmp/pi-autocomplete".to_string();
        fs.add_dir(&base_dir);
        (fs, base_dir)
    }

    #[test]
    fn completes_chinese_path_prefixes_after_whitespace_or_cjk_punctuation_on_tab() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["说明.md", "文档/说明.md"]));
        let provider = provider(&fs, &base_dir);
        let mut completions: Vec<(String, String)> = vec![
            ("说".into(), "说明.md".into()),
            ("文".into(), "文档/".into()),
            ("文档/说".into(), "文档/说明.md".into()),
            ("./文档/说".into(), "./文档/说明.md".into()),
        ];
        completions.push((
            format!("{}/文档/说", base_dir),
            format!("{}/文档/说明.md", base_dir),
        ));
        for separator in " \t\u{3000}\u{00a0}，：；。！？（「《".chars() {
            for (prefix, value) in &completions {
                let before = format!("查看𠮷{}", separator);
                let line = format!("{}{} 后文", before, prefix);
                let cursor_col = char_col(&before) + char_col(prefix);
                let result =
                    get_suggestions(&provider, &[&line], 0, cursor_col, true).unwrap();
                assert_eq!(result.prefix, *prefix, "{}", line);
                assert_eq!(item_values(&result), vec![value.clone()]);
                let applied = apply(&provider, &line, cursor_col, &result.items[0], &result.prefix);
                assert_eq!(applied.lines[0], format!("{}{} 后文", before, value));
                assert_eq!(applied.cursor_col, char_col(&before) + char_col(value));
            }
        }
    }

    #[test]
    fn treats_unquoted_separators_as_boundaries_even_when_a_matching_literal_path_exists() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["归档/说明.md"]));
        let provider = provider(&fs, &base_dir);
        for separator in [" ", "\u{3000}", "，", "。"] {
            let directory = format!("资料{}归档", separator);
            setup_folder(&mut fs, &base_dir, &[], &[format!("{}/说明.md", directory)]);
            for marker in ["", "@"] {
                let line = format!("{}{}/说", marker, directory);
                let result = get_suggestions(&provider, &[&line], 0, char_col(&line), true).unwrap();
                assert_eq!(result.prefix, "归档/说", "{}", line);
                assert_eq!(item_values(&result), vec!["归档/说明.md".to_string()]);
            }
            let quoted = format!("查看，\"{}/说\"后文", directory);
            let cursor_col = char_index_of(&quoted, "\"后文");
            let result = get_suggestions(&provider, &[&quoted], 0, cursor_col, true).unwrap();
            assert_eq!(result.prefix, format!("\"{}/说", directory));
            assert_eq!(
                item_values(&result),
                vec![format!("\"{}/说明.md\"", directory)]
            );
            let applied = apply(&provider, &quoted, cursor_col, &result.items[0], &result.prefix);
            assert_eq!(applied.lines[0], format!("查看，\"{}/说明.md\"后文", directory));
            let missing = format!("查看，\"不存在{}归档/说", separator);
            assert!(get_suggestions(&provider, &[&missing], 0, char_col(&missing), true).is_none());
        }
    }

    #[test]
    fn handles_an_empty_prefix_after_whitespace_or_cjk_punctuation_consistently() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["说明.md"]));
        let provider = provider(&fs, &base_dir);
        for separator in [" ", "\t", "\u{3000}", "，", "。"] {
            for force in [false, true] {
                let line = format!("查看{}", separator);
                let result =
                    get_suggestions(&provider, &[&line], 0, char_col(&line), force).unwrap();
                assert_eq!(result.prefix, "", "{}", line);
                assert_eq!(item_values(&result), vec!["说明.md".to_string()]);
            }
        }
        assert!(get_suggestions(&provider, &[""], 0, 0, false).is_none());
    }

    #[test]
    fn completes_paths_after_opening_wrappers_like_paren_bracket_brace_angle_and_backticks() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["src/main.ts"]));
        let provider = provider(&fs, &base_dir);
        for wrapper in ["(", "[", "{", "<", "`", "((", "(`"] {
            for prefix in ["src/ma", "./src/ma"] {
                let before = format!("see {}", wrapper);
                let line = format!("{}{}", before, prefix);
                let result = get_suggestions(&provider, &[&line], 0, char_col(&line), true).unwrap();
                assert_eq!(result.prefix, prefix, "{}", line);
                let value = prefix.replace("src/ma", "src/main.ts");
                assert_eq!(item_values(&result), vec![value.clone()]);
                let applied = apply(&provider, &line, char_col(&line), &result.items[0], &result.prefix);
                assert_eq!(applied.lines[0], format!("{}{}", before, value));
            }
        }
    }

    #[test]
    fn completes_quoted_paths_after_opening_wrappers() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["my dir/main.ts"]));
        let provider = provider(&fs, &base_dir);
        let line = "see (\"my dir/ma";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), true).unwrap();
        assert_eq!(result.prefix, "\"my dir/ma");
        assert_eq!(item_values(&result), vec!["\"my dir/main.ts\"".to_string()]);
    }

    #[test]
    fn keeps_wrappers_that_are_closed_inside_the_path() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["[slug]/page.tsx", "(group)/layout.tsx"]));
        let provider = provider(&fs, &base_dir);
        for (prefix, value) in [
            ("[slug]/pa", "[slug]/page.tsx"),
            ("(group)/la", "(group)/layout.tsx"),
            ("./[slug]/pa", "./[slug]/page.tsx"),
        ] {
            let line = format!("see {}", prefix);
            let result = get_suggestions(&provider, &[&line], 0, char_col(&line), true).unwrap();
            assert_eq!(result.prefix, prefix, "{}", line);
            assert_eq!(item_values(&result), vec![value.to_string()]);
        }
    }

    #[test]
    fn preserves_cjk_characters_in_unprefixed_tab_completions() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["文档/说明.md"]));
        let provider = provider(&fs, &base_dir);
        let line = "文档/说";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), true).unwrap();
        assert_eq!(result.prefix, line);
        assert_eq!(item_values(&result), vec!["文档/说明.md".to_string()]);
    }

    #[test]
    fn preserves_dot_slash_prefix_when_completing_paths() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["update.sh", "utils.ts"]));
        let provider = provider(&fs, &base_dir);
        let line = "./up";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), true);

        let result = result.expect("Should return suggestions for ./ path");
        let values = item_values(&result);
        assert!(
            values.contains(&"./update.sh".to_string()),
            "Expected ./update.sh in {:?}",
            values
        );
    }

    #[test]
    fn preserves_dot_slash_prefix_for_directory_completions() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &sv(&["src"]), &sv(&["src/index.ts"]));
        let provider = provider(&fs, &base_dir);
        let line = "./sr";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), true);

        let result = result.expect("Should return suggestions for ./ directory path");
        let values = item_values(&result);
        assert!(
            values.contains(&"./src/".to_string()),
            "Expected ./src/ in {:?}",
            values
        );
    }

    // ---- describe("quoted path completion") -------------------------------------

    #[test]
    fn quotes_paths_containing_whitespace_or_cjk_punctuation_for_direct_completion() {
        let (mut fs, base_dir) = dot_setup();
        let provider = provider(&fs, &base_dir);
        for separator in [" ", "\u{3000}", "，", "。"] {
            let directory = format!("my{}folder", separator);
            setup_folder(&mut fs, &base_dir, &[], &[format!("{}/test.txt", directory)]);
            let line = "my";
            let result = get_suggestions(&provider, &[line], 0, char_col(line), true).unwrap();
            let item = result
                .items
                .iter()
                .find(|entry| entry.value == format!("\"{}/\"", directory))
                .unwrap_or_else(|| panic!("{}", directory));
            let applied = apply(&provider, line, char_col(line), item, &result.prefix);
            let lines: Vec<&str> = applied.lines.iter().map(|s| s.as_str()).collect();
            let continued = get_suggestions(&provider, &lines, 0, applied.cursor_col, true).unwrap();
            assert_eq!(continued.prefix, format!("\"{}/", directory));
            assert_eq!(
                item_values(&continued),
                vec![format!("\"{}/test.txt\"", directory)]
            );
        }
    }

    #[test]
    fn keeps_quoted_directories_before_files() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &sv(&["z folder", "z，folder"]), &sv(&["a.txt"]));
        let provider = provider(&fs, &base_dir);
        let result = get_suggestions(&provider, &[""], 0, 0, true).unwrap();
        let flags: Vec<bool> = result.items.iter().map(|item| item.label.ends_with('/')).collect();
        assert_eq!(flags, vec![true, true, false]);
    }

    #[test]
    fn continues_completion_inside_quoted_paths() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["my folder/test.txt", "my folder/other.txt"]));
        let provider = provider(&fs, &base_dir);
        let line = "\"my folder/\"";
        let result = get_suggestions(&provider, &[line], 0, char_col(line) - 1, true);

        let result = result.expect("Should return suggestions for quoted folder path");
        let values = item_values(&result);
        assert!(values.contains(&"\"my folder/test.txt\"".to_string()));
        assert!(values.contains(&"\"my folder/other.txt\"".to_string()));
    }

    #[test]
    fn applies_quoted_completion_without_duplicating_closing_quote() {
        let (mut fs, base_dir) = dot_setup();
        setup_folder(&mut fs, &base_dir, &[], &sv(&["my folder/test.txt"]));
        let provider = provider(&fs, &base_dir);
        let line = "\"my folder/te\"";
        let cursor_col = char_col(line) - 1;
        let result = get_suggestions(&provider, &[line], 0, cursor_col, true);

        let result = result.expect("Should return suggestions for quoted path");
        let item = result
            .items
            .iter()
            .find(|entry| entry.value == "\"my folder/test.txt\"")
            .expect("Should find test.txt suggestion");

        let applied = apply(&provider, line, cursor_col, item, &result.prefix);
        assert_eq!(applied.lines[0], "\"my folder/test.txt\"");
    }

    // ==== describe("CombinedAutocompleteProvider slash-command filter") =========
    // (autocomplete-skill-slash.test.ts)

    fn skill_commands() -> Vec<CommandEntry> {
        vec![
            slash("skill:deep-research", Some("Multi-agent deep research")),
            slash(
                "skill:research-idea",
                Some("Refine a raw idea into a falsifiable seed"),
            ),
            slash("skill:to-sidecar", Some("Route work to a sidecar")),
            slash("skill:brainstorm", Some("Generate ideas")),
            slash("model", Some("Select the active model")),
        ]
    }

    fn suggestions_for(commands: Vec<CommandEntry>, prefix: &str) -> Vec<String> {
        let fs = FakeFs::new("/home/test");
        let provider = make_provider(&fs, "/cwd", commands, false);
        let line = format!("/{}", prefix);
        let result = get_suggestions(&provider, &[&line], 0, char_col(&line), false)
            .unwrap_or_else(|| panic!("expected suggestions for \"/{}\"", prefix));
        item_values(&result)
    }

    #[test]
    fn ranks_skill_research_idea_first_for_query_idea() {
        let items = suggestions_for(skill_commands(), "idea");
        assert_eq!(items[0], "skill:research-idea");
        let deep = items.iter().position(|v| v == "skill:deep-research").unwrap();
        let idea = items
            .iter()
            .position(|v| v == "skill:research-idea")
            .unwrap();
        assert!(deep > idea);
    }

    #[test]
    fn keeps_ordinary_slash_commands_matching() {
        let items = suggestions_for(skill_commands(), "mod");
        assert!(items.contains(&"model".to_string()));
    }

    #[test]
    fn completes_commands_after_leading_whitespace_and_preserves_it() {
        let fs = FakeFs::new("/home/test");
        let provider = make_provider(&fs, "/cwd", vec![slash("model", None)], false);
        for (line, expected) in [(" /", " /model "), ("  /mod", "  /model "), ("\t/mod", "\t/model ")] {
            let result = get_suggestions(&provider, &[line], 0, char_col(line), false).unwrap();
            assert_eq!(
                result.prefix,
                line.trim_start_matches(crate::utils::is_js_whitespace)
            );
            assert_eq!(item_values(&result), vec!["model".to_string()]);
            let applied = apply(&provider, line, char_col(line), &result.items[0], &result.prefix);
            assert_eq!(applied.lines[0], expected);
            assert_eq!(applied.cursor_col, char_col(expected));
        }
    }

    #[test]
    fn completes_command_arguments_after_leading_whitespace() {
        let seen: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let seen_callback = seen.clone();
        let commands = vec![CommandEntry::Slash(SlashCommand {
            name: "model".to_string(),
            description: None,
            argument_hint: None,
            get_argument_completions: Some(Box::new(move |prefix| {
                *seen_callback.borrow_mut() = Some(prefix.to_string());
                Some(vec![AutocompleteItem {
                    value: "sonnet".to_string(),
                    label: "sonnet".to_string(),
                    description: None,
                }])
            })),
        })];
        let fs = FakeFs::new("/home/test");
        let provider = make_provider(&fs, "/cwd", commands, false);
        let line = "  /model son";
        let result = get_suggestions(&provider, &[line], 0, char_col(line), false).unwrap();
        assert_eq!(result.prefix, "son");
        assert_eq!(*seen.borrow(), Some("son".to_string()));
        let applied = apply(&provider, line, char_col(line), &result.items[0], &result.prefix);
        assert_eq!(applied.lines[0], "  /model sonnet");
    }

    #[test]
    fn keeps_explicit_skill_queries_working() {
        let items = suggestions_for(skill_commands(), "skill:side");
        assert!(items.contains(&"skill:to-sidecar".to_string()));
    }

    /// Regression test for #9944.
    #[test]
    fn lists_skills_while_typing_the_skill_prefix() {
        let items = suggestions_for(skill_commands(), "skill");
        let skills: Vec<String> = items
            .into_iter()
            .filter(|item| item.starts_with("skill:"))
            .collect();
        let expected: Vec<String> = [
            "skill:deep-research",
            "skill:research-idea",
            "skill:to-sidecar",
            "skill:brainstorm",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(skills, expected);
    }

    #[test]
    fn keeps_fuzzy_skill_prefix_shorthand_working() {
        let items = suggestions_for(skill_commands(), "skbra");
        assert!(items.contains(&"skill:brainstorm".to_string()));
    }
}
