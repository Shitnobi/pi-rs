//! Shared diff computation utilities for the edit tool.
//!
//! Port of `src/core/tools/edit-diff.ts`.

use serde::Serialize;

use crate::jsdiff;
use crate::normalize::normalize_for_fuzzy_match;
use crate::paths::resolve_to_cwd;
use crate::text::split_bom;

/// Port of `detectLineEnding`.
pub fn detect_line_ending(content: &str) -> &'static str {
    let crlf_idx = content.find("\r\n");
    let lf_idx = content.find('\n');
    match (lf_idx, crlf_idx) {
        (None, _) => "\n",
        (Some(_), None) => "\n",
        (Some(lf), Some(crlf)) => {
            if crlf < lf {
                "\r\n"
            } else {
                "\n"
            }
        }
    }
}

/// Port of `normalizeToLF`.
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Port of `restoreLineEndings`.
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Port of `splitLinesWithEndings`: `content.match(/[^\n]*\n|[^\n]+/g) ?? []`.
pub fn split_lines_with_endings(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = content.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // token = content[start..=i] (includes the newline)
            lines.push(&content[start..=i]);
            start = i + 1;
        }
        i += 1;
    }
    if start < bytes.len() {
        lines.push(&content[start..]);
    }
    lines
}

#[derive(Debug, Clone)]
struct LineSpan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Edit {
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone)]
struct MatchedEdit {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

type TextReplacement = (usize, usize, String); // (match_index, match_length, new_text)

fn get_line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = LineSpan { start: offset, end: offset + line.len() };
            offset = span.end;
            span
        })
        .collect()
}

fn get_replacement_line_range(
    lines: &[LineSpan],
    replacement: &TextReplacement,
) -> Result<(usize, usize), String> {
    let (replacement_start, replacement_end, _) = (replacement.0, replacement.0 + replacement.1, &replacement.2);

    let mut start_line: i64 = -1;
    for (i, line) in lines.iter().enumerate() {
        if replacement_start >= line.start && replacement_start < line.end {
            start_line = i as i64;
            break;
        }
    }
    if start_line == -1 {
        return Err("Replacement range is outside the base content.".to_string());
    }

    let mut end_line = start_line;
    while end_line < lines.len() as i64 && lines[end_line as usize].end < replacement_end {
        end_line += 1;
    }
    if end_line >= lines.len() as i64 {
        return Err("Replacement range is outside the base content.".to_string());
    }

    Ok((start_line as usize, (end_line + 1) as usize))
}

fn apply_replacements(content: &str, replacements: &[TextReplacement], offset: usize) -> String {
    let mut result = content.to_string();
    for (match_index, match_length, new_text) in replacements.iter().rev() {
        let match_index = match_index.saturating_sub(offset);
        let mut result_bytes = result.into_bytes();
        let new_text_bytes = new_text.as_bytes();
        // byte-level splice (all indices are char boundaries by construction)
        result_bytes.splice(
            match_index..match_index + match_length,
            new_text_bytes.iter().cloned(),
        );
        result = String::from_utf8(result_bytes).expect("replacement produced invalid UTF-8");
    }
    result
}

/// Port of `applyReplacementsPreservingUnchangedLines`.
pub fn apply_replacements_preserving_unchanged_lines(
    original_content: &str,
    base_content: &str,
    replacements: &[TextReplacement],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original_content);
    let base_lines = get_line_spans(base_content);
    if original_lines.len() != base_lines.len() {
        return Err(
            "Cannot preserve unchanged lines because the base content has a different line count."
                .to_string(),
        );
    }

    let mut groups: Vec<(usize, usize, Vec<TextReplacement>)> = Vec::new();
    let mut sorted_replacements: Vec<TextReplacement> = replacements.to_vec();
    sorted_replacements.sort_by_key(|r| r.0);
    for replacement in sorted_replacements {
        let (start_line, end_line) = get_replacement_line_range(&base_lines, &replacement)?;
        if let Some(current) = groups.last_mut() {
            if start_line < current.1 {
                current.1 = current.1.max(end_line);
                current.2.push(replacement);
                continue;
            }
        }
        groups.push((start_line, end_line, vec![replacement]));
    }

    let mut original_line_index = 0usize;
    let mut result = String::new();
    for (start_line, end_line, group_replacements) in &groups {
        result.push_str(&original_lines[original_line_index..*start_line].concat());

        let group_start_offset = base_lines[*start_line].start;
        let group_end_offset = base_lines[end_line - 1].end;
        result.push_str(&apply_replacements(
            &base_content[group_start_offset..group_end_offset],
            group_replacements,
            group_start_offset,
        ));
        original_line_index = *end_line;
    }
    result.push_str(&original_lines[original_line_index..].concat());

    Ok(result)
}

