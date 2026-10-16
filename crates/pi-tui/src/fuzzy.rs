//! Fuzzy matching utilities — port of `fuzzy.ts`.
//!
//! Matches if all query characters appear in order (not necessarily
//! consecutive). Lower score = better match.
//!
//! Operates on UTF-16 code units of the lowercased strings so that indexing
//! (`indexOf`, `slice` positions) matches the JS implementation exactly.

/// Result of [`fuzzy_match`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

/// JS `String.prototype.toLowerCase()` semantics: full Unicode lowercasing
/// (includes final-sigma context), collected as UTF-16 code units.
fn lower_utf16(s: &str) -> Vec<u16> {
    s.to_lowercase().encode_utf16().collect()
}

/// `haystack.indexOf(needle, from)` over UTF-16 code units.
fn index_of_from(haystack: &[u16], needle: u16, from: usize) -> Option<usize> {
    let start = from.min(haystack.len());
    haystack[start..]
        .iter()
        .position(|&c| c == needle)
        .map(|i| i + start)
}

/// Decode a single UTF-16 code unit to a char (surrogates map to `None`).
fn unit_to_char(u: u16) -> Option<char> {
    char::from_u32(u as u32).filter(|c| (*c as u32) < 0x80 || !matches!(u, 0xd800..=0xdfff))
}

/// `/[\s\-_./:]/.test(char)` — JS `\s` set plus the explicit boundary chars.
fn is_boundary_char(u: u16) -> bool {
    let Some(c) = unit_to_char(u) else {
        return false;
    };
    matches!(c, '-' | '_' | '.' | '/' | ':') || crate::utils::is_js_whitespace(c)
}

/// `matchQuery` inner closure of `fuzzyMatch`.
fn match_query(query: &[u16], text_lower: &[u16]) -> FuzzyMatch {
    if query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    if query.len() > text_lower.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    let mut query_index = 0usize;
    let mut score = 0.0f64;
    let mut last_match_index: i64 = -1;
    let mut consecutive_matches: i64 = 0;

    while query_index < query.len() {
        let Some(i) = index_of_from(text_lower, query[query_index], (last_match_index + 1) as usize)
        else {
            break;
        };
        let i = i as i64;

        let is_word_boundary = i == 0 || is_boundary_char(text_lower[(i - 1) as usize]);

        // Reward consecutive matches
        if last_match_index == i - 1 {
            consecutive_matches += 1;
            score -= (consecutive_matches * 5) as f64;
        } else {
            consecutive_matches = 0;
            // Penalize gaps
            if last_match_index >= 0 {
                score += ((i - last_match_index - 1) * 2) as f64;
            }
        }

        // Reward word boundary matches
        if is_word_boundary {
            score -= 10.0;
        }

        // Slight penalty for later matches
        score += i as f64 * 0.1;

        last_match_index = i;
        query_index += 1;
    }

    if query_index < query.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    if query == text_lower {
        score -= 100.0;
    }

    FuzzyMatch {
        matches: true,
        score,
    }
}

/// `queryLower.match(/^(?<letters>[a-z]+)(?<digits>[0-9]+)$/)` or the
/// digits-then-letters form; returns the swapped query.
fn swapped_query(query_lower: &[u16]) -> Option<Vec<u16>> {
    let is_letter = |&u: &u16| (b'a' as u16..=b'z' as u16).contains(&u);
    let is_digit = |&u: &u16| (b'0' as u16..=b'9' as u16).contains(&u);

    // letters-then-digits form
    if let Some(split) = query_lower
        .iter()
        .position(|&u| is_digit(&u))
        .filter(|&first_digit| {
            first_digit > 0
                && query_lower[..first_digit].iter().all(|&u| is_letter(&u))
                && query_lower[first_digit..].iter().all(|&u| is_digit(&u))
        })
    {
        let digits = query_lower[split..].to_vec();
        let letters = query_lower[..split].to_vec();
        return Some([digits, letters].concat());
    }
    // digits-then-letters form
    if let Some(split) = query_lower
        .iter()
        .position(|&u| is_letter(&u))
        .filter(|&first_letter| {
            first_letter > 0
                && query_lower[..first_letter].iter().all(|&u| is_digit(&u))
                && query_lower[first_letter..].iter().all(|&u| is_letter(&u))
        })
    {
        let digits = query_lower[..split].to_vec();
        let letters = query_lower[split..].to_vec();
        return Some([letters, digits].concat());
    }
    None
}

