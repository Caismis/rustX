//! Guarded, non-destructive Session deletion preflight.
//!
//! Ownership comes from catalog membership and typed native durable ownership
//! commits. Origin references and model-visible history are never traversed.
//! The Session execution workspace is never a deletion resource: children and
//! Workflows share it, so deletion neither inspects nor removes it.
use crate::durable::{JournalStore, ScheduleStore};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::session::{SessionCatalog, SessionId, SessionNode};
use super::session_ownership::{OwnedConversation, SessionOwnership};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::events::types::RuntimeEvent;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId};
use crate::runtime::local_storage::{ConversationExclusion, OwnershipSnapshot, ProductRoot};

/// Finite durable ownership inspection. Holds the ownership snapshot while
/// deriving the target, but never excludes live Conversation allocations.
#[derive(Debug)]
pub struct DeletionTargetSnapshot {
    session_id: SessionId,
    nodes: Vec<SessionNode>,
    conversations: Vec<OwnedConversation>,
    revision: [u8; 32],
    _authority: OwnershipSnapshot,
}

impl DeletionTargetSnapshot {
    /// Inspect a finite durable ownership target, including resident Sessions.
    /// # Errors
    /// Missing or ambiguous ownership, invalid metadata, an activation
    /// terminal without proven physical settlement, or an activation
    /// admission without proven rollback fail closed.
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
        // The same freeze protects this durable read and the revision.
        for owned in &conversations {
            let store = SqliteConversationStore::open_existing(
                owned.conversation_id.clone(),
                &owned.database,
            )
            .map_err(invalid)?;
            check_native_ownership(&store, || Ok(()))?;
        }
        let revision = ownership_revision(&authority, session_id, &nodes, &conversations)?;
        Ok(Self {
            session_id: session_id.clone(),
            nodes,
            conversations,
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

    /// Canonical semantic token for target ownership.
    #[must_use]
    pub fn ownership_revision(&self) -> &[u8; 32] {
        &self.revision
    }
}

fn invalid(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// Validates the closed native ownership vocabulary and the remaining native
/// deletion barriers. This is neither a text search nor a second subagent
/// lifecycle persistence system: an activation terminal without native
/// physical proof, or an activation admission whose rollback was not proven,
/// keeps the Session undeletable. Removing a projection is never proof.
fn check_native_ownership(
    store: &SqliteConversationStore,
    check: impl Fn() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let summary = store.schedule_summary().map_err(invalid)?;
    check()?;
    let starts = crate::durable::ScheduledWorkflowStore::scheduled_workflow_pending(store, 256)
        .map_err(invalid)?;
    if summary.enabled > 0 || !starts.is_empty() {
        let ids = starts
            .iter()
            .take(4)
            .map(|start| start.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(invalid(format!(
            "{} enabled schedules and {} Accepted/Claimed Workflow starts block Session deletion; starts [{}]. Disable future schedules, then inspect/cancel/reconcile each native start",
            summary.enabled,
            starts.len(),
            ids
        )));
    }
    let mut barriers = OwnershipBarriers::default();
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
            barriers.apply(envelope.event)?;
        }
    }
    barriers.finish()
}

/// The native ownership facts one parent journal folds into: durable Agent
/// identity and execution binding, committed activations, and the physical
/// settlement and admission barriers that keep a Session undeletable.
#[derive(Default)]
struct OwnershipBarriers {
    agents: BTreeMap<AgentId, (ConversationId, std::path::PathBuf)>,
    activations: BTreeSet<SubagentId>,
    pending_admissions: BTreeSet<SubagentId>,
    unproven_activations: BTreeSet<SubagentId>,
}

impl OwnershipBarriers {
    fn apply(&mut self, event: RuntimeEvent) -> std::io::Result<()> {
        use crate::events::types::{AgentActivationAdmissionPhase, SubagentOwnershipKind};
        match event {
            RuntimeEvent::AgentActivationAdmission {
                activation_id,
                phase,
                ..
            } => match phase {
                AgentActivationAdmissionPhase::Reserved => {
                    self.pending_admissions.insert(activation_id);
                }
                AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven: true,
                } => {
                    self.pending_admissions.remove(&activation_id);
                }
                AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven: false,
                } => {}
            },
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                child_agent_id,
                child_conversation_id,
                admitted_authority,
                workspace,
                ownership,
                ..
            } => {
                self.pending_admissions.remove(&subagent_id);
                if !self.activations.insert(subagent_id) {
                    return Err(invalid("duplicate activation ownership"));
                }
                match (admitted_authority.is_some(), ownership) {
                    (true, SubagentOwnershipKind::Normal) => {
                        if self
                            .agents
                            .insert(child_agent_id, (child_conversation_id, workspace))
                            .is_some()
                        {
                            return Err(invalid("duplicate durable Agent admission"));
                        }
                    }
                    (false, SubagentOwnershipKind::Normal) => {
                        // A resumed activation keeps its Agent's identity and
                        // frozen execution binding exactly.
                        if self.agents.get(&child_agent_id)
                            != Some(&(child_conversation_id, workspace))
                        {
                            return Err(invalid(
                                "resumed Agent changed admitted identity or execution binding",
                            ));
                        }
                    }
                    (false, SubagentOwnershipKind::Workflow) => {}
                    (true, SubagentOwnershipKind::Workflow) => {
                        return Err(invalid(
                            "a finite Workflow child cannot own durable Agent authority",
                        ));
                    }
                }
            }
            RuntimeEvent::SubagentTerminalPublished {
                subagent_id,
                physical_settlement_proven,
                ..
            }
            | RuntimeEvent::SubagentTerminalSettled {
                subagent_id,
                physical_settlement_proven,
                ..
            } => {
                if !self.activations.contains(&subagent_id) {
                    return Err(invalid("terminal without ownership"));
                }
                if !physical_settlement_proven {
                    self.unproven_activations.insert(subagent_id);
                }
            }
            RuntimeEvent::SubagentPhysicalSettlementProven { subagent_id, .. } => {
                self.unproven_activations.remove(&subagent_id);
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> std::io::Result<()> {
        if !self.unproven_activations.is_empty() {
            return Err(invalid(
                "Agent activation terminal has no proven physical settlement",
            ));
        }
        if !self.pending_admissions.is_empty() {
            return Err(invalid(
                "Agent activation admission has no proven physical settlement",
            ));
        }
        Ok(())
    }
}

// Explicit length-delimited semantic vocabulary. No event envelopes or catalog
// serialization enter this digest. Collection tags/counts make it unambiguous.
fn ownership_revision(
    root: &ProductRoot,
    session: &SessionId,
    nodes: &[SessionNode],
    conversations: &[OwnedConversation],
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
    text(&mut hash, "rustx/session-deletion-ownership/v2");
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
        let store = SqliteConversationStore::open_existing(
            conversation.conversation_id.clone(),
            &conversation.database,
        )
        .map_err(invalid)?;
        let schedule = store.schedule_summary().map_err(invalid)?;
        text(&mut hash, "scheduler-generation");
        field(&mut hash, &schedule.generation.to_be_bytes());
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
    Ok(hash.finalize().into())
}

/// Destructive allocation authority, acquired only after managed writers retire.
#[derive(Debug)]
pub struct DeletionExclusion {
    _targets: Vec<ConversationExclusion>,
    _physical: Vec<crate::runtime::subagent::physical_recovery::RecoveredPhysicalProof>,
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
            _physical: crate::runtime::subagent::physical_recovery::consumed_session_allocations(
                &root,
                target.session_id(),
            )?
            .into_iter()
            .map(|(activation, conversation)| {
                crate::runtime::subagent::physical_recovery::prove(
                    &root,
                    target.session_id(),
                    &conversation,
                    &activation,
                )?
                .ok_or_else(|| {
                    invalid(format!(
                        "physical activation {activation} remains unresolved"
                    ))
                })
            })
            .collect::<std::io::Result<Vec<_>>>()?,
        })
    }
}
