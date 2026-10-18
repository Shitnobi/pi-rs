//! Port of `packages/tui/src/components/image.ts` — the `Image` component
//! with the Kitty PNG transcoding cache. Ground truth:
//! /tmp/pi-repo/packages/tui/src/components/image.ts (167 lines).
//!
//! ## Divergences from TS
//! - The module-level `imageTranscoder` / `pngCache` singletons become
//!   `thread_local!` `RefCell` state, matching the crate-wide convention
//!   for module state (see `terminal_image`); cargo-test tests each run on
//!   their own thread, so the singletons are per-thread.
//! - `pngCache: Map<string, string | null>` becomes a
//!   `Vec<(String, Option<String>)>` used as an insertion-ordered map
//!   (back = most recently used, front = least recently used). A cached
//!   `null` is `Some(None)` (present-but-null); a cache miss is absence
//!   (the TS `cached === undefined` check). Delete+re-insert on access and
//!   first-key eviction past 32 entries mirror `Map` ordering exactly.
//! - `ImageTranscoder` is an owned
//!   `Box<dyn Fn(&str, &str) -> Option<String> + Send + Sync>`.
//! - `ImageTheme.fallbackColor` is an owned `Box<dyn Fn(&str) -> String>`;
//!   `Image` owns its theme (TS stores the shared theme object).
//! - `ImageOptions` numerics are `usize` (cast to `i64` for the
//!   JS-number math); `imageId` is `i64` per the terminal-image port.
//! - `render` returns a clone of the cached lines (TS returns the same
//!   array reference — value-identical, not reference-identical).
//! - JS truthiness of `data` (an empty string is falsy and routes to the
//!   text fallback) is mirrored with explicit emptiness checks.

use std::cell::RefCell;

use crate::component::Component;
use crate::terminal_image::{
    allocate_image_id, get_capabilities, get_cell_dimensions, get_image_dimensions,
    get_png_dimensions, image_fallback, render_image, ImageDimensions, ImageProtocol,
    ImageRenderOptions,
};
use crate::utils::truncate_to_width_default;

/// TS `ImageTranscoder`: converts base64 image data to base64 PNG data, or
/// returns null if it cannot. Called synchronously during rendering.
pub type ImageTranscoder = Box<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;

thread_local! {
    /// `let imageTranscoder: ImageTranscoder | undefined` (image.ts:21).
    static IMAGE_TRANSCODER: RefCell<Option<ImageTranscoder>> = const { RefCell::new(None) };
    /// `const pngCache = new Map<string, string | null>()` (image.ts:23).
    /// Insertion-ordered: back = MRU, front = LRU.
    static PNG_CACHE: RefCell<Vec<(String, Option<String>)>> = const { RefCell::new(Vec::new()) };
}

/// Register the converter used for non-PNG images on Kitty-protocol
/// terminals, which only accept PNG. Without one, such images render as
/// text fallbacks. `None` unregisters (TS passes `undefined`). Like the TS,
/// this also clears the png cache (image.ts:27-31).
pub fn set_image_transcoder(transcoder: Option<ImageTranscoder>) {
    IMAGE_TRANSCODER.with(|t| *t.borrow_mut() = transcoder);
    PNG_CACHE.with(|cache| cache.borrow_mut().clear());
}

/// `toPng` (image.ts:33-41) — transcode through the registered transcoder
/// with the module-level LRU cache. Cache hits (including cached-null
/// hits) do not invoke the transcoder; every access re-inserts the key as
/// MRU; the cache holds at most 32 entries.
fn to_png(base64_data: &str, mime_type: &str) -> Option<String> {
    // `if (!imageTranscoder) return null;`
    if !IMAGE_TRANSCODER.with(|t| t.borrow().is_some()) {
        return None;
    }

    // `const cached = pngCache.get(base64Data);`
    // `const png = cached === undefined ? imageTranscoder(...) : cached;`
    // `pngCache.delete(base64Data); pngCache.set(base64Data, png);`
    let cached = PNG_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        match cache.iter().position(|(key, _)| key == base64_data) {
            Some(pos) => {
                let (_, value) = cache.remove(pos);
                cache.push((base64_data.to_string(), value.clone()));
                Some(value)
            }
            None => None,
        }
    });
    let png = match cached {
        Some(value) => value,
        None => {
            let result = IMAGE_TRANSCODER.with(|t| {
                let borrow = t.borrow();
                let transcoder = borrow.as_ref().expect("transcoder presence checked above");
                transcoder(base64_data, mime_type)
            });
            // `pngCache.set(base64Data, png)` — the insert half.
            PNG_CACHE.with(|cache| {
                cache.borrow_mut().push((base64_data.to_string(), result.clone()))
            });
            result
        }
    };

    // `if (pngCache.size > 32) pngCache.delete(pngCache.keys().next().value!);`
    PNG_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() > 32 {
            cache.remove(0);
        }
    });
    png
}

