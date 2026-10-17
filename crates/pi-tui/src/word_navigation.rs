//! Port of `word-navigation.ts` — `findWordBackward` / `findWordForward`.
//!
//! Cursor positions and returned positions are UTF-16 code-unit indices
//! (JS `String` semantics), matching the rest of the TUI port. The default
//! segmenter is [`crate::unicode::word_break::segment_words`], the bit-exact
//! port of Node's `Intl.Segmenter` word granularity.

use crate::unicode::word_break::{segment_words_u16, WordSegment};
use crate::utils::{is_punctuation_char, is_whitespace_char, utf16_len};

/// Options for word navigation functions.
/// When omitted, uses the default Intl.Segmenter word segmentation.
pub struct WordNavigationOptions<'a> {
	/// Custom segmenter returning word segments for the given text.
	pub segment: Option<&'a dyn Fn(&str) -> Vec<WordSegment>>,
	/// Predicate identifying atomic segments that should be treated as single
	/// units (e.g. paste markers).
	pub is_atomic_segment: Option<&'a dyn Fn(&str) -> bool>,
}

/// Iterate `(char, utf16_len)` over a UTF-16 slice, combining surrogate
/// pairs. Lone surrogates decode to U+FFFD with length 1 — they are never
/// whitespace or punctuation, so classification is unaffected.
fn u16_chars(units: &[u16]) -> impl Iterator<Item = (char, usize)> + '_ {
	let mut i = 0usize;
	std::iter::from_fn(move || {
		if i >= units.len() {
			return None;
		}
		let hi = units[i];
		let paired = (0xd800..0xdc00).contains(&hi)
			&& i + 1 < units.len()
			&& (0xdc00..0xe000).contains(&units[i + 1]);
		let len = if paired { 2 } else { 1 };
		let cp = if paired {
			0x10000 + (((hi as u32) - 0xd800) << 10) + ((units[i + 1] as u32) - 0xdc00)
		} else {
			hi as u32
		};
		i += len;
		Some((char::from_u32(cp).unwrap_or('\u{fffd}'), len))
	})
}

/// `/\s/.test(segment)` — unanchored, so true iff ANY unit is whitespace.
fn is_ws_units(units: &[u16]) -> bool {
	u16_chars(units).any(|(c, _)| is_whitespace_char(c))
}

/// Build `(start, end, is_word_like)` segments over `units` (UTF-16 indices).
/// The default path uses the u16-level segmenter directly (exact, including
/// lone surrogates). A custom `segment` fn receives a lossy `&str` — exact
/// for code-point-aligned slices, which is how the editor calls it.
fn build_segments(
	units: &[u16],
	options: Option<&WordNavigationOptions>,
) -> Vec<(usize, usize, bool)> {
	match options.and_then(|o| o.segment) {
		Some(f) => {
			let s = String::from_utf16_lossy(units);
			let mut out = Vec::new();
			let mut u = 0usize;
			for w in f(&s) {
				let len = utf16_len(&w.segment);
				out.push((u, u + len, w.is_word_like));
				u += len;
			}
			out
		}
		None => segment_words_u16(units),
	}
}

/// Find the cursor position after moving one word backward from `cursor` in
/// `text`. Skips trailing whitespace, then stops at the next word/punctuation
/// boundary.
///
/// Pure function - does not mutate any state.
pub fn find_word_backward(text: &str, cursor: usize, options: Option<&WordNavigationOptions>) -> usize {
	let units: Vec<u16> = text.encode_utf16().collect();
	find_word_backward_u16(&units, cursor, options)
}

/// [`find_word_backward`] over raw UTF-16 units. `cursor` may split a
/// surrogate pair (JS `String.slice` semantics: the prefix then ends in a
/// lone surrogate, which the segmenter treats as its own unit).
pub fn find_word_backward_u16(
	units: &[u16],
	cursor: usize,
	options: Option<&WordNavigationOptions>,
) -> usize {
	if cursor == 0 {
		return 0;
	}

	let prefix = &units[..cursor.min(units.len())];
	let is_atomic = options.and_then(|o| o.is_atomic_segment);
	let atomic = |seg: &[u16]| is_atomic.is_some_and(|f| f(&String::from_utf16_lossy(seg)));
	let mut segments = build_segments(prefix, options);
	let mut new_cursor = cursor;

	// Skip trailing whitespace
	while let Some(&(start, end, _)) = segments.last() {
		let seg = &prefix[start..end];
		if !atomic(seg) && is_ws_units(seg) {
			new_cursor -= end - start;
			segments.pop();
		} else {
			break;
		}
	}

	let Some(&(start, end, word_like)) = segments.last() else {
		return new_cursor;
	};

	if atomic(&prefix[start..end]) {
		// Skip one atomic segment.
		new_cursor -= end - start;
	} else if word_like {
		// Skip inside one word-like segment, preserving ASCII punctuation
		// boundaries. [...segment.matchAll(PUNCTUATION_REGEX, "g")] → last
		// punctuation char's UTF-16 end index.
		let mut last_end: Option<usize> = None;
		let mut u = 0usize;
		for (c, len) in u16_chars(&prefix[start..end]) {
			if is_punctuation_char(c) {
				last_end = Some(u + len);
			}
			u += len;
		}
		match last_end {
			None => new_cursor -= end - start,
			Some(e) => new_cursor -= (end - start) - e,
		}
	} else {
		// Skip non-word non-whitespace run (punctuation)
		while let Some(&(s, e, wl)) = segments.last() {
			let seg = &prefix[s..e];
			if !atomic(seg) && !wl && !is_ws_units(seg) {
				new_cursor -= e - s;
				segments.pop();
			} else {
				break;
			}
		}
	}

	new_cursor
}

