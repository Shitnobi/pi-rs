//! Provider attribution headers: port of
//! `packages/coding-agent/src/core/provider-attribution.ts`.
//!
//! Merges session-affinity headers, telemetry-gated attribution headers,
//! and caller-supplied header sources into one ordered header map. The
//! port never reads the environment: the TS `isInstallTelemetryEnabled`
//! settings/env lookup collapses to the `telemetry_enabled` flag owned by
//! the caller (the parity harness injects it).
//!
//! Semantics kept verbatim (do not "fix"):
//! - `matchesHost` compares the WHATWG-parsed hostname (lowercased);
//!   invalid URLs never match. Implemented with
//!   `pi_core::mcp_servers::parse_url`, whose `hostname` is lowercased the
//!   same way.
//! - `isOpenRouterModel` keeps the raw-substring `baseUrl.includes(...)`
//!   check — NOT a hostname match (`https://proxy.example/openrouter.ai/v1`
//!   counts, mixed-case hosts do not).

use indexmap::IndexMap;

use pi_core::mcp_servers::parse_url;

use crate::pi_ai::model::Model;

/// `ProviderHeaders` — ordered header map. All values produced by this
/// merge are strings (`null` suppression values only occur in provider
/// default-header maps, which never flow through here).
pub type ProviderHeaders = IndexMap<String, String>;

const OPENROUTER_HOST: &str = "openrouter.ai";
const NVIDIA_NIM_HOST: &str = "integrate.api.nvidia.com";
const CLOUDFLARE_API_HOST: &str = "api.cloudflare.com";
const CLOUDFLARE_AI_GATEWAY_HOST: &str = "gateway.ai.cloudflare.com";
const OPENCODE_HOST: &str = "opencode.ai";

/// `matchesHost(baseUrl, expectedHost)`: `new URL(baseUrl).hostname ===
/// expectedHost` — WHATWG lowercases hostnames, so `parse_url`'s
/// lowercased `hostname` is the equivalent. Invalid URLs → `false`.
fn matches_host(base_url: &str, expected_host: &str) -> bool {
	parse_url(base_url).is_some_and(|url| url.hostname == expected_host)
}

/// `isOpenRouterModel` — raw substring check on the untouched `baseUrl`.
fn is_open_router_model(model: &Model) -> bool {
	model.provider == "openrouter" || model.base_url.contains(OPENROUTER_HOST)
}

/// `isNvidiaNimModel`.
fn is_nvidia_nim_model(model: &Model) -> bool {
	model.provider == "nvidia" || matches_host(&model.base_url, NVIDIA_NIM_HOST)
}

/// `isCloudflareModel`.
fn is_cloudflare_model(model: &Model) -> bool {
	model.provider == "cloudflare-workers-ai"
		|| model.provider == "cloudflare-ai-gateway"
		|| matches_host(&model.base_url, CLOUDFLARE_API_HOST)
		|| matches_host(&model.base_url, CLOUDFLARE_AI_GATEWAY_HOST)
}

/// `getDefaultAttributionHeaders` (the telemetry gate is applied by the
/// caller, mirroring the TS settings-manager check that wraps the call).
fn default_attribution_headers(model: &Model) -> Option<ProviderHeaders> {
	let mut headers = ProviderHeaders::new();
	if is_open_router_model(model) {
		headers.insert("HTTP-Referer".to_string(), "https://pi.dev".to_string());
		headers.insert("X-OpenRouter-Title".to_string(), "pi".to_string());
		headers.insert("X-OpenRouter-Categories".to_string(), "cli-agent".to_string());
	} else if is_nvidia_nim_model(model) {
		headers.insert("X-BILLING-INVOKE-ORIGIN".to_string(), "Pi".to_string());
	} else if is_cloudflare_model(model) {
		headers.insert("User-Agent".to_string(), "pi-coding-agent".to_string());
	} else {
		return None;
	}
	Some(headers)
}

/// `getSessionHeaders` — only for `opencode`/`opencode-go` providers or an
/// `opencode.ai` base-URL hostname. An absent OR empty session id yields
/// nothing (JS `!sessionId`).
fn session_headers(model: &Model, session_id: Option<&str>) -> Option<ProviderHeaders> {
	let session_id = session_id?;
	if session_id.is_empty() {
		return None;
	}
	if model.provider != "opencode"
		&& model.provider != "opencode-go"
		&& !matches_host(&model.base_url, OPENCODE_HOST)
	{
		return None;
	}
	let mut headers = ProviderHeaders::new();
	headers.insert("x-opencode-session".to_string(), session_id.to_string());
	headers.insert("x-opencode-client".to_string(), "pi".to_string());
	Some(headers)
}