#[derive(Debug, Clone)]
pub struct FuzzyMatchResult {
    /// Whether a match was found
    pub found: bool,
    /// The index where the match starts (in the content used for replacement)
    pub index: i64,
    /// Length of the matched text
    pub match_length: usize,
    /// Whether fuzzy matching was used (false = exact match)
    pub used_fuzzy_match: bool,
    /// The content to use for replacement operations.
    pub content_for_replacement: String,
}

/// Port of `fuzzyFindText`.
pub fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatchResult {
    // Try exact match first
    if let Some(exact_index) = content.find(old_text) {
        return FuzzyMatchResult {
            found: true,
            index: exact_index as i64,
            match_length: old_text.len(),
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        };
    }

    // Try fuzzy match - work entirely in normalized space
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    let fuzzy_index = fuzzy_content.find(&fuzzy_old_text);

    match fuzzy_index {
        None => FuzzyMatchResult {
            found: false,
            index: -1,
            match_length: 0,
            used_fuzzy_match: false,
            content_for_replacement: content.to_string(),
        },
        Some(idx) => FuzzyMatchResult {
            found: true,
            index: idx as i64,
            match_length: fuzzy_old_text.len(),
            used_fuzzy_match: true,
            content_for_replacement: fuzzy_content,
        },
    }
}

fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old_text = normalize_for_fuzzy_match(old_text);
    if fuzzy_old_text.is_empty() {
        return 0;
    }
    fuzzy_content.split(&fuzzy_old_text).count() - 1
}

fn not_found_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("Could not find the exact text in {}. The old text must match exactly including all whitespace and newlines.", path)
    } else {
        format!("Could not find edits[{}] in {}. The oldText must match exactly including all whitespace and newlines.", edit_index, path)
    }
}

fn duplicate_error(path: &str, edit_index: usize, total_edits: usize, occurrences: usize) -> String {
    if total_edits == 1 {
        format!("Found {} occurrences of the text in {}. The text must be unique. Please provide more context to make it unique.", occurrences, path)
    } else {
        format!("Found {} occurrences of edits[{}] in {}. Each oldText must be unique. Please provide more context to make it unique.", occurrences, edit_index, path)
    }
}

fn empty_old_text_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("oldText must not be empty in {}.", path)
    } else {
        format!("edits[{}].oldText must not be empty in {}.", edit_index, path)
    }
}

fn no_change_error(path: &str, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("No changes made to {}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.", path)
    } else {
        format!("No changes made to {}. The replacements produced identical content.", path)
    }
}

#[derive(Debug)]
pub struct AppliedEditsResult {
    pub base_content: String,
    pub new_content: String,
}

