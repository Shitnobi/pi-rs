//! Port of `packages/tui/src/alt-screen-search.ts` (327 LOC).
//!
//! Transcript search for the alt screen: builds a searchable corpus from
//! rendered transcript lines (whitespace-normalized, ANSI-stripped), finds
//! case-insensitive query matches, and maps them back to rendered
//! (row, column) segments. Also provides the search bar component
//! ([`AltScreenSearchComponent`]).
//!
//! Port notes:
//! - The crate is pure std — no regex crate. The TS regexes become explicit
//!   matchers (see [`find_search_corpus_matches`] and the JS-whitespace set in
//!   [`is_js_whitespace`]), following the `terminal_image.rs` no-regex pattern.
//! - TS string indices are UTF-16 code units; this port indexes the corpus by
//!   Unicode scalars (`char`s). The two agree for all BMP text, and because
//!   every astral grapheme gets its own span (one grapheme per span in the
//!   non-ASCII path), match→segment mapping is unaffected.
//! - JS `/iu` regex case folding is approximated with `char::to_lowercase`
//!   on both haystack and needle (identical for ASCII and the vast majority
//!   of Unicode; diverges only for exotic simple-case-folding pairs like
//!   `ſ`/`s`).
//! - The component mirrors the TS shape exactly (`onQueryChange` /
//!   `navigationButtonStyle` closures). The optional [`std::rc::Weak`] host
//!   handle (`set_host`) is port machinery for the alt-screen consumer: when
//!   set, a query change also calls `request_render` (the TS consumer does
//!   this inside its own `onQueryChange` closure; `None` by default keeps the
//!   observable behavior identical to TS).

use std::cell::RefCell;
use std::rc::Weak;

use crate::component::{Component, Focusable, TuiHost};
use crate::components::input::{Input, InputOptions};
use crate::keybindings::get_keybindings;
use crate::unicode::grapheme_segments;
use crate::utils::{strip_terminal_sequences, truncate_to_width, visible_width};

/// alt-screen-search.ts:8-15 — `SearchSourceSpan`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchSourceSpan {
    text_start: usize,
    text_end: usize,
    row: usize,
    start_col: usize,
    end_col: usize,
    linear_columns: bool,
}

/// alt-screen-search.ts:17-20 — `SearchCorpus`.
#[derive(Debug, Clone, Default)]
struct SearchCorpus {
    text: String,
    spans: Vec<SearchSourceSpan>,
}

/// alt-screen-search.ts:22-26 — `AltScreenSearchSegment`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltScreenSearchSegment {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// alt-screen-search.ts:28-30 — `AltScreenSearchMatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltScreenSearchMatch {
    pub segments: Vec<AltScreenSearchSegment>,
}

/// alt-screen-search.ts:150-153 — `AltScreenSearchResult`.
///
/// TS hands back a reference to the index's live matches array; the Rust port
/// returns a clone (matches are only ever read by consumers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltScreenSearchResult {
    pub matches: Vec<AltScreenSearchMatch>,
    pub changed: bool,
}

/// alt-screen-search.ts:32 — `PRINTABLE_ASCII = /^[\x20-\x7e]*$/`.
fn is_printable_ascii(line: &str) -> bool {
    // alt-screen-search.ts:32
    line.chars().all(|c| ('\u{20}'..='\u{7e}').contains(&c))
}

/// The ECMAScript `\s` character class (WhiteSpace + LineTerminator):
/// `\t\n\v\f\r`, space, NBSP, Zs, LS/PS, and ZWNBSP (`\ufeff`).
/// Used by the grapheme-path whitespace test `/^\s+$/u` (alt-screen-search.ts:83)
/// and `normalizeQuery`'s `/\s+/gu` + `trim()` (alt-screen-search.ts:109).
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
            | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// alt-screen-search.ts:34-106 — `buildSearchCorpus`.
fn build_search_corpus(lines: &[String]) -> SearchCorpus {
    // alt-screen-search.ts:34-106
    let mut chunks: Vec<String> = Vec::new();
    let mut spans: Vec<SearchSourceSpan> = Vec::new();
    let mut text_length = 0usize;
    let mut pending_separator = false;

    for (row, source) in lines.iter().enumerate() {
        // alt-screen-search.ts:48
        let line = strip_terminal_sequences(source);
        let mut column = 0usize;

        // Rendered transcripts are overwhelmingly ASCII. Index complete
        // non-space runs at once instead of segmenting and allocating one
        // mapping per cell. (alt-screen-search.ts:51-53)
        if is_printable_ascii(&line) {
            // alt-screen-search.ts:54-78
            let bytes = line.as_bytes();
            let mut index = 0usize;
            while index < bytes.len() {
                if bytes[index] == 0x20 {
                    if text_length > 0 {
                        pending_separator = true;
                    }
                    column += 1;
                    index += 1;
                    continue;
                }
                let mut end = index + 1;
                while end < bytes.len() && bytes[end] != 0x20 {
                    end += 1;
                }
                // appendSeparator (alt-screen-search.ts:40-45)
                if pending_separator {
                    chunks.push(" ".to_string());
                    text_length += 1;
                    pending_separator = false;
                }
                let text = line[index..end].to_string();
                spans.push(SearchSourceSpan {
                    text_start: text_length,
                    text_end: text_length + text.len(),
                    row,
                    start_col: column,
                    end_col: column + text.len(),
                    linear_columns: true,
                });
                chunks.push(text.clone());
                text_length += text.len();
                column += text.len();
                index = end;
            }
        } else {
            // alt-screen-search.ts:79-101
            for grapheme in grapheme_segments(&line) {
                let width = visible_width(grapheme);
                // `/^\s+$/u.test(text)` (alt-screen-search.ts:83)
                if grapheme.chars().all(is_js_whitespace) {
                    if text_length > 0 {
                        pending_separator = true;
                    }
                    column += width;
                    continue;
                }
                // appendSeparator (alt-screen-search.ts:40-45)
                if pending_separator {
                    chunks.push(" ".to_string());
                    text_length += 1;
                    pending_separator = false;
                }
                // TS `text.length` is UTF-16 units; char count here (see
                // module docs — segment mapping is unaffected).
                let char_len = grapheme.chars().count();
                spans.push(SearchSourceSpan {
                    text_start: text_length,
                    text_end: text_length + char_len,
                    row,
                    start_col: column,
                    end_col: column + width,
                    linear_columns: false,
                });
                chunks.push(grapheme.to_string());
                text_length += char_len;
                column += width;
            }
        }
        // alt-screen-search.ts:102
        if text_length > 0 {
            pending_separator = true;
        }
    }

    SearchCorpus {
        text: chunks.concat(),
        spans,
    }
}

