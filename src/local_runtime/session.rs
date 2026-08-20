//! Native local user-session state (Issue #88).
//!
//! A product [`Session`] is deliberately above the conversation runtime. Its
//! graph contains nodes, and every node points at exactly one independent
//! durable [`ConversationId`]. Conversation tables therefore remain linear;
//! this module never adds branch columns or teaches `ConversationSurface`
//! about siblings.
//!
//! The catalog is Rust-owned durable product metadata. It is not a TUI cache,
//! and no client receives a storage path. Destination publication follows one
//! small commit protocol:
//!
//! ```text
//! prepare private conversation database + validate seed
//!     -> atomically replace catalog metadata
//!     -> destination becomes selectable
//! ```
//!
//! A failed preparation or catalog write cannot leave a visible catalog entry
//! pointing at an unusable conversation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::{SurfaceRevision, message_id_of};
use crate::durable::{ConversationStore, ConversationStoreError, SqliteConversationStore};
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, ToolMessageBlock,
    UserMessageBlock,
};
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId};

use super::config::LocalConversationConfig;

/// The persisted native session-catalog schema.
pub const SESSION_CATALOG_SCHEMA_VERSION: u32 = 1;

macro_rules! session_id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates an identity from a non-empty product-owned string.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the serialized identity.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

session_id_type! {
    /// Identifies one user-facing local Session.
    SessionId
}

session_id_type! {
    /// Identifies one linear lineage node inside a Session graph.
    SessionNodeId
}

/// Why one `SessionNode` was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionNodeOrigin {
    /// A new empty conversation lineage.
    New,
    /// A copy of an exact committed source Surface revision.
    Clone {
        /// Source Session identity.
        source_session: SessionId,
        /// Source node identity.
        source_node: SessionNodeId,
        /// Exact source Surface revision selected before materialization.
        source_surface_revision: SurfaceRevision,
    },
    /// A lineage seeded immediately before one selected user message.
    Fork {
        /// Source Session identity.
        source_session: SessionId,
        /// Source node identity.
        source_node: SessionNodeId,
        /// Exact source Surface revision selected before materialization.
        source_surface_revision: SurfaceRevision,
        /// The source user-message boundary restored into the editor.
        source_user_message: MessageId,
    },
}

/// One node in the native Session graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionNode {
    /// Node identity.
    pub id: SessionNodeId,
    /// Parent node within the same Session, when this is a tree branch.
    pub parent: Option<SessionNodeId>,
    /// The one independent linear `ConversationRuntime` lineage of this node.
    pub conversation_id: ConversationId,
    /// Immutable product-level origin metadata.
    pub origin: SessionNodeOrigin,
}

/// A bounded public snapshot of one Session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Session identity.
    pub id: SessionId,
    /// User-defined display name.
    pub name: String,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last metadata/active-node publication instant.
    pub updated_at: DateTime<Utc>,
    /// The active node selected in this Session.
    pub active_node: SessionNodeId,
    /// All persisted nodes, in deterministic node-id order.
    pub nodes: Vec<SessionNode>,
}

/// One bounded row in the `/resume` selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identity.
    pub id: SessionId,
    /// Display name.
    pub name: String,
    /// Last metadata/active-node publication instant.
    pub updated_at: DateTime<Utc>,
    /// Active node in the session.
    pub active_node: SessionNodeId,
    /// Whether this is the currently selected Session.
    pub active: bool,
}

/// One user-message boundary the native product exposes for `/fork` and
/// `/tree`. The revision is part of the selection, so later source mutations
/// cannot change what the selection means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUserMessageBoundary {
    /// The exact retained Surface revision containing the message.
    pub surface_revision: SurfaceRevision,
    /// The canonical user message at that boundary.
    pub message: UserMessageBlock,
}

/// A source snapshot selected from one immutable durable Surface revision.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalConversationSnapshot {
    /// Source `ConversationId`.
    pub conversation_id: ConversationId,
    /// The exact selected Surface revision.
    pub surface_revision: SurfaceRevision,
    /// Canonical messages active at that revision, in Surface order.
    pub messages: Vec<MessageBlock>,
}

