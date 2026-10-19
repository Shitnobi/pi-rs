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

	// ------------------------------------------------------------------
	// JS \s / JS dot semantics (U+2028/U+2029/U+00A0/U+FEFF/U+0085)
	//
	// JS `.` excludes all four line terminators and JS `\s` is the
	// WhiteSpace+LineTerminator set (incl. U+FEFF, excl. U+0085) — both
	// differ from Rust regex/trim defaults and are regression-guarded here.
	// ------------------------------------------------------------------
	ml("ls-heading-paragraph", "# aa\u2028bb"),
	ml("ps-heading-paragraph", "# aa\u2029bb"),
	ml("ls-in-heading-text", "# foo\u2028bar"),
	ml("ls-paragraph-join", "foo\u2028bar"),
	ml("ls-setext", "foo\u2028===\n"),
	ml("ls-in-table", "| a\u2028b |\n| --- |\n| c |"),
	ml("ls-hr-shape", "*\u2028*\u2028*\n"),
	ml("ps-paragraph-join", "foo\u2029bar"),
	ml("nbsp-heading", "#\u00a0foo"),
	ml("nbsp-heading-after", "# foo\u00a0bar"),
	ml("u1680-heading", "#\u1680foo"),
	ml("u2000-heading", "#\u2000foo"),
	ml("u200a-heading", "#\u200afoo"),
	ml("nnbsp-heading", "#\u202ffoo"),
	ml("mmsp-heading", "#\u205ffoo"),
	ml("ideo-heading", "#\u3000foo"),
	ml("bom-heading", "#\ufefffoo"),
	ml("bom-heading-after", "# foo\ufeffbar"),
	ml("nel-heading", "#\u0085foo"),
	ml("bom-fence-lang", "```\ufeffjs\nx\n```"),
	ml("nel-fence-lang", "```\u0085js\nx\n```"),
	ml("bom-list-item", "-\ufefffoo"),
	ml("bom-blockquote", ">\ufefffoo"),
	ml("bom-table-cell", "| a |\n| --- |\n| \ufeffb |"),
	ml("bom-lheading-trim", "a\ufeffb\n==="),
	ml("nel-lheading-trim", "a\u0085b\n==="),
	ml("nbsp-list", "-\u00a0foo"),
	ml("ls-code-indent", "   \u2028x"),
	ml("bom-inline-text", "a\ufeff*b*c"),

	// -------------------------------------------- astral / surrogate coverage
	ml("astral-para", "a\u{1F600}b"),
	ml("astral-math", "x\u{1D400}y"),
	ml("astral-cjk-ext", "x\u{20000}y"),
	ml("astral-heading", "# \u{1F600} head"),
	ml("astral-heading-only", "#\u{1F600}"),
	ml("astral-setext", "T\u{1F600}\n==="),
	ml("astral-lheading", "\u{1F600}\n==="),
	ml("astral-em", "**\u{1F600}**"),
	ml("astral-em-mixed", "a*\u{1F600}*b"),
	ml("astral-codespan", "`\u{1F600}`"),
	ml("astral-codespan-dbl", "``\u{1F600}``"),
	ml("astral-link-text", "[\u{1F600}](url)"),
	ml("astral-link-url", "[t](u\u{1F600}rl)"),
	ml("astral-autolink", "<http://\u{1F600}.com>"),
	ml("astral-autolink-email", "<a\u{1F600}@b.com>"),
	ml("astral-html-tag", "<a\u{1F600}>text</a\u{1F600}>"),
	ml("astral-html-attr", '<div title="\u{1F600}">x</div>'),
	ml("astral-html-block", "<pre>\n\u{1F600}\n</pre>"),
	ml("astral-table-cell", "| a | b |\n|---|---|\n| \u{1F600} | 2 |"),
	ml("astral-table-head", "| \u{1F600} | b |\n|---|---|\n| 1 | 2 |"),
	ml("astral-list-item", "- \u{1F600}"),
	ml("astral-task", "- [\u{1F600}] task"),
	ml("astral-blockquote", "> \u{1F600}"),
	ml("astral-fence-lang", "```\u{1F600}\ncode\n```"),
	ml("astral-fence-body", "```\n\u{1F600}\n```"),
	ml("astral-escape", "\\\u{1F600}"),
	ml("astral-reflink", "[\u{1F600}][r]\n\n[r]: url"),
	ml("astral-strike", "~~\u{1F600}~~"),
	ml("astral-hardbreak", "a\u{1F600}  \nb"),
	ml("astral-zwj", "\u200d\u{1F600}\u200d"),
	ml("astral-flag", "\u{1F1FA}\u{1F1F8} flag"),
	ml("astral-tag-seq", "\u{1F3F4}\u{E0067}\u{E0062}\u{E0053}\u{E0063}\u{E0074}"),
	ml("astral-skin", "\u{1F44D}\u{1F3FB}"),
	ml("astral-keycap", "1\uFE0F\u20E3"),
];

