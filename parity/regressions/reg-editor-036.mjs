// AUTO-GENERATED fuzz regression (parity/fuzz target: editor).
// JS `String.prototype.trim` treats U+FEFF (ZWNBSP) as whitespace; Rust
// `str::trim` does not. A BOM-only history entry must be REJECTED by
// add_to_history on both sides (fixed via crate::utils::js_trim).
export const cases = [
	{
		name: "fuzz/reg-editor-036",
		tool: "tui-editor",
		args: { name: "f", group: "tui-editor", theme: "default", script: [{ input: "\u001b[A" }], history: ["\uFEFF"] },
	},
];
