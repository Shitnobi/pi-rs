//! StdinBuffer buffers input and emits complete sequences.
//!
//! Rust port of `packages/tui/src/stdin-buffer.ts` (1:1 behavior).
//!
//! This is necessary because stdin data events can arrive in partial chunks,
//! especially for escape sequences like mouse events. Without buffering,
//! partial sequences can be misinterpreted as regular keypresses.
//!
//! For example, the mouse SGR sequence `\x1b[<35;20;5m` might arrive as:
//! - Event 1: `\x1b`
//! - Event 2: `[<35`
//! - Event 3: `;20;5m`
//!
//! The buffer accumulates these until a complete sequence is detected.
//! Call the [`StdinBuffer::process`] method to feed input data.
//!
//! ## WTF-16 semantics
//!
//! The TS original operates on JS strings (UTF-16 code units): plain text is
//! split into *single code units*, so an astral character (e.g. an emoji)
//! arrives as two separate `data` events, one per surrogate half. This port
//! keeps that behavior by working on `Vec<u16>` internally; events carry raw
//! UTF-16 units. Use [`units_to_string`] when the content is known to be
//! valid UTF-16.
//!
//! ## Timeouts
//!
//! The TS original schedules a `setTimeout` when input remains buffered; this
//! port records the pending timeout kind ([`PendingTimeout`]) instead. The
//! runtime owner calls [`StdinBuffer::fire_timeout`] when the deadline
//! expires (tests call it directly, mirroring the mocked-timer upstream
//! tests).
//!
//! Based on code from OpenTUI (https://github.com/anomalyco/opentui)
//! MIT License - Copyright (c) 2025 opentui

const ESC: u16 = 0x1b;
const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 50;
const DEFAULT_ESCAPE_TIMEOUT_MS: u64 = 10;
const BRACKETED_PASTE_START: &[u16] = &[0x1b, b'[' as u16, b'2' as u16, b'0' as u16, b'0' as u16, b'~' as u16];
const BRACKETED_PASTE_END: &[u16] = &[0x1b, b'[' as u16, b'2' as u16, b'0' as u16, b'1' as u16, b'~' as u16];

/// A single buffered input event, in UTF-16 code units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinBufferEvent {
    /// A complete input sequence (`data` event in the TS original).
    Data(Vec<u16>),
    /// Bracketed paste content (`paste` event in the TS original).
    Paste(Vec<u16>),
}

/// Which timeout the TS original would have scheduled for buffered input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTimeout {
    /// A lone ESC waiting to be dispatched as the Escape key
    /// (`escapeTimeout`, default 10 ms).
    Escape,
    /// An incomplete keyboard/mouse sequence (`timeout`, default 50 ms).
    Sequence,
}

