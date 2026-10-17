import { cases } from "/home/shinobi/AI-Trident/pi-rs/parity/cases.mjs";
import { spawnSync } from "node:child_process";
const run = (c) => {
	const r = spawnSync("node", ["/home/shinobi/AI-Trident/pi-rs/parity/ts-harness.js", "/tmp"], {
		input: JSON.stringify({ tool: c.tool, args: c.args }), encoding: "utf8" });
	if (r.status !== 0) return { __err: (r.stderr || "").split("\n").slice(-3).join(" ") };
	return JSON.parse(r.stdout);
};
const get = (n) => cases.find((c) => c.name === "tui-editor/" + n);
const show = (n, f) => { const o = run(get(n)); console.log("=== " + n); f(o); };
show("history-submit-reads-marker", (o) => console.log("onSubmit:", JSON.stringify(o.events.onSubmit[0]?.slice(0, 60)), "| text:", JSON.stringify(o.text)));
show("scroll-down-border", (o) => console.log("lines[0]:", JSON.stringify(o.frames[0].lines[0]), "| lines[1]:", JSON.stringify(o.frames[0].lines[1]), "| last:", JSON.stringify(o.frames[0].lines.at(-1))));
show("paste-marker-undo", (o) => console.log("text:", JSON.stringify(o.text), "| exp:", JSON.stringify(o.expandedText)));
show("backslash-enter-newline", (o) => console.log("lines:", JSON.stringify(o.lines), "| onSubmit:", JSON.stringify(o.events.onSubmit)));
show("sticky-column-reset-typing", (o) => console.log("cursor:", JSON.stringify(o.cursor)));
show("csi-u-ctrl-rejected", (o) => console.log("text:", JSON.stringify(o.text), "| cursor:", JSON.stringify(o.cursor)));
show("enter-submit-empty", (o) => console.log("onSubmit:", JSON.stringify(o.events.onSubmit), "| onChange tail:", JSON.stringify(o.events.onChange.slice(-2))));
show("yank-pop-without-yank", (o) => console.log("text:", JSON.stringify(o.text)));
show("render-focused-cursor-marker", (o) => console.log("line[1]:", JSON.stringify(o.frames[0].lines[1])));
show("typing-into-settext", (o) => console.log("text:", JSON.stringify(o.text)));
show("paste-control-filtered", (o) => console.log("text:", JSON.stringify(o.text)));
show("wrap-padding-x", (o) => console.log("lines[0..2]:", JSON.stringify(o.frames[0].lines.slice(0, 3))));
show("paste-unterminated", (o) => console.log("text:", JSON.stringify(o.text)));
show("history-empty-up-down", (o) => console.log("text:", JSON.stringify(o.text), "| cursor:", JSON.stringify(o.cursor)));
show("scroll-top-then-up", (o) => console.log("cursor:", JSON.stringify(o.cursor), "| text:", JSON.stringify(o.text)));
