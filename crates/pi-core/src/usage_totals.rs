//! Port of `src/core/usage-totals.ts`.
//!
//! `getUsageCostBreakdown` is deferred until the session-info layer needs it.

use serde_json::Value;

use crate::ai::{js_number, Usage};

/// Running usage totals.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageTotals {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cost: f64,
}

/// `createUsageTotals`.
pub fn create_usage_totals() -> UsageTotals {
    UsageTotals::default()
}

/// `addUsageToTotals`: add one usage to running totals.
pub fn add_usage_to_totals(totals: &mut UsageTotals, usage: &Usage) {
    totals.input += usage.input;
    totals.output += usage.output;
    totals.cache_read += usage.cache_read;
    totals.cache_write += usage.cache_write;
    totals.cost += usage.cost.total;
}

/// `combineUsage`: sum of two usages, keeping the optional token splits when
/// either side reports them.
pub fn combine_usage(first: &Usage, second: &Usage) -> Usage {
    Usage {
        input: first.input + second.input,
        output: first.output + second.output,
        cache_read: first.cache_read + second.cache_read,
        cache_write: first.cache_write + second.cache_write,
        cache_write_1h: match (first.cache_write_1h, second.cache_write_1h) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        },
        reasoning: match (first.reasoning, second.reasoning) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        },
        total_tokens: first.total_tokens + second.total_tokens,
        cost: UsageCostSum::sum(&first.cost, &second.cost),
    }
}

struct UsageCostSum;

impl UsageCostSum {
    fn sum(first: &crate::ai::UsageCost, second: &crate::ai::UsageCost) -> crate::ai::UsageCost {
        crate::ai::UsageCost {
            input: first.input + second.input,
            output: first.output + second.output,
            cache_read: first.cache_read + second.cache_read,
            cache_write: first.cache_write + second.cache_write,
            total: first.total + second.total,
        }
    }
}

/// Serialize a `UsageTotals` for display.
pub fn totals_to_value(totals: &UsageTotals) -> Value {
    serde_json::json!({
        "input": js_number(totals.input),
        "output": js_number(totals.output),
        "cacheRead": js_number(totals.cache_read),
        "cacheWrite": js_number(totals.cache_write),
        "cost": js_number(totals.cost),
    })
}

/// Parse a usage from JSON (see `Usage::from_value`).
pub fn usage_from_value(value: &Value) -> Usage {
    Usage::from_value(value)
}

/// Convenience: combine two usage JSON values.
pub fn combine_usage_values(first: &Value, second: &Value) -> Value {
    combine_usage(&Usage::from_value(first), &Usage::from_value(second)).to_value()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(input: f64, output: f64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0.0,
            cache_write: 0.0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + output,
            cost: crate::ai::UsageCost {
                input: input * 0.001,
                output: output * 0.002,
                cache_read: 0.0,
                cache_write: 0.0,
                total: input * 0.001 + output * 0.002,
            },
        }
    }

    #[test]
    fn combine_keeps_optional_splits() {
        let a = Usage { reasoning: Some(5.0), ..usage(10.0, 20.0) };
        let b = Usage { cache_write_1h: Some(0.0), ..usage(1.0, 2.0) };
        let combined = combine_usage(&a, &b);
        assert_eq!(combined.input, 11.0);
        assert_eq!(combined.reasoning, Some(5.0));
        assert_eq!(combined.cache_write_1h, Some(0.0));
        let neither = combine_usage(&usage(1.0, 1.0), &usage(2.0, 2.0));
        assert_eq!(neither.reasoning, None);
        assert_eq!(neither.cache_write_1h, None);
    }

    #[test]
    fn totals_accumulate() {
        let mut totals = create_usage_totals();
        add_usage_to_totals(&mut totals, &usage(10.0, 20.0));
        add_usage_to_totals(&mut totals, &usage(1.0, 2.0));
        assert_eq!(totals.input, 11.0);
        assert!((totals.cost - (10.0 * 0.001 + 20.0 * 0.002 + 1.0 * 0.001 + 2.0 * 0.002)).abs() < 1e-12);
    }

    #[test]
    fn value_roundtrip() {
        let value = json!({
            "input": 3, "output": 4, "cacheRead": 5, "cacheWrite": 6,
            "cacheWrite1h": 1, "reasoning": 2, "totalTokens": 18,
            "cost": {"input": 0.1, "output": 0.2, "cacheRead": 0, "cacheWrite": 0, "total": 0.3}
        });
        let parsed = Usage::from_value(&value);
        assert_eq!(parsed.cache_write_1h, Some(1.0));
        assert_eq!(parsed.reasoning, Some(2.0));
        let back = parsed.to_value();
        assert_eq!(back["input"], json!(3));
        assert_eq!(back["cost"]["total"], json!(0.3));
        assert_eq!(crate::ai::as_f64(&back["totalTokens"]), 18.0);
    }
}
