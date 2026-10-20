//! Port of `packages/coding-agent/src/core/http-dispatcher.ts` — parse/format half.
//!
//! The TS module has two responsibilities:
//!
//! 1. **Pure parse/format of the HTTP idle-timeout setting**
//!    (`DEFAULT_HTTP_IDLE_TIMEOUT_MS`, `HTTP_IDLE_TIMEOUT_CHOICES`,
//!    `parseHttpIdleTimeoutMs`, `formatHttpIdleTimeoutMs`) — fully ported here
//!    and parity-testable.
//! 2. **Global HTTP client configuration** (`applyHttpProxySettings`,
//!    `configureHttpDispatcher` + the `withUndiciErrorListener` /
//!    `createUndiciClient` / `createUndiciOriginDispatcher` helpers and the
//!    module-level `originalGlobalFetch` / `installedGlobalFetch` tracking) —
//!    **deferred**. These mutate process-global state (`process.env`
//!    `HTTP_PROXY`/`HTTPS_PROXY` `??=`, `undici.setGlobalDispatcher`,
//!    `undici.install()`) and are not parity-testable (plan §3.2: "Not
//!    parity-tested; ported as builder config for the real HTTP client").
//!    When the real HTTP stack lands, they become builder options on the
//!    `HttpDispatcher` trait implementation (undici `EnvHttpProxyAgent` with
//!    `allowH2: false`, `proxyTunnel: true`, `bodyTimeout`/`headersTimeout`
//!    = the parsed timeout, `autoSelectFamilyAttemptTimeout` 2 000).
//!
//! TS source: /tmp/pi-repo/packages/coding-agent/src/core/http-dispatcher.ts
//! (113 LOC; only the parse/format exports are in scope here).

use serde_json::Value;

// JS-semantics helpers (`js_trim`, `js_to_number`, `js_number_to_string`) —
// see `crates/pi-providers/src/pi_ai/js.rs` (delegate A, foundation wave).
use crate::pi_ai::js::{js_number_to_string, js_to_number, js_trim};

// ============================================================================
// Constants
// ============================================================================

/// Node's 250 ms default can terminate valid connection attempts on
/// high-latency routes (TS: `DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300_000`).
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// One selectable idle-timeout choice (TS: `{ label, timeoutMs }` entries of
/// `HTTP_IDLE_TIMEOUT_CHOICES`, `as const`). `timeout_ms == 0` means
/// "disabled".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpIdleTimeoutChoice {
    pub label: &'static str,
    pub timeout_ms: u64,
}

/// TS: `HTTP_IDLE_TIMEOUT_CHOICES` — order preserved (first match wins in
/// `format_http_idle_timeout_ms`).
pub const HTTP_IDLE_TIMEOUT_CHOICES: &[HttpIdleTimeoutChoice] = &[
    HttpIdleTimeoutChoice { label: "30 sec", timeout_ms: 30_000 },
    HttpIdleTimeoutChoice { label: "1 min", timeout_ms: 60_000 },
    HttpIdleTimeoutChoice { label: "2 min", timeout_ms: 120_000 },
    HttpIdleTimeoutChoice { label: "5 min", timeout_ms: 300_000 },
    HttpIdleTimeoutChoice { label: "disabled", timeout_ms: 0 },
];

// ============================================================================
// parseHttpIdleTimeoutMs
// ============================================================================

/// Port of `parseHttpIdleTimeoutMs(value: unknown): number | undefined`.
///
/// Three-valued result: `None` = invalid, `Some(0)` = "disabled" (meaningful),
/// `Some(n)` = a timeout in milliseconds.
///
/// JS semantics reproduced (verified against the TS dist):
/// - string values are `trim()`med (JS Unicode trim incl. `\u00A0`, `\uFEFF`)
///   and compared case-insensitively against `"disabled"` → `Some(0)`
/// - trimmed-empty strings → `None` (guarded *before* `Number("") → 0`)
/// - other strings go through `Number(trimmed)` (accepts `"1e3"`, `"0x1E"` →
///   30, `"+5"`, `".5"`, `"5."`, `"  0xA  "`; rejects `"5abc"`, `"1_000"`;
///   `"Infinity"` parses but is then rejected by `Number.isFinite`)
/// - numbers must be finite and `>= 0` (`-0` passes), then `Math.floor`ed
/// - any other JSON type (bool/null/object/array/absent) → `None`
///
/// Divergence note: the TS returns the raw (floored) JS number, so
/// `1e300` stays `1e300`; this port returns `Option<u64>` per the plan §4.2
/// API sketch and saturates out-of-range values to `u64::MAX`.
pub fn parse_http_idle_timeout_ms(value: &Value) -> Option<u64> {
    if let Value::String(s) = value {
        let trimmed = js_trim(s);
        // JS `trimmed.toLowerCase() === "disabled"` — full Unicode lowercase
        // mapping; any input that could lowercase to "disabled" is ASCII.
        if trimmed.to_lowercase() == "disabled" {
            return Some(0);
        }
        if trimmed.is_empty() {
            return None;
        }
        return parse_number_value(js_to_number(trimmed));
    }
    if let Value::Number(n) = value {
        return parse_number_value(n.as_f64().unwrap_or(f64::NAN));
    }
    None
}

