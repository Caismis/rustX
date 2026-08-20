//! The typed model-facing input contract of the native Edit tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Edit tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EditInput {
    /// The absolute path of the file to edit. It must resolve inside the
    /// workspace root; the managed tool-output root is read-only.
    pub file_path: String,
    /// The replacements to apply. Every replacement is matched against the
    /// file as it was before this call, and all of them are applied together
    /// as one change.
    #[schemars(length(min = 1))]
    pub edits: Vec<EditReplacement>,
}

/// One exact-text replacement of an [`EditInput`].
///
/// The model-facing JSON names are `oldText` and `newText`; the Rust field
/// names stay idiomatic and the serde/schema rename is the contract.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EditReplacement {
    /// The exact text to replace. It must occur exactly once in the file;
    /// never a fuzzy or semantic match.
    #[serde(rename = "oldText")]
    #[schemars(rename = "oldText", length(min = 1))]
    pub old_text: String,
    /// The text that replaces it.
    #[serde(rename = "newText")]
    #[schemars(rename = "newText")]
    pub new_text: String,
}

impl EditInput {
    /// Deserializes and semantically validates one Edit invocation.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation; no filesystem access happens here.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        input.validate()?;
        Ok(input)
    }

    /// The tool-specific semantic rules of the edit set. The generated
    /// schema states the same two constraints, so they also hold for a
    /// direct executor call that bypasses the registry preflight.
    ///
    /// An empty edit set describes no transformation at all, and an empty
    /// `oldText` has no exact-match semantics at all.
    fn validate(&self) -> Result<(), String> {
        if !std::path::Path::new(&self.file_path).is_absolute() {
            return Err("edit requires an absolute file_path".to_owned());
        }
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
