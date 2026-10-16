//! Port of pi's image processing pipeline:
//! - `utils/image-process.ts` — `processImage` (normalize + resize + hints)
//! - `utils/image-resize-core.ts` — `resizeImageInProcess`
//! - `utils/image-convert.ts` — `convertImageBytesToPng`
//! - `utils/exif-orientation.ts` — EXIF orientation detection/apply
//!
//! All decode/resize/encode work runs through [`pi_photon`], which executes
//! the exact same WASM binary as the original — so outputs are byte-identical.

use pi_photon::PhotonImage;

use crate::read::base64_encode;

// ============================================================================
// Options / results (image-resize-core.ts)
// ============================================================================

/// 4.5MB of base64 payload. Provides headroom below Anthropic's 5MB limit.
const DEFAULT_MAX_BYTES: f64 = 4.5 * 1024.0 * 1024.0;

const DEFAULT_MAX_WIDTH: u32 = 2000;
const DEFAULT_MAX_HEIGHT: u32 = 2000;
const DEFAULT_JPEG_QUALITY: u8 = 80;

#[derive(Debug, Clone, Copy)]
pub struct ImageResizeOptions {
	pub max_width: u32,
	pub max_height: u32,
	pub max_bytes: f64,
	pub jpeg_quality: u8,
}

impl Default for ImageResizeOptions {
	fn default() -> Self {
		ImageResizeOptions {
			max_width: DEFAULT_MAX_WIDTH,
			max_height: DEFAULT_MAX_HEIGHT,
			max_bytes: DEFAULT_MAX_BYTES,
			jpeg_quality: DEFAULT_JPEG_QUALITY,
		}
	}
}

#[derive(Debug, Clone)]
pub struct ResizedImage {
	/// base64
	pub data: String,
	pub mime_type: String,
	pub original_width: u32,
	pub original_height: u32,
	pub width: u32,
	pub height: u32,
	pub was_resized: bool,
}

#[derive(Debug, Clone)]
pub enum ProcessImageResult {
	Ok {
		/// base64
		data: String,
		mime_type: String,
		hints: Vec<String>,
	},
	Err {
		message: String,
	},
}

// ============================================================================
// processImage (image-process.ts)
// ============================================================================

fn base_mime_type(mime_type: &str) -> String {
	mime_type
		.split(';')
		.next()
		.unwrap_or(mime_type)
		.trim()
		.to_lowercase()
}

fn normalize_supported_image_mime_type(mime_type: &str) -> Option<&'static str> {
	match base_mime_type(mime_type).as_str() {
		"image/png" => Some("image/png"),
		"image/jpeg" | "image/jpg" => Some("image/jpeg"),
		"image/gif" => Some("image/gif"),
		"image/webp" => Some("image/webp"),
		_ => None,
	}
}

/// `processImage` — normalize to a supported inline format, optionally resize
/// to provider limits, and produce user-facing hints.
pub fn process_image(
	bytes: &[u8],
	mime_type: &str,
	auto_resize_images: bool,
	resize_options: Option<&ImageResizeOptions>,
) -> ProcessImageResult {
	// normalizeImage
	let (normalized_bytes, normalized_mime, converted_from) =
		match normalize_supported_image_mime_type(mime_type) {
			Some(supported) => (bytes.to_vec(), supported.to_string(), None),
			None => {
				// convertImageBytesToPng
				match convert_image_bytes_to_png(bytes) {
					Some(png_bytes) => (png_bytes, "image/png".to_string(), Some(base_mime_type(mime_type))),
					None => {
						return ProcessImageResult::Err {
							message: "[Image omitted: could not be converted to a supported inline image format.]"
								.to_string(),
						}
					}
				}
			}
		};

	if auto_resize_images {
		let resized = match resize_image(&normalized_bytes, &normalized_mime, resize_options) {
			Some(r) => r,
			None => {
				return ProcessImageResult::Err {
					message: "[Image omitted: could not be resized below the inline image size limit.]".to_string(),
				}
			}
		};

		let mut hints = Vec::new();
		if let Some(hint) = conversion_hint(converted_from.as_deref(), &resized.mime_type) {
			hints.push(hint);
		}
		if let Some(note) = format_dimension_note(&resized) {
			hints.push(note);
		}

		ProcessImageResult::Ok { data: resized.data, mime_type: resized.mime_type, hints }
	} else {
		let mut hints = Vec::new();
		if let Some(hint) = conversion_hint(converted_from.as_deref(), &normalized_mime) {
			hints.push(hint);
		}
		ProcessImageResult::Ok {
			data: base64_encode(&normalized_bytes),
			mime_type: normalized_mime,
			hints,
		}
	}
}

