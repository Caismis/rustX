//! Native Session/Conversation ownership facts shared by inspection and deletion.
//! One global identity map, derived only under the product ownership freeze.
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use super::session::{SessionCatalog, SessionId, SessionNode};
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::events::types::RuntimeEvent;
use crate::runtime::identity::{AgentId, ConversationId};
use crate::runtime::local_storage::{OwnershipSnapshot, ProductRoot};

/// A lineage and its exclusive private allocation, never a workspace allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedConversation {
    pub conversation_id: ConversationId,
    pub parent_conversation: Option<ConversationId>,
    pub private_root: PathBuf,
    pub database: PathBuf,
    pub inspection_socket: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum OwnershipInspectionError {
    Invalid,
    DeletedConversation,
    ConversationUnavailable { descendant: bool },
    Cancelled,
    UnknownSession,
}
impl std::fmt::Display for OwnershipInspectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid or ambiguous durable Conversation ownership",
            Self::DeletedConversation => {
                "live ownership references a deleted Conversation identity"
            }
            Self::ConversationUnavailable { descendant: true } => {
                "required descendant ownership history unavailable"
            }
            Self::ConversationUnavailable { descendant: false } => {
                "required Conversation ownership history unavailable"
            }
            Self::Cancelled => "ownership inspection cancelled",
            Self::UnknownSession => "unknown Session",
        })
    }
}
impl std::error::Error for OwnershipInspectionError {}
use OwnershipInspectionError as Error;

pub(crate) struct SelectedSessionOwnership {
    pub(crate) nodes: Vec<SessionNode>,
    pub(crate) conversations: Vec<OwnedConversation>,
}

pub(crate) struct SessionOwnership {
    nodes: BTreeMap<SessionId, Vec<SessionNode>>,
    conversations: BTreeMap<ConversationId, (SessionId, OwnedConversation)>,
}
impl SessionOwnership {
    /// Every root/child claim enters one global map before it is visited. The
    /// borrowed freeze must cover catalog reading, traversal and the caller's
    /// subsequent use of these facts (cut capture or deletion revision).
    pub(crate) fn inspect(
        root: &ProductRoot,
        _freeze: &OwnershipSnapshot,
        catalog: &SessionCatalog,
        check: impl Fn() -> std::io::Result<()>,
    ) -> Result<Self, Error> {
        let nodes = catalog.ownership_nodes();
        let mut pending = VecDeque::new();
        for (owner, roots) in &nodes {
            for node in roots {
                pending.push_back((owner.clone(), node.conversation_id.clone(), None));
            }
        }
        let mut conversations = BTreeMap::new();
        let mut agent_owners = BTreeMap::new();
        let mut unavailable = None;
        while let Some((owner, id, parent)) = pending.pop_front() {
            check().map_err(|_| Error::Cancelled)?;
            safe_identity(&id)?;
            if catalog.conversation_is_deleted(&id) {
                return Err(Error::DeletedConversation);
            }
            if conversations.contains_key(&id) {
                return Err(Error::Invalid);
            }
            let database = root
                .confined(&catalog.database_path(&owner, &id))
                .map_err(|_| Error::Invalid)?;
            let private_root = database.parent().ok_or(Error::Invalid)?.to_path_buf();
            let descendant = parent.is_some();
            let inspection_socket = if descendant {
                Some(
                    root.confined(
                        &crate::runtime::subagent::child_conversation_inspection_socket_path(
                            root.root(),
                            &id,
                        ),
                    )
                    .map_err(|_| Error::Invalid)?,
                )
            } else {
                None
            };
            root.confined(&private_root.join("tool-output"))
                .map_err(|_| Error::Invalid)?;
            conversations.insert(
                id.clone(),
                (
                    owner.clone(),
                    OwnedConversation {
                        conversation_id: id.clone(),
                        parent_conversation: parent,
                        private_root,
                        database: database.clone(),
                        inspection_socket,
                    },
                ),
            );
            let Ok(store) = SqliteConversationStore::open_existing(id.clone(), &database) else {
                // Continue validating other reachable claims: ambiguity must not
                // depend on which Session's unavailable allocation was visited first.
                unavailable.get_or_insert(Error::ConversationUnavailable { descendant });
                continue;
            };
            for (child, agent) in read_children(&store, &check)? {
                if agent_owners.insert(agent, (id.clone(), child.clone())).is_some() {
                    return Err(Error::Invalid);
                }
                pending.push_back((owner.clone(), child, Some(id.clone())));
            }
        }
        if let Some(error) = unavailable {
            return Err(error);
        }
        Ok(Self {
            nodes,
            conversations,
        })
    }

