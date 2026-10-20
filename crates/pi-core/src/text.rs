//! Text helpers ported from `src/utils/text.ts` (plus `stripJsonComments`
//! from `packages/coding-agent/src/utils/json.ts`).

/// Split a leading UTF-8 byte order mark from decoded text.
/// Port of `splitBom` in `src/utils/text.ts`. Returns `(bom, text)`.
pub fn split_bom(content: &str) -> (&str, &str) {
	if let Some(rest) = content.strip_prefix('\u{FEFF}') {
		("\u{FEFF}", rest)
	} else {
		("", content)
	}
}

/// Remove a leading UTF-8 byte order mark from decoded text.
pub fn strip_bom(content: &str) -> &str {
	split_bom(content).1
}

/// Sanitize binary output for display/storage. Port of
/// `sanitizeBinaryOutput` in `src/utils/shell.ts`: removes control
/// characters (except tab, newline, carriage return) and the Unicode
/// interlinear annotation characters U+FFF9..U+FFFB.
pub fn sanitize_binary_output(s: &str) -> String {
	s.chars()
		.filter(|&c| {
			let u = c as u32;
			!((0x00..=0x08).contains(&u)
				|| u == 0x0B
				|| u == 0x0C
				|| (0x0E..=0x1F).contains(&u)
				|| (0xFFF9..=0xFFFB).contains(&u))
		})
		.collect()
}

/// Strip `//` line comments and trailing commas from JSON, leaving string
/// literals untouched. Port of `stripJsonComments` in
/// `packages/coding-agent/src/utils/json.ts`:
///
/// ```ts
/// input
///   .replace(/"(?:\\.|[^"\\])*"|\/\/[^\n]*/g, (m) => (m[0] === '"' ? m : ""))
///   .replace(/"(?:\\.|[^"\\])*"|,(\s*[}\]])/g, (m, tail) => tail ?? m);
/// ```
///
/// Implemented as a hand-rolled two-pass scanner (the regex crate is not a
/// dependency). Divergence from the regex: a backslash inside a string
/// literal escapes any following character here, while JS `\\.` excludes
/// line terminators; the difference is only observable for invalid JSON
/// (a raw newline directly after a backslash inside a would-be string
/// literal, which `JSON.parse` rejects anyway).
pub fn strip_json_comments(input: &str) -> String {
	strip_trailing_commas(&strip_line_comments(input))
}

/// Byte length of the char starting at byte `i` (the scanners only stop at
/// ASCII bytes, which are always UTF-8 char boundaries).
fn char_len_at(input: &str, i: usize) -> usize {
	input[i..].chars().next().map_or(1, char::len_utf8)
}

/// Byte index of the closing quote of the string literal starting at
/// `start`, or `None` when the literal is unterminated (the original
/// regexes would then not treat it as a string literal at all).
fn string_literal_end(input: &str, start: usize) -> Option<usize> {
	let bytes = input.as_bytes();
	let mut i = start + 1;
	while i < bytes.len() {
		match bytes[i] {
			b'\\' => {
				if i + 1 >= bytes.len() {
					return None;
				}
				i += 1 + char_len_at(input, i + 1);
			}
			b'"' => return Some(i),
			_ => i += char_len_at(input, i),
		}
	}
	None
}

/// Pass 1: drop `//`-to-EOL comments, keep string literals verbatim.
fn strip_line_comments(input: &str) -> String {
	let bytes = input.as_bytes();
	let mut out = String::with_capacity(input.len());
	let mut i = 0;
	while i < bytes.len() {
		match bytes[i] {
			b'"' => match string_literal_end(input, i) {
				Some(end) => {
					out.push_str(&input[i..=end]);
					i = end + 1;
				}
				None => {
					out.push('"');
					i += 1;
				}
			},
			b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
				// `\/\/[^\n]*` — drop up to (not including) the next newline.
				while i < bytes.len() && bytes[i] != b'\n' {
					i += 1;
				}
			}
			_ => {
				let len = char_len_at(input, i);
				out.push_str(&input[i..i + len]);
				i += len;
			}
		}
	}
	out
}

/// JS regex `\s` (includes `\uFEFF`; excludes `\u0085`, unlike Unicode
/// `White_Space`).
fn is_js_whitespace(c: char) -> bool {
	matches!(c,
		'\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}'
		| '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}'
		| '\u{3000}' | '\u{FEFF}')
}

