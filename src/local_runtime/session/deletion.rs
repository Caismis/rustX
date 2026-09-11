//! Two-phase Session deletion. Catalog rename is visibility; successful parent
//! fsync is durable authority. Only `CleanupWork` minted after that barrier can
//! remove private resources. Recovery republishes the SAME record before retry.
use super::{
    CatalogCommitError, CatalogDocument, SessionCatalog, SessionError, SessionId, SessionNodeId,
    validate_id,
};
use crate::runtime::identity::ConversationId;
use crate::runtime::local_storage::{ConversationExclusion, ProductRoot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Semantic allocation identity; paths are derived from the trusted product root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeletionScope {
    /// One catalog node's private Conversation allocation.
    Node {
        node_id: SessionNodeId,
        conversation_id: ConversationId,
    },
    /// One durably owned descendant's private allocation.
    Child {
        conversation_id: ConversationId,
        parent_conversation: ConversationId,
    },
}
impl DeletionScope {
    fn conversation(&self) -> &ConversationId {
        match self {
            Self::Node {
                conversation_id, ..
            }
            | Self::Child {
                conversation_id, ..
            } => conversation_id,
        }
    }
    fn path(&self, root: &ProductRoot, session: &SessionId) -> PathBuf {
        match self {
            Self::Node {
                conversation_id, ..
            } => root
                .root()
                .join("sessions")
                .join(session.as_str())
                .join("conversations")
                .join(conversation_id.as_str()),
            Self::Child {
                conversation_id, ..
            } => root.root().join("subagents").join(conversation_id.as_str()),
        }
    }
    fn allocation(&self, root: &ProductRoot, session: &SessionId) -> std::io::Result<PathBuf> {
        root.confined(&self.path(root, session))
    }
}
/// Finite presentation snapshot. Contains no guards or caller-authored paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDeletePreview {
    pub session_id: SessionId,
    pub name: Option<String>,
    pub target_revision: String,
    pub scopes: Vec<DeletionScope>,
}
/// Pre-commit safety rejection. No force path exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeletionBlocker {
    CurrentSession,
    InUse,
    Workspace { resources: Vec<String> },
    InvalidOwnership { detail: String },
}
/// Complete public control vocabulary, shared by preview, execute and recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionDeleteResult {
    Preview {
        preview: SessionDeletePreview,
    },
    Deleted {
        session_id: SessionId,
    },
    Stale {
        session_id: SessionId,
        actual_revision: String,
    },
    Blocked {
        session_id: SessionId,
        reason: DeletionBlocker,
    },
    CommittedCleanupPending {
        record: DeletionRecord,
        detail: Option<String>,
    },
    CommittedDurabilityUncertain {
        record: DeletionRecord,
        detail: String,
    },
    NotFound {
        session_id: SessionId,
    },
}
/// Persisted state is deliberately small: replay the complete idempotent workset
/// until all removals AND their directory barriers succeed, then publish Deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionPhase {
    CleanupPending,
    Deleted,
}
/// The frozen record owns cleanup and reserves identities permanently. Completed
/// records retain identities (not history) to reject explicit/restored ID reuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeletionRecord {
    pub session_id: SessionId,
    pub target_revision: String,
    pub scopes: Vec<DeletionScope>,
    pub phase: DeletionPhase,
}
/// An owned cleanup capability. It never borrows a catalog or root snapshot.
#[derive(Debug)]
pub(crate) struct CleanupWork {
    root: ProductRoot,
    #[cfg(test)]
    pub(crate) cleanup_gate: Option<Arc<crate::runtime::conversation_runtime::Gate>>,
    pub(crate) record: DeletionRecord,
    _controller: Option<Arc<crate::runtime::local_storage::ProductController>>,
}
impl CleanupWork {
    pub(crate) fn run(&self) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(gate) = &self.cleanup_gate {
            gate.enter();
        }
        // Admission consults the catalog while holding ConversationAccess. Since
        // the record is durably authoritative, new normal access cannot enter.
        // Existing holders still exclude cleanup. Lock only one private unit at
        // a time; no root/catalog guard spans recursive filesystem operations.
        for scope in &self.record.scopes {
            let path = scope.allocation(&self.root, &self.record.session_id)?;
            let guard = match ConversationExclusion::acquire(&self.root, &path) {
                Ok(guard) => Some(guard),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            if let DeletionScope::Child {
                conversation_id, ..
            } = scope
            {
                // The short Unix socket is intentionally outside the private
                // allocation. Its native identity is frozen by the child scope.
                let socket = self.root.confined(
                    &crate::runtime::subagent::child_conversation_inspection_socket_path(
                        self.root.root(),
                        conversation_id,
                    ),
                )?;
                match fs::remove_file(socket) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                File::open(self.root.root())?.sync_all()?;
            }
            match fs::remove_dir_all(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            // Missing residue on retry still needs a barrier: an earlier unlink
            // may have been visible without surviving a machine crash.
            let mut parent = path.parent().expect("allocation parent");
            while !parent.exists() {
                parent = parent.parent().expect("product root exists");
            }
            File::open(parent)?.sync_all()?;
            drop(guard);
            #[cfg(test)]
            process_gate("cleanup_item");
        }
        Ok(())
    }
}

impl SessionCatalog {
    fn existing_deletion(&self, id: &SessionId) -> Option<SessionDeleteResult> {
        self.document.deletions.get(id).map(|record| {
            if let Err(error) = File::open(&self.path)
                .and_then(|f| f.sync_all())
                .and_then(|()| super::sync_directory_ancestry(&self.root))
            {
                return SessionDeleteResult::CommittedDurabilityUncertain {
                    record: record.clone(),
                    detail: error.to_string(),
                };
            }
            match record.phase {
                DeletionPhase::Deleted => SessionDeleteResult::Deleted {
                    session_id: id.clone(),
                },
                DeletionPhase::CleanupPending => SessionDeleteResult::CommittedCleanupPending {
                    record: record.clone(),
                    detail: None,
                },
            }
        })
    }
    fn preview_preflight(
        &self,
        id: &SessionId,
    ) -> Result<super::super::session_deletion::SessionDeletionPreflight, SessionDeleteResult> {
        let blocked = |reason| SessionDeleteResult::Blocked {
            session_id: id.clone(),
            reason,
        };
        if let Some(result) = self.existing_deletion(id) {
            return Err(result);
        }
        if !self.document.sessions.contains_key(id) {
            return Err(SessionDeleteResult::NotFound {
                session_id: id.clone(),
            });
        }
        if self.document.active_session == *id {
            return Err(blocked(DeletionBlocker::CurrentSession));
        }
        let preflight = self.deletion_preflight(id).map_err(|e| {
            blocked(if e.kind() == std::io::ErrorKind::WouldBlock {
                DeletionBlocker::InUse
            } else {
                DeletionBlocker::InvalidOwnership {
                    detail: e.to_string(),
                }
            })
        })?;
        Ok(preflight)
    }
    fn workspace_blocked(
        preflight: &super::super::session_deletion::SessionDeletionPreflight,
    ) -> Option<SessionDeleteResult> {
        (!preflight.workspace_blockers().is_empty()).then(|| SessionDeleteResult::Blocked {
            session_id: preflight.session_id().clone(),
            reason: DeletionBlocker::Workspace {
                resources: preflight
                    .workspace_blockers()
                    .iter()
                    .map(|b| b.resource_id.clone())
                    .collect(),
            },
        })
    }
    fn snapshot_preflight(
        &self,
        preflight: &super::super::session_deletion::SessionDeletionPreflight,
    ) -> SessionDeletePreview {
        let scopes = preflight
            .conversations()
            .iter()
            .map(|c| {
                if let Some(parent) = &c.parent_conversation {
                    DeletionScope::Child {
                        conversation_id: c.conversation_id.clone(),
                        parent_conversation: parent.clone(),
                    }
                } else {
                    let node = preflight
                        .nodes()
                        .iter()
                        .find(|n| n.conversation_id == c.conversation_id)
                        .expect("preflight node authority");
                    DeletionScope::Node {
                        node_id: node.id.clone(),
                        conversation_id: c.conversation_id.clone(),
                    }
                }
            })
            .collect();
        SessionDeletePreview {
            session_id: preflight.session_id().clone(),
            name: self.document.sessions[preflight.session_id()].name.clone(),
            target_revision: preflight.ownership_revision().iter().fold(
                String::with_capacity(64),
                |mut text, b| {
                    use std::fmt::Write;
                    write!(&mut text, "{b:02x}").expect("write string");
                    text
                },
            ),
            scopes,
        }
    }
    /// Return a finite snapshot and release all exclusion before confirmation.
    #[must_use]
    pub fn delete_preview(&self, id: &SessionId) -> SessionDeleteResult {
        match self.preview_preflight(id) {
            Ok(preflight) => {
                if let Some(result) = Self::workspace_blocked(&preflight) {
                    return result;
                }
                SessionDeleteResult::Preview {
                    preview: self.snapshot_preflight(&preflight),
                }
            }
            Err(result) => result,
        }
    }
    /// Fresh preflight, semantic validation and one coherent durable mutation.
    /// An error is strictly pre-visibility; post-visibility uncertainty is typed.
    pub(crate) fn commit_delete(
        &mut self,
        id: &SessionId,
        revision: &str,
    ) -> Result<Result<CleanupWork, SessionDeleteResult>, SessionError> {
        let preflight = match self.preview_preflight(id) {
            Ok(p) => p,
            Err(r) => return Ok(Err(r)),
        };
        let preview = self.snapshot_preflight(&preflight);
        if preview.target_revision != revision {
            return Ok(Err(SessionDeleteResult::Stale {
                session_id: id.clone(),
                actual_revision: preview.target_revision,
            }));
        }
        if let Some(result) = Self::workspace_blocked(&preflight) {
            return Ok(Err(result));
        }
        let record = DeletionRecord {
            session_id: id.clone(),
            target_revision: preview.target_revision,
            scopes: preview.scopes,
            phase: DeletionPhase::CleanupPending,
        };
        let mut next = self.document.clone();
        next.sessions.remove(id);
        next.deletions.insert(id.clone(), record.clone());
        // Preflight already holds the exclusive root snapshot. Do not reacquire
        // a shared mutation lock or release the snapshot before publication.
        match self.commit_under_ownership(next) {
            Ok(()) => {}
            Err(SessionError::CatalogCommit {
                error: CatalogCommitError::CommittedButDurabilityUncertain { detail, .. },
            }) => {
                return Ok(Err(SessionDeleteResult::CommittedDurabilityUncertain {
                    record,
                    detail,
                }));
            }
            Err(e) => return Err(e),
        }
        drop(preflight);
        #[cfg(test)]
        process_gate("logical_commit");
        Ok(Ok(CleanupWork {
            #[cfg(test)]
            cleanup_gate: None,
            root: self.product.clone(),
            record,
            _controller: self.lifecycle.clone(),
        }))
    }
    /// Republish the existing frozen authority to establish durability, including
    /// after an uncertain rename. Never call preflight or inspect a Conversation.
    pub(crate) fn recover_delete(
        &mut self,
        id: &SessionId,
    ) -> Result<CleanupWork, SessionDeleteResult> {
        let Some(record) = self.document.deletions.get(id).cloned() else {
            return Err(SessionDeleteResult::NotFound {
                session_id: id.clone(),
            });
        };
        // Even a visible Deleted record may have had an uncertain final barrier.
        // Republish before emitting the terminal result.
        if let Err(e) = self.commit(self.document.clone()) {
            return Err(SessionDeleteResult::CommittedDurabilityUncertain {
                record,
                detail: e.to_string(),
            });
        }
        if record.phase == DeletionPhase::Deleted {
            return Err(SessionDeleteResult::Deleted {
                session_id: id.clone(),
            });
        }
        Ok(CleanupWork {
            #[cfg(test)]
            cleanup_gate: None,
            root: self.product.clone(),
            record,
            _controller: self.lifecycle.clone(),
        })
    }
    pub(crate) fn finish_delete(
        &mut self,
        frozen: &DeletionRecord,
        cleanup: std::io::Result<()>,
    ) -> SessionDeleteResult {
        if self
            .document
            .deletions
            .get(&frozen.session_id)
            .is_some_and(|record| record.phase == DeletionPhase::Deleted)
        {
            // Another retry may have finalized while this worker was running.
            // A delayed failure cannot regress the authoritative terminal state.
            return self
                .existing_deletion(&frozen.session_id)
                .expect("retained terminal identity");
        }
        if let Err(e) = cleanup {
            return SessionDeleteResult::CommittedCleanupPending {
                record: frozen.clone(),
                detail: Some(e.to_string()),
            };
        }
        let mut next = self.document.clone();
        let record = next
            .deletions
            .get_mut(&frozen.session_id)
            .expect("committed deletion retained");
        record.phase = DeletionPhase::Deleted;
        let completed = record.clone();
        match self.commit(next) {
            Ok(()) => SessionDeleteResult::Deleted {
                session_id: frozen.session_id.clone(),
            },
            Err(SessionError::CatalogCommit {
                error: CatalogCommitError::CommittedButDurabilityUncertain { detail, .. },
            }) => SessionDeleteResult::CommittedDurabilityUncertain {
                record: completed,
                detail,
            },
            Err(e) => SessionDeleteResult::CommittedCleanupPending {
                record: frozen.clone(),
                detail: Some(e.to_string()),
            },
        }
    }
    pub(crate) fn pending_deletion_ids(&self) -> Vec<SessionId> {
        self.document
            .deletions
            .iter()
            .filter(|(_, record)| record.phase == DeletionPhase::CleanupPending)
            .map(|(id, _)| id.clone())
            .collect()
    }
    // Test driver of the same startup boundary; production runs owned work in
    // composition or the supervisor, never inside a catalog mutation method.
    #[cfg(test)]
    pub(crate) fn recover_deletions(&mut self) -> Vec<SessionDeleteResult> {
        let ids: Vec<_> = self
            .document
            .deletions
            .iter()
            .filter(|(_, r)| r.phase == DeletionPhase::CleanupPending)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .map(|id| match self.recover_delete(&id) {
                Ok(work) => {
                    let result = work.run();
                    self.finish_delete(&work.record, result)
                }
                Err(result) => result,
            })
            .collect()
    }
    pub(super) fn reject_deleted_identity(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
        conversation: &ConversationId,
    ) -> Result<(), SessionError> {
        if self.document.deletions.contains_key(session)
            || self.document.deletions.values().any(|r| {
                r.scopes.iter().any(|s| {
                    s.conversation() == conversation
                        || matches!(s, DeletionScope::Node { node_id, .. } if node_id == node)
                })
            })
        {
            return Err(SessionError::Catalog {
                detail: "deleted identity cannot be reused".into(),
            });
        }
        Ok(())
    }
    pub(crate) fn conversation_is_deleted(&self, conversation: &ConversationId) -> bool {
        self.document.deletions.values().any(|record| {
            record
                .scopes
                .iter()
                .any(|scope| scope.conversation() == conversation)
        })
    }
    /// Storage admission uses the same catalog authority, under the existing
    /// allocation exclusion protocol. Residual files never grant normal access.
    pub(crate) fn check_allocation_live(
        root: &ProductRoot,
        allocation: &Path,
    ) -> std::io::Result<()> {
        let Some(catalog) = Self::read_under_guard(root).map_err(std::io::Error::other)? else {
            return Ok(());
        };
        // Recognize native placement only to reject reused identities, never
        // to discover ownership. A restored ID cannot evade a tombstone by
        // moving from a node allocation to a child allocation (or vice versa).
        let components: Vec<_> = allocation
            .strip_prefix(root.root())
            .map_err(std::io::Error::other)?
            .iter()
            .collect();
        let (session, conversation) = match components.as_slice() {
            [kind, session, conversations, conversation, ..]
                if *kind == "sessions" && *conversations == "conversations" =>
            {
                (Some(*session), Some(*conversation))
            }
            [kind, conversation, ..] if *kind == "subagents" => (None, Some(*conversation)),
            _ => (None, None),
        };
        for record in catalog.document.deletions.values() {
            for scope in &record.scopes {
                if session == Some(std::ffi::OsStr::new(record.session_id.as_str()))
                    || conversation == Some(std::ffi::OsStr::new(scope.conversation().as_str()))
                    || allocation.starts_with(scope.path(root, &record.session_id))
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Conversation identity is deleted",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn validate_records(document: &CatalogDocument) -> Result<(), SessionError> {
    let invalid = || SessionError::Catalog {
        detail: "invalid or reused deletion identity".into(),
    };
    let mut conversations = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    for (id, record) in &document.deletions {
        validate_id(id.as_str(), "deleted session")?;
        if *id != record.session_id
            || document.sessions.contains_key(id)
            || record.scopes.is_empty()
            || record.target_revision.len() != 64
            || !record
                .target_revision
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
        let owned: BTreeSet<_> = record
            .scopes
            .iter()
            .map(DeletionScope::conversation)
            .collect();
        for scope in &record.scopes {
            // Every child chain must terminate at a frozen catalog node. The
            // persisted authority must not encode a detached cycle or root.
            let mut current = scope;
            let mut seen = BTreeSet::new();
            while let DeletionScope::Child {
                parent_conversation,
                ..
            } = current
            {
                if !seen.insert(current.conversation()) {
                    return Err(invalid());
                }
                current = record
                    .scopes
                    .iter()
                    .find(|candidate| candidate.conversation() == parent_conversation)
                    .ok_or_else(invalid)?;
            }
            validate_id(scope.conversation().as_str(), "deleted conversation")?;
            if !conversations.insert(scope.conversation()) {
                return Err(invalid());
            }
            match scope {
                DeletionScope::Node { node_id, .. } => {
                    validate_id(node_id.as_str(), "deleted node")?;
                    if !nodes.insert(node_id) {
                        return Err(invalid());
                    }
                }
                DeletionScope::Child {
                    parent_conversation,
                    ..
                } => {
                    if !owned.contains(parent_conversation) {
                        return Err(invalid());
                    }
                }
            }
        }
    }
    for session in document.sessions.values() {
        for node in session.nodes.values() {
            if nodes.contains(&node.id) || conversations.contains(&node.conversation_id) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn process_gate(boundary: &str) {
    use std::io::Read;
    if std::env::var("RUSTX_255_GATE").as_deref() == Ok(boundary) {
        println!("DELETE_GATE:{boundary}");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
    }
}