/// Find the cursor position after moving one word forward from `cursor` in
/// `text`. Skips leading whitespace, then stops at the next word/punctuation
/// boundary.
///
/// Pure function - does not mutate any state.
pub fn find_word_forward(text: &str, cursor: usize, options: Option<&WordNavigationOptions>) -> usize {
	let units: Vec<u16> = text.encode_utf16().collect();
	find_word_forward_u16(&units, cursor, options)
}

/// [`find_word_forward`] over raw UTF-16 units (see [`find_word_backward_u16`]).
pub fn find_word_forward_u16(
	units: &[u16],
	cursor: usize,
	options: Option<&WordNavigationOptions>,
) -> usize {
	let total = units.len();
	if cursor >= total {
		return total;
	}

	let after = &units[cursor..];
	let is_atomic = options.and_then(|o| o.is_atomic_segment);
	let atomic = |seg: &[u16]| is_atomic.is_some_and(|f| f(&String::from_utf16_lossy(seg)));
	let segments = build_segments(after, options);
	let mut it = segments.iter();
	let mut next = it.next();
	let mut new_cursor = cursor;

	// Skip leading whitespace
	while let Some(&(start, end, _)) = next {
		let seg = &after[start..end];
		if !atomic(seg) && is_ws_units(seg) {
			new_cursor += end - start;
			next = it.next();
		} else {
			break;
		}
	}

	let Some(&(start, end, word_like)) = next else {
		return new_cursor;
	};

	if atomic(&after[start..end]) {
		// Skip one atomic segment.
		new_cursor += end - start;
	} else if word_like {
		// Skip inside one word-like segment, preserving ASCII punctuation
		// boundaries. PUNCTUATION_REGEX.exec(segment)?.index → first
		// punctuation char's UTF-16 index.
		let mut first: Option<usize> = None;
		let mut u = 0usize;
		for (c, len) in u16_chars(&after[start..end]) {
			if is_punctuation_char(c) {
				first = Some(u);
				break;
			}
			u += len;
		}
		new_cursor += first.unwrap_or(end - start);
	} else {
		// Skip non-word non-whitespace run (punctuation)
		while let Some(&(s, e, wl)) = next {
			let seg = &after[s..e];
			if !atomic(seg) && !wl && !is_ws_units(seg) {
				new_cursor += e - s;
				next = it.next();
			} else {
				break;
			}
		}
	}

	new_cursor
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashMap;

	// ---- port of test/word-navigation.test.ts --------------------------------

	#[test]
	fn backward_basic_words() {
		let text = "hello world";
		assert_eq!(find_word_backward(text, 11, None), 6);
		assert_eq!(find_word_backward(text, 6, None), 0);
	}

	#[test]
	fn backward_dotted() {
		let text = "foo.bar";
		assert_eq!(find_word_backward(text, 7, None), 4);
		assert_eq!(find_word_backward(text, 4, None), 3);
		assert_eq!(find_word_backward(text, 3, None), 0);
	}

	#[test]
	fn backward_colon() {
		let text = "foo:bar";
		assert_eq!(find_word_backward(text, 7, None), 4);
		assert_eq!(find_word_backward(text, 4, None), 3);
		assert_eq!(find_word_backward(text, 3, None), 0);
	}

	#[test]
	fn backward_path() {
		let text = "path/to/file";
		assert_eq!(find_word_backward(text, 12, None), 8);
		assert_eq!(find_word_backward(text, 8, None), 7);
		// "/to" is one word-like segment with "/" as punctuation boundary
		assert_eq!(find_word_backward(text, 7, None), 5);
		assert_eq!(find_word_backward(text, 5, None), 4);
		assert_eq!(find_word_backward(text, 4, None), 0);
	}

	#[test]
	fn backward_cjk_mixed() {
		let text = "你好世界 test";
		assert_eq!(find_word_backward(text, utf16_len(text), None), 5);
		// Intl.Segmenter treats each CJK char as a separate word-like segment
		assert_eq!(find_word_backward(text, 5, None), 2);
		assert_eq!(find_word_backward(text, 2, None), 0);
	}

	#[test]
	fn backward_whitespace_at_boundaries() {
		let text = "  hello  ";
		assert_eq!(find_word_backward(text, 9, None), 2);
		assert_eq!(find_word_backward(text, 2, None), 0);
	}

	#[test]
	fn backward_punctuation_run() {
		let text = "foo...bar";
		assert_eq!(find_word_backward(text, 9, None), 6);
		assert_eq!(find_word_backward(text, 6, None), 3);
		assert_eq!(find_word_backward(text, 3, None), 0);
	}

	#[test]
	fn backward_cursor_at_0_returns_0() {
		assert_eq!(find_word_backward("hello", 0, None), 0);
	}

	#[test]
	fn forward_basic_words() {
		let text = "hello world";
		assert_eq!(find_word_forward(text, 0, None), 5);
		assert_eq!(find_word_forward(text, 5, None), 11);
	}

	#[test]
	fn forward_dotted() {
		let text = "foo.bar";
		assert_eq!(find_word_forward(text, 0, None), 3);
		assert_eq!(find_word_forward(text, 3, None), 4);
		assert_eq!(find_word_forward(text, 4, None), 7);
	}

	#[test]
	fn forward_colon() {
		let text = "foo:bar";
		assert_eq!(find_word_forward(text, 0, None), 3);
		assert_eq!(find_word_forward(text, 3, None), 4);
		assert_eq!(find_word_forward(text, 4, None), 7);
	}

	#[test]
	fn forward_path() {
		let text = "path/to/file";
		assert_eq!(find_word_forward(text, 0, None), 4);
		assert_eq!(find_word_forward(text, 4, None), 5);
		assert_eq!(find_word_forward(text, 5, None), 7);
		assert_eq!(find_word_forward(text, 7, None), 8);
		assert_eq!(find_word_forward(text, 8, None), 12);
	}

	#[test]
	fn forward_cjk_mixed() {
		let text = "你好世界 test";
		let first_end = find_word_forward(text, 0, None);
		assert!(first_end > 0);
		assert!(first_end <= 4);
		// Walk to end
		let mut pos = 0;
		while pos < utf16_len(text) {
			let next = find_word_forward(text, pos, None);
			if next == pos {
				break;
			}
			pos = next;
		}
		assert_eq!(pos, utf16_len(text));
	}

	#[test]
	fn forward_whitespace_at_boundaries() {
		let text = "  hello  ";
		assert_eq!(find_word_forward(text, 0, None), 7);
		assert_eq!(find_word_forward(text, 7, None), 9);
	}

	#[test]
	fn forward_punctuation_run() {
		let text = "foo...bar";
		assert_eq!(find_word_forward(text, 0, None), 3);
		assert_eq!(find_word_forward(text, 3, None), 6);
		assert_eq!(find_word_forward(text, 6, None), 9);
	}

	#[test]
	fn forward_cursor_at_end_returns_end() {
		assert_eq!(find_word_forward("hello", 5, None), 5);
	}

	// ---- atomic segments ------------------------------------------------------

	fn word(s: &str) -> WordSegment {
		WordSegment { segment: s.to_string(), is_word_like: true }
	}
	fn nonword(s: &str) -> WordSegment {
		WordSegment { segment: s.to_string(), is_word_like: false }
	}

	#[test]
	fn atomic_segments() {
		let marker = "[paste #1 +5 lines]";
		let text = format!("hello {} world", marker);

		// The functions slice text before calling segment(), so we map each
		// expected substring to its pre-split segments.
		let mut segment_map: HashMap<&str, Vec<WordSegment>> = HashMap::new();
		segment_map.insert(
			text.as_str(),
			vec![word("hello"), nonword(" "), word(marker), nonword(" "), word("world")],
		);
		// backward from end: slice(0, len) = full text
		segment_map.insert(
			&text[..text.len()],
			vec![word("hello"), nonword(" "), word(marker), nonword(" "), word("world")],
		);
		// backward from 26: slice(0, 26) = "hello [paste #1 +5 lines] "
		let before26 = &text[..26];
		segment_map.insert(before26, vec![word("hello"), nonword(" "), word(marker), nonword(" ")]);
		// forward from 6: slice(6) = "[paste #1 +5 lines] world"
		let after6 = &text[6..];
		segment_map.insert(after6, vec![word(marker), nonword(" "), word("world")]);

		let segment = |input: &str| segment_map.get(input).cloned().unwrap_or_default();
		let is_atomic = |s: &str| s == marker;
		let opts = WordNavigationOptions {
			segment: Some(&segment),
			is_atomic_segment: Some(&is_atomic),
		};

		// backward skips word then stops before atomic marker
		assert_eq!(find_word_backward(&text, utf16_len(&text), Some(&opts)), 26);
		// backward skips whitespace then atomic marker as one unit
		assert_eq!(find_word_backward(&text, 26, Some(&opts)), 6);
		// forward skips atomic marker as one unit
		assert_eq!(find_word_forward(&text, 6, Some(&opts)), 6 + utf16_len(marker));
	}
}
