//! The typed model-facing input contract of the native Edit tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Edit tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EditInput {
    /// The workspace-relative path of the file to edit.
    pub path: String,
    /// The exact text to replace; never a fuzzy or semantic match.
    #[schemars(length(min = 1))]
    pub old_text: String,
    /// The replacement text.
    pub new_text: String,
    /// Whether every exact match is replaced instead of exactly one.
    #[serde(default)]
    pub replace_all: bool,
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

    /// The tool-specific semantic rule of the replacement anchor: an empty
    /// `old_text` has no exact match semantics at all. The generated schema
    /// states the same constraint, so this rule also holds for a direct
    /// executor call.
    fn validate(&self) -> Result<(), String> {
        if self.old_text.is_empty() {
            return Err("edit requires a non-empty old_text".to_owned());
        }
        Ok(())
    }
}
