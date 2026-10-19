// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-009",
		tool: "tui-utils",
		args: {"fn":"extractSegments","line":"𠀁❤ñ[H️🏿\u001b[\u001b[3m 🇸Ç🏻خ","beforeEnd":9,"afterStart":14,"afterLen":1},
	},
];
