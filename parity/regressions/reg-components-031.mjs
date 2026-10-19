// AUTO-GENERATED fuzz regression (parity/fuzz target: components).
// Reproduces a confirmed TS-vs-Rust divergence; minimized by ddmin.
export const cases = [
	{
		name: "fuzz/reg-components-031",
		tool: "tui-components",
		args: {"cases":[{"name":"b","component":{"type":"h-stack","entries":[{"component":{"type":"cancellable-loader"},"visible":"a","minSize":0}],"gap":1},"script":[{"render":true}]}]},
	},
];
