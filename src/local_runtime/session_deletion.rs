//! Guarded, non-destructive Session deletion preflight.
//!
//! Ownership comes from catalog membership and typed native durable ownership
//! commits. Origin references and model-visible history are never traversed.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::session::{SessionCatalog, SessionId, SessionNode};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::events::types::{
    RuntimeEvent, SubagentWorkspaceDisposalSettlement, SubagentWorkspaceTerminalResource,
};
use crate::runtime::identity::ConversationId;
use crate::runtime::local_storage::LocalStorageGuard;
use crate::runtime::subagent::child_conversation_store_path;
use crate::runtime::workspace::{WorkspaceCleanup, WorkspaceDisposalSettlement, WorkspaceSnapshot};

/// A lineage and its exclusive private allocation, never a workspace allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedConversation {
    pub conversation_id: ConversationId,
    pub parent_conversation: Option<ConversationId>,
    pub private_root: PathBuf,
    pub database: PathBuf,
    pub inspection_socket: Option<PathBuf>,
}

/// Native disposal authority must settle this resource before deletion can proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceBlocker {
    pub conversation_id: ConversationId,
    pub resource_id: String,
    pub workspace: WorkspaceSnapshot,
}

/// An authoritative snapshot whose OS exclusion remains held until it is dropped.
/// No cleanup or deletion methods are exposed by this foundational contract.
#[derive(Debug)]
pub struct SessionDeletionPreflight {
    session_id: SessionId,
    nodes: Vec<SessionNode>,
    conversations: Vec<OwnedConversation>,
    workspace_blockers: Vec<WorkspaceBlocker>,
    revision: [u8; 32],
    _authority: LocalStorageGuard,
}

