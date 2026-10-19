//! Tool discovery: a BM25 ranker over tool metadata, shared by `searchTools()`
//! in codemode scripts and the optional `tool_search` tool.
//!
//! Port of `src/extensions/tool-search/tool.ts` (packages/coding-agent), the
//! pure parts: tokenization, document building and the ranker. The tool
//! definition itself (`createToolSearchToolDefinition`) needs the extension
//! API and stays with the extension layer.
//!
//! TS line → Rust item map:
//! - L11–12  `TOOL_SEARCH_TOOL_NAME` / `DEFAULT_TOOL_SEARCH_LIMIT` → consts
//! - L14–25  `ToolSearchDocument` / `ToolSearchMatch` → structs
//! - L27–30  `ToolRanker` → [`ToolRanker`]
//! - L32–58  `STOP_WORDS` / `stem` / `tokenize` → [`tokenize`]
//! - L60–80  `isObject` / `schemaText` → `schema_text`
//! - L82–96  `createToolSearchDocument` → [`create_tool_search_document`]
//! - L98–146 `Bm25Ranker` → [`Bm25Ranker`]
//! - L222    `TOOL_SEARCH_DESCRIPTION` → [`TOOL_SEARCH_DESCRIPTION`]

use serde_json::Value;

/// `TOOL_SEARCH_TOOL_NAME`.
pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";
/// `DEFAULT_TOOL_SEARCH_LIMIT`.
pub const DEFAULT_TOOL_SEARCH_LIMIT: usize = 8;

/// A tool as the ranker sees it: its name and the text built by
/// [`create_tool_search_document`].
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSearchDocument {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSearchMatch {
    pub name: String,
    pub score: f64,
}

/// Ranks tools for a query. BM25 today; a hybrid ranker with embeddings can
/// replace it.
pub trait ToolRanker {
    fn rank(&self, query: &str, documents: &[ToolSearchDocument], limit: usize) -> Vec<ToolSearchMatch>;
}

/// The search text of a tool: `Pick<ToolInfo, "name" | "description" |
/// "parameters">` in TS — kept as an owned struct so the parity harness can
/// build it straight from JSON.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// `ToolNamespace`: a group of related tools, such as the tools of one MCP
/// server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolNamespace {
    pub name: String,
    pub description: Option<String>,
    pub instructions: Option<String>,
}

/// JS `String.prototype.trim` (non-empty check, for `createToolSearchDocument`).
fn js_trim_nonempty(s: &str) -> bool {
    !s.chars().all(crate::js_string::is_js_whitespace_or_terminator)
}

fn is_stop_word(term: &str) -> bool {
    matches!(
        term,
        "a" | "an" | "and" | "are" | "as" | "at" | "be" | "by" | "for" | "from" | "in" | "is" | "it" | "of" | "on"
            | "or" | "that" | "the" | "this" | "to" | "with"
    )
}

/// Naive singular form, so `issues` matches `issue` and `searches` matches
/// `search`. `stem` (L38). Tokens are `[a-z0-9]+`, so byte slicing is safe.
fn stem(term: &str) -> String {
    let n = term.len();
    if n > 4 && term.ends_with("ies") {
        return format!("{}y", &term[..n - 3]);
    }
    if n > 4
        && (term.ends_with("ches")
            || term.ends_with("shes")
            || term.ends_with("sses")
            || term.ends_with("xes")
            || term.ends_with("zes"))
    {
        return term[..n - 2].to_string();
    }
    if n > 3 && term.ends_with('s') && !term.ends_with("ss") {
        return term[..n - 1].to_string();
    }
    term.to_string()
}

