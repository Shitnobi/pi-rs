//! Port of `src/core/defaults.ts`.
//!
//! Type-only module in the TS: `DEFAULT_THINKING_LEVEL` and
//! `THINKING_LEVEL_OPTIONS`, typed by `ThinkingLevel` from
//! `@earendil-works/pi-agent-core` (`packages/agent/src/types.ts` L349:
//! `"off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"`).

use std::fmt;

use serde::{Deserialize, Serialize};

/// `ThinkingLevel` from pi-agent-core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
	Off,
	Minimal,
	Low,
	Medium,
	High,
	Xhigh,
	Max,
}

impl ThinkingLevel {
	/// The TS string value.
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Off => "off",
			Self::Minimal => "minimal",
			Self::Low => "low",
			Self::Medium => "medium",
			Self::High => "high",
			Self::Xhigh => "xhigh",
			Self::Max => "max",
		}
	}
}

impl fmt::Display for ThinkingLevel {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(self.as_str())
	}
}

/// `DEFAULT_THINKING_LEVEL`.
pub const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Medium;

/// `THINKING_LEVEL_OPTIONS`.
pub const THINKING_LEVEL_OPTIONS: [ThinkingLevel; 7] = [
	ThinkingLevel::Off,
	ThinkingLevel::Minimal,
	ThinkingLevel::Low,
	ThinkingLevel::Medium,
	ThinkingLevel::High,
	ThinkingLevel::Xhigh,
	ThinkingLevel::Max,
];

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn default_thinking_level_is_medium() {
		assert_eq!(DEFAULT_THINKING_LEVEL, ThinkingLevel::Medium);
		assert_eq!(DEFAULT_THINKING_LEVEL.as_str(), "medium");
	}

	#[test]
	fn thinking_level_options_match_ts() {
		assert_eq!(
			THINKING_LEVEL_OPTIONS.map(|level| level.as_str()),
			["off", "minimal", "low", "medium", "high", "xhigh", "max"]
		);
	}

	#[test]
	fn thinking_level_serde_round_trips() {
		for level in THINKING_LEVEL_OPTIONS {
			let json = serde_json::to_string(&level).unwrap();
			assert_eq!(json, format!("\"{}\"", level.as_str()));
			assert_eq!(serde_json::from_str::<ThinkingLevel>(&json).unwrap(), level);
		}
	}
}
