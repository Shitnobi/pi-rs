//! Line diff and unified patch generation.
//!
//! Exact port of the `diff` npm package (v8.0.4) subset used by pi:
//! - `Diff.diffLines` (tokenized Myers diff with diagonal pruning,
//!   `libesm/diff/line.js` + `libesm/diff/base.js`)
//! - `structuredPatch` / `formatPatch` / `createTwoFilesPatch`
//!   (`libesm/patch/create.js`)
//!
//! The original pi implementation calls these via `Diff.diffLines` and
//! `Diff.createTwoFilesPatch(..., { headerOptions: FILE_HEADERS_ONLY })`.

use serde::Serialize;

/// One diff component, equivalent to jsdiff's `{ value, added, removed, count }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub value: String,
    pub added: bool,
    pub removed: bool,
    pub count: usize,
}

/// Tokenize into lines that keep their trailing newline, merged exactly like
/// jsdiff's line tokenizer (`value.split(/(\n|\r\n)/)` + merge + drop empties).
fn tokenize(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    // JS `value.split(/(\n|\r\n)/)`: content parts separated by newline
    // tokens. Splitting on '\n' in Rust yields the same parts, except a
    // "\r\n" separator leaves its '\r' at the end of the preceding part;
    // re-appending '\n' below reproduces it exactly.
    let mut parts: Vec<&str> = value.split('\n').collect();
    // Ignore the final empty token that occurs if the string ends with a newline.
    let had_trailing_newline = value.ends_with('\n');
    if had_trailing_newline {
        parts.pop();
    }
    // Merge content and line separators into single tokens: every part had a
    // separator after it unless it is the final part of a string that does
    // not end with a newline.
    let n = parts.len();
    let mut tokens: Vec<String> = Vec::with_capacity(n);
    for (i, part) in parts.iter().enumerate() {
        let mut t = part.to_string();
        if had_trailing_newline || i + 1 < n {
            t.push('\n');
        }
        tokens.push(t);
    }
    tokens
}

#[derive(Debug, Clone)]
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    prev: Option<usize>,
}

#[derive(Debug, Clone)]
struct PathNode {
    old_pos: i64,
    last_component: Option<usize>,
}

struct Myers<'a> {
    old_tokens: &'a [String],
    new_tokens: &'a [String],
    arena: Vec<Component>,
}

impl<'a> Myers<'a> {
    fn add_to_path(&mut self, path: &PathNode, added: bool, removed: bool, old_pos_inc: i64) -> PathNode {
        let last = path.last_component;
        let new_comp = if let Some(idx) = last {
            let c = &self.arena[idx];
            if c.added == added && c.removed == removed {
                Component {
                    count: c.count + 1,
                    added,
                    removed,
                    prev: c.prev,
                }
            } else {
                Component { count: 1, added, removed, prev: last }
            }
        } else {
            Component { count: 1, added, removed, prev: None }
        };
        self.arena.push(new_comp);
        PathNode {
            old_pos: path.old_pos + old_pos_inc,
            last_component: Some(self.arena.len() - 1),
        }
    }

    fn extract_common(&mut self, base_path: &mut PathNode, diagonal_path: i64) -> i64 {
        let new_len = self.new_tokens.len() as i64;
        let old_len = self.old_tokens.len() as i64;
        let mut old_pos = base_path.old_pos;
        let mut new_pos = old_pos - diagonal_path;
        let mut common_count: usize = 0;
        while new_pos + 1 < new_len
            && old_pos + 1 < old_len
            && self.old_tokens[(old_pos + 1) as usize] == self.new_tokens[(new_pos + 1) as usize]
        {
            new_pos += 1;
            old_pos += 1;
            common_count += 1;
        }
        if common_count > 0 {
            // jsdiff always appends a new common component here (no merging).
            let prev = base_path.last_component;
            self.arena.push(Component { count: common_count, added: false, removed: false, prev });
            base_path.last_component = Some(self.arena.len() - 1);
        }
        base_path.old_pos = old_pos;
        new_pos
    }

