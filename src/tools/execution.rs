//! The model-facing asynchronous execution control envelope (Issue #162).
//!
//! `execution` is the **single model-facing observation and cancellation
//! control plane** for conversation-owned asynchronous executions. This
//! module owns only the small typed envelope both creation paths and the
//! intrinsic share:
//!
//! - the explicit [`ExecutionKind`] of one execution;
//! - the typed [`ExecutionHandle`] every model-visible creation result
//!   returns and every `execution(status|cancel)` target names.
//!
//! It deliberately owns **no lifecycle state, no registry, no cancellation
//! implementation, no durability, and no result channel**. The domain
//! registries — [`ConversationBackgroundRegistry`] for detached tool
//! executions and [`SubagentRegistry`] for subagent children — remain the
//! sole authorities for lifecycle, cancellation, durability, settlement,
//! and terminal publication. This envelope is identity only: it never
//! becomes a second source of truth.
//!
//! The tagged snapshot *response* of the intrinsic is owned by the
//! intrinsic itself (`crate::tools::native::execution`), which converts
//! the authoritative domain snapshot into a bounded tagged model-facing
//! result.
//!
//! [`ConversationBackgroundRegistry`]: crate::tools::background::ConversationBackgroundRegistry
//! [`SubagentRegistry`]: crate::runtime::subagent::SubagentRegistry

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::runtime::identity::{SubagentId, ToolExecutionId};

/// The explicit kind of one conversation-owned asynchronous execution.
///
/// The kind is always explicit in model-facing surfaces: creation results
/// tag their handle with it, `execution(status|cancel)` targets carry it,
/// and Agent Status renders it. The runtime never infers a kind from an id
/// string and never falls through from one domain to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKind {
    // A detached tool execution owned by the conversation background registry.
    Tool,
    // An asynchronous one-shot subagent child owned by the subagent registry.
    Subagent,
}

impl ExecutionKind {
    /// The stable model-facing name of the kind.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Subagent => "subagent",
        }
    }
}

/// The typed model-facing handle of one conversation-owned asynchronous
/// execution.
///
/// The handle is the canonical continuation affordance: every model-visible
/// creation result returns exactly one, and every `execution(status|cancel)`
/// target names one. It carries the explicit kind plus the owning domain's
/// model-facing id string — never a guessed kind, never a bare id. The
/// domain identity types (`ToolExecutionId`, `SubagentId`) remain internal
/// to their registries; the handle is their model-facing projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionHandle {
    /// The explicit execution kind.
    pub kind: ExecutionKind,
    /// The owning domain's model-facing id string.
    pub id: String,
}

impl ExecutionHandle {
    /// The handle of one detached tool execution.
    #[must_use]
    pub fn tool(execution_id: &ToolExecutionId) -> Self {
        Self {
            kind: ExecutionKind::Tool,
            id: execution_id.to_string(),
        }
    }

    /// The handle of one subagent child.
    #[must_use]
    pub fn subagent(subagent_id: &SubagentId) -> Self {
        Self {
            kind: ExecutionKind::Subagent,
            id: subagent_id.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionHandle, ExecutionKind};
    use crate::runtime::identity::{SubagentId, ToolExecutionId};

    #[test]
    fn tool_handle_is_a_tagged_kind_id_pair() {
        let handle = ExecutionHandle::tool(&ToolExecutionId::new("exec_1"));
        assert_eq!(handle.kind, ExecutionKind::Tool);
        assert_eq!(handle.id, "exec_1");
        assert_eq!(
            serde_json::to_value(&handle).expect("handle serializes"),
            serde_json::json!({"kind": "tool", "id": "exec_1"})
        );
    }

    #[test]
    fn subagent_handle_is_a_tagged_kind_id_pair() {
        let handle = ExecutionHandle::subagent(&SubagentId::new("conversation-1-subagent-2"));
        assert_eq!(handle.kind, ExecutionKind::Subagent);
        assert_eq!(handle.id, "conversation-1-subagent-2");
        assert_eq!(
            serde_json::to_value(&handle).expect("handle serializes"),
            serde_json::json!({"kind": "subagent", "id": "conversation-1-subagent-2"})
        );
    }

    #[test]
    fn kind_names_are_the_closed_model_vocabulary() {
        assert_eq!(ExecutionKind::Tool.name(), "tool");
        assert_eq!(ExecutionKind::Subagent.name(), "subagent");
    }
}
