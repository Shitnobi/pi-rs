// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-010",
		tool: "tui-utils",
		args: {"fn":"wrapTextWithAnsi","text":"text\u001b[\u001b[1m🇺中","width":7},
	},
];
