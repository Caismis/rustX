//! The typed model-facing input contract of the native Grep tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::limits::{DEFAULT_GREP_MATCHES, MAX_GREP_CONTEXT_LINES, MAX_GREP_MATCHES};
use crate::tools::native::input::decode;

/// The canonical input contract of the Grep tool.
///
/// The model-facing JSON name of the case flag is `ignoreCase`; the Rust
/// field name stays idiomatic and the serde/schema rename is the contract.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GrepInput {
    /// The pattern to search for. A regular expression unless `literal` is
    /// true.
    pub pattern: String,
    /// The workspace-relative directory to search. Defaults to the whole
    /// workspace.
    pub path: Option<String>,
    /// A glob restricting which files are searched, matched against paths
    /// relative to the search root. Defaults to searching every file.
    pub glob: Option<String>,
    /// Whether to match case-insensitively. Defaults to false, a
    /// case-sensitive search.
    #[serde(rename = "ignoreCase")]
    #[schemars(rename = "ignoreCase")]
    pub ignore_case: Option<bool>,
    /// Whether to search for the pattern as literal text instead of a
    /// regular expression. Defaults to false. Use it to avoid escaping
    /// regex metacharacters.
    pub literal: Option<bool>,
    /// How many lines of surrounding context to return around each matching
    /// line. Defaults to 0.
    #[schemars(range(min = 0, max = 20))]
    pub context: Option<u32>,
    /// The maximum number of matches to return. Defaults to 200.
    #[schemars(range(min = 1, max = 2000))]
    pub limit: Option<u32>,
}

impl GrepInput {
    /// Deserializes and semantically validates one Grep invocation.
    ///
    /// Regex and glob compilation are execution concerns: an unparsable
    /// expression is reported by the executor with its diagnostic.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        input.validate()?;
        Ok(input)
    }

    /// The tool-specific semantic rules of the two bounded numeric fields.
    /// The generated schema states the same bounds, so they also hold for a
    /// direct executor call that bypasses the registry preflight.
    fn validate(&self) -> Result<(), String> {
        if let Some(context) = self.context
            && context > MAX_GREP_CONTEXT_LINES
        {
            return Err(format!(
                "grep allows at most {MAX_GREP_CONTEXT_LINES} lines of context"
            ));
        }
        match self.limit {
            Some(0) => Err("grep requires a limit of at least 1".to_owned()),
            Some(limit) if limit as usize > MAX_GREP_MATCHES => {
                Err(format!("grep returns at most {MAX_GREP_MATCHES} matches"))
            }
            _ => Ok(()),
        }
    }

    /// Whether the search is case-insensitive.
    pub(super) fn ignore_case(&self) -> bool {
        self.ignore_case.unwrap_or(false)
    }

    /// Whether the pattern is literal text rather than a regular expression.
    pub(super) fn literal(&self) -> bool {
        self.literal.unwrap_or(false)
    }

    /// The effective number of context lines around each matching line.
    pub(super) fn context(&self) -> usize {
        self.context.unwrap_or(0) as usize
    }

    /// The effective maximum number of returned matches.
    pub(super) fn limit(&self) -> usize {
        self.limit
            .map_or(DEFAULT_GREP_MATCHES, |limit| limit as usize)
            .min(MAX_GREP_MATCHES)
    }
}