/// Lowercase terms, split at camelCase boundaries and non-alphanumerics,
/// without stop words. Port of `tokenize` (L45): the two camelCase splits run
/// on the original casing, then `toLowerCase()`, then a split on runs of
/// non-`[a-z0-9]` characters.
pub fn tokenize(text: &str) -> Vec<String> {
    // Pass 1: `([a-z0-9])([A-Z])` → "$1 $2".
    let mut pass1 = String::with_capacity(text.len() + 8);
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if let Some(prev) = prev {
            if (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && c.is_ascii_uppercase() {
                pass1.push(' ');
            }
        }
        pass1.push(c);
        prev = Some(c);
    }
    // Pass 2: `([A-Z]+)([A-Z][a-z])` → "$1 $2" — a space before the last
    // uppercase of a run when a lowercase follows it.
    let chars: Vec<char> = pass1.chars().collect();
    let mut pass2 = String::with_capacity(pass1.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0
            && c.is_ascii_uppercase()
            && chars[i - 1].is_ascii_uppercase()
            && chars.get(i + 1).map(|n| n.is_ascii_lowercase()).unwrap_or(false)
        {
            pass2.push(' ');
        }
        pass2.push(c);
    }
    let lower = pass2.to_lowercase();
    lower
        .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit())
        .filter(|term| !term.is_empty())
        .filter(|term| !is_stop_word(term))
        .map(stem)
        .collect()
}

/// Schema descriptions and property names, recursively (`schemaText`, L60).
fn schema_text(schema: &Value, parts: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else { return };
    if let Some(Value::String(description)) = obj.get("description") {
        parts.push(description.clone());
    }
    if let Some(Value::Object(properties)) = obj.get("properties") {
        for (name, property) in properties {
            parts.push(name.clone());
            schema_text(property, parts);
        }
    }
    if let Some(items) = obj.get("items") {
        schema_text(items, parts);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(variants)) = obj.get(key) {
            for variant in variants {
                schema_text(variant, parts);
            }
        }
    }
}

/// Search text of a tool: the name, the name with `_` as spaces, the
/// description, schema descriptions and property names, and the namespace
/// with its description and instructions. Port of `createToolSearchDocument`
/// (L82).
pub fn create_tool_search_document(tool: &ToolInfo, namespace: Option<&ToolNamespace>) -> ToolSearchDocument {
    let mut parts = vec![tool.name.clone(), tool.name.replace('_', " "), tool.description.clone()];
    schema_text(&tool.parameters, &mut parts);
    if let Some(namespace) = namespace {
        parts.push(namespace.name.clone());
        parts.push(namespace.description.clone().unwrap_or_default());
        parts.push(namespace.instructions.clone().unwrap_or_default());
    }
    let text = parts
        .into_iter()
        .filter(|part| js_trim_nonempty(part))
        .collect::<Vec<_>>()
        .join(" ");
    ToolSearchDocument { name: tool.name.clone(), text }
}

/// Okapi BM25 with the usual parameters. Ties keep document order (JS sort is
/// stable; so is Rust's). Port of `Bm25Ranker` (L98). The idf uses JS
/// `Math.log` (V8/fdlibm, see [`crate::js_math`]) — the system `ln` disagrees
/// with V8 for ~0.7% of inputs by 1 ulp, which would change BM25 scores in
/// model-visible output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25Ranker {
    k1: f64,
    b: f64,
}

impl Default for Bm25Ranker {
    fn default() -> Self {
        Bm25Ranker { k1: 1.2, b: 0.75 }
    }
}

impl Bm25Ranker {
    pub fn new() -> Bm25Ranker {
        Bm25Ranker::default()
    }

    /// `new Bm25Ranker({ k1, b })`.
    pub fn with_params(k1: f64, b: f64) -> Bm25Ranker {
        Bm25Ranker { k1, b }
    }

