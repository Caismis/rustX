//! The typed model-facing input contract of the native Read tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The 1-based first line read when `offset` is omitted.
const DEFAULT_OFFSET: u64 = 1;

/// The number of lines read when `limit` is omitted.
const DEFAULT_LIMIT: u64 = 200;

/// The canonical input contract of the Read tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadInput {
    /// The workspace-relative path of the file to read.
    pub path: String,
    /// The 1-based line to start reading from. Defaults to 1, the first
    /// line of the file.
    #[schemars(range(min = 1))]
    pub offset: Option<u64>,
    /// The maximum number of lines to return. Defaults to 200.
    #[schemars(range(min = 1))]
    pub limit: Option<u64>,
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

    /// The tool-specific semantic rules of the line window. The schema
    /// already rejects `0`; the same rule is enforced on the typed value so
    /// a direct executor call can never read from a zero line either.
    fn validate(&self) -> Result<(), String> {
        if self.offset == Some(0) {
            return Err("read requires a 1-based offset of at least 1".to_owned());
        }
        if self.limit == Some(0) {
            return Err("read requires a limit of at least 1".to_owned());
        }
        Ok(())
    }

    /// The effective 1-based first line.
    pub(super) fn offset(&self) -> u64 {
        self.offset.unwrap_or(DEFAULT_OFFSET)
    }

    /// The effective maximum number of lines.
    pub(super) fn limit(&self) -> u64 {
        self.limit.unwrap_or(DEFAULT_LIMIT)
    }
}
