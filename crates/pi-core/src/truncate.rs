//! Shared truncation utilities for tool outputs.
//!
//! Port of `src/core/tools/truncate.ts`. Truncation is based on two independent
//! limits — whichever is hit first wins:
//! - Line limit (default: 2000 lines)
//! - Byte limit (default: 50KB)
//!
//! Never returns partial lines (except bash tail truncation edge case).

use serde::Serialize;

pub const DEFAULT_MAX_LINES: u64 = 2000;
pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024; // 50KB
/// Max chars per grep match line (`GREP_MAX_LINE_LENGTH`).
pub const GREP_MAX_LINE_LENGTH: usize = 500;

#[derive(Debug, Clone, Copy, Default)]
pub struct TruncationOptions {
    /// Maximum number of lines (default: 2000)
    pub max_lines: Option<u64>,
    /// Maximum number of bytes (default: 50KB)
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TruncationResult {
    /// The truncated content
    pub content: String,
    /// Whether truncation occurred
    pub truncated: bool,
    /// Which limit was hit: "lines", "bytes", or null if not truncated
    #[serde(rename = "truncatedBy")]
    pub truncated_by: Option<String>,
    /// Total number of lines in the original content
    #[serde(rename = "totalLines")]
    pub total_lines: u64,
    /// Total number of bytes in the original content
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    /// Number of complete lines in the truncated output
    #[serde(rename = "outputLines")]
    pub output_lines: u64,
    /// Number of bytes in the truncated output
    #[serde(rename = "outputBytes")]
    pub output_bytes: u64,
    /// Whether the last line was partially truncated (tail truncation edge case)
    #[serde(rename = "lastLinePartial")]
    pub last_line_partial: bool,
    /// Whether the first line exceeded the byte limit (head truncation)
    #[serde(rename = "firstLineExceedsLimit")]
    pub first_line_exceeds_limit: bool,
    /// The max lines limit that was applied
    #[serde(rename = "maxLines")]
    pub max_lines: u64,
    /// The max bytes limit that was applied
    #[serde(rename = "maxBytes")]
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MiddleTruncationResult {
    pub content: String,
    pub truncated: bool,
    #[serde(rename = "removedChars")]
    pub removed_chars: usize,
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    #[serde(rename = "totalLines")]
    pub total_lines: u64,
}

/// JS `content.split("\n")` with the trailing empty element popped when the
/// content ends with a newline; empty content yields no lines.
fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Format bytes as human-readable size. Port of `formatSize`.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{}KB", crate::js_string::js_to_fixed_1(bytes as f64 / 1024.0))
    } else {
        format!(
            "{}MB",
            crate::js_string::js_to_fixed_1(bytes as f64 / (1024.0 * 1024.0))
        )
    }
}

fn not_truncated(content: &str, total_lines: u64, total_bytes: u64, max_lines: u64, max_bytes: u64) -> TruncationResult {
    TruncationResult {
        content: content.to_string(),
        truncated: false,
        truncated_by: None,
        total_lines,
        total_bytes,
        output_lines: total_lines,
        output_bytes: total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate content from the head (keep first N lines/bytes).
/// Port of `truncateHead`.
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);

    let total_bytes = content.len() as u64;
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len() as u64;

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return not_truncated(content, total_lines, total_bytes, max_lines, max_bytes);
    }

    // Check if first line alone exceeds byte limit
    let first_line_bytes = lines[0].len() as u64;
    if first_line_bytes > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some("bytes".to_string()),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            last_line_partial: false,
            first_line_exceeds_limit: true,
            max_lines,
            max_bytes,
        };
    }

    // Collect complete lines that fit
    let mut output_lines_arr: Vec<&str> = Vec::new();
    let mut output_bytes_count: u64 = 0;
    let mut truncated_by = "lines";

    let line_cap = std::cmp::min(lines.len() as u64, max_lines) as usize;
    for (i, &line) in lines.iter().take(line_cap).enumerate() {
        // +1 for newline on all but the first line
        let line_bytes = line.len() as u64 + if i > 0 { 1 } else { 0 };

        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }

        output_lines_arr.push(line);
        output_bytes_count += line_bytes;
    }

    // If we exited due to line limit
    if output_lines_arr.len() as u64 >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = "lines";
    }

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = output_content.len() as u64;

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by.to_string()),
        total_lines,
        total_bytes,
        output_lines: output_lines_arr.len() as u64,
        output_bytes: final_output_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate content from the tail (keep last N lines/bytes).
