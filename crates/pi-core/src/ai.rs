//! pi-ai type surface and utilities.
//!
//! Ports of `packages/ai/src/utils/uuid.ts` (`uuidv7`),
//! `packages/ai/src/utils/text.ts` (`contentText`) and
//! `packages/ai/src/utils/transcript.ts` (`getCurrentSystemMessage`), plus the
//! `Usage` type used across session entries and tool results.
//!
//! Messages are represented as `serde_json::Value` (like the rest of the
//! session layer) so unknown fields round-trip exactly.

use std::sync::Mutex;

use serde_json::{Map, Value};

// ============================================================================
// Number formatting
// ============================================================================

/// JSON number with JS `JSON.stringify` semantics: integral floats print
/// without a decimal point. Returns an integer `Value` when the value is
/// integral and fits, else a float `Value`.
pub fn js_number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0 {
        Value::from(v as i64)
    } else {
        // serde_json prints non-finite as null, like JSON.stringify does.
        Value::from(v)
    }
}

/// Read a JSON value as an f64 (JS `Number`).
pub fn as_f64(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

// ============================================================================
// Usage
// ============================================================================

/// `Usage` from `pi-ai`: token counts and cost. All numbers are JS numbers;
/// optional splits (`cacheWrite1h`, `reasoning`) are `None` when absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    /// Subset of `cacheWrite` written with 1h retention (Anthropic only).
    pub cache_write_1h: Option<f64>,
    /// Reasoning tokens (subset of `output`); `None` when unreported.
    pub reasoning: Option<f64>,
    pub total_tokens: f64,
    pub cost: UsageCost,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

impl Usage {
    pub fn from_value(value: &Value) -> Usage {
        let f = |key: &str| value.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Usage {
            input: f("input"),
            output: f("output"),
            cache_read: f("cacheRead"),
            cache_write: f("cacheWrite"),
            cache_write_1h: value.get("cacheWrite1h").filter(|v| !v.is_null()).and_then(|v| v.as_f64()),
            reasoning: value.get("reasoning").filter(|v| !v.is_null()).and_then(|v| v.as_f64()),
            total_tokens: f("totalTokens"),
            cost: UsageCost {
                input: value["cost"]["input"].as_f64().unwrap_or(0.0),
                output: value["cost"]["output"].as_f64().unwrap_or(0.0),
                cache_read: value["cost"]["cacheRead"].as_f64().unwrap_or(0.0),
                cache_write: value["cost"]["cacheWrite"].as_f64().unwrap_or(0.0),
                total: value["cost"]["total"].as_f64().unwrap_or(0.0),
            },
        }
    }

    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("input".into(), js_number(self.input));
        map.insert("output".into(), js_number(self.output));
        map.insert("cacheRead".into(), js_number(self.cache_read));
        map.insert("cacheWrite".into(), js_number(self.cache_write));
        if let Some(v) = self.cache_write_1h {
            map.insert("cacheWrite1h".into(), js_number(v));
        }
        if let Some(v) = self.reasoning {
            map.insert("reasoning".into(), js_number(v));
        }
        map.insert("totalTokens".into(), js_number(self.total_tokens));
        let mut cost = Map::new();
        cost.insert("input".into(), js_number(self.cost.input));
        cost.insert("output".into(), js_number(self.cost.output));
        cost.insert("cacheRead".into(), js_number(self.cost.cache_read));
        cost.insert("cacheWrite".into(), js_number(self.cost.cache_write));
        cost.insert("total".into(), js_number(self.cost.total));
        map.insert("cost".into(), Value::Object(cost));
        Value::Object(map)
    }
}

// ============================================================================
// contentText
// ============================================================================

/// `contentText`: extract and join text blocks from message content.
pub fn content_text(content: &Value, separator: &str) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(separator),
        _ => String::new(),
    }
}

/// `contentText(content)` with the default `"\n"` separator.
pub fn content_text_default(content: &Value) -> String {
    content_text(content, "\n")
}

// ============================================================================
// getCurrentSystemMessage / getCurrentTools
// ============================================================================

/// `getCurrentTools`: replay every system message's `toolsAdded` /
/// `toolsRemoved` into the current tool list (last definition per name wins).
fn get_current_tools(messages: &[Value]) -> Vec<Value> {
    let mut tools: Vec<(String, Value)> = Vec::new();
    for message in messages {
        if message["role"] != "system" {
            continue;
        }
        if let Some(removed) = message["toolsRemoved"].as_array() {
            for tool in removed {
                if let Some(name) = tool["name"].as_str() {
                    tools.retain(|(n, _)| n != name);
                }
            }
        }
        if let Some(added) = message["toolsAdded"].as_array() {
            for tool in added {
                if let Some(name) = tool["name"].as_str() {
                    tools.retain(|(n, _)| n != name);
                    tools.push((name.to_string(), tool.clone()));
                }
            }
        }
    }
    tools.into_iter().map(|(_, tool)| tool).collect()
}

