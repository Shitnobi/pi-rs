// T6 differential parity cases — Markdown renderer (styled line output).
//
// The Rust `components/markdown.rs` port must reproduce the TS Markdown
// component's render() exactly under the deterministic marker theme defined
// in ts-harness.js / pi-cli main.rs (heading wraps as `#<t>`, bold as
// `**t**`, quoteBorder is `|`, …). Cases drawn from
// /tmp/pi-repo/packages/tui/test/markdown.test.ts plus streaming/partial
// fence shapes.

/** `name` shorthand for the tui-markdown-render tool. */
const mr = (name, source, extra = {}) => ({
	name: `tui-markdown-render/${name}`,
	tool: "tui-markdown-render",
	args: { fn: "render", source, ...extra },
});

export const markdownRenderCases = [
	// ------------------------------------------------------------- basics
	mr("empty", ""),
	mr("whitespace-only", "   \n  \n"),
	mr("plain-text", "hello world"),
	mr("multiline-paragraph", "line one\nline two"),
	mr("two-paragraphs", "first\n\nsecond"),
	mr("unicode-text", "café — naïve 中文 🎉"),

	// ------------------------------------------------------------ heading
	mr("heading-1", "# Hello"),
	mr("heading-2", "## Sub"),
	mr("heading-3", "### Deep"),
	mr("heading-4", "#### Deeper"),
	mr("heading-5", "##### Five"),
	mr("heading-6", "###### Six"),
	mr("heading-with-inline", "## **bold** and _em_"),
	mr("setext-heading", "Title\n====="),
	mr("setext-heading-2", "Sub\n---"),

	// ---------------------------------------------------------- emphasis
	mr("bold", "**bold**"),
	mr("italic", "_em_"),
	mr("italic-star", "*em*"),
	mr("bold-italic", "***both***"),
	mr("strikethrough", "~~gone~~"),
	mr("strict-strikethrough-spaces", "~~ not ~~"),
	mr("strict-strikethrough-tildes", "~~~x~~~"),
	mr("intraword-emphasis", "snake_case_word a*b*c"),
	mr("nested-emphasis", "_a **b** c_"),
	mr("emphasis-adjacent", "**a**_b_"),

	// ------------------------------------------------------------- links
	mr("link", "[text](https://example.com)"),
	mr("link-with-title", '[text](https://example.com "Title")'),
	mr("autolink", "<https://example.com>"),
	mr("bare-url", "see https://example.com/page"),
	mr("email-autolink", "mail@example.com"),
	mr("reflink", "[foo]\n\n[foo]: /url \"t\""),
	mr("link-in-sentence", "go [here](/x) now"),

	// ------------------------------------------------------------ code
	mr("codespan", "`code`"),
	mr("codespan-double", "``a ` b``"),
	mr("fenced-code", "```\nlet x = 1;\n```"),
	mr("fenced-code-lang", "```rust\nfn main() {}\n```"),
	mr("fenced-code-tilde", "~~~\nplain\n~~~"),
	mr("indented-code", "    indented code\n    second line"),
	mr("partial-fence-open", "```\ncode without close"),
	mr("partial-fence-marker", "```\ncode\n``"),
	mr("code-with-backticks-inside", "```\nuse `x`;\n```"),
	mr("highlight-placeholder", "```js\nconst a = 1;\n```"),

	// --------------------------------------------------------- blockquote
	mr("blockquote", "> quoted"),
	mr("blockquote-multiline", "> one\n> two"),
	mr("blockquote-nested", "> outer\n> > inner"),
	mr("blockquote-with-markdown", "> # heading\n> **bold**"),
	mr("blockquote-lazy", "> one\nlazy"),

	// ------------------------------------------------------------- lists
	mr("bullet-list", "- one\n- two\n- three"),
	mr("bullet-list-star", "* one\n* two"),
	mr("bullet-list-plus", "+ one\n+ two"),
	mr("ordered-list", "1. first\n2. second"),
	mr("ordered-list-paren", "1) first\n2) second"),
	mr("ordered-list-start", "3. third\n4. fourth"),
	mr("nested-list", "- a\n  - b\n    - c"),
	mr("mixed-nested-list", "1. one\n   - bullet\n2. two"),
	mr("loose-list", "- a\n\n- b"),
	mr("tight-list", "- a\n- b"),
	mr("task-list", "- [ ] todo\n- [x] done"),
	mr("list-with-paragraphs", "- item with\n  continuation\n\n- next"),
	mr("list-inline-markup", "- **bold** item\n- `code` item"),
	mr("deep-nesting", "- a\n  - b\n    - c\n      - d"),
	mr("ordered-preserve-markers", "1. one\n2. two", {
		options: { preserveOrderedListMarkers: true },
	}),

	// ------------------------------------------------------------- table
	mr("table-basic", "| a | b |\n| --- | --- |\n| 1 | 2 |"),
	mr("table-alignment", "| l | c | r |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |"),
	mr("table-inline-markup", "| **a** | `b` |\n| --- | --- |\n| [l](/x) | _i_ |"),
	mr("table-no-leading-pipe", "a | b\n--- | ---\n1 | 2"),
	mr("table-ragged-rows", "| a | b |\n| --- | --- |\n| 1 |"),
	mr("table-with-blank-line", "| a |\n| --- |\n| 1 |\n\n| b |\n| --- |\n| 2 |"),

	// ----------------------------------------------------------- misc block
	mr("hr", "---"),
	mr("hr-asterisk", "***"),
	mr("hr-underscore", "___"),
	mr("html-inline", "before <b>bold</b> after"),
	mr("html-block", "<div>\nblock\n</div>"),
	mr("escape", "\\*not emphasis\\*"),
	mr("escape-preserved", "\\*kept\\*", {
		options: { preserveBackslashEscapes: true },
	}),
	mr("hard-break", "line one  \nline two"),
	mr("tabs", "a\tb\n\n\tindented after tab"),

	// --------------------------------------------------------------- latex
	mr("latex-block", "$$\nE = mc^2\n$$"),
	mr("latex-block-bracket", "\\[ x^2 \\]"),
	mr("latex-inline", "$x^2$ and $y_1$"),
	mr("latex-inline-paren", "\\(a+b\\)"),
	mr("latex-display-fraction", "$$\\frac{a}{b}$$"),
	mr("latex-pending-block", "$$\nunclosed"),
	mr("latex-disabled", "$$\nE = mc^2\n$$", {
		options: { renderLatex: false },
	}),
	mr("latex-currency-not-math", "costs $5 and $10"),
	mr("latex-mixed-markdown", "formula $x^2$ in **text**"),

	// ----------------------------------------------------------- padding
	mr("padding-x", "hello", { paddingX: 2 }),
	mr("padding-y", "hello", { paddingY: 1 }),
	mr("padding-both", "# hi", { paddingX: 1, paddingY: 1 }),

	// ------------------------------------------------------------- widths
	mr("narrow-wrap", "this is a long paragraph that must wrap", { width: 20 }),
	mr("narrow-heading", "## a fairly long heading line", { width: 24 }),
	mr("narrow-list", "- item one\n- item two is quite long", { width: 16 }),
	mr("narrow-table", "| aaa | bbb |\n| --- | --- |\n| 1 | 2 |", { width: 12 }),
	mr("very-narrow", "wrapping", { width: 4 }),

	// --------------------------------------------------------- composite
	mr(
		"composite-doc",
		[
			"# Title",
			"",
			"Intro with **bold**, _em_, `code`, and a [link](https://example.com).",
			"",
			"## Section",
			"",
			"- item one",
			"- item two",
			"  - nested",
			"",
			"1. first",
			"2. second",
			"",
			"> quote line",
			"> second quote",
			"",
			"```rust",
			"fn main() {",
			"    println!(\"hi\");",
			"}",
			"```",
			"",
			"| a | b |",
			"| --- | :-: |",
			"| 1 | 2 |",
			"",
			"---",
			"",
			"$$",
			"\\sum_{i=1}^n i",
			"$$",
			"",
			"Final $x^2$ formula.",
		].join("\n"),
	),
	mr(
		"streaming-fence-progression",
		["```\nline one\nline two\n``"].join("\n"),
	),

	// ------------------------------------------------- JS \s / dot semantics
	// Regression guards for the JS-regex/trim whitespace semantics
	// (U+2028/U+2029 line terminators, Unicode \s in headings/fences,
	// U+FEFF vs U+0085 trim behavior).
	mr("ls-heading-paragraph", "# aa\u2028bb"),
	mr("ps-heading-paragraph", "# aa\u2029bb"),
	mr("ls-paragraph-join", "foo\u2028bar"),
	mr("ls-setext", "foo\u2028===\n"),
	mr("ls-in-table", "| a\u2028b |\n| --- |\n| c |"),
	mr("nbsp-heading", "#\u00a0foo"),
	mr("u2000-heading", "#\u2000foo"),
	mr("ideo-heading", "#\u3000foo"),
	mr("bom-heading", "#\ufefffoo"),
	mr("bom-heading-after", "# foo\ufeffbar"),
	mr("nel-heading", "#\u0085foo"),
	mr("bom-fence-lang", "```\ufeffjs\nx\n```"),
	mr("nel-fence-lang", "```\u0085js\nx\n```"),
	mr("bom-list-item", "-\ufefffoo"),
	mr("bom-blockquote", ">\ufefffoo"),
	mr("bom-lheading-trim", "a\ufeffb\n==="),
	mr("nel-lheading-trim", "a\u0085b\n==="),
	mr("bom-inline-text", "a\ufeff*b*c"),
	mr("ls-hr-shape", "*\u2028*\u2028*\n"),
];
