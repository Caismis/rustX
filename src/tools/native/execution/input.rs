//! The typed model-facing input contract of the `execution` runtime
//! intrinsic (Issue #162).

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::execution::ExecutionKind;
use crate::tools::native::input::decode;

/// The canonical input contract of the `execution` intrinsic.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutionInput {
    /// The requested operation.
    pub action: ExecutionAction,
    /// The explicit tagged target of the operation.
    pub target: ExecutionTarget,
}

/// The two operations of the intrinsic. There is intentionally no list, no
/// wait, no delete, and no generic scheduling API.
///
/// The variants carry no per-variant documentation on purpose: the model
/// facing surface stays the canonical `{"type": "string", "enum":
/// ["status", "cancel"]}` schema instead of a documented `oneOf` union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExecutionAction {
    // Returns the canonical structured snapshot of the execution.
    Status,
    // Requests cancellation and returns the canonical snapshot afterwards.
    Cancel,
}

/// The explicit tagged target: kind + id.
///
/// The kind is always explicit. The intrinsic never infers a kind from an
/// id prefix and never falls through from one registry to another; a
/// mismatched kind/id pair is resolved by the selected domain authority as
/// an unknown id.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutionTarget {
    /// The explicit execution kind.
    pub kind: ExecutionKind,
    /// The owning domain's model-facing id string.
    pub id: String,
}

impl ExecutionInput {
    /// Deserializes one `execution` invocation.
    ///
    /// Id resolution is an execution concern: an unknown id (including
    /// another conversation's id) is a normal failed tool result produced by
    /// the executor, not an input contract violation.
    ///
    /// # Errors
    ///
    /// Returns the deterministic rejection message of the first input
    /// contract violation, including an action outside the two supported
    /// operations or an unknown execution kind.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::EXECUTION_TOOL_NAME, arguments)
    }
}
