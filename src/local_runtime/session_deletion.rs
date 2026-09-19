//! Guarded, non-destructive Session deletion preflight.
//!
//! Ownership comes from catalog membership and typed native durable ownership
//! commits. Origin references and model-visible history are never traversed.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::session::{SessionCatalog, SessionId, SessionNode};
use super::session_ownership::{OwnedConversation, SessionOwnership};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::events::types::{
    RuntimeEvent, SubagentWorkspaceDisposalSettlement, SubagentWorkspaceTerminalResource,
};
use crate::runtime::identity::ConversationId;
use crate::runtime::local_storage::{ConversationExclusion, OwnershipSnapshot, ProductRoot};
use crate::runtime::workspace::{
    WorkspaceDisposalSettlement, WorkspaceSettlementDisposition, WorkspaceSnapshot,
};

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

/// Finite durable ownership inspection. Holds the ownership snapshot while
/// deriving the target, but never excludes live Conversation allocations.
#[derive(Debug)]
pub struct DeletionTargetSnapshot {
    session_id: SessionId,
    nodes: Vec<SessionNode>,
    conversations: Vec<OwnedConversation>,
    workspace_blockers: Vec<WorkspaceBlocker>,
    revision: [u8; 32],
    _authority: OwnershipSnapshot,
}

impl DeletionTargetSnapshot {
    /// Inspect a finite durable ownership target, including resident Sessions.
    /// # Errors
    /// Missing or ambiguous ownership and invalid metadata fail closed.
    pub fn inspect(root: &Path, session_id: &SessionId) -> std::io::Result<Self> {
        let authority = ProductRoot::existing(root)?;
        let freeze = authority.freeze_ownership()?;
        let catalog = SessionCatalog::read_under_guard(&authority)
            .map_err(invalid)?
            .ok_or_else(|| invalid("unknown Session catalog"))?;
        let selected = SessionOwnership::inspect(&authority, &freeze, &catalog, || Ok(()))
            .and_then(|ownership| ownership.select(session_id))
            .map_err(invalid)?;
        let nodes = selected.nodes;
        let conversations = selected.conversations;
        let mut blockers = Vec::new();
        // Deletion adds resource/disposal facts to the shared ownership set.
        // The same freeze protects this read and the revision calculation.
        for owned in &conversations {
            let store = SqliteConversationStore::open_existing(
                owned.conversation_id.clone(),
                &owned.database,
            )
            .map_err(invalid)?;
            blockers.extend(read_facts(&store)?.blockers.into_iter().map(
                |(resource_id, (workspace, state))| WorkspaceBlocker {
                    conversation_id: owned.conversation_id.clone(),
                    resource_id,
                    workspace,
                    state,
                },
            ));
        }
        blockers.sort_by(|a, b| {
            (&a.conversation_id, &a.resource_id).cmp(&(&b.conversation_id, &b.resource_id))
        });
        let revision =
            ownership_revision(&authority, session_id, &nodes, &conversations, &blockers)?;
        Ok(Self {
            session_id: session_id.clone(),
            nodes,
            conversations,
            workspace_blockers: blockers,
            revision,
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

struct Facts {
    blockers: BTreeMap<String, (WorkspaceSnapshot, WorkspaceBlockerState)>,
}

// Reuse the existing typed native ownership and disposal authority. This is
// neither a text search nor a second subagent lifecycle persistence system.
#[allow(clippy::too_many_lines)] // One closed native ownership/disposal vocabulary.
fn read_facts(store: &SqliteConversationStore) -> std::io::Result<Facts> {
    read_facts_while(store, || Ok(()))
}

#[allow(clippy::too_many_lines)] // One closed native ownership/disposal vocabulary.
fn read_facts_while(
    store: &SqliteConversationStore,
    check: impl Fn() -> std::io::Result<()>,
) -> std::io::Result<Facts> {
    let mut blockers = BTreeMap::new();
    let mut resources = BTreeSet::new();
    // Retain immutable authority even after its blocker is disposed. Borrowing
    // is a reference to this native fact, never a second disposal authority.
    let mut workflow_owners = BTreeMap::new();
    let mut borrowed_children = BTreeSet::new();
    let mut cursor = None;
    let through = store.event_high_watermark().map_err(invalid)?;
    while cursor.unwrap_or(0) < through {
        check()?;
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
                    workspace,
                    ownership,
                    ..
                } => {
                    workspace.validate().map_err(invalid)?;
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
    Ok(Facts { blockers })
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

/// Destructive allocation authority, acquired only after managed writers retire.
#[derive(Debug)]
pub struct DeletionExclusion {
    _targets: Vec<ConversationExclusion>,
}
impl DeletionExclusion {
    /// # Errors
    /// Any remaining external allocation owner excludes destructive authority.
    pub fn acquire(root: &Path, target: &DeletionTargetSnapshot) -> std::io::Result<Self> {
        let root = ProductRoot::existing(root)?;
        Ok(Self {
            _targets: target
                .conversations
                .iter()
                .map(|c| ConversationExclusion::acquire(&root, &c.private_root))
                .collect::<std::io::Result<Vec<_>>>()?,
        })
    }
}
