// T6 Wave D differential parity cases — marked lexer (token trees).
//
// The Rust `marked` port must reproduce the exact token stream that
// `components/markdown.ts` builds (tab normalization + the marked v18
// Lexer/Tokenizer with the StrictStrikethroughTokenizer override and the
// latex block/inline extensions + trimPartialClosingFences). The TS side
// exposes it through the `lexMarkdown` test seam; tokens serialize as plain
// JSON (undefined fields vanish), and the Rust side must emit structurally
// identical trees.
//
// Case sources are drawn from /tmp/pi-repo/packages/tui/test/markdown.test.ts
// (streaming/partial code fences, latex tokenization, tables, task lists,
// reflinks, escapes, strict strikethrough, html passthrough) plus the
// CommonMark-flavored shapes the renderer relies on.

/** `name` shorthand for the tui-markdown-lex tool. */
const ml = (name, source) => ({
	name: `tui-markdown-lex/${name}`,
	tool: "tui-markdown-lex",
	args: { fn: "lex", source },
});

export const markdownLexCases = [
	// ------------------------------------------------------------------
	// Basics
	// ------------------------------------------------------------------
	ml("empty", ""),
	ml("blank-only", "   \n\n  \n"),
	ml("plain-paragraph", "Hello world"),
	ml("two-paragraphs", "First paragraph.\n\nSecond paragraph."),
	ml("soft-break", "line one\nline two"),
	ml("hard-break", "line one  \nline two"),
	ml("trailing-newline", "text\n"),
	ml("crlf", "para one\r\n\r\npara two\r\n"),

	// ------------------------------------------------------------------
	// Headings
	// ------------------------------------------------------------------
	ml("heading-h1", "# Hello"),
	ml("heading-h6", "###### Deep"),
	ml("heading-with-inline", "# Hello **bold** and `code`"),
	ml("heading-trailing-hash", "## Heading ##"),
	ml("heading-no-space", "#NoSpace"),
	ml("heading-level-7", "####### Seven hashes"),
	ml("setext-h1", "Title\n====="),
	ml("setext-h2", "Subtitle\n-----"),
	ml("setext-vs-hr", "Title\n---"),

	// ------------------------------------------------------------------
	// Emphasis (the tricky em/strong interplay)
	// ------------------------------------------------------------------
	ml("bold", "**bold**"),
	ml("italic", "*italic*"),
	ml("bold-underscore", "__bold__"),
	ml("italic-underscore", "_italic_"),
	ml("bold-italic", "***both***"),
	ml("nested-emphasis", "**bold with *italic* inside**"),
	ml("intraword-underscore", "snake_case_word"),
	ml("unmatched-stars", "*unclosed bold"),
	ml("double-star-alone", "** not bold **"),
	ml("em-across-punctuation", "a*foo*bar"),
	ml("strong-in-words", "foo**bar**baz"),

	// ------------------------------------------------------------------
	// Strikethrough (StrictStrikethroughTokenizer override)
	// ------------------------------------------------------------------
	ml("strikethrough-basic", "~~struck~~"),
	ml("strikethrough-single-tilde", "~single~"),
	ml("strikethrough-with-space", "~~ not struck ~~"),
	ml("strikethrough-nested-em", "~~struck *em* inside~~"),
	ml("strikethrough-three-tildes", "~~~three~~~"),
	ml("strikethrough-unmatched", "~~unclosed"),

	// ------------------------------------------------------------------
	// Code
	// ------------------------------------------------------------------
	ml("codespan", "inline `code` here"),
	ml("codespan-double-backtick", "``code with ` inside``"),
	ml("codespan-leading-space", "` code`"),
	ml("code-fence-basic", "```\ncode line\n```"),
	ml("code-fence-lang", "```rust\nfn main() {}\n```"),
	ml("code-fence-tilde", "~~~\ntilde fence\n~~~"),
	ml("code-fence-info-string", "```js title=\"x\"\nlet a = 1\n```"),
	ml("code-fence-unclosed", "```\nstreaming without close"),
	ml("code-fence-partial-close-1", "```\ncontent\n``"),
	ml("code-fence-partial-close-2", "```\ncontent\n`"),
	ml("code-fence-partial-close-tilde", "~~~\ncontent\n~~"),
	ml("code-fence-full-close-after-partial", "```\ncontent\n``\n```"),
	ml("indented-code", "    indented code block\n    second line"),
	ml("code-fence-inside-list", "- item\n  ```\n  fenced\n  ```"),

	// ------------------------------------------------------------------
	// Lists
	// ------------------------------------------------------------------
	ml("ul-basic", "- one\n- two\n- three"),
	ml("ul-plus", "+ one\n+ two"),
	ml("ul-star", "* one\n* two"),
	ml("ol-basic", "1. one\n2. two"),
	ml("ol-paren", "1) one\n2) two"),
	ml("ol-start-at-5", "5. five\n6. six"),
	ml("ol-big-number", "123456789. big"),
	ml("task-list", "- [ ] unchecked\n- [x] checked"),
	ml("task-list-uppercase", "- [X] upper checked"),
	ml("loose-list", "- one\n\n- two\n\n- three"),
	ml("nested-list", "- outer\n  - inner\n    - deepest"),
	ml("mixed-list", "1. ordered\n   - nested unordered\n2. back to ordered"),
	ml("list-with-paragraphs", "- first\n\n  second paragraph\n- next"),
	ml("list-multiline-items", "- line one\n  line two\n- next item"),

	// ------------------------------------------------------------------
	// Blockquotes
	// ------------------------------------------------------------------
	ml("blockquote-basic", "> quoted text"),
	ml("blockquote-multi", "> first\n> second"),
	ml("blockquote-nested", "> outer\n> > inner"),
	ml("blockquote-lazy", "> first\nlazy continuation"),
	ml("blockquote-with-list", "> - item one\n> - item two"),
	ml("blockquote-with-heading", "> ## Heading inside"),

	// ------------------------------------------------------------------
	// Horizontal rules
	// ------------------------------------------------------------------
	ml("hr-dashes", "---"),
	ml("hr-underscores", "___"),
	ml("hr-stars", "***"),
	ml("hr-spaced", "- - -"),

	// ------------------------------------------------------------------
	// Links / images
	// ------------------------------------------------------------------
	ml("link-basic", "[text](https://example.com)"),
	ml("link-with-title", "[text](https://example.com \"Title\")"),
	ml("link-angle-brackets", "[text](<https://example.com/a b>)"),
	ml("image-basic", "![alt](https://example.com/img.png)"),
	ml("autolink", "<https://example.com>"),
	ml("autolink-email", "<user@example.com>"),
	ml("bare-url", "visit https://example.com now"),
	ml("bare-www", "visit www.example.com now"),
	ml("bare-email", "mail user@example.com please"),
	ml("reflink-defined", "[text][ref]\n\n[ref]: https://example.com"),
	ml("reflink-undefined", "[text][missing]"),
	ml("nolink-shorthand", "[ref]\n\n[ref]: https://example.com"),
	ml("link-in-text-emphasis", "see [the **docs**](https://example.com) here"),
	ml("nested-brackets", "[a [b] c](https://example.com)"),

	// ------------------------------------------------------------------
	// Tables
	// ------------------------------------------------------------------
	ml("table-basic", "| a | b |\n| --- | --- |\n| 1 | 2 |"),
	ml("table-alignment", "| left | center | right |\n| :--- | :---: | ---: |\n| 1 | 2 | 3 |"),
	ml("table-inline-markup", "| **bold** | `code` |\n| --- | --- |\n| *em* | [x](y) |"),
	ml("table-no-leading-pipe", "a | b\n--- | ---\n1 | 2"),
	ml("table-ragged-rows", "| a | b |\n| --- | --- |\n| 1 |\n| 1 | 2 | 3 |"),
	ml("table-with-blank-line", "| a | b |\n| --- | --- |\n| 1 | 2 |\n\nafter"),
	ml("not-a-table", "| a | b |\n1 | 2"),

	// ------------------------------------------------------------------
	// HTML
	// ------------------------------------------------------------------
	ml("html-block", "<div>\nsome block\n</div>"),
	ml("html-comment", "<!-- a comment -->"),
	ml("html-inline", "before <span>after</span> end"),
	ml("html-script-block", "<script>\nvar x = 1;\n</script>"),
	ml("html-self-closing", "<br>\n\ntext after"),

	// ------------------------------------------------------------------
	// Escapes
	// ------------------------------------------------------------------
	ml("escape-asterisk", "\\*not italic\\*"),
	ml("escape-bracket", "\\[not a link\\]"),
	ml("escape-backslash", "a\\\\b"),
	ml("escape-pipe", "a \\| b"),

	// ------------------------------------------------------------------
	// Latex extensions (markdown.ts custom tokenizers)
	// ------------------------------------------------------------------
	ml("latex-inline-dollar", "math $x^2$ end"),
	ml("latex-inline-double-dollar", "math $$x^2$$ end"),
	ml("latex-inline-paren", "math \\(x^2\\) end"),
	ml("latex-inline-bracket", "math \\[x^2\\] end"),
	ml("latex-block-dollar", "$$\nx^2 + y^2\n$$"),
	ml("latex-block-bracket", "\\[\nx^2\n\\]"),
	ml("latex-pending-block", "$$\n\\frac{1}{2}"),
	ml("latex-pending-inline", "value $x^2"),
	ml("latex-currency-not-math", "costs $5 and $10 total"),
	ml("latex-uppercase-words", "the $AMD$ stock"),
	ml("latex-escaped-dollar", "\\$5 not math"),
	ml("latex-with-code-inside", "$`code`$ not math"),
	ml("latex-block-followed-by-text", "$$\nx^2\n$$\n\nparagraph"),

	// ------------------------------------------------------------------
	// Definitions (affect the links table for reflinks)
	// ------------------------------------------------------------------
	ml("def-basic", "[ref]: https://example.com"),
	ml("def-with-title", "[ref]: https://example.com \"The Title\""),
	ml("def-multiple", "[a]: https://a.example\n[b]: https://b.example"),

	// ------------------------------------------------------------------
	// Composite / realistic documents
	// ------------------------------------------------------------------
	ml(
		"composite-doc",
		[
			"# Title",
			"",
			"Intro paragraph with **bold**, *italic*, `code`, and a [link](https://example.com).",
			"",
			"## Section",
			"",
			"- item one",
			"- item two with `code`",
			"  - nested item",
			"",
			"> A quote",
			"> spanning lines",
			"",
			"```python",
			"def f():",
			"    return 42",
			"```",
			"",
			"| col | other |",
			"| --- | --- |",
			"| 1 | 2 |",
			"",
			"$$",
			"E = mc^2",
			"$$",
			"",
			"Final ~~struck~~ line.",
		].join("\n"),
	),
	ml(
		"streaming-transcript",
		[
			"Here is some analysis.",
			"",
			"```rust",
			"fn main() {",
			'    println!("hi");',
		].join("\n"),
	),
	ml(
		"streaming-transcript-close",
		[
			"Here is some analysis.",
			"",
			"```rust",
			"fn main() {",
			'    println!("hi");',
			"```",
		].join("\n"),
	),
	ml(
		"nested-structures",
		[
			"> ## Heading in quote",
			">",
			"> - list in quote",
			">   - nested",
			">",
			"> ```",
			"> code in quote",
			"> ```",
		].join("\n"),
	),
	ml("unicode-text", "café — naïve 中文 🎉 emoji"),
	ml("tabs-normalization", "a\tb\n\n\tindented after tab"),
];
