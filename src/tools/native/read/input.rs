//! The typed model-facing input contract of the native Read tool.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The 1-based first line read when `offset` is omitted.
const DEFAULT_OFFSET: u64 = 1;

/// The canonical input contract of the Read tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadInput {
    /// A relative path resolved against the execution cwd, or an absolute
    /// host filesystem path.
    pub path: String,
    /// The 1-based line to start reading from. Zero is normalized to one.
    #[schemars(range(min = 0))]
    pub offset: Option<u64>,
    /// An optional positive line limit. When omitted, Read continues until
    /// its complete-line 2000-line or 50KB boundary.
    #[schemars(range(min = 1))]
    pub limit: Option<u64>,
}

impl ReadInput {
    /// Deserializes and semantically validates one Read invocation.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let input: Self = decode(super::NAME, arguments)?;
        if input.limit == Some(0) {
            return Err("read requires a limit of at least 1".to_owned());
        }
        Ok(input)
    }

    /// The effective 1-based first line.
    pub(super) fn offset(&self) -> u64 {
        self.offset.unwrap_or(DEFAULT_OFFSET).max(DEFAULT_OFFSET)
    }
}

/// The path of one historical canonical Read call, when its recorded
/// arguments identify one.
///
/// This decodes only what compaction file-operation metadata needs — the
/// path — and deliberately not the full input contract: a historical call may
/// have been rejected by a later validation rule, yet the path it named is
/// still a conversation fact. Execution validation stays with
/// [`ReadInput::parse`].
pub(crate) fn operation_path(arguments: &serde_json::Value) -> Option<String> {
    crate::tools::native::input::file_path_argument(arguments)
}