/// A private, already-seeded destination waiting for catalog publication.
///
/// It is intentionally not visible through the Runtime Client protocol. A
/// prepared lineage is either published by `SessionCatalog` or remains an
/// unreferenced private directory.
#[derive(Debug, Clone)]
pub(crate) struct PreparedLineage {
    pub(crate) session_id: SessionId,
    pub(crate) node_id: SessionNodeId,
    pub(crate) conversation_id: ConversationId,
    pub(crate) config: LocalConversationConfig,
    pub(crate) database_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u32,
    active_session: SessionId,
    next_session_ordinal: u64,
    next_node_ordinal: u64,
    sessions: BTreeMap<SessionId, PersistedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct PersistedSession {
    id: SessionId,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    active_node: SessionNodeId,
    nodes: BTreeMap<SessionNodeId, SessionNode>,
    /// Runtime configuration is native product state. It is copied when a
    /// new Session/lineage is prepared, but never interpreted by the TUI.
    config: LocalConversationConfig,
}

/// The native durable `SessionCatalog` and graph authority.
#[derive(Debug, Clone)]
pub struct SessionCatalog {
    root: PathBuf,
    path: PathBuf,
    document: CatalogDocument,
}

impl SessionCatalog {
    /// Opens the native catalog, creating and publishing a root Session when
    /// this product root has no catalog yet.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the catalog, bootstrap configuration, or
    /// initial private conversation seed cannot be read, validated, or
    /// durably published.
    pub fn open(
        runtime_root: &Path,
        bootstrap_config: &LocalConversationConfig,
    ) -> Result<Self, SessionError> {
        let root = runtime_root.join("sessions");
        fs::create_dir_all(&root).map_err(|error| SessionError::Io {
            path: root.clone(),
            detail: error.to_string(),
        })?;
        let path = root.join("catalog.json");
        if path.exists() {
            let bytes = fs::read(&path).map_err(|error| SessionError::Io {
                path: path.clone(),
                detail: error.to_string(),
            })?;
            let document: CatalogDocument =
                serde_json::from_slice(&bytes).map_err(|error| SessionError::Catalog {
                    detail: format!("cannot decode {}: {error}", path.display()),
                })?;
            validate_document(&document)?;
            return Ok(Self {
                root,
                path,
                document,
            });
        }

        bootstrap_config
            .validate()
            .map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })?;
        validate_id(bootstrap_config.conversation_id.as_str(), "conversation")?;
        let session_id = SessionId::new("session-1");
        let node_id = SessionNodeId::new("node-1");
        let conversation_id = bootstrap_config.conversation_id.clone();
        let database_path = conversation_database_path(&root, &session_id, &conversation_id);
        initialize_database(&database_path, &conversation_id, &[])?;
        let now = Utc::now();
        let node = SessionNode {
            id: node_id.clone(),
            parent: None,
            conversation_id,
            origin: SessionNodeOrigin::New,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(node_id.clone(), node);
        let mut sessions = BTreeMap::new();
        sessions.insert(
            session_id.clone(),
            PersistedSession {
                id: session_id.clone(),
                name: "New session".to_owned(),
                created_at: now,
                updated_at: now,
                active_node: node_id,
                nodes,
                config: bootstrap_config.clone(),
            },
        );
        let document = CatalogDocument {
            schema_version: SESSION_CATALOG_SCHEMA_VERSION,
            active_session: session_id,
            next_session_ordinal: 2,
            next_node_ordinal: 2,
            sessions,
        };
        let catalog = Self {
            root,
            path,
            document,
        };
        catalog.persist(&catalog.document)?;
        Ok(catalog)
    }

    /// Returns all persisted sessions in deterministic metadata order.
    #[must_use]
    pub fn list(&self) -> Vec<SessionSummary> {
        self.document
            .sessions
            .values()
            .map(|session| SessionSummary {
                id: session.id.clone(),
                name: session.name.clone(),
                updated_at: session.updated_at,
                active_node: session.active_node.clone(),
                active: session.id == self.document.active_session,
            })
            .collect()
    }

    /// Returns the active Session snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownSession`] when the catalog's active
    /// identity is not present.
    pub fn active_snapshot(&self) -> Result<SessionSnapshot, SessionError> {
        self.snapshot(&self.document.active_session)
    }

