//! The typed model-facing input contract of the native Edit tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Edit tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EditInput {
    /// A relative path resolved against the execution cwd, or an absolute
    /// host filesystem path.
    pub path: String,
    /// Replacements matched against one original file snapshot and committed
    /// together as one change.
    #[schemars(length(min = 1))]
    pub edits: Vec<EditReplacement>,
}

/// One text replacement of an [`EditInput`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EditReplacement {
    /// The source text to replace. Exact matching is preferred; when exact
    /// matching fails, the executor applies its documented NFKC-based fuzzy
    /// normalization without guessing between ambiguous candidates.
    #[serde(rename = "oldText")]
    #[schemars(rename = "oldText", length(min = 1))]
    pub old_text: String,
    /// The replacement text.
    #[serde(rename = "newText")]
    #[schemars(rename = "newText")]
    pub new_text: String,
}

impl EditInput {
    /// Deserializes and semantically validates one canonical Edit invocation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        input.validate()?;
        Ok(input)
    }

    fn validate(&self) -> Result<(), String> {
        if self.edits.is_empty() {
            return Err("edit requires at least one replacement".to_owned());
        }
        if let Some(index) = self
            .edits
            .iter()
            .position(|replacement| replacement.old_text.is_empty())
        {
            return Err(format!(
                "edit requires a non-empty oldText; edits[{index}] is empty"
            ));
        }
        Ok(())
    }
}

/// Normalizes known model-output deviations into the one canonical Edit
/// object before schema validation. The malformed spellings are deliberately
/// not part of the generated provider schema.
pub(super) fn normalize_arguments(
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(object) = arguments.as_object() else {
        return Ok(arguments.clone());
    };
    let mut normalized = object.clone();

    if let Some(edits) = normalized.get("edits").cloned() {
        let canonical_edits = match edits {
            serde_json::Value::String(encoded) => serde_json::from_str(&encoded).map_err(|_| {
                "invalid edit arguments: edits must be a JSON array or edit object".to_owned()
            })?,
            value => value,
        };
        let canonical_edits = match canonical_edits {
            serde_json::Value::Object(_) => serde_json::Value::Array(vec![canonical_edits]),
            _ => canonical_edits,
        };
        normalized.insert("edits".to_owned(), canonical_edits);
    }

    let top_level_edit = normalized
        .get("oldText")
        .and_then(serde_json::Value::as_str)
        .zip(
            normalized
                .get("newText")
                .and_then(serde_json::Value::as_str),
        )
        .map(|(old_text, new_text)| serde_json::json!({"oldText": old_text, "newText": new_text}));
    if let Some(edit) = top_level_edit {
        let edits = normalized
            .remove("edits")
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
        let mut edits = match edits {
            serde_json::Value::Array(edits) => edits,
            serde_json::Value::Object(edit) => vec![serde_json::Value::Object(edit)],
            other => vec![other],
        };
        edits.push(edit);
        normalized.insert("edits".to_owned(), serde_json::Value::Array(edits));
        normalized.remove("oldText");
        normalized.remove("newText");
    }

    Ok(serde_json::Value::Object(normalized))
}
