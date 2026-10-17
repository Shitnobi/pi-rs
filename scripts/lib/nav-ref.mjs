// TS word-navigation reference logic (word-navigation.ts) operating on an
// arbitrary Intl.Segmenter instance — used by the differential batter.
// Kept 1:1 with /tmp/pi-repo/packages/tui/src/word-navigation.ts.

// Non-global, like the TS original (utils.ts) — .exec() is stateless.
const PUNCTUATION_REGEX = /[(){}[\]<>.,;:'"!?+\-=*/\\|&%^$#@~`]/;

export function isWs(s) {
	return /\s/.test(s);
}

export function findWordBackwardRef(refSeg, text, cursor) {
	if (cursor <= 0) return 0;
	const textBeforeCursor = text.slice(0, cursor);
	const segments = [...refSeg.segment(textBeforeCursor)];
	let newCursor = cursor;
	while (segments.length > 0 && isWs(segments[segments.length - 1].segment)) {
		newCursor -= segments.pop().segment.length || 0;
	}
	if (segments.length === 0) return newCursor;
	const last = segments[segments.length - 1];
	if (last.isWordLike) {
		const segment = last.segment;
		const matches = [...segment.matchAll(new RegExp(PUNCTUATION_REGEX, "g"))];
		if (matches.length <= 0) newCursor -= segment.length;
		else {
			const lastMatch = matches[matches.length - 1];
			newCursor -= segment.length - (lastMatch.index + lastMatch[0].length);
		}
	} else {
		while (
			segments.length > 0 &&
			!segments[segments.length - 1].isWordLike &&
			!isWs(segments[segments.length - 1].segment)
		) {
			newCursor -= segments.pop().segment.length || 0;
		}
	}
	return newCursor;
}

export function findWordForwardRef(refSeg, text, cursor) {
	if (cursor >= text.length) return text.length;
	const textAfterCursor = text.slice(cursor);
	const iterator = refSeg.segment(textAfterCursor)[Symbol.iterator]();
	let next = iterator.next();
	let newCursor = cursor;
	while (!next.done && isWs(next.value.segment)) {
		newCursor += next.value.segment.length;
		next = iterator.next();
	}
	if (next.done) return newCursor;
	if (next.value.isWordLike) {
		newCursor += PUNCTUATION_REGEX.exec(next.value.segment)?.index ?? next.value.segment.length;
	} else {
		while (!next.done && !next.value.isWordLike && !isWs(next.value.segment)) {
			newCursor += next.value.segment.length;
			next = iterator.next();
		}
	}
	return newCursor;
}
