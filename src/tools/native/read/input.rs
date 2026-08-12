//! The typed model-facing input contract of the native Read tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The 1-based first line read when `start_line` is omitted.
const DEFAULT_START_LINE: u64 = 1;

/// The number of lines read when `line_count` is omitted.
const DEFAULT_LINE_COUNT: u64 = 200;

/// The canonical input contract of the Read tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadInput {
    /// The workspace-relative path of the file to read.
    pub path: String,
    /// The 1-based first line to read; defaults to the first line.
    #[schemars(range(min = 1))]
    pub start_line: Option<u64>,
    /// The maximum number of lines to read.
    #[schemars(range(min = 1))]
    pub line_count: Option<u64>,
}

impl ReadInput {
    /// Deserializes and semantically validates one Read invocation.
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

    /// The tool-specific semantic rules of the slicing window. The schema
    /// already rejects `0`; the same rule is enforced on the typed value so
    /// a direct executor call can never slice from a zero line either.
    fn validate(&self) -> Result<(), String> {
        if self.start_line == Some(0) {
            return Err("read requires a 1-based start_line of at least 1".to_owned());
        }
        if self.line_count == Some(0) {
            return Err("read requires a line_count of at least 1".to_owned());
        }
        Ok(())
    }

    /// The effective 1-based first line.
    pub(super) fn start_line(&self) -> u64 {
        self.start_line.unwrap_or(DEFAULT_START_LINE)
    }

    /// The effective maximum number of lines.
    pub(super) fn line_count(&self) -> u64 {
        self.line_count.unwrap_or(DEFAULT_LINE_COUNT)
    }
}
