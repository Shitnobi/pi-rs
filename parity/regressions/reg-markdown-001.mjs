// AUTO-GENERATED fuzz regression (parity/fuzz target: markdown).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-markdown-001",
		tool: "tui-markdown-lex",
		args: {"fn":"lex","source":"🇺_+_"},
	},
];
