//! Port of the pure functions of `packages/tui/src/components/editor.ts`:
//! paste-marker segmentation (`segmentWithMarkers`, editor.ts:38-105) and
//! word wrapping (`wordWrapLine`, editor.ts:124-216).
//!
//! ## Index space
//!
//! All indices (`TextChunk::start_index`/`end_index`,
//! `GraphemeSegment::index`) are **UTF-16 code units** (JS `String`
//! semantics), matching `Intl.SegmentData.index` and the rest of the port
//! (see `word_navigation.rs`). Paste IDs are `u32` (TS `number`, practically
//! small integers from an incrementing counter).
//!
//! **Owner: T4 delegate D2.**

use std::collections::HashSet;

use crate::unicode::grapheme_segments;
use crate::unicode::is_cjk_script;
use crate::unicode::word_break::{segment_words, WordSegment};
use crate::utils::{is_whitespace_char, utf16_len, visible_width};

/// `TextChunk` (editor.ts:107-111) — one word-wrapped chunk of a line.
/// `start_index`/`end_index` are UTF-16 code-unit offsets into the
/// original line (JS `line.slice(chunkStart, …)` semantics).
#[derive(Debug, Clone, PartialEq)]
pub struct TextChunk {
    pub text: String,
    pub start_index: usize,
    pub end_index: usize,
}

/// A grapheme segment with its UTF-16 index (`Intl.SegmentData` shape,
/// restricted to what the editor consumes).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphemeSegment {
    pub segment: String,
    /// UTF-16 code-unit index of the segment start.
    pub index: usize,
}

// ---------------------------------------------------------------------------
// Paste-marker shape matching (editor.ts:26-34)
// ---------------------------------------------------------------------------

/// `PASTE_MARKER_REGEX` / `PASTE_MARKER_SINGLE`
/// (editor.ts:30-33): `\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]`.
///
/// Matches the pattern as a **prefix** of `s` (the global regex scans for
/// non-overlapping prefix matches left-to-right; `PASTE_MARKER_SINGLE` is the
/// same pattern anchored at both ends). Returns `(match_len_in_bytes, id)`.
///
/// Marker text is pure ASCII, so byte length == UTF-16 length. The regex
/// backtracking is safe to collapse to greedy-only matching: if a maximal
/// `\d+` run fails to continue the pattern, any shorter run is followed by a
/// digit, which can neither start the ` (…)` optional group nor be `]`.
fn match_marker_prefix(s: &str) -> Option<(usize, u64)> {
    let b = s.as_bytes();
    if !b.starts_with(b"[paste #") {
        return None;
    }
    let mut i = 8usize; // len("[paste #")
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        return None; // `(\d+)` requires at least one digit
    }
    // JS `Number.parseInt(m[1], 10)` — practically small IDs; saturate on
    // overflow (such an ID can never be in the valid-IDs set anyway).
    let id: u64 = s[digits_start..i].parse().unwrap_or(u64::MAX);
    if i < b.len() && b[i] == b' ' {
        // The optional group `( (\+\d+ lines|\d+ chars))?` cannot be skipped
        // here (skipping would require `]` at the space position), so it must
        // match in full.
        i += 1;
        if i < b.len() && b[i] == b'+' {
            i += 1;
            let ds = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == ds || !b[i..].starts_with(b" lines") {
                return None;
            }
            i += b" lines".len();
        } else {
            let ds = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i == ds || !b[i..].starts_with(b" chars") {
                return None;
            }
            i += b" chars".len();
        }
    }
    if i >= b.len() || b[i] != b']' {
        return None;
    }
    Some((i + 1, id))
}