/// TS `ImageTheme` (image.ts:43-45).
pub struct ImageTheme {
    pub fallback_color: Box<dyn Fn(&str) -> String>,
}

/// TS `ImageOptions` (image.ts:47-54) — all fields optional.
#[derive(Debug, Clone, Default)]
pub struct ImageOptions {
    pub max_width_cells: Option<usize>,
    pub max_height_cells: Option<usize>,
    pub filename: Option<String>,
    /// Kitty image ID. If provided, reuses this ID (for animations/updates).
    pub image_id: Option<i64>,
}

/// TS `class Image implements Component` (image.ts:56-167).
pub struct Image {
    base64_data: String,
    mime_type: String,
    dimensions: ImageDimensions,
    theme: ImageTheme,
    options: ImageOptions,
    image_id: Option<i64>,
    /// Converted PNG data for Kitty. Failures are not stored so a later
    /// transcoder can retry (`this.pngData ??= toPng(...) ?? undefined`).
    png_data: Option<String>,

    cached_lines: Option<Vec<String>>,
    cached_width: Option<usize>,
}

impl Image {
    /// `constructor(base64Data, mimeType, theme, options = {}, dimensions?)`
    /// (image.ts:68-82). `options` has no Rust default parameter; callers
    /// pass `ImageOptions::default()` for `{}`.
    pub fn new(
        base64_data: &str,
        mime_type: &str,
        theme: ImageTheme,
        options: ImageOptions,
        dimensions: Option<ImageDimensions>,
    ) -> Image {
        // `this.dimensions = dimensions || getImageDimensions(...) || { 800, 600 }`
        let dimensions = dimensions
            .or_else(|| get_image_dimensions(base64_data, mime_type))
            .unwrap_or(ImageDimensions { width_px: 800, height_px: 600 });
        let image_id = options.image_id;
        Image {
            base64_data: base64_data.to_string(),
            mime_type: mime_type.to_string(),
            dimensions,
            theme,
            options,
            image_id,
            png_data: None,
            cached_lines: None,
            cached_width: None,
        }
    }

    /// `getImageId()` (image.ts:85-87) — the Kitty image ID used by this
    /// image (if any).
    pub fn image_id(&self) -> Option<i64> {
        self.image_id
    }

    /// `invalidate()` (image.ts:89-92).
    pub fn invalidate(&mut self) {
        self.cached_lines = None;
        self.cached_width = None;
    }
}

