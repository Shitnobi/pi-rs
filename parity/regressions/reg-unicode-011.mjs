// AUTO-GENERATED fuzz regression (parity/fuzz target: unicode).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-unicode-011",
		tool: "tui-utils",
		args: {"fn":"wrapTextWithAnsi","text":"3m\u001b[\u001b[3m⌚日×m⌛𠀁","width":11},
	},
];