/// `getCurrentSystemMessage`: replay every system message into one leading
/// system message holding the current prompt and tools.
pub fn get_current_system_message(messages: &[Value]) -> Option<Value> {
    let mut content: Vec<String> = Vec::new();
    let mut sections: Vec<(String, String)> = Vec::new();
    // Models the TS `let timestamp: number | undefined` with `??=` semantics:
    // the raw value of the first non-nullish system-message timestamp is kept
    // as-is (even a string); `None` means "never assigned" (undefined).
    let mut timestamp: Option<Value> = None;
    for message in messages {
        if message["role"] != "system" {
            continue;
        }
        if timestamp.as_ref().is_none_or(|v| v.is_null()) {
            timestamp = message.get("timestamp").cloned();
        }
        let text = content_text_default(&message["content"]);
        if !text.is_empty() {
            content.push(text);
        }
        if let Some(map) = message["sections"].as_object() {
            for (name, value) in map {
                if value.is_null() {
                    sections.retain(|(n, _)| n != name);
                } else if let Some(text) = value.as_str() {
                    sections.retain(|(n, _)| n != name);
                    sections.push((name.clone(), text.to_string()));
                }
            }
        }
    }
    let tools = get_current_tools(messages);
    if timestamp.is_none() && tools.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert("role".into(), Value::String("system".into()));
    out.insert("content".into(), Value::String(content.join("\n\n")));
    if !sections.is_empty() {
        let mut map = Map::new();
        for (name, value) in sections {
            map.insert(name, Value::String(value));
        }
        out.insert("sections".into(), Value::Object(map));
    }
    if !tools.is_empty() {
        out.insert("toolsAdded".into(), Value::Array(tools));
    }
    // `timestamp: timestamp ?? 0` — undefined/null become 0, else the raw value.
    out.insert(
        "timestamp".into(),
        timestamp.filter(|v| !v.is_null()).unwrap_or_else(|| js_number(0.0)),
    );
    Some(Value::Object(out))
}

// ============================================================================
// uuidv7
// ============================================================================

const MAX_UUID_V7_TIMESTAMP: u64 = 0xffff_ffff_ffff;
const MAX_SEQUENCE: u128 = (1u128 << 41) - 1;

struct UuidV7State {
    last_ordinary_timestamp: i64,
    sequence: Option<u128>,
}

fn uuid_state() -> &'static Mutex<UuidV7State> {
    static STATE: Mutex<UuidV7State> = Mutex::new(UuidV7State {
        last_ordinary_timestamp: -1,
        sequence: None,
    });
    &STATE
}

/// Random bytes from the OS (`crypto.getRandomValues` equivalent).
pub fn os_random_bytes(out: &mut [u8]) {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(out).is_ok() {
            return;
        }
    }
    // Extremely unlikely fallback.
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    for b in out.iter_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *b = (seed >> 24) as u8;
    }
}

/// Milliseconds since the Unix epoch (`Date.now()`).
pub fn date_now_millis() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// `new Date().toISOString()` — UTC ISO-8601 with millisecond precision.
pub fn date_to_iso_string(millis: f64) -> String {
    let total_secs = (millis / 1000.0).floor() as i64;
    let ms = (millis - total_secs as f64 * 1000.0).round() as u32;
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month,
        day,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
        ms % 1000
    )
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Generate a time-ordered UUIDv7 (`packages/ai/src/utils/uuid.ts`). A
/// supplied timestamp is preserved for follower ids.
pub fn uuidv7(timestamp_ms: Option<f64>) -> Result<String, String> {
    let requested = timestamp_ms.unwrap_or_else(date_now_millis);
    if requested.fract() != 0.0 || requested < 0.0 || requested > MAX_UUID_V7_TIMESTAMP as f64 {
        return Err(format!(
            "UUIDv7 timestamp must be an integer between 0 and {}",
            MAX_UUID_V7_TIMESTAMP
        ));
    }

    let mut state = uuid_state().lock().unwrap();
    let effective_timestamp: u64 = if timestamp_ms.is_none() {
        let requested = requested as i64;
        let effective = requested.max(state.last_ordinary_timestamp);
        state.last_ordinary_timestamp = effective;
        effective as u64
    } else {
        requested as u64
    };

    let mut bytes = [0u8; 16];
    os_random_bytes(&mut bytes);
    let sequence = match state.sequence {
        None => {
            let s = ((bytes[1] as u128) << 32)
                | ((bytes[2] as u128) << 24)
                | ((bytes[3] as u128) << 16)
                | ((bytes[4] as u128) << 8)
                | (bytes[5] as u128);
            state.sequence = Some(s);
            s
        }
        Some(s) => {
            if s == MAX_SEQUENCE {
                return Err("UUIDv7 generator sequence exhausted".to_string());
            }
            let next = s + 1;
            state.sequence = Some(next);
            next
        }
    };

    let timestamp = effective_timestamp as u128;
    for (index, byte) in bytes.iter_mut().enumerate().take(6) {
        *byte = ((timestamp >> ((5 - index) * 8)) & 0xff) as u8;
    }
    bytes[6] = 0x70 | ((sequence >> 37) & 0x0f) as u8;
    bytes[7] = ((sequence >> 29) & 0xff) as u8;
    bytes[8] = 0x80 | ((sequence >> 23) & 0x3f) as u8;
    bytes[9] = ((sequence >> 15) & 0xff) as u8;
    bytes[10] = ((sequence >> 7) & 0xff) as u8;
    bytes[11] = ((sequence & 0x7f) << 1) as u8 | (bytes[11] & 0x01);

    let hex: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    ))
}

