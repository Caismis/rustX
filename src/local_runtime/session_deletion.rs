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
use crate::runtime::local_storage::{ConversationExclusion, OwnershipSnapshot, ProductRoot};
use crate::runtime::subagent::child_conversation_store_path;
use crate::runtime::workspace::{
    WorkspaceDisposalSettlement, WorkspaceSettlementDisposition, WorkspaceSnapshot,
};

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
    pub state: WorkspaceBlockerState,
}

/// Final disposal-relevant state; diagnostics and execution history are excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceBlockerState {
    Owned,
    Retained { head_commit: String, dirty: bool },
    Unresolved(crate::runtime::workspace::WorkspaceUnresolvedReason),
    BranchOnly,
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
    _targets: Vec<ConversationExclusion>,
    _authority: OwnershipSnapshot,
}

impl SessionDeletionPreflight {
    /// Freeze ownership transitions, derive the target, then acquire exclusive
    /// allocation guards in `ConversationId` order. The final target acquisition
    /// linearizes exclusive authority. All guards remain held by the snapshot.
    /// Live target access causes `WouldBlock`; unrelated runtimes remain usable.
    ///
    /// # Errors
    /// Missing or ambiguous ownership, invalid metadata and live access fail closed.
    pub fn acquire(root: &Path, session_id: &SessionId) -> std::io::Result<Self> {
        let authority = ProductRoot::existing(root)?;
        let freeze = authority.freeze_ownership()?;
        let catalog = SessionCatalog::read_under_guard(&authority)
            .map_err(invalid)?
            .ok_or_else(|| invalid("unknown Session catalog"))?;
        let sessions = catalog.deletion_nodes();
        let nodes = sessions
            .get(session_id)
            .ok_or_else(|| invalid("unknown Session"))?
            .clone();
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
            let facts = read_facts(&store)?;
            for child in facts.children {
                safe_identity(&child)?;
                let database =
                    authority.confined(&child_conversation_store_path(authority.root(), &child))?;
                pending.push((owner.clone(), child, Some(id.clone()), database));
            }
            if owner == *session_id {
                blockers.extend(facts.blockers.into_iter().map(
                    |(resource_id, (workspace, state))| WorkspaceBlocker {
                        conversation_id: id.clone(),
                        resource_id,
                        workspace,
                        state,
                    },
                ));
            }
            all.insert(id, (owner, owned));
        }
        let conversations: Vec<_> = all
            .into_values()
            .filter_map(|(owner, lineage)| (owner == *session_id).then_some(lineage))
            .collect();
        blockers.sort_by(|a, b| {
            (&a.conversation_id, &a.resource_id).cmp(&(&b.conversation_id, &b.resource_id))
        });
        let targets = conversations
            .iter()
            .map(|c| ConversationExclusion::acquire(&authority, &c.private_root))
            .collect::<std::io::Result<Vec<_>>>()?;
        let revision =
            ownership_revision(&authority, session_id, &nodes, &conversations, &blockers)?;
        Ok(Self {
            session_id: session_id.clone(),
            nodes,
            conversations,
            workspace_blockers: blockers,
            revision,
            _targets: targets,
            _authority: freeze,
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
    /// Canonical semantic token for target ownership and final blocker state.
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
    blockers: BTreeMap<String, (WorkspaceSnapshot, WorkspaceBlockerState)>,
}

// Reuse the existing typed native ownership and disposal authority. This is
// neither a text search nor a second subagent lifecycle persistence system.
#[allow(clippy::too_many_lines)] // One closed native ownership/disposal vocabulary.
fn read_facts(store: &SqliteConversationStore) -> std::io::Result<Facts> {
    let mut children = BTreeSet::new();
    let mut blockers = BTreeMap::new();
    let mut resources = BTreeSet::new();
    // Retain immutable authority even after its blocker is disposed. Borrowing
    // is a reference to this native fact, never a second disposal authority.
    let mut workflow_owners = BTreeMap::new();
    let mut borrowed_children = BTreeSet::new();
    let mut cursor = None;
    let through = store.event_high_watermark().map_err(invalid)?;
    while cursor.unwrap_or(0) < through {
        let page = store.read_events(cursor, 256).map_err(invalid)?;
        if page.events.is_empty() {
            break;
        }
        if page.next_sequence <= cursor {
            return Err(invalid("non-monotonic durable event cursor"));
        }
        cursor = page.next_sequence;
        for envelope in page.events {
            if envelope.sequence > through {
                break;
            }
            if envelope.conversation_id != *store.conversation_id() {
                return Err(invalid("foreign ownership envelope"));
            }
            match envelope.event {
                RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id,
                    child_conversation_id,
                    workspace,
                    ownership,
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
                    if let Some(run) = &workspace.borrowed_from {
                        let mut physical_owner = workspace.clone();
                        physical_owner.borrowed_from = None;
                        if run.conversation_id != *store.conversation_id()
                            || ownership != crate::events::types::SubagentOwnershipKind::Workflow
                            || workflow_owners.get(run) != Some(&physical_owner)
                        {
                            return Err(invalid(
                                "borrowed child workspace has no matching Workflow owner",
                            ));
                        }
                        borrowed_children.insert(key);
                    } else if workspace.is_isolated() {
                        blockers.insert(key, (workspace, WorkspaceBlockerState::Owned));
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
                    match workspace_resource {
                        SubagentWorkspaceTerminalResource::None => {
                            blockers.remove(&key);
                        }
                        SubagentWorkspaceTerminalResource::Retained { handoff } => {
                            let (workspace, state) = blockers
                                .get_mut(&key)
                                .ok_or_else(|| invalid("retained resource without ownership"))?;
                            validate_handoff(workspace, &handoff)?;
                            *state = WorkspaceBlockerState::Retained {
                                head_commit: handoff.head_commit,
                                dirty: handoff.dirty,
                            };
                        }
                        SubagentWorkspaceTerminalResource::PreservedUnresolved {
                            reason, ..
                        } => {
                            blockers
                                .get_mut(&key)
                                .ok_or_else(|| invalid("unresolved resource without ownership"))?
                                .1 = WorkspaceBlockerState::Unresolved(reason);
                        }
                    }
                }
                RuntimeEvent::SubagentWorkspaceDisposalSettled {
                    subagent_id,
                    settlement,
                    ..
                } => {
                    let key = format!("child:{subagent_id}");
                    if !resources.contains(&key) || borrowed_children.contains(&key) {
                        return Err(invalid("disposal without independent child ownership"));
                    }
                    match settlement {
                        SubagentWorkspaceDisposalSettlement::Disposed => {
                            blockers.remove(&key);
                        }
                        SubagentWorkspaceDisposalSettlement::WorktreeRemoved => {
                            blockers
                                .get_mut(&key)
                                .ok_or_else(|| invalid("partial disposal without resource"))?
                                .1 = WorkspaceBlockerState::BranchOnly;
                        }
                    }
                }

                RuntimeEvent::WorkflowWorkspaceOwned { run_id, workspace } => {
                    workspace.validate().map_err(invalid)?;
                    if run_id.conversation_id != *store.conversation_id()
                        || run_id.attempt_id.as_str().is_empty()
                        || run_id.invocation == 0
                        || envelope.event_id
                            != crate::runtime::workspace::workflow_resource_event_id(
                                &run_id, "owned",
                            )
                        || !workspace.is_isolated()
                        || workspace.borrowed_from.is_some()
                    {
                        return Err(invalid("invalid Workflow workspace ownership authority"));
                    }
                    workflow_owners.insert(run_id.clone(), workspace.clone());
                    let key = format!(
                        "workflow:{}",
                        serde_json::to_string(&run_id).map_err(invalid)?
                    );
                    if !resources.insert(key.clone()) {
                        return Err(invalid("duplicate Workflow ownership"));
                    }
                    if workspace.is_isolated() {
                        blockers.insert(key, (workspace, WorkspaceBlockerState::Owned));
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
                    match workspace.disposition {
                        WorkspaceSettlementDisposition::Retained { handoff, .. } => {
                            let (owned, state) = blockers
                                .get_mut(&key)
                                .ok_or_else(|| invalid("retained workflow without ownership"))?;
                            validate_handoff(owned, &handoff)?;
                            *state = WorkspaceBlockerState::Retained {
                                head_commit: handoff.head_commit,
                                dirty: handoff.dirty,
                            };
                        }
                        WorkspaceSettlementDisposition::PreservedUnresolved { reason, .. } => {
                            blockers
                                .get_mut(&key)
                                .ok_or_else(|| invalid("unresolved workflow without ownership"))?
                                .1 = WorkspaceBlockerState::Unresolved(reason);
                        }
                        _ => {
                            blockers.remove(&key);
                        }
                    }
                }
                RuntimeEvent::WorkflowWorkspaceDisposalSettled { run_id, settlement } => {
                    let key = format!(
                        "workflow:{}",
                        serde_json::to_string(&run_id).map_err(invalid)?
                    );
                    if !resources.contains(&key) {
                        return Err(invalid("disposal without ownership"));
                    }
                    match settlement {
                        WorkspaceDisposalSettlement::Disposed
                        | WorkspaceDisposalSettlement::AlreadyDisposed => {
                            blockers.remove(&key);
                        }
                        WorkspaceDisposalSettlement::WorktreeRemoved { .. } => {
                            blockers
                                .get_mut(&key)
                                .ok_or_else(|| {
                                    invalid("partial workflow disposal without resource")
                                })?
                                .1 = WorkspaceBlockerState::BranchOnly;
                        }
                        WorkspaceDisposalSettlement::NothingRemoved { .. } => {}
                    }
                }

                _ => {}
            }
        }
    }
    Ok(Facts { children, blockers })
}

// Explicit length-delimited semantic vocabulary. No event envelopes or catalog
// serialization enter this digest. Collection tags/counts make it unambiguous.
fn ownership_revision(
    root: &ProductRoot,
    session: &SessionId,
    nodes: &[SessionNode],
    conversations: &[OwnedConversation],
    blockers: &[WorkspaceBlocker],
) -> std::io::Result<[u8; 32]> {
    fn field(hash: &mut Sha256, value: &[u8]) {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    fn text(hash: &mut Sha256, value: &str) {
        field(hash, value.as_bytes());
    }
    fn path(hash: &mut Sha256, value: &Path) {
        field(hash, value.as_os_str().as_encoded_bytes());
    }
    let mut hash = Sha256::new();
    text(&mut hash, "rustx/session-deletion-ownership/v1");
    text(&mut hash, session.as_str());
    let mut nodes: Vec<_> = nodes.iter().collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    for node in nodes {
        text(&mut hash, "node");
        text(&mut hash, node.id.as_str());
        text(&mut hash, node.parent.as_ref().map_or("", |id| id.as_str()));
        text(&mut hash, node.conversation_id.as_str());
    }
    for conversation in conversations {
        text(&mut hash, "conversation");
        text(&mut hash, conversation.conversation_id.as_str());
        text(
            &mut hash,
            conversation
                .parent_conversation
                .as_ref()
                .map_or("", |id| id.as_str()),
        );
        path(
            &mut hash,
            conversation
                .private_root
                .strip_prefix(root.root())
                .map_err(invalid)?,
        );
    }
    for blocker in blockers {
        text(&mut hash, "workspace-blocker");
        text(&mut hash, blocker.conversation_id.as_str());
        text(&mut hash, &blocker.resource_id);
        let workspace = blocker
            .workspace
            .git_worktree()
            .ok_or_else(|| invalid("blocker without physical ownership"))?;
        path(&mut hash, &workspace.source_repository_root);
        path(&mut hash, &workspace.physical_worktree_root);
        text(&mut hash, &workspace.branch);
        text(&mut hash, &workspace.base_commit);
        path(&mut hash, &workspace.repository_relative_workspace);
        path(&mut hash, &blocker.workspace.logical_workspace);
        match &blocker.state {
            WorkspaceBlockerState::Owned => text(&mut hash, "owned"),
            WorkspaceBlockerState::Retained { head_commit, dirty } => {
                text(&mut hash, "retained");
                text(&mut hash, head_commit);
                text(&mut hash, if *dirty { "dirty" } else { "clean" });
            }
            WorkspaceBlockerState::Unresolved(reason) => {
                text(&mut hash, match reason {
                    crate::runtime::workspace::WorkspaceUnresolvedReason::PhysicalSettlement => "unresolved-physical",
                    crate::runtime::workspace::WorkspaceUnresolvedReason::NestedContainment => "unresolved-containment",
                });
            }
            WorkspaceBlockerState::BranchOnly => text(&mut hash, "branch-only"),
        }
    }
    Ok(hash.finalize().into())
}

fn validate_handoff(
    workspace: &WorkspaceSnapshot,
    handoff: &crate::runtime::workspace::WorkspaceHandoff,
) -> std::io::Result<()> {
    let tree = workspace
        .git_worktree()
        .ok_or_else(|| invalid("handoff without worktree"))?;
    if handoff.physical_worktree_root != tree.physical_worktree_root
        || handoff.branch != tree.branch
        || handoff.base_commit != tree.base_commit
        || handoff.logical_workspace != workspace.logical_workspace
    {
        return Err(invalid("foreign workspace handoff"));
    }
    Ok(())
}