/// alt-screen-search.ts:108-110 — `normalizeQuery` (`replace(/\s+/gu, " ").trim()`).
fn normalize_query(query: &str) -> String {
    // alt-screen-search.ts:108-110
    let mut out = String::with_capacity(query.len());
    let mut in_whitespace = false;
    for c in query.chars() {
        if is_js_whitespace(c) {
            in_whitespace = true;
        } else {
            if in_whitespace && !out.is_empty() {
                out.push(' ');
            }
            in_whitespace = false;
            out.push(c);
        }
    }
    // A separator is only ever pushed *before* a non-whitespace char, so the
    // result is already trimmed at both ends (empty for all-whitespace input).
    out
}

/// alt-screen-search.ts:112-114 — `escapeRegExp`.
///
/// With no regex engine, queries are matched literally, so escaping is a
/// no-op; kept as a documented identity for the source map.
fn escape_reg_exp(_text: &str) -> String {
    // alt-screen-search.ts:112-114
    String::new()
}

/// alt-screen-search.ts:116-148 — `findSearchCorpusMatches`.
///
/// The TS `new RegExp(escapeRegExp(query), "giu")` + `matchAll` becomes an
/// explicit case-insensitive literal scan: fold both haystack and needle with
/// `to_lowercase`, then find non-overlapping leftmost occurrences (exactly
/// `String.prototype.matchAll` semantics for non-empty patterns).
fn find_search_corpus_matches(
    corpus: &SearchCorpus,
    normalized_query: &str,
) -> Vec<AltScreenSearchMatch> {
    // alt-screen-search.ts:117
    if normalized_query.is_empty() {
        return Vec::new();
    }
    let _ = escape_reg_exp; // source-map anchor (see doc above)

    // Case-folded haystack with a map from folded positions back to original
    // char positions (a char may lower-case to multiple chars, e.g. `İ`).
    let haystack: Vec<char> = corpus.text.chars().collect();
    let mut folded: Vec<char> = Vec::with_capacity(haystack.len());
    let mut fold_map: Vec<usize> = Vec::with_capacity(haystack.len());
    for (index, c) in haystack.iter().enumerate() {
        for lowered in c.to_lowercase() {
            folded.push(lowered);
            fold_map.push(index);
        }
    }
    let needle: Vec<char> = normalized_query.to_lowercase().chars().collect();

    let mut matches: Vec<AltScreenSearchMatch> = Vec::new();
    let mut span_index = 0usize;
    let mut folded_index = 0usize;
    while folded_index + needle.len() <= folded.len() {
        if folded[folded_index..folded_index + needle.len()] == needle[..] {
            // alt-screen-search.ts:123-124
            let start = fold_map[folded_index];
            let end = fold_map[folded_index + needle.len() - 1] + 1;
            // alt-screen-search.ts:125
            while span_index < corpus.spans.len() && corpus.spans[span_index].text_end <= start {
                span_index += 1;
            }

            // alt-screen-search.ts:127-142
            let mut segments: Vec<AltScreenSearchSegment> = Vec::new();
            for index in span_index..corpus.spans.len() {
                let span = &corpus.spans[index];
                // alt-screen-search.ts:130
                if span.text_start >= end {
                    break;
                }
                // alt-screen-search.ts:131
                if span.text_end <= start {
                    continue;
                }
                // alt-screen-search.ts:132-135
                let start_col = if span.linear_columns {
                    span.start_col + start.max(span.text_start) - span.text_start
                } else {
                    span.start_col
                };
                let end_col = if span.linear_columns {
                    span.start_col + end.min(span.text_end) - span.text_start
                } else {
                    span.end_col
                };
                // alt-screen-search.ts:136-141
                match segments.last_mut() {
                    Some(previous)
                        if previous.row == span.row && start_col <= previous.end_col =>
                    {
                        previous.end_col = previous.end_col.max(end_col);
                    }
                    _ => segments.push(AltScreenSearchSegment {
                        row: span.row,
                        start_col,
                        end_col,
                    }),
                }
            }
            // alt-screen-search.ts:143
            while span_index < corpus.spans.len() && corpus.spans[span_index].text_end <= end {
                span_index += 1;
            }
            // alt-screen-search.ts:144
            if !segments.is_empty() {
                matches.push(AltScreenSearchMatch { segments });
            }
            folded_index += needle.len();
        } else {
            folded_index += 1;
        }
    }

    matches
}

