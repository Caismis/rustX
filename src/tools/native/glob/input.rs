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
    /// The absolute directory to search. It must resolve inside the
    /// workspace root or the read-only managed tool-output root. Omit it to
    /// search the workspace root.
    pub path: Option<String>,
}

impl GlobInput {
    /// Deserializes and semantically validates one Glob invocation.
    ///
    /// Pattern compilation is an execution concern: an unparsable pattern
    /// is reported by the executor with the globset diagnostic. The
    /// absolute-locator rule is enforced on the typed value so a direct
    /// executor call can never bypass it.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        if let Some(path) = &input.path
            && !std::path::Path::new(path).is_absolute()
        {
            return Err("glob requires an absolute path when one is supplied".to_owned());
        }
        Ok(input)
    }
}