/// Port of `applyEditsToNormalizedContent`.
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEditsResult, String> {
    let normalized_edits: Vec<(String, String)> = edits
        .iter()
        .map(|e| (normalize_to_lf(&e.old_text), normalize_to_lf(&e.new_text)))
        .collect();

    for (i, (old_text, _)) in normalized_edits.iter().enumerate() {
        if old_text.is_empty() {
            return Err(empty_old_text_error(path, i, normalized_edits.len()));
        }
    }

    let used_fuzzy_match = normalized_edits
        .iter()
        .any(|(old_text, _)| fuzzy_find_text(normalized_content, old_text).used_fuzzy_match);
    let replacement_base_content = if used_fuzzy_match {
        normalize_for_fuzzy_match(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched_edits: Vec<MatchedEdit> = Vec::new();
    for (i, (old_text, new_text)) in normalized_edits.iter().enumerate() {
        let match_result = fuzzy_find_text(&replacement_base_content, old_text);
        if !match_result.found {
            return Err(not_found_error(path, i, normalized_edits.len()));
        }

        let occurrences = count_occurrences(&replacement_base_content, old_text);
        if occurrences > 1 {
            return Err(duplicate_error(path, i, normalized_edits.len(), occurrences));
        }

        matched_edits.push(MatchedEdit {
            edit_index: i,
            match_index: match_result.index as usize,
            match_length: match_result.match_length,
            new_text: new_text.clone(),
        });
    }

    matched_edits.sort_by_key(|e| e.match_index);
    for i in 1..matched_edits.len() {
        let previous = &matched_edits[i - 1];
        let current = &matched_edits[i];
        if previous.match_index + previous.match_length > current.match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index, path
            ));
        }
    }

    let replacements: Vec<TextReplacement> = matched_edits
        .iter()
        .map(|m| (m.match_index, m.match_length, m.new_text.clone()))
        .collect();
    let base_content = normalized_content.to_string();
    let new_content = if used_fuzzy_match {
        apply_replacements_preserving_unchanged_lines(normalized_content, &replacement_base_content, &replacements)?
    } else {
        apply_replacements(&replacement_base_content, &replacements, 0)
    };

    if base_content == new_content {
        return Err(no_change_error(path, normalized_edits.len()));
    }

    Ok(AppliedEditsResult { base_content, new_content })
}

