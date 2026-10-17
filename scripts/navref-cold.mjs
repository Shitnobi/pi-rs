// Cold-process reference runner: computes segmentWords / findWordBackward /
// findWordForward expectations with a FRESH Intl.Segmenter as the process's
// very first segmentation activity (ICU's engine factory starts cold).
// Usage: node scripts/navref-cold.mjs <input.json>
//   input: { fn: "segmentWords", texts: [..] }
//        | { fn: "findWordBackward"|"findWordForward", text, cursors: [..] }
//   output (stdout): JSON array of results
import { readFileSync } from "node:fs";
import { findWordBackwardRef, findWordForwardRef } from "./lib/nav-ref.mjs";

const input = JSON.parse(readFileSync(process.argv[2], "utf8"));
const refSeg = new Intl.Segmenter(undefined, { granularity: "word" });

let result;
if (input.fn === "segmentWords") {
	result = input.texts.map((t) =>
		[...refSeg.segment(t)].map(({ segment, isWordLike }) => [segment, isWordLike]),
	);
} else if (input.fn === "findWordBackward") {
	result = input.cursors.map((c) => findWordBackwardRef(refSeg, input.text, c));
} else if (input.fn === "findWordForward") {
	result = input.cursors.map((c) => findWordForwardRef(refSeg, input.text, c));
} else {
	throw new Error(`unknown fn: ${input.fn}`);
}
console.log(JSON.stringify(result));