/// `isPasteMarker` (editor.ts:38-46):
/// `segment.length >= 10 && PASTE_MARKER_SINGLE.test(segment)` — shape only.
/// (Any string matching `PASTE_MARKER_SINGLE` is ASCII and ≥ 10 UTF-16 units,
/// so the length guard is subsumed by the full match, but it is kept for
/// fidelity with the TS fast-path check.)
pub fn is_paste_marker(segment: &str) -> bool {
    utf16_len(segment) >= 10
        && match_marker_prefix(segment)
            .map(|(len, _)| len == segment.len())
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Marker span scanning (editor.ts:57-68: `text.matchAll(PASTE_MARKER_REGEX)`)
// ---------------------------------------------------------------------------

/// One `matchAll` result with a valid ID: `[start, end)` in UTF-16 units.
struct MarkerSpan {
    start: usize,
    end: usize,
    #[allow(dead_code)]
    id: u64,
    text: String,
}

/// `text.matchAll(PASTE_MARKER_REGEX)` — non-overlapping matches,
/// left-to-right. Positions are UTF-16 code units (matches are ASCII, so byte
/// length == UTF-16 length; the surrounding text may not be).
fn find_marker_spans(text: &str) -> Vec<MarkerSpan> {
    let mut spans = Vec::new();
    let mut u16_pos = 0usize;
    let mut it = text.char_indices().peekable();
    while let Some((byte_pos, ch)) = it.next() {
        // Matches can only start at '['; a failed attempt advances one char
        // (JS regex semantics), a successful one advances past the match.
        if ch == '[' {
            if let Some((len, id)) = match_marker_prefix(&text[byte_pos..]) {
                spans.push(MarkerSpan {
                    start: u16_pos,
                    end: u16_pos + len,
                    id,
                    text: text[byte_pos..byte_pos + len].to_string(),
                });
                // Skip the remaining matched chars (all ASCII).
                for _ in 1..len {
                    it.next();
                }
                u16_pos += len;
                continue;
            }
        }
        u16_pos += ch.len_utf16();
    }
    spans
}

/// The `markers` array from editor.ts:64-70 — spans whose ID is in
/// `valid_ids`. The TS fast path (`validIds.size === 0 ||
/// !text.includes("[paste #")`) is preserved; both routes yield no markers.
fn valid_marker_spans(text: &str, valid_ids: &HashSet<u32>) -> Vec<MarkerSpan> {
    if valid_ids.is_empty() || !text.contains("[paste #") {
        return Vec::new();
    }
    find_marker_spans(text)
        .into_iter()
        .filter(|m| m.id <= u32::MAX as u64 && valid_ids.contains(&(m.id as u32)))
        .collect()
}

/// `graphemeSegmenter.segment(text)` with UTF-16 indices
/// (`Intl.SegmentData { segment, index }`).
fn default_grapheme_segments(line: &str) -> Vec<GraphemeSegment> {
    let mut out = Vec::new();
    let mut index = 0usize;
    for g in grapheme_segments(line) {
        out.push(GraphemeSegment {
            segment: g.to_string(),
            index,
        });
        index += utf16_len(g);
    }
    out
}

/// `segmentWithMarkers(text, graphemeSegmenter, validIds)`
/// (editor.ts:49-105) at **grapheme** granularity: segment `text` with
/// `crate::unicode::grapheme_segments`, then merge runs of graphemes that
/// form a `[paste #N …]` marker with `N` in `valid_ids` into a single
/// atomic segment (markers with invalid IDs are left segmented normally).
pub fn segment_graphemes_with_markers(
    text: &str,
    valid_ids: &HashSet<u32>,
) -> Vec<GraphemeSegment> {
    // Fast path: no paste markers in the text or no valid IDs (editor.ts:53-55).
    let markers = valid_marker_spans(text, valid_ids);

    let base = default_grapheme_segments(text);
    if markers.is_empty() {
        return base;
    }

    // Build merged segment list (editor.ts:73-103).
    let mut result: Vec<GraphemeSegment> = Vec::with_capacity(base.len());
    let mut marker_idx = 0usize;

    for seg in &base {
        // Skip past markers that are entirely before this segment.
        while marker_idx < markers.len() && markers[marker_idx].end <= seg.index {
            marker_idx += 1;
        }

        if let Some(marker) = markers.get(marker_idx) {
            if seg.index >= marker.start && seg.index < marker.end {
                // This segment falls inside a marker. If this is the first
                // segment of the marker, emit a merged segment; otherwise
                // skip (already merged into the first segment).
                if seg.index == marker.start {
                    result.push(GraphemeSegment {
                        segment: marker.text.clone(),
                        index: marker.start,
                    });
                }
                continue;
            }
        }
        result.push(seg.clone());
    }

    result
}

/// `segmentWithMarkers(text, wordSegmenter, validIds)` (editor.ts:49-105)
/// at **word** granularity: segment `text` with
/// `crate::unicode::word_break::segment_words`, then merge runs forming a
/// valid paste marker into a single atomic segment.
///
/// The TS merge (editor.ts:74-77) pushes `{ segment, index, input }` — an
/// object **without** an `isWordLike` property — so a merged marker reads as
/// `isWordLike === undefined` (falsy). We encode that as `is_word_like:
/// false`. (The marker's underlying first word segment is `[`, which is not
/// word-like either, so this matches the base segmenter's value as well.)
/// Word segments carry no index in the Rust API, so indices are accumulated
/// in UTF-16 units to drive the merge.
pub fn segment_words_with_markers(text: &str, valid_ids: &HashSet<u32>) -> Vec<WordSegment> {
    // Fast path: no paste markers in the text or no valid IDs (editor.ts:53-55).
    let markers = valid_marker_spans(text, valid_ids);

    let base = segment_words(text);
    if markers.is_empty() {
        return base;
    }

    // Parallel UTF-16 start indices for the base word segments.
    let mut indices = Vec::with_capacity(base.len());
    let mut index = 0usize;
    for word in &base {
        indices.push(index);
        index += utf16_len(&word.segment);
    }

    // Build merged segment list (editor.ts:73-103).
    let mut result: Vec<WordSegment> = Vec::with_capacity(base.len());
    let mut marker_idx = 0usize;

    for (seg_i, word) in base.iter().enumerate() {
        let seg_index = indices[seg_i];
        // Skip past markers that are entirely before this segment.
        while marker_idx < markers.len() && markers[marker_idx].end <= seg_index {
            marker_idx += 1;
        }

        if let Some(marker) = markers.get(marker_idx) {
            if seg_index >= marker.start && seg_index < marker.end {
                // This segment falls inside a marker. If this is the first
                // segment of the marker, emit a merged segment; otherwise
                // skip (already merged into the first segment).
                if seg_index == marker.start {
                    result.push(WordSegment {
                        segment: marker.text.clone(),
                        is_word_like: false, // TS merged object has no isWordLike
                    });
                }
                continue;
            }
        }
        result.push(word.clone());
    }

    result
}

// ---------------------------------------------------------------------------
// Word wrap (editor.ts:124-216)
// ---------------------------------------------------------------------------

/// `cjkBreakRegex.test(segment)` (utils.ts:54-55) — unanchored test over the
/// whole segment: true if any code point carries a CJK script extension.
/// (Same semantics as the private `cjk_break_test` in utils.rs; reimplemented
/// here per the module contract.)
fn cjk_break_test(segment: &str) -> bool {
    segment.chars().any(|c| is_cjk_script(c as u32))
}

/// `isWhitespaceChar(grapheme)` (utils.ts:1011-1013) — `/\s/.test(grapheme)`:
/// true if any character of the (possibly multi-char) grapheme is JS
/// whitespace.
fn is_whitespace_segment(segment: &str) -> bool {
    segment.chars().any(is_whitespace_char)
}

/// JS `String.prototype.slice(start, end)` in UTF-16 code units (the slice
/// bounds used here always fall on grapheme boundaries).
fn utf16_slice(s: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut u = 0usize;
    for ch in s.chars() {
        if u >= end {
            break;
        }
        let len = ch.len_utf16();
        if u >= start {
            out.push(ch);
        }
        u += len;
    }
    out
}

/// `wordWrapLine` (editor.ts:124-216): split a line into word-wrapped
/// chunks. Wraps at word boundaries when possible (whitespace followed by
/// non-whitespace, or CJK adjacency), backtracks to the last wrap
/// opportunity when overflow allows, force-breaks otherwise, and re-wraps
/// single atomic segments wider than `max_width` (e.g. paste markers) at
/// grapheme granularity while keeping them logically atomic.
///
/// `pre_segmented` is the pre-segmented graphemes (paste-marker aware);
/// when `None` the default grapheme segmenter is used.
///
/// Widths are tracked as `i64` internally: TS uses `number` and the
/// backtrack comparison `currentWidth - wrapOppWidth + gWidth <= maxWidth`
/// is computed with possible negative intermediates (JS semantics) — `i64`
/// reproduces that exactly without panicking.
pub fn word_wrap_line(
    line: &str,
    max_width: usize,
    pre_segmented: Option<&[GraphemeSegment]>,
) -> Vec<TextChunk> {
    // editor.ts:141-143: `if (!line || maxWidth <= 0)`.
    if line.is_empty() || max_width == 0 {
        return vec![TextChunk {
            text: String::new(),
            start_index: 0,
            end_index: 0,
        }];
    }
    let max_width = max_width as i64;

    // editor.ts:145-147: lineWidth fast path (`line.length` = UTF-16 length).
    if (visible_width(line) as i64) <= max_width {
        return vec![TextChunk {
            text: line.to_string(),
            start_index: 0,
            end_index: utf16_len(line),
        }];
    }

    let owned_segments;
    let segments: &[GraphemeSegment] = match pre_segmented {
        Some(segments) => segments,
        None => {
            owned_segments = default_grapheme_segments(line);
            &owned_segments
        }
    };

    let mut chunks: Vec<TextChunk> = Vec::new();

    let mut current_width: i64 = 0;
    let mut chunk_start = 0usize;

    // Wrap opportunity: the position after the last whitespace before a
    // non-whitespace grapheme, i.e. where a line break is allowed.
    let mut wrap_opp_index: i64 = -1;
    let mut wrap_opp_width: i64 = 0;

    for i in 0..segments.len() {
        let seg = &segments[i];
        let grapheme = seg.segment.as_str();
        let g_width = visible_width(grapheme) as i64;
        let char_index = seg.index;
        let is_ws = !is_paste_marker(grapheme) && is_whitespace_segment(grapheme);

        // Overflow check before advancing (editor.ts:168-186).
        if current_width + g_width > max_width {
            if wrap_opp_index >= 0 && current_width - wrap_opp_width + g_width <= max_width {
                // Backtrack to last wrap opportunity (the remaining content
                // plus the current grapheme still fits within maxWidth).
                let wrap_index = wrap_opp_index as usize;
                chunks.push(TextChunk {
                    text: utf16_slice(line, chunk_start, wrap_index),
                    start_index: chunk_start,
                    end_index: wrap_index,
                });
                chunk_start = wrap_index;
                current_width -= wrap_opp_width;
            } else if chunk_start < char_index {
                // No viable wrap opportunity: force-break at current position.
                // This also handles the case where backtracking to a word
                // boundary wouldn't help because the remaining content plus
                // the current grapheme (e.g. a wide character) still exceeds
                // maxWidth.
                chunks.push(TextChunk {
                    text: utf16_slice(line, chunk_start, char_index),
                    start_index: chunk_start,
                    end_index: char_index,
                });
                chunk_start = char_index;
                current_width = 0;
            }
            wrap_opp_index = -1;
        }

        // editor.ts:188-205: single atomic segment wider than maxWidth
        // (e.g. paste marker in a narrow terminal). Re-wrap at grapheme
        // granularity; the segment remains logically atomic for cursor
        // movement / editing — the split is purely visual.
        if g_width > max_width {
            let sub_chunks = word_wrap_line(grapheme, max_width as usize, None);
            for sc in &sub_chunks[..sub_chunks.len() - 1] {
                chunks.push(TextChunk {
                    text: sc.text.clone(),
                    start_index: char_index + sc.start_index,
                    end_index: char_index + sc.end_index,
                });
            }
            let last = &sub_chunks[sub_chunks.len() - 1];
            chunk_start = char_index + last.start_index;
            current_width = visible_width(&last.text) as i64;
            wrap_opp_index = -1;
            continue;
        }

        // Advance.
        current_width += g_width;

        // Record wrap opportunity (editor.ts:209-216): whitespace followed by
        // non-whitespace (multiple spaces join; the break point is after the
        // last space), or at a boundary where either side is CJK (CJK allows
        // breaking between any adjacent characters).
        let next = if i + 1 < segments.len() {
            Some(&segments[i + 1])
        } else {
            None
        };
        if is_ws {
            if let Some(next) = next {
                if is_paste_marker(&next.segment) || !is_whitespace_segment(&next.segment) {
                    wrap_opp_index = next.index as i64;
                    wrap_opp_width = current_width;
                }
            }
        } else if let Some(next) = next {
            if !is_whitespace_segment(&next.segment) {
                let is_cjk = !is_paste_marker(grapheme) && cjk_break_test(grapheme);
                let next_is_cjk = !is_paste_marker(&next.segment) && cjk_break_test(&next.segment);
                if is_cjk || next_is_cjk {
                    wrap_opp_index = next.index as i64;
                    wrap_opp_width = current_width;
                }
            }
        }
    }

    // Push final chunk (editor.ts:219-220).
    let line_len = utf16_len(line);
    chunks.push(TextChunk {
        text: utf16_slice(line, chunk_start, line_len),
        start_index: chunk_start,
        end_index: line_len,
    });

    chunks
}

// ---------------------------------------------------------------------------
// Tests — ports of the wordWrapLine tests in
// /tmp/pi-repo/packages/tui/test/editor.test.ts (word-wrap describe block,
// editor.test.ts:947-1200, plus the overflow-recheck regression at 4124),
// and direct unit tests for the paste-marker segmentation functions modeled
// on the paste-marker tests (editor.test.ts:3815-4066).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `TextChunk` literal helper.
    fn tc(text: &str, start: usize, end: usize) -> TextChunk {
        TextChunk {
            text: text.to_string(),
            start_index: start,
            end_index: end,
        }
    }

    /// Rebuild the original line from a chunk array
    /// (`chunks.map(c => line.slice(c.startIndex, c.endIndex)).join("")`).
    fn reconstruct(line: &str, chunks: &[TextChunk]) -> String {
        let mut out = String::new();
        for c in chunks {
            out.push_str(&utf16_slice(line, c.start_index, c.end_index));
        }
        out
    }

    /// The chunks tile the line exactly (0 → line.length in UTF-16 units,
    /// no gaps or overlaps) — implicit in the upstream reconstruction check.
    fn assert_tiling(line: &str, chunks: &[TextChunk]) {
        assert_eq!(chunks.first().map(|c| c.start_index), Some(0));
        assert_eq!(
            chunks.last().map(|c| c.end_index),
            Some(utf16_len(line)),
            "last chunk must end at line end"
        );
        for w in chunks.windows(2) {
            assert_eq!(
                w[0].end_index, w[1].start_index,
                "chunks must be contiguous"
            );
        }
    }

    fn ids(v: &[u32]) -> HashSet<u32> {
        v.iter().copied().collect()
    }

    // --- is_paste_marker (editor.ts:38-46) ----------------------------------

    #[test]
    fn is_paste_marker_matches_all_marker_shapes() {
        // "[paste #1]" is the 10-char minimum.
        assert!(is_paste_marker("[paste #1]"));
        assert!(is_paste_marker("[paste #12]"));
        assert!(is_paste_marker("[paste #12 +3 lines]"));
        assert!(is_paste_marker("[paste #2 1234 chars]"));
        assert!(is_paste_marker("[paste #0 +0 lines]"));
        // Leading zeros parse fine in the shape (parseInt sees the digits).
        assert!(is_paste_marker("[paste #007]"));
    }

    #[test]
    fn is_paste_marker_rejects_non_marker_shapes() {
        assert!(!is_paste_marker(""));
        assert!(!is_paste_marker("x"));
        assert!(!is_paste_marker("[paste #]"));
        assert!(!is_paste_marker("[paste #1x]"));
        assert!(!is_paste_marker("[paste#1]"));
        assert!(!is_paste_marker(" [paste #1]"));
        assert!(!is_paste_marker("[paste #1] "));
        assert!(!is_paste_marker("[paste #1 +3 lines] "));
        assert!(!is_paste_marker("x[paste #1 +3 lines]"));
        // Group shape mismatches.
        assert!(!is_paste_marker("[paste #1 +3 chars]")); // '+' with "chars"
        assert!(!is_paste_marker("[paste #1 3 lines]")); // digits with "lines"
        assert!(!is_paste_marker("[paste #1 +3 lines")); // missing ']'
        assert!(!is_paste_marker("[paste #1 + lines]")); // no digits
        assert!(!is_paste_marker("[paste #1 12 chars")); // missing ']'
    }

    // --- segment_graphemes_with_markers (editor.ts:49-105) ------------------

    #[test]
    fn segment_graphemes_merges_valid_marker_atomically() {
        // Mirrors "treats paste marker as single unit for right arrow"
        // (editor.test.ts:3820): text "A[paste #1 +20 lines]B".
        let text = "A[paste #1 +20 lines]B";
        let segs = segment_graphemes_with_markers(text, &ids(&[1]));
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].segment, "A");
        assert_eq!(segs[0].index, 0);
        assert_eq!(segs[1].segment, "[paste #1 +20 lines]");
        assert_eq!(segs[1].index, 1);
        assert_eq!(segs[2].segment, "B");
        assert_eq!(segs[2].index, 21); // 1 + marker's 20 UTF-16 units
        let joined: String = segs.iter().map(|s| s.segment.as_str()).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn segment_graphemes_empty_valid_ids_leaves_text_segmented() {
        // Mirrors "does not treat manually typed marker-like text as atomic"
        // (editor.test.ts:3986): no valid paste ID → not atomic.
        let text = "[paste #1 +20 lines]";
        let segs = segment_graphemes_with_markers(text, &HashSet::new());
        assert_eq!(segs.len(), 20, "fully segmented at grapheme granularity");
        for (i, seg) in segs.iter().enumerate() {
            assert_eq!(seg.index, i);
        }
        assert_eq!(segs[0].segment, "[");
        assert_eq!(segs[19].segment, "]");
        let joined: String = segs.iter().map(|s| s.segment.as_str()).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn segment_graphemes_invalid_id_not_merged() {
        // "[paste #99 +5 lines]" with only ID 1 valid → normal segmentation.
        let text = "[paste #99 +5 lines]";
        let segs = segment_graphemes_with_markers(text, &ids(&[1]));
        assert_eq!(segs.len(), 20);
        assert!(
            !segs.iter().any(|s| s.segment == "[paste #99 +5 lines]"),
            "invalid-ID marker must not be merged"
        );
        let joined: String = segs.iter().map(|s| s.segment.as_str()).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn segment_graphemes_multiple_markers() {
        // Mirrors "handles multiple paste markers in same line"
        // (editor.test.ts:3961): "[paste #1 +20 lines] [paste #2 5 chars]".
        let text = "[paste #1 +20 lines] [paste #2 5 chars]";
        let segs = segment_graphemes_with_markers(text, &ids(&[1, 2]));
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].segment, "[paste #1 +20 lines]");
        assert_eq!(segs[0].index, 0);
        assert_eq!(segs[1].segment, " ");
        assert_eq!(segs[1].index, 20);
        assert_eq!(segs[2].segment, "[paste #2 5 chars]");
        assert_eq!(segs[2].index, 21);
    }

    #[test]
    fn segment_graphemes_partially_valid_ids() {
        // Only marker #1 merged (20 UTF-16 units); marker #2 (invalid ID,
        // 18 UTF-16 units) stays segmented.
        let text = "[paste #1 +20 lines] [paste #2 5 chars]";
        let segs = segment_graphemes_with_markers(text, &ids(&[1]));
        assert_eq!(segs.len(), 1 + 1 + 18);
        assert_eq!(segs[0].segment, "[paste #1 +20 lines]");
        assert!(
            !segs.iter().any(|s| s.segment == "[paste #2 5 chars]"),
            "invalid-ID marker must not be merged"
        );
        let joined: String = segs.iter().map(|s| s.segment.as_str()).collect();
        assert_eq!(joined, text);
    }

    // --- segment_words_with_markers (editor.ts:49-105) ----------------------

    #[test]
    fn segment_words_merges_marker_atomically() {
        // Mirrors "treats paste marker as single unit for word movement"
        // (editor.test.ts:3881): text "X [paste #1 +20 lines] Y".
        let text = "X [paste #1 +20 lines] Y";
        let segs = segment_words_with_markers(text, &ids(&[1]));
        // Base word segmentation: "X", " ", then the marker's words ("[",
        // "paste", "#", "1", …) all swallowed by the merge, then " ", "Y".
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[0].segment, "X");
        assert!(segs[0].is_word_like);
        assert_eq!(segs[1].segment, " ");
        assert!(!segs[1].is_word_like);
        assert_eq!(segs[2].segment, "[paste #1 +20 lines]");
        // The TS merged segment object carries no isWordLike property →
        // `undefined` (falsy). See editor.ts:74-77.
        assert!(!segs[2].is_word_like);
        assert_eq!(segs[3].segment, " ");
        assert_eq!(segs[4].segment, "Y");
        assert!(segs[4].is_word_like);
        let joined: String = segs.iter().map(|s| s.segment.as_str()).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn segment_words_marker_between_word_chars() {
        // Marker embedded directly between word characters: "A[paste #1 4 chars]B".
        let text = "A[paste #1 4 chars]B";
        let segs = segment_words_with_markers(text, &ids(&[1]));
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].segment, "A");
        assert!(segs[0].is_word_like);
        assert_eq!(segs[1].segment, "[paste #1 4 chars]");
        assert!(!segs[1].is_word_like);
        assert_eq!(segs[2].segment, "B");
        assert!(segs[2].is_word_like);
    }

    #[test]
    fn segment_words_invalid_id_not_merged() {
        // Mirrors "does not treat manually typed marker-like text as atomic"
        // (editor.test.ts:3986) at word granularity.
        let text = "X [paste #99 +5 lines] Y";
        let segs = segment_words_with_markers(text, &ids(&[1]));
        assert!(
            !segs.iter().any(|s| s.segment == "[paste #99 +5 lines]"),
            "invalid-ID marker must not be merged"
        );
        assert!(segs.len() > 5, "marker words stay individually segmented");
        let joined: String = segs.iter().map(|s| s.segment.as_str()).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn segment_words_empty_valid_ids_is_plain_segmentation() {
        let text = "X [paste #1 +20 lines] Y";
        let plain = segment_words(text);
        let segs = segment_words_with_markers(text, &HashSet::new());
        assert_eq!(segs, plain);
    }

    // --- wordWrapLine (editor.test.ts:974-1195) ------------------------------

    #[test]
    fn wraps_word_to_next_line_when_it_ends_exactly_at_terminal_width() {
        // "hello " (6) + "world" (5) = 11, but "world" is non-whitespace
        // ending at width. Thus, wrap it to next line. The trailing space
        // stays with "hello" on line 1.
        let chunks = word_wrap_line("hello world test", 11, None);
        assert_eq!(chunks, vec![tc("hello ", 0, 6), tc("world test", 6, 16)]);
    }

    #[test]
    fn keeps_whitespace_at_terminal_width_boundary_on_same_line() {
        // "hello world " is exactly 12 chars (including trailing space).
        // The space at position 12 should stay on the first line.
        let chunks = word_wrap_line("hello world test", 12, None);
        assert_eq!(chunks, vec![tc("hello world ", 0, 12), tc("test", 12, 16)]);
    }

    #[test]
    fn handles_unbreakable_word_filling_width_exactly_followed_by_space() {
        let chunks = word_wrap_line("aaaaaaaaaaaa aaaa", 12, None);
        assert_eq!(chunks, vec![tc("aaaaaaaaaaaa", 0, 12), tc(" aaaa", 12, 17)]);
    }

    #[test]
    fn wraps_word_to_next_line_when_it_fits_width_but_not_remaining_space() {
        let chunks = word_wrap_line("      aaaaaaaaaaaa", 12, None);
        assert_eq!(chunks, vec![tc("      ", 0, 6), tc("aaaaaaaaaaaa", 6, 18)]);
    }

    #[test]
    fn keeps_word_with_multi_space_and_following_word_together_when_they_fit() {
        let line = format!("Lorem ipsum dolor sit amet,{}consectetur", " ".repeat(4));
        let chunks = word_wrap_line(&line, 30, None);
        assert_eq!(
            chunks,
            vec![
                tc("Lorem ipsum dolor sit ", 0, 22),
                tc(&format!("amet,{}consectetur", " ".repeat(4)), 22, 42)
            ]
        );
    }

    #[test]
    fn keeps_word_with_multi_space_and_following_word_when_they_fill_width_exactly() {
        let line = format!("Lorem ipsum dolor sit amet,{}consectetur", " ".repeat(14));
        let chunks = word_wrap_line(&line, 30, None);
        assert_eq!(
            chunks,
            vec![
                tc("Lorem ipsum dolor sit ", 0, 22),
                tc(&format!("amet,{}consectetur", " ".repeat(14)), 22, 52)
            ]
        );
    }

    #[test]
    fn splits_when_word_plus_multi_space_plus_word_exceeds_width() {
        let line = format!("Lorem ipsum dolor sit amet,{}consectetur", " ".repeat(15));
        let chunks = word_wrap_line(&line, 30, None);
        assert_eq!(
            chunks,
            vec![
                tc("Lorem ipsum dolor sit ", 0, 22),
                tc(&format!("amet,{}", " ".repeat(15)), 22, 42),
                tc("consectetur", 42, 53)
            ]
        );
    }

    #[test]
    fn breaks_long_whitespace_at_line_boundary() {
        let line = format!("Lorem ipsum dolor sit amet,{}consectetur", " ".repeat(25));
        let chunks = word_wrap_line(&line, 30, None);
        assert_eq!(
            chunks,
            vec![
                tc("Lorem ipsum dolor sit ", 0, 22),
                tc(&format!("amet,{}", " ".repeat(25)), 22, 52),
                tc("consectetur", 52, 63)
            ]
        );
    }

    #[test]
    fn breaks_long_whitespace_at_line_boundary_2() {
        let line = format!("Lorem ipsum dolor sit amet,{}consectetur", " ".repeat(26));
        let chunks = word_wrap_line(&line, 30, None);
        assert_eq!(
            chunks,
            vec![
                tc("Lorem ipsum dolor sit ", 0, 22),
                // The whitespace run only fits 25 of its 26 spaces on the
                // second line; the 26th leads the third.
                tc(&format!("amet,{}", " ".repeat(25)), 22, 52),
                tc(" consectetur", 52, 64)
            ]
        );
    }

    #[test]
    fn breaks_whitespace_spanning_full_lines() {
        let line = format!("Lorem ipsum dolor sit amet,{}consectetur", " ".repeat(37));
        let chunks = word_wrap_line(&line, 30, None);
        assert_eq!(
            chunks,
            vec![
                tc("Lorem ipsum dolor sit ", 0, 22),
                tc(&format!("amet,{}", " ".repeat(25)), 22, 52),
                tc(&format!("{}consectetur", " ".repeat(12)), 52, 75)
            ]
        );
    }

    #[test]
    fn force_breaks_when_wide_char_after_word_boundary_wrap_still_overflows() {
        // " " (1) + "a"*186 (186) + "你" (2) = 189 visible width.
        // maxWidth = 187: backtracking to the space would leave 186 + 2 =
        // 188 > 187, so the algorithm must force-break before the wide char
        // instead.
        let line = format!(" {}你", "a".repeat(186));
        let chunks = word_wrap_line(&line, 187, None);

        for chunk in &chunks {
            assert!(
                visible_width(&chunk.text) <= 187,
                "chunk {:?} has visible width {}, expected <= 187",
                chunk.text.chars().take(20).collect::<String>(),
                visible_width(&chunk.text)
            );
        }
        // Verify no content is lost.
        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
        // "你" is 1 UTF-16 unit: the line is 188 UTF-16 units long, the wide
        // char starts at index 187.
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], tc(&format!(" {}", "a".repeat(186)), 0, 187));
        assert_eq!(chunks[1], tc("你", 187, 188));
    }

    /// Pre-segmented graphemes helper for the oversized-atomic-segment tests
    /// (the TS tests build `Intl.SegmentData[]` literals).
    fn segs(parts: &[(&str, usize)]) -> Vec<GraphemeSegment> {
        parts
            .iter()
            .map(|(s, i)| GraphemeSegment {
                segment: s.to_string(),
                index: *i,
            })
            .collect()
    }

    #[test]
    fn splits_oversized_atomic_segment_across_multiple_chunks() {
        // Simulate a paste marker wider than maxWidth by passing
        // pre-segmented data.
        let marker = "[paste #1 +20 lines]"; // 21 chars
        let line = format!("A{marker}B");
        let segments = segs(&[("A", 0), (marker, 1), ("B", 1 + marker.len())]);

        let chunks = word_wrap_line(&line, 10, Some(&segments));

        // Every chunk must fit within maxWidth.
        for chunk in &chunks {
            assert!(
                visible_width(&chunk.text) <= 10,
                "chunk {:?} has visible width {}, expected <= 10",
                chunk.text,
                visible_width(&chunk.text)
            );
        }
        // Verify no content is lost.
        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
        assert_eq!(
            chunks,
            vec![
                tc("A", 0, 1),
                tc("[paste #1 ", 1, 11),
                tc("+20 lines]", 11, 21),
                tc("B", 21, 22)
            ]
        );
    }

    #[test]
    fn splits_oversized_atomic_segment_at_start_of_line() {
        let marker = "[paste #1 +20 lines]"; // 21 chars
        let line = format!("{marker}B");
        let segments = segs(&[(marker, 0), ("B", marker.len())]);

        let chunks = word_wrap_line(&line, 10, Some(&segments));

        for chunk in &chunks {
            assert!(visible_width(&chunk.text) <= 10);
        }
        // "B" ends up on the last line (either alone or with the marker tail)
        assert!(chunks[chunks.len() - 1].text.contains('B'));

        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
    }

    #[test]
    fn splits_oversized_atomic_segment_at_end_of_line() {
        let marker = "[paste #1 +20 lines]"; // 21 chars
        let line = format!("A{marker}");
        let segments = segs(&[("A", 0), (marker, 1)]);

        let chunks = word_wrap_line(&line, 10, Some(&segments));

        for chunk in &chunks {
            assert!(visible_width(&chunk.text) <= 10);
        }
        assert_eq!(chunks[0].text, "A");

        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
    }

    #[test]
    fn splits_consecutive_oversized_atomic_segments() {
        let m1 = "[paste #1 +20 lines]"; // 21 chars
        let m2 = "[paste #2 +30 lines]"; // 21 chars
        let line = format!("{m1}{m2}");
        let segments = segs(&[(m1, 0), (m2, m1.len())]);

        let chunks = word_wrap_line(&line, 10, Some(&segments));

        for chunk in &chunks {
            assert!(
                visible_width(&chunk.text) <= 10,
                "chunk {:?} has visible width {}, expected <= 10",
                chunk.text,
                visible_width(&chunk.text)
            );
        }

        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
    }

    #[test]
    fn wraps_normally_after_oversized_atomic_segment() {
        let marker = "[paste #1 +20 lines]"; // 21 chars
        let line = format!("{marker} hello world");
        let mut parts = vec![(marker, 0usize), (" ", marker.len())];
        for (i, ch) in "hello world".chars().enumerate() {
            parts.push((ch.to_string().leak(), marker.len() + 1 + i));
        }
        let segments = segs(&parts);

        let chunks = word_wrap_line(&line, 10, Some(&segments));

        // All chunks must fit.
        for chunk in &chunks {
            assert!(
                visible_width(&chunk.text) <= 10,
                "chunk {:?} has visible width {}, expected <= 10",
                chunk.text,
                visible_width(&chunk.text)
            );
        }

        // Last chunk should contain "world" (normal wrapping resumes)
        assert_eq!(chunks[chunks.len() - 1].text, "world");

        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
    }

    #[test]
    fn word_wrap_line_re_checks_overflow_after_backtracking_to_wrap_opportunity() {
        // Port of editor.test.ts:4124 ("wordWrapLine re-checks overflow after
        // backtracking to wrap opportunity") — reproduce crash #2:
        // " " + "b".repeat(35) + atomic_marker(20 chars) + "bbbb",
        // layoutWidth=53. After wrapping at the space, the remaining 35 b's +
        // marker = 55 must trigger a second force-break instead of silently
        // overflowing. (The upstream test drives this through Editor.render;
        // here it is reproduced as the equivalent direct wordWrapLine call
        // with the pre-segmented segments the editor would produce: a paste
        // of "line\n".repeat(27) yields the marker "[paste #1 +27 lines]".)
        let marker = "[paste #1 +27 lines]"; // 20 chars
        let line = format!(" {}{marker}bbbb", "b".repeat(35));

        let mut parts = vec![(" ", 0usize)];
        for i in 0..35 {
            parts.push(("b", 1 + i));
        }
        parts.push((marker, 36));
        for i in 0..4 {
            parts.push(("b", 56 + i));
        }
        let segments = segs(&parts);

        let chunks = word_wrap_line(&line, 53, Some(&segments));
        for chunk in &chunks {
            assert!(
                visible_width(&chunk.text) <= 53,
                "chunk {:?} has visible width {}, expected <= 53",
                chunk.text.chars().take(20).collect::<String>(),
                visible_width(&chunk.text)
            );
        }
        assert_eq!(reconstruct(&line, &chunks), line);
        assert_tiling(&line, &chunks);
    }

    // --- wordWrapLine edge cases ----------------------------------------------

    #[test]
    fn word_wrap_line_empty_line_and_zero_width() {
        // editor.ts:141-143: `if (!line || maxWidth <= 0)`.
        assert_eq!(word_wrap_line("", 10, None), vec![tc("", 0, 0)]);
        assert_eq!(word_wrap_line("hello", 0, None), vec![tc("", 0, 0)]);
    }

    #[test]
    fn word_wrap_line_fast_path_keeps_utf16_indices() {
        // Line that fits: single chunk spanning the full line; "😀" is one
        // grapheme but two UTF-16 code units.
        let chunks = word_wrap_line("a😀b", 10, None);
        assert_eq!(chunks, vec![tc("a😀b", 0, 4)]);
    }
}
