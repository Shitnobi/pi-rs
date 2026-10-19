// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-007",
		tool: "tui-utils",
		args: {"fn":"wrapTextWithAnsi","text":"\u001b[\u001b[3m§क","width":1},
	},
];
