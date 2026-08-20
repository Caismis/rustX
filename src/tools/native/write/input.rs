//! The typed model-facing input contract of the native Write tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Write tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WriteInput {
    /// The absolute path of the file to create or replace. It must resolve
    /// inside the workspace root; the managed tool-output root is
    /// read-only.
    pub file_path: String,
    /// The complete new UTF-8 content of the file.
    pub content: String,
}

impl WriteInput {
    /// Deserializes and semantically validates one Write invocation.
    ///
    /// The parent directory rule and the workspace boundary are execution
    /// concerns; the absolute-locator rule is enforced on the typed value
    /// so a direct executor call can never bypass it.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        if !std::path::Path::new(&input.file_path).is_absolute() {
            return Err("write requires an absolute file_path".to_owned());
        }
        Ok(input)
    }
}