    /// Returns one Session snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownSession`] when `id` is not persisted.
    pub fn snapshot(&self, id: &SessionId) -> Result<SessionSnapshot, SessionError> {
        let session =
            self.document
                .sessions
                .get(id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: id.clone(),
                })?;
        Ok(snapshot_of(session))
    }

    /// Returns the active Session and node configuration needed by native
    /// composition. No storage path is part of this public product view.
    pub(crate) fn active_lineage(
        &self,
    ) -> Result<(SessionId, SessionNode, LocalConversationConfig), SessionError> {
        let session = self
            .document
            .sessions
            .get(&self.document.active_session)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: self.document.active_session.clone(),
            })?;
        let node =
            session
                .nodes
                .get(&session.active_node)
                .ok_or_else(|| SessionError::UnknownNode {
                    session_id: session.id.clone(),
                    node_id: session.active_node.clone(),
                })?;
        Ok((session.id.clone(), node.clone(), session.config.clone()))
    }

    /// Returns one named lineage and its conversation configuration.
    pub(crate) fn lineage(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<(SessionNode, LocalConversationConfig), SessionError> {
        let session =
            self.document
                .sessions
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let node_id = node_id.unwrap_or(&session.active_node);
        let node = session
            .nodes
            .get(node_id)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
            })?;
        Ok((node.clone(), session.config.clone()))
    }

    /// Returns the private database path for a known node.
    pub(crate) fn database_path(
        &self,
        session_id: &SessionId,
        conversation_id: &ConversationId,
    ) -> PathBuf {
        conversation_database_path(&self.root, session_id, conversation_id)
    }

    /// Atomically changes a Session display name. This touches metadata only.
    pub(crate) fn rename(
        &mut self,
        session_id: &SessionId,
        name: &str,
    ) -> Result<SessionSnapshot, SessionError> {
        let name = normalize_name(name)?;
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        session.name = name;
        session.updated_at = Utc::now();
        self.commit(next)?;
        self.snapshot(session_id)
    }

    /// Persists the accepted live model configuration for the active
    /// Session. The `ConversationRuntime` remains the live authority; this
    /// record is used when that runtime is replaced and recovered later.
    pub(crate) fn persist_active_model(
        &mut self,
        model: crate::model::session::SessionModelConfig,
    ) -> Result<(), SessionError> {
        let mut next = self.document.clone();
        let active_session = next.active_session.clone();
        let session =
            next.sessions
                .get_mut(&active_session)
                .ok_or(SessionError::UnknownSession {
                    session_id: active_session,
                })?;
        session.config.model = model;
        session.updated_at = Utc::now();
        self.commit(next)
    }

    /// Atomically selects an existing Session/node after the caller has
    /// reached native runtime quiescence.
    pub(crate) fn select(
        &mut self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<SessionSnapshot, SessionError> {
        let (_, _) = self.lineage(session_id, node_id)?;
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let selected_node = node_id
            .cloned()
            .unwrap_or_else(|| session.active_node.clone());
        session.active_node = selected_node.clone();
        session.config.conversation_id = session
            .nodes
            .get(&selected_node)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: selected_node.clone(),
            })?
            .conversation_id
            .clone();
        session.updated_at = Utc::now();
        next.active_session = session_id.clone();
        self.commit(next)?;
        self.snapshot(session_id)
    }

    /// Prepares a new independent Session with a fresh `ConversationId`.
    pub(crate) fn prepare_session(
        &self,
        template: &LocalConversationConfig,
        seed: &[MessageBlock],
    ) -> Result<PreparedLineage, SessionError> {
        let (session_id, node_id, conversation_id) = self.allocate_ids();
        self.prepare_session_with_ids(template, session_id, node_id, conversation_id, seed)
    }

    /// Prepares a clone from the exact source revision selected by the
    /// caller. The source revision and source message bodies are immutable
    /// inputs to this preparation.
    pub(crate) fn prepare_clone_session(
        &self,
        template: &LocalConversationConfig,
        source: &HistoricalConversationSnapshot,
    ) -> Result<PreparedLineage, SessionError> {
        let (session_id, node_id, conversation_id) = self.allocate_ids();
        let seed = remap_seed(&conversation_id, &source.messages)?;
        self.prepare_session_with_ids(template, session_id, node_id, conversation_id, &seed)
    }

    /// Prepares an independent Session fork and returns the selected original
    /// user prompt for uncommitted editor restoration.
    pub(crate) fn prepare_fork_session(
        &self,
        template: &LocalConversationConfig,
        source: &HistoricalConversationSnapshot,
        message_id: &MessageId,
    ) -> Result<
        (
            PreparedLineage,
            Vec<crate::message::types::UserContentBlock>,
        ),
        SessionError,
    > {
        let index = source
            .messages
            .iter()
            .position(|message| {
                matches!(message, MessageBlock::User(user) if user.id == *message_id
                    && user.kind == InboundKind::Message)
            })
            .ok_or_else(|| SessionError::UnknownBoundary {
                message_id: message_id.clone(),
            })?;
        let MessageBlock::User(user) = &source.messages[index] else {
            return Err(SessionError::UnknownBoundary {
                message_id: message_id.clone(),
            });
        };
        let editor_content = user.content.clone();
        let (session_id, node_id, conversation_id) = self.allocate_ids();
        let seed = remap_seed(&conversation_id, &source.messages[..index])?;
        let prepared =
            self.prepare_session_with_ids(template, session_id, node_id, conversation_id, &seed)?;
        Ok((prepared, editor_content))
    }

    /// Prepares a tree node with the same user-boundary semantics as `/fork`:
    /// canonical seed is the exact prefix before the selected user message,
    /// and the selected prompt is returned separately for editor restoration.
    pub(crate) fn prepare_tree_node_at_user_message(
        &self,
        session_id: &SessionId,
        template: &LocalConversationConfig,
        source: &HistoricalConversationSnapshot,
        message_id: &MessageId,
    ) -> Result<
        (
            PreparedLineage,
            Vec<crate::message::types::UserContentBlock>,
        ),
        SessionError,
    > {
        let index = source
            .messages
            .iter()
            .position(|message| {
                matches!(message, MessageBlock::User(user) if user.id == *message_id
                    && user.kind == InboundKind::Message)
            })
            .ok_or_else(|| SessionError::UnknownBoundary {
                message_id: message_id.clone(),
            })?;
        let MessageBlock::User(user) = &source.messages[index] else {
            return Err(SessionError::UnknownBoundary {
                message_id: message_id.clone(),
            });
        };
        let editor_content = user.content.clone();
        let mut node_ordinal = self.document.next_node_ordinal.max(1);
        let (node_id, conversation_id, database_path) = loop {
            let node_id = SessionNodeId::new(format!("node-{node_ordinal}"));
            let conversation_id = ConversationId::new(format!("conversation-node-{node_ordinal}"));
            let database_path =
                conversation_database_path(&self.root, session_id, &conversation_id);
            let node_id_taken = self
                .document
                .sessions
                .values()
                .any(|session| session.nodes.contains_key(&node_id));
            if !node_id_taken && !database_path.exists() {
                break (node_id, conversation_id, database_path);
            }
            node_ordinal = node_ordinal.saturating_add(1);
        };
        let seed = remap_seed(&conversation_id, &source.messages[..index])?;
        initialize_database(&database_path, &conversation_id, &seed)?;
        let mut config = template.clone();
        config.conversation_id = conversation_id.clone();
        Ok((
            PreparedLineage {
                session_id: session_id.clone(),
                node_id,
                conversation_id,
                config,
                database_path,
            },
            editor_content,
        ))
    }

    fn prepare_session_with_ids(
        &self,
        template: &LocalConversationConfig,
        session_id: SessionId,
        node_id: SessionNodeId,
        conversation_id: ConversationId,
        seed: &[MessageBlock],
    ) -> Result<PreparedLineage, SessionError> {
        let database_path = conversation_database_path(&self.root, &session_id, &conversation_id);
        initialize_database(&database_path, &conversation_id, seed)?;
        let mut config = template.clone();
        config.conversation_id = conversation_id.clone();
        Ok(PreparedLineage {
            session_id,
            node_id,
            conversation_id,
            config,
            database_path,
        })
    }

    /// Publishes a prepared independent Session and makes it active.
    pub(crate) fn publish_session(
        &mut self,
        prepared: PreparedLineage,
        name: &str,
        origin: SessionNodeOrigin,
    ) -> Result<SessionSnapshot, SessionError> {
        if self.document.sessions.contains_key(&prepared.session_id) {
            return Err(SessionError::Catalog {
                detail: format!(
                    "prepared Session {} has already been published",
                    prepared.session_id
                ),
            });
        }
        if !prepared.database_path.is_file() {
            return Err(SessionError::Catalog {
                detail: "prepared conversation seed is not a database file".to_owned(),
            });
        }
        let name = normalize_name(name)?;
        let now = Utc::now();
        let node = SessionNode {
            id: prepared.node_id.clone(),
            parent: None,
            conversation_id: prepared.conversation_id,
            origin,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(prepared.node_id.clone(), node);
        let mut next = self.document.clone();
        next.sessions.insert(
            prepared.session_id.clone(),
            PersistedSession {
                id: prepared.session_id.clone(),
                name,
                created_at: now,
                updated_at: now,
                active_node: prepared.node_id,
                nodes,
                config: prepared.config,
            },
        );
        next.active_session = prepared.session_id.clone();
        next.next_session_ordinal = next.next_session_ordinal.saturating_add(1);
        next.next_node_ordinal = next.next_node_ordinal.saturating_add(1);
        self.commit(next)?;
        self.snapshot(&prepared.session_id)
    }

    /// Publishes a prepared branch node inside an existing Session and makes
    /// it the active node.
    pub(crate) fn publish_node(
        &mut self,
        session_id: &SessionId,
        prepared: PreparedLineage,
        parent: SessionNodeId,
        origin: SessionNodeOrigin,
    ) -> Result<SessionSnapshot, SessionError> {
        if prepared.session_id != *session_id {
            return Err(SessionError::Catalog {
                detail: "prepared node belongs to another Session".to_owned(),
            });
        }
        if !prepared.database_path.is_file() {
            return Err(SessionError::Catalog {
                detail: "prepared conversation seed is not a database file".to_owned(),
            });
        }
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        if !session.nodes.contains_key(&parent) {
            return Err(SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: parent,
            });
        }
        session.config.conversation_id = prepared.conversation_id.clone();
        let node = SessionNode {
            id: prepared.node_id.clone(),
            parent: Some(parent),
            conversation_id: prepared.conversation_id,
            origin,
        };
        session.nodes.insert(prepared.node_id.clone(), node);
        session.active_node = prepared.node_id;
        session.updated_at = Utc::now();
        next.active_session = session_id.clone();
        next.next_node_ordinal = next.next_node_ordinal.saturating_add(1);
        self.commit(next)?;
        self.snapshot(session_id)
    }

    /// Validates that a selected node still has a coherent durable store.
    /// This is performed before active-selection publication.
    pub(crate) fn validate_storage(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<(), SessionError> {
        let (node, _) = self.lineage(session_id, node_id)?;
        let path = self.database_path(session_id, &node.conversation_id);
        let store = SqliteConversationStore::open(node.conversation_id, &path)
            .map_err(SessionError::Store)?;
        store.load_head().map_err(SessionError::Store).map(|_| ())
    }

    fn allocate_ids(&self) -> (SessionId, SessionNodeId, ConversationId) {
        let mut session_ordinal = self.document.next_session_ordinal.max(1);
        let mut node_ordinal = self.document.next_node_ordinal.max(1);
        loop {
            let session_id = SessionId::new(format!("session-{session_ordinal}"));
            let node_id = SessionNodeId::new(format!("node-{node_ordinal}"));
            let conversation_id = ConversationId::new(format!("conversation-{session_ordinal}"));
            let db = conversation_database_path(&self.root, &session_id, &conversation_id);
            let node_taken = self
                .document
                .sessions
                .values()
                .any(|session| session.nodes.contains_key(&node_id));
            let conversation_taken = self
                .document
                .sessions
                .values()
                .flat_map(|session| session.nodes.values())
                .any(|node| node.conversation_id == conversation_id);
            if !self.document.sessions.contains_key(&session_id)
                && !node_taken
                && !conversation_taken
                && !db.exists()
            {
                return (session_id, node_id, conversation_id);
            }
            session_ordinal = session_ordinal.saturating_add(1);
            node_ordinal = node_ordinal.saturating_add(1);
        }
    }

    fn commit(&mut self, next: CatalogDocument) -> Result<(), SessionError> {
        validate_document(&next)?;
        self.persist(&next)?;
        self.document = next;
        Ok(())
    }

    fn persist(&self, document: &CatalogDocument) -> Result<(), SessionError> {
        let bytes = serde_json::to_vec_pretty(document).map_err(|error| SessionError::Catalog {
            detail: format!("cannot encode catalog: {error}"),
        })?;
        atomic_write(&self.path, &bytes)
    }
}

