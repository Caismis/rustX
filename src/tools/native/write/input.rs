//! The typed model-facing input contract of the native Write tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Write tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WriteInput {
    /// The workspace-relative path of the file to create or replace.
    pub path: String,
    /// The complete new UTF-8 content of the file.
    pub content: String,
}

impl WriteInput {
    /// Deserializes one Write invocation.
    ///
    /// Write has no semantic input rules beyond its contract: the parent
    /// directory rule and the workspace boundary are execution concerns.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::NAME, arguments)
    }
}
