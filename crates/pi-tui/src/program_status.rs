//! Program Status Protocol (OSC 7501) — port of `program-status.ts`.
//!
//! A program tells the terminal whether it is idle, working, blocked on the
//! user, done, or failed. Only the root record is supported.
//!
//! Spec: <https://www.superlogical.com/rex/docs/build/program-status>

use crate::utils::is_js_whitespace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramState {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
    /// `clear` removes the status instead of reporting one.
    Clear,
}

impl ProgramState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Blocked => "blocked",
            ProgramState::Done => "done",
            ProgramState::Error => "error",
            ProgramState::Clear => "clear",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => ProgramState::Idle,
            "working" => ProgramState::Working,
            "blocked" => ProgramState::Blocked,
            "done" => ProgramState::Done,
            "error" => ProgramState::Error,
            "clear" => ProgramState::Clear,
            _ => return None,
        })
    }
}

/// What a blocked program waits for. Omitted for other states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramStatusKind {
    Permission,
    Question,
    Auth,
}

impl ProgramStatusKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgramStatusKind::Permission => "permission",
            ProgramStatusKind::Question => "question",
            ProgramStatusKind::Auth => "auth",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "permission" => ProgramStatusKind::Permission,
            "question" => ProgramStatusKind::Question,
            "auth" => ProgramStatusKind::Auth,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProgramStatus {
    pub state: Option<ProgramState>,
    /// Stable program name, `[A-Za-z0-9_.+-]{1,32}`. Other values are omitted.
    pub app: Option<String>,
    pub kind: Option<ProgramStatusKind>,
    /// One human-readable line. Control characters become spaces; longer
    /// text is cut to the spec limit.
    pub message: Option<String>,
}

/// Feature detection query. A supporting terminal replies with the same body.
pub const PROGRAM_STATUS_QUERY: &str = "\x1b]7501;?\x1b\\";

/// The reply to [`PROGRAM_STATUS_QUERY`]. Later spec revisions may add pairs
/// after the `?`. Port of `/^\x1b\]7501;\?[^\x07\x1b]*(?:\x07|\x1b\\)$/`.
pub fn is_program_status_reply(sequence: &str) -> bool {
    let Some(rest) = sequence.strip_prefix("\x1b]7501;?") else {
        return false;
    };
    let body = if let Some(b) = rest.strip_suffix("\x1b\\") {
        b
    } else if let Some(b) = rest.strip_suffix('\x07') {
        b
    } else {
        return false;
    };
    !body.chars().any(|c| c == '\x07' || c == '\x1b')
}