/// Pass 2: drop trailing commas before `}`/`]`, keeping the whitespace and
/// the bracket (the regex tail group).
fn strip_trailing_commas(input: &str) -> String {
	let bytes = input.as_bytes();
	let mut out = String::with_capacity(input.len());
	let mut i = 0;
	while i < bytes.len() {
		match bytes[i] {
			b'"' => match string_literal_end(input, i) {
				Some(end) => {
					out.push_str(&input[i..=end]);
					i = end + 1;
				}
				None => {
					out.push('"');
					i += 1;
				}
			},
			b',' => {
				// `,(\s*[}\]])` — drop the comma, keep the `\s*[}\]]` tail.
				let mut j = i + 1;
				while j < bytes.len() {
					let c = input[j..].chars().next().unwrap_or(' ');
					if is_js_whitespace(c) {
						j += c.len_utf8();
					} else {
						break;
					}
				}
				if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
					out.push_str(&input[i + 1..=j]);
					i = j + 1;
				} else {
					out.push(',');
					i += 1;
				}
			}
			_ => {
				let len = char_len_at(input, i);
				out.push_str(&input[i..i + len]);
				i += len;
			}
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn split_bom_behaviour() {
		assert_eq!(split_bom("\u{FEFF}abc"), ("\u{FEFF}", "abc"));
		assert_eq!(split_bom("abc"), ("", "abc"));
		assert_eq!(split_bom("a\u{FEFF}b"), ("", "a\u{FEFF}b"));
		assert_eq!(strip_bom("\u{FEFF}abc"), "abc");
	}

	#[test]
	fn strip_json_comments_line_comments() {
		// Comment after a value; the newline is kept (`//[^\n]*`).
		assert_eq!(
			strip_json_comments("{\"a\": 1, // trailing\n\"b\": 2}"),
			"{\"a\": 1, \n\"b\": 2}"
		);
		// Whole-line comment.
		assert_eq!(strip_json_comments("// header\n{\"a\": 1}"), "\n{\"a\": 1}");
		// Comment at EOF without a trailing newline.
		assert_eq!(strip_json_comments("{\"a\": 1} // eof"), "{\"a\": 1} ");
		// `//` before any string content.
		assert_eq!(strip_json_comments("//x\n//y\n1"), "\n\n1");
	}

	#[test]
	fn strip_json_comments_leaves_strings_alone() {
		// `//` inside a string literal.
		assert_eq!(
			strip_json_comments("{\"u\": \"http://example.com\"}"),
			"{\"u\": \"http://example.com\"}"
		);
		// `,}` inside a string literal (pass 2).
		assert_eq!(
			strip_json_comments("{\"s\": \"a,}\"}"),
			"{\"s\": \"a,}\"}"
		);
		// Escaped quote inside a string, `//` after it.
		assert_eq!(
			strip_json_comments("{\"s\": \"a\\\"b // x\"}"),
			"{\"s\": \"a\\\"b // x\"}"
		);
		// Backslash-escaped slash (`\/`) inside a string.
		assert_eq!(strip_json_comments(r#"{"s": "a\/b"}"#), r#"{"s": "a\/b"}"#);
	}

	#[test]
	fn strip_json_comments_trailing_commas() {
		assert_eq!(strip_json_comments("{\"a\": 1,}"), "{\"a\": 1}");
		assert_eq!(strip_json_comments("[1, 2,]"), "[1, 2]");
		// Whitespace between comma and bracket is part of the kept tail.
		assert_eq!(strip_json_comments("{\"a\": [1, 2, ], }"), "{\"a\": [1, 2 ] }");
		// Commas not followed by `}`/`]` are kept.
		assert_eq!(
			strip_json_comments("{\"a\": 1, \"b\": 2}"),
			"{\"a\": 1, \"b\": 2}"
		);
		// Consecutive commas: only the one before the bracket is dropped.
		assert_eq!(strip_json_comments("[1,,]"), "[1,]");
		// Empty containers.
		assert_eq!(strip_json_comments("[,]"), "[]");
	}

	#[test]
	fn strip_json_comments_unterminated_string() {
		// The regexes only recognise closed string literals; an unterminated
		// one passes through untouched.
		assert_eq!(strip_json_comments("\"abc"), "\"abc");
		assert_eq!(strip_json_comments("\"a\\"), "\"a\\");
	}

	#[test]
	fn strip_json_comments_round_trips_through_json() {
		let stripped = strip_json_comments(
			"{\n\t// provider config\n\t\"providers\": {\"a\": {\"name\": \"A\",}}, // end\n}",
		);
		let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stripped);
		assert_eq!(
			parsed.unwrap()["providers"]["a"]["name"].as_str(),
			Some("A")
		);
	}
}