/// `crypto.randomUUID()` — version 4 UUID.
pub fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    os_random_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_number_formatting() {
        assert_eq!(js_number(5.0), serde_json::json!(5));
        assert_eq!(js_number(5.5), serde_json::json!(5.5));
        assert_eq!(js_number(0.0), serde_json::json!(0));
    }

    #[test]
    fn uuidv7_shape_and_ordering() {
        let a = uuidv7(None).unwrap();
        let b = uuidv7(None).unwrap();
        // Version and variant bits.
        assert!(a.starts_with(|_: char| true));
        let bytes: Vec<&str> = a.split('-').collect();
        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes[0].len(), 8);
        assert!(bytes[2].starts_with('7'));
        assert!(bytes[3].starts_with('8') || bytes[3].starts_with('9') || bytes[3].starts_with('a') || bytes[3].starts_with('b'));
        // Same-millisecond sequence increments keep ordering.
        assert!(a <= b);
        // Explicit timestamps are preserved.
        let fixed = uuidv7(Some(1700000000000.0)).unwrap();
        assert!(fixed.starts_with("018bcfe5-")); // 48-bit ms timestamp prefix
    }

    #[test]
    fn uuidv7_rejects_bad_timestamps() {
        assert!(uuidv7(Some(1.5)).is_err());
        assert!(uuidv7(Some(-1.0)).is_err());
    }

    #[test]
    fn content_text_extraction() {
        assert_eq!(content_text(&serde_json::json!("plain"), "\n"), "plain");
        let content = serde_json::json!([
            {"type": "text", "text": "a"},
            {"type": "image", "data": "x", "mimeType": "image/png"},
            {"type": "text", "text": "b"},
        ]);
        assert_eq!(content_text(&content, "\n"), "a\nb");
        assert_eq!(content_text(&content, " "), "a b");
    }

    #[test]
    fn current_system_message_replay() {
        let messages = serde_json::json!([
            {"role": "system", "content": "base prompt", "timestamp": 1},
            {"role": "user", "content": "hi"},
            {"role": "system", "content": "", "sections": {"skills": "use tools"}, "timestamp": 2},
            {
                "role": "system",
                "content": "",
                "sections": {"skills": null, "style": "be terse"},
                "toolsAdded": [{"name": "read", "description": "d", "parameters": {}}],
                "toolsRemoved": [],
                "timestamp": 3
            },
        ]);
        let msgs = messages.as_array().unwrap();
        let merged = get_current_system_message(msgs).unwrap();
        assert_eq!(merged["content"], "base prompt");
        assert_eq!(merged["sections"]["skills"], Value::Null);
        assert_eq!(merged["sections"]["style"], "be terse");
        assert_eq!(merged["toolsAdded"].as_array().unwrap().len(), 1);
        assert_eq!(merged["timestamp"], 1);
        assert!(get_current_system_message(&[serde_json::json!({"role": "user", "content": "x"})]).is_none());
    }

    #[test]
    fn iso_string_formatting() {
        assert_eq!(date_to_iso_string(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(date_to_iso_string(1767323045123.0), "2026-01-02T03:04:05.123Z");
        assert_eq!(date_to_iso_string(123.0), "1970-01-01T00:00:00.123Z");
    }
}