/// Port of `fuzzyMatch(query, text)`.
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = lower_utf16(query);
    let text_lower = lower_utf16(text);

    let primary_match = match_query(&query_lower, &text_lower);
    if primary_match.matches {
        return primary_match;
    }

    let Some(swapped) = swapped_query(&query_lower) else {
        return primary_match;
    };
    let swapped_match = match_query(&swapped, &text_lower);
    if !swapped_match.matches {
        return primary_match;
    }

    FuzzyMatch {
        matches: true,
        score: swapped_match.score + 5.0,
    }
}

/// Split `query.trim()` on `/[\s/]+/` and drop empty tokens.
fn query_tokens(query: &str) -> Vec<String> {
    let trimmed = crate::utils::js_trim(query);
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in trimmed.chars() {
        if crate::utils::is_js_whitespace(c) || c == '/' {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Port of `fuzzyFilter(items, query, getText)`: filter and sort items by
/// fuzzy match quality (best first; stable, like JS `Array#sort`).
pub fn fuzzy_filter<'a, T>(
    items: &'a [T],
    query: &str,
    get_text: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    if crate::utils::js_trim(query).is_empty() {
        return items.iter().collect();
    }

    let tokens = query_tokens(query);
    if tokens.is_empty() {
        return items.iter().collect();
    }

    let mut results: Vec<(&T, f64)> = Vec::new();
    for item in items {
        let text = get_text(item);
        let mut total_score = 0.0f64;
        let mut all_match = true;
        for token in &tokens {
            let m = fuzzy_match(token, text);
            if m.matches {
                total_score += m.score;
            } else {
                all_match = false;
                break;
            }
        }
        if all_match {
            results.push((item, total_score));
        }
    }

    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    results.into_iter().map(|(item, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything_with_score_0() {
        let result = fuzzy_match("", "anything");
        assert!(result.matches);
        assert_eq!(result.score, 0.0);
    }

    #[test]
    fn query_longer_than_text_does_not_match() {
        assert!(!fuzzy_match("longquery", "short").matches);
    }

    #[test]
    fn exact_match_has_good_score() {
        let result = fuzzy_match("test", "test");
        assert!(result.matches);
        assert!(result.score < 0.0);
    }

    #[test]
    fn characters_must_appear_in_order() {
        assert!(fuzzy_match("abc", "aXbXc").matches);
        assert!(!fuzzy_match("abc", "cba").matches);
    }

    #[test]
    fn case_insensitive_matching() {
        assert!(fuzzy_match("ABC", "abc").matches);
        assert!(fuzzy_match("abc", "ABC").matches);
    }

    #[test]
    fn consecutive_matches_score_better_than_scattered() {
        let consecutive = fuzzy_match("foo", "foobar");
        let scattered = fuzzy_match("foo", "f_o_o_bar");
        assert!(consecutive.matches);
        assert!(scattered.matches);
        assert!(consecutive.score < scattered.score);
    }

    #[test]
    fn word_boundary_matches_score_better() {
        let at_boundary = fuzzy_match("fb", "foo-bar");
        let not_at_boundary = fuzzy_match("fb", "afbx");
        assert!(at_boundary.matches);
        assert!(not_at_boundary.matches);
        assert!(at_boundary.score < not_at_boundary.score);
    }

    #[test]
    fn matches_swapped_alpha_numeric_tokens() {
        assert!(fuzzy_match("codex52", "gpt-5.2-codex").matches);
    }

    #[test]
    fn filter_empty_query_returns_all_items() {
        let items = vec!["apple", "banana", "cherry"];
        let result = fuzzy_filter(&items, "", |x| x);
        assert_eq!(result, vec![&"apple", &"banana", &"cherry"]);
    }

    #[test]
    fn filter_sorts_by_match_quality() {
        let items = vec!["a_p_p", "app", "application"];
        let result = fuzzy_filter(&items, "app", |x| x);
        assert_eq!(*result[0], "app");
    }

    #[test]
    fn filter_prioritizes_exact_matches_over_longer_prefixes() {
        let items = vec!["clone", "cl"];
        let result = fuzzy_filter(&items, "cl", |x| x);
        assert_eq!(result, vec![&"cl", &"clone"]);
    }

    #[test]
    fn filter_matches_slash_separated_queries_against_reordered_text() {
        // The upstream test composes `${model.id} ${model.provider}` as the text.
        let items = vec![("gpt-5.5", "openai-codex")];
        let composed: Vec<String> = items.iter().map(|(id, p)| format!("{id} {p}")).collect();
        let result = fuzzy_filter(&composed, "openai-codex/gpt-5.5", |s| s);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "gpt-5.5 openai-codex");
    }
}
