// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-012",
		tool: "tui-utils",
		args: {"fn":"getOsc8LinkAtColumn","line":"\u001b]8;;a\u001b\\link\u001b]8;;\u001b\\ü⌚","column":6},
	},
];