fn conversion_hint(from: Option<&str>, to: &str) -> Option<String> {
	let from = from?;
	if from == to {
		return None;
	}
	Some(format!("[Image converted from {} to {}.]", from, to))
}

/// `formatDimensionNote` — helps the model map display coordinates back to
/// the original image.
pub fn format_dimension_note(result: &ResizedImage) -> Option<String> {
	if !result.was_resized {
		return None;
	}
	let scale = result.original_width as f64 / result.width as f64;
	Some(format!(
		"[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {} to map to original image.]",
		result.original_width,
		result.original_height,
		result.width,
		result.height,
		pi_core::js_string::js_to_fixed_2(scale)
	))
}

// ============================================================================
// resizeImageInProcess (image-resize-core.ts)
// ============================================================================

struct EncodedCandidate {
	data: String,
	encoded_size: usize,
	mime_type: String,
}

/// `resizeImage` / `resizeImageInProcess` — fit within max dimensions and
/// encoded (base64) size. Returns `None` if the image cannot be resized below
/// `max_bytes`.
///
/// Strategy:
/// 1. First resize to maxWidth/maxHeight
/// 2. Try both PNG and JPEG formats, pick the smaller one (first that fits)
/// 3. If still too large, try JPEG with decreasing quality
/// 4. If still too large, progressively reduce dimensions until 1x1
pub fn resize_image(
	input_bytes: &[u8],
	mime_type: &str,
	options: Option<&ImageResizeOptions>,
) -> Option<ResizedImage> {
	let opts = options.copied().unwrap_or_default();
	// Math.ceil(byteLength / 3) * 4
	let input_base64_size = (input_bytes.len().div_ceil(3) as u64 * 4) as f64;

	// The whole body is a try/catch in the original: decode or encode
	// failures (wasm panics on corrupt data) map to None.
	let raw_image = PhotonImage::new_from_byteslice(input_bytes).ok()?;
	let image = apply_exif_orientation(raw_image, input_bytes);

	let original_width = image.width();
	let original_height = image.height();
	let format = mime_type.split('/').nth(1).unwrap_or("png");

	// Check if already within all limits (dimensions AND encoded size).
	if original_width <= opts.max_width
		&& original_height <= opts.max_height
		&& input_base64_size < opts.max_bytes
	{
		let mime = if mime_type.is_empty() { format!("image/{}", format) } else { mime_type.to_string() };
		return Some(ResizedImage {
			data: base64_encode(input_bytes),
			mime_type: mime,
			original_width,
			original_height,
			width: original_width,
			height: original_height,
			was_resized: false,
		});
	}

	// Calculate initial dimensions respecting max limits.
	let mut target_width = original_width;
	let mut target_height = original_height;
	if target_width > opts.max_width {
		target_height = ((target_height as f64 * opts.max_width as f64) / target_width as f64).round() as u32;
		target_width = opts.max_width;
	}
	if target_height > opts.max_height {
		target_width = ((target_width as f64 * opts.max_height as f64) / target_height as f64).round() as u32;
		target_height = opts.max_height;
	}

	// qualitySteps = Array.from(new Set([opts.jpegQuality, 85, 70, 55, 40]))
	let mut quality_steps: Vec<u8> = Vec::new();
	for q in [opts.jpeg_quality, 85, 70, 55, 40] {
		if !quality_steps.contains(&q) {
			quality_steps.push(q);
		}
	}

	let try_encodings = |width: u32, height: u32, jpeg_qualities: &[u8]| -> Option<Vec<EncodedCandidate>> {
		let resized = image.resize(width, height).ok()?;
		let mut candidates = vec![EncodedCandidate {
			data: base64_encode(&resized.get_bytes().ok()?),
			encoded_size: 0,
			mime_type: "image/png".to_string(),
		}];
		for &quality in jpeg_qualities {
			let jpeg = resized.get_bytes_jpeg(quality).ok()?;
			candidates.push(EncodedCandidate {
				data: base64_encode(&jpeg),
				encoded_size: 0,
				mime_type: "image/jpeg".to_string(),
			});
		}
		for candidate in &mut candidates {
			// Buffer.byteLength(base64 string, "utf-8") == string length
			candidate.encoded_size = candidate.data.len();
		}
		Some(candidates)
	};

	let mut current_width = target_width;
	let mut current_height = target_height;

	loop {
		let candidates = try_encodings(current_width, current_height, &quality_steps)?;
		for candidate in candidates {
			if (candidate.encoded_size as f64) < opts.max_bytes {
				return Some(ResizedImage {
					data: candidate.data,
					mime_type: candidate.mime_type,
					original_width,
					original_height,
					width: current_width,
					height: current_height,
					was_resized: true,
				});
			}
		}

		if current_width == 1 && current_height == 1 {
			break;
		}

		let next_width = if current_width == 1 { 1 } else { std::cmp::max(1, (current_width as f64 * 0.75).floor() as u32) };
		let next_height = if current_height == 1 { 1 } else { std::cmp::max(1, (current_height as f64 * 0.75).floor() as u32) };
		if next_width == current_width && next_height == current_height {
			break;
		}

		current_width = next_width;
		current_height = next_height;
	}

	None
}

