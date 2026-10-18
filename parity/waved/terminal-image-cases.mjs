// T6 Wave D differential parity cases — terminal-image completion.
//
// Ports of the upstream TS tests in /tmp/pi-repo/packages/tui/test:
//   - terminal-image.test.ts ("Kitty image cursor movement", "image cell
//     sizing", "iTerm2 image encoding", "imageFallback", "hyperlink",
//     "Bug regression tests" for isImageLine)
//
// The harness contract: { tool: "tui-terminal-image", args: { fn, ... } }.
// Stateful fns (renderImage/renderImageAndCrop/imageFallback) pass
// `setup: { capabilities, cellDimensions }`, applied before the call and
// reset afterwards on both sides. Base64 image fixtures are synthesized
// with Buffer (PNG/GIF/WebP headers + dimension fields), mirroring the
// Rust fixtures built with the same byte layouts.

/** `name` shorthand for the tui-terminal-image tool. */
const ti = (name, args) => ({ name: `tui-terminal-image/${name}`, tool: "tui-terminal-image", args });

/** Kitty capabilities + cell dimensions setup. */
const kittySetup = (widthPx, heightPx) => ({
	capabilities: { images: "kitty", trueColor: true, hyperlinks: true },
	cellDimensions: { widthPx, heightPx },
});
const iterm2Setup = (widthPx, heightPx) => ({
	capabilities: { images: "iterm2", trueColor: true, hyperlinks: true },
	cellDimensions: { widthPx, heightPx },
});
const noImageSetup = { capabilities: { images: null, trueColor: false, hyperlinks: false } };
const hyperlinksSetup = { capabilities: { images: null, trueColor: true, hyperlinks: true } };

/** PNG fixture: signature + IHDR width/height (big-endian) at 16/20. */
function pngB64(widthPx, heightPx) {
	const b = Buffer.alloc(24);
	b[0] = 0x89;
	b.write("PNG", 1, "ascii");
	b.writeUInt32BE(widthPx, 16);
	b.writeUInt32BE(heightPx, 20);
	return b.toString("base64");
}

/** GIF fixture: signature + logical screen size (little-endian) at 6/8. */
function gifB64(widthPx, heightPx) {
	const b = Buffer.alloc(10);
	b.write("GIF89a", 0, "ascii");
	b.writeUInt16LE(widthPx, 6);
	b.writeUInt16LE(heightPx, 8);
	return b.toString("base64");
}

/** WebP fixture with the requested chunk ("VP8 " / "VP8L" / "VP8X"). */
function webpB64(chunk, widthPx, heightPx) {
	const b = Buffer.alloc(30);
	b.write("RIFF", 0, "ascii");
	b.write("WEBP", 8, "ascii");
	b.write(chunk, 12, "ascii");
	if (chunk === "VP8 ") {
		b.writeUInt16LE(widthPx & 0x3fff, 26);
		b.writeUInt16LE(heightPx & 0x3fff, 28);
	} else if (chunk === "VP8L") {
		const bits = ((heightPx - 1) << 14) | (widthPx - 1);
		b.writeUInt32LE(bits, 21);
	} else {
		b[24] = (widthPx - 1) & 0xff;
		b[25] = ((widthPx - 1) >> 8) & 0xff;
		b[26] = ((widthPx - 1) >> 16) & 0xff;
		b[27] = (heightPx - 1) & 0xff;
		b[28] = ((heightPx - 1) >> 8) & 0xff;
		b[29] = ((heightPx - 1) >> 16) & 0xff;
	}
	return b.toString("base64");
}

const KITTY_PREFIX = "\x1b_G";
const ITERM2_PREFIX = "\x1b]1337;File=";