    fn build_values(&self, last_component: Option<usize>) -> Vec<Change> {
        let mut components: Vec<usize> = Vec::new();
        let mut cur = last_component;
        while let Some(idx) = cur {
            components.push(idx);
            cur = self.arena[idx].prev;
        }
        components.reverse();

        let mut out: Vec<Change> = Vec::new();
        let mut new_pos: usize = 0;
        let mut old_pos: usize = 0;
        for idx in components {
            let comp = &self.arena[idx];
            let count = comp.count;
            if !comp.removed {
                let value: String =
                    self.new_tokens[new_pos..(new_pos + count).min(self.new_tokens.len())].concat();
                new_pos += count;
                if !comp.added {
                    old_pos += count;
                }
                out.push(Change { value, added: comp.added, removed: false, count });
            } else {
                let value: String =
                    self.old_tokens[old_pos..(old_pos + count).min(self.old_tokens.len())].concat();
                old_pos += count;
                out.push(Change { value, added: false, removed: true, count });
            }
        }
        out
    }
}

/// Port of jsdiff `diffLines(oldStr, newStr)`.
pub fn diff_lines(old_str: &str, new_str: &str) -> Vec<Change> {
    let old_tokens = tokenize(old_str);
    let new_tokens = tokenize(new_str);

    let old_len = old_tokens.len() as i64;
    let new_len = new_tokens.len() as i64;

    let mut myers = Myers {
        old_tokens: &old_tokens,
        new_tokens: &new_tokens,
        arena: Vec::new(),
    };

    let mut best_path: std::collections::HashMap<i64, PathNode> = std::collections::HashMap::new();
    let seed = PathNode { old_pos: -1, last_component: None };
    let mut seed = seed;
    let seed_new_pos = myers.extract_common(&mut seed, 0);
    if seed.old_pos + 1 >= old_len && seed_new_pos + 1 >= new_len {
        return myers.build_values(seed.last_component);
    }
    best_path.insert(0, seed);

    let mut min_diagonal_to_consider = i64::MIN;
    let mut max_diagonal_to_consider = i64::MAX;

    let mut edit_length: i64 = 1;
    let max_edit_length = new_len + old_len;

    while edit_length <= max_edit_length {
        let mut diagonal_path = std::cmp::max(min_diagonal_to_consider, -edit_length);
        while diagonal_path <= std::cmp::min(max_diagonal_to_consider, edit_length) {
            let remove_path = best_path.remove(&(diagonal_path - 1));
            let add_path = best_path.get(&(diagonal_path + 1)).cloned();

            let can_add = match &add_path {
                Some(ap) => {
                    let add_path_new_pos = ap.old_pos - diagonal_path;
                    (0..new_len).contains(&add_path_new_pos)
                }
                None => false,
            };
            let can_remove = match &remove_path {
                Some(rp) => rp.old_pos + 1 < old_len,
                None => false,
            };

            if !can_add && !can_remove {
                // Prune this diagonal.
                best_path.remove(&diagonal_path);
                diagonal_path += 2;
                continue;
            }

            let base_path = if !can_remove || (can_add && remove_path.as_ref().unwrap().old_pos < add_path.as_ref().unwrap().old_pos) {
                myers.add_to_path(add_path.as_ref().unwrap(), true, false, 0)
            } else {
                myers.add_to_path(remove_path.as_ref().unwrap(), false, true, 1)
            };

            let mut base_path = base_path;
            let new_pos = myers.extract_common(&mut base_path, diagonal_path);

            if base_path.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return myers.build_values(base_path.last_component);
            } else {
                if base_path.old_pos + 1 >= old_len {
                    max_diagonal_to_consider =
                        std::cmp::min(max_diagonal_to_consider, diagonal_path - 1);
                }
                if new_pos + 1 >= new_len {
                    min_diagonal_to_consider =
                        std::cmp::max(min_diagonal_to_consider, diagonal_path + 1);
                }
                best_path.insert(diagonal_path, base_path);
            }

            diagonal_path += 2;
        }
        edit_length += 1;
    }

    // Unreachable when maxEditLength = oldLen + newLen (the greedy Myers
    // search always terminates). Mirror jsdiff's undefined return as empty.
    Vec::new()
}

/// jsdiff `splitLines` (patch/create.js): lines with their trailing newline;
/// the last line keeps no newline if the text does not end with one.
fn patch_split_lines(text: &str) -> Vec<String> {
    let has_trailing_nl = text.ends_with('\n');
    let mut result: Vec<String> = text.split('\n').map(|l| format!("{}\n", l)).collect();
    if has_trailing_nl {
        result.pop();
    } else {
        let last = result.pop().unwrap_or_default();
        result.push(last[..last.len().saturating_sub(1)].to_string());
    }
    result
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    #[serde(rename = "oldStart")]
    pub old_start: i64,
    #[serde(rename = "oldLines")]
    pub old_lines: i64,
    #[serde(rename = "newStart")]
    pub new_start: i64,
    #[serde(rename = "newLines")]
    pub new_lines: i64,
    pub lines: Vec<String>,
}

