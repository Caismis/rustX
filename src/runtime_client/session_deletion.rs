//! Bounded Session-control protocol projections. Native recovery authority is
//! deliberately absent: clients confirm identity and revision, never a workset.
use crate::local_runtime::SessionId;
use serde::{Deserialize, Serialize};

/// Confirmation metadata; counts include the complete native ownership graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientSessionDeletePreview {
    pub session_id: SessionId,
    /// Display name, truncated to at most 256 Unicode scalar values.
    pub name: Option<String>,
    pub target_revision: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub owned_node_count: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub owned_conversation_count: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub owned_child_count: u64,
}

/// Bounded safety summary. Resource identities and storage diagnostics stay native.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientSessionDeletionBlocker {
    ResourceConflict,
    Workspace {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        resource_count: u64,
    },
    InvalidOwnership,
}

/// Bounded external outcomes shared by App Server and local presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientSessionDeletionResult {
    Preview {
        preview: RuntimeClientSessionDeletePreview,
    },
    Deleted {
        session_id: SessionId,
    },
    /// Invalidates confirmation; only a fresh preview supplies a replacement token.
    Stale {
        session_id: SessionId,
    },
    Blocked {
        session_id: SessionId,
        reason: RuntimeClientSessionDeletionBlocker,
    },
    CommittedCleanupPending {
        session_id: SessionId,
    },
    CommittedDurabilityUncertain {
        session_id: SessionId,
    },
    NotFound {
        session_id: SessionId,
    },
}

/// Projects a native deletion outcome onto the bounded App Server wire.
/// Neither recovery capabilities nor storage error strings cross it.
pub(crate) fn project(
    result: crate::local_runtime::session::deletion::SessionDeleteResult,
) -> RuntimeClientSessionDeletionResult {
    use crate::local_runtime::session::deletion::{
        DeletionBlocker as Blocker, DeletionScope, SessionDeleteResult as Native,
    };
    use RuntimeClientSessionDeletionBlocker as Reason;
    use RuntimeClientSessionDeletionResult as Wire;
    match result {
        Native::Preview { preview } => {
            let nodes = preview
                .scopes
                .iter()
                .filter(|s| matches!(s, DeletionScope::Node { .. }))
                .count() as u64;
            let total = preview.scopes.len() as u64;
            Wire::Preview {
                preview: RuntimeClientSessionDeletePreview {
                    session_id: preview.session_id,
                    name: preview.name.map(|name| name.chars().take(256).collect()),
                    target_revision: preview.target_revision,
                    owned_node_count: nodes,
                    owned_conversation_count: total,
                    owned_child_count: total - nodes,
                },
            }
        }
        Native::Deleted { session_id } => Wire::Deleted { session_id },
        Native::NotFound { session_id } => Wire::NotFound { session_id },
        // A stale execution invalidates confirmation, never mints its replacement.
        Native::Stale { session_id, .. } => Wire::Stale { session_id },
        Native::Blocked { session_id, reason } => Wire::Blocked {
            session_id,
            reason: match reason {
                Blocker::ResourceConflict => Reason::ResourceConflict,
                Blocker::Workspace { resources } => Reason::Workspace {
                    resource_count: resources.len() as u64,
                },
                Blocker::InvalidOwnership { .. } => Reason::InvalidOwnership,
            },
        },
        Native::CommittedCleanupPending { record, .. } => Wire::CommittedCleanupPending {
            session_id: record.session_id,
        },
        Native::CommittedDurabilityUncertain { session_id, .. } => {
            Wire::CommittedDurabilityUncertain { session_id }
        }
    }
}