    pub(crate) fn select(mut self, session: &SessionId) -> Result<SelectedSessionOwnership, Error> {
        Ok(SelectedSessionOwnership {
            nodes: self.nodes.remove(session).ok_or(Error::UnknownSession)?,
            conversations: self
                .conversations
                .into_values()
                .filter_map(|(owner, conversation)| (owner == *session).then_some(conversation))
                .collect(),
        })
    }
}

/// Resolve inspection through the same global ownership facts, never orphan files.
pub(crate) fn conversation_owner(
    root: &Path,
    target: &ConversationId,
) -> std::io::Result<SessionId> {
    let root = ProductRoot::existing(root)?;
    let freeze = root.freeze_ownership()?;
    let catalog = SessionCatalog::read_under_guard(&root)
        .map_err(std::io::Error::other)?
        .ok_or_else(|| std::io::Error::other("unknown Session catalog"))?;
    let ownership = SessionOwnership::inspect(&root, &freeze, &catalog, || Ok(()))
        .map_err(std::io::Error::other)?;
    ownership
        .conversations
        .get(target)
        .map(|(owner, _)| owner.clone())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Conversation has no durable Session owner",
            )
        })
}

fn safe_identity(id: &ConversationId) -> Result<(), Error> {
    let value = id.as_str();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}

/// Native child ownership commits only. Origin references, authored content and
/// disposal state cannot add or remove Conversation ownership.
fn read_children(
    store: &SqliteConversationStore,
    check: &impl Fn() -> std::io::Result<()>,
) -> Result<BTreeMap<ConversationId, AgentId>, Error> {
    let mut children = BTreeMap::new();
    let mut agents = BTreeMap::new();
    let through = store.event_high_watermark().map_err(|_| Error::Invalid)?;
    let mut cursor = None;
    while cursor.unwrap_or(0) < through {
        check().map_err(|_| Error::Cancelled)?;
        let page = store.read_events(cursor, 256).map_err(|_| Error::Invalid)?;
        if page.events.is_empty() || page.next_sequence <= cursor {
            return Err(Error::Invalid);
        }
        cursor = page.next_sequence;
        for envelope in page.events {
            if envelope.sequence > through {
                break;
            }
            if envelope.conversation_id != *store.conversation_id() {
                return Err(Error::Invalid);
            }
            if let RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id,
                child_agent_id,
                parent_agent_id,
                child_conversation_id,
                ownership,
                admitted_authority,
                ..
            } = envelope.event
            {
                safe_identity(&child_conversation_id)?;
                if envelope.event_id
                    != crate::runtime::subagent::subagent_ownership_event_id(&subagent_id)
                {
                    return Err(Error::Invalid);
                }
                if let Some((owner, parent, durable)) = children.get(&child_conversation_id) {
                    // Activation admission is another fact about the SAME
                    // child Conversation, never another ownership edge.
                    if owner != &child_agent_id
                        || parent != &parent_agent_id
                        || !durable
                        || ownership != crate::events::types::SubagentOwnershipKind::Normal
                        || admitted_authority.is_some()
                    {
                        return Err(Error::Invalid);
                    }
                } else {
                    if (ownership == crate::events::types::SubagentOwnershipKind::Normal)
                        != admitted_authority.is_some()
                    {
                        return Err(Error::Invalid);
                    }
                    if agents
                        .insert(child_agent_id.clone(), child_conversation_id.clone())
                        .is_some()
                    {
                        return Err(Error::Invalid);
                    }
                    children.insert(
                        child_conversation_id,
                        (child_agent_id, parent_agent_id, admitted_authority.is_some()),
                    );
                }
            }
        }
    }
    Ok(children.into_iter().map(|(conversation, (agent, _, _))| (conversation, agent)).collect())
}