/// `/^[A-Za-z0-9_.+-]{1,32}$/`
fn app_pattern_valid(app: &str) -> bool {
    let count = app.chars().count();
    (1..=32).contains(&count)
        && app
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

/// Replace runs of control characters (C0, DEL, C1) with a single space,
/// mirroring `.replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ")`.
fn replace_control_characters(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        let is_control = matches!(c as u32, 0x00..=0x1f | 0x7f..=0x9f);
        if is_control {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

/// JS `String.prototype.trim()` (whitespace set is JS `\s`).
fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// Standard base64 with padding (`Buffer.from(...).toString("base64")`).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decoded `msg` limit. Its base64 encoding stays under the 2732-byte
/// encoded limit.
const MAX_MESSAGE_BYTES: usize = 2048;

/// Cut `text` to at most `max_bytes` UTF-8 bytes, at a code point boundary
/// (port of `truncateUtf8`, which iterates code points like JS `for..of`).
fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut bytes = 0usize;
    let mut end = 0usize;
    for c in text.chars() {
        let size = c.len_utf8();
        if bytes + size > max_bytes {
            break;
        }
        bytes += size;
        end += size;
    }
    &text[..end]
}

/// Encode a status report. Terminals discard reports whose text contains
/// control characters, so they are replaced.
pub fn format_program_status(status: &ProgramStatus) -> String {
    let state = status.state.unwrap_or(ProgramState::Clear);
    let mut pairs = vec![format!("state={}", state.as_str())];
    if let Some(app) = &status.app {
        if app_pattern_valid(app) {
            pairs.push(format!("app={app}"));
        }
    }
    if state == ProgramState::Blocked {
        if let Some(kind) = status.kind {
            pairs.push(format!("kind={}", kind.as_str()));
        }
    }
    let message = replace_control_characters(status.message.as_deref().unwrap_or(""));
    let message = js_trim(&message);
    let message = truncate_utf8(message, MAX_MESSAGE_BYTES);
    if !message.is_empty() {
        pairs.push(format!("msg={}", base64_encode(message.as_bytes())));
    }
    format!("\x1b]7501;{}\x1b\\", pairs.join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_message(sequence: &str) -> Option<String> {
        let start = sequence.find(":msg=")?;
        let end = sequence[start + 5..]
            .find("\x1b\\")
            .map(|i| start + 5 + i)
            .unwrap_or(sequence.len());
        let b64 = &sequence[start + 5..end];
        Some(base64_decode(b64))
    }

    /// Test-only base64 decode (mirrors `Buffer.from(b64, "base64")`).
    fn base64_decode(b64: &str) -> String {
        let table = |c: char| -> Option<u32> {
            match c {
                'A'..='Z' => Some(c as u32 - 'A' as u32),
                'a'..='z' => Some(c as u32 - 'a' as u32 + 26),
                '0'..='9' => Some(c as u32 - '0' as u32 + 52),
                '+' => Some(62),
                '/' => Some(63),
                _ => None,
            }
        };
        let mut bytes = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for c in b64.chars() {
            if c == '=' {
                break;
            }
            let Some(value) = table(c) else { break };
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                bytes.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn encodes_state_app_kind_and_base64_message() {
        let status = ProgramStatus {
            state: Some(ProgramState::Blocked),
            app: Some("pi".to_string()),
            kind: Some(ProgramStatusKind::Permission),
            message: Some("Allow bash?".to_string()),
        };
        assert_eq!(
            format_program_status(&status),
            format!(
                "\x1b]7501;state=blocked:app=pi:kind=permission:msg={}\x1b\\",
                base64_encode(b"Allow bash?")
            )
        );
        assert_eq!(
            format_program_status(&ProgramStatus {
                state: Some(ProgramState::Clear),
                ..Default::default()
            }),
            "\x1b]7501;state=clear\x1b\\"
        );
    }

    #[test]
    fn omits_kind_outside_blocked_invalid_apps_and_empty_messages() {
        let status = ProgramStatus {
            state: Some(ProgramState::Working),
            app: Some("my app".to_string()),
            kind: Some(ProgramStatusKind::Auth),
            message: Some(" \n ".to_string()),
        };
        assert_eq!(
            format_program_status(&status),
            "\x1b]7501;state=working\x1b\\"
        );
        let with_max_app = format_program_status(&ProgramStatus {
            state: Some(ProgramState::Idle),
            app: Some("a".repeat(32)),
            ..Default::default()
        });
        assert!(with_max_app.contains(":app=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1b"));
        let with_long_app = format_program_status(&ProgramStatus {
            state: Some(ProgramState::Idle),
            app: Some("a".repeat(33)),
            ..Default::default()
        });
        assert!(!with_long_app.contains("app="));
    }

    #[test]
    fn replaces_control_characters() {
        let sequence = format_program_status(&ProgramStatus {
            state: Some(ProgramState::Error),
            message: Some("first\nsecond\u{1b}[31m\u{9b}third\t".to_string()),
            ..Default::default()
        });
        assert_eq!(
            decode_message(&sequence).as_deref(),
            Some("first second [31m third")
        );
    }

    #[test]
    fn cuts_long_messages_at_utf8_boundary() {
        let sequence = format_program_status(&ProgramStatus {
            state: Some(ProgramState::Working),
            app: Some("pi".to_string()),
            message: Some("é".repeat(2000)),
            ..Default::default()
        });
        let message = decode_message(&sequence).unwrap();
        assert_eq!(message, "é".repeat(1024));
        assert!(message.len() <= 2048);
        assert!(sequence.len() <= 4096);
    }

    #[test]
    fn recognizes_program_status_replies() {
        assert!(is_program_status_reply("\x1b]7501;?\x1b\\"));
        assert!(is_program_status_reply("\x1b]7501;?\x07"));
        assert!(is_program_status_reply("\x1b]7501;?version=2\x1b\\"));
        assert!(!is_program_status_reply("\x1b]7501;state=idle\x1b\\"));
        assert!(!is_program_status_reply("\x1b]11;rgb:0000/0000/0000\x07"));
        // A BEL may not appear inside the body.
        assert!(!is_program_status_reply("\x1b]7501;?a\x07b\x07"));
    }
}