/// The `typeof value === "number"` branch of `parseHttpIdleTimeoutMs`:
/// `!Number.isFinite(value) || value < 0` → invalid, else `Math.floor(value)`.
fn parse_number_value(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let floored = value.floor();
    if floored >= u64::MAX as f64 {
        Some(u64::MAX)
    } else {
        Some(floored as u64)
    }
}

// ============================================================================
// formatHttpIdleTimeoutMs
// ============================================================================

/// Port of `formatHttpIdleTimeoutMs(timeoutMs: number): string`.
///
/// Returns the matching `HTTP_IDLE_TIMEOUT_CHOICES` label (`0` → "disabled",
/// `30_000` → "30 sec", …; JS `===` so `-0` matches `0`), else
/// `` `${timeoutMs / 1000} sec` `` using JS number-to-string
/// (shortest round-trip with ECMAScript plain-vs-exponential rules):
/// `1500` → "1.5 sec", `100` → "0.1 sec", `1e21` → "1000000000000000000 sec",
/// `1e-7` → "9.999999999999999e-11 sec", `NaN` → "NaN sec".
pub fn format_http_idle_timeout_ms(timeout_ms: f64) -> String {
    if let Some(choice) = HTTP_IDLE_TIMEOUT_CHOICES
        .iter()
        .find(|item| (item.timeout_ms as f64) == timeout_ms)
    {
        return choice.label.to_string();
    }
    format!("{} sec", js_number_to_string(timeout_ms / 1000.0))
}

