//! The typed model-facing input contract of the `background_task`
//! runtime intrinsic.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::native::input::decode;

/// The canonical input contract of the `background_task` intrinsic.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BackgroundTaskInput {
    /// The execution id of a background execution of this conversation.
    pub execution_id: String,
    /// The requested operation.
    pub action: BackgroundTaskAction,
}

/// The two operations of the intrinsic. There is intentionally no list and
/// no delete operation.
///
/// The variants carry no per-variant documentation on purpose: the model
/// facing surface stays the canonical `{"type": "string", "enum":
/// ["status", "cancel"]}` schema instead of a documented `oneOf` union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum BackgroundTaskAction {
    // Returns the canonical structured snapshot of the execution.
    Status,
    // Requests cancellation and returns the canonical snapshot afterwards.
    Cancel,
}

impl BackgroundTaskInput {
    /// Deserializes one `background_task` invocation.
    ///
    /// Execution-id resolution is an execution concern: an unknown id
    /// (including another conversation's id) is a normal failed tool
    /// result produced by the executor, not an input contract violation.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation, including an action outside the two supported
    /// operations.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::BACKGROUND_TASK_NAME, arguments)
    }
}