/// Port of `truncateTail`.
pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);

    let total_bytes = content.len() as u64;
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len() as u64;

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return not_truncated(content, total_lines, total_bytes, max_lines, max_bytes);
    }

    // Work backwards from the end
    let mut output_lines_arr: Vec<String> = Vec::new();
    let mut output_bytes_count: u64 = 0;
    let mut truncated_by = "lines";
    let mut last_line_partial = false;

    let mut i = lines.len() as i64 - 1;
    while i >= 0 && (output_lines_arr.len() as u64) < max_lines {
        let line = lines[i as usize];
        let line_bytes = line.len() as u64 + if !output_lines_arr.is_empty() { 1 } else { 0 };

        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = "bytes";
            // Edge case: no lines added yet and this line exceeds maxBytes:
            // take the end of the line (partial).
            if output_lines_arr.is_empty() {
                let truncated_line = truncate_string_to_bytes_from_end(line, max_bytes as usize);
                output_bytes_count = truncated_line.len() as u64;
                output_lines_arr.insert(0, truncated_line);
                last_line_partial = true;
            }
            break;
        }

        output_lines_arr.insert(0, line.to_string());
        output_bytes_count += line_bytes;
        i -= 1;
    }

    // If we exited due to line limit
    if output_lines_arr.len() as u64 >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = "lines";
    }

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = output_content.len() as u64;

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by.to_string()),
        total_lines,
        total_bytes,
        output_lines: output_lines_arr.len() as u64,
        output_bytes: final_output_bytes,
        last_line_partial,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate a string to fit within a byte limit (from the end), cutting only
/// at valid UTF-8 character boundaries. Port of `truncateStringToBytesFromEnd`.
fn truncate_string_to_bytes_from_end(s: &str, max_bytes: usize) -> String {
    let buf = s.as_bytes();
    if buf.len() <= max_bytes {
        return s.to_string();
    }

    // Start from the end, skip maxBytes back
    let mut start = buf.len() - max_bytes;

    // Find a valid UTF-8 boundary (start of a character)
    while start < buf.len() && (buf[start] & 0xc0) == 0x80 {
        start += 1;
    }

    String::from_utf8_lossy(&buf[start..]).into_owned()
}

/// Truncate a single line to max chars, adding `[truncated]` suffix.
/// Port of `truncateLine`. (The original slices by UTF-16 code units; we slice
/// by Unicode scalar values, which is identical for BMP text.)
pub fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    if crate::js_string::js_length(line) <= max_chars {
        return (line.to_string(), false);
    }
    let mut text: String = line.chars().take(max_chars).collect();
    text.push_str("... [truncated]");
    (text, true)
}