/// alt-screen-search.ts:155-184 — `AltScreenSearchIndex`.
///
/// Cache the searchable corpus and matches while rendered transcript lines
/// remain unchanged.
#[derive(Debug, Clone, Default)]
pub struct AltScreenSearchIndex {
    source_lines: Option<Vec<String>>,
    corpus: Option<SearchCorpus>,
    normalized_query: Option<String>,
    matches: Vec<AltScreenSearchMatch>,
}

impl AltScreenSearchIndex {
    /// Default constructor (TS `new AltScreenSearchIndex()` — empty state).
    pub fn new() -> Self {
        Self::default()
    }

    /// alt-screen-search.ts:162-183 — `search(lines, query)`.
    pub fn search(&mut self, lines: &[String], query: &str) -> AltScreenSearchResult {
        // alt-screen-search.ts:163-170
        let mut source_changed = match &self.source_lines {
            Some(previous) => previous.len() != lines.len(),
            None => true,
        };
        if !source_changed {
            if let Some(previous) = &self.source_lines {
                for (index, line) in lines.iter().enumerate() {
                    if &previous[index] == line {
                        continue;
                    }
                    source_changed = true;
                    break;
                }
            }
        }
        // alt-screen-search.ts:171-174
        if source_changed || self.corpus.is_none() {
            self.source_lines = Some(lines.to_vec());
            self.corpus = Some(build_search_corpus(lines));
        }

        // alt-screen-search.ts:176-181
        let normalized = normalize_query(query);
        let changed = source_changed || self.normalized_query.as_deref() != Some(normalized.as_str());
        if changed {
            self.normalized_query = Some(normalized.clone());
            self.matches = find_search_corpus_matches(
                self.corpus.as_ref().expect("corpus built above"),
                &normalized,
            );
        }
        // alt-screen-search.ts:182
        AltScreenSearchResult {
            matches: self.matches.clone(),
            changed,
        }
    }
}

/// alt-screen-search.ts:186-189 — `findAltScreenSearchMatches`.
pub fn find_alt_screen_search_matches(
    lines: &[String],
    query: &str,
) -> Vec<AltScreenSearchMatch> {
    // alt-screen-search.ts:187-188
    let normalized_query = normalize_query(query);
    if normalized_query.is_empty() {
        Vec::new()
    } else {
        find_search_corpus_matches(&build_search_corpus(lines), &normalized_query)
    }
}

/// alt-screen-search.ts:191-195 — `getAltScreenSearchMatchKey`.
pub fn get_alt_screen_search_match_key(search_match: &AltScreenSearchMatch) -> String {
    // alt-screen-search.ts:192-194
    match (
        search_match.segments.first(),
        search_match.segments.last(),
    ) {
        (Some(first), Some(last)) => {
            format!("{}:{}:{}:{}", first.row, first.start_col, last.row, last.end_col)
        }
        _ => String::new(),
    }
}

/// TS `navigationButtonStyle: (text: string, hovered: boolean) => string`
/// (alt-screen-search.ts:204).
pub type NavigationButtonStyle = Box<dyn Fn(&str, bool) -> String>;

/// TS `onQueryChange: (query: string) => void` (alt-screen-search.ts:203).
pub type OnQueryChange = Box<dyn FnMut(&str)>;

/// alt-screen-search.ts:197-326 — `AltScreenSearchComponent`.
pub struct AltScreenSearchComponent {
    /// alt-screen-search.ts:198-202 — the wrapped `Input`.
    input: Input,
    /// alt-screen-search.ts:203
    on_query_change: OnQueryChange,
    /// alt-screen-search.ts:204 (defaults to identity, alt-screen-search.ts:216)
    navigation_button_style: NavigationButtonStyle,
    /// Port machinery (no TS counterpart): optional weak host so a query
    /// change can schedule a re-render. `None` keeps behavior identical to
    /// the TS component, whose consumer requests renders itself.
    host: Option<Weak<RefCell<dyn TuiHost>>>,
    /// alt-screen-search.ts:205
    result_count: i64,
    /// alt-screen-search.ts:206
    result_index: i64,
    /// alt-screen-search.ts:207-210 (-1 = "no hit region" sentinel)
    previous_button_start: i64,
    previous_button_end: i64,
    next_button_start: i64,
    next_button_end: i64,
    /// alt-screen-search.ts:211 (`-1 | 1 | undefined`)
    hovered_navigation_direction: Option<i64>,
    /// alt-screen-search.ts:212
    focused: bool,
}

impl AltScreenSearchComponent {
    /// alt-screen-search.ts:214-220 — constructor with the default identity
    /// `navigationButtonStyle` (alt-screen-search.ts:216).
    pub fn new(on_query_change: OnQueryChange) -> Self {
        // alt-screen-search.ts:198-202
        let input = Input::with_options(InputOptions {
            prompt: Some(" ".to_string()),
            placeholder: Some("Find in transcript".to_string()),
            placeholder_style: Some(Box::new(|text| format!("\x1b[2m{}\x1b[22m", text))),
        });
        AltScreenSearchComponent {
            input,
            on_query_change,
            navigation_button_style: Box::new(|text, _hovered| text.to_string()),
            host: None,
            result_count: 0,
            result_index: -1,
            previous_button_start: -1,
            previous_button_end: -1,
            next_button_start: -1,
            next_button_end: -1,
            hovered_navigation_direction: None,
            focused: false,
        }
    }