// ============================================================================
// convertImageBytesToPng (image-convert.ts)
// ============================================================================

fn convert_image_bytes_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
	let raw_image = PhotonImage::new_from_byteslice(bytes).ok()?;
	let image = apply_exif_orientation(raw_image, bytes);
	image.get_bytes().ok()
}

// ============================================================================
// EXIF orientation (exif-orientation.ts)
// ============================================================================

fn read_orientation_from_tiff(bytes: &[u8], tiff_start: usize) -> u32 {
	if tiff_start + 8 > bytes.len() {
		return 1;
	}

	let byte_order = ((bytes[tiff_start] as u32) << 8) | bytes[tiff_start + 1] as u32;
	let le = byte_order == 0x4949;

	let read16 = |pos: usize| -> u32 {
		if le {
			bytes[pos] as u32 | ((bytes[pos + 1] as u32) << 8)
		} else {
			((bytes[pos] as u32) << 8) | bytes[pos + 1] as u32
		}
	};
	let read32 = |pos: usize| -> u32 {
		if le {
			bytes[pos] as u32 | ((bytes[pos + 1] as u32) << 8) | ((bytes[pos + 2] as u32) << 16) | ((bytes[pos + 3] as u32) << 24)
		} else {
			((bytes[pos] as u32) << 24) | ((bytes[pos + 1] as u32) << 16) | ((bytes[pos + 2] as u32) << 8) | bytes[pos + 3] as u32
		}
	};

	let ifd_offset = read32(tiff_start + 4) as usize;
	let ifd_start = tiff_start + ifd_offset;
	if ifd_start + 2 > bytes.len() {
		return 1;
	}

	let entry_count = read16(ifd_start);
	for i in 0..entry_count {
		let entry_pos = ifd_start + 2 + (i as usize) * 12;
		if entry_pos + 12 > bytes.len() {
			return 1;
		}
		if read16(entry_pos) == 0x0112 {
			let value = read16(entry_pos + 8);
			return if (1..=8).contains(&value) { value } else { 1 };
		}
	}

	1
}