/// Lossy conversion of UTF-16 units into a Rust string
/// (lone surrogates become U+FFFD).
pub fn units_to_string(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn is_control_string(data: &[u16]) -> bool {
    if data.first() != Some(&ESC) {
        return false;
    }
    matches!(data.get(1), Some(v) if *v == b']' as u16 || *v == b'P' as u16 || *v == b'_' as u16)
}

fn is_digit_unit(u: u16) -> bool {
    (b'0' as u16..=b'9' as u16).contains(&u)
}

/// `/^\x1b(?:\](?:1[01];|4;\d{1,3};|7501;\?)|P>\||_G(?:[a-z]=\d+,)*[a-z]=\d+;)/`
///
/// Color and program-status OSC replies, XTVersion, and Kitty graphics
/// replies. Until their header is complete, the input may be batched legacy
/// Alt keystrokes.
fn terminal_reply_header_pattern_test(data: &[u16]) -> bool {
    if data.first() != Some(&ESC) {
        return false;
    }
    let rest = &data[1..];
    match rest.first() {
        Some(c) if *c == b']' as u16 => {
            let osc = &rest[1..];
            // 1[01]; → "10;" or "11;"
            if osc.len() >= 3 && osc[0] == b'1' as u16 && (osc[1] == b'0' as u16 || osc[1] == b'1' as u16) && osc[2] == b';' as u16 {
                return true;
            }
            // 4;\d{1,3};
            if osc.first() == Some(&(b'4' as u16)) && osc.get(1) == Some(&(b';' as u16)) {
                let mut i = 2;
                let digits_start = i;
                while i < osc.len() && is_digit_unit(osc[i]) && i - digits_start < 3 {
                    i += 1;
                }
                let digit_count = i - digits_start;
                if (1..=3).contains(&digit_count) && osc.get(i) == Some(&(b';' as u16)) {
                    return true;
                }
            }
            // 7501;?
            if osc.len() >= 6
                && osc[..5].iter().copied().eq("7501;".encode_utf16())
                && osc[5] == b'?' as u16
            {
                return true;
            }
            false
        }
        Some(c) if *c == b'P' as u16 => rest.get(1) == Some(&(b'>' as u16)) && rest.get(2) == Some(&(b'|' as u16)),
        Some(c) if *c == b'_' as u16 => {
            let apc = &rest[1..];
            if apc.first() != Some(&(b'G' as u16)) {
                return false;
            }
            // (?:[a-z]=\d+,)*[a-z]=\d+;
            let mut i = 1;
            loop {
                // one [a-z]=\d+ group
                let group_start = i;
                if !matches!(apc.get(i), Some(c) if (b'a' as u16..=b'z' as u16).contains(c)) {
                    return false;
                }
                i += 1;
                if apc.get(i) != Some(&(b'=' as u16)) {
                    return false;
                }
                i += 1;
                let digits_start = i;
                while i < apc.len() && is_digit_unit(apc[i]) {
                    i += 1;
                }
                if i == digits_start {
                    return false;
                }
                match apc.get(i) {
                    Some(&c) if c == b',' as u16 => {
                        i += 1;
                        continue;
                    }
                    Some(&c) if c == b';' as u16 => return true,
                    _ => {
                        let _ = group_start;
                        return false;
                    }
                }
            }
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqStatus {
    Complete,
    Incomplete,
    NotEscape,
}

/// Check if a string is a complete escape sequence or needs more data.
fn is_complete_sequence(data: &[u16]) -> SeqStatus {
    if data.first() != Some(&ESC) {
        return SeqStatus::NotEscape;
    }

    if data.len() == 1 {
        return SeqStatus::Incomplete;
    }

    let after_esc = &data[1..];

    // CSI sequences: ESC [
    if after_esc.first() == Some(&(b'[' as u16)) {
        // Check for old-style mouse sequence: ESC[M + 3 bytes
        if after_esc.get(1) == Some(&(b'M' as u16)) {
            // Old-style mouse needs ESC[M + 3 bytes = 6 total
            return if data.len() >= 6 { SeqStatus::Complete } else { SeqStatus::Incomplete };
        }
        return is_complete_csi_sequence(data);
    }

    // OSC sequences: ESC ]
    if after_esc.first() == Some(&(b']' as u16)) {
        return is_complete_osc_sequence(data);
    }

    // DCS sequences: ESC P ... ESC \ (includes XTVersion responses)
    if after_esc.first() == Some(&(b'P' as u16)) {
        return is_complete_dcs_sequence(data);
    }

    // APC sequences: ESC _ ... ESC \ (includes Kitty graphics responses)
    if after_esc.first() == Some(&(b'_' as u16)) {
        return is_complete_apc_sequence(data);
    }

    // SS3 sequences: ESC O
    if after_esc.first() == Some(&(b'O' as u16)) {
        // ESC O followed by a single character
        return if after_esc.len() >= 2 { SeqStatus::Complete } else { SeqStatus::Incomplete };
    }

    // Meta key sequences: ESC followed by a single character
    if after_esc.len() == 1 {
        return SeqStatus::Complete;
    }

    // Unknown escape sequence - treat as complete
    SeqStatus::Complete
}

/// CSI sequences: ESC [ ... followed by a final byte (0x40-0x7E)
fn is_complete_csi_sequence(data: &[u16]) -> SeqStatus {
    if !(data.len() >= 2 && data[0] == ESC && data[1] == b'[' as u16) {
        return SeqStatus::Complete;
    }

    // Need at least ESC [ and one more character
    if data.len() < 3 {
        return SeqStatus::Incomplete;
    }

    let payload = &data[2..];

    // CSI sequences end with a byte in the range 0x40-0x7E (@-~)
    let last_char = payload[payload.len() - 1];
    let last_char_code = last_char;

    if (0x40..=0x7e).contains(&last_char_code) {
        // Special handling for SGR mouse sequences
        // Format: ESC[<B;X;Ym or ESC[<B;X;YM
        if payload.first() == Some(&(b'<' as u16)) {
            // Must have format: <digits;digits;digits[Mm]
            if sgr_mouse_match(payload) {
                return SeqStatus::Complete;
            }
            // If it ends with M or m but doesn't match the pattern, still incomplete
            if last_char == b'M' as u16 || last_char == b'm' as u16 {
                // Check if we have the right structure
                let inner = &payload[1..payload.len() - 1];
                let parts: Vec<&[u16]> = split_units(inner, b';' as u16);
                if parts.len() == 3 && parts.iter().all(|p| all_digits(p)) {
                    return SeqStatus::Complete;
                }
            }

            return SeqStatus::Incomplete;
        }

        return SeqStatus::Complete;
    }

    SeqStatus::Incomplete
}

/// `/^<\d+;\d+;\d+[Mm]$/`
fn sgr_mouse_match(payload: &[u16]) -> bool {
    let mut i = 0usize;
    if payload.get(i) != Some(&(b'<' as u16)) {
        return false;
    }
    i += 1;
    if !consume_digits(payload, &mut i) {
        return false;
    }
    for _ in 0..2 {
        if payload.get(i) != Some(&(b';' as u16)) {
            return false;
        }
        i += 1;
        if !consume_digits(payload, &mut i) {
            return false;
        }
    }
    matches!(payload.get(i), Some(c) if *c == b'M' as u16 || *c == b'm' as u16) && i + 1 == payload.len()
}

fn consume_digits(data: &[u16], i: &mut usize) -> bool {
    let start = *i;
    while *i < data.len() && is_digit_unit(data[*i]) {
        *i += 1;
    }
    *i > start
}

fn all_digits(part: &[u16]) -> bool {
    // /^\d+$/ — empty does not match
    !part.is_empty() && part.iter().all(|c| is_digit_unit(*c))
}

fn split_units(data: &[u16], sep: u16) -> Vec<&[u16]> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    for (i, &c) in data.iter().enumerate() {
        if c == sep {
            parts.push(&data[start..i]);
            start = i + 1;
        }
    }
    parts.push(&data[start..]);
    parts
}

/// OSC sequences end with ST (ESC \) or BEL (\x07)
fn is_complete_osc_sequence(data: &[u16]) -> SeqStatus {
    if !(data.len() >= 2 && data[0] == ESC && data[1] == b']' as u16) {
        return SeqStatus::Complete;
    }
    if ends_with_st(data) || data.last() == Some(&0x07) {
        return SeqStatus::Complete;
    }
    SeqStatus::Incomplete
}

/// DCS sequences end with ST (ESC \)
fn is_complete_dcs_sequence(data: &[u16]) -> SeqStatus {
    if !(data.len() >= 2 && data[0] == ESC && data[1] == b'P' as u16) {
        return SeqStatus::Complete;
    }
    if ends_with_st(data) {
        return SeqStatus::Complete;
    }
    SeqStatus::Incomplete
}

/// APC sequences end with ST (ESC \)
fn is_complete_apc_sequence(data: &[u16]) -> SeqStatus {
    if !(data.len() >= 2 && data[0] == ESC && data[1] == b'_' as u16) {
        return SeqStatus::Complete;
    }
    if ends_with_st(data) {
        return SeqStatus::Complete;
    }
    SeqStatus::Incomplete
}

fn ends_with_st(data: &[u16]) -> bool {
    data.len() >= 2 && data[data.len() - 2] == ESC && data[data.len() - 1] == b'\\' as u16
}

fn find_subunit(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// `/^\x1b\[(\d+)(?::\d*)?(?::\d+)?u$/` — codepoint when ≥ 32.
fn parse_unmodified_kitty_printable_codepoint(sequence: &[u16]) -> Option<i64> {
    if sequence.len() < 4 || sequence[0] != ESC || sequence[1] != b'[' as u16 {
        return None;
    }
    if *sequence.last().expect("len checked") != b'u' as u16 {
        return None;
    }
    let mut i = 2usize;
    if !consume_digits(sequence, &mut i) {
        return None;
    }
    // (?::\d*)? — optional ':' + zero or more digits
    if sequence.get(i) == Some(&(b':' as u16)) {
        i += 1;
        while i < sequence.len() && is_digit_unit(sequence[i]) {
            i += 1;
        }
    }
    // (?::(\d+))? — optional ':' + one or more digits
    if sequence.get(i) == Some(&(b':' as u16)) {
        let save = i;
        i += 1;
        if !consume_digits(sequence, &mut i) {
            i = save;
        }
    }
    if i != sequence.len() - 1 {
        return None;
    }
    // Parse the leading digit run (it is at most ~6 digits for real inputs;
    // saturate beyond u64 like the JS double would keep growing).
    let mut value: u64 = 0;
    let mut j = 2;
    while j < sequence.len() && is_digit_unit(sequence[j]) {
        value = value.saturating_mul(10).saturating_add((sequence[j] - b'0' as u16) as u64);
        j += 1;
    }
    let codepoint = value as i64;
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

/// Split accumulated buffer into complete sequences.
fn extract_complete_sequences(buffer: &[u16]) -> (Vec<Vec<u16>>, Vec<u16>) {
    let mut sequences: Vec<Vec<u16>> = Vec::new();
    let mut pos = 0usize;

    while pos < buffer.len() {
        let remaining = &buffer[pos..];

        // Try to extract a sequence starting at this position
        if remaining[0] == ESC {
            let terminal_reply = is_control_string(remaining) && terminal_reply_header_pattern_test(remaining);
            // Find the end of this escape sequence
            let mut seq_end = 1usize;
            let mut exhausted = true;
            while seq_end <= remaining.len() {
                if terminal_reply && seq_end > 2 {
                    let char = remaining[seq_end - 1];
                    if char == 0x18 || char == 0x1a {
                        // CAN and SUB cancel the string, not subsequent keyboard input.
                        pos += seq_end;
                        exhausted = false;
                        break;
                    }
                    if char == ESC && seq_end < remaining.len() && remaining[seq_end] != b'\\' as u16 {
                        // A new escape sequence aborts an unterminated reply. Keep a
                        // trailing ESC buffered until we can distinguish it from ST.
                        pos += seq_end - 1;
                        exhausted = false;
                        break;
                    }
                }
                let candidate = &remaining[..seq_end];

                match is_complete_sequence(candidate) {
                    SeqStatus::Complete => {
                        // WezTerm with enable_kitty_keyboard sends the Escape key press as a
                        // raw '\x1b' byte (simple text path in encode_kitty, ignoring
                        // DISAMBIGUATE_ESCAPE_CODES) and the release as a full Kitty CSI-u
                        // sequence. These arrive concatenated as '\x1b\x1b[27;...u'.
                        // The buffer would normally treat '\x1b\x1b' as a complete meta-key
                        // sequence (ESC + single char), leaving '[27;...u' to be typed as
                        // plain text. If the character immediately following '\x1b\x1b'
                        // would begin a new escape sequence, emit only the first ESC and
                        // restart from the second.
                        if candidate.len() == 2 && candidate == [ESC, ESC] {
                            if let Some(&next_char) = remaining.get(seq_end) {
                                if next_char == b'[' as u16
                                    || next_char == b']' as u16
                                    || next_char == b'O' as u16
                                    || next_char == b'P' as u16
                                    || next_char == b'_' as u16
                                {
                                    sequences.push(vec![ESC]);
                                    pos += 1;
                                    exhausted = false;
                                    break;
                                }
                            }
                        }
                        sequences.push(candidate.to_vec());
                        pos += seq_end;
                        exhausted = false;
                        break;
                    }
                    SeqStatus::Incomplete => {
                        seq_end += 1;
                    }
                    SeqStatus::NotEscape => {
                        // Should not happen when starting with ESC
                        sequences.push(candidate.to_vec());
                        pos += seq_end;
                        exhausted = false;
                        break;
                    }
                }
            }

            if exhausted {
                return (sequences, remaining.to_vec());
            }
        } else {
            // Not an escape sequence - take a single character
            // (a single UTF-16 code unit, exactly like the TS original)
            sequences.push(vec![remaining[0]]);
            pos += 1;
        }
    }

    (sequences, Vec::new())
}

/// Options for [`StdinBuffer`].
#[derive(Debug, Clone, Copy)]
pub struct StdinBufferOptions {
    /// Maximum time to wait for an incomplete keyboard or mouse sequence
    /// (default: 50ms). Strings with a recognized terminal reply header wait
    /// for their terminator instead, so delayed replies cannot become
    /// keypresses. Ambiguous legacy Alt-key input still uses this timeout.
    pub timeout_ms: u64,
    /// Maximum time to wait after a lone ESC before treating it as Escape
    /// (default: 10ms). Increase for high-latency Alt+key input (SSH).
    pub escape_timeout_ms: u64,
}

impl Default for StdinBufferOptions {
    fn default() -> Self {
        StdinBufferOptions {
            timeout_ms: DEFAULT_SEQUENCE_TIMEOUT_MS,
            escape_timeout_ms: DEFAULT_ESCAPE_TIMEOUT_MS,
        }
    }
}

/// Buffers stdin input and emits complete sequences via `data`/`paste`
/// events. Handles partial escape sequences that arrive across multiple
/// chunks.
///
/// Events accumulate in order and are drained with [`StdinBuffer::take_events`].
#[derive(Debug)]
pub struct StdinBuffer {
    buffer: Vec<u16>,
    events: Vec<StdinBufferEvent>,
    timeout_ms: u64,
    escape_timeout_ms: u64,
    paste_mode: bool,
    paste_buffer: Vec<u16>,
    pending_kitty_printable_codepoint: Option<i64>,
    pending_timeout: Option<PendingTimeout>,
}

impl Default for StdinBuffer {
    fn default() -> Self {
        StdinBuffer::new(StdinBufferOptions::default())
    }
}

impl StdinBuffer {
    pub fn new(options: StdinBufferOptions) -> Self {
        StdinBuffer {
            buffer: Vec::new(),
            events: Vec::new(),
            timeout_ms: options.timeout_ms,
            escape_timeout_ms: options.escape_timeout_ms,
            paste_mode: false,
            paste_buffer: Vec::new(),
            pending_kitty_printable_codepoint: None,
            pending_timeout: None,
        }
    }

    /// The timeout kind currently pending, if any (the TS original's
    /// scheduled `setTimeout`).
    pub fn pending_timeout(&self) -> Option<PendingTimeout> {
        self.pending_timeout
    }

    /// The configured timeout in milliseconds for the pending timeout kind.
    pub fn pending_timeout_ms(&self, kind: PendingTimeout) -> u64 {
        match kind {
            PendingTimeout::Escape => self.escape_timeout_ms,
            PendingTimeout::Sequence => self.timeout_ms,
        }
    }

    /// Drain all buffered events, in emission order.
    pub fn take_events(&mut self) -> Vec<StdinBufferEvent> {
        std::mem::take(&mut self.events)
    }

    /// Feed a string as input (converted to UTF-16 code units).
    pub fn process(&mut self, data: &str) {
        let units: Vec<u16> = data.encode_utf16().collect();
        self.process_units(&units);
    }

    /// Feed raw bytes, mirroring the TS `Buffer` input path: a single byte
    /// above 127 becomes `ESC + (byte - 128)`; anything else is decoded as
    /// UTF-8 with replacement characters for invalid sequences.
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        if bytes.len() == 1 && bytes[0] > 127 {
            let units = vec![ESC, (bytes[0] - 128) as u16];
            self.process_units(&units);
        } else {
            let s = String::from_utf8_lossy(bytes);
            self.process(&s);
        }
    }

    /// Feed UTF-16 code units (the TS string input path).
    pub fn process_units(&mut self, str: &[u16]) {
        // Clear any pending timeout
        self.pending_timeout = None;

        if str.is_empty() && self.buffer.is_empty() {
            self.emit_data_sequence(&[]);
            return;
        }

        self.buffer.extend_from_slice(str);

        if self.paste_mode {
            self.paste_buffer.append(&mut self.buffer);

            if let Some(end_index) = find_subunit(&self.paste_buffer, BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_index].to_vec();
                let remaining = self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_vec();

                self.paste_mode = false;
                self.paste_buffer = Vec::new();
                self.pending_kitty_printable_codepoint = None;

                self.events.push(StdinBufferEvent::Paste(pasted_content));

                if !remaining.is_empty() {
                    self.process_units(&remaining);
                }
            }
            return;
        }

        if let Some(start_index) = find_subunit(&self.buffer, BRACKETED_PASTE_START) {
            if start_index > 0 {
                let before_paste = self.buffer[..start_index].to_vec();
                let (sequences, _discarded_remainder) = extract_complete_sequences(&before_paste);
                // The TS original drops `result.remainder` here; a partial
                // sequence directly before a paste start is lost.
                for sequence in sequences {
                    self.emit_data_sequence(&sequence);
                }
            }

            self.pending_kitty_printable_codepoint = None;
            self.buffer = self.buffer[start_index + BRACKETED_PASTE_START.len()..].to_vec();
            self.paste_mode = true;
            self.paste_buffer = std::mem::take(&mut self.buffer);

            if let Some(end_index) = find_subunit(&self.paste_buffer, BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_index].to_vec();
                let remaining = self.paste_buffer[end_index + BRACKETED_PASTE_END.len()..].to_vec();

                self.paste_mode = false;
                self.paste_buffer = Vec::new();
                self.pending_kitty_printable_codepoint = None;

                self.events.push(StdinBufferEvent::Paste(pasted_content));

                if !remaining.is_empty() {
                    self.process_units(&remaining);
                }
            }
            return;
        }

        let (sequences, remainder) = extract_complete_sequences(&self.buffer);
        self.buffer = remainder;

        for sequence in sequences {
            self.emit_data_sequence(&sequence);
        }

        if !self.buffer.is_empty() {
            // Only a recognizable reply header establishes a delimited string.
            // Flushing it would turn a delayed tail into text and an OSC's BEL
            // terminator into Ctrl+G (the external editor shortcut). Other input,
            // including Alt+] followed by text, must retain bounded recovery.
            if terminal_reply_header_pattern_test(&self.buffer) {
                return;
            }
            self.pending_timeout = if self.buffer.len() == 1 && self.buffer[0] == ESC {
                Some(PendingTimeout::Escape)
            } else {
                Some(PendingTimeout::Sequence)
            };
        }
    }

    /// Execute the body of the TS `setTimeout` callback (the pending
    /// timeout's deadline has been reached). No-op when no timeout is
    /// pending.
    pub fn fire_timeout(&mut self) {
        if self.pending_timeout.is_none() {
            return;
        }
        self.pending_timeout = None;

        let flushed = self.flush();

        for sequence in flushed {
            if sequence.len() > 2 && is_control_string(&sequence) {
                // Recover the Alt key and reparse its tail so batched text,
                // Enter, and control keys are not swallowed as one event.
                let (head, tail) = sequence.split_at(2);
                let head = head.to_vec();
                self.emit_data_sequence(&head);
                self.process_units(tail);
            } else {
                self.emit_data_sequence(&sequence);
            }
        }
    }

    fn emit_data_sequence(&mut self, sequence: &[u16]) {
        let raw_codepoint = if sequence.len() == 1 { Some(sequence[0] as i64) } else { None };
        if raw_codepoint.is_some() && raw_codepoint == self.pending_kitty_printable_codepoint {
            self.pending_kitty_printable_codepoint = None;
            return;
        }

        self.pending_kitty_printable_codepoint = parse_unmodified_kitty_printable_codepoint(sequence);
        self.events.push(StdinBufferEvent::Data(sequence.to_vec()));
    }

    /// Flush the buffer as a single sequence (the TS `flush()`); clears the
    /// pending timeout and the Kitty printable dedup state.
    pub fn flush(&mut self) -> Vec<Vec<u16>> {
        self.pending_timeout = None;

        if self.buffer.is_empty() {
            return Vec::new();
        }

        let sequences = vec![std::mem::take(&mut self.buffer)];
        self.pending_kitty_printable_codepoint = None;
        sequences
    }

    /// Clear buffered content and pending state without emitting.
    pub fn clear(&mut self) {
        self.pending_timeout = None;
        self.buffer = Vec::new();
        self.paste_mode = false;
        self.paste_buffer = Vec::new();
        self.pending_kitty_printable_codepoint = None;
    }

    /// The raw buffered units (UTF-16), like the TS `getBuffer()`.
    pub fn buffer(&self) -> &[u16] {
        &self.buffer
    }

    /// The buffered units as a (lossy) string.
    pub fn get_buffer(&self) -> String {
        units_to_string(&self.buffer)
    }

    /// Alias for [`StdinBuffer::clear`] (TS `destroy()`).
    pub fn destroy(&mut self) {
        self.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests — port of packages/tui/test/stdin-buffer.test.ts
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::matches_key;

    /// The upstream `beforeEach` constructs `new StdinBuffer({ timeout: 10 })`.
    const DEFAULT_TEST_TIMEOUT_MS: u64 = 10;

    /// Encode a string as UTF-16 code units (the TS string representation).
    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Mirrors the upstream harness: a `StdinBuffer` plus collectors for the
    /// `data` and `paste` events (`buffer.on("data", ...)` etc.).
    struct Harness {
        buffer: StdinBuffer,
        sequences: Vec<Vec<u16>>,
        pastes: Vec<Vec<u16>>,
    }

    impl Harness {
        /// `new StdinBuffer({ timeout: 10 })` — the `beforeEach` buffer.
        fn standard() -> Self {
            Self::with_options(StdinBufferOptions {
                timeout_ms: DEFAULT_TEST_TIMEOUT_MS,
                escape_timeout_ms: DEFAULT_ESCAPE_TIMEOUT_MS,
            })
        }

        /// `new StdinBuffer()` — default 50ms sequence / 10ms escape timeouts.
        fn with_defaults() -> Self {
            Self::with_options(StdinBufferOptions::default())
        }

        fn with_options(options: StdinBufferOptions) -> Self {
            Harness {
                buffer: StdinBuffer::new(options),
                sequences: Vec::new(),
                pastes: Vec::new(),
            }
        }

        fn process(&mut self, data: &str) {
            self.buffer.process(data);
            self.collect();
        }

        fn process_bytes(&mut self, data: &[u8]) {
            self.buffer.process_bytes(data);
            self.collect();
        }

        /// The mocked-timer `tick()` reaching a pending deadline (or an
        /// `await wait(ms)` past the timeout), then draining new events.
        fn fire_timeout(&mut self) {
            self.buffer.fire_timeout();
            self.collect();
        }

        fn collect(&mut self) {
            for event in self.buffer.take_events() {
                match event {
                    StdinBufferEvent::Data(sequence) => self.sequences.push(sequence),
                    StdinBufferEvent::Paste(data) => self.pastes.push(data),
                }
            }
        }

        /// Emitted `data` events as (lossy) strings, mirroring the upstream
        /// `emittedSequences` assertions.
        fn data(&self) -> Vec<String> {
            self.sequences.iter().map(|s| units_to_string(s)).collect()
        }

        fn assert_data(&self, expected: &[&str]) {
            assert_eq!(
                self.data(),
                expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "data events mismatch"
            );
        }

        fn assert_pastes(&self, expected: &[&str]) {
            let actual: Vec<String> = self.pastes.iter().map(|s| units_to_string(s)).collect();
            assert_eq!(
                actual,
                expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "paste events mismatch"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Regular Characters
    // -----------------------------------------------------------------------

    #[test]
    fn should_pass_through_regular_characters_immediately() {
        let mut h = Harness::standard();
        h.process("a");
        h.assert_data(&["a"]);
    }

    #[test]
    fn should_pass_through_multiple_regular_characters() {
        let mut h = Harness::standard();
        h.process("abc");
        h.assert_data(&["a", "b", "c"]);
    }

    #[test]
    fn should_handle_unicode_characters() {
        let mut h = Harness::standard();
        h.process("hello 世界");
        h.assert_data(&["h", "e", "l", "l", "o", " ", "世", "界"]);
    }

    // -----------------------------------------------------------------------
    // Complete Escape Sequences
    // -----------------------------------------------------------------------

    #[test]
    fn should_pass_through_complete_mouse_sgr_sequences() {
        let mut h = Harness::standard();
        h.process("\x1b[<35;20;5m");
        h.assert_data(&["\x1b[<35;20;5m"]);
    }

    #[test]
    fn should_pass_through_complete_arrow_key_sequences() {
        let mut h = Harness::standard();
        h.process("\x1b[A");
        h.assert_data(&["\x1b[A"]);
    }

    #[test]
    fn should_pass_through_complete_function_key_sequences() {
        let mut h = Harness::standard();
        h.process("\x1b[11~");
        h.assert_data(&["\x1b[11~"]);
    }

    #[test]
    fn should_pass_through_meta_key_sequences() {
        let mut h = Harness::standard();
        h.process("\x1ba");
        h.assert_data(&["\x1ba"]);
    }

    #[test]
    fn should_pass_through_ss3_sequences() {
        let mut h = Harness::standard();
        h.process("\x1bOA");
        h.assert_data(&["\x1bOA"]);
    }

    // -----------------------------------------------------------------------
    // Partial Escape Sequences
    // -----------------------------------------------------------------------

    #[test]
    fn should_buffer_incomplete_mouse_sgr_sequence() {
        let mut h = Harness::standard();
        h.process("\x1b");
        h.assert_data(&[]);
        assert_eq!(h.buffer.get_buffer(), "\x1b");

        h.process("[<35");
        h.assert_data(&[]);
        assert_eq!(h.buffer.get_buffer(), "\x1b[<35");

        h.process(";20;5m");
        h.assert_data(&["\x1b[<35;20;5m"]);
        assert_eq!(h.buffer.get_buffer(), "");
    }

    #[test]
    fn should_buffer_incomplete_csi_sequence() {
        let mut h = Harness::standard();
        h.process("\x1b[");
        h.assert_data(&[]);

        h.process("1;");
        h.assert_data(&[]);

        h.process("5H");
        h.assert_data(&["\x1b[1;5H"]);
    }

    #[test]
    fn should_buffer_split_across_many_chunks() {
        let mut h = Harness::standard();
        for chunk in ["\x1b", "[", "<", "3", "5", ";", "2", "0", ";", "5", "m"] {
            h.process(chunk);
        }
        h.assert_data(&["\x1b[<35;20;5m"]);
    }

    #[test]
    fn should_flush_incomplete_sequence_after_timeout() {
        let mut h = Harness::standard();
        h.process("\x1b[<35");
        h.assert_data(&[]);

        // await wait(15) — past the 10ms sequence timeout.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));
        h.fire_timeout();

        h.assert_data(&["\x1b[<35"]);
    }

    #[test]
    fn should_flush_a_lone_esc_as_escape_when_cr_arrives_after_the_timeout() {
        // matches_key reads the global kitty-protocol flag; serialize against
        // other tests that flip it (the TS suite runs sequentially).
        let _guard = crate::test_support::global_test_lock();

        // Legacy-mode Alt+Enter is ESC + CR; when the terminal/transport splits
        // the bytes further apart than the timeout, ESC is flushed alone and the
        // host sees Escape (interrupt) instead of Alt+Enter. This locks in the
        // behavior so the configurable timeout in ProcessTerminal stays honest.
        let mut h = Harness::standard();
        h.process("\x1b");
        // await wait(20) — past the 10ms escape timeout.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Escape));
        h.fire_timeout();
        h.process("\r");

        h.assert_data(&["\x1b", "\r"]);
        assert!(matches_key(&h.data()[0], "escape"));
    }

    #[test]
    fn should_merge_esc_cr_split_across_chunks_within_a_larger_timeout() {
        let _guard = crate::test_support::global_test_lock();

        let mut h = Harness::with_options(StdinBufferOptions {
            timeout_ms: 50,
            escape_timeout_ms: 100,
        });
        h.process("\x1b");
        // await wait(20): past the 10ms default escapeTimeout, well inside the
        // configured 100ms escapeTimeout — the timeout must NOT fire.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Escape));
        assert_eq!(h.buffer.pending_timeout_ms(PendingTimeout::Escape), 100);
        h.process("\r");

        h.assert_data(&["\x1b\r"]);
        assert!(matches_key(&h.data()[0], "alt+enter"));
    }

    #[test]
    fn does_not_apply_the_sequence_timeout_to_a_lone_esc() {
        let mut h = Harness::with_options(StdinBufferOptions {
            timeout_ms: 100,
            escape_timeout_ms: 10,
        });
        h.process("\x1b");
        // await wait(20): the lone ESC is governed by the 10ms escape timeout,
        // not the configured 100ms sequence timeout, so it has already fired.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Escape));
        assert_eq!(h.buffer.pending_timeout_ms(PendingTimeout::Escape), 10);
        h.fire_timeout();
        h.process("\r");

        h.assert_data(&["\x1b", "\r"]);
        assert!(matches_key(&h.data()[0], "escape"));
    }

    #[test]
    fn keeps_fragmented_mouse_sequences_buffered_across_delayed_chunks_by_default() {
        let mut h = Harness::with_defaults();
        h.process("\x1b[");
        // await wait(20) — inside the 50ms default sequence timeout.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));
        assert_eq!(h.buffer.pending_timeout_ms(PendingTimeout::Sequence), 50);
        h.process("<65;48;39M");
        h.assert_data(&["\x1b[<65;48;39M"]);
    }

    // -----------------------------------------------------------------------
    // Terminal Reply Strings
    // -----------------------------------------------------------------------

    /// Upstream: `keeps delayed ${reply} intact at every split after its
    /// reply header` (one generated test per reply string).
    fn keeps_delayed_reply_intact_at_every_split(header: &str, reply: &str) {
        for split in header.len()..reply.len() {
            let mut h = Harness::with_defaults();
            h.process(&reply[..split]);
            // t.mock.timers.tick(5000) — expire any pending timeout.
            h.fire_timeout();
            h.process(&format!("{}x", &reply[split..]));
            // The reply must arrive as a single event (a split-off BEL
            // terminator would surface as ctrl+g) followed by the "x".
            assert_eq!(h.data(), vec![reply.to_string(), "x".to_string()], "split at {split}");
        }
    }

    #[test]
    fn keeps_delayed_osc4_color_reply_intact_at_every_split_after_its_reply_header() {
        // Ghostty's Ayu Light palette color 1.
        keeps_delayed_reply_intact_at_every_split("\x1b]4;1;", "\x1b]4;1;rgb:eaea/6c6c/6d6d\x07");
    }

    #[test]
    fn keeps_delayed_osc10_foreground_reply_intact_at_every_split_after_its_reply_header() {
        keeps_delayed_reply_intact_at_every_split("\x1b]10;", "\x1b]10;rgb:ffff/ffff/ffff\x1b\\");
    }

    #[test]
    fn keeps_delayed_osc11_background_reply_intact_at_every_split_after_its_reply_header() {
        keeps_delayed_reply_intact_at_every_split("\x1b]11;", "\x1b]11;rgb:0000/0000/0000\x07");
    }

    #[test]
    fn keeps_delayed_osc4_color_reply_tail_intact_at_every_split_after_its_reply_header() {
        // Another reported color-reply tail.
        keeps_delayed_reply_intact_at_every_split("\x1b]4;1;", "\x1b]4;1;rgb:6464/6464/4d4d\x07");
    }

    #[test]
    fn keeps_delayed_dcs_xtversion_reply_intact_at_every_split_after_its_reply_header() {
        keeps_delayed_reply_intact_at_every_split("\x1bP>|", "\x1bP>|Ghostty 1.3.1\x1b\\");
    }

    #[test]
    fn keeps_delayed_apc_kitty_graphics_reply_intact_at_every_split_after_its_reply_header() {
        keeps_delayed_reply_intact_at_every_split("\x1b_Gi=1;", "\x1b_Gi=1;OK\x1b\\");
    }

    #[test]
    fn keeps_delayed_osc7501_reply_intact_at_every_split_after_its_reply_header() {
        keeps_delayed_reply_intact_at_every_split("\x1b]7501;?", "\x1b]7501;?\x1b\\");
    }

    #[test]
    fn keeps_an_osc_reply_buffered_across_multiple_expired_sequence_timeouts() {
        let mut h = Harness::standard();
        h.process("\x1b]4;1;rgb:eaea/6c");
        // The recognized reply header means no timeout is scheduled at all;
        // t.mock.timers.tick(100) fires nothing.
        assert_eq!(h.buffer.pending_timeout(), None);
        h.fire_timeout();
        h.assert_data(&[]);
        h.process("6c/6d6d");
        assert_eq!(h.buffer.pending_timeout(), None);
        h.fire_timeout();
        h.assert_data(&[]);
        h.process("\x07");
        h.assert_data(&["\x1b]4;1;rgb:eaea/6c6c/6d6d\x07"]);
    }

    #[test]
    fn still_times_out_bare_string_introducers_as_alt_keys() {
        let mut h = Harness::standard();
        for prefix in ["\x1b]", "\x1bP", "\x1b_"] {
            h.process(prefix);
            // t.mock.timers.tick(10) — the beforeEach sequence timeout is 10ms.
            assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));
            h.fire_timeout();
        }
        h.assert_data(&["\x1b]", "\x1bP", "\x1b_"]);
    }

    /// Upstream: `recovers batched legacy Alt input ${ambiguous} within the
    /// sequence timeout` (one generated test per ambiguous string).
    fn recovers_batched_legacy_alt_input(ambiguous: &str) {
        let mut h = Harness::with_defaults();
        h.process(ambiguous);
        assert!(h.sequences.is_empty());

        // t.mock.timers.tick(20) — still inside the 50ms default sequence
        // timeout, so nothing has been flushed yet.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));

        h.process("x\r\x03\x04");
        assert!(h.sequences.is_empty());
        // t.mock.timers.tick(49) — the process() above rescheduled the timeout.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));

        // t.mock.timers.tick(1) — the sequence timeout fires and recovers the
        // Alt key plus its batched tail.
        h.fire_timeout();

        let mut expected = vec![ambiguous[..2].to_string()];
        expected.extend(ambiguous[2..].chars().map(|c| c.to_string()));
        expected.extend(["x", "\r", "\x03", "\x04"].map(|s| s.to_string()));
        assert_eq!(h.data(), expected);
        assert_eq!(h.buffer.get_buffer(), "");

        h.process("y\r\x03\x04");
        expected.extend(["y", "\r", "\x03", "\x04"].map(|s| s.to_string()));
        assert_eq!(h.data(), expected);
    }

    #[test]
    fn recovers_batched_legacy_alt_input_osc_a_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b]a");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_osc_1_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b]1");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_osc_10_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b]10");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_osc_0_title_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b]0;title");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_osc_4_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b]4;");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_osc_4_a_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b]4;a;");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_dcs_a_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1bPa");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_dcs_gt_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1bP>");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_apc_a_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b_a");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_apc_g_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b_G");
    }

    #[test]
    fn recovers_batched_legacy_alt_input_apc_gi_1_within_the_sequence_timeout() {
        recovers_batched_legacy_alt_input("\x1b_Gi=1");
    }

    #[test]
    fn recognizes_a_reply_header_assembled_within_the_ambiguous_input_timeout() {
        let mut h = Harness::standard();
        h.process("\x1b]1");
        // t.mock.timers.tick(5) — inside the 10ms sequence timeout.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));
        h.process("0;");
        // The buffer is now "\x1b]10;" — a recognized reply header, so no
        // timeout is scheduled; tick(5000) fires nothing.
        assert_eq!(h.buffer.pending_timeout(), None);
        h.fire_timeout();
        h.assert_data(&[]);
        h.process("rgb:ffff/ffff/ffff\x1b\\");
        h.assert_data(&["\x1b]10;rgb:ffff/ffff/ffff\x1b\\"]);
    }

    #[test]
    fn recovers_from_an_unterminated_reply_when_a_new_escape_sequence_arrives() {
        let mut h = Harness::standard();
        h.process("\x1b]4;1;rgb:eaea/6c");
        h.fire_timeout(); // t.mock.timers.tick(100)
        h.process("\x1b");
        h.fire_timeout(); // t.mock.timers.tick(100)
        h.process("[Ahello");
        h.assert_data(&["\x1b[A", "h", "e", "l", "l", "o"]);
    }

    #[test]
    fn discards_a_canceled_reply_without_swallowing_subsequent_input() {
        let mut h = Harness::standard();
        for cancel in ["\x18", "\x1a"] {
            h.process(&format!("\x1b]11;rgb:0000{cancel}x"));
        }
        h.assert_data(&["x", "x"]);
    }

    // -----------------------------------------------------------------------
    // Mixed Content
    // -----------------------------------------------------------------------

    #[test]
    fn should_handle_characters_followed_by_escape_sequence() {
        let mut h = Harness::standard();
        h.process("abc\x1b[A");
        h.assert_data(&["a", "b", "c", "\x1b[A"]);
    }

    #[test]
    fn should_handle_escape_sequence_followed_by_characters() {
        let mut h = Harness::standard();
        h.process("\x1b[Aabc");
        h.assert_data(&["\x1b[A", "a", "b", "c"]);
    }

    #[test]
    fn should_handle_multiple_complete_sequences() {
        let mut h = Harness::standard();
        h.process("\x1b[A\x1b[B\x1b[C");
        h.assert_data(&["\x1b[A", "\x1b[B", "\x1b[C"]);
    }

    #[test]
    fn should_handle_partial_sequence_with_preceding_characters() {
        let mut h = Harness::standard();
        h.process("abc\x1b[<35");
        h.assert_data(&["a", "b", "c"]);
        assert_eq!(h.buffer.get_buffer(), "\x1b[<35");

        h.process(";20;5m");
        h.assert_data(&["a", "b", "c", "\x1b[<35;20;5m"]);
    }

    // -----------------------------------------------------------------------
    // Kitty Keyboard Protocol
    // -----------------------------------------------------------------------

    #[test]
    fn should_handle_kitty_csi_u_press_events() {
        // Press 'a' in Kitty protocol
        let mut h = Harness::standard();
        h.process("\x1b[97u");
        h.assert_data(&["\x1b[97u"]);
    }

    #[test]
    fn should_handle_kitty_csi_u_release_events() {
        // Release 'a' in Kitty protocol
        let mut h = Harness::standard();
        h.process("\x1b[97;1:3u");
        h.assert_data(&["\x1b[97;1:3u"]);
    }

    #[test]
    fn should_handle_batched_kitty_press_and_release() {
        // Press 'a', release 'a' batched together (common over SSH)
        let mut h = Harness::standard();
        h.process("\x1b[97u\x1b[97;1:3u");
        h.assert_data(&["\x1b[97u", "\x1b[97;1:3u"]);
    }

    #[test]
    fn should_handle_multiple_batched_kitty_events() {
        // Press 'a', release 'a', press 'b', release 'b'
        let mut h = Harness::standard();
        h.process("\x1b[97u\x1b[97;1:3u\x1b[98u\x1b[98;1:3u");
        h.assert_data(&["\x1b[97u", "\x1b[97;1:3u", "\x1b[98u", "\x1b[98;1:3u"]);
    }

    #[test]
    fn should_handle_kitty_arrow_keys_with_event_type() {
        // Up arrow press with event type
        let mut h = Harness::standard();
        h.process("\x1b[1;1:1A");
        h.assert_data(&["\x1b[1;1:1A"]);
    }

    #[test]
    fn should_handle_kitty_functional_keys_with_event_type() {
        // Delete key release
        let mut h = Harness::standard();
        h.process("\x1b[3;1:3~");
        h.assert_data(&["\x1b[3;1:3~"]);
    }

    #[test]
    fn should_split_esc_esc_csi_into_standalone_esc_and_the_csi_sequence_wezterm_escape_key_regression() {
        // WezTerm with enable_kitty_keyboard sends Escape key press as raw \x1b
        // and the release as a full Kitty CSI-u sequence, concatenated.
        // The buffer must not treat \x1b\x1b as a complete meta-key when the
        // following byte starts a new escape sequence.
        let mut h = Harness::standard();
        h.process("\x1b\x1b[27;129:3u");
        h.assert_data(&["\x1b", "\x1b[27;129:3u"]);
    }

    #[test]
    fn should_split_esc_esc_csi_with_no_modifier_no_num_lock() {
        let mut h = Harness::standard();
        h.process("\x1b\x1b[27;1:3u");
        h.assert_data(&["\x1b", "\x1b[27;1:3u"]);
    }

    #[test]
    fn should_still_emit_esc_esc_as_a_single_sequence_when_not_followed_by_a_new_escape() {
        // \x1b\x1b alone (no following CSI) stays as-is — e.g. ctrl+alt+[
        let mut h = Harness::standard();
        h.process("\x1b\x1b");
        h.assert_data(&["\x1b\x1b"]);
    }

    #[test]
    fn should_handle_plain_characters_mixed_with_kitty_sequences() {
        // Plain 'a' followed by Kitty release
        let mut h = Harness::standard();
        h.process("a\x1b[97;1:3u");
        h.assert_data(&["a", "\x1b[97;1:3u"]);
    }

    #[test]
    fn should_drop_raw_duplicate_character_after_matching_kitty_printable_sequence() {
        let mut h = Harness::standard();
        h.process("\x1b[224u\u{e0}");
        h.assert_data(&["\x1b[224u"]);
    }

    #[test]
    fn should_drop_raw_duplicate_character_after_matching_kitty_printable_sequence_across_chunks() {
        let mut h = Harness::standard();
        h.process("\x1b[64u");
        h.process("@");
        h.assert_data(&["\x1b[64u"]);
    }

    #[test]
    fn should_keep_non_matching_plain_character_after_kitty_printable_sequence() {
        let mut h = Harness::standard();
        h.process("\x1b[97ub");
        h.assert_data(&["\x1b[97u", "b"]);
    }

    #[test]
    fn should_keep_raw_character_after_modified_kitty_printable_sequence() {
        let mut h = Harness::standard();
        h.process("\x1b[64;3u@");
        h.assert_data(&["\x1b[64;3u", "@"]);
    }

    #[test]
    fn should_handle_rapid_typing_simulation_with_kitty_protocol() {
        // Simulates typing "hi" quickly with releases interleaved
        let mut h = Harness::standard();
        h.process("\x1b[104u\x1b[104;1:3u\x1b[105u\x1b[105;1:3u");
        h.assert_data(&["\x1b[104u", "\x1b[104;1:3u", "\x1b[105u", "\x1b[105;1:3u"]);
    }

    // -----------------------------------------------------------------------
    // Mouse Events
    // -----------------------------------------------------------------------

    #[test]
    fn should_handle_mouse_press_event() {
        let mut h = Harness::standard();
        h.process("\x1b[<0;10;5M");
        h.assert_data(&["\x1b[<0;10;5M"]);
    }

    #[test]
    fn should_handle_mouse_release_event() {
        let mut h = Harness::standard();
        h.process("\x1b[<0;10;5m");
        h.assert_data(&["\x1b[<0;10;5m"]);
    }

    #[test]
    fn should_handle_mouse_move_event() {
        let mut h = Harness::standard();
        h.process("\x1b[<35;20;5m");
        h.assert_data(&["\x1b[<35;20;5m"]);
    }

    #[test]
    fn should_handle_split_mouse_events() {
        let mut h = Harness::standard();
        h.process("\x1b[<3");
        h.process("5;1");
        h.process("5;");
        h.process("10m");
        h.assert_data(&["\x1b[<35;15;10m"]);
    }

    #[test]
    fn should_handle_multiple_mouse_events() {
        let mut h = Harness::standard();
        h.process("\x1b[<35;1;1m\x1b[<35;2;2m\x1b[<35;3;3m");
        h.assert_data(&["\x1b[<35;1;1m", "\x1b[<35;2;2m", "\x1b[<35;3;3m"]);
    }

    #[test]
    fn should_handle_old_style_mouse_sequence_esc_m_plus_3_bytes() {
        let mut h = Harness::standard();
        h.process("\x1b[M abc");
        h.assert_data(&["\x1b[M ab", "c"]);
    }

    #[test]
    fn should_buffer_incomplete_old_style_mouse_sequence() {
        let mut h = Harness::standard();
        h.process("\x1b[M");
        assert_eq!(h.buffer.get_buffer(), "\x1b[M");

        h.process(" a");
        assert_eq!(h.buffer.get_buffer(), "\x1b[M a");

        h.process("b");
        h.assert_data(&["\x1b[M ab"]);
    }

    // -----------------------------------------------------------------------
    // Edge Cases
    // -----------------------------------------------------------------------

    #[test]
    fn should_handle_empty_input() {
        let mut h = Harness::standard();
        h.process("");
        // Empty string emits an empty data event
        h.assert_data(&[""]);
    }

    #[test]
    fn should_handle_lone_escape_character_with_timeout() {
        let mut h = Harness::standard();
        h.process("\x1b");
        h.assert_data(&[]);

        // After timeout, should emit
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Escape));
        h.fire_timeout();
        h.assert_data(&["\x1b"]);
    }

    #[test]
    fn flushes_a_lone_escape_promptly_with_the_longer_default_sequence_timeout() {
        let mut h = Harness::with_defaults();
        h.process("\x1b");
        // await wait(20): the escape timeout (10ms) governs a lone ESC even
        // though the sequence timeout is 50ms.
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Escape));
        h.fire_timeout();
        h.assert_data(&["\x1b"]);
    }

    #[test]
    fn should_handle_lone_escape_character_with_explicit_flush() {
        let mut h = Harness::standard();
        h.process("\x1b");
        h.assert_data(&[]);

        let flushed = h.buffer.flush();
        assert_eq!(flushed, vec![u("\x1b")]);
    }

    #[test]
    fn should_handle_buffer_input() {
        let mut h = Harness::standard();
        h.process_bytes(b"\x1b[A");
        h.assert_data(&["\x1b[A"]);
    }

    #[test]
    fn should_handle_very_long_sequences() {
        let mut h = Harness::standard();
        let long_seq = format!("\x1b[{}H", "1;".repeat(50));
        h.process(&long_seq);
        h.assert_data(&[&long_seq]);
    }

    // -----------------------------------------------------------------------
    // Flush
    // -----------------------------------------------------------------------

    #[test]
    fn should_flush_incomplete_sequences() {
        let mut h = Harness::standard();
        h.process("\x1b[<35");
        let flushed = h.buffer.flush();
        assert_eq!(flushed, vec![u("\x1b[<35")]);
        assert_eq!(h.buffer.get_buffer(), "");
    }

    #[test]
    fn should_return_empty_array_if_nothing_to_flush() {
        let mut h = Harness::standard();
        let flushed = h.buffer.flush();
        assert!(flushed.is_empty());
    }

    #[test]
    fn should_emit_flushed_data_via_timeout() {
        let mut h = Harness::standard();
        h.process("\x1b[<35");
        h.assert_data(&[]);

        // Wait for timeout to flush
        assert_eq!(h.buffer.pending_timeout(), Some(PendingTimeout::Sequence));
        h.fire_timeout();
        h.assert_data(&["\x1b[<35"]);
    }

    // -----------------------------------------------------------------------
    // Clear
    // -----------------------------------------------------------------------

    #[test]
    fn should_clear_buffered_content_without_emitting() {
        let mut h = Harness::standard();
        h.process("\x1b[<35");
        assert_eq!(h.buffer.get_buffer(), "\x1b[<35");

        h.buffer.clear();
        assert_eq!(h.buffer.get_buffer(), "");
        h.assert_data(&[]);
    }

    // -----------------------------------------------------------------------
    // Bracketed Paste
    // -----------------------------------------------------------------------

    #[test]
    fn should_emit_paste_event_for_complete_bracketed_paste() {
        let mut h = Harness::standard();
        h.process("\x1b[200~hello world\x1b[201~");

        h.assert_pastes(&["hello world"]);
        h.assert_data(&[]); // No data events during paste
    }

    #[test]
    fn should_handle_paste_arriving_in_chunks() {
        let mut h = Harness::standard();
        h.process("\x1b[200~");
        h.assert_pastes(&[]);

        h.process("hello ");
        h.assert_pastes(&[]);

        h.process("world\x1b[201~");
        h.assert_pastes(&["hello world"]);
        h.assert_data(&[]);
    }

    #[test]
    fn should_handle_paste_with_input_before_and_after() {
        let mut h = Harness::standard();
        h.process("a");
        h.process("\x1b[200~pasted\x1b[201~");
        h.process("b");

        h.assert_data(&["a", "b"]);
        h.assert_pastes(&["pasted"]);
    }

    #[test]
    fn should_handle_paste_with_newlines() {
        let mut h = Harness::standard();
        h.process("\x1b[200~line1\nline2\nline3\x1b[201~");

        h.assert_pastes(&["line1\nline2\nline3"]);
        h.assert_data(&[]);
    }

    #[test]
    fn should_handle_paste_with_unicode() {
        let mut h = Harness::standard();
        h.process("\x1b[200~Hello 世界 \u{1f389}\x1b[201~");

        h.assert_pastes(&["Hello 世界 \u{1f389}"]);
        h.assert_data(&[]);
    }

    // -----------------------------------------------------------------------
    // Destroy
    // -----------------------------------------------------------------------

    #[test]
    fn should_clear_buffer_on_destroy() {
        let mut h = Harness::standard();
        h.process("\x1b[<35");
        assert_eq!(h.buffer.get_buffer(), "\x1b[<35");

        h.buffer.destroy();
        assert_eq!(h.buffer.get_buffer(), "");
    }

    #[test]
    fn should_clear_pending_timeouts_on_destroy() {
        let mut h = Harness::standard();
        h.process("\x1b[<35");
        h.buffer.destroy();

        // Wait longer than timeout — destroy cleared the pending timeout, so
        // firing it must not emit anything.
        assert_eq!(h.buffer.pending_timeout(), None);
        h.fire_timeout();
        h.assert_data(&[]);
    }

    // -----------------------------------------------------------------------
    // Extra: WTF-16 astral (surrogate) semantics lock-in
    //
    // The TS original operates on UTF-16 code units, so astral characters are
    // split into two data events (one per surrogate half) and the Kitty
    // printable dedup — which compares a single unit — fails for them.
    // -----------------------------------------------------------------------

    #[test]
    fn splits_astral_characters_into_one_data_event_per_surrogate_half() {
        // 'a😀b' → Data [0x61], [0xd83d], [0xde00], [0x62].
        let mut h = Harness::standard();
        h.process("a\u{1f600}b");
        assert_eq!(h.sequences, vec![u("a"), vec![0xd83d], vec![0xde00], u("b")]);
    }

    #[test]
    fn kitty_printable_dedup_fails_for_astral_characters_and_emits_both_surrogate_halves() {
        // \x1b[128512u + '😀': the dedup compares a single UTF-16 unit against
        // the pending kitty codepoint; 128512 never matches a surrogate half,
        // so the emoji still arrives as two data events after the sequence.
        let mut h = Harness::standard();
        h.process("\x1b[128512u");
        h.process("\u{1f600}");
        assert_eq!(h.sequences, vec![u("\x1b[128512u"), vec![0xd83d], vec![0xde00]]);

        // Same when batched into a single process() call.
        let mut h = Harness::standard();
        h.process("\x1b[128512u\u{1f600}");
        assert_eq!(h.sequences, vec![u("\x1b[128512u"), vec![0xd83d], vec![0xde00]]);
    }

    #[test]
    fn kitty_printable_dedup_succeeds_for_bmp_characters() {
        // BMP '\x1b[97u' + 'a' → only the sequence; dedup works.
        let mut h = Harness::standard();
        h.process("\x1b[97u");
        h.process("a");
        h.assert_data(&["\x1b[97u"]);
    }

    #[test]
    fn kitty_printable_dedup_drops_only_the_first_matching_unit() {
        // After a dropped duplicate the pending codepoint is cleared, so a
        // second '@' is a normal keypress.
        let mut h = Harness::standard();
        h.process("\x1b[64u");
        h.process("@@");
        h.assert_data(&["\x1b[64u", "@"]);
    }
}
