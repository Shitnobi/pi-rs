//! The `read` tool. Port of `src/core/tools/read.ts` (text path; image
//! handling is tracked as a follow-up in TRACKING.md).

use serde::Deserialize;

use pi_core::paths::resolve_read_path;
use pi_core::truncate::{format_size, truncate_head, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};

use crate::node_fs;
use crate::{ToolError, ToolOutput, ToolResult};

#[derive(Debug, Deserialize)]
pub struct ReadToolInput {
    pub path: String,
    /// Line number to start reading from (1-indexed)
    pub offset: Option<f64>,
    /// Maximum number of lines to read
    pub limit: Option<f64>,
}

/// Image MIME sniffing port of `src/utils/mime.ts`
/// (`detectSupportedImageMimeType` on the first bytes).
pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    fn starts_with(buffer: &[u8], prefix: &[u8]) -> bool {
        buffer.len() >= prefix.len() && &buffer[..prefix.len()] == prefix
    }
    fn starts_with_ascii(buffer: &[u8], offset: usize, s: &str) -> bool {
        buffer.len() >= offset + s.len() && &buffer[offset..offset + s.len()] == s.as_bytes()
    }
    fn read_uint32_be(buffer: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([buffer[offset], buffer[offset + 1], buffer[offset + 2], buffer[offset + 3]])
    }

    if starts_with(buffer, &[0xff, 0xd8, 0xff]) {
        return if buffer.get(3) == Some(&0xf7) { None } else { Some("image/jpeg") };
    }
    const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    if starts_with(buffer, &PNG_SIGNATURE) {
        let is_png = buffer.len() >= 16 && read_uint32_be(buffer, 8) == 13 && starts_with_ascii(buffer, 12, "IHDR");
        if is_png && !is_animated_png(buffer) {
            return Some("image/png");
        }
        return None;
    }
    if starts_with_ascii(buffer, 0, "GIF87a") || starts_with_ascii(buffer, 0, "GIF89a") {
        return Some("image/gif");
    }
    if starts_with_ascii(buffer, 0, "RIFF") && starts_with_ascii(buffer, 8, "WEBP") {
        return Some("image/webp");
    }
    if starts_with_ascii(buffer, 0, "BM") && is_bmp(buffer) {
        return Some("image/bmp");
    }
    None
}

/// Full validation port of `isBmp` in `src/utils/mime.ts`.
fn is_bmp(buffer: &[u8]) -> bool {
    fn read_u32_le(buffer: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([buffer[offset], buffer[offset + 1], buffer[offset + 2], buffer[offset + 3]])
    }
    fn read_u16_le(buffer: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([buffer[offset], buffer[offset + 1]])
    }
    if buffer.len() < 26 {
        return false;
    }
    let declared_file_size = read_u32_le(buffer, 2);
    let pixel_data_offset = read_u32_le(buffer, 10);
    let dib_header_size = read_u32_le(buffer, 14);
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }
    let (color_planes, bits_per_pixel): (u16, u16);
    if dib_header_size == 12 {
        color_planes = read_u16_le(buffer, 22);
        bits_per_pixel = read_u16_le(buffer, 24);
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        color_planes = read_u16_le(buffer, 26);
        bits_per_pixel = read_u16_le(buffer, 28);
    } else {
        return false;
    }
    color_planes == 1 && [1u16, 4, 8, 16, 24, 32].contains(&bits_per_pixel)
}