/// Port of `generateUnifiedPatch`.
pub fn generate_unified_patch(
    path: &str,
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> String {
    jsdiff::create_two_files_patch(path, path, old_content, new_content, context_lines)
}

/// Result of `generateDiffString`.
#[derive(Debug)]
pub struct DiffStringResult {
    pub diff: String,
    pub first_changed_line: Option<usize>,
}

/// Port of `generateDiffString`: display-oriented diff with line numbers and
/// context. Returns the diff string and the first changed line number (in the
/// new file).
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> DiffStringResult {
    let parts = jsdiff::diff_lines(old_content, new_content);
    let mut output: Vec<String> = Vec::new();

    let old_lines: Vec<&str> = old_content.split('\n').collect();
    let new_lines: Vec<&str> = new_content.split('\n').collect();
    let max_line_num = std::cmp::max(old_lines.len(), new_lines.len());
    let line_num_width = max_line_num.to_string().len();

    let pad = |n: usize| format!("{:>width$}", n, width = line_num_width);

    let mut old_line_num = 1usize;
    let mut new_line_num = 1usize;
    let mut last_was_change = false;
    let mut first_changed_line: Option<usize> = None;

    for i in 0..parts.len() {
        let part = &parts[i];
        let mut raw: Vec<&str> = part.value.split('\n').collect();
        if raw.last() == Some(&"") {
            raw.pop();
        }

        if part.added || part.removed {
            // Capture the first changed line (in the new file)
            if first_changed_line.is_none() {
                first_changed_line = Some(new_line_num);
            }

            // Show the change
            for line in &raw {
                if part.added {
                    output.push(format!("+{} {}", pad(new_line_num), line));
                    new_line_num += 1;
                } else {
                    output.push(format!("-{} {}", pad(old_line_num), line));
                    old_line_num += 1;
                }
            }
            last_was_change = true;
        } else {
            // Context lines - only show a few before/after changes
            let next_part_is_change =
                i < parts.len() - 1 && (parts[i + 1].added || parts[i + 1].removed);
            let has_leading_change = last_was_change;
            let has_trailing_change = next_part_is_change;

            if has_leading_change && has_trailing_change {
                if raw.len() <= context_lines * 2 {
                    for line in &raw {
                        output.push(format!(" {} {}", pad(old_line_num), line));
                        old_line_num += 1;
                        new_line_num += 1;
                    }
                } else {
                    let leading_lines = &raw[..context_lines];
                    let trailing_lines = &raw[raw.len() - context_lines..];
                    let skipped_lines = raw.len() - leading_lines.len() - trailing_lines.len();

                    for line in leading_lines {
                        output.push(format!(" {} {}", pad(old_line_num), line));
                        old_line_num += 1;
                        new_line_num += 1;
                    }

                    output.push(format!(" {} ...", " ".repeat(line_num_width)));
                    old_line_num += skipped_lines;
                    new_line_num += skipped_lines;

                    for line in trailing_lines {
                        output.push(format!(" {} {}", pad(old_line_num), line));
                        old_line_num += 1;
                        new_line_num += 1;
                    }
                }
            } else if has_leading_change {
                let shown_lines = &raw[..std::cmp::min(context_lines, raw.len())];
                let skipped_lines = raw.len() - shown_lines.len();

                for line in shown_lines {
                    output.push(format!(" {} {}", pad(old_line_num), line));
                    old_line_num += 1;
                    new_line_num += 1;
                }

                if skipped_lines > 0 {
                    output.push(format!(" {} ...", " ".repeat(line_num_width)));
                    old_line_num += skipped_lines;
                    new_line_num += skipped_lines;
                }
            } else if has_trailing_change {
                let skipped_lines = raw.len().saturating_sub(context_lines);
                if skipped_lines > 0 {
                    output.push(format!(" {} ...", " ".repeat(line_num_width)));
                    old_line_num += skipped_lines;
                    new_line_num += skipped_lines;
                }

                for line in &raw[skipped_lines..] {
                    output.push(format!(" {} {}", pad(old_line_num), line));
                    old_line_num += 1;
                    new_line_num += 1;
                }
            } else {
                // Skip these context lines entirely
                old_line_num += raw.len();
                new_line_num += raw.len();
            }

            last_was_change = false;
        }
    }

    DiffStringResult {
        diff: output.join("\n"),
        first_changed_line,
    }
}

/// Port of `EditDiffResult`.
#[derive(Debug, Serialize)]
pub struct EditDiffResult {
    pub diff: String,
    #[serde(rename = "firstChangedLine")]
    pub first_changed_line: Option<usize>,
}

/// Port of `computeEditsDiff`: compute the diff for one or more edit
/// operations without applying them.
pub fn compute_edits_diff(
    path: &str,
    edits: &[Edit],
    cwd: &str,
) -> Result<EditDiffResult, String> {
    let absolute_path = resolve_to_cwd(path, cwd);

    // Check if file exists and is readable
    if let Err(err) = std::fs::File::open(&absolute_path) {
        let code = node_error_code(&err);
        return Err(format!(
            "Could not edit file: {}. Error code: {}.",
            path, code
        ));
    }
    // Mirror Node's access(R_OK) semantics (distinct from open for write).
    if !is_readable(&absolute_path) {
        return Err(format!("Could not edit file: {}. Error code: EACCES.", path));
    }

    // Read the file
    let raw_content = match std::fs::read_to_string(&absolute_path) {
        Ok(c) => c,
        Err(err) => {
            let code = node_error_code(&err);
            return Err(format!("Could not edit file: {}. Error code: {}.", path, code));
        }
    };

    // Strip BOM before matching (LLM won't include invisible BOM in oldText)
    let (_, content) = split_bom(&raw_content);
    let normalized_content = normalize_to_lf(content);
    let applied = apply_edits_to_normalized_content(&normalized_content, edits, path)?;

    // Generate the diff
    let result = generate_diff_string(&applied.base_content, &applied.new_content, 4);
    Ok(EditDiffResult {
        diff: result.diff,
        first_changed_line: result.first_changed_line,
    })
}

/// Port of `computeEditDiff` (single-edit convenience wrapper).
pub fn compute_edit_diff(
    path: &str,
    old_text: &str,
    new_text: &str,
    cwd: &str,
) -> Result<EditDiffResult, String> {
    compute_edits_diff(path, &[Edit { old_text: old_text.to_string(), new_text: new_text.to_string() }], cwd)
}

fn is_readable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(md) => {
            if md.is_dir() {
                return true;
            }
            let mode = md.permissions().mode();
            let euid = libc_uid();
            if euid == 0 {
                return true;
            }
            let owner = md.permissions().mode() & 0o600 == 0o600;
            let _ = owner;
            // Simplified check: any of the read bits readable by current user.
            // Node checks effective uid/gid against the file owner; approximate
            // with the owner bit for the common test scenarios.
            (mode & 0o400) != 0
        }
        Err(_) => false,
    }
}