    /// alt-screen-search.ts:214-220 — constructor with an explicit
    /// `navigationButtonStyle`.
    pub fn with_navigation_button_style(
        on_query_change: OnQueryChange,
        navigation_button_style: NavigationButtonStyle,
    ) -> Self {
        let mut component = Self::new(on_query_change);
        component.navigation_button_style = navigation_button_style;
        component
    }

    /// Port machinery (no TS counterpart): install the weak host consulted on
    /// query changes. See the field docs above.
    pub fn set_host(&mut self, host: Weak<RefCell<dyn TuiHost>>) {
        self.host = Some(host);
    }

    /// alt-screen-search.ts:231-234 — `setResult(index, count)`.
    pub fn set_result(&mut self, index: i64, count: i64) {
        self.result_index = index;
        self.result_count = count;
    }

    /// alt-screen-search.ts:236-241 — `getNavigationDirectionAt(row, column)`.
    pub fn get_navigation_direction_at(&self, row: i64, column: i64) -> Option<i64> {
        // alt-screen-search.ts:237-240
        if row != 2 {
            return None;
        }
        if column >= self.previous_button_start && column < self.previous_button_end {
            return Some(-1);
        }
        if column >= self.next_button_start && column < self.next_button_end {
            return Some(1);
        }
        None
    }

    /// alt-screen-search.ts:243-247 — `setHoveredNavigationDirection`.
    pub fn set_hovered_navigation_direction(&mut self, direction: Option<i64>) -> bool {
        // alt-screen-search.ts:244-246
        if direction == self.hovered_navigation_direction {
            return false;
        }
        self.hovered_navigation_direction = direction;
        true
    }

    /// alt-screen-search.ts:249-254 — `handleInput(data)`.
    fn handle_query_input(&mut self, data: &str) {
        // alt-screen-search.ts:250-253
        let previous = self.input.get_value().to_string();
        Component::handle_input(&mut self.input, data);
        let query = self.input.get_value().to_string();
        if query != previous {
            (self.on_query_change)(&query);
            // Port machinery (see `host` docs): the TS consumer requests a
            // render from its own `onQueryChange` closure.
            if let Some(host) = self.host.as_ref().and_then(|w| w.upgrade()) {
                host.borrow_mut().request_render();
            }
        }
    }

