//! Port of `packages/coding-agent/src/core/settings-defaults.ts` — runtime
//! defaults shared by settings accessors and the published JSON Schema.
//!
//! The TS `SETTINGS_DEFAULTS` is a frozen `as const` object literal; the port
//! returns a fresh `serde_json::Value` built with `preserve_order`, so key
//! insertion order matches the literal exactly (JSON round-trips keep it).
//! There are no `undefined`-valued keys in the literal, so nothing is ever
//! serialized as `null`.

use serde_json::{json, Value};

/// `DEFAULT_MAX_AGENT_RETRY_DELAY_MS` from `@earendil-works/pi-ai/utils/retry`
/// (`60_000`), re-exported as `SETTINGS_DEFAULTS.retry.maxAgentDelayMs`.
pub const DEFAULT_MAX_AGENT_RETRY_DELAY_MS: u64 = 60_000;

/// Port of `SETTINGS_DEFAULTS`. Returns a new object on every call (the TS
/// value is deeply frozen; treat the result as read-only).
pub fn settings_defaults() -> Value {
	json!({
		"transport": "auto",
		"sessionShareDestination": "auto",
		"steeringMode": "one-at-a-time",
		"followUpMode": "one-at-a-time",
		"compaction": {
			"enabled": true,
			"reserveTokens": 16384,
			"keepRecentTokens": 20000,
		},
		"branchSummary": {
			"reserveTokens": 16384,
			"skipPrompt": false,
		},
		"retry": {
			"enabled": true,
			"maxRetries": 3,
			"baseDelayMs": 2000,
			"maxAgentDelayMs": DEFAULT_MAX_AGENT_RETRY_DELAY_MS,
			"provider": {
				"maxRetryDelayMs": 60000,
			},
		},
		"hideThinkingBlock": false,
		"showCacheMissNotices": false,
		"quietStartup": false,
		"defaultProjectTrust": "ask",
		"collapseChangelog": false,
		"enableInstallTelemetry": true,
		"enableAnalytics": false,
		"enableSkillCommands": true,
		"terminal": {
			"showImages": true,
			"imageWidthCells": 60,
			"clearOnShrink": false,
			"showTerminalProgress": false,
		},
		"images": {
			"autoResize": true,
			"blockImages": false,
		},
		"doubleEscapeAction": "tree",
		"treeFilterMode": "default",
		"editorPaddingX": 0,
		"outputPad": 1,
		"autocompleteMaxVisible": 5,
		"markdown": {
			"codeBlockIndent": "  ",
			"mermaid": "streaming",
		},
		"warnings": {
			"anthropicExtraUsage": true,
		},
		"codemode": {
			"mode": "on",
			"inlineBudget": 3000,
		},
		"cacheWarming": "streaming",
		"tuiMode": "fullscreen",
		"fullscreenExitOutput": "transcript",
		"fullscreenScrollbar": "auto",
		"fullscreenCopyOnSelect": true,
		"fullscreenWheelScrollLines": "auto",
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn matches_ts_literal_exactly() {
		let expected = r#"{
  "transport": "auto",
  "sessionShareDestination": "auto",
  "steeringMode": "one-at-a-time",
  "followUpMode": "one-at-a-time",
  "compaction": {
    "enabled": true,
    "reserveTokens": 16384,
    "keepRecentTokens": 20000
  },
  "branchSummary": {
    "reserveTokens": 16384,
    "skipPrompt": false
  },
  "retry": {
    "enabled": true,
    "maxRetries": 3,
    "baseDelayMs": 2000,
    "maxAgentDelayMs": 60000,
    "provider": {
      "maxRetryDelayMs": 60000
    }
  },
  "hideThinkingBlock": false,
  "showCacheMissNotices": false,
  "quietStartup": false,
  "defaultProjectTrust": "ask",
  "collapseChangelog": false,
  "enableInstallTelemetry": true,
  "enableAnalytics": false,
  "enableSkillCommands": true,
  "terminal": {
    "showImages": true,
    "imageWidthCells": 60,
    "clearOnShrink": false,
    "showTerminalProgress": false
  },
  "images": {
    "autoResize": true,
    "blockImages": false
  },
  "doubleEscapeAction": "tree",
  "treeFilterMode": "default",
  "editorPaddingX": 0,
  "outputPad": 1,
  "autocompleteMaxVisible": 5,
  "markdown": {
    "codeBlockIndent": "  ",
    "mermaid": "streaming"
  },
  "warnings": {
    "anthropicExtraUsage": true
  },
  "codemode": {
    "mode": "on",
    "inlineBudget": 3000
  },
  "cacheWarming": "streaming",
  "tuiMode": "fullscreen",
  "fullscreenExitOutput": "transcript",
  "fullscreenScrollbar": "auto",
  "fullscreenCopyOnSelect": true,
  "fullscreenWheelScrollLines": "auto"
}"#;
		let actual = serde_json::to_string_pretty(&settings_defaults()).unwrap();
		assert_eq!(actual, expected);
	}

	#[test]
	fn top_level_key_order_matches_ts() {
		let defaults = settings_defaults();
		let keys: Vec<&str> = defaults.as_object().unwrap().keys().map(String::as_str).collect();
		assert_eq!(
			keys,
			vec![
				"transport",
				"sessionShareDestination",
				"steeringMode",
				"followUpMode",
				"compaction",
				"branchSummary",
				"retry",
				"hideThinkingBlock",
				"showCacheMissNotices",
				"quietStartup",
				"defaultProjectTrust",
				"collapseChangelog",
				"enableInstallTelemetry",
				"enableAnalytics",
				"enableSkillCommands",
				"terminal",
				"images",
				"doubleEscapeAction",
				"treeFilterMode",
				"editorPaddingX",
				"outputPad",
				"autocompleteMaxVisible",
				"markdown",
				"warnings",
				"codemode",
				"cacheWarming",
				"tuiMode",
				"fullscreenExitOutput",
				"fullscreenScrollbar",
				"fullscreenCopyOnSelect",
				"fullscreenWheelScrollLines",
			]
		);
	}

	#[test]
	fn max_agent_retry_delay_constant() {
		assert_eq!(DEFAULT_MAX_AGENT_RETRY_DELAY_MS, 60_000);
		assert_eq!(
			settings_defaults()["retry"]["maxAgentDelayMs"].as_u64(),
			Some(DEFAULT_MAX_AGENT_RETRY_DELAY_MS)
		);
	}

	#[test]
	fn each_call_returns_a_fresh_object() {
		let a = settings_defaults();
		let mut b = settings_defaults();
		assert_eq!(a, b);
		b["transport"] = json!("sse");
		assert_eq!(a["transport"], json!("auto"));
	}
}