impl SessionDeletionPreflight {
    /// Acquire exclusive lifecycle authority, then read and validate ownership.
    ///
    /// The successful OS acquisition is the exclusion linearization point.
    /// The returned value retains it across the complete snapshot lifetime.
    /// Live controllers, children and inspection readers cause `WouldBlock`.
    ///
    /// # Errors
    /// Missing or ambiguous ownership, invalid metadata and live access fail closed.
    pub fn acquire(root: &Path, session_id: &SessionId) -> std::io::Result<Self> {
        let authority = LocalStorageGuard::exclusive_existing(root)?;
        let catalog_path = authority.confined(&authority.root().join("sessions/catalog.json"))?;
        let catalog = SessionCatalog::read_under_guard(&authority)
            .map_err(invalid)?
            .ok_or_else(|| invalid("unknown Session catalog"))?;
        let sessions = catalog.deletion_nodes();
        let nodes = sessions
            .get(session_id)
            .ok_or_else(|| invalid("unknown Session"))?
            .clone();
        let mut digest = Sha256::new();
        digest.update(std::fs::read(catalog_path)?);
        let mut all = BTreeMap::new();
        let mut blockers = Vec::new();
        // Check ownership uniqueness across all native Sessions. An ambiguous
        // child shared by two parents must not be assigned to either Session.
        let mut pending = Vec::new();
        for (owner, nodes) in sessions {
            for node in nodes {
                let database =
                    authority.confined(&catalog.database_path(&owner, &node.conversation_id))?;
                pending.push((owner.clone(), node.conversation_id, None, database));
            }
        }
        while let Some((owner, id, parent, database)) = pending.pop() {
            safe_identity(&id)?;
            if all.contains_key(&id) {
                return Err(invalid("duplicate or cyclic Conversation ownership"));
            }
            let private_root = database
                .parent()
                .ok_or_else(|| invalid("missing private allocation"))?
                .to_path_buf();
            let inspection_socket = if parent.is_some() {
                Some(authority.confined(
                    &crate::runtime::subagent::child_conversation_inspection_socket_path(
                        authority.root(),
                        &id,
                    ),
                )?)
            } else {
                None
            };
            let owned = OwnedConversation {
                inspection_socket,
                conversation_id: id.clone(),
                parent_conversation: parent,
                private_root,
                database: database.clone(),
            };
            authority.confined(&owned.private_root.join("tool-output"))?;
            let store =
                SqliteConversationStore::open_existing(id.clone(), &database).map_err(invalid)?;
            let facts = read_facts(&store, &mut digest)?;
            for child in facts.children {
                safe_identity(&child)?;
                let database =
                    authority.confined(&child_conversation_store_path(authority.root(), &child))?;
                pending.push((owner.clone(), child, Some(id.clone()), database));
            }
            if owner == *session_id {
                blockers.extend(facts.blockers.into_iter().map(|(resource_id, workspace)| {
                    WorkspaceBlocker {
                        conversation_id: id.clone(),
                        resource_id,
                        workspace,
                    }
                }));
            }
            all.insert(id, (owner, owned));
        }
        let conversations = all
            .into_values()
            .filter_map(|(owner, lineage)| (owner == *session_id).then_some(lineage))
            .collect();
        blockers.sort_by(|a, b| {
            (&a.conversation_id, &a.resource_id).cmp(&(&b.conversation_id, &b.resource_id))
        });
        Ok(Self {
            session_id: session_id.clone(),
            nodes,
            conversations,
            workspace_blockers: blockers,
            revision: digest.finalize().into(),
            _authority: authority,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    #[must_use]
    pub fn nodes(&self) -> &[SessionNode] {
        &self.nodes
    }
    #[must_use]
    pub fn conversations(&self) -> &[OwnedConversation] {
        &self.conversations
    }
    #[must_use]
    pub fn workspace_blockers(&self) -> &[WorkspaceBlocker] {
        &self.workspace_blockers
    }
    /// Equality token for the observed catalog and native durable facts.
    #[must_use]
    pub fn ownership_revision(&self) -> &[u8; 32] {
        &self.revision
    }
}

fn invalid(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

fn safe_identity(id: &ConversationId) -> std::io::Result<()> {
    let value = id.as_str();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(invalid("invalid Conversation identity"));
    }
    Ok(())
}

struct Facts {
    children: BTreeSet<ConversationId>,
    blockers: BTreeMap<String, WorkspaceSnapshot>,
}

// Reuse the existing typed native ownership and disposal authority. This is
// neither a text search nor a second subagent lifecycle persistence system.
#[allow(clippy::too_many_lines)] // One closed native ownership/disposal vocabulary.
fn read_facts(store: &SqliteConversationStore, digest: &mut Sha256) -> std::io::Result<Facts> {
    let mut children = BTreeSet::new();
    let mut blockers = BTreeMap::new();
    let mut resources = BTreeSet::new();
    let mut cursor = None;
    loop {
        let page = store.read_events(cursor, 256).map_err(invalid)?;
        if page.events.is_empty() {
            break;
        }
        if page.next_sequence <= cursor {
            return Err(invalid("non-monotonic durable event cursor"));
        }
        cursor = page.next_sequence;
        for envelope in page.events {
            if envelope.conversation_id != *store.conversation_id() {
                return Err(invalid("foreign ownership envelope"));
            }
            digest.update(serde_json::to_vec(&envelope).map_err(invalid)?);
            match envelope.event {
                RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id,
                    child_conversation_id,
                    workspace,
                    ..
                } => {
                    workspace.validate().map_err(invalid)?;
                    if envelope.event_id
                        != crate::runtime::subagent::subagent_ownership_event_id(&subagent_id)
                    {
                        return Err(invalid("child ownership commit identity mismatch"));
                    }
                    if !children.insert(child_conversation_id) {
                        return Err(invalid("duplicate child ownership"));
                    }
                    let key = format!("child:{subagent_id}");
                    if !resources.insert(key.clone()) {
                        return Err(invalid("duplicate resource ownership"));
                    }
                    if workspace.is_isolated() {
                        blockers.insert(key, workspace);
                    }
                }
                RuntimeEvent::SubagentTerminalPublished {
                    subagent_id,
                    workspace_resource,
                    ..
                }
                | RuntimeEvent::SubagentTerminalSettled {
                    subagent_id,
                    workspace_resource,
                    ..
                } => {
                    let key = format!("child:{subagent_id}");
                    if !resources.contains(&key) {
                        return Err(invalid("terminal without ownership"));
                    }
                    if matches!(workspace_resource, SubagentWorkspaceTerminalResource::None) {
                        blockers.remove(&key);
                    }
                }
                RuntimeEvent::SubagentWorkspaceDisposalSettled {
                    subagent_id,
                    settlement: SubagentWorkspaceDisposalSettlement::Disposed,
                    ..
                } => {
                    let key = format!("child:{subagent_id}");
                    if !resources.contains(&key) {
                        return Err(invalid("disposal without ownership"));
                    }
                    blockers.remove(&key);
                }
                RuntimeEvent::WorkflowWorkspaceOwned { run_id, workspace } => {
                    workspace.validate().map_err(invalid)?;
                    if run_id.conversation_id != *store.conversation_id() {
                        return Err(invalid("foreign Workflow workspace"));
                    }
                    let key = format!(
                        "workflow:{}",
                        serde_json::to_string(&run_id).map_err(invalid)?
                    );
                    if !resources.insert(key.clone()) {
                        return Err(invalid("duplicate Workflow ownership"));
                    }
                    if workspace.is_isolated() {
                        blockers.insert(key, workspace);
                    }
                }
                RuntimeEvent::WorkflowWorkspaceSettled {
                    run_id, workspace, ..
                } => {
                    let key = format!(
                        "workflow:{}",
                        serde_json::to_string(&run_id).map_err(invalid)?
                    );
                    if !resources.contains(&key) {
                        return Err(invalid("settlement without ownership"));
                    }
                    if workspace.cleanup() != WorkspaceCleanup::Preserved {
                        blockers.remove(&key);
                    }
                }
                RuntimeEvent::WorkflowWorkspaceDisposalSettled {
                    run_id,
                    settlement:
                        WorkspaceDisposalSettlement::Disposed
                        | WorkspaceDisposalSettlement::AlreadyDisposed,
                } => {
                    let key = format!(
                        "workflow:{}",
                        serde_json::to_string(&run_id).map_err(invalid)?
                    );
                    if !resources.contains(&key) {
                        return Err(invalid("disposal without ownership"));
                    }
                    blockers.remove(&key);
                }
                _ => {}
            }
        }
    }
    Ok(Facts { children, blockers })
}
