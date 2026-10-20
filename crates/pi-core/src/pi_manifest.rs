//! Port of `src/core/pi-manifest.ts`.
//!
//! `readPiManifest` in the TS reads a `package.json` from disk with
//! `readFileSync` and parses its `pi` field; this port takes the file
//! CONTENT as an argument so no ambient filesystem is touched. Callers
//! inject the bytes (or a reader closure) at their boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::text::strip_bom;

/// `PiManifest`: the `pi` field of a `package.json`, declaring resources the
/// package provides. Fields whose entries are not all strings are omitted,
/// like the TS loop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiManifest {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub extensions: Option<Vec<String>>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub skills: Option<Vec<String>>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub prompts: Option<Vec<String>>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub themes: Option<Vec<String>>,
}

/// `RESOURCE_FIELDS`.
const RESOURCE_FIELDS: [&str; 4] = ["extensions", "skills", "prompts", "themes"];

/// `isObject` from the TS: a JSON object, not an array.
fn is_object(value: &Value) -> bool {
	value.is_object()
}

/// `readPiManifest`: parse the text of a `package.json` and return its `pi`
/// manifest. Returns `None` when the text is not valid JSON, the document is
/// not an object, or it has no `pi` object — the TS catches all of those
/// (plus the `readFileSync` failure this port pushes to the caller).
pub fn read_pi_manifest(package_json_content: &str) -> Option<PiManifest> {
	let pkg: Value = serde_json::from_str(strip_bom(package_json_content)).ok()?;
	let pi = pkg.get("pi")?;
	if !is_object(&pkg) || !is_object(pi) {
		return None;
	}

	let mut manifest = PiManifest::default();
	for field in RESOURCE_FIELDS {
		let entries = pi.get(field);
		if let Some(strings) = entries.and_then(string_array) {
			match field {
				"extensions" => manifest.extensions = Some(strings),
				"skills" => manifest.skills = Some(strings),
				"prompts" => manifest.prompts = Some(strings),
				"themes" => manifest.themes = Some(strings),
				_ => {}
			}
		}
	}
	Some(manifest)
}

/// `Array.isArray(entries) && entries.every((entry) => typeof entry === "string")`.
fn string_array(entries: &Value) -> Option<Vec<String>> {
	let array = entries.as_array()?;
	if array.iter().all(|entry| entry.is_string()) {
		Some(array.iter().map(|entry| entry.as_str().unwrap().to_string()).collect())
	} else {
		None
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn reads_all_resource_fields() {
		let content = json!({
			"name": "pkg",
			"pi": {
				"extensions": ["ext-a"],
				"skills": ["skill-a", "skill-b"],
				"prompts": [],
				"themes": ["theme-a"],
			}
		})
		.to_string();
		let manifest = read_pi_manifest(&content).unwrap();
		assert_eq!(
			manifest,
			PiManifest {
				extensions: Some(vec!["ext-a".to_string()]),
				skills: Some(vec!["skill-a".to_string(), "skill-b".to_string()]),
				prompts: Some(vec![]),
				themes: Some(vec!["theme-a".to_string()]),
			}
		);
	}

	#[test]
	fn strips_a_bom() {
		let content = "\u{FEFF}{\"pi\":{\"skills\":[\"a\"]}}";
		let manifest = read_pi_manifest(content).unwrap();
		assert_eq!(manifest.skills, Some(vec!["a".to_string()]));
	}

	#[test]
	fn empty_pi_object_gives_empty_manifest() {
		let manifest = read_pi_manifest("{\"pi\":{}}").unwrap();
		assert_eq!(manifest, PiManifest::default());
		// The manifest is `{}` — no resource fields serialized.
		assert_eq!(serde_json::to_value(&manifest).unwrap(), json!({}));
	}

	#[test]
	fn missing_pi_returns_none() {
		assert!(read_pi_manifest("{\"name\":\"pkg\"}").is_none());
	}

	#[test]
	fn non_object_pi_returns_none() {
		assert!(read_pi_manifest("{\"pi\":[]}").is_none());
		assert!(read_pi_manifest("{\"pi\":\"x\"}").is_none());
		assert!(read_pi_manifest("[1,2]").is_none());
		assert!(read_pi_manifest("\"str\"").is_none());
	}

	#[test]
	fn invalid_json_returns_none() {
		assert!(read_pi_manifest("{oops").is_none());
		assert!(read_pi_manifest("").is_none());
	}

	#[test]
	fn non_string_entries_are_omitted() {
		let manifest = read_pi_manifest(
			"{\"pi\":{\"skills\":[\"a\",1],\"prompts\":[\"p\"],\"themes\":\"not-an-array\"}}",
		)
		.unwrap();
		assert_eq!(manifest.skills, None);
		assert_eq!(manifest.prompts, Some(vec!["p".to_string()]));
		assert_eq!(manifest.themes, None);
	}
}
