//! Port of `src/core/diagnostics.ts`.
//!
//! The TS module only declares the `ResourceCollision` and
//! `ResourceDiagnostic` interfaces. They were already ported (as minimal
//! local structs) in [`crate::prompt_templates`], which `crate::skills`
//! consumes; this module re-exports them so `diagnostics` is the canonical
//! import path, matching the TS module layout, without duplicating the
//! types.

pub use crate::prompt_templates::{DiagnosticType, ResourceCollision, ResourceDiagnostic};

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn collision_serializes_with_ts_field_names() {
		let collision = ResourceCollision {
			resource_type: "skill".to_string(),
			name: "build".to_string(),
			winner_path: "/a".to_string(),
			loser_path: "/b".to_string(),
			winner_source: Some("npm:foo".to_string()),
			loser_source: None,
		};
		let json = serde_json::to_value(&collision).unwrap();
		assert_eq!(
			json,
			serde_json::json!({
				"resourceType": "skill",
				"name": "build",
				"winnerPath": "/a",
				"loserPath": "/b",
				"winnerSource": "npm:foo"
			})
		);
	}

	#[test]
	fn diagnostic_serializes_with_ts_field_names() {
		let diagnostic = ResourceDiagnostic {
			r#type: DiagnosticType::Warning,
			message: "m".to_string(),
			path: None,
			collision: None,
		};
		let json = serde_json::to_value(&diagnostic).unwrap();
		assert_eq!(json, serde_json::json!({ "type": "warning", "message": "m" }));
	}
}