    /// alt-screen-search.ts:260-326 — `render(width)`.
    fn render_search(&mut self, width: usize) -> Vec<String> {
        // alt-screen-search.ts:261-262
        let safe_width = width.max(1);
        let inner_width = (safe_width as i64 - 2).max(0);

        // alt-screen-search.ts:263-272 — `formatKey`
        let format_key = |key: Option<&str>| -> String {
            match key {
                Some(key) if !key.is_empty() => key
                    .split('+')
                    .map(|part| {
                        // `process.platform === "darwin"` (alt-screen-search.ts:268)
                        if cfg!(target_os = "macos") && part.eq_ignore_ascii_case("alt") {
                            "Option".to_string()
                        } else {
                            // `part.charAt(0).toUpperCase() + part.slice(1)`
                            let mut chars = part.chars();
                            match chars.next() {
                                Some(first) => {
                                    first.to_uppercase().collect::<String>() + chars.as_str()
                                }
                                None => String::new(),
                            }
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("+"),
                _ => "Unbound".to_string(),
            }
        };
        // alt-screen-search.ts:273-275
        let keybindings = get_keybindings();
        let previous_key =
            format_key(keybindings.get_keys("tui.altScreen.searchPrevious").first().map(String::as_str));
        let next_key =
            format_key(keybindings.get_keys("tui.altScreen.searchNext").first().map(String::as_str));

        // alt-screen-search.ts:276-281
        let query = self.input.get_value();
        let result = if query.is_empty() {
            String::new()
        } else if self.result_count == 0 {
            "No matches".to_string()
        } else {
            format!("{}/{}", self.result_index + 1, self.result_count)
        };

        // alt-screen-search.ts:282-288
        let result_space = (inner_width - 3).max(0) as usize;
        let visible_result = truncate_to_width(&result, result_space, "", false);
        let result_text = if visible_result.is_empty() {
            String::new()
        } else {
            format!("\x1b[2m {} \x1b[22m", visible_result)
        };
        let input_width = (inner_width - visible_width(&result_text) as i64).max(0) as usize;
        let input_line = {
            let rendered = Component::render(&mut self.input, input_width.max(1));
            truncate_to_width(
                rendered.first().map(String::as_str).unwrap_or(""),
                input_width,
                "",
                false,
            )
        };
        let input_padding =
            " ".repeat((input_width as i64 - visible_width(&input_line) as i64).max(0) as usize);
        let content = format!("{}{}{}", input_line, input_padding, result_text);

        // alt-screen-search.ts:290-301
        let mut previous_button = format!("↑ {}", previous_key);
        let mut next_button = format!("↓ {}", next_key);
        let mut separator = " · ".to_string();
        let outer_gap_width: i64 = 1;
        let available_controls_width = (inner_width - outer_gap_width * 2 - 1).max(0);
        let mut controls_width = visible_width(&previous_button) as i64
            + visible_width(&separator) as i64
            + visible_width(&next_button) as i64;
        if controls_width > available_controls_width {
            previous_button = "↑".to_string();
            next_button = "↓".to_string();
            separator = " ".to_string();
            controls_width = visible_width(&previous_button) as i64
                + visible_width(&separator) as i64
                + visible_width(&next_button) as i64;
        }

        // alt-screen-search.ts:302-318
        let show_buttons = controls_width <= available_controls_width;
        let rendered_buttons = if show_buttons {
            format!(
                "{}{}{}",
                (self.navigation_button_style)(
                    &previous_button,
                    self.hovered_navigation_direction == Some(-1)
                ),
                separator,
                (self.navigation_button_style)(
                    &next_button,
                    self.hovered_navigation_direction == Some(1)
                ),
            )
        } else {
            String::new()
        };
        let outer_gaps_width = if show_buttons {
            outer_gap_width * 2
        } else {
            0
        };
        let right_rule_width = if !rendered_buttons.is_empty()
            && inner_width > controls_width + outer_gaps_width
        {
            1
        } else {
            0
        };
        let left_rule_width = (inner_width
            - if show_buttons { controls_width } else { 0 }
            - outer_gaps_width
            - right_rule_width)
            .max(0);
        let previous_start = 1 + left_rule_width + outer_gap_width;
        self.previous_button_start = if show_buttons { previous_start } else { -1 };
        self.previous_button_end =
            if show_buttons { previous_start + visible_width(&previous_button) as i64 } else { -1 };
        self.next_button_start = if show_buttons {
            self.previous_button_end + visible_width(&separator) as i64
        } else {
            -1
        };
        self.next_button_end = if show_buttons {
            self.next_button_start + visible_width(&next_button) as i64
        } else {
            -1
        };

        // alt-screen-search.ts:320-326
        if safe_width == 1 {
            return vec!["┌".to_string(), "│".to_string(), "└".to_string()];
        }
        vec![
            format!("┌{}┐", "─".repeat(inner_width as usize)),
            format!("│{}│", content),
            format!(
                "└{}{}{}{}{}┘",
                "─".repeat(left_rule_width as usize),
                if rendered_buttons.is_empty() { "" } else { " " },
                rendered_buttons,
                if rendered_buttons.is_empty() { "" } else { " " },
                "─".repeat(right_rule_width as usize),
            ),
        ]
    }
}

impl Component for AltScreenSearchComponent {
    /// alt-screen-search.ts:260-326
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_search(width)
    }

    /// alt-screen-search.ts:249-254
    fn handle_input(&mut self, data: &str) {
        self.handle_query_input(data);
    }

    /// TS `component.handleInput` is present (alt-screen-search.ts:249).
    fn handles_input(&self) -> bool {
        true
    }

    /// alt-screen-search.ts:256-258
    fn invalidate(&mut self) {
        Component::invalidate(&mut self.input);
    }

    /// TS `"focused" in component` — this component implements `Focusable`
    /// (alt-screen-search.ts:197).
    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for AltScreenSearchComponent {
    /// alt-screen-search.ts:222-224
    fn focused(&self) -> bool {
        self.focused
    }

    /// alt-screen-search.ts:226-229
    fn set_focused(&mut self, value: bool) {
        self.focused = value;
        Focusable::set_focused(&mut self.input, value);
    }
}

// =============================================================================
// Tests — port of the AltScreenSearch cases in packages/tui/test/tui-alt-screen.test.ts
// (plus direct corpus/normalization/matcher unit tests for the no-regex port)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Component;
    use crate::keybindings::{
        set_keybindings, KeybindingsConfig, KeybindingsManager, tui_keybinding_definitions,
    };
    use crate::test_support::{FakeTui, global_test_lock};
    use crate::utils::{strip_terminal_sequences, visible_width};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn lines(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn strip_all(rendered: &[String]) -> Vec<String> {
        rendered.iter().map(|line| strip_terminal_sequences(line)).collect()
    }

    /// Install the default keybindings (the lazy `getKeybindings()` state) so
    /// render assertions are deterministic regardless of test ordering.
    fn reset_default_keybindings() {
        set_keybindings(KeybindingsManager::new(
            tui_keybinding_definitions(),
            KeybindingsConfig::default(),
        ));
    }

    // Direct corpus-building unit tests (no upstream counterpart — they pin
    // the no-regex port of alt-screen-search.ts:34-106).

    #[test]
    fn corpus_indexes_ascii_lines_as_word_spans() {
        // alt-screen-search.ts:34-106
        let _lock = global_test_lock();
        let corpus = build_search_corpus(&lines(&["foo  bar", " baz"]));
        assert_eq!(corpus.text, "foo bar baz");
        assert_eq!(corpus.spans.len(), 3);
        // "foo": text [0,3), columns [0,3), linear
        assert_eq!(
            corpus.spans[0],
            SearchSourceSpan {
                text_start: 0,
                text_end: 3,
                row: 0,
                start_col: 0,
                end_col: 3,
                linear_columns: true,
            }
        );
        // "bar": after two spaces, columns [5,8), linear
        assert_eq!(corpus.spans[1].start_col, 5);
        assert_eq!(corpus.spans[1].end_col, 8);
        assert_eq!(corpus.spans[1].text_start, 4);
        // "baz": new row, separator collapsed to one space
        assert_eq!(corpus.spans[2].row, 1);
        assert_eq!(corpus.spans[2].start_col, 1);
        assert_eq!(corpus.spans[2].text_start, 8);
    }

    #[test]
    fn corpus_strips_ansi_and_indexes_graphemes_with_display_widths() {
        // alt-screen-search.ts:48, 79-101
        let _lock = global_test_lock();
        let corpus = build_search_corpus(&lines(&["\x1b[31mA界🙂éZ\x1b[0m"]));
        assert_eq!(corpus.text, "A界🙂éZ");
        // One span per grapheme; column advances by visible width.
        assert_eq!(corpus.spans.len(), 5);
        assert_eq!(corpus.spans[1].start_col, 1);
        assert_eq!(corpus.spans[1].end_col, 3); // 界 is 2 columns wide
        assert_eq!(corpus.spans[2].start_col, 3);
        assert_eq!(corpus.spans[2].end_col, 5); // 🙂 is 2 columns wide
        assert_eq!(corpus.spans[4].end_col, 7);
        assert!(!corpus.spans[0].linear_columns);
    }

    #[test]
    fn corpus_collapses_leading_and_interleaved_whitespace_into_separators() {
        // alt-screen-search.ts:40-45, 56-57, 83-84. TS-verified against the
        // compiled module: "two" matches at row 0, cols 9-12.
        let _lock = global_test_lock();
        let corpus = build_search_corpus(&lines(&["   one\ttwo   ", "   ", "three"]));
        assert_eq!(corpus.text, "one two three");
        // The tab makes row 0 non-printable-ASCII, so it indexes per
        // grapheme: o, n, e, t, w, o (row 0) + "three" (row 2, ASCII fast
        // path) = 7 spans.
        assert_eq!(corpus.spans.len(), 7);
        // "two" letters start at the rendered column after three spaces,
        // "one" (3 cols) and the tab (visibleWidth("\t") == 3): column 9.
        assert_eq!(corpus.spans[3].start_col, 9);
        assert_eq!(corpus.spans[3].end_col, 10);
        assert_eq!(corpus.spans[5].end_col, 12);
        // The blank row contributes nothing.
        assert!(corpus.spans.iter().all(|span| span.row != 1));
    }

    #[test]
    fn normalize_query_collapses_whitespace_runs_and_trims() {
        // alt-screen-search.ts:108-110
        assert_eq!(normalize_query("  a \t\n b  "), "a b");
        assert_eq!(normalize_query("\u{a0}x\u{3000}y\u{feff}"), "x y");
        assert_eq!(normalize_query("   "), "");
    }

    // Port of tui-alt-screen.test.ts:596-605
    // "searches normalized rendered transcript text across rows"
    #[test]
    fn searches_normalized_rendered_transcript_text_across_rows() {
        // alt-screen-search.ts:186-189
        let _lock = global_test_lock();
        let matches = find_alt_screen_search_matches(
            &lines(&["alpha QUICK", "brown fox"]),
            "quick brown",
        );
        assert_eq!(
            matches,
            vec![AltScreenSearchMatch {
                segments: vec![
                    AltScreenSearchSegment { row: 0, start_col: 6, end_col: 11 },
                    AltScreenSearchSegment { row: 1, start_col: 0, end_col: 5 },
                ],
            }]
        );
    }

    // Port of tui-alt-screen.test.ts:607-618
    // "maps normalized ASCII and Unicode search matches back to rendered columns"
    #[test]
    fn maps_normalized_ascii_and_unicode_search_matches_back_to_rendered_columns() {
        // alt-screen-search.ts:186-189
        let _lock = global_test_lock();
        let matches = find_alt_screen_search_matches(
            &lines(&["\x1b[31mfoo  bar\x1b[0m", "A界🙂éZ"]),
            "oo   bar\nA界🙂é",
        );
        assert_eq!(
            matches,
            vec![AltScreenSearchMatch {
                segments: vec![
                    AltScreenSearchSegment { row: 0, start_col: 1, end_col: 3 },
                    AltScreenSearchSegment { row: 0, start_col: 5, end_col: 8 },
                    AltScreenSearchSegment { row: 1, start_col: 0, end_col: 6 },
                ],
            }]
        );
    }

    #[test]
    fn finds_case_insensitive_non_overlapping_multiple_matches() {
        // alt-screen-search.ts:116-148 (`giu` semantics)
        let _lock = global_test_lock();
        let matches = find_alt_screen_search_matches(&lines(&["needle one", "NEEDLE two"]), "needle");
        assert_eq!(matches.len(), 2);
        // matchAll is non-overlapping: "aaaa" contains "aa" twice.
        assert_eq!(find_alt_screen_search_matches(&lines(&["aaaa"]), "AA").len(), 2);
        // Queries are matched literally (TS escapes them first).
        assert_eq!(
            find_alt_screen_search_matches(&lines(&["a.b axb"]), "a.b").len(),
            1
        );
    }

    #[test]
    fn empty_and_whitespace_only_queries_return_no_matches() {
        // alt-screen-search.ts:117, 188
        let _lock = global_test_lock();
        assert!(find_alt_screen_search_matches(&lines(&["haystack"]), "").is_empty());
        assert!(find_alt_screen_search_matches(&lines(&["haystack"]), " \n ").is_empty());
    }

    #[test]
    fn match_key_uses_first_and_last_segment_extents() {
        // alt-screen-search.ts:191-195
        let _lock = global_test_lock();
        let matches = find_alt_screen_search_matches(
            &lines(&["alpha QUICK", "brown fox"]),
            "quick brown",
        );
        assert_eq!(get_alt_screen_search_match_key(&matches[0]), "0:6:1:5");
        // Single-segment match collapses to one row extent.
        let single = find_alt_screen_search_matches(&lines(&["hello"]), "hello");
        assert_eq!(get_alt_screen_search_match_key(&single[0]), "0:0:0:5");
        // No segments → empty key.
        assert_eq!(
            get_alt_screen_search_match_key(&AltScreenSearchMatch { segments: vec![] }),
            ""
        );
    }

    // Port of tui-alt-screen.test.ts:620-635
    // "reuses indexed transcript matches until the query or rendered lines change"
    #[test]
    fn reuses_indexed_transcript_matches_until_the_query_or_rendered_lines_change() {
        // alt-screen-search.ts:162-183
        let _lock = global_test_lock();
        let mut index = AltScreenSearchIndex::new();

        let initial = index.search(&lines(&["alpha needle", "omega"]), "needle");
        assert!(initial.changed);
        assert_eq!(initial.matches.len(), 1);

        let cached = index.search(&lines(&["alpha needle", "omega"]), "needle");
        assert!(!cached.changed);
        assert_eq!(cached.matches, initial.matches);

        let changed_query = index.search(&lines(&["alpha needle", "omega"]), "omega");
        assert!(changed_query.changed);
        assert_eq!(
            changed_query.matches[0].segments,
            vec![AltScreenSearchSegment { row: 1, start_col: 0, end_col: 5 }]
        );

        let changed_lines = index.search(&lines(&["alpha needle", "no match"]), "omega");
        assert!(changed_lines.changed);
        assert!(changed_lines.matches.is_empty());
    }

    #[test]
    fn index_detects_length_and_content_line_changes() {
        // alt-screen-search.ts:163-170
        let _lock = global_test_lock();
        let mut index = AltScreenSearchIndex::new();
        index.search(&lines(&["a", "b"]), "x");
        // Same lines + query → cached.
        assert!(!index.search(&lines(&["a", "b"]), "x").changed);
        // Length change.
        assert!(index.search(&lines(&["a"]), "x").changed);
        // Content change (and length change back).
        assert!(index.search(&lines(&["a", "c"]), "x").changed);
        assert!(!index.search(&lines(&["a", "c"]), "x").changed);
        // Whitespace-equivalent queries hit the same cache entry.
        assert!(!index.search(&lines(&["a", "c"]), "  x \t").changed);
    }

    // Port of tui-alt-screen.test.ts:637-660
    // "renders transcript search with a muted placeholder and right-aligned controls"
    #[test]
    fn renders_transcript_search_with_a_muted_placeholder_and_right_aligned_controls() {
        // alt-screen-search.ts:260-326
        let _lock = global_test_lock();
        reset_default_keybindings();
        let mut component = AltScreenSearchComponent::new(Box::new(|_query| {}));
        let rendered = Component::render(&mut component, 48);
        let stripped = strip_all(&rendered);

        assert_eq!(stripped.len(), 3);
        assert!(stripped.iter().all(|line| visible_width(line) == 48));
        // /^┌─+┐$/
        assert_eq!(stripped[0], format!("┌{}┐", "─".repeat(46)));
        // /^│ Find in transcript +│$/
        assert!(stripped[1].starts_with("│ Find in transcript"));
        assert!(stripped[1].ends_with("│"));
        let inner = stripped[1]
            .strip_prefix("│ Find in transcript")
            .and_then(|rest| rest.strip_suffix("│"))
            .expect("placeholder row shape");
        assert!(inner.chars().all(|c| c == ' '));
        // Muted placeholder (alt-screen-search.ts:201).
        assert!(rendered[1].contains("\x1b[2m"));
        // /^└─+ ↑ Shift+Enter · ↓ Enter ─┘$/
        assert_eq!(
            stripped[2],
            format!("└{} ↑ Shift+Enter · ↓ Enter ─┘", "─".repeat(20))
        );

        let controls = &stripped[2];
        // `getNavigationDirectionAt` takes a column; convert byte offsets to
        // columns by counting chars before the match.
        let col_of = |needle: &str| -> i64 {
            let byte = controls.find(needle).unwrap();
            controls[..byte].chars().count() as i64
        };
        let up = col_of("↑");
        let shift_enter = col_of("Shift+Enter");
        let dot = col_of("·");
        let down = col_of("↓");
        let last_enter_byte = controls.rfind("Enter").unwrap();
        let last_enter = controls[..last_enter_byte].chars().count() as i64;
        assert_eq!(component.get_navigation_direction_at(2, up), Some(-1));
        assert_eq!(component.get_navigation_direction_at(2, shift_enter + 5), Some(-1));
        assert_eq!(component.get_navigation_direction_at(2, dot), None);
        assert_eq!(component.get_navigation_direction_at(2, down), Some(1));
        assert_eq!(component.get_navigation_direction_at(2, last_enter + 2), Some(1));
        // Not the button row.
        assert_eq!(component.get_navigation_direction_at(1, up), None);

        // alt-screen-search.ts:641-649 (populated state)
        Component::handle_input(&mut component, "n");
        component.set_result(0, 2);
        let populated_render = Component::render(&mut component, 48);
        let populated = strip_all(&populated_render);
        assert!(populated[1].contains('n'));
        assert!(populated[1].contains("1/2"));
        assert!(populated_render[1].contains("\x1b[2m 1/2 \x1b[22m"));
        assert!(!populated.iter().any(|line| line.contains("Find in transcript")));
    }

    #[test]
    fn renders_no_matches_result_and_narrow_width_hides_buttons() {
        // alt-screen-search.ts:279-280, 296-302
        let _lock = global_test_lock();
        reset_default_keybindings();
        let mut component = AltScreenSearchComponent::new(Box::new(|_query| {}));
        Component::handle_input(&mut component, "zzz");
        component.set_result(-1, 0);
        let rendered = Component::render(&mut component, 48);
        let stripped = strip_all(&rendered);
        assert!(stripped[1].contains("No matches"));
        assert!(rendered[1].contains("\x1b[2m No matches \x1b[22m"));

        // Width 10: innerWidth 8, availableControlsWidth 5 < full controls →
        // buttons collapse to bare arrows (controlsWidth 3 ≤ 5) and stay
        // visible (TS-verified: ["┌────────┐", …, "└── ↑ ↓ ─┘"]).
        let narrow = strip_all(&Component::render(&mut component, 10));
        assert_eq!(narrow.len(), 3);
        assert!(narrow.iter().all(|line| visible_width(line) == 10));
        assert_eq!(narrow[2], "└── ↑ ↓ ─┘");
        // Collapsed buttons start at column 4 (1 + leftRule 2 + gap 1).
        assert_eq!(component.get_navigation_direction_at(2, 1), None);
        assert_eq!(component.get_navigation_direction_at(2, 4), Some(-1));
        assert_eq!(component.get_navigation_direction_at(2, 6), Some(1));

        // Width 7: innerWidth 5, availableControlsWidth 2 < collapsed
        // controlsWidth 3 → buttons hidden; hit regions cleared
        // (TS-verified: ["┌─────┐", …, "└─────┘"]).
        let hidden = strip_all(&Component::render(&mut component, 7));
        assert_eq!(hidden.len(), 3);
        assert!(hidden.iter().all(|line| visible_width(line) == 7));
        assert_eq!(hidden[2], "└─────┘");
        assert_eq!(component.get_navigation_direction_at(2, 1), None);
        assert_eq!(component.get_navigation_direction_at(2, 3), None);

        // Width 1 skeleton (alt-screen-search.ts:320).
        assert_eq!(
            Component::render(&mut component, 1),
            vec!["┌".to_string(), "│".to_string(), "└".to_string()]
        );
    }

    #[test]
    fn navigation_hover_toggles_and_styles_buttons() {
        // alt-screen-search.ts:243-247, 303-307
        let _lock = global_test_lock();
        reset_default_keybindings();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let recorder = calls.clone();
        let mut component = AltScreenSearchComponent::with_navigation_button_style(
            Box::new(|_query| {}),
            Box::new(move |text, hovered| {
                recorder
                    .borrow_mut()
                    .push((text.to_string(), hovered));
                format!("[{}|{}]", text, hovered)
            }),
        );
        let rendered = Component::render(&mut component, 48);
        assert!(rendered[2].contains("[↑ Shift+Enter|false]"));
        assert!(rendered[2].contains("[↓ Enter|false]"));

        assert!(component.set_hovered_navigation_direction(Some(1)));
        assert!(!component.set_hovered_navigation_direction(Some(1)));
        let hovered = Component::render(&mut component, 48);
        assert!(hovered[2].contains("[↑ Shift+Enter|false]"));
        assert!(hovered[2].contains("[↓ Enter|true]"));
        assert!(component.set_hovered_navigation_direction(None));
        // Recorded (text, hovered) pairs from both renders.
        let recorded = calls.borrow();
        assert_eq!(recorded.len(), 4);
        assert_eq!(recorded[2], ("↑ Shift+Enter".to_string(), false));
        assert_eq!(recorded[3], ("↓ Enter".to_string(), true));
    }

    #[test]
    fn query_change_callback_fires_only_when_the_value_changes() {
        // alt-screen-search.ts:249-254
        let _lock = global_test_lock();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let recorder = calls.clone();
        let mut component =
            AltScreenSearchComponent::new(Box::new(move |query| {
                recorder.borrow_mut().push(query.to_string());
            }));

        Component::handle_input(&mut component, "n");
        Component::handle_input(&mut component, "e");
        Component::handle_input(&mut component, "\x7f"); // backspace deletes "e"
        assert_eq!(*calls.borrow(), vec!["n".to_string(), "ne".to_string(), "n".to_string()]);
        assert_eq!(component.input.get_value(), "n");
    }

    #[test]
    fn focus_is_forwarded_to_the_inner_input() {
        // alt-screen-search.ts:197, 222-229
        let _lock = global_test_lock();
        let mut component = AltScreenSearchComponent::new(Box::new(|_query| {}));
        assert!(!component.focused());
        assert!(!Focusable::focused(&component.input));

        component.set_focused(true);
        assert!(component.focused());
        assert!(Focusable::focused(&component.input));

        component.set_focused(false);
        assert!(!component.focused());
        assert!(!Focusable::focused(&component.input));
    }

    #[test]
    fn as_focusable_exposes_the_component_and_handles_input() {
        // alt-screen-search.ts:197, 249 (Component surface)
        let _lock = global_test_lock();
        let mut component = AltScreenSearchComponent::new(Box::new(|_query| {}));
        assert!(component.handles_input());
        let focusable = Component::as_focusable(&mut component).expect("focusable");
        focusable.set_focused(true);
        assert!(component.focused());
    }

    #[test]
    fn weak_host_requests_render_on_query_change_when_set() {
        // Port machinery (no TS counterpart — see `host` field docs).
        let _lock = global_test_lock();
        let host: Rc<RefCell<FakeTui>> = Rc::new(RefCell::new(FakeTui::new(80, 24)));
        let weak = Rc::downgrade(&(host.clone() as Rc<RefCell<dyn crate::component::TuiHost>>));
        let mut component = AltScreenSearchComponent::new(Box::new(|_query| {}));
        component.set_host(weak);

        Component::handle_input(&mut component, "q");
        assert_eq!(host.borrow().render_requests, 1);

        // Same value (no change) → no extra render.
        Component::handle_input(&mut component, "\x08"); // backspace on empty selection is a no-op here
        host.borrow_mut().render_requests = 0;
        Component::handle_input(&mut component, "\x08");
        assert_eq!(host.borrow().render_requests, 0);
    }
}
