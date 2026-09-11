//! Bounded Session-control protocol projections. Native recovery authority is
//! deliberately absent: clients confirm identity and revision, never a workset.
use serde::{Deserialize, Serialize};

/// Confirmation metadata; counts include the complete native ownership graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientSessionDeletePreview {
    pub session_id: String,
    /// Display name, truncated to at most 256 Unicode scalar values.
    pub name: Option<String>,
    pub target_revision: String,
    pub owned_node_count: u64,
    pub owned_conversation_count: u64,
    pub owned_child_count: u64,
}

/// Bounded safety summary. Resource identities and storage diagnostics stay native.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientSessionDeletionBlocker {
    CurrentSession,
    InUse,
    Workspace { resource_count: u64 },
    InvalidOwnership,
}

/// External outcomes, independently owned by the Runtime Client protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientSessionDeletionResult {
    Preview {
        preview: RuntimeClientSessionDeletePreview,
    },
    Deleted {
        session_id: String,
    },
    Stale {
        session_id: String,
        actual_revision: String,
    },
    Blocked {
        session_id: String,
        reason: RuntimeClientSessionDeletionBlocker,
    },
    CommittedCleanupPending {
        session_id: String,
    },
    CommittedDurabilityUncertain {
        session_id: String,
    },
    NotFound {
        session_id: String,
    },
}