// ============================================================================
// Tests (fixtures probed against the TS dist with node)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constants_match_ts() {
        assert_eq!(DEFAULT_HTTP_IDLE_TIMEOUT_MS, 300_000);
        let labels: Vec<&str> =
            HTTP_IDLE_TIMEOUT_CHOICES.iter().map(|c| c.label).collect();
        assert_eq!(
            labels,
            vec!["30 sec", "1 min", "2 min", "5 min", "disabled"]
        );
        let timeouts: Vec<u64> =
            HTTP_IDLE_TIMEOUT_CHOICES.iter().map(|c| c.timeout_ms).collect();
        assert_eq!(timeouts, vec![30_000, 60_000, 120_000, 300_000, 0]);
    }

    #[test]
    fn parse_accepts_disabled_spellings() {
        // "disabled" / "DISABLED" / " disabled " / "Disabled " → 0
        for s in ["disabled", "DISABLED", " disabled ", "Disabled "] {
            assert_eq!(parse_http_idle_timeout_ms(&json!(s)), Some(0), "{s:?}");
        }
    }

    #[test]
    fn parse_string_number_semantics() {
        // Number(trimmed) JS semantics, probed against the dist.
        assert_eq!(parse_http_idle_timeout_ms(&json!("1e3")), Some(1000));
        assert_eq!(parse_http_idle_timeout_ms(&json!("0x1E")), Some(30));
        assert_eq!(parse_http_idle_timeout_ms(&json!("+5")), Some(5));
        assert_eq!(parse_http_idle_timeout_ms(&json!("  42  ")), Some(42));
        assert_eq!(parse_http_idle_timeout_ms(&json!("  0xA  ")), Some(10));
        assert_eq!(parse_http_idle_timeout_ms(&json!("0b101")), Some(5));
        assert_eq!(parse_http_idle_timeout_ms(&json!("0o17")), Some(15));
        assert_eq!(parse_http_idle_timeout_ms(&json!(".5")), Some(0)); // floor(0.5)
        assert_eq!(parse_http_idle_timeout_ms(&json!("5.")), Some(5));
        assert_eq!(parse_http_idle_timeout_ms(&json!("2.9")), Some(2)); // floor
        assert_eq!(parse_http_idle_timeout_ms(&json!("-0")), Some(0)); // -0 >= 0
        assert_eq!(parse_http_idle_timeout_ms(&json!("0")), Some(0));
        assert_eq!(parse_http_idle_timeout_ms(&json!("30000")), Some(30000));
    }

    #[test]
    fn parse_rejects_invalid_strings() {
        for s in [
            "Infinity",    // parses to Infinity, rejected by isFinite
            "-Infinity",
            "5abc",
            "1_000", // numeric separators rejected by Number()
            "",
            "   ", // trimmed-empty guarded before Number("") → 0
        ] {
            assert_eq!(parse_http_idle_timeout_ms(&json!(s)), None, "{s:?}");
        }
    }

    #[test]
    fn parse_number_values() {
        assert_eq!(parse_http_idle_timeout_ms(&json!(30.7)), Some(30));
        assert_eq!(parse_http_idle_timeout_ms(&json!(-1)), None);
        assert_eq!(parse_http_idle_timeout_ms(&json!(-0.5)), None); // < 0
        assert_eq!(parse_http_idle_timeout_ms(&json!(0)), Some(0));
        // serde_json keeps -0 as f64 -0.0; JS -0 >= 0 → floor(-0) = -0 → 0
        let neg_zero: f64 = serde_json::from_str("-0").unwrap();
        assert_eq!(parse_http_idle_timeout_ms(&json!(neg_zero)), Some(0));
    }

    #[test]
    fn parse_rejects_non_number_types() {
        // typeof value !== "string" && !== "number" → undefined
        for v in [json!(true), json!(null), json!({"a": 1}), json!([5]), Value::Null] {
            assert_eq!(parse_http_idle_timeout_ms(&v), None, "{v}");
        }
    }

    #[test]
    fn parse_out_of_range_saturates() {
        // TS returns the raw number (1e300); plan §4.2 pins Option<u64>.
        assert_eq!(parse_http_idle_timeout_ms(&json!(1e300)), Some(u64::MAX));
    }

    #[test]
    fn format_choice_labels() {
        assert_eq!(format_http_idle_timeout_ms(30_000.0), "30 sec");
        assert_eq!(format_http_idle_timeout_ms(60_000.0), "1 min");
        assert_eq!(format_http_idle_timeout_ms(120_000.0), "2 min");
        assert_eq!(format_http_idle_timeout_ms(300_000.0), "5 min");
        assert_eq!(format_http_idle_timeout_ms(0.0), "disabled");
        assert_eq!(format_http_idle_timeout_ms(-0.0), "disabled"); // -0 === 0
    }

    #[test]
    fn format_fractional_seconds_js_number_to_string() {
        // `${timeoutMs / 1000} sec` — probed against the dist.
        assert_eq!(format_http_idle_timeout_ms(1500.0), "1.5 sec");
        assert_eq!(format_http_idle_timeout_ms(1501.0), "1.501 sec");
        assert_eq!(format_http_idle_timeout_ms(100.0), "0.1 sec");
        assert_eq!(format_http_idle_timeout_ms(500.0), "0.5 sec");
        assert_eq!(format_http_idle_timeout_ms(1.0), "0.001 sec");
        assert_eq!(format_http_idle_timeout_ms(-5.0), "-0.005 sec");
        assert_eq!(format_http_idle_timeout_ms(0.5), "0.0005 sec");
        assert_eq!(format_http_idle_timeout_ms(30000.5), "30.0005 sec");
    }

    #[test]
    fn format_extremes_and_specials() {
        // 123456789012345678901 → x/1000 = 123456789012345680 (plain digits)
        assert_eq!(
            format_http_idle_timeout_ms(123456789012345678901.0),
            "123456789012345680 sec"
        );
        // 1e21 / 1000 = 1e18 — still below the 1e21 exponent threshold
        assert_eq!(
            format_http_idle_timeout_ms(1e21),
            "1000000000000000000 sec"
        );
        // 1e-7 / 1000 → smallest magnitude with exponent form
        assert_eq!(
            format_http_idle_timeout_ms(1e-7),
            "9.999999999999999e-11 sec"
        );
        assert_eq!(format_http_idle_timeout_ms(f64::NAN), "NaN sec");
        assert_eq!(format_http_idle_timeout_ms(f64::INFINITY), "Infinity sec");
        assert_eq!(
            format_http_idle_timeout_ms(f64::NEG_INFINITY),
            "-Infinity sec"
        );
    }
}