/// Reconstructs a destination seed with destination-owned message and tool
/// identities. Runtime lifecycle identities are not present in this input and
/// therefore cannot leak into the destination.
pub(crate) fn remap_seed(
    destination: &ConversationId,
    messages: &[MessageBlock],
) -> Result<Vec<MessageBlock>, SessionError> {
    let mut message_ids = BTreeMap::new();
    let mut call_ids = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let old = message_id_of(message);
        let new = MessageId::new(format!("{destination}-message-{}", index + 1));
        if message_ids.insert(old.clone(), new).is_some() {
            return Err(SessionError::Seed {
                detail: format!("source seed repeats MessageId {old}"),
            });
        }
        if let MessageBlock::Assistant(assistant) = message {
            for (call_index, content) in assistant.content.iter().enumerate() {
                if let AssistantContentBlock::ToolCall(call) = content {
                    let new_call = ToolCallId::new(format!(
                        "{destination}-tool-call-{}-{}",
                        index + 1,
                        call_index + 1
                    ));
                    if call_ids.insert(call.id.clone(), new_call).is_some() {
                        return Err(SessionError::Seed {
                            detail: format!("source seed repeats ToolCallId {}", call.id),
                        });
                    }
                }
            }
        }
    }

    messages
        .iter()
        .map(|message| remap_message(message, &message_ids, &call_ids))
        .collect()
}

