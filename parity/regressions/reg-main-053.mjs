// AUTO-GENERATED fuzz regression (parity/fuzz target: main).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-main-053",
		tool: "tui-main",
		args: {"name":"f","script":[{"type":"showOverlay","component":{"type":"scroll-view","child":{"type":"input"}}},{"type":"start"},{"type":"input","data":"é"}],"cols":0,"rows":0},
	},
];
