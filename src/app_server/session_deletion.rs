//! App Server-owned bounded Session deletion wire projections. Native recovery authority is
//! deliberately absent: clients confirm identity and revision, never a workset.
// Schema names are pinned to the published App Server v39 definitions. They
// do not imply Runtime Client ownership or expose a Runtime Client alias.
use crate::local_runtime::SessionId;
use serde::{Deserialize, Serialize};

/// Confirmation metadata; counts include the complete native ownership graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "RuntimeClientSessionDeletePreview")]
pub struct SessionDeletePreview {
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
#[schemars(rename = "RuntimeClientSessionDeletionBlocker")]
pub enum SessionDeletionBlocker {
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
#[schemars(rename = "RuntimeClientSessionDeletionResult")]
pub enum SessionDeletionResult {
    Preview {
        preview: SessionDeletePreview,
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
        reason: SessionDeletionBlocker,
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
) -> SessionDeletionResult {
    use crate::local_runtime::session::deletion::{
        DeletionBlocker as Blocker, DeletionScope, SessionDeleteResult as Native,
    };
    use SessionDeletionBlocker as Reason;
    use SessionDeletionResult as Wire;
    match result {
        Native::Preview { preview } => {
            let nodes = preview
                .scopes
                .iter()
                .filter(|s| matches!(s, DeletionScope::Node { .. }))
                .count() as u64;
            let total = preview.scopes.len() as u64;
            Wire::Preview {
                preview: SessionDeletePreview {
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

#[cfg(test)]
mod tests {
    #[test]
    fn session_deletion_results_reject_obsolete_sequential_identities() {
        assert!(
            serde_json::from_value::<super::SessionDeletionResult>(
                serde_json::json!({"status": "deleted", "session_id": "session-1"})
            )
            .is_err()
        );
    }
    #[test]
    fn deletion_wire_roundtrips_every_shared_protocol_result() {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/app-server/deletion-fixtures.json"
        )))
        .unwrap();
        let mut statuses = std::collections::BTreeSet::new();
        for fixture in fixtures {
            statuses.insert(fixture["status"].as_str().unwrap().to_owned());
            let result: super::SessionDeletionResult =
                serde_json::from_value(fixture.clone()).unwrap();
            assert_eq!(serde_json::to_value(result).unwrap(), fixture);
        }
        assert_eq!(statuses.len(), 7);
    }
    #[test]
    fn deletion_protocol_projection_is_bounded_for_large_frozen_graphs() {
        use super::project as project_session_deletion;
        use crate::local_runtime::session::deletion::{
            DeletionBlocker, DeletionRecord, DeletionScope, SessionDeletePreview,
            SessionDeleteResult,
        };
        use crate::local_runtime::{SessionId, SessionNodeId};
        use crate::runtime::identity::ConversationId;
        let session_id = SessionId::generate();
        let mut lengths = Vec::new();
        for size in [1, 10, 10_000] {
            let scopes: Vec<_> = (0..size)
                .map(|_| DeletionScope::Child {
                    conversation_id: ConversationId::generate(),
                    parent_conversation: ConversationId::new(
                        "conv_413055e0-cb3a-7c6d-89b6-b446a98e0ad1",
                    ),
                })
                .chain(std::iter::once(DeletionScope::Node {
                    node_id: SessionNodeId::new("node_35971be6-e9bb-724a-8955-82fe0e42e048"),
                    conversation_id: ConversationId::new(
                        "conv_413055e0-cb3a-7c6d-89b6-b446a98e0ad1",
                    ),
                }))
                .collect();
            let preview = SessionDeletePreview {
                upload_workspaces: vec![],
                session_id: session_id.clone(),
                name: Some("x".repeat(10_000)),
                target_revision: "a".repeat(64),
                scopes: scopes.clone(),
            };
            let projected =
                serde_json::to_value(project_session_deletion(SessionDeleteResult::Preview {
                    preview,
                }))
                .unwrap();
            assert_eq!(projected["preview"]["owned_child_count"], size);
            assert_eq!(projected["preview"]["owned_conversation_count"], size + 1);
            assert_eq!(projected["preview"]["owned_node_count"], 1);
            let text = projected.to_string();
            assert!(text.len() < 600);
            assert!(!text.contains("scopes"));
            let record = DeletionRecord {
                agent_workspaces: vec![],
                upload_workspaces: vec![],
                session_id: session_id.clone(),
                target_revision: "a".repeat(64),
                scopes,
            };
            let results = [
                SessionDeleteResult::CommittedCleanupPending {
                    record: record.clone(),
                    detail: Some("/private/path".repeat(size)),
                },
                SessionDeleteResult::CommittedDurabilityUncertain {
                    session_id: record.session_id,
                    detail: "/private/path".repeat(size),
                },
                SessionDeleteResult::Blocked {
                    session_id: session_id.clone(),
                    reason: DeletionBlocker::Workspace {
                        resources: vec!["private resource".into(); size],
                    },
                },
            ];
            let serialized: Vec<_> = results
                .into_iter()
                .map(|r| serde_json::to_string(&project_session_deletion(r)).unwrap())
                .collect();
            for text in &serialized {
                assert!(text.len() < 140);
                assert!(!text.contains("private"));
                assert!(!text.contains("scopes"));
            }
            lengths.push((text.len(), serialized[0].len(), serialized[1].len()));
        }
        assert_eq!(lengths[0].1, lengths[2].1);
        assert_eq!(lengths[0].2, lengths[2].2);
        // Only the decimal count widths may grow, never the descendant collection.
        assert_eq!(lengths[2].0 - lengths[0].0, 8);
    }
}