impl Component for Image {
    /// `render(width)` (image.ts:94-167).
    fn render(&mut self, width: usize) -> Vec<String> {
        // `if (this.cachedLines && this.cachedWidth === width)`
        if let (Some(lines), Some(cached_width)) = (&self.cached_lines, self.cached_width) {
            if cached_width == width {
                return lines.clone();
            }
        }

        // `const maxWidth = Math.max(1, Math.min(width - 2, this.options.maxWidthCells ?? 60));`
        let max_width = 1i64.max((width as i64 - 2).min(self.options.max_width_cells.unwrap_or(60) as i64));
        let cell_dimensions = get_cell_dimensions();
        // `Math.max(1, Math.ceil((maxWidth * cellDimensions.widthPx) / cellDimensions.heightPx))`
        let default_max_height = 1i64.max(
            ((max_width as f64 * cell_dimensions.width_px as f64)
                / cell_dimensions.height_px as f64)
                .ceil() as i64,
        );
        let max_height = self
            .options
            .max_height_cells
            .map(|h| h as i64)
            .unwrap_or(default_max_height);

        let caps = get_capabilities();
        let kitty = caps.images == Some(ImageProtocol::Kitty);

        // `let data: string | null = this.base64Data; let dimensions = this.dimensions;`
        let mut dimensions = self.dimensions;
        let data: Option<&str>;
        if kitty && self.mime_type != "image/png" {
            // `this.pngData ??= toPng(...) ?? undefined;` — failure (null)
            // is NOT cached on the Image, so a later transcoder can retry.
            if self.png_data.is_none() {
                self.png_data = to_png(&self.base64_data, &self.mime_type);
            }
            // `data = this.pngData ?? null;` (JS truthiness: "" is falsy)
            data = self.png_data.as_deref().filter(|png| !png.is_empty());
            // Conversion may apply EXIF rotation, so prefer the PNG's own
            // dimensions: `if (data) dimensions = getPngDimensions(data) ?? dimensions;`
            if data.is_some() {
                dimensions = get_png_dimensions(data.expect("checked just above"))
                    .unwrap_or(dimensions);
            }
        } else {
            data = Some(self.base64_data.as_str()).filter(|d| !d.is_empty());
        }

        let lines: Vec<String> = if caps.images.is_some() && data.is_some() {
            let data = data.expect("checked just above");
            // `if (caps.images === "kitty" && this.imageId === undefined)`
            if kitty && self.image_id.is_none() {
                self.image_id = Some(allocate_image_id());
            }
            let result = render_image(
                data,
                dimensions,
                &ImageRenderOptions {
                    max_width_cells: Some(max_width),
                    max_height_cells: Some(max_height),
                    preserve_aspect_ratio: None,
                    image_id: self.image_id,
                    move_cursor: Some(false),
                },
            );

            match result {
                Some(result) => {
                    // Store the image ID for later cleanup
                    // (`if (result.imageId) this.imageId = result.imageId;`)
                    if let Some(image_id) = result.image_id {
                        self.image_id = Some(image_id);
                    }

                    if kitty {
                        // For Kitty: C=1 prevents cursor movement.
                        // Don't need the cursor movement.
                        // `lines = [result.sequence];` then `rows - 1`
                        // empty lines so TUI accounts for image height.
                        let mut lines = vec![result.sequence];
                        for _ in 0..(result.rows - 1) {
                            lines.push(String::new());
                        }
                        lines
                    } else {
                        // First (rows-1) lines are empty and cleared before
                        // the image is drawn. Last line: move cursor back
                        // up, draw the image, then move back down so TUI
                        // cursor accounting stays inside the scroll area.
                        let mut lines: Vec<String> = Vec::new();
                        for _ in 0..(result.rows - 1) {
                            lines.push(String::new());
                        }
                        let row_offset = result.rows - 1;
                        let move_up = if row_offset > 0 {
                            format!("\x1b[{row_offset}A")
                        } else {
                            String::new()
                        };
                        lines.push(move_up + &result.sequence);
                        lines
                    }
                }
                None => self.fallback_lines(width),
            }
        } else {
            self.fallback_lines(width)
        };

        self.cached_lines = Some(lines.clone());
        self.cached_width = Some(width);
        lines
    }

    /// `invalidate()` (image.ts:89-92).
    fn invalidate(&mut self) {
        self.cached_lines = None;
        self.cached_width = None;
    }
}