#[derive(Debug)]
pub struct StructuredPatch {
    pub old_file_name: String,
    pub new_file_name: String,
    pub hunks: Vec<Hunk>,
}

/// Port of jsdiff `structuredPatch` (default context = 4).
pub fn structured_patch(
    old_file_name: &str,
    new_file_name: &str,
    old_str: &str,
    new_str: &str,
    context: usize,
) -> StructuredPatch {
    let mut diff = diff_lines(old_str, new_str);
    // Track lines per change like the original (cached on the object).
    let mut lines_per_change: Vec<Vec<String>> = diff
        .iter()
        .map(|c| patch_split_lines(&c.value))
        .collect();
    // Append an empty change to make cleanup easier.
    diff.push(Change { value: String::new(), added: false, removed: false, count: 0 });
    lines_per_change.push(Vec::new());

    let hunks: Vec<Hunk> = {
        let mut hunks: Vec<Hunk> = Vec::new();
        let mut old_range_start: i64 = 0;
        let mut new_range_start: i64 = 0;
        let mut cur_range: Vec<String> = Vec::new();
        let mut old_line: i64 = 1;
        let mut new_line: i64 = 1;

        for i in 0..diff.len() {
            let current = &diff[i];
            let lines = &lines_per_change[i];

            if current.added || current.removed {
                // If we have previous context, start with that
                if old_range_start == 0 {
                    let prev_lines: &[String] = if i > 0 { &lines_per_change[i - 1] } else { &[] };
                    old_range_start = old_line;
                    new_range_start = new_line;
                    if !prev_lines.is_empty() {
                        if context > 0 {
                            let start = prev_lines.len().saturating_sub(context);
                            cur_range = prev_lines[start..]
                                .iter()
                                .map(|entry| format!(" {}", entry))
                                .collect();
                        } else {
                            cur_range = Vec::new();
                        }
                        old_range_start -= cur_range.len() as i64;
                        new_range_start -= cur_range.len() as i64;
                    }
                }
                // Output our changes
                for line in lines {
                    cur_range.push(format!("{}{}", if current.added { "+" } else { "-" }, line));
                }
                // Track the updated file position
                if current.added {
                    new_line += lines.len() as i64;
                } else {
                    old_line += lines.len() as i64;
                }
            } else {
                // Identical context lines. Track line changes
                if old_range_start != 0 {
                    // Close out any changes that have been output (or join overlapping)
                    if lines.len() <= context * 2 && i < diff.len() - 2 {
                        // Overlapping
                        for line in lines {
                            cur_range.push(format!(" {}", line));
                        }
                    } else {
                        // end the range and output
                        let context_size = std::cmp::min(lines.len(), context);
                        for line in &lines[..context_size] {
                            cur_range.push(format!(" {}", line));
                        }
                        hunks.push(Hunk {
                            old_start: old_range_start,
                            old_lines: old_line - old_range_start + context_size as i64,
                            new_start: new_range_start,
                            new_lines: new_line - new_range_start + context_size as i64,
                            lines: std::mem::take(&mut cur_range),
                        });
                        old_range_start = 0;
                        new_range_start = 0;
                        cur_range = Vec::new();
                    }
                }
                old_line += lines.len() as i64;
                new_line += lines.len() as i64;
            }
        }

        // Step 2: eliminate the trailing `\n` from each line, add
        // "\ No newline at end of file" where needed.
        for hunk in &mut hunks {
            let mut i = 0;
            while i < hunk.lines.len() {
                if hunk.lines[i].ends_with('\n') {
                    let l = hunk.lines[i].clone();
                    hunk.lines[i] = l[..l.len() - 1].to_string();
                } else {
                    hunk.lines.insert(i + 1, "\\ No newline at end of file".to_string());
                    i += 1;
                }
                i += 1;
            }
        }

        hunks
    };

    StructuredPatch {
        old_file_name: old_file_name.to_string(),
        new_file_name: new_file_name.to_string(),
        hunks,
    }
}

