// AUTO-GENERATED fuzz regression (parity/fuzz target: main).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-main-038",
		tool: "tui-main",
		args: {"name":"f","script":[{"type":"addChild","component":{"type":"mouse-region","child":{"type":"box","children":[{"type":"text","text":"o"}],"paddingX":0}}},{"type":"renderNow"},{"type":"addChild","component":{"type":"mouse-region","child":{"type":"scroll-view","child":{"type":"truncated-text","paddingX":1}}}},{"type":"renderNow"}],"cols":0,"rows":1},
	},
];
