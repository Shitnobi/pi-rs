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
//! **Owner: T4 delegate D2.** The stubs below pin the API contract; replace
//! each `todo!()` with the ported implementation (and add the ported
//! upstream tests in a `#[cfg(test)]` module at the end of this file).

use std::collections::HashSet;

use crate::unicode::word_break::WordSegment;

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

/// `isPasteMarker` (editor.ts:38-46): does this segment match
/// `^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$`?
///
/// Note: this checks the *shape* only. Validity of the paste ID is checked
/// by the caller (the `validIds` set in `segmentWithMarkers`).
pub fn is_paste_marker(segment: &str) -> bool {
    let _ = segment;
    todo!("[T4-D2] editor.ts:38-46 — PASTE_MARKER_SINGLE")
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
    let _ = (text, valid_ids);
    todo!("[T4-D2] editor.ts:49-105 at grapheme granularity")
}

/// `segmentWithMarkers(text, wordSegmenter, validIds)` (editor.ts:49-105)
/// at **word** granularity: segment `text` with
/// `crate::unicode::word_break::segment_words`, then merge runs forming a
/// valid paste marker into a single atomic segment. `is_word_like` of the
/// merged marker segment is that of the underlying marker word segments
/// (see editor.ts:71-86 — D2: verify against the TS merge logic).
pub fn segment_words_with_markers(text: &str, valid_ids: &HashSet<u32>) -> Vec<WordSegment> {
    let _ = (text, valid_ids);
    todo!("[T4-D2] editor.ts:49-105 at word granularity")
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
pub fn word_wrap_line(line: &str, max_width: usize, pre_segmented: Option<&[GraphemeSegment]>) -> Vec<TextChunk> {
    let _ = (line, max_width, pre_segmented);
    todo!("[T4-D2] editor.ts:124-216")
}
