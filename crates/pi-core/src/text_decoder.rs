//! Streaming UTF-8 decoder with WHATWG `TextDecoder` semantics.
//!
//! Port of the `TextDecoder` usage in the original (`new TextDecoder()` with
//! default options, `{ stream: true }` across chunks):
//! - invalid sequences are replaced with U+FFFD following the WHATWG
//!   UTF-8 decoder algorithm (maximal-subpart behavior, byte reprocessing),
//! - an incomplete trailing sequence is held back until more data arrives
//!   (stream mode) or flushed as U+FFFD at end of stream,
//! - a leading U+FEFF BOM is stripped at the start of the stream (default
//!   `ignoreBOM: false`), even when split across chunks.

#[derive(Debug)]
struct State {
    /// Bytes still needed to complete the current sequence.
    need: u8,
    /// Continuation bytes seen so far.
    seen: u8,
    /// Valid range (lower..=upper) for the next continuation byte.
    lower: u8,
    upper: u8,
    /// Code point accumulated so far.
    cp: u32,
}

impl State {
    fn new() -> Self {
        State { need: 0, seen: 0, lower: 0, upper: 0, cp: 0 }
    }
}

#[derive(Debug)]
pub struct StreamingDecoder {
    state: State,
    /// Whether the start-of-stream BOM check is resolved.
    bom_resolved: bool,
    /// Buffered bytes at stream start that may still turn out to be a BOM.
    head: Vec<u8>,
}

impl Default for StreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

impl StreamingDecoder {
    pub fn new() -> Self {
        StreamingDecoder { state: State::new(), bom_resolved: false, head: Vec::new() }
    }

    /// `decoder.decode(data, { stream: true })`: decode a chunk, holding back
    /// an incomplete trailing sequence.
    pub fn decode(&mut self, data: &[u8]) -> String {
        let mut out = String::new();
        self.feed(data, &mut out);
        out
    }

    /// `decoder.decode()`: flush any pending incomplete sequence (as U+FFFD).
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        // Resolve a still-buffered BOM prefix through the state machine.
        if !self.bom_resolved {
            let head = std::mem::take(&mut self.head);
            self.bom_resolved = true;
            self.feed(&head, &mut out);
        }
        if self.state.need > 0 {
            out.push('\u{FFFD}');
            self.state = State::new();
        }
        out
    }

    fn feed(&mut self, data: &[u8], out: &mut String) {
        if !self.bom_resolved {
            // Buffer bytes until we can decide whether the stream starts
            // with a BOM (up to 3 bytes).
            let mut consumed = 0;
            for &b in data {
                consumed += 1;
                self.head.push(b);
                if self.head.len() == 3 {
                    break;
                }
            }
            if self.head.len() == 3 {
                self.bom_resolved = true;
                if self.head == BOM {
                    // BOM: strip it and continue with the rest of the data.
                    self.head.clear();
                } else {
                    let head = std::mem::take(&mut self.head);
                    self.process(&head, out);
                }
                self.process(&data[consumed..], out);
                return;
            }
            // Fewer than 3 bytes buffered: either a proper BOM prefix (wait
            // for more data) or a mismatch (process as ordinary bytes).
            let still_bom = self.head.iter().zip(BOM.iter()).all(|(&a, &b)| a == b);
            if !still_bom {
                let head = std::mem::take(&mut self.head);
                self.bom_resolved = true;
                self.process(&head, out);
            }
            return;
        }
        self.process(data, out);
    }

    fn process(&mut self, data: &[u8], out: &mut String) {
        let mut i = 0;
        while i < data.len() {
            if !self.step(data[i], out) {
                i += 1;
            }
        }
    }

    /// Process one byte. Returns `false` when the byte was consumed, `true`
    /// when it was NOT consumed (error reprocessing) and should be retried.
    fn step(&mut self, byte: u8, out: &mut String) -> bool {
        if self.state.need == 0 {
            if byte <= 0x7F {
                out.push(byte as char);
            } else if (0xC2..=0xDF).contains(&byte) {
                self.state.need = 1;
                self.state.lower = 0x80;
                self.state.upper = 0xBF;
                self.state.cp = (byte & 0x1F) as u32;
            } else if byte == 0xE0 {
                self.state.need = 2;
                self.state.lower = 0xA0;
                self.state.upper = 0xBF;
                self.state.cp = (byte & 0x0F) as u32;
            } else if (0xE1..=0xEC).contains(&byte) || (0xEE..=0xEF).contains(&byte) {
                self.state.need = 2;
                self.state.lower = 0x80;
                self.state.upper = 0xBF;
                self.state.cp = (byte & 0x0F) as u32;
            } else if byte == 0xED {
                self.state.need = 2;
                self.state.lower = 0x80;
                self.state.upper = 0x9F;
                self.state.cp = (byte & 0x0F) as u32;
            } else if byte == 0xF0 {
                self.state.need = 3;
                self.state.lower = 0x90;
                self.state.upper = 0xBF;
                self.state.cp = (byte & 0x07) as u32;
            } else if (0xF1..=0xF3).contains(&byte) {
                self.state.need = 3;
                self.state.lower = 0x80;
                self.state.upper = 0xBF;
                self.state.cp = (byte & 0x07) as u32;
            } else if byte == 0xF4 {
                self.state.need = 3;
                self.state.lower = 0x80;
                self.state.upper = 0x8F;
                self.state.cp = (byte & 0x07) as u32;
            } else {
                // 0x80..=0xC1, 0xF5..=0xFF: invalid leading byte.
                out.push('\u{FFFD}');
            }
            return false;
        }
        if self.state.lower <= byte && byte <= self.state.upper {
            self.state.lower = 0x80;
            self.state.upper = 0xBF;
            self.state.cp = (self.state.cp << 6) | (byte & 0x3F) as u32;
            self.state.seen += 1;
            if self.state.seen == self.state.need {
                if let Some(c) = char::from_u32(self.state.cp) {
                    out.push(c);
                }
                self.state = State::new();
            }
            false
        } else {
            // Invalid continuation: emit U+FFFD and reprocess the byte.
            out.push('\u{FFFD}');
            self.state = State::new();
            true
        }
    }
}

