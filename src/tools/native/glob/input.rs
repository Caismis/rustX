//! The typed model-facing input contract of the native Glob tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the Glob tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GlobInput {
    /// The glob pattern, matched against paths relative to the search root.
    pub pattern: String,
    /// An optional relative or absolute directory search root. Omit it to
    /// search the execution cwd.
    pub path: Option<String>,
    /// The maximum number of results to return. Defaults to 1000; larger
    /// values are accepted and remain bounded by the tool's byte budget.
    #[schemars(range(min = 1))]
    pub limit: Option<u64>,
}

impl GlobInput {
    /// Deserializes and semantically validates one Glob invocation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        if input.limit == Some(0) {
            return Err("glob requires a limit of at least 1".to_owned());
        }
        Ok(input)
    }

    pub(super) fn limit(&self) -> u64 {
        self.limit
            .unwrap_or(crate::tools::limits::DEFAULT_GLOB_RESULTS)
    }
}
