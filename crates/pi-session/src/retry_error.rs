//! Port of `packages/ai/src/utils/overflow.ts` (`isContextOverflow`) and
//! `packages/ai/src/utils/retry.ts` (`isRetryableAssistantError`) — the
//! assistant-error classifiers `AgentSession._isRetryableError` uses.
//!
//! Messages are `serde_json::Value` (the established pi-core pattern), so
//! the classifiers read the camelCase fields directly.

use fancy_regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// `OVERFLOW_PATTERNS` (overflow.ts) — context-window-exceeded error text
/// by provider. JS `/i` flags become `(?i)` groups.
fn overflow_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        const SOURCES: &[&str] = &[
            r"(?i)prompt (?:is )?too long",
            r"(?i)prompt exceeds max length",
            r"(?i)request_too_large",
            r"(?i)input is too long for requested model",
            r"(?i)exceeds the context window",
            r"(?i)exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))",
            r"(?i)input token count.*exceeds the maximum",
            r"(?i)maximum prompt length is \d+",
            r"(?i)reduce the length of the messages",
            r"(?i)maximum context length is \d+ tokens",
            r"(?i)exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?",
            r"(?i)input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)",
            r"(?i)exceeds the limit of \d+",
            r"(?i)exceeds the available context size",
            r"(?i)greater than the context length",
            r"(?i)context window exceeds limit",
            r"(?i)exceeded model token limit",
            r"(?i)too large for model with \d+ maximum context length",
            r"(?i)prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?",
            r"(?i)model_context_window_exceeded",
            r"(?i)prompt too long; exceeded (?:max )?context length",
            r"(?i)range of input length should be",
            r"(?i)context[_ ]length[_ ]exceeded",
            r"(?i)too many tokens",
            r"(?i)token limit exceeded",
        ];
        SOURCES.iter().map(|s| Regex::new(s).expect("overflow pattern")).collect()
    })
}

/// `CEREBRAS_BODYLESS_OVERFLOW_PATTERN` (overflow.ts).
fn cerebras_bodyless_overflow_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)^4(?:00|13)\s*(?:status code)?\s*\(no body\)").expect("cerebras pattern"))
}

/// `NON_OVERFLOW_PATTERNS` (overflow.ts) — rate-limit / throttling text
/// excluded from overflow detection even when it also matches an overflow
/// pattern.
fn non_overflow_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        const SOURCES: &[&str] = &[
            r"(?i)^(Throttling error|Service unavailable):",
            r"(?i)rate limit",
            r"(?i)too many requests",
        ];
        SOURCES.iter().map(|s| Regex::new(s).expect("non-overflow pattern")).collect()
    })
}

/// `NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN` (retry.ts).
fn non_retryable_provider_limit_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        const SOURCES: &[&str] = &[
            "GoUsageLimitError",
            "FreeUsageLimitError",
            "Monthly usage limit reached",
            "available balance",
            "insufficient_quota",
            "out of budget",
            "quota exceeded",
            "billing",
            "subscription_sharing_usage_limit_exceeded",
        ];
        Regex::new(&format!("(?i){}", SOURCES.join("|"))).expect("non-retryable pattern")
    })
}

/// `RETRYABLE_PROVIDER_ERROR_PATTERN` (retry.ts) — transient provider /
/// transport failure text.
fn retryable_provider_error_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        const SOURCES: &[&str] = &[
            "overloaded",
            "server_busy",
            "servers are currently busy",
            "currently experiencing high demand",
            "model is at capacity",
            "rate.?limit",
            "too many requests",
            "429",
            "500",
            "502",
            "503",
            "504",
            "520",
            "524",
            "service.?(?:is.?)?(?:temporarily.?)?unavailable",
            "server.?error",
            "internal.?error",
            "provider.?returned.?(?:an.?)?error",
            "exceeded request buffer limit while retrying upstream",
            "unable to forward upstream",
            "upstream.?request.?failed",
            "upstream_stream_read_error",
            "stream was interrupted",
            "http/?2 stream failed",
            "network.?error",
            "connection.?error",
            "connection.?refused",
            "connection.?lost",
            "other side closed",
            "fetch failed",
            r"request to \S+ failed, reason:",
            "getaddrinfo",
            "ENOTFOUND",
            "EAI_AGAIN",
            "upstream.?connect",
            "reset before headers",
            "socket hang up",
            "socket connection was closed",
            "timed? out",
            "timeout",
            "terminated",
            "websocket.?closed",
            "websocket.?error",
            "ended without",
            "stream ended before message_stop",
            "stream ended before a terminal response event",
            "http2 request did not get a response",
            "pending stream has been canceled",
            "retry delay",
            "you can retry your request",
            "try your request again",
            "please retry your request",
            "ResourceExhausted",
            "subscription_sharing_usage_unavailable",
            "subscription_sharing_user_unavailable",
        ];
        Regex::new(&format!("(?i){}", SOURCES.join("|"))).expect("retryable pattern")
    })
}

fn field_str<'a>(message: &'a Value, key: &str) -> Option<&'a str> {
    message.get(key).and_then(Value::as_str)
}

fn usage_field(message: &Value, key: &str) -> f64 {
    // TS `message.usage.input` — absent fields produce `undefined`, and
    // `undefined + number` is `NaN`, which fails every comparison below.
    message
        .get("usage")
        .and_then(|usage| usage.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN)
}

/// TS `isContextOverflow(message, contextWindow?)` (overflow.ts).
pub fn is_context_overflow(message: &Value, context_window: Option<f64>) -> bool {
    // Case 1: error message patterns.
    if field_str(message, "stopReason") == Some("error") {
        if let Some(error_message) = field_str(message, "errorMessage") {
            let is_non_overflow =
                non_overflow_patterns().iter().any(|p| p.is_match(error_message).unwrap_or(false));
            if !is_non_overflow {
                if overflow_patterns().iter().any(|p| p.is_match(error_message).unwrap_or(false)) {
                    return true;
                }
                if field_str(message, "provider") == Some("cerebras")
                    && cerebras_bodyless_overflow_pattern().is_match(error_message).unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }

    let context_window = match context_window {
        Some(window) if window > 0.0 => window,
        _ => return false,
    };

    // Case 2: silent overflow — successful but usage exceeds the window.
    if field_str(message, "stopReason") == Some("stop") {
        let input_tokens = usage_field(message, "input") + usage_field(message, "cacheRead");
        if input_tokens > context_window {
            return true;
        }
    }

    // Case 3: length-stop overflow — truncated input filling the window
    // with zero output.
    if field_str(message, "stopReason") == Some("length") && usage_field(message, "output") == 0.0 {
        let input_tokens = usage_field(message, "input") + usage_field(message, "cacheRead");
        if input_tokens >= context_window * 0.99 {
            return true;
        }
    }

    false
}

/// TS `isRetryableAssistantError(message)` (retry.ts).
pub fn is_retryable_assistant_error(message: &Value) -> bool {
    let error_message = match field_str(message, "stopReason") {
        Some("error") => field_str(message, "errorMessage"),
        _ => None,
    };
    let Some(error_message) = error_message else { return false };
    if non_retryable_provider_limit_pattern().is_match(error_message).unwrap_or(false) {
        return false;
    }
    retryable_provider_error_pattern().is_match(error_message).unwrap_or(false)
}