/// One-shot `new TextDecoder().decode(bytes)`: BOM strip + replacement chars.
pub fn decode_oneshot(bytes: &[u8]) -> String {
    let mut decoder = StreamingDecoder::new();
    let mut out = decoder.decode(bytes);
    out.push_str(&decoder.finish());
    out
}

/// `Buffer.toString("utf8")`: like `decode_oneshot` but a leading BOM is
/// PRESERVED (Node's Buffer decoder does not strip it).
pub fn decode_utf8_node(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let mut out = String::from("\u{FEFF}");
        out.push_str(&decode_oneshot(&bytes[3..]));
        out
    } else {
        decode_oneshot(bytes)
    }
}

/// `new TextDecoder().decode(head, { stream: true })` for a byte slice whose
/// trailing incomplete sequence is dropped (nothing follows it): used by
/// `readFullOutput` head decoding.
pub fn decode_stream_holdback(bytes: &[u8]) -> String {
    StreamingDecoder::new().decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> Vec<u8> {
        bytes.to_vec()
    }

    #[test]
    fn oneshot_ground_truth() {
        // Ground truth generated with Node's TextDecoder (see /tmp scripts).
        assert_eq!(decode_oneshot(&hex(&[0xF0, 0x9F, 0x98])), "\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0xE1, 0x80, 0x41])), "\u{FFFD}A");
        assert_eq!(decode_oneshot(&hex(&[0xE0, 0x80])), "\u{FFFD}\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0xC0, 0xAF])), "\u{FFFD}\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0xF5, 0x80, 0x80, 0x80])), "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0xED, 0xA0, 0x80])), "\u{FFFD}\u{FFFD}\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0xFF])), "\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0xC3])), "\u{FFFD}");
        assert_eq!(decode_oneshot(&hex(&[0x61, 0xF0, 0x9F, 0x98, 0x85, 0x62])), "a\u{1F605}b");
        // BOM stripped at start, kept mid-stream.
        assert_eq!(decode_oneshot(&hex(&[0xEF, 0xBB, 0xBF, 0x61])), "a");
        assert_eq!(decode_oneshot(&hex(&[0x61, 0xEF, 0xBB, 0xBF, 0x62])), "a\u{FEFF}b");
        // EF BB followed by non-BF: maximal subpart then reprocessed byte.
        assert_eq!(decode_oneshot(&hex(&[0xEF, 0xBB, 0x42])), "\u{FFFD}B");
        assert_eq!(decode_oneshot(&hex(&[0xEF, 0x41])), "\u{FFFD}A");
        assert_eq!(decode_oneshot(&hex(&[0xEF, 0xBB])), "\u{FFFD}");
    }

    #[test]
    fn streaming_holdback_and_flush() {
        let mut d = StreamingDecoder::new();
        assert_eq!(d.decode(&hex(&[0x61, 0xC3])), "a");
        assert_eq!(d.decode(&hex(&[0xA9, 0x62])), "\u{E9}b");
        assert_eq!(d.finish(), "");

        let mut d = StreamingDecoder::new();
        assert_eq!(d.decode(&hex(&[0xC3])), "");
        assert_eq!(d.finish(), "\u{FFFD}");

        // BOM split across chunks.
        let mut d = StreamingDecoder::new();
        assert_eq!(d.decode(&hex(&[0xEF, 0xBB])), "");
        assert_eq!(d.decode(&hex(&[0xBF, 0x78])), "x");
        assert_eq!(d.finish(), "");

        // Error byte then held sequence.
        let mut d = StreamingDecoder::new();
        assert_eq!(d.decode(&hex(&[0xFF, 0xC3])), "\u{FFFD}");
        assert_eq!(d.decode(&hex(&[0xA9])), "\u{E9}");
        assert_eq!(d.finish(), "");
    }

    #[test]
    fn holdback_helper_drops_incomplete_tail() {
        assert_eq!(decode_stream_holdback(&hex(&[0x61, 0xC3])), "a");
        assert_eq!(decode_stream_holdback(&hex(&[0xEF, 0xBB])), "");
    }
}