fn find_jpeg_tiff_offset(bytes: &[u8]) -> Option<usize> {
	let mut offset = 2usize;
	while offset < bytes.len().saturating_sub(1) {
		if bytes[offset] != 0xff {
			return None;
		}
		let marker = bytes[offset + 1];
		if marker == 0xff {
			offset += 1;
			continue;
		}

		if marker == 0xe1 {
			if offset + 4 >= bytes.len() {
				return None;
			}
			let segment_start = offset + 4;
			if segment_start + 6 > bytes.len() {
				return None;
			}
			if has_exif_header(bytes, segment_start) {
				return Some(segment_start + 6);
			}
		}

		if offset + 4 > bytes.len() {
			return None;
		}
		let length = ((bytes[offset + 2] as usize) << 8) | bytes[offset + 3] as usize;
		offset += 2 + length;
	}

	None
}

fn find_webp_tiff_offset(bytes: &[u8]) -> Option<usize> {
	let mut offset = 12usize;
	while offset + 8 <= bytes.len() {
		let chunk_id = &bytes[offset..offset + 4];
		let chunk_size = bytes[offset + 4] as usize
			| ((bytes[offset + 5] as usize) << 8)
			| ((bytes[offset + 6] as usize) << 16)
			| ((bytes[offset + 7] as usize) << 24);
		let data_start = offset + 8;

		if chunk_id == b"EXIF" {
			if data_start + chunk_size > bytes.len() {
				return None;
			}
			// Some WebP files have "Exif\0\0" prefix before the TIFF header
			let tiff_start = if chunk_size >= 6 && has_exif_header(bytes, data_start) {
				data_start + 6
			} else {
				data_start
			};
			return Some(tiff_start);
		}

		// RIFF chunks are padded to even size
		offset = data_start + chunk_size + (chunk_size % 2);
	}

	None
}

fn has_exif_header(bytes: &[u8], offset: usize) -> bool {
	bytes.len() >= offset + 6
		&& bytes[offset] == 0x45
		&& bytes[offset + 1] == 0x78
		&& bytes[offset + 2] == 0x69
		&& bytes[offset + 3] == 0x66
		&& bytes[offset + 4] == 0x00
		&& bytes[offset + 5] == 0x00
}

fn get_exif_orientation(bytes: &[u8]) -> u32 {
	let tiff_offset = if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xd8 {
		find_jpeg_tiff_offset(bytes)
	} else if bytes.len() >= 12
		&& bytes[0] == 0x52
		&& bytes[1] == 0x49
		&& bytes[2] == 0x46
		&& bytes[3] == 0x46
		&& bytes[8] == 0x57
		&& bytes[9] == 0x45
		&& bytes[10] == 0x42
		&& bytes[11] == 0x50
	{
		find_webp_tiff_offset(bytes)
	} else {
		None
	};

	match tiff_offset {
		Some(offset) => read_orientation_from_tiff(bytes, offset),
		None => 1,
	}
}

/// `applyExifOrientation` — takes ownership of `image` and returns an image
/// with EXIF orientation applied (flips mutate in place; rotations return a
/// new image, like the original).
fn apply_exif_orientation(image: PhotonImage, original_bytes: &[u8]) -> PhotonImage {
	let orientation = get_exif_orientation(original_bytes);
	if orientation == 1 {
		return image;
	}

	match orientation {
		2 => {
			let _ = image.fliph();
			image
		}
		3 => {
			let _ = image.fliph();
			let _ = image.flipv();
			image
		}
		4 => {
			let _ = image.flipv();
			image
		}
		5 => {
			if let Some(rotated) = rotate90(&image, |x, y, _w, h| x * h + (h - 1 - y)) {
				let _ = rotated.fliph();
				rotated
			} else {
				image
			}
		}
		6 => rotate90(&image, |x, y, _w, h| x * h + (h - 1 - y)).unwrap_or(image),
		7 => {
			if let Some(rotated) = rotate90(&image, |x, y, w, h| (w - 1 - x) * h + y) {
				let _ = rotated.fliph();
				rotated
			} else {
				image
			}
		}
		8 => rotate90(&image, |x, y, w, h| (w - 1 - x) * h + y).unwrap_or(image),
		_ => image,
	}
}