/// Port of jsdiff `formatPatch` with `FILE_HEADERS_ONLY` header options
/// (includeIndex=false, includeUnderline=false, includeFileHeaders=true,
/// no old/new headers).
pub fn format_patch_file_headers_only(patch: &StructuredPatch) -> String {
    let mut ret: Vec<String> = Vec::new();
    ret.push(format!("--- {}", patch.old_file_name));
    ret.push(format!("+++ {}", patch.new_file_name));
    let mut hunks = patch.hunks.clone();
    for hunk in &mut hunks {
        // Unified Diff Format quirk: If the chunk size is 0,
        // the first number is one lower than one would expect.
        if hunk.old_lines == 0 {
            hunk.old_start -= 1;
        }
        if hunk.new_lines == 0 {
            hunk.new_start -= 1;
        }
        ret.push(format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        ));
        for line in &hunk.lines {
            ret.push(line.clone());
        }
    }
    let mut s = ret.join("\n");
    s.push('\n');
    s
}

/// Port of `Diff.createTwoFilesPatch(path, path, oldStr, newStr, undefined,
/// undefined, { context, headerOptions: FILE_HEADERS_ONLY })` as used by
/// pi's `generateUnifiedPatch`.
pub fn create_two_files_patch(
    old_file_name: &str,
    new_file_name: &str,
    old_str: &str,
    new_str: &str,
    context: usize,
) -> String {
    let patch = structured_patch(old_file_name, new_file_name, old_str, new_str, context);
    format_patch_file_headers_only(&patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(parts: &[Change]) -> Vec<(char, &str)> {
        parts
            .iter()
            .map(|c| {
                let tag = if c.added {
                    '+'
                } else if c.removed {
                    '-'
                } else {
                    ' '
                };
                (tag, c.value.as_str())
            })
            .collect()
    }

    #[test]
    fn tokenize_lines() {
        assert_eq!(tokenize("a\nb\n"), vec!["a\n", "b\n"]);
        assert_eq!(tokenize("a\nb"), vec!["a\n", "b"]);
        assert_eq!(tokenize("\n"), vec!["\n"]);
        assert_eq!(tokenize(""), Vec::<String>::new());
        assert_eq!(tokenize("a"), vec!["a"]);
        // \r stays with the line content, \r\n separator merged
        assert_eq!(tokenize("a\r\nb"), vec!["a\r\n", "b"]);
    }

    #[test]
    fn diff_lines_basic() {
        let parts = diff_lines("a\nb\nc\n", "a\nB\nc\n");
        let tagged = changed(&parts);
        assert_eq!(tagged, vec![(' ', "a\n"), ('-', "b\n"), ('+', "B\n"), (' ', "c\n")]);
    }

    #[test]
    fn diff_lines_append() {
        let parts = diff_lines("a\n", "a\nb\n");
        assert_eq!(changed(&parts), vec![(' ', "a\n"), ('+', "b\n")]);
    }

    #[test]
    fn diff_lines_identity() {
        let parts = diff_lines("x\ny\n", "x\ny\n");
        assert_eq!(changed(&parts), vec![(' ', "x\ny\n")]);
        assert_eq!(parts[0].count, 2);
    }

    #[test]
    fn diff_lines_no_trailing_newline() {
        let parts = diff_lines("a", "b");
        assert_eq!(changed(&parts), vec![('-', "a"), ('+', "b")]);
    }

    #[test]
    fn unified_patch_format() {
        let patch = create_two_files_patch("f.txt", "f.txt", "Hello, world!", "Hello, testing!", 4);
        assert_eq!(
            patch,
            "--- f.txt\n+++ f.txt\n@@ -1,1 +1,1 @@\n-Hello, world!\n\\ No newline at end of file\n+Hello, testing!\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn unified_patch_context() {
        let old: String = (0..20).map(|i| format!("line{}\n", i)).collect();
        let new: String = (0..20)
            .map(|i| if i == 10 { "lineCHANGED\n".to_string() } else { format!("line{}\n", i) })
            .collect();
        let patch = create_two_files_patch("f", "f", &old, &new, 4);
        assert!(patch.contains("@@ -7,9 +7,9 @@"));
        assert!(patch.contains("+lineCHANGED"));
    }

    #[test]
    fn no_newline_marker() {
        let patch = create_two_files_patch("f", "f", "a\nb", "a\nB", 4);
        assert!(patch.contains("\\ No newline at end of file"));
    }
}
