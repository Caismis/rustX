//! The model-facing asynchronous execution control envelope (Issue #162).
//!
//! `execution` is the **single model-facing observation and cancellation
//! control plane** for conversation-owned asynchronous executions. This
//! module owns only the small typed envelope both creation paths and the
//! intrinsic share:
//!
//! - the explicit [`ExecutionKind`] of one execution;
//! - the typed [`ExecutionHandle`] every model-visible creation result
//!   returns and every `execution(status|cancel)` target names;
//! - the single global [`MAX_LISTED_EXECUTIONS`] response bound of
//!   `execution(list)` (Issue #180), which is a model-facing response
//!   policy rather than any domain's invariant.
//!
//! It deliberately owns **no lifecycle state, no registry, no read model,
//! no cancellation implementation, no durability, and no result channel**.
//! The domain registries — [`ConversationBackgroundRegistry`] for detached
//! tool executions and [`SubagentRegistry`] for subagent children — remain
//! the sole authorities for lifecycle, cancellation, durability,
//! settlement, and terminal publication, and each owns its **own** bounded
//! discovery read model ([`BackgroundExecutionListing`],
//! [`SubagentListing`]). Neither domain depends on this module for that:
//! the dependency runs one way only, from the model-facing control plane
//! down to the domain authorities. This envelope is identity only: it
//! never becomes a second source of truth.
//!
//! The tagged snapshot and listing *responses* of the intrinsic are owned
//! by the intrinsic itself (`crate::tools::native::execution`), which
//! converts the authoritative domain read models into bounded tagged
//! model-facing results.
//!
//! [`ConversationBackgroundRegistry`]: crate::tools::background::ConversationBackgroundRegistry
//! [`BackgroundExecutionListing`]: crate::tools::background::BackgroundExecutionListing
//! [`SubagentRegistry`]: crate::runtime::subagent::SubagentRegistry
//! [`SubagentListing`]: crate::runtime::subagent::SubagentListing

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
///
/// The handle is also the `execution` intrinsic's typed target input: the
/// model echoes back the exact handle a creation result returned, so the
/// input contract and the continuation affordance are one type. `Deserialize`
/// and `JsonSchema` exist for that input role; the handle is always
/// serialized in creation results and deserialized only as a control-plane
/// target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

/// The global bound on how many executions one `execution(list)` response
/// may carry (Issue #180).
///
/// The bound is a single **global** response bound, not a per-domain quota:
/// the intrinsic merges the two domain listings and keeps at most this many
/// entries in total, so the externally visible bound is exactly this number
/// however the matching executions are distributed across the domains.
///
/// It lives here, on the model-facing control envelope, because it is a
/// property of the `execution(list)` *response* and of nothing else. It is
/// deliberately not a domain invariant: a registry bounds only how much it
/// materializes, from an explicit limit its caller supplies, and neither
/// domain knows what the model-facing response is allowed to carry.
pub const MAX_LISTED_EXECUTIONS: usize = 64;

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

    /// The layering rule of this envelope, asserted where it is stated
    /// (Issue #180).
    ///
    /// `execution` is the model-facing control plane; the subagent runtime
    /// is a domain authority underneath it. The control plane may name the
    /// domain, never the reverse — and because a reverse edge compiles
    /// perfectly well, the only thing that keeps it out is a test that
    /// looks. Each domain owns its own bounded listing read model
    /// (`SubagentListing`, `BackgroundExecutionListing`) precisely so it
    /// never has to reach up here for one.
    #[test]
    fn the_subagent_domain_never_depends_on_this_model_facing_control_plane() {
        let subagent =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime/subagent");
        let mut sources = vec![subagent.with_extension("rs")];
        sources.extend(
            std::fs::read_dir(&subagent)
                .expect("subagent domain directory")
                .map(|entry| entry.expect("directory entry").path())
                .filter(|path| path.extension().is_some_and(|kind| kind == "rs")),
        );
        assert!(
            sources.len() > 1,
            "the domain has more than one source file"
        );

        for source in sources {
            let text = std::fs::read_to_string(&source).expect("subagent domain source");
            assert!(
                !text.contains("tools::execution"),
                "{} names the model-facing execution control plane: the subagent \
                 domain must own its own read models, never depend on the control \
                 plane that consumes them",
                source.display()
            );
        }
    }
}
