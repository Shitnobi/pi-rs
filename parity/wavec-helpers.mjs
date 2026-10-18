// Shared builders for T5 Wave C tui-main/tui-alt parity cases.
//
// Case shape (consumed by run-parity.mjs):
//   { name: "tui-main/<short>", tool: "tui-main", args: { name: <short>, ... } }
// The `args` object is passed to the harness as-is:
//   { name, cols, rows, screenLines?, capabilities?, showHardwareCursor?,
//     altOptions?, clearOnShrink?, script: [op, ...] }
//
// Script ops (from docs/T5_PLAN.md Wave C contract):
//   { type: "addChild", component }            add a child to the screen root
//   { type: "input", data }                    feed a string to the input handler
//   { type: "terminalReply", data }            same pipe (DA1/OSC replies)
//   { type: "setFocus", ref: "cN" | null }     focus a registered component
//   { type: "showOverlay", component, options? }
//   { type: "hideOverlay" }
//   { type: "requestRender", force? }
//   { type: "renderNow", force? }
//   { type: "resize", cols, rows }
//   { type: "tickMs", n }                      advance the virtual clock
//   { type: "start" }
//   { type: "stop", preserveScreen? }
// alt-only:
//   { type: "setLayoutRoot", component | null }
//   { type: "scrollBy", lines }
//   { type: "flash", message, durationMs? }
//
// Component specs use the tui-components vocabulary: text, truncated-text,
// spacer, box, h-stack, v-stack, loader, cancellable-loader, mouse-region,
// scroll-view, input, select-list, settings-list. Every built component is
// auto-registered under "c0", "c1", ... in build order (children before the
// parent that contains them; refs accumulate across all ops in the script).
// `setFocus` and the `focused` output field use those refs.

export function tm(name, script, o = {}) {
	return {
		name: "tui-main/" + name,
		tool: "tui-main",
		args: {
			name,
			cols: o.cols ?? 40,
			rows: o.rows ?? 10,
			...(o.screenLines ? { screenLines: true } : {}),
			...(o.capabilities ? { capabilities: o.capabilities } : {}),
			...(o.showHardwareCursor !== undefined ? { showHardwareCursor: o.showHardwareCursor } : {}),
			...(o.clearOnShrink !== undefined ? { clearOnShrink: o.clearOnShrink } : {}),
			script,
		},
	};
}

export function ta(name, script, o = {}) {
	return {
		name: "tui-alt/" + name,
		tool: "tui-alt",
		args: {
			name,
			cols: o.cols ?? 40,
			rows: o.rows ?? 10,
			...(o.screenLines ? { screenLines: true } : {}),
			...(o.capabilities ? { capabilities: o.capabilities } : {}),
			...(o.showHardwareCursor !== undefined ? { showHardwareCursor: o.showHardwareCursor } : {}),
			...(o.altOptions ? { altOptions: o.altOptions } : {}),
			...(o.clearOnShrink !== undefined ? { clearOnShrink: o.clearOnShrink } : {}),
			script,
		},
	};
}

// --- component spec builders (mirror cases.mjs tui-components vocabulary) ---
export function txt(text, px, py, bg) {
	return {
		type: "text",
		text,
		...(px !== undefined ? { paddingX: px } : {}),
		...(py !== undefined ? { paddingY: py } : {}),
		...(bg ? { bg } : {}),
	};
}
export function ttxt(text, px, py) {
	return {
		type: "truncated-text",
		text,
		...(px !== undefined ? { paddingX: px } : {}),
		...(py !== undefined ? { paddingY: py } : {}),
	};
}
export function spc(lines) {
	return { type: "spacer", ...(lines !== undefined ? { lines } : {}) };
}
export function boxc(children, px, py, bg) {
	return {
		type: "box",
		children,
		...(px !== undefined ? { paddingX: px } : {}),
		...(py !== undefined ? { paddingY: py } : {}),
		...(bg ? { bg } : {}),
	};
}
export function ent(component, o = {}) {
	return {
		component,
		...(o.basis !== undefined ? { basis: o.basis } : {}),
		...(o.grow !== undefined ? { grow: o.grow } : {}),
		...(o.shrink !== undefined ? { shrink: o.shrink } : {}),
		...(o.min !== undefined ? { minSize: o.min } : {}),
		...(o.max !== undefined ? { maxSize: o.max } : {}),
		...(o.visible !== undefined ? { visible: o.visible } : {}),
	};
}
export function hstk(entries, o = {}) {
	return { type: "h-stack", entries, ...(o.gap !== undefined ? { gap: o.gap } : {}), ...(o.align ? { align: o.align } : {}) };
}
export function vstk(entries, o = {}) {
	return { type: "v-stack", entries, ...(o.gap !== undefined ? { gap: o.gap } : {}), ...(o.align ? { align: o.align } : {}) };
}
export function inp(o = {}) {
	return { type: "input", ...o };
}
export function slist(items, o = {}) {
	return { type: "select-list", items, ...o };
}
export function stlist(items, o = {}) {
	return { type: "settings-list", items, ...o };
}
export function ld(o = {}) {
	return { type: "loader", ...o };
}
export function cld(o = {}) {
	return { type: "cancellable-loader", ...o };
}
export function svw(child, o = {}) {
	return { type: "scroll-view", child, ...o };
}
export function mr(child) {
	return { type: "mouse-region", child };
}
export function sitem(value, label, description) {
	return { value, label, ...(description !== undefined ? { description } : {}) };
}
export function sit(id, label, currentValue, o = {}) {
	return { id, label, currentValue, ...o };
}

// --- script op shorthands ---
export const start = () => ({ type: "start" });
export const stop = (o = {}) => ({ type: "stop", ...(o.preserveScreen !== undefined ? { preserveScreen: o.preserveScreen } : {}) });
export const key = (data) => ({ type: "input", data });
export const reply = (data) => ({ type: "terminalReply", data });
export const tick = (n) => ({ type: "tickMs", n });
export const resize = (cols, rows) => ({ type: "resize", cols, rows });
export const focus = (ref) => ({ type: "setFocus", ref: ref ?? null });
export const addChild = (component) => ({ type: "addChild", component });
export const showOverlay = (component, options) => ({
	type: "showOverlay",
	component,
	...(options ? { options } : {}),
});
export const hideOverlay = () => ({ type: "hideOverlay" });
export const requestRender = (force) => ({ type: "requestRender", ...(force !== undefined ? { force } : {}) });
export const renderNow = (force) => ({ type: "renderNow", ...(force !== undefined ? { force } : {}) });
export const setLayoutRoot = (component) => ({ type: "setLayoutRoot", component });
export const scrollBy = (lines) => ({ type: "scrollBy", lines });
export const flash = (message, durationMs) => ({
	type: "flash",
	message,
	...(durationMs !== undefined ? { durationMs } : {}),
});
