// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-014",
		tool: "tui-utils",
		args: {"fn":"getGraphemeCellRange","line":"⌛‍‍⌚ᆨ𠀀𠀀°\t","column":12},
	},
];
