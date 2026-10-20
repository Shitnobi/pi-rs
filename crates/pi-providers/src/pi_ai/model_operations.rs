//! Model type operations — port of `pi-ai` `src/utils/model-operations.ts`.
//!
//! TS `assert*` functions throw `ModelsError("provider", …)`; the Rust
//! port returns `Result<_, ModelsError>`. `imageErrorResult` /
//! `classifierErrorResult` mirror the TS JSON byte-for-byte (key order
//! included — these values surface in parity output). `Date.now()` is an
//! explicit `timestamp` parameter (ports never read the ambient clock).

use serde_json::{json, Value};

use crate::error::JsError;
use crate::pi_ai::model::{
	AnyModel, ClassifierModel, ImageModel, Model, ModelInputModality,
};
use crate::pi_ai::models_error::{ModelsError, ModelsErrorKind};
use crate::pi_ai::provider::{AssistantImages, ClassifierContext, ClassifierResult};

/// TS `assertChatModel` — `asserts model is Model<Api>`.
pub fn assert_chat_model(model: &AnyModel) -> Result<&Model, ModelsError> {
	match model {
		AnyModel::Chat(m)
			if m.model_type.as_deref().unwrap_or("chat") == "chat" =>
			Ok(m),
		_ => Err(ModelsError::new(
			ModelsErrorKind::Provider,
			format!("Model {}/{} is not a chat model", model.provider(), model.id()),
		)),
	}
}

/// TS `assertImageModel`.
pub fn assert_image_model(model: &AnyModel) -> Result<&ImageModel, ModelsError> {
	match model {
		AnyModel::Image(m) if m.model_type.as_deref() == Some("image") => Ok(m),
		_ => Err(ModelsError::new(
			ModelsErrorKind::Provider,
			format!("Model {}/{} is not an image model", model.provider(), model.id()),
		)),
	}
}

/// TS `assertClassifierModel`.
pub fn assert_classifier_model(model: &AnyModel) -> Result<&ClassifierModel, ModelsError> {
	match model {
		AnyModel::Classifier(m) if m.model_type.as_deref() == Some("classifier") => Ok(m),
		_ => Err(ModelsError::new(
			ModelsErrorKind::Provider,
			format!("Model {}/{} is not a classifier model", model.provider(), model.id()),
		)),
	}
}

/// TS `assertClassifierInputSupported` — rejects classifier images for
/// models whose catalog entry does not accept image input. The context is
/// the placeholder JSON form; `context.images` is an array when present.
pub fn assert_classifier_input_supported(
	model: &ClassifierModel,
	context: &ClassifierContext,
) -> Result<(), ModelsError> {
	let has_images = context
		.0
		.get("images")
		.and_then(Value::as_array)
		.is_some_and(|images| !images.is_empty());
	if has_images && !model.input.contains(&ModelInputModality::Image) {
		return Err(ModelsError::new(
			ModelsErrorKind::Provider,
			format!("Model {}/{} does not accept image input", model.provider, model.id),
		));
	}
	Ok(())
}

/// `error instanceof Error ? error.message : String(error)` — the Rust
/// error channel is already string-valued.
fn error_message(error: &JsError) -> String {
	error.message.clone()
}

/// TS `imageErrorResult` — an `AssistantImages` carrying the error payload.
pub fn image_error_result(
	model: &ImageModel,
	error: &JsError,
	aborted: bool,
	timestamp: f64,
) -> AssistantImages {
	AssistantImages(json!({
		"api": model.api,
		"provider": model.provider,
		"model": model.id,
		"output": [],
		"stopReason": if aborted { "aborted" } else { "error" },
		"errorMessage": error_message(error),
		"timestamp": timestamp,
	}))
}

