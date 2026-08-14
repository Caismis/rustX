//! The typed model-facing input contract of the native Glob tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Glob tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GlobInput {
    /// The glob pattern, matched against paths relative to the search root.
    /// `*` and `?` never match a path separator; use `**` to cross
    /// directories.
    pub pattern: String,
    /// The workspace-relative directory to search. Defaults to the whole
    /// workspace.
    pub path: Option<String>,
}

impl GlobInput {
    /// Deserializes one Glob invocation.
    ///
    /// Pattern compilation is an execution concern: an unparsable pattern
    /// is reported by the executor with the globset diagnostic.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::NAME, arguments)
    }
}