/// `rotate90` — pixel-level 90° rotation; the new image has the dimensions
/// swapped (`new PhotonImage(dst, h, w)` in the original).
fn rotate90<F>(image: &PhotonImage, dst_index: F) -> Option<PhotonImage>
where
	F: Fn(usize, usize, usize, usize) -> usize,
{
	let w = image.width() as usize;
	let h = image.height() as usize;
	let src = image.raw_pixels().ok()?;
	let mut dst = vec![0u8; src.len()];

	for y in 0..h {
		for x in 0..w {
			let src_idx = (y * w + x) * 4;
			let dst_idx = dst_index(x, y, w, h) * 4;
			dst[dst_idx] = src[src_idx];
			dst[dst_idx + 1] = src[src_idx + 1];
			dst[dst_idx + 2] = src[src_idx + 2];
			dst[dst_idx + 3] = src[src_idx + 3];
		}
	}

	PhotonImage::from_raw_pixels(&dst, h as u32, w as u32).ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn base_mime_type_splits_params() {
		assert_eq!(base_mime_type("image/png; charset=binary"), "image/png");
		assert_eq!(base_mime_type("  IMAGE/JPG "), "image/jpg");
	}

	#[test]
	fn normalize_supported_mimes() {
		assert_eq!(normalize_supported_image_mime_type("image/png"), Some("image/png"));
		assert_eq!(normalize_supported_image_mime_type("image/jpg"), Some("image/jpeg"));
		assert_eq!(normalize_supported_image_mime_type("image/bmp"), None);
	}

	#[test]
	fn conversion_hints() {
		assert_eq!(conversion_hint(None, "image/png"), None);
		assert_eq!(conversion_hint(Some("image/png"), "image/png"), None);
		assert_eq!(
			conversion_hint(Some("image/bmp"), "image/png"),
			Some("[Image converted from image/bmp to image/png.]".to_string())
		);
	}

	#[test]
	fn dimension_note() {
		let r = ResizedImage {
			data: String::new(),
			mime_type: "image/png".into(),
			original_width: 2500,
			original_height: 300,
			width: 2000,
			height: 240,
			was_resized: false,
		};
		assert_eq!(format_dimension_note(&r), None);
		let r2 = ResizedImage { was_resized: true, ..r };
		assert_eq!(
			format_dimension_note(&r2).unwrap(),
			"[Image: original 2500x300, displayed at 2000x240. Multiply coordinates by 1.25 to map to original image.]"
		);
	}

	#[test]
	fn exif_orientation_from_jpeg() {
		// No EXIF: stays 1.
		assert_eq!(get_exif_orientation(&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00]), 1);
		// JPEG with APP1 Exif segment, big-endian TIFF, orientation = 6.
		// Layout: FFD8 FFE1 <len:2> "Exif\0\0" <MM 0x2a 0x00000008> <count:1> <tag 0x0112 type 3 count 1> <value 6>
		let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1, 0x00, 0x20];
		jpeg.extend_from_slice(b"Exif\0\0");
		jpeg.extend_from_slice(&[0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x08]); // MM, magic, IFD at 8
		jpeg.extend_from_slice(&[0x00, 0x01]); // 1 entry
		jpeg.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x06, 0x00, 0x00]); // tag 0x0112 SHORT 1 = 6
		assert_eq!(get_exif_orientation(&jpeg), 6);
	}

	#[test]
	fn process_image_passthrough_png() {
		// A 1x1 PNG within all limits: passthrough (original bytes, no hints).
		let png: &[u8] = &[
			0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
			0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
			0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
			0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00,
			0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
		];
		match process_image(png, "image/png", true, None) {
			ProcessImageResult::Ok { data, mime_type, hints } => {
				assert_eq!(mime_type, "image/png");
				assert!(hints.is_empty());
				assert_eq!(data, base64_encode(png));
			}
			_ => panic!("expected ok"),
		}
	}

	#[test]
	fn process_image_corrupt_fails() {
		// Unconvertible bytes: the "could not be converted" path.
		match process_image(b"garbage-not-an-image", "image/bmp", true, None) {
			ProcessImageResult::Err { message } => {
				assert_eq!(message, "[Image omitted: could not be converted to a supported inline image format.]")
			}
			_ => panic!("expected err"),
		}
	}
}