/// Keep the start and the end of `content`, half of `maxBytes` each, with a
/// `…N chars truncated…` marker in the middle. Port of `truncateMiddle`.
pub fn truncate_middle(content: &str, max_bytes: u64) -> MiddleTruncationResult {
    let buf = content.as_bytes();
    let total_lines = split_lines_for_counting(content).len() as u64;
    if buf.len() as u64 <= max_bytes {
        return MiddleTruncationResult {
            content: content.to_string(),
            truncated: false,
            removed_chars: 0,
            total_bytes: buf.len() as u64,
            total_lines,
        };
    }
    let max_bytes = max_bytes as usize;
    // Continuation bytes (10xxxxxx) are not character starts.
    let is_boundary = |index: usize| index >= buf.len() || (buf[index] & 0xc0) != 0x80;
    let mut head_end = max_bytes / 2;
    while head_end > 0 && !is_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = buf.len() - (max_bytes - max_bytes / 2);
    while tail_start < buf.len() && !is_boundary(tail_start) {
        tail_start += 1;
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let tail = String::from_utf8_lossy(&buf[tail_start..]).into_owned();
    let removed_chars = String::from_utf8_lossy(&buf[head_end..tail_start])
        .chars()
        .count();
    MiddleTruncationResult {
        content: format!("{}\u{2026}{} chars truncated\u{2026}{}", head, removed_chars, tail),
        truncated: true,
        removed_chars,
        total_bytes: buf.len() as u64,
        total_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_matches_original() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(50 * 1024), "50.0KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
    }

    #[test]
    fn head_no_truncation() {
        let r = truncate_head("a\nb\nc", TruncationOptions::default());
        assert!(!r.truncated);
        assert_eq!(r.content, "a\nb\nc");
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.total_bytes, 5);
    }

    #[test]
    fn head_line_limit() {
        let content: String = (0..2500).map(|i| format!("Line {}\n", i + 1)).collect();
        // 2500 lines each with trailing newline -> 2500 counted lines
        let r = truncate_head(&content, TruncationOptions::default());
        assert!(r.truncated);
        assert_eq!(r.truncated_by.as_deref(), Some("lines"));
        assert_eq!(r.output_lines, 2000);
        assert!(r.content.starts_with("Line 1\n"));
        assert!(r.content.ends_with("Line 2000"));
    }

    #[test]
    fn head_byte_limit() {
        let content: String = (0..500)
            .map(|i| format!("Line {}: {}\n", i + 1, "x".repeat(200)))
            .collect();
        let r = truncate_head(&content, TruncationOptions::default());
        assert!(r.truncated);
        assert_eq!(r.truncated_by.as_deref(), Some("bytes"));
        assert!(r.output_bytes <= DEFAULT_MAX_BYTES);
    }

    #[test]
    fn head_first_line_exceeds() {
        let content = format!("{}\nsecond", "x".repeat(60000));
        let r = truncate_head(&content, TruncationOptions::default());
        assert!(r.truncated);
        assert!(r.first_line_exceeds_limit);
        assert_eq!(r.content, "");
    }

    #[test]
    fn tail_keeps_end() {
        let content: String = (0..2500).map(|i| format!("Line {}\n", i + 1)).collect();
        let r = truncate_tail(&content, TruncationOptions::default());
        assert!(r.truncated);
        assert_eq!(r.truncated_by.as_deref(), Some("lines"));
        assert!(r.content.ends_with("Line 2500"));
        assert!(r.content.starts_with("Line 501\n"));
    }

    #[test]
    fn tail_partial_line() {
        // Single enormous line with no newline: keep last maxBytes as partial.
        let content = "y".repeat(60000);
        let r = truncate_tail(&content, TruncationOptions::default());
        assert!(r.truncated);
        assert!(r.last_line_partial);
        assert_eq!(r.output_bytes as usize, DEFAULT_MAX_BYTES as usize);
        assert!(r.content.chars().all(|c| c == 'y'));
    }

    #[test]
    fn truncate_line_suffix() {
        let line = "z".repeat(600);
        let (text, was) = truncate_line(&line, GREP_MAX_LINE_LENGTH);
        assert!(was);
        assert_eq!(crate::js_string::js_length(&text), 500 + "... [truncated]".len());
        let (text2, was2) = truncate_line("short", GREP_MAX_LINE_LENGTH);
        assert!(!was2);
        assert_eq!(text2, "short");
    }

    #[test]
    fn middle_truncation() {
        let content = "abcdefghij".repeat(1000); // 10000 bytes
        let r = truncate_middle(&content, 100);
        assert!(r.truncated);
        assert_eq!(r.total_bytes, 10000);
        assert!(r.content.starts_with("abcdefghij"));
        assert!(r.content.contains(" chars truncated\u{2026}"));
        let r2 = truncate_middle("small", 100);
        assert!(!r2.truncated);
    }
}