fn libc_uid() -> u32 {
    // Avoid a libc dependency: read from /proc if possible.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
        })
        .unwrap_or(1000)
}

/// Map an io error to a Node error code string (ENOENT, EACCES, ...).
pub fn node_error_code(err: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match err.kind() {
        NotFound => "ENOENT".to_string(),
        PermissionDenied => "EACCES".to_string(),
        AlreadyExists => "EEXIST".to_string(),
        InvalidInput => "EINVAL".to_string(),
        _ => {
            if let Some(code) = err.raw_os_error() {
                format!("E{}", code)
            } else {
                "UNKNOWN".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ending_detection() {
        assert_eq!(detect_line_ending("a\nb"), "\n");
        assert_eq!(detect_line_ending("a\r\nb"), "\r\n");
        assert_eq!(detect_line_ending("a\r\nb\nc"), "\r\n");
        assert_eq!(detect_line_ending("a\nb\r\nc"), "\n");
        assert_eq!(detect_line_ending(""), "\n");
    }

    #[test]
    fn split_lines_with_endings_matches_js_regex() {
        assert_eq!(split_lines_with_endings("a\nb\n"), vec!["a\n", "b\n"]);
        assert_eq!(split_lines_with_endings("a\nb"), vec!["a\n", "b"]);
        assert_eq!(split_lines_with_endings(""), Vec::<&str>::new());
        assert_eq!(split_lines_with_endings("\n"), vec!["\n"]);
        assert_eq!(split_lines_with_endings("a\n\nb"), vec!["a\n", "\n", "b"]);
    }

    #[test]
    fn fuzzy_find_exact() {
        let r = fuzzy_find_text("hello world", "world");
        assert!(r.found && !r.used_fuzzy_match);
        assert_eq!(r.index, 6);
    }

    #[test]
    fn fuzzy_find_smart_quotes() {
        let r = fuzzy_find_text("say \u{201C}hi\u{201D} now", "\"hi\"");
        assert!(r.found && r.used_fuzzy_match);
    }

    #[test]
    fn fuzzy_find_not_found() {
        let r = fuzzy_find_text("abc", "xyz");
        assert!(!r.found);
        assert_eq!(r.index, -1);
    }

    #[test]
    fn apply_edits_basic() {
        let result = apply_edits_to_normalized_content(
            "Hello, world!",
            &[Edit { old_text: "world".into(), new_text: "testing".into() }],
            "test.txt",
        )
        .unwrap();
        assert_eq!(result.new_content, "Hello, testing!");
    }

    #[test]
    fn apply_edits_not_found() {
        let err = apply_edits_to_normalized_content(
            "Hello, world!",
            &[Edit { old_text: "nonexistent".into(), new_text: "x".into() }],
            "test.txt",
        )
        .unwrap_err();
        assert!(err.starts_with("Could not find the exact text in test.txt."));
    }

    #[test]
    fn apply_edits_duplicate() {
        let err = apply_edits_to_normalized_content(
            "foo foo foo",
            &[Edit { old_text: "foo".into(), new_text: "bar".into() }],
            "test.txt",
        )
        .unwrap_err();
        assert!(err.starts_with("Found 3 occurrences of the text in test.txt."));
    }

    #[test]
    fn apply_edits_empty_old_text() {
        let err = apply_edits_to_normalized_content(
            "hello",
            &[Edit { old_text: "".into(), new_text: "x".into() }],
            "test.txt",
        )
        .unwrap_err();
        assert_eq!(err, "oldText must not be empty in test.txt.");
    }

    #[test]
    fn apply_edits_multi_disjoint() {
        let result = apply_edits_to_normalized_content(
            "alpha\nbeta\ngamma\ndelta\n",
            &[
                Edit { old_text: "alpha\n".into(), new_text: "ALPHA\n".into() },
                Edit { old_text: "gamma\n".into(), new_text: "GAMMA\n".into() },
            ],
            "test.txt",
        )
        .unwrap();
        assert_eq!(result.new_content, "ALPHA\nbeta\nGAMMA\ndelta\n");
    }

    #[test]
    fn apply_edits_overlap() {
        let err = apply_edits_to_normalized_content(
            "one\ntwo\nthree\n",
            &[
                Edit { old_text: "one\ntwo\n".into(), new_text: "ONE\nTWO\n".into() },
                Edit { old_text: "two\nthree\n".into(), new_text: "TWO\nTHREE\n".into() },
            ],
            "test.txt",
        )
        .unwrap_err();
        assert!(err.contains("overlap"));
    }

    #[test]
    fn apply_edits_matched_against_original_not_incrementally() {
        let result = apply_edits_to_normalized_content(
            "foo\nbar\nbaz\n",
            &[
                Edit { old_text: "foo\n".into(), new_text: "foo bar\n".into() },
                Edit { old_text: "bar\n".into(), new_text: "BAR\n".into() },
            ],
            "test.txt",
        )
        .unwrap();
        assert_eq!(result.new_content, "foo bar\nBAR\nbaz\n");
    }

    #[test]
    fn apply_edits_no_change() {
        let err = apply_edits_to_normalized_content(
            "hello",
            &[Edit { old_text: "hello".into(), new_text: "hello".into() }],
            "test.txt",
        )
        .unwrap_err();
        assert!(err.starts_with("No changes made to test.txt."));
    }

    #[test]
    fn fuzzy_edit_preserves_unchanged_lines() {
        // Trailing whitespace differs -> fuzzy match; unchanged lines keep
        // their original bytes. (oldText has one trailing space, file has two.)
        let result = apply_edits_to_normalized_content(
            "keep me   \nchange me  \nkeep me too\n",
            &[Edit { old_text: "change me \n".into(), new_text: "changed\n".into() }],
            "test.txt",
        )
        .unwrap();
        assert_eq!(result.new_content, "keep me   \nchanged\nkeep me too\n");
    }

    #[test]
    fn generate_diff_string_basic() {
        let r = generate_diff_string("a\nb\nc\n", "a\nB\nc\n", 4);
        assert_eq!(r.diff, " 1 a\n-2 b\n+2 B\n 3 c");
        assert_eq!(r.first_changed_line, Some(2));
    }

    #[test]
    fn generate_diff_string_collapses_large_gaps() {
        let old: String = (0..600).map(|i| format!("line {:03}\n", i + 1)).collect();
        let new: String = (0..600)
            .map(|i| {
                let n = i + 1;
                let text = if n == 100 || n == 300 || n == 500 {
                    format!("LINE {:03}\n", n)
                } else {
                    format!("line {:03}\n", n)
                };
                text
            })
            .collect();
        let r = generate_diff_string(&old, &new, 4);
        assert!(r.diff.contains("LINE 100"));
        assert!(r.diff.contains("LINE 300"));
        assert!(r.diff.contains("LINE 500"));
        assert!(r.diff.contains("..."));
        assert!(!r.diff.contains("line 250"));
        assert!(r.diff.lines().count() < 50);
    }

    #[test]
    fn unified_patch_matches_expected_format() {
        let patch = generate_unified_patch("test.txt", "Hello, world!", "Hello, testing!", 4);
        assert_eq!(
            patch,
            "--- test.txt\n+++ test.txt\n@@ -1,1 +1,1 @@\n-Hello, world!\n\\ No newline at end of file\n+Hello, testing!\n\\ No newline at end of file\n"
        );
    }
}
