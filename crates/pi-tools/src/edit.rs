//! The `edit` tool. Port of `src/core/tools/edit.ts`.

use serde::Deserialize;

use pi_core::edit_diff::{
    apply_edits_to_normalized_content, detect_line_ending, generate_diff_string,
    generate_unified_patch, normalize_to_lf, restore_line_endings, Edit,
};
use pi_core::paths::resolve_to_cwd;
use pi_core::text::split_bom;

use crate::node_fs;
use crate::{ToolError, ToolOutput, ToolResult};

#[derive(Debug, Deserialize)]
pub struct EditToolInput {
    pub path: String,
    pub edits: Vec<EditInput>,
}

#[derive(Debug, Deserialize)]
pub struct EditInput {
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// Port of `prepareEditArguments` quirks:
/// - `edits` sent as a JSON string (parsed to array or single edit)
/// - single edit object instead of a one-element array
/// - legacy top-level `oldText`/`newText` folded into `edits`
pub fn prepare_edit_arguments(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    if let Some(edits) = obj.get_mut("edits") {
        if let Some(s) = edits.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                if parsed.is_array() {
                    *edits = parsed;
                } else if is_single_edit_input(&parsed) {
                    *edits = serde_json::Value::Array(vec![parsed]);
                }
            }
        } else if is_single_edit_input(edits) {
            let single = edits.clone();
            *edits = serde_json::Value::Array(vec![single]);
        }
    }

    let legacy_old = obj.get("oldText").and_then(|v| v.as_str()).map(|s| s.to_string());
    let legacy_new = obj.get("newText").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let (Some(old_text), Some(new_text)) = (legacy_old, legacy_new) {
        obj.remove("oldText");
        obj.remove("newText");
        let mut edits = obj
            .get("edits")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        edits.push(serde_json::json!({ "oldText": old_text, "newText": new_text }));
        obj.insert("edits".to_string(), serde_json::Value::Array(edits));
    }
}

fn is_single_edit_input(value: &serde_json::Value) -> bool {
    value.is_object()
        && value.get("oldText").and_then(|v| v.as_str()).is_some()
        && value.get("newText").and_then(|v| v.as_str()).is_some()
}

/// Port of `validateEditInput`.
fn validate_edit_input(input: &EditToolInput) -> Result<(&str, Vec<Edit>), ToolError> {
    if input.edits.is_empty() {
        return Err(ToolError::new(
            "Edit tool input is invalid. edits must contain at least one replacement.",
        ));
    }
    let edits = input
        .edits
        .iter()
        .map(|e| Edit { old_text: e.old_text.clone(), new_text: e.new_text.clone() })
        .collect();
    Ok((input.path.as_str(), edits))
}

pub fn edit_execute(cwd: &str, input: &EditToolInput) -> ToolResult {
    let (path, edits) = validate_edit_input(input)?;
    let absolute_path = resolve_to_cwd(path, cwd);

    // Check if file exists and is readable + writable.
    // Node: fsAccess(absolutePath, constants.R_OK | constants.W_OK)
    if let Err(err) = std::fs::OpenOptions::new().read(true).write(true).open(&absolute_path) {
        let code = node_fs::node_code(&err);
        return Err(ToolError::new(format!(
            "Could not edit file: {}. Error code: {}.",
            path, code
        )));
    }

    // Read the file.
    let raw_content = match std::fs::read(&absolute_path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(err) => {
            let code = node_fs::node_code(&err);
            return Err(ToolError::new(format!(
                "Could not edit file: {}. Error code: {}.",
                path, code
            )));
        }
    };

    // Strip BOM before matching.
    let (bom, content) = split_bom(&raw_content);
    let original_ending = detect_line_ending(content);
    let normalized_content = normalize_to_lf(content);
    let applied = apply_edits_to_normalized_content(&normalized_content, &edits, path)
        .map_err(ToolError::new)?;

    let final_content = format!("{}{}", bom, restore_line_endings(&applied.new_content, original_ending));
    if let Err(message) = node_fs::node_write_file(&absolute_path, &final_content) {
        return Err(ToolError::new(message));
    }

    let diff_result = generate_diff_string(&applied.base_content, &applied.new_content, 4);
    let patch = generate_unified_patch(path, &applied.base_content, &applied.new_content, 4);

    let details = serde_json::json!({
        "diff": diff_result.diff,
        "patch": patch,
        "firstChangedLine": diff_result.first_changed_line,
    });

    Ok(ToolOutput::text_with_details(
        format!("Successfully replaced {} block(s) in {}.", edits.len(), path),
        details,
    ))
}