/// `mergeProviderAttributionHeaders` — session headers, then telemetry-
/// gated default attribution headers, then each header source in order
/// (later entries win on key collisions). An empty merge is `undefined`.
pub fn merge_provider_attribution_headers(
	model: &Model,
	telemetry_enabled: bool,
	session_id: Option<&str>,
	header_sources: &[Option<ProviderHeaders>],
) -> Option<ProviderHeaders> {
	let mut merged = ProviderHeaders::new();
	if let Some(headers) = session_headers(model, session_id) {
		merged.extend(headers);
	}
	if telemetry_enabled {
		if let Some(headers) = default_attribution_headers(model) {
			merged.extend(headers);
		}
	}
	for headers in header_sources.iter().flatten() {
		merged.extend(headers.clone());
	}
	if merged.is_empty() {
		None
	} else {
		Some(merged)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::{json, Value};

	fn model(provider: &str, base_url: &str) -> Model {
		let value = json!({
			"id": "m",
			"name": "Model",
			"api": "anthropic-messages",
			"provider": provider,
			"baseUrl": base_url,
			"input": ["text"],
			"cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
			"reasoning": false,
			"contextWindow": 128000,
			"maxTokens": 16384,
		});
		let map = match value {
			Value::Object(map) => map,
			_ => unreachable!(),
		};
		Model::from_map(map).expect("valid model")
	}

	fn merged(
		provider: &str,
		base_url: &str,
		telemetry_enabled: bool,
		session_id: Option<&str>,
		header_sources: &[Option<ProviderHeaders>],
	) -> Option<ProviderHeaders> {
		merge_provider_attribution_headers(
			&model(provider, base_url),
			telemetry_enabled,
			session_id,
			header_sources,
		)
	}

	fn pairs(headers: Option<ProviderHeaders>) -> Vec<(String, String)> {
		headers
			.map(|headers| headers.into_iter().collect())
			.unwrap_or_default()
	}

	#[test]
	fn telemetry_off_no_headers_is_none() {
		assert_eq!(merged("anthropic", "https://api.anthropic.com", false, None, &[]), None);
	}

	#[test]
	fn telemetry_on_anthropic_is_none() {
		assert_eq!(merged("anthropic", "https://api.anthropic.com", true, None, &[]), None);
	}

	#[test]
	fn openrouter_provider_headers() {
		let headers = merged("openrouter", "https://openrouter.ai/api/v1", true, None, &[]);
		assert_eq!(
			pairs(headers),
			vec![
				("HTTP-Referer".to_string(), "https://pi.dev".to_string()),
				("X-OpenRouter-Title".to_string(), "pi".to_string()),
				("X-OpenRouter-Categories".to_string(), "cli-agent".to_string()),
			]
		);
	}

	#[test]
	fn openrouter_provider_telemetry_off_is_none() {
		assert_eq!(merged("openrouter", "https://openrouter.ai/api/v1", false, None, &[]), None);
	}

	#[test]
	fn openrouter_baseurl_substring_matches() {
		// Hostname is proxy.example — only the raw substring rule matches.
		let headers = merged("myprov", "https://proxy.example/openrouter.ai/v1", true, None, &[]);
		assert_eq!(
			pairs(headers),
			vec![
				("HTTP-Referer".to_string(), "https://pi.dev".to_string()),
				("X-OpenRouter-Title".to_string(), "pi".to_string()),
				("X-OpenRouter-Categories".to_string(), "cli-agent".to_string()),
			]
		);
	}

	#[test]
	fn openrouter_lookalike_host_still_substring_matches() {
		// hostname "openrouter.ai.example.com" is not openrouter.ai, but the
		// substring check is raw.
		let headers = merged("myprov", "https://openrouter.ai.example.com/v1", true, None, &[]);
		assert_eq!(pairs(headers).len(), 3);
	}

	#[test]
	fn openrouter_mixed_case_substring_does_not_match() {
		// `includes` is case-sensitive; hostname lowercasing does not apply.
		assert_eq!(merged("myprov", "https://OpenRouter.AI/v1", true, None, &[]), None);
	}

	#[test]
	fn nvidia_provider_headers() {
		let headers = merged("nvidia", "https://integrate.api.nvidia.com/v1", true, None, &[]);
		assert_eq!(
			pairs(headers),
			vec![("X-BILLING-INVOKE-ORIGIN".to_string(), "Pi".to_string())]
		);
	}

	#[test]
	fn nvidia_hostname_matches() {
		let headers = merged("myprov", "https://integrate.api.nvidia.com/v1", true, None, &[]);
		assert_eq!(pairs(headers).len(), 1);
	}

	#[test]
	fn nvidia_hostname_is_lowercased() {
		// WHATWG `new URL(x).hostname` lowercases the host.
		let headers = merged("myprov", "https://Integrate.API.NVIDIA.COM/v1", true, None, &[]);
		assert_eq!(pairs(headers).len(), 1);
	}

	#[test]
	fn nvidia_substring_not_hostname_does_not_match() {
		assert_eq!(merged("myprov", "https://evil.example/integrate.api.nvidia.com", true, None, &[]), None);
	}

	#[test]
	fn cloudflare_provider_kinds_match() {
		assert_eq!(
			pairs(merged("cloudflare-workers-ai", "https://api.cloudflare.com/client/v4", true, None, &[])),
			vec![("User-Agent".to_string(), "pi-coding-agent".to_string())]
		);
		assert_eq!(merged("cloudflare-ai-gateway", "https://x.example", true, None, &[]).is_some(), true);
	}

	#[test]
	fn cloudflare_hostnames_match() {
		assert_eq!(
			pairs(merged("myprov", "https://api.cloudflare.com/client/v4/accounts/abc", true, None, &[])),
			vec![("User-Agent".to_string(), "pi-coding-agent".to_string())]
		);
		assert_eq!(
			pairs(merged("myprov", "https://gateway.ai.cloudflare.com/v1/abc/anthropic", true, None, &[])),
			vec![("User-Agent".to_string(), "pi-coding-agent".to_string())]
		);
	}

	#[test]
	fn opencode_session_headers() {
		let headers = merged("opencode", "https://opencode.ai/api", false, Some("s-1"), &[]);
		assert_eq!(
			pairs(headers),
			vec![
				("x-opencode-session".to_string(), "s-1".to_string()),
				("x-opencode-client".to_string(), "pi".to_string()),
			]
		);
	}

	#[test]
	fn opencode_session_plus_default_headers_order() {
		// Session headers first, then defaults (no key overlap, order only
		// observable via JSON key order).
		let headers =
			merged("opencode", "https://integrate.api.nvidia.com/v1", true, Some("s-1"), &[]);
		assert_eq!(
			pairs(headers),
			vec![
				("x-opencode-session".to_string(), "s-1".to_string()),
				("x-opencode-client".to_string(), "pi".to_string()),
				("X-BILLING-INVOKE-ORIGIN".to_string(), "Pi".to_string()),
			]
		);
	}

	#[test]
	fn opencode_go_provider_session() {
		let headers = merged("opencode-go", "https://opencode.ai/api", false, Some("s-1"), &[]);
		assert_eq!(pairs(headers).len(), 2);
	}

	#[test]
	fn opencode_hostname_session() {
		let headers = merged("myprov", "https://opencode.ai/api", false, Some("s-1"), &[]);
		assert_eq!(pairs(headers).len(), 2);
	}

	#[test]
	fn session_on_non_opencode_is_ignored() {
		assert_eq!(merged("anthropic", "https://api.anthropic.com", false, Some("s-1"), &[]), None);
	}

	#[test]
	fn garbage_url_falls_back_to_provider_only() {
		// Invalid URL → matchesHost false; provider id still applies.
		let headers = merged("nvidia", "not a url", true, None, &[]);
		assert_eq!(pairs(headers).len(), 1);
		// opencode session headers still apply via provider with garbage URL.
		let headers = merged("opencode", "not a url", false, Some("s-1"), &[]);
		assert_eq!(pairs(headers).len(), 2);
		// No provider match and garbage URL → nothing.
		assert_eq!(merged("myprov", "not a url", true, None, &[]), None);
	}

	#[test]
	fn header_sources_merge_and_later_wins() {
		let mut first = ProviderHeaders::new();
		first.insert("a".to_string(), "1".to_string());
		let mut second = ProviderHeaders::new();
		second.insert("a".to_string(), "2".to_string());
		second.insert("b".to_string(), "3".to_string());
		let headers = merged("myprov", "https://x.example", false, None, &[Some(first), None, Some(second)]);
		assert_eq!(
			pairs(headers),
			vec![
				("a".to_string(), "2".to_string()),
				("b".to_string(), "3".to_string()),
			]
		);
	}

	#[test]
	fn header_source_overrides_default() {
		let mut source = ProviderHeaders::new();
		source.insert("X-OpenRouter-Title".to_string(), "custom".to_string());
		let headers =
			merged("openrouter", "https://openrouter.ai/api/v1", true, None, &[Some(source)]);
		assert_eq!(
			pairs(headers),
			vec![
				("HTTP-Referer".to_string(), "https://pi.dev".to_string()),
				("X-OpenRouter-Title".to_string(), "custom".to_string()),
				("X-OpenRouter-Categories".to_string(), "cli-agent".to_string()),
			]
		);
	}
}
