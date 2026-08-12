//! The typed model-facing input contract of the native Grep tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The workspace-relative search root used when `path` is omitted.
fn default_root() -> String {
    ".".to_owned()
}

/// The file filter used when `glob` is omitted.
fn default_glob() -> String {
    "**/*".to_owned()
}

/// Search is case-sensitive unless the model asks otherwise.
fn default_case_sensitive() -> bool {
    true
}

/// The canonical input contract of the Grep tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GrepInput {
    /// The Rust regex searched for in every matched file.
    pub pattern: String,
    /// The workspace-relative directory the search starts from.
    #[serde(default = "default_root")]
    pub path: String,
    /// The glob filter deciding which workspace files are searched.
    #[serde(default = "default_glob")]
    pub glob: String,
    /// Whether the regex matches case-sensitively.
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
}

impl GrepInput {
    /// Deserializes one Grep invocation.
    ///
    /// Regex and glob compilation are execution concerns: an unparsable
    /// expression is reported by the executor with its diagnostic.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::NAME, arguments)
    }
}