fn remap_message(
    message: &MessageBlock,
    message_ids: &BTreeMap<MessageId, MessageId>,
    call_ids: &BTreeMap<ToolCallId, ToolCallId>,
) -> Result<MessageBlock, SessionError> {
    let message_id = |id: &MessageId| {
        message_ids
            .get(id)
            .cloned()
            .ok_or_else(|| SessionError::Seed {
                detail: format!("source seed references unknown MessageId {id}"),
            })
    };
    let call_id = |id: &ToolCallId| {
        call_ids.get(id).cloned().ok_or_else(|| SessionError::Seed {
            detail: format!("source seed references unknown ToolCallId {id}"),
        })
    };

    match message {
        MessageBlock::System(system) => Ok(MessageBlock::System(
            crate::message::types::SystemMessageBlock {
                id: message_id(&system.id)?,
                authority: system.authority,
                content: system.content.clone(),
            },
        )),
        MessageBlock::User(user) => Ok(MessageBlock::User(UserMessageBlock {
            id: message_id(&user.id)?,
            content: user.content.clone(),
            source: user.source.clone(),
            kind: user.kind.clone(),
            timestamp: user.timestamp,
        })),
        MessageBlock::Assistant(assistant) => Ok(MessageBlock::Assistant(AssistantMessageBlock {
            id: message_id(&assistant.id)?,
            content: assistant
                .content
                .iter()
                .map(|content| match content {
                    AssistantContentBlock::ToolCall(call) => Ok(AssistantContentBlock::ToolCall(
                        crate::tools::types::ToolCall {
                            id: call_id(&call.id)?,
                            tool_id: call.tool_id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        },
                    )),
                    other => Ok(other.clone()),
                })
                .collect::<Result<Vec<_>, SessionError>>()?,
        })),
        MessageBlock::Tool(tool) => Ok(MessageBlock::Tool(ToolMessageBlock {
            id: message_id(&tool.id)?,
            tool_call_id: call_id(&tool.tool_call_id)?,
            tool_id: tool.tool_id.clone(),
            result: tool.result.clone(),
        })),
    }
}

fn snapshot_of(session: &PersistedSession) -> SessionSnapshot {
    SessionSnapshot {
        id: session.id.clone(),
        name: session.name.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        active_node: session.active_node.clone(),
        nodes: session.nodes.values().cloned().collect(),
    }
}

fn validate_document(document: &CatalogDocument) -> Result<(), SessionError> {
    if document.schema_version != SESSION_CATALOG_SCHEMA_VERSION {
        return Err(SessionError::Catalog {
            detail: format!(
                "unsupported session catalog schema {}; expected {}",
                document.schema_version, SESSION_CATALOG_SCHEMA_VERSION
            ),
        });
    }
    if document.sessions.is_empty() {
        return Err(SessionError::Catalog {
            detail: "session catalog must contain at least one session".to_owned(),
        });
    }
    let mut conversation_ids = BTreeSet::new();
    let mut node_ids = BTreeSet::new();
    for (session_id, session) in &document.sessions {
        validate_id(session_id.as_str(), "session")?;
        if session.id != *session_id {
            return Err(SessionError::Catalog {
                detail: format!("session key {session_id} disagrees with its record identity"),
            });
        }
        if session.nodes.is_empty() {
            return Err(SessionError::Catalog {
                detail: format!("session {session_id} has no nodes"),
            });
        }
        validate_active_config(session_id, session)?;
        session
            .config
            .validate()
            .map_err(|error| SessionError::Catalog {
                detail: format!("session {session_id} config: {error}"),
            })?;
        for (node_id, node) in &session.nodes {
            validate_id(node_id.as_str(), "node")?;
            if node.id != *node_id {
                return Err(SessionError::Catalog {
                    detail: format!("session {session_id} contains an invalid node {node_id}"),
                });
            }
            if !node_ids.insert(node.id.clone()) {
                return Err(SessionError::Catalog {
                    detail: format!("SessionNode identity {node_id} is duplicated"),
                });
            }
            validate_id(node.conversation_id.as_str(), "conversation")?;
            if !conversation_ids.insert(node.conversation_id.clone()) {
                return Err(SessionError::Catalog {
                    detail: format!(
                        "conversation {} is mapped by more than one SessionNode",
                        node.conversation_id
                    ),
                });
            }
            if let Some(parent) = &node.parent
                && !session.nodes.contains_key(parent)
            {
                return Err(SessionError::Catalog {
                    detail: format!("node {node_id} points at missing parent {parent}"),
                });
            }
        }
        for node in session.nodes.values() {
            let mut seen = BTreeSet::new();
            let mut current = Some(node.id.clone());
            while let Some(node_id) = current {
                if !seen.insert(node_id.clone()) {
                    return Err(SessionError::Catalog {
                        detail: format!("Session {session_id} graph contains a parent cycle"),
                    });
                }
                current = session
                    .nodes
                    .get(&node_id)
                    .and_then(|parent| parent.parent.clone());
            }
        }
    }
    if !document.sessions.contains_key(&document.active_session) {
        return Err(SessionError::Catalog {
            detail: format!("active session {} is missing", document.active_session),
        });
    }
    Ok(())
}

