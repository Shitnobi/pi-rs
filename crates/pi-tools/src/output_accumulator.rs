//! Incrementally tracks streaming output with bounded memory.
//! Port of `src/core/tools/output-accumulator.ts`.
//!
//! Appends decode chunks with a streaming UTF-8 decoder, keeps only a
//! decoded tail for display snapshots, and opens a temp file when the full
//! output needs to be preserved.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

use serde::Serialize;

use pi_core::text_decoder::{decode_oneshot, decode_stream_holdback, StreamingDecoder};
use pi_core::truncate::{truncate_tail, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};

use crate::output_files::create_output_file_stream;

#[derive(Debug, Default, Clone)]
pub struct OutputAccumulatorOptions {
    pub max_lines: Option<u64>,
    pub max_bytes: Option<u64>,
    pub temp_file_prefix: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputSnapshot {
    pub content: String,
    pub truncation: TruncationResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FullOutput {
    pub content: String,
    /// Whether `content` omits part of the output.
    pub truncated: bool,
}

pub struct OutputAccumulator {
    max_lines: u64,
    max_bytes: u64,
    max_rolling_bytes: u64,
    temp_file_prefix: String,
    decoder: StreamingDecoder,

    raw_chunks: Vec<Vec<u8>>,
    tail_text: String,
    tail_bytes: u64,
    tail_starts_at_line_boundary: bool,
    total_raw_bytes: u64,
    total_decoded_bytes: u64,
    completed_lines: u64,
    total_lines: u64,
    current_line_bytes: u64,
    has_open_line: bool,
    finished: bool,

    temp_file_path: Option<PathBuf>,
    temp_file: Option<File>,
}

impl OutputAccumulator {
    pub fn new(options: OutputAccumulatorOptions) -> Self {
        let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
        let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
        OutputAccumulator {
            max_lines,
            max_bytes,
            max_rolling_bytes: (max_bytes * 2).max(1),
            temp_file_prefix: options.temp_file_prefix.unwrap_or_else(|| "pi-output".to_string()),
            decoder: StreamingDecoder::new(),
            raw_chunks: Vec::new(),
            tail_text: String::new(),
            tail_bytes: 0,
            tail_starts_at_line_boundary: true,
            total_raw_bytes: 0,
            total_decoded_bytes: 0,
            completed_lines: 0,
            total_lines: 0,
            current_line_bytes: 0,
            has_open_line: false,
            finished: false,
            temp_file_path: None,
            temp_file: None,
        }
    }

    pub fn append(&mut self, data: &[u8]) -> Result<(), String> {
        if self.finished {
            return Err("Cannot append to a finished output accumulator".to_string());
        }

        self.total_raw_bytes += data.len() as u64;
        let text = self.decoder.decode(data);
        self.append_decoded_text(&text);

        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_file()?;
            if let Some(f) = &mut self.temp_file {
                f.write_all(data).map_err(|e| e.to_string())?;
            }
        } else if !data.is_empty() {
            self.raw_chunks.push(data.to_vec());
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let text = self.decoder.finish();
        self.append_decoded_text(&text);
        if self.should_use_temp_file() {
            self.ensure_temp_file()?;
        }
        Ok(())
    }

    pub fn snapshot(&mut self, persist_if_truncated: bool) -> Result<OutputSnapshot, String> {
        let tail_truncation = truncate_tail(
            &self.get_snapshot_text(),
            TruncationOptions { max_lines: Some(self.max_lines), max_bytes: Some(self.max_bytes) },
        );
        let truncated = self.total_lines > self.max_lines || self.total_decoded_bytes > self.max_bytes;
        let truncated_by = if truncated {
            tail_truncation
                .truncated_by
                .clone()
                .or_else(|| if self.total_decoded_bytes > self.max_bytes { Some("bytes".to_string()) } else { Some("lines".to_string()) })
        } else {
            None
        };
        let mut truncation = tail_truncation;
        truncation.truncated = truncated;
        truncation.truncated_by = truncated_by;
        truncation.total_lines = self.total_lines;
        truncation.total_bytes = self.total_decoded_bytes;
        truncation.max_lines = self.max_lines;
        truncation.max_bytes = self.max_bytes;

        if persist_if_truncated && truncation.truncated {
            self.ensure_temp_file()?;
        }

        Ok(OutputSnapshot {
            content: truncation.content.clone(),
            truncation,
            full_output_path: self.temp_file_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        })
    }

    /// Close the temp file stream. Call after the last `append`/`finish`.
    pub fn close_temp_file(&mut self) -> Result<(), String> {
        if let Some(mut f) = self.temp_file.take() {
            f.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The complete output, for callers that can take more than the display
    /// snapshot. Call after `finish()` and `close_temp_file()`. Output longer
    /// than `max_bytes` raw bytes keeps its first and last `max_bytes / 2`
    /// bytes around an omission marker.
    pub fn read_full_output(&self, max_bytes: u64) -> Result<FullOutput, String> {
        if self.temp_file_path.is_none() {
            let all: Vec<u8> = self.raw_chunks.iter().flatten().cloned().collect();
            return Ok(FullOutput { content: decode_oneshot(&all), truncated: false });
        }
        let path = self.temp_file_path.as_ref().unwrap();
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        if size <= max_bytes {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            return Ok(FullOutput { content: decode_oneshot(&buf), truncated: false });
        }
        let head_bytes = (max_bytes / 2) as usize;
        let tail_bytes = (max_bytes as usize) - head_bytes;
        let mut head = vec![0u8; head_bytes];
        let mut tail = vec![0u8; tail_bytes];
        {
            use std::io::Seek;
            file.read_exact(&mut head).map_err(|e| e.to_string())?;
            file.seek(std::io::SeekFrom::Start(size - tail_bytes as u64)).map_err(|e| e.to_string())?;
            file.read_exact(&mut tail).map_err(|e| e.to_string())?;
        }
        // Cut at character boundaries: streaming decode holds back an
        // incomplete trailing sequence, and the tail skips leading
        // continuation bytes.
        let head_text = decode_stream_holdback(&head);
        let mut tail_start = 0;
        while tail_start < tail.len() && (tail[tail_start] & 0xc0) == 0x80 {
            tail_start += 1;
        }
        let tail_text = decode_oneshot(&tail[tail_start..]);
        let omitted = size - head_bytes as u64 - tail_bytes as u64;
        Ok(FullOutput {
            content: format!("{}\n\n[... {} bytes omitted ...]\n\n{}", head_text, omitted, tail_text),
            truncated: true,
        })
    }

    pub fn get_last_line_bytes(&self) -> u64 {
        self.current_line_bytes
    }

    fn append_decoded_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let bytes = text.len() as u64;
        self.total_decoded_bytes += bytes;
        self.tail_text.push_str(text);
        self.tail_bytes += bytes;
        if self.tail_bytes > self.max_rolling_bytes * 2 {
            self.trim_tail();
        }

        let mut newlines = 0u64;
        let mut last_newline: Option<usize> = None;
        for (i, c) in text.char_indices() {
            if c == '\n' {
                newlines += 1;
                last_newline = Some(i);
            }
        }
        if newlines == 0 {
            self.current_line_bytes += bytes;
            self.has_open_line = true;
        } else {
            self.completed_lines += newlines;
            let tail = &text[last_newline.unwrap() + 1..];
            self.current_line_bytes = tail.len() as u64;
            self.has_open_line = !tail.is_empty();
        }
        self.total_lines = self.completed_lines + if self.has_open_line { 1 } else { 0 };
    }

    fn trim_tail(&mut self) {
        let buffer = self.tail_text.as_bytes();
        if buffer.len() as u64 <= self.max_rolling_bytes {
            self.tail_bytes = buffer.len() as u64;
            return;
        }

        let mut start = buffer.len() - self.max_rolling_bytes as usize;
        while start < buffer.len() && (buffer[start] & 0xc0) == 0x80 {
            start += 1;
        }

        self.tail_starts_at_line_boundary =
            if start == 0 { self.tail_starts_at_line_boundary } else { buffer[start - 1] == 0x0a };
        self.tail_text = String::from_utf8_lossy(&buffer[start..]).into_owned();
        self.tail_bytes = self.tail_text.len() as u64;
    }

    fn get_snapshot_text(&self) -> String {
        if self.tail_starts_at_line_boundary {
            self.tail_text.clone()
        } else {
            match self.tail_text.find('\n') {
                None => self.tail_text.clone(),
                Some(i) => self.tail_text[i + 1..].to_string(),
            }
        }
    }

    fn should_use_temp_file(&self) -> bool {
        self.total_raw_bytes > self.max_bytes || self.total_decoded_bytes > self.max_bytes || self.total_lines > self.max_lines
    }

    fn ensure_temp_file(&mut self) -> Result<(), String> {
        if self.temp_file_path.is_some() {
            return Ok(());
        }
        let (path, mut file) =
            create_output_file_stream(&self.temp_file_prefix, ".log").map_err(|e| e.to_string())?;
        for chunk in &self.raw_chunks {
            file.write_all(chunk).map_err(|e| e.to_string())?;
        }
        self.raw_chunks.clear();
        self.temp_file_path = Some(path);
        self.temp_file = Some(file);
        Ok(())
    }
}
