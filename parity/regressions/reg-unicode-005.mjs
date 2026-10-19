// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-005",
		tool: "tui-utils",
		args: {"fn":"getActiveBackgroundAnsi","text":"\u001b[\u001b[43m"},
	},
];