    pub fn rank(&self, query: &str, documents: &[ToolSearchDocument], limit: usize) -> Vec<ToolSearchMatch> {
        // [...new Set(tokenize(query))] — dedupe, first occurrence order.
        let mut query_terms: Vec<String> = Vec::new();
        for term in tokenize(query) {
            if !query_terms.contains(&term) {
                query_terms.push(term);
            }
        }
        if query_terms.is_empty() || documents.is_empty() || limit == 0 {
            return Vec::new();
        }
        let term_counts: Vec<std::collections::HashMap<String, usize>> = documents
            .iter()
            .map(|document| {
                let mut counts = std::collections::HashMap::new();
                for term in tokenize(&document.text) {
                    *counts.entry(term).or_insert(0) += 1;
                }
                counts
            })
            .collect();
        let lengths: Vec<usize> = term_counts.iter().map(|counts| counts.values().sum()).collect();
        let total_length: usize = lengths.iter().sum();
        // `sum / documents.length || 1` — 0 becomes 1 (all-empty documents).
        let average_length = {
            let avg = total_length as f64 / documents.len() as f64;
            if avg == 0.0 { 1.0 } else { avg }
        };
        let idf: std::collections::HashMap<&str, f64> = query_terms
            .iter()
            .map(|term| {
                let frequency = term_counts.iter().filter(|counts| counts.contains_key(term)).count();
                let idf = crate::js_math::js_math_log(1.0 + (documents.len() as f64 - frequency as f64 + 0.5) / (frequency as f64 + 0.5));
                (term.as_str(), idf)
            })
            .collect();
        let mut matches: Vec<ToolSearchMatch> = Vec::new();
        for (index, document) in documents.iter().enumerate() {
            let mut score = 0.0;
            for term in &query_terms {
                let Some(&count) = term_counts[index].get(term) else { continue };
                let norm = self.k1 * (1.0 - self.b + (self.b * lengths[index] as f64) / average_length);
                score += idf[term.as_str()] * ((count as f64 * (self.k1 + 1.0)) / (count as f64 + norm));
            }
            if score > 0.0 {
                matches.push(ToolSearchMatch { name: document.name.clone(), score });
            }
        }
        matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        matches.truncate(limit);
        matches
    }
}

impl ToolRanker for Bm25Ranker {
    fn rank(&self, query: &str, documents: &[ToolSearchDocument], limit: usize) -> Vec<ToolSearchMatch> {
        Bm25Ranker::rank(self, query, documents, limit)
    }
}