/// TS `classifierErrorResult` — a `ClassifierResult` carrying the error.
pub fn classifier_error_result(
	model: &ClassifierModel,
	error: &JsError,
	aborted: bool,
	timestamp: f64,
) -> ClassifierResult {
	ClassifierResult(json!({
		"api": model.api,
		"provider": model.provider,
		"model": model.id,
		"answers": {},
		"stopReason": if aborted { "aborted" } else { "error" },
		"errorMessage": error_message(error),
		"timestamp": timestamp,
	}))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pi_ai::model::ModelInputModality;

	fn chat(id: &str, ty: Option<&str>) -> AnyModel {
		let mut v = json!({
			"id": id, "name": id, "api": "a", "provider": "p", "baseUrl": "",
			"input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"reasoning": false, "contextWindow": 100, "maxTokens": 10
		});
		if let Some(ty) = ty {
			v["type"] = json!(ty);
		}
		AnyModel::Chat(serde_json::from_value(v).unwrap())
	}

	fn image(id: &str, ty: Option<&str>) -> AnyModel {
		let mut v = json!({
			"id": id, "name": id, "api": "a", "provider": "p", "baseUrl": "",
			"input": ["text"], "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"output": ["image"]
		});
		if let Some(ty) = ty {
			v["type"] = json!(ty);
		}
		AnyModel::Image(serde_json::from_value(v).unwrap())
	}

	fn classifier(ty: Option<&str>, input: &[&str]) -> AnyModel {
		let mut v = json!({
			"id": "c", "name": "c", "api": "a", "provider": "p", "baseUrl": "",
			"input": input, "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
			"contextWindow": 100
		});
		if let Some(ty) = ty {
			v["type"] = json!(ty);
		}
		AnyModel::Classifier(serde_json::from_value(v).unwrap())
	}

	#[test]
	fn assert_type_narrowing() {
		// Untagged and tagged chat pass; image with type "chat" fails.
		assert!(assert_chat_model(&chat("m", None)).is_ok());
		assert!(assert_chat_model(&chat("m", Some("chat"))).is_ok());
		assert_eq!(
			assert_chat_model(&chat("m", Some("image"))).unwrap_err().message,
			"Model p/m is not a chat model"
		);
		assert!(assert_image_model(&image("i", Some("image"))).is_ok());
		assert_eq!(
			assert_image_model(&chat("m", None)).unwrap_err().message,
			"Model p/m is not an image model"
		);
		assert!(assert_classifier_model(&classifier(Some("classifier"), &["text"])).is_ok());
		assert_eq!(
			assert_classifier_model(&image("i", Some("image"))).unwrap_err().message,
			"Model p/i is not a classifier model"
		);
	}

	#[test]
	fn classifier_input_support() {
		let text_only = classifier(Some("classifier"), &["text"]);
		let AnyModel::Classifier(text_only) = &text_only else { unreachable!() };
		let image_capable = classifier(Some("classifier"), &["text", "image"]);
		let AnyModel::Classifier(image_capable) = &image_capable else { unreachable!() };

		let no_images = ClassifierContext(json!({"prompt": "x"}));
		let with_images = ClassifierContext(json!({"images": [{"type": "image"}]}));
		let empty_images = ClassifierContext(json!({"images": []}));

		assert!(assert_classifier_input_supported(text_only, &no_images).is_ok());
		assert!(assert_classifier_input_supported(text_only, &empty_images).is_ok());
		assert!(assert_classifier_input_supported(image_capable, &with_images).is_ok());
		assert_eq!(
			assert_classifier_input_supported(text_only, &with_images).unwrap_err().message,
			"Model p/c does not accept image input"
		);
		assert!(text_only.input.contains(&ModelInputModality::Text));
	}

	#[test]
	fn error_result_shapes() {
		let AnyModel::Image(model) = &image("i", Some("image")) else { unreachable!() };
		let result = image_error_result(model, &JsError::new("boom"), false, 1234.5);
		assert_eq!(
			result.0,
			json!({
				"api": "a", "provider": "p", "model": "i", "output": [],
				"stopReason": "error", "errorMessage": "boom", "timestamp": 1234.5
			})
		);
		let aborted = image_error_result(model, &JsError::new("x"), true, 0.0);
		assert_eq!(aborted.0["stopReason"], json!("aborted"));

		let AnyModel::Classifier(cmodel) = &classifier(Some("classifier"), &["text"])
		else {
			unreachable!()
		};
		let result = classifier_error_result(cmodel, &JsError::new("nope"), false, 7.0);
		assert_eq!(result.0["answers"], json!({}));
		assert_eq!(result.0["stopReason"], json!("error"));
		assert_eq!(result.0["errorMessage"], json!("nope"));
	}
}
