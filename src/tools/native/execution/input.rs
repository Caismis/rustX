//! The typed model-facing input contract of the `execution` runtime
//! intrinsic (Issue #162, extended for discovery by Issue #180).

use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::execution::{ExecutionHandle, ExecutionKind};
use crate::tools::native::input::decode;

/// The canonical input contract of the `execution` intrinsic.
///
/// The contract is **action-tagged**: the action selects the variant, and
/// each variant carries exactly the fields that action needs. `status` and
/// `cancel` name one execution and therefore require a `target`; `list`
/// names none and therefore accepts no `target` at all. Encoding that with
/// one optional `target` field would have made two different requests
/// spellable the same way and left `{"action": "status"}` structurally
/// legal; the tagged union makes every ill-formed combination a schema
/// violation instead of a runtime special case.
///
/// The target of `status`/`cancel` is the canonical typed
/// [`ExecutionHandle`] itself — the same handle every creation result
/// returns — so the model echoes back the exact handle it was given instead
/// of reconstructing a structurally identical shape. The kind is always
/// explicit; the intrinsic never infers a kind from an id prefix and never
/// falls through from one registry to another.
///
/// The root carries an explicit `"type": "object"` (`extend`) because the
/// generated union is a root `oneOf`, and the canonical tool-schema policy
/// requires every model-facing input schema to be a root object schema.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub(super) enum ExecutionInput {
    /// Returns the canonical structured snapshot of one execution.
    Status {
        /// The explicit tagged target of the operation.
        target: ExecutionHandle,
    },
    /// Requests cancellation and returns the canonical snapshot afterwards.
    Cancel {
        /// The explicit tagged target of the operation.
        target: ExecutionHandle,
    },
    /// Returns the bounded, deterministically ordered listing of the
    /// conversation's own executions.
    List {
        /// The optional bounded filter; omitted means no filter.
        #[serde(default)]
        filter: ExecutionFilter,
    },
}

/// The closed filter vocabulary of `execution(list)` (Issue #180).
///
/// Both fields are optional and omission is the only spelling of "do not
/// filter on this axis". There is deliberately no query expression, no sort
/// key, no cursor, no label selector, and no conversation selector: the
/// listing is always the calling conversation's own executions, and its
/// order and bound are runtime-owned rather than caller-selectable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutionFilter {
    /// Restricts the listing to one execution domain; omitted lists both.
    #[serde(default)]
    pub kind: Option<ExecutionKind>,
    /// When true, restricts the listing to lifecycle-active executions;
    /// omitted or false lists active and terminal executions alike.
    #[serde(default)]
    pub active_only: Option<bool>,
}

impl ExecutionFilter {
    /// Whether the filter restricts the listing to active executions.
    pub(super) fn active_only(self) -> bool {
        self.active_only.unwrap_or(false)
    }
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
    /// contract violation, including an action outside the three supported
    /// operations, an unknown execution kind, a `target` on `list`, a
    /// missing `target` on `status`/`cancel`, and any unknown field.
    pub(super) fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        decode(super::EXECUTION_TOOL_NAME, arguments)
    }
}