fn validate_active_config(
    session_id: &SessionId,
    session: &PersistedSession,
) -> Result<(), SessionError> {
    let Some(active_node) = session.nodes.get(&session.active_node) else {
        return Err(SessionError::Catalog {
            detail: format!(
                "session {session_id} selects missing node {}",
                session.active_node
            ),
        });
    };
    if session.config.conversation_id != active_node.conversation_id {
        return Err(SessionError::Catalog {
            detail: format!(
                "session {session_id} config conversation {} disagrees with active node conversation {}",
                session.config.conversation_id, active_node.conversation_id
            ),
        });
    }
    Ok(())
}

fn validate_id(value: &str, kind: &str) -> Result<(), SessionError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(SessionError::Catalog {
            detail: format!("{kind} identity is not a safe product id"),
        });
    }
    Ok(())
}

fn normalize_name(name: &str) -> Result<String, SessionError> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(SessionError::InvalidName);
    }
    Ok(name)
}

fn conversation_database_path(
    root: &Path,
    session_id: &SessionId,
    conversation_id: &ConversationId,
) -> PathBuf {
    root.join(session_id.as_str())
        .join("conversations")
        .join(conversation_id.as_str())
        .join("conversation.sqlite")
}

fn initialize_database(
    path: &Path,
    conversation_id: &ConversationId,
    seed: &[MessageBlock],
) -> Result<(), SessionError> {
    let parent = path.parent().ok_or_else(|| SessionError::Io {
        path: path.to_path_buf(),
        detail: "conversation database has no parent".to_owned(),
    })?;
    fs::create_dir_all(parent).map_err(|error| SessionError::Io {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })?;
    let store = SqliteConversationStore::open(conversation_id.clone(), path)
        .map_err(SessionError::Store)?;
    store.initialize(seed).map_err(SessionError::Store)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    let parent = path.parent().ok_or_else(|| SessionError::Io {
        path: path.to_path_buf(),
        detail: "catalog has no parent".to_owned(),
    })?;
    fs::create_dir_all(parent).map_err(|error| SessionError::Io {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })?;
    let temporary = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| SessionError::Io {
            path: temporary.clone(),
            detail: error.to_string(),
        })?;
    file.write_all(bytes).map_err(|error| SessionError::Io {
        path: temporary.clone(),
        detail: error.to_string(),
    })?;
    file.sync_all().map_err(|error| SessionError::Io {
        path: temporary.clone(),
        detail: error.to_string(),
    })?;
    fs::rename(&temporary, path).map_err(|error| SessionError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let directory = File::open(parent).map_err(|error| SessionError::Io {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })?;
    directory.sync_all().map_err(|error| SessionError::Io {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })
}

/// A native Session-domain failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// A storage operation failed.
    Io { path: PathBuf, detail: String },
    /// The catalog or its graph is malformed.
    Catalog { detail: String },
    /// Durable conversation storage rejected a seed or validation read.
    Store(ConversationStoreError),
    /// Historical materialization or identity remapping rejected the source.
    Seed { detail: String },
    /// A requested Session does not exist.
    UnknownSession { session_id: SessionId },
    /// A requested node does not exist.
    UnknownNode {
        session_id: SessionId,
        node_id: SessionNodeId,
    },
    /// A requested user boundary does not exist in the selected revision.
    UnknownBoundary { message_id: MessageId },
    /// Session names are bounded non-empty metadata.
    InvalidName,
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io { path, detail } => write!(f, "session storage {}: {detail}", path.display()),
            Self::Catalog { detail } => write!(f, "session catalog: {detail}"),
            Self::Store(error) => write!(f, "conversation seed: {error}"),
            Self::Seed { detail } => write!(f, "conversation seed: {detail}"),
            Self::UnknownSession { session_id } => write!(f, "unknown Session {session_id}"),
            Self::UnknownNode {
                session_id,
                node_id,
            } => {
                write!(f, "unknown node {node_id} in Session {session_id}")
            }
            Self::UnknownBoundary { message_id } => {
                write!(f, "unknown fork boundary user message {message_id}")
            }
            Self::InvalidName => {
                f.write_str("session name must be 1-120 non-whitespace characters")
            }
        }
    }
}

impl std::error::Error for SessionError {}

#[cfg(test)]
mod tests {
    use super::{HistoricalConversationSnapshot, SessionCatalog, SessionError, SessionNodeOrigin};
    use crate::conversation::SurfaceRevision;
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::local_runtime::LocalConversationConfig;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, SystemAuthority,
        SystemMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
    use tempfile::TempDir;