impl Image {
    /// The shared fallback path (image.ts:154-155, 162-163):
    /// `[truncateToWidth(this.theme.fallbackColor(imageFallback(...)), width)]`.
    fn fallback_lines(&self, width: usize) -> Vec<String> {
        let fallback = image_fallback(
            &self.mime_type,
            Some(self.dimensions),
            self.options.filename.as_deref(),
        );
        let colored = (self.theme.fallback_color)(&fallback);
        vec![truncate_to_width_default(&colored, width)]
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::terminal_image::{
        base64_encode, set_capabilities, set_image_id_random_source, TerminalCapabilities,
    };

    /// `"[Image: ...]"` fallback body for `[image/jpeg] 800x600`
    /// (default dimensions, no filename, hyperlinks off).
    const DEFAULT_FALLBACK: &str = "[Image: [image/jpeg] 800x600]";

    fn plain_theme() -> ImageTheme {
        ImageTheme { fallback_color: Box::new(|s| s.to_string()) }
    }

    fn counting_theme(count: &Arc<AtomicUsize>) -> ImageTheme {
        let count = Arc::clone(count);
        ImageTheme {
            fallback_color: Box::new(move |s| {
                count.fetch_add(1, Ordering::SeqCst);
                s.to_string()
            }),
        }
    }

    fn set_caps(images: Option<ImageProtocol>) {
        set_capabilities(TerminalCapabilities { images, true_color: true, hyperlinks: false });
    }

    /// Deterministic image IDs: `allocateImageId()` always returns 1.
    fn fix_image_ids() {
        set_image_id_random_source(Some(Box::new(|| 0.0)));
    }

    /// Fake PNG whose IHDR claims `width_px` x `height_px` — only the magic
    /// bytes and dimension fields are read by `get_png_dimensions`.
    fn fake_png(width_px: u32, height_px: u32) -> String {
        let mut bytes = vec![0u8; 24];
        bytes[0] = 0x89;
        bytes[1] = 0x50;
        bytes[2] = 0x4E;
        bytes[3] = 0x47;
        bytes[16..20].copy_from_slice(&width_px.to_be_bytes());
        bytes[20..24].copy_from_slice(&height_px.to_be_bytes());
        base64_encode(&bytes)
    }

    fn counting_transcoder(count: &Arc<AtomicUsize>) -> ImageTranscoder {
        let count = Arc::clone(count);
        Box::new(move |data: &str, _mime: &str| {
            count.fetch_add(1, Ordering::SeqCst);
            Some(format!("png:{data}"))
        })
    }

    fn png_cache_len() -> usize {
        PNG_CACHE.with(|cache| cache.borrow().len())
    }

    // -------------------------------------------------------------------------
    // pngCache LRU (toPng)
    // -------------------------------------------------------------------------

    #[test]
    fn png_cache_lru_eviction_and_reinsert_ordering() {
        let count = Arc::new(AtomicUsize::new(0));
        set_image_transcoder(Some(counting_transcoder(&count)));

        // Fill past 32 entries: the 33rd insert evicts the LRU (k1).
        for i in 1..=33 {
            let key = format!("k{i}");
            assert_eq!(to_png(&key, "image/jpeg"), Some(format!("png:{key}")));
        }
        assert_eq!(count.load(Ordering::SeqCst), 33);
        assert_eq!(png_cache_len(), 32);

        // k1 was evicted: a fresh access re-invokes the transcoder, and the
        // re-insert evicts the new LRU (k2).
        assert_eq!(to_png("k1", "image/jpeg"), Some("png:k1".to_string()));
        assert_eq!(count.load(Ordering::SeqCst), 34);
        assert_eq!(png_cache_len(), 32);

        // k2 was evicted by k1's re-insert: another transcoder call. It is
        // now the MRU entry.
        assert_eq!(to_png("k2", "image/jpeg"), Some("png:k2".to_string()));
        assert_eq!(count.load(Ordering::SeqCst), 35);

        // A new key overflows the cache again; the LRU is now k3 (k2 was
        // refreshed), so k2 survives and k3 is evicted.
        assert_eq!(to_png("k34", "image/jpeg"), Some("png:k34".to_string()));
        assert_eq!(count.load(Ordering::SeqCst), 36);
        assert_eq!(png_cache_len(), 32);
        assert_eq!(to_png("k2", "image/jpeg"), Some("png:k2".to_string()));
        assert_eq!(count.load(Ordering::SeqCst), 36);
        assert_eq!(to_png("k3", "image/jpeg"), Some("png:k3".to_string()));
        assert_eq!(count.load(Ordering::SeqCst), 37);

        // set_image_transcoder clears the cache (and unregisters).
        set_image_transcoder(None);
        assert_eq!(png_cache_len(), 0);
        // Without a transcoder, toPng returns null without touching the cache.
        assert_eq!(to_png("k2", "image/jpeg"), None);
        assert_eq!(png_cache_len(), 0);
    }

    #[test]
    fn cached_null_is_cached_but_image_retries() {
        set_caps(Some(ImageProtocol::Kitty));
        fix_image_ids();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        set_image_transcoder(Some(Box::new(move |data: &str, _mime: &str| {
            counter.fetch_add(1, Ordering::SeqCst);
            if data == "bad-data" {
                None
            } else {
                Some(fake_png(45, 90))
            }
        })));

        let expected_fallback = truncate_to_width_default(DEFAULT_FALLBACK, 20);
        let mut image =
            Image::new("bad-data", "image/jpeg", plain_theme(), ImageOptions::default(), None);
        // Transcode fails → data null → text fallback (default dims,
        // truncated to the render width).
        let lines = image.render(20);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], expected_fallback);
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // Same width: cachedLines hit, nothing re-runs.
        image.render(20);
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // After invalidate: the Image retries toPng (failure was not stored
        // on the Image), but pngCache holds the cached null, so the
        // transcoder itself is not re-invoked.
        image.invalidate();
        let lines = image.render(20);
        assert_eq!(lines, vec![expected_fallback]);
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // Registering a new transcoder clears the pngCache; the retry now
        // succeeds and the Kitty path renders.
        set_image_transcoder(Some(Box::new(|_data: &str, _mime: &str| {
            Some(fake_png(45, 90))
        })));
        image.invalidate();
        let lines = image.render(20);
        assert_eq!(lines.len(), 9);
        assert!(lines[0].starts_with("\x1b_G"));
        assert!(lines[1..].iter().all(|line| line.is_empty()));
    }

    // -------------------------------------------------------------------------
    // Fallback path (no capabilities)
    // -------------------------------------------------------------------------

    #[test]
    fn fallback_when_no_capabilities() {
        set_caps(None);
        let theme = ImageTheme { fallback_color: Box::new(|s| format!("<c>{s}</c>")) };
        let options = ImageOptions {
            filename: Some("pic.jpg".to_string()),
            ..Default::default()
        };
        let mut image = Image::new("abc", "image/jpeg", theme, options, None);

        let lines = image.render(40);
        let fallback = image_fallback(
            "image/jpeg",
            Some(ImageDimensions { width_px: 800, height_px: 600 }),
            Some("pic.jpg"),
        );
        assert_eq!(
            lines,
            vec![truncate_to_width_default(&format!("<c>{fallback}</c>"), 40)]
        );

        // Narrow width truncates to the render width.
        let mut image = Image::new("abc", "image/jpeg", plain_theme(), ImageOptions::default(), None);
        let lines = image.render(15);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            truncate_to_width_default(DEFAULT_FALLBACK, 15)
        );
        assert!(crate::utils::visible_width(&lines[0]) <= 15);
    }

    #[test]
    fn default_dimensions_when_get_image_dimensions_returns_none() {
        set_caps(None);
        // getJpegDimensions always returns None (upstream stub) → default 800x600.
        let mut image = Image::new("x", "image/jpeg", plain_theme(), ImageOptions::default(), None);
        assert_eq!(image.render(80), vec![DEFAULT_FALLBACK.to_string()]);

        // Precomputed dimensions win over getImageDimensions.
        let mut image = Image::new(
            "x",
            "image/jpeg",
            plain_theme(),
            ImageOptions::default(),
            Some(ImageDimensions { width_px: 100, height_px: 50 }),
        );
        assert_eq!(image.render(80), vec!["[Image: [image/jpeg] 100x50]".to_string()]);

        // PNG data parses in the constructor when no dimensions are given.
        let png = fake_png(45, 90);
        let mut image = Image::new(&png, "image/png", plain_theme(), ImageOptions::default(), None);
        assert_eq!(image.render(80), vec!["[Image: [image/png] 45x90]".to_string()]);
    }

    // -------------------------------------------------------------------------
    // Kitty path
    // -------------------------------------------------------------------------

    #[test]
    fn kitty_transcodes_non_png_and_uses_png_dimensions() {
        set_caps(Some(ImageProtocol::Kitty));
        fix_image_ids();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        set_image_transcoder(Some(Box::new(move |_data: &str, _mime: &str| {
            counter.fetch_add(1, Ordering::SeqCst);
            Some(fake_png(45, 90))
        })));

        let mut image =
            Image::new("jpeg-bytes", "image/jpeg", plain_theme(), ImageOptions::default(), None);
        let lines = image.render(20);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // 45x90 px at 9x18 cells, maxWidth 18 / maxHeight 9 → rows 9:
        // [sequence] + 8 empty lines.
        assert_eq!(lines.len(), 9);
        assert!(lines[0].starts_with("\x1b_G"));
        assert!(lines[0].contains("i=1"));
        assert!(lines[1..].iter().all(|line| line.is_empty()));
        // imageId was allocated and stored.
        assert_eq!(image.image_id(), Some(1));

        // A second Image with the same source data hits the pngCache.
        let mut image2 =
            Image::new("jpeg-bytes", "image/jpeg", plain_theme(), ImageOptions::default(), None);
        image2.render(20);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn kitty_renders_png_directly_without_transcoder() {
        set_caps(Some(ImageProtocol::Kitty));
        fix_image_ids();
        // No transcoder registered at all — a PNG must still render.
        set_image_transcoder(None);
        let png = fake_png(45, 90);
        let mut image = Image::new(&png, "image/png", plain_theme(), ImageOptions::default(), None);
        let lines = image.render(20);
        assert_eq!(lines.len(), 9);
        assert!(lines[0].starts_with("\x1b_G"));
        assert!(lines[0].contains("i=1"));
        assert!(lines[1..].iter().all(|line| line.is_empty()));
        assert_eq!(image.image_id(), Some(1));

        // A provided imageId is reused (animations/updates).
        let mut image = Image::new(
            &png,
            "image/png",
            plain_theme(),
            ImageOptions { image_id: Some(42), ..Default::default() },
            None,
        );
        let lines = image.render(20);
        assert!(lines[0].contains("i=42"));
        assert_eq!(image.image_id(), Some(42));
    }

    // -------------------------------------------------------------------------
    // iTerm2 path
    // -------------------------------------------------------------------------

    #[test]
    fn iterm_line_shape_is_empty_rows_then_cursor_up_sequence() {
        set_caps(Some(ImageProtocol::Iterm2));
        let png = fake_png(45, 90);
        let mut image = Image::new(&png, "image/png", plain_theme(), ImageOptions::default(), None);
        let lines = image.render(20);
        // rows = 9: 8 empty lines, then cursor-up + sequence on the last.
        assert_eq!(lines.len(), 9);
        assert!(lines[..8].iter().all(|line| line.is_empty()));
        assert!(lines[8].starts_with("\x1b[8A\x1b]1337;File="));
        assert!(lines[8].contains(&png));

        // Single-row image: rowOffset 0 → no cursor-up prefix at all.
        let mut image = Image::new(
            &fake_png(162, 9),
            "image/png",
            plain_theme(),
            ImageOptions::default(),
            None,
        );
        let lines = image.render(20);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("\x1b]1337;File="));
    }

    // -------------------------------------------------------------------------
    // Render cache
    // -------------------------------------------------------------------------

    #[test]
    fn render_cache_hit_on_same_width_and_invalidate() {
        set_caps(None);
        let count = Arc::new(AtomicUsize::new(0));
        let options = ImageOptions { filename: Some("f.jpg".to_string()), ..Default::default() };
        let mut image = Image::new("d", "image/jpeg", counting_theme(&count), options, None);

        // "[Image: f.jpg [image/jpeg] 800x600]" is 35 columns wide.
        let first = image.render(30);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Same width: cached lines returned, no re-render.
        assert_eq!(image.render(30), first);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Different width: recomputed (different truncation).
        let wide = image.render(40);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_ne!(first, wide);
        assert!(crate::utils::visible_width(&first[0]) <= 30);
        // invalidate() clears the cache: recompute at the same width.
        image.invalidate();
        assert_eq!(image.render(40), wide);
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
