//! The typed model-facing input contract of the native Write tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Write tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WriteInput {
    /// A relative path resolved against the execution cwd, or an absolute
    /// host filesystem path.
    pub path: String,
    /// The complete new UTF-8 content of the file.
    pub content: String,
}

impl WriteInput {
    /// Deserializes one canonical Write invocation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::NAME, arguments)
    }
}