    const CONFIG: &str = r#"{
        "conversationId": "conversation-root",
        "agentId": "agent-a",
        "model": {"model": "provider/model"},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
    }"#;

    fn config() -> LocalConversationConfig {
        LocalConversationConfig::from_json_slice(CONFIG.as_bytes()).expect("valid test config")
    }

    fn text(value: &str) -> UserContentBlock {
        UserContentBlock::Text(TextBlock {
            text: value.to_owned(),
        })
    }

    fn user(id: &str, value: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![text(value)],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    fn source_history() -> Vec<MessageBlock> {
        vec![
            MessageBlock::System(SystemMessageBlock {
                id: MessageId::new("source-system"),
                authority: SystemAuthority::Runtime,
                content: vec![TextBlock {
                    text: "bootstrap".to_owned(),
                }],
            }),
            user("source-user-a", "A"),
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("source-assistant"),
                content: vec![
                    AssistantContentBlock::Text(TextBlock {
                        text: "B".to_owned(),
                    }),
                    AssistantContentBlock::ToolCall(ToolCall {
                        id: ToolCallId::new("source-call"),
                        tool_id: ToolId::new("tool-test"),
                        name: "test_tool".to_owned(),
                        arguments: serde_json::json!({"value": 1}),
                    }),
                ],
            }),
            MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("source-tool-result"),
                tool_call_id: ToolCallId::new("source-call"),
                tool_id: ToolId::new("tool-test"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 1,
                    exit_code: Some(0),
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            }),
            user("source-user-c", "C"),
        ]
    }

    fn open_catalog() -> (TempDir, SessionCatalog, LocalConversationConfig) {
        let directory = tempfile::tempdir().expect("temp directory");
        let config = config();
        let catalog = SessionCatalog::open(directory.path(), &config).expect("catalog");
        (directory, catalog, config)
    }

    fn append_history(
        catalog: &SessionCatalog,
        history: &[MessageBlock],
    ) -> (
        ConversationId,
        crate::local_runtime::SessionId,
        crate::local_runtime::SessionNodeId,
    ) {
        let (session_id, node, _) = catalog.active_lineage().expect("root lineage");
        let path = catalog.database_path(&session_id, &node.conversation_id);
        let store =
            SqliteConversationStore::open(node.conversation_id.clone(), &path).expect("root store");
        for message in history {
            store
                .append_canonical(message)
                .expect("append canonical history");
        }
        (node.conversation_id, session_id, node.id)
    }

    fn store_for(
        catalog: &SessionCatalog,
        session_id: &crate::local_runtime::SessionId,
        conversation_id: &ConversationId,
    ) -> SqliteConversationStore {
        SqliteConversationStore::open(
            conversation_id.clone(),
            &catalog.database_path(session_id, conversation_id),
        )
        .expect("conversation store")
    }

    #[test]
    fn new_and_name_publish_metadata_without_mutating_old_history() {
        let (_directory, mut catalog, config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let before = source_store.load_canonical().expect("source history");

        let prepared = catalog
            .prepare_session(&config, &[])
            .expect("prepare new session");
        let new_session_id = prepared.session_id.clone();
        let snapshot = catalog
            .publish_session(prepared, "New session", SessionNodeOrigin::New)
            .expect("publish new session");

        assert_ne!(snapshot.id, source_session);
        assert_ne!(
            snapshot.nodes[0].conversation_id, source_conversation,
            "new Session must own a new ConversationId"
        );
        assert_eq!(catalog.list().len(), 2);
        assert_eq!(
            source_store
                .load_canonical()
                .expect("source history after new"),
            before
        );

        let renamed = catalog
            .rename(&new_session_id, "review branch")
            .expect("rename metadata");
        assert_eq!(renamed.name, "review branch");
        let new_node = &renamed.nodes[0];
        let new_store = store_for(&catalog, &new_session_id, &new_node.conversation_id);
        assert!(
            new_store
                .load_canonical()
                .expect("new canonical history")
                .is_empty(),
            "new starts with only the intended empty bootstrap state"
        );
    }

    #[test]
    fn accepted_model_configuration_is_catalog_metadata_for_runtime_replacement() {
        let (directory, mut catalog, config) = open_catalog();
        let mut model = config.model.clone();
        model.model = serde_json::from_value(serde_json::json!("provider/next-model"))
            .expect("model reference");
        catalog
            .persist_active_model(model.clone())
            .expect("persist model metadata");

        let reopened = SessionCatalog::open(directory.path(), &config).expect("reopen catalog");
        let (_, _, reopened_config) = reopened.active_lineage().expect("active lineage");
        assert_eq!(reopened_config.model, model);
    }

    #[test]
    fn clone_uses_exact_revision_and_isolates_execution_identity_domains() {
        let (_directory, mut catalog, config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let selected_revision = source_store.load_head().expect("source head").revision;
        let source_snapshot = HistoricalConversationSnapshot {
            conversation_id: source_conversation.clone(),
            surface_revision: selected_revision,
            messages: source_store
                .load_surface_snapshot(selected_revision)
                .expect("historical source snapshot"),
        };

        let prepared = catalog
            .prepare_clone_session(&config, &source_snapshot)
            .expect("prepare exact clone");
        let destination_session = prepared.session_id.clone();
        let destination_conversation = prepared.conversation_id.clone();
        let destination_store =
            store_for(&catalog, &destination_session, &destination_conversation);
        let cloned_before_source_mutation = destination_store
            .load_canonical()
            .expect("clone canonical history");

        source_store
            .append_canonical(&user("source-late", "later source work"))
            .expect("mutate source after selected revision");
        assert_eq!(
            destination_store
                .load_canonical()
                .expect("clone remains independent"),
            cloned_before_source_mutation
        );

        let assistant = cloned_before_source_mutation
            .iter()
            .find_map(|message| match message {
                MessageBlock::Assistant(message) => Some(message),
                _ => None,
            })
            .expect("cloned assistant");
        let call = assistant
            .content
            .iter()
            .find_map(|content| match content {
                AssistantContentBlock::ToolCall(call) => Some(call),
                _ => None,
            })
            .expect("cloned tool call");
        assert_ne!(assistant.id, MessageId::new("source-assistant"));
        assert_ne!(call.id, ToolCallId::new("source-call"));
        let result = cloned_before_source_mutation
            .iter()
            .find_map(|message| match message {
                MessageBlock::Tool(message) => Some(message),
                _ => None,
            })
            .expect("cloned tool result");
        assert_eq!(result.tool_call_id, call.id);

        assert!(
            destination_store
                .read_events(None, 64)
                .expect("destination events")
                .events
                .is_empty()
        );
        assert!(
            destination_store
                .read_request_snapshots(None, 64)
                .expect("destination request snapshots")
                .snapshots
                .is_empty()
        );

        let destination = catalog
            .publish_session(
                prepared,
                "Clone of session-1",
                SessionNodeOrigin::Clone {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: selected_revision,
                },
            )
            .expect("publish clone");
        assert_eq!(destination.id, destination_session);
        assert_eq!(
            destination.nodes[0].conversation_id,
            destination_conversation
        );
        assert_ne!(source_conversation, destination.nodes[0].conversation_id);
    }

    #[test]
    fn fork_seeds_before_user_and_returns_uncommitted_prompt() {
        let (_directory, catalog, config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = HistoricalConversationSnapshot {
            conversation_id: source_conversation,
            surface_revision: revision,
            messages: source_store
                .load_surface_snapshot(revision)
                .expect("historical source"),
        };

        let (prepared, editor_content) = catalog
            .prepare_fork_session(&config, &source, &MessageId::new("source-user-c"))
            .expect("prepare fork");
        assert_eq!(editor_content, vec![text("C")]);
        let destination_store =
            store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
        let prefix = destination_store
            .load_canonical()
            .expect("fork canonical prefix");
        assert_eq!(prefix.len(), 4);
        assert!(prefix.iter().all(|message| {
            !matches!(message, MessageBlock::User(user) if user.id == MessageId::new("source-user-c"))
        }));

        let source_after_prepare = source
            .messages
            .iter()
            .map(super::message_id_of)
            .collect::<Vec<_>>();
        assert_eq!(source_after_prepare.len(), 5);
        assert_eq!(
            source_store
                .load_canonical()
                .expect("source remains untouched")
                .len(),
            5
        );
    }

    #[test]
    fn tree_branch_is_a_distinct_linear_node_and_failed_publication_is_invisible() {
        let (_directory, mut catalog, config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = HistoricalConversationSnapshot {
            conversation_id: source_conversation.clone(),
            surface_revision: revision,
            messages: source_store
                .load_surface_snapshot(revision)
                .expect("historical source"),
        };

        let before_failed = catalog.snapshot(&source_session).expect("source snapshot");
        let failed = catalog
            .prepare_session(&config, &[])
            .expect("prepare private destination");
        std::fs::remove_file(&failed.database_path).expect("remove private seed");
        assert!(matches!(
            catalog.publish_session(failed, "invisible", SessionNodeOrigin::New),
            Err(SessionError::Catalog { .. })
        ));
        assert_eq!(
            catalog
                .snapshot(&source_session)
                .expect("after failed publication"),
            before_failed
        );

        let (prepared, _editor) = catalog
            .prepare_tree_node_at_user_message(
                &source_session,
                &config,
                &source,
                &MessageId::new("source-user-c"),
            )
            .expect("prepare tree node");
        let branch_conversation = prepared.conversation_id.clone();
        let snapshot = catalog
            .publish_node(
                &source_session,
                prepared,
                source_node.clone(),
                SessionNodeOrigin::Fork {
                    source_session: source_session.clone(),
                    source_node,
                    source_surface_revision: revision,
                    source_user_message: MessageId::new("source-user-c"),
                },
            )
            .expect("publish tree node");
        assert_eq!(snapshot.nodes.len(), 2);
        assert_eq!(snapshot.active_node, snapshot.nodes[1].id);
        assert_ne!(snapshot.nodes[0].conversation_id, branch_conversation);
        assert!(
            snapshot
                .nodes
                .iter()
                .all(|node| node.conversation_id != source_conversation
                    || node.origin == SessionNodeOrigin::New)
        );
        assert_eq!(catalog.list().len(), 1, "tree branch stays in one Session");

        // Session and node ordinals are independent domains. A new Session
        // after a tree branch must not reuse the branch's globally unique
        // SessionNodeId.
        let new_session = catalog
            .prepare_session(&config, &[])
            .expect("prepare new session after tree branch");
        assert_ne!(new_session.node_id, snapshot.active_node);
        catalog
            .publish_session(new_session, "after branch", SessionNodeOrigin::New)
            .expect("publish new session after tree branch");
        assert_eq!(catalog.list().len(), 2);
    }

    #[test]
    fn surface_revision_is_a_linear_history_boundary() {
        let (_directory, catalog, _config) = open_catalog();
        let (conversation, session, _node) = append_history(&catalog, &source_history());
        let store = store_for(&catalog, &session, &conversation);
        let first = store
            .load_surface_snapshot(SurfaceRevision::new(2))
            .expect("historical revision");
        let head = store.load_head().expect("head");
        assert_eq!(first.len(), 2);
        assert!(head.revision > SurfaceRevision::new(2));
    }
}