fn is_animated_png(buffer: &[u8]) -> bool {
    fn read_uint32_be(buffer: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([buffer[offset], buffer[offset + 1], buffer[offset + 2], buffer[offset + 3]])
    }
    fn starts_with_ascii(buffer: &[u8], offset: usize, s: &str) -> bool {
        buffer.len() >= offset + s.len() && &buffer[offset..offset + s.len()] == s.as_bytes()
    }
    const PNG_SIGNATURE_LEN: usize = 8;
    let mut offset = PNG_SIGNATURE_LEN;
    while offset + 8 <= buffer.len() {
        let chunk_length = read_uint32_be(buffer, offset) as usize;
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buffer, chunk_type_offset, "acTL") {
            return true;
        }
        if starts_with_ascii(buffer, chunk_type_offset, "IDAT") {
            return false;
        }
        let next_offset = offset + 8 + chunk_length + 4;
        if next_offset <= offset || next_offset > buffer.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

/// Port of the read tool's `execute` (text files; images return a
/// non-resized image block which is a documented phase-2 gap).
pub fn read_execute(cwd: &str, input: &ReadToolInput) -> ToolResult {
    let absolute_path = resolve_read_path(&input.path, cwd);

    // Check if file exists and is readable.
    if let Err(message) = node_fs::node_access(&absolute_path) {
        return Err(ToolError::new(message));
    }

    let buffer = match std::fs::read(&absolute_path) {
        Ok(b) => b,
        Err(err) => {
            return Err(ToolError::new(node_fs::node_error_message("open", &absolute_path, &err)));
        }
    };

    // Image path. The original converts/resizes via the photon WASM
    // module (image-process.ts) before returning base64; that is a
    // documented phase-2 gap (see TRACKING.md). Phase 1 returns the raw
    // image data, but with the original result shape: a text note plus an
    // image content block, no details, and structuredContent describing
    // the image.
    if let Some(mime) = detect_supported_image_mime_type(&buffer) {
        let note = format!("Read image file [{}]", mime);
        let data = base64_encode(&buffer);
        return Ok(ToolOutput {
            content: vec![
                crate::ContentBlock::Text { text: note.clone() },
                crate::ContentBlock::Image { data: data.clone(), mime_type: mime.to_string() },
            ],
            details: None,
            is_error: None,
            structured_content: Some(serde_json::json!({
                "type": "image", "data": data, "mimeType": mime, "note": note,
            })),
        });
    }

    // Read text content.
    let text_content = String::from_utf8_lossy(&buffer).into_owned();
    let all_lines: Vec<&str> = text_content.split('\n').collect();
    let total_file_lines = all_lines.len();

    // Apply offset if specified. Convert from 1-indexed input to 0-indexed.
    let offset = input.offset.map(|o| o as i64).filter(|o| *o != 0);
    let start_line: usize = offset.map(|o| std::cmp::max(0, o - 1) as usize).unwrap_or(0);
    let start_line_display = start_line + 1;

    // Check if offset is out of bounds.
    if start_line >= all_lines.len() {
        return Err(ToolError::new(format!(
            "Offset {} is beyond end of file ({} lines total)",
            input.offset.unwrap(),
            total_file_lines
        )));
    }

    let limit = input.limit.map(|l| l as i64).filter(|l| *l >= 0);

    let selected_content: String;
    let mut user_limited_lines: Option<usize> = None;
    if let Some(limit) = limit {
        // JS: endLine = Math.min(startLine + limit, allLines.length);
        // slice(startLine, endLine) is empty when endLine <= startLine.
        let end_line = std::cmp::min(start_line.saturating_add(limit.max(0) as usize), all_lines.len());
        let end_line = std::cmp::max(end_line, start_line);
        selected_content = all_lines[start_line..end_line].join("\n");
        user_limited_lines = Some(end_line - start_line);
    } else {
        selected_content = all_lines[start_line..].join("\n");
    }

    // Apply truncation, respecting both line and byte limits.
    let truncation = truncate_head(&selected_content, Default::default());
    let output_text: String;
    let details: Option<serde_json::Value>;
    if truncation.first_line_exceeds_limit {
        // First line alone exceeds the byte limit.
        let first_line_size = format_size(all_lines[start_line].len() as u64);
        output_text = format!(
            "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
            start_line_display,
            first_line_size,
            format_size(DEFAULT_MAX_BYTES),
            start_line_display,
            input.path,
            DEFAULT_MAX_BYTES
        );
        details = Some(serde_json::json!({ "truncation": truncation }));
    } else if truncation.truncated {
        // Truncation occurred. Build an actionable continuation notice.
        let end_line_display = start_line_display + truncation.output_lines as usize - 1;
        let next_offset = end_line_display + 1;
        let mut text = truncation.content.clone();
        if truncation.truncated_by.as_deref() == Some("lines") {
            text.push_str(&format!(
                "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                start_line_display, end_line_display, total_file_lines, next_offset
            ));
        } else {
            text.push_str(&format!(
                "\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                start_line_display,
                end_line_display,
                total_file_lines,
                format_size(DEFAULT_MAX_BYTES),
                next_offset
            ));
        }
        output_text = text;
        details = Some(serde_json::json!({ "truncation": truncation }));
    } else if let Some(user_limited) = user_limited_lines {
        if start_line + user_limited < all_lines.len() {
            // User-specified limit stopped early.
            let remaining = all_lines.len() - (start_line + user_limited);
            let next_offset = start_line + user_limited + 1;
            output_text = format!(
                "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                truncation.content, remaining, next_offset
            );
            details = None;
        } else {
            output_text = truncation.content;
            details = None;
        }
    } else {
        output_text = truncation.content;
        details = None;
    }

    let _ = DEFAULT_MAX_LINES;
    Ok(ToolOutput {
        content: vec![crate::ContentBlock::Text { text: output_text.clone() }],
        details,
        is_error: None,
        // toReadOutput: the text of the first text block.
        structured_content: Some(serde_json::json!(output_text)),
    })
}

/// Minimal base64 (standard alphabet, padded), matching Buffer.toString("base64").
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}
