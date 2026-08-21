//! The typed model-facing input contract of the native Grep tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::limits::MAX_GREP_CONTEXT_LINES;
use crate::tools::native::input::decode;

/// The canonical input contract of the Grep tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GrepInput {
    /// The pattern to search for. A regular expression unless `literal` is
    /// true.
    pub pattern: String,
    /// An optional relative or absolute directory/file search root. Omit it
    /// to search the execution cwd.
    pub path: Option<String>,
    /// A glob restricting which files are searched, matched against paths
    /// relative to the search root.
    pub glob: Option<String>,
    /// Whether to match case-insensitively.
    #[serde(rename = "ignoreCase")]
    #[schemars(rename = "ignoreCase")]
    pub ignore_case: Option<bool>,
    /// Whether to search for literal text instead of a regular expression.
    pub literal: Option<bool>,
    /// How many lines of surrounding context to return on each side.
    #[schemars(range(min = 0, max = 20))]
    pub context: Option<u32>,
    /// The maximum number of matching lines to return. Defaults to 100.
    /// Larger values are accepted and are bounded by the tool's finite
    /// content budget rather than by a schema maximum.
    #[schemars(range(min = 1))]
    pub limit: Option<u64>,
}

impl GrepInput {
    /// Deserializes and semantically validates one Grep invocation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        if input.context.unwrap_or(0) > MAX_GREP_CONTEXT_LINES {
            return Err(format!(
                "grep allows at most {MAX_GREP_CONTEXT_LINES} lines of context"
            ));
        }
        if input.limit == Some(0) {
            return Err("grep requires a limit of at least 1".to_owned());
        }
        Ok(input)
    }

    pub(super) fn ignore_case(&self) -> bool {
        self.ignore_case.unwrap_or(false)
    }

    pub(super) fn literal(&self) -> bool {
        self.literal.unwrap_or(false)
    }

    pub(super) fn context(&self) -> usize {
        self.context.unwrap_or(0) as usize
    }

    pub(super) fn limit(&self) -> u64 {
        self.limit
            .unwrap_or(crate::tools::limits::DEFAULT_GREP_MATCHES)
    }
}