/// The `tool_search` description (`TOOL_SEARCH_DESCRIPTION`, L222). It does
/// not list the searchable tools or their namespaces, so it stays the same
/// while tools are registered.
pub const TOOL_SEARCH_DESCRIPTION: &str = concat!(
    "# Tool discovery\n\n",
    "Searches over deferred tool metadata with BM25 and exposes matching tools for the next model call.\n\n",
    "Some of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`tool_search`) to search for the required tools. For MCP tool discovery, always use `tool_search`."
);

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(name: &str, text: &str) -> ToolSearchDocument {
        ToolSearchDocument { name: name.to_string(), text: text.to_string() }
    }

    // Expected values verified against the TS dist:
    // new Bm25Ranker().rank("file read", docs) etc.
    fn sample_docs() -> Vec<ToolSearchDocument> {
        vec![
            doc("read_file", "read file contents filesystem"),
            doc("write_file", "write file save disk"),
            doc("search_web", "search the web internet"),
        ]
    }

    #[test]
    fn tokenize_camel_case_and_separators() {
        assert_eq!(
            tokenize("FooBar_baz HTTPServer2 x-y"),
            vec!["foo", "bar", "baz", "http", "server2", "x", "y"]
        );
        assert_eq!(tokenize("ABCDef ghI"), vec!["abc", "def", "gh", "i"]);
        assert_eq!(
            tokenize("a1b2 camelCase snake_case kebab-case dots.periods"),
            vec!["a1b2", "camel", "case", "snake", "case", "kebab", "case", "dot", "period"]
        );
    }

    #[test]
    fn tokenize_stems_and_stop_words() {
        assert_eq!(
            tokenize("get_issues searches classes analyses the AND with"),
            vec!["get", "issue", "search", "class", "analyse"]
        );
        // Verified against TS: stem("buses") = "buse" — only `ies` and the
        // ches|shes|sses|xes|zes endings drop 2 chars; plain `s` drops 1.
        assert_eq!(tokenize("buses uses classes"), vec!["buse", "use", "class"]);
        // `ss` endings keep the s.
        assert_eq!(tokenize("process address"), vec!["process", "address"]);
        // Short words are not stemmed.
        assert_eq!(tokenize("is ties"), vec!["tie"]);
        assert_eq!(tokenize(""), Vec::<String>::new());
        assert_eq!(tokenize("..."), Vec::<String>::new());
        // Non-ASCII splits like non-alphanumerics after lowercasing.
        assert_eq!(tokenize("héllo wörld"), vec!["h", "llo", "w", "rld"]);
    }

    #[test]
    fn bm25_scores_match_ts() {
        let docs = sample_docs();
        let matches = Bm25Ranker::new().rank("file read", &docs, 8);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].name, "read_file");
        assert_eq!(matches[0].score, 1.3988109860808995);
        assert_eq!(matches[1].name, "write_file");
        assert_eq!(matches[1].score, 0.45315090947198405);

        let matches = Bm25Ranker::new().rank("file", &docs, 1);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "read_file");
        assert_eq!(matches[0].score, 0.45315090947198405);

        let matches = Bm25Ranker::with_params(2.0, 0.0).rank("file", &docs, 8);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].score, 0.4700036292457356);
        assert_eq!(matches[1].score, 0.4700036292457356);
    }

    #[test]
    fn bm25_edge_cases() {
        let docs = sample_docs();
        // Empty query → no terms → no matches.
        assert!(Bm25Ranker::new().rank("", &docs, 8).is_empty());
        // Stop-word-only query → no terms.
        assert!(Bm25Ranker::new().rank("the", &[doc("a", "the the")], 8).is_empty());
        assert!(Bm25Ranker::new().rank("the", &docs, 8).is_empty());
        // No documents / zero limit.
        assert!(Bm25Ranker::new().rank("file", &[], 8).is_empty());
        assert!(Bm25Ranker::new().rank("file", &docs, 0).is_empty());
        // Query terms that appear nowhere score 0 and are dropped.
        assert!(Bm25Ranker::new().rank("zzz", &docs, 8).is_empty());
        // Deduplicated query terms: "file file" scores like "file".
        assert_eq!(
            Bm25Ranker::new().rank("file file", &docs, 8),
            Bm25Ranker::new().rank("file", &docs, 8)
        );
        // Limit truncates in rank order.
        let matches = Bm25Ranker::new().rank("file", &docs, 2);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].name, "read_file");
    }

    #[test]
    fn bm25_ties_keep_document_order() {
        let docs = vec![doc("a", "alpha"), doc("b", "alpha"), doc("c", "beta")];
        let matches = Bm25Ranker::new().rank("alpha", &docs, 8);
        assert_eq!(matches.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(matches[0].score, matches[1].score);
    }

    #[test]
    fn document_text_includes_schema_and_namespace() {
        let doc = create_tool_search_document(
            &ToolInfo {
                name: "list_issues".to_string(),
                description: "Lists GitHub issues".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "description": "params",
                    "properties": {
                        "state": { "type": "string", "description": "open or closed" },
                        "labels": { "type": "array", "items": { "type": "string", "description": "label names" } }
                    },
                    "anyOf": [{ "type": "string", "description": "alt" }]
                }),
            },
            Some(&ToolNamespace {
                name: "github".to_string(),
                description: Some("GitHub tools".to_string()),
                instructions: Some("Use for repo work".to_string()),
            }),
        );
        assert_eq!(doc.name, "list_issues");
        assert_eq!(
            doc.text,
            "list_issues list issues Lists GitHub issues params state open or closed labels label names alt github GitHub tools Use for repo work"
        );
    }

    #[test]
    fn document_without_namespace_or_schema() {
        let doc = create_tool_search_document(
            &ToolInfo {
                name: "t".to_string(),
                description: String::new(),
                parameters: serde_json::json!("not an object"),
            },
            None,
        );
        // Verified against TS: the name appears twice (raw and with `_` as
        // spaces); only whitespace-only parts are filtered, and the empty
        // description is the part that drops out.
        assert_eq!(doc.text, "t t");
        // Missing / non-object namespace fields contribute nothing.
        let doc = create_tool_search_document(
            &ToolInfo { name: "t".to_string(), description: "d".to_string(), parameters: Value::Null },
            Some(&ToolNamespace { name: "ns".to_string(), description: None, instructions: None }),
        );
        // Verified against TS: the name appears twice here as well.
        assert_eq!(doc.text, "t t d ns");
    }
}