export const terminalImageCases = [
	// ------------------------------------------------------------------
	// encodeKitty (terminal-image.test.ts "Kitty image cursor movement")
	// ------------------------------------------------------------------
	ti("encode-kitty-default", { fn: "encodeKitty", base64Data: "AAAA", options: {} }),
	ti("encode-kitty-no-move-cursor", {
		fn: "encodeKitty",
		base64Data: "AAAA",
		options: { columns: 2, rows: 2, moveCursor: false },
	}),
	ti("encode-kitty-image-id", {
		fn: "encodeKitty",
		base64Data: "QUJDRA==",
		options: { columns: 10, rows: 4, imageId: 8938 },
	}),
	ti("encode-kitty-zero-options", {
		// TS truthiness: 0-valued columns/rows/imageId are skipped.
		fn: "encodeKitty",
		base64Data: "AAAA",
		options: { columns: 0, rows: 0, imageId: 0 },
	}),
	ti("encode-kitty-exact-chunk-boundary", {
		fn: "encodeKitty",
		base64Data: "A".repeat(4096),
		options: { columns: 5 },
	}),
	ti("encode-kitty-chunked", {
		fn: "encodeKitty",
		base64Data: "A".repeat(4096 * 2 + 100),
		options: { columns: 5, rows: 3 },
	}),
	ti("encode-kitty-chunked-with-id", {
		fn: "encodeKitty",
		base64Data: Buffer.alloc(10000, 7).toString("base64"),
		options: { imageId: 42, moveCursor: false },
	}),

	// ------------------------------------------------------------------
	// encodeITerm2 (terminal-image.test.ts "iTerm2 image encoding")
	// ------------------------------------------------------------------
	ti("encode-iterm2-size-and-dims", {
		fn: "encodeITerm2",
		base64Data: "AAAA",
		options: { width: 2, height: "auto" },
	}),
	ti("encode-iterm2-empty-options", { fn: "encodeITerm2", base64Data: "AAAA", options: {} }),
	ti("encode-iterm2-name-and-flags", {
		fn: "encodeITerm2",
		base64Data: "AAAA",
		options: { name: "pic.png", preserveAspectRatio: false, inline: false },
	}),
	ti("encode-iterm2-large-size", {
		fn: "encodeITerm2",
		base64Data: Buffer.alloc(1000, 1).toString("base64"),
		options: { width: 60, height: "auto" },
	}),
	ti("encode-iterm2-odd-base64-length", {
		// 2 leftover chars decode to 1 byte: size=1.
		fn: "encodeITerm2",
		base64Data: "AA",
		options: { width: 1 },
	}),

	// ------------------------------------------------------------------
	// calculateImageCellSize / calculateImageRows ("image cell sizing")
	// ------------------------------------------------------------------
	ti("cell-size-width-scale", {
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 90, heightPx: 180 },
		maxWidthCells: 10,
		cellDimensions: { widthPx: 9, heightPx: 18 },
		optimizeAspectRatio: false,
	}),
	ti("cell-size-height-capped", {
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 90, heightPx: 180 },
		maxWidthCells: 10,
		maxHeightCells: 5,
		cellDimensions: { widthPx: 9, heightPx: 18 },
		optimizeAspectRatio: false,
	}),
	ti("cell-size-kitty-optimize-keeps-ceiling", {
		// 615x86 in 15x28 cells, 60 wide: ideal rows 4.49 → keep 5.
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 615, heightPx: 86 },
		maxWidthCells: 60,
		cellDimensions: { widthPx: 15, heightPx: 28 },
		optimizeAspectRatio: true,
	}),
	ti("cell-size-kitty-height-limited-thin", {
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		maxWidthCells: 30,
		maxHeightCells: 15,
		cellDimensions: { widthPx: 14, heightPx: 28 },
		optimizeAspectRatio: true,
	}),
	ti("cell-size-thin-images-square-box", {
		// 1x1 cells: [1, 140, 149] px widths against maxHeight 10.
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 140, heightPx: 1000 },
		maxWidthCells: 30,
		maxHeightCells: 10,
		cellDimensions: { widthPx: 1, heightPx: 1 },
		optimizeAspectRatio: true,
	}),
	ti("cell-size-iterm2-ceiling", {
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		maxWidthCells: 30,
		maxHeightCells: 15,
		cellDimensions: { widthPx: 14, heightPx: 28 },
		optimizeAspectRatio: false,
	}),
	ti("cell-size-zero-pixels", {
		// Zero dimensions clamp to 1px on both sides.
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 0, heightPx: 0 },
		maxWidthCells: 10,
		cellDimensions: { widthPx: 9, heightPx: 18 },
		optimizeAspectRatio: true,
	}),
	ti("cell-size-max-width-floor", {
		fn: "calculateImageCellSize",
		imageDimensions: { widthPx: 100, heightPx: 100 },
		maxWidthCells: 0,
		cellDimensions: { widthPx: 9, heightPx: 18 },
		optimizeAspectRatio: false,
	}),
	ti("calculate-rows-default-cells", {
		fn: "calculateImageRows",
		imageDimensions: { widthPx: 615, heightPx: 86 },
		targetWidthCells: 60,
	}),

	// ------------------------------------------------------------------
	// dimension parsers
	// ------------------------------------------------------------------
	ti("png-dimensions", { fn: "getPngDimensions", base64Data: pngB64(615, 86) }),
	ti("png-dimensions-large", { fn: "getPngDimensions", base64Data: pngB64(4000, 3000) }),
	ti("png-dimensions-too-short", { fn: "getPngDimensions", base64Data: "AAAA" }),
	ti("png-dimensions-bad-signature", {
		fn: "getPngDimensions",
		base64Data: (() => {
			const b = Buffer.alloc(24, 1);
			return b.toString("base64");
		})(),
	}),
	ti("gif-dimensions", { fn: "getGifDimensions", base64Data: gifB64(640, 480) }),
	ti("gif-dimensions-87a", {
		fn: "getGifDimensions",
		base64Data: (() => {
			const raw = Buffer.alloc(10);
			raw.write("GIF87a", 0, "ascii");
			raw.writeUInt16LE(1, 6);
			raw.writeUInt16LE(2, 8);
			return raw.toString("base64");
		})(),
	}),
	ti("gif-dimensions-bad-signature", { fn: "getGifDimensions", base64Data: pngB64(615, 86) }),
	ti("webp-dimensions-vp8", { fn: "getWebpDimensions", base64Data: webpB64("VP8 ", 320, 240) }),
	ti("webp-dimensions-vp8l", { fn: "getWebpDimensions", base64Data: webpB64("VP8L", 640, 480) }),
	ti("webp-dimensions-vp8x", { fn: "getWebpDimensions", base64Data: webpB64("VP8X", 100, 50) }),
	ti("webp-dimensions-not-webp", { fn: "getWebpDimensions", base64Data: pngB64(615, 86) }),
	ti("get-image-dimensions-png", { fn: "getImageDimensions", base64Data: pngB64(615, 86), mimeType: "image/png" }),
	ti("get-image-dimensions-jpeg-stub", { fn: "getImageDimensions", base64Data: "AAAA", mimeType: "image/jpeg" }),
	ti("get-image-dimensions-gif", { fn: "getImageDimensions", base64Data: gifB64(16, 16), mimeType: "image/gif" }),
	ti("get-image-dimensions-webp", { fn: "getImageDimensions", base64Data: webpB64("VP8X", 800, 600), mimeType: "image/webp" }),
	ti("get-image-dimensions-unknown-mime", { fn: "getImageDimensions", base64Data: pngB64(615, 86), mimeType: "image/bmp" }),

	// ------------------------------------------------------------------
	// isImageLine ("Bug regression tests")
	// ------------------------------------------------------------------
	ti("is-image-line-kitty-start", { fn: "isImageLine", line: `${KITTY_PREFIX}a=T,f=100;DATA\x1b\\` }),
	ti("is-image-line-iterm2-start", { fn: "isImageLine", line: `${ITERM2_PREFIX}inline=1:DATA\x07` }),
	ti("is-image-line-mid-line", { fn: "isImageLine", line: `text before ${KITTY_PREFIX}a=T;DATA\x1b\\ after` }),
	ti("is-image-line-plain-text", { fn: "isImageLine", line: "no images here" }),
	ti("is-image-line-empty", { fn: "isImageLine", line: "" }),
	ti("is-image-line-partial-kitty", { fn: "isImageLine", line: "some \x1b_G without payload" }),

	// ------------------------------------------------------------------
	// renderImage ("Kitty image cursor movement" / "image cell sizing")
	// ------------------------------------------------------------------
	ti("render-kitty-default-cursor", {
		fn: "renderImage",
		setup: kittySetup(10, 10),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 20, heightPx: 20 },
		options: { maxWidthCells: 2 },
	}),
	ti("render-kitty-no-move-cursor", {
		fn: "renderImage",
		setup: kittySetup(10, 10),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 20, heightPx: 20 },
		options: { maxWidthCells: 2, moveCursor: false },
	}),
	ti("render-kitty-thin-image", {
		fn: "renderImage",
		setup: kittySetup(9, 18),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 1200, heightPx: 12 },
		options: { maxWidthCells: 60 },
	}),
	ti("render-kitty-ceiling-placement", {
		fn: "renderImage",
		setup: kittySetup(15, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 615, heightPx: 86 },
		options: { maxWidthCells: 60 },
	}),
	ti("render-kitty-height-limited-with-id", {
		fn: "renderImage",
		setup: kittySetup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15, imageId: 8938, moveCursor: false },
	}),
	ti("render-kitty-image-id-zero", {
		// imageId 0: `!== undefined` still registers metadata, but encodeKitty
		// truthiness drops `i=0` from the sequence.
		fn: "renderImage",
		setup: kittySetup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15, imageId: 0 },
	}),
	ti("render-iterm2-ceiling", {
		fn: "renderImage",
		setup: iterm2Setup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15 },
	}),
	ti("render-iterm2-default-width", {
		fn: "renderImage",
		setup: iterm2Setup(9, 18),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 615, heightPx: 86 },
		options: { maxWidthCells: 60 },
	}),
	ti("render-no-protocol", {
		fn: "renderImage",
		setup: noImageSetup,
		base64Data: "AAAA",
		imageDimensions: { widthPx: 20, heightPx: 20 },
		options: {},
	}),

	// ------------------------------------------------------------------
	// renderImageAndCrop (metadata registry end-to-end)
	// ------------------------------------------------------------------
	ti("render-crop-kitty-partial", {
		fn: "renderImageAndCrop",
		setup: kittySetup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15, imageId: 8938, moveCursor: false },
		hiddenRows: 1,
		visibleRows: 2,
	}),
	ti("render-crop-kitty-full-visibility", {
		// hiddenRows 0 + full rows: cropKittyImageLine returns the line
		// unchanged; placement still resolves.
		fn: "renderImageAndCrop",
		setup: kittySetup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15, imageId: 7, moveCursor: false },
		hiddenRows: 0,
		visibleRows: 15,
	}),
	ti("render-crop-kitty-no-image-id", {
		// Without imageId there is no registered metadata: crop is a no-op
		// and the placement is null.
		fn: "renderImageAndCrop",
		setup: kittySetup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15 },
		hiddenRows: 2,
		visibleRows: 3,
	}),
	ti("render-crop-kitty-overlong-visibility", {
		// visibleRows beyond the placement clamps to the remaining rows.
		fn: "renderImageAndCrop",
		setup: kittySetup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15, imageId: 9, moveCursor: false },
		hiddenRows: 13,
		visibleRows: 99,
	}),
	ti("render-crop-iterm2", {
		// iTerm2: cropKittyImageLine no-ops (no kitty metadata), placement
		// is null.
		fn: "renderImageAndCrop",
		setup: iterm2Setup(14, 28),
		base64Data: "AAAA",
		imageDimensions: { widthPx: 400, heightPx: 900 },
		options: { maxWidthCells: 30, maxHeightCells: 15 },
		hiddenRows: 1,
		visibleRows: 2,
	}),

	// ------------------------------------------------------------------
	// hyperlink / imageFallback
	// ------------------------------------------------------------------
	ti("hyperlink-basic", { fn: "hyperlink", text: "text", url: "https://example.com" }),
	ti("hyperlink-empty-text", { fn: "hyperlink", text: "", url: "file:///x" }),
	ti("hyperlink-with-ansi", {
		fn: "hyperlink",
		text: "\x1b[34mstyled\x1b[0m",
		url: "https://example.com",
	}),
	ti("image-fallback-no-filename", { fn: "imageFallback", mimeType: "image/png" }),
	ti("image-fallback-dimensions-only", {
		fn: "imageFallback",
		mimeType: "image/png",
		dimensions: { widthPx: 100, heightPx: 50 },
	}),
	ti("image-fallback-basename", {
		fn: "imageFallback",
		mimeType: "image/png",
		dimensions: { widthPx: 100, heightPx: 50 },
		filename: "pic.png",
	}),
	ti("image-fallback-absolute-no-hyperlinks", {
		fn: "imageFallback",
		mimeType: "image/png",
		filename: "/tmp/abs/pic.png",
	}),
	ti("image-fallback-home-shortened", {
		fn: "imageFallback",
		mimeType: "image/png",
		filename: `${process.env.HOME}/docs/pic.png`,
	}),
	ti("image-fallback-home-hyperlinked", {
		fn: "imageFallback",
		setup: hyperlinksSetup,
		mimeType: "image/png",
		filename: `${process.env.HOME}/docs/pic.png`,
	}),
	ti("image-fallback-absolute-hyperlinked", {
		fn: "imageFallback",
		setup: hyperlinksSetup,
		mimeType: "image/jpeg",
		dimensions: { widthPx: 640, heightPx: 480 },
		filename: "/var/tmp/a b.png",
	}),
];
