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
//! small commit protocol with an explicit visibility point:
//!
//! ```text
//! prepare private conversation database + validate seed
//!     -> write/fsync temporary catalog
//!     -> rename temporary catalog (visibility commit point)
//!     -> destination is visible to catalog readers
//!     -> fsync parent directory (durability barrier)
//!     -> publication success is reported
//! ```
//!
//! A pre-rename failure leaves the old document authoritative. A post-rename
//! durability failure reports that visibility committed but durability is
//! uncertain and keeps the in-memory document aligned with the visible file.
//! A failed preparation or catalog write cannot leave a visible catalog entry
//! pointing at an unusable conversation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::{SurfaceOp, SurfaceRevision, apply_surface_op, message_id_of};
use crate::durable::{
    ConversationStore, ConversationStoreError, LineageSeed, SqliteConversationStore,
};
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock,
};
use crate::model::session::SessionModelConfig;
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId};

/// The persisted native session-catalog schema.
///
/// The version gates *meaning*, not layout. Version 4 is the first catalog
/// whose `Clone` and `Fork` origins promise that the destination lineage
/// retained the source's Surface operation history — the provenance
/// `lineage_cut` copies and every later `/fork` or `/tree` of the copy
/// reads back. A version-3 catalog carries the same fields and the same
/// origin records, and its destinations were seeded by flattening the source
/// Surface into one append per active message: a history that never happened,
/// in which a compaction summary appears to predate the user message it
/// actually postdates.
///
/// Nothing distinguishes the two at the record level, so a reader cannot tell
/// a genuine copied history from a flattened one by inspection. Opening a
/// version-3 catalog under this code would take a lineage the document itself
/// now considers wrong and branch from it as though its boundaries were the
/// source's. The version is therefore the boundary: a catalog written before
/// the promise is refused rather than silently reinterpreted. Because a
/// version-3 destination's real provenance was discarded at seed time, no
/// migration can reconstruct it, and none is attempted.
pub const SESSION_CATALOG_SCHEMA_VERSION: u32 = 4;

/// The largest display name a Session may carry.
pub const SESSION_NAME_LIMIT: usize = 120;

/// The largest `/resume` row preview derived from a Session's first user
/// message.
const SESSION_PREVIEW_LIMIT: usize = 120;

/// The largest page a native Session list request may return.
pub const SESSION_LIST_PAGE_LIMIT: usize = 32;

/// The largest page a native Session tree/history request may return.
pub const SESSION_TREE_PAGE_LIMIT: usize = 32;

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

/// Bounded authoritative metadata for one Session.
///
/// The graph is deliberately not embedded here. Callers that need the graph
/// use the bounded tree page seam below, so `/session`, switch results, and
/// restart metadata never materialize every historical node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Session identity.
    pub id: SessionId,
    /// The user-defined display name, when this Session has one.
    ///
    /// A Session is born unnamed. The name is display metadata a user
    /// chooses, never an identity: nothing resolves a Session by it, and an
    /// unnamed Session is a complete, ordinary Session.
    pub name: Option<String>,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last metadata/active-node publication instant.
    pub updated_at: DateTime<Utc>,
    /// The active node selected in this Session.
    pub active_node: SessionNodeId,
    /// The conversation owned by the active node.
    pub active_conversation_id: ConversationId,
    /// Number of persisted nodes, useful metadata for a bounded tree view.
    pub node_count: usize,
}

/// A bounded page of persisted Session summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListPage {
    /// Rows in deterministic Session-id order.
    pub sessions: Vec<SessionSummary>,
    /// Offset for the next page, when more matching rows exist.
    pub next_offset: Option<usize>,
}

/// A bounded page of nodes in one Session graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNodePage {
    /// Nodes in deterministic node-id order.
    pub nodes: Vec<SessionNode>,
    /// Offset for the next page, when more nodes exist.
    pub next_offset: Option<usize>,
}

/// A bounded page of historical branchable user-message boundaries.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUserMessageBoundaryPage {
    /// Boundaries in their first-appearance order.
    pub boundaries: Vec<SessionUserMessageBoundary>,
    /// Offset for the next page, when more boundaries exist.
    pub next_offset: Option<usize>,
}

/// One bounded row in the `/resume` selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identity.
    pub id: SessionId,
    /// The user-defined display name, when this Session has one.
    pub name: Option<String>,
    /// The first user message of this Session's root lineage, bounded to one
    /// line. It is what an unnamed row is recognized by, and it is derived
    /// for the page rather than stored: the catalog keeps no copy of
    /// conversation content.
    pub preview: Option<String>,
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

/// A source lineage selected at one immutable durable Surface revision.
///
/// A lineage copy needs all three of what the source conversation is at the
/// selected cut, because they are not the same set of facts:
///
/// ```text
/// messages         what the model sees there  -- the Surface at that revision
/// canonical        what the conversation is   -- the Ledger, retired facts included
/// surface_history  how it came to see that    -- the retained operations through it
/// ```
///
/// Compaction is the transition that separates them. It retires results from
/// the Surface and leaves them canonical, so conversation-owned state derived
/// from canonical history — the task list is the first, and will not be the
/// last — outlives its own model-visible record. A snapshot carrying only
/// `messages` would make a copy of a compacted conversation mean something
/// different from a copy of the same conversation one moment earlier, which
/// is a lineage semantics that changes behind the user's back.
///
/// `surface_history` is the third part for the reason one step further out.
/// `messages` and `canonical` together fix what the copy *is*; neither
/// records why the Surface looks the way it does, and that is what the
/// copy's own later fork or tree reads. A compaction makes Surface order and
/// Ledger order disagree, so a copy that kept only the final projection
/// would present branch points the source never had — see [`lineage_cut`].
///
/// `canonical` is the source Ledger as read; the *cut* is taken by the
/// `prepare_*` that uses this snapshot, from the boundary it selected, so a
/// fact committed after the selected revision is never inherited.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoricalConversationSnapshot {
    /// Source `ConversationId`.
    pub conversation_id: ConversationId,
    /// The exact selected Surface revision.
    pub surface_revision: SurfaceRevision,
    /// Canonical messages active at that revision, in Surface order.
    pub messages: Vec<MessageBlock>,
    /// The source's durable canonical history, in Ledger commit order. A
    /// superset of `messages`.
    pub canonical: Vec<MessageBlock>,
    /// The source's retained Surface operations through `surface_revision`,
    /// in revision order. Replaying them yields `messages`.
    pub surface_history: Vec<SurfaceOp>,
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
    pub(crate) state: SessionPersistentState,
    pub(crate) database_path: PathBuf,
}

/// The intentionally small durable state owned by one Session.
///
/// Runtime/project configuration is never copied here. The selected model is
/// the one Session-local user choice that survives restart and resume; all
/// other execution settings are supplied by the current runtime composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct SessionPersistentState {
    pub(crate) model: SessionModelConfig,
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
    /// Absent until a user names this Session; see [`SessionSnapshot::name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    active_node: SessionNodeId,
    nodes: BTreeMap<SessionNodeId, SessionNode>,
    /// Only intentionally Session-local choices are persisted here.
    state: SessionPersistentState,
}

/// One complete catalog document a caller intends to commit, held before
/// anything durable has changed.
///
/// Startup is the reason this type exists. A launch decides where it begins
/// — continue the active Session, start an empty one, bind a named one —
/// and then has to compose a runtime for that destination, which is the
/// step that can still fail: a Session whose recorded model no longer
/// exists in `models.jsonc`, a database that will not open, a capability
/// composition that cannot be built. Publishing the decision first and
/// composing afterwards leaves a process that failed to start having
/// silently moved the active selection, so the next launch begins somewhere
/// the user never asked for.
///
/// Holding the decision here inverts that: every fallible step runs against
/// the planned destination, and the catalog changes once, at the end, in a
/// single transaction. A failure before that transaction leaves the catalog
/// byte-for-byte as it was.
#[derive(Debug, Clone)]
pub(crate) struct PlannedCatalog {
    /// The complete document to persist.
    document: CatalogDocument,
    /// Whether the plan differs from the catalog it was planned against.
    /// An unchanged plan commits nothing.
    changed: bool,
}

impl PlannedCatalog {
    /// The Session node this plan makes active, and its Session-local state.
    ///
    /// Read from the planned document, not from the catalog on disk: this is
    /// the destination the caller must compose for.
    pub(crate) fn active_lineage(
        &self,
    ) -> Result<(SessionId, SessionNode, SessionPersistentState), SessionError> {
        active_lineage_of(&self.document)
    }

    /// Names the Session this plan makes active.
    ///
    /// Naming is metadata and can only follow the decision about where the
    /// launch starts, so it applies to the plan rather than to the catalog:
    /// a launch that fails to compose renames nothing.
    pub(crate) fn with_name(mut self, name: &str) -> Result<Self, SessionError> {
        let name = normalize_name(name)?;
        let active = self.document.active_session.clone();
        let session = self
            .document
            .sessions
            .get_mut(&active)
            .ok_or(SessionError::UnknownSession { session_id: active })?;
        session.name = Some(name);
        session.updated_at = Utc::now();
        self.changed = true;
        Ok(self)
    }
}

/// The active lineage of one catalog document.
fn active_lineage_of(
    document: &CatalogDocument,
) -> Result<(SessionId, SessionNode, SessionPersistentState), SessionError> {
    let session = document
        .sessions
        .get(&document.active_session)
        .ok_or_else(|| SessionError::UnknownSession {
            session_id: document.active_session.clone(),
        })?;
    let node =
        session
            .nodes
            .get(&session.active_node)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
            })?;
    Ok((session.id.clone(), node.clone(), session.state.clone()))
}

/// The native durable `SessionCatalog` and graph authority.
#[derive(Debug, Clone)]
pub struct SessionCatalog {
    root: PathBuf,
    path: PathBuf,
    document: CatalogDocument,
    /// Whether `document` has ever been written to `path`.
    ///
    /// A first launch composes against a catalog that exists only in
    /// memory (see [`Self::create_unpublished`]), so an unpublished catalog
    /// has a pending first write even when nothing about the launch changed
    /// it: [`Self::plan_unchanged`] plans that write, and it lands in the
    /// same single startup transaction as every other catalog decision.
    published: bool,
    #[cfg(test)]
    write_fault: Arc<Mutex<Option<CatalogWriteFault>>>,
}

impl SessionCatalog {
    /// Opens the native catalog when this product root already has one.
    ///
    /// This path never creates durable Session state. First-Session
    /// publication is an explicit operation performed only after the
    /// composition layer has validated the initial Session-local model.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the catalog cannot be read or validated.
    pub fn open_existing(runtime_root: &Path) -> Result<Option<Self>, SessionError> {
        let root = runtime_root.join("sessions");
        fs::create_dir_all(&root).map_err(|error| SessionError::Io {
            path: root.clone(),
            detail: error.to_string(),
        })?;
        let path = root.join("catalog.json");
        if !path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(&path).map_err(|error| SessionError::Io {
            path: path.clone(),
            detail: error.to_string(),
        })?;
        let document: CatalogDocument =
            serde_json::from_slice(&bytes).map_err(|error| SessionError::Catalog {
                detail: format!("cannot decode {}: {error}", path.display()),
            })?;
        validate_document(&document)?;
        Ok(Some(Self {
            root,
            path,
            document,
            published: true,
            #[cfg(test)]
            write_fault: Arc::new(Mutex::new(None)),
        }))
    }

    /// Creates and publishes the first root Session with an already-validated
    /// Session-local model.
    ///
    /// Model catalog resolution deliberately does not happen here. The
    /// composition layer owns that authority and must complete it before
    /// calling this mutating Session-domain operation.
    ///
    /// Startup does not take this path: it composes against
    /// [`Self::create_unpublished`] and commits the first catalog write in
    /// the same transaction as every other startup decision. Nothing in
    /// production creates and publishes a first Session in one step, so this
    /// is the test-only shorthand for "a runtime root that has already
    /// launched once".
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the private conversation seed or catalog
    /// publication cannot be completed.
    #[cfg(test)]
    pub(crate) fn create(
        runtime_root: &Path,
        state: &SessionPersistentState,
    ) -> Result<Self, SessionError> {
        let mut catalog = Self::create_unpublished(runtime_root, state)?;
        let planned = catalog.plan_unchanged();
        catalog.commit_planned(planned)?;
        Ok(catalog)
    }

    /// Builds the first root Session **without** publishing it.
    ///
    /// The returned catalog names a root Session that `catalog.json` does
    /// not yet mention. That is deliberate: a first launch still has to
    /// compose a runtime for that Session — workspace, capabilities,
    /// recovery, host binding — and every one of those steps can fail. A
    /// catalog written before them leaves a visible, resumable Session
    /// behind a launch that never started, and the next launch resumes into
    /// it.
    ///
    /// The seeded conversation database is not a published fact: a
    /// conversation the catalog does not name is neither selectable nor
    /// resumable, so an abandoned first launch leaves an inert file and
    /// nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the private conversation seed cannot be
    /// created.
    pub(crate) fn create_unpublished(
        runtime_root: &Path,
        state: &SessionPersistentState,
    ) -> Result<Self, SessionError> {
        let root = runtime_root.join("sessions");
        fs::create_dir_all(&root).map_err(|error| SessionError::Io {
            path: root.clone(),
            detail: error.to_string(),
        })?;
        let path = root.join("catalog.json");
        if path.exists() {
            return Err(SessionError::Catalog {
                detail: format!(
                    "cannot create first Session: {} already exists",
                    path.display()
                ),
            });
        }

        let session_id = SessionId::new("session-1");
        let node_id = SessionNodeId::new("node-1");
        let conversation_id = ConversationId::new("conversation-1");
        let database_path = conversation_database_path(&root, &session_id, &conversation_id);
        initialize_database(
            &database_path,
            &conversation_id,
            &LineageSeed::history(Vec::new()),
        )?;
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
                name: None,
                created_at: now,
                updated_at: now,
                active_node: node_id,
                nodes,
                state: state.clone(),
            },
        );
        let document = CatalogDocument {
            schema_version: SESSION_CATALOG_SCHEMA_VERSION,
            active_session: session_id,
            next_session_ordinal: 2,
            next_node_ordinal: 2,
            sessions,
        };
        Ok(Self {
            root,
            path,
            document,
            published: false,
            #[cfg(test)]
            write_fault: Arc::new(Mutex::new(None)),
        })
    }

    /// Arms one deterministic catalog-write fault for the next mutation.
    #[cfg(test)]
    pub(crate) fn arm_write_fault_before_rename(&self) {
        *self
            .write_fault
            .lock()
            .expect("catalog write fault lock poisoned") = Some(CatalogWriteFault::BeforeRename);
    }

    /// Arms one deterministic post-rename durability fault for the next
    /// mutation.
    #[cfg(test)]
    pub(crate) fn arm_write_fault_after_rename(&self) {
        *self
            .write_fault
            .lock()
            .expect("catalog write fault lock poisoned") = Some(CatalogWriteFault::AfterRename);
    }

    /// Returns one bounded, searchable Session-list page.
    ///
    /// Ordering is ascending Session identity. The offset is a domain-specific
    /// continuation: there is no global maximum number of Sessions, and
    /// callers can reach older matching rows by requesting the returned
    /// offset. Only the requested page is materialized for the projection.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the requested page size is outside the
    /// native bound.
    pub fn list_page(
        &self,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<SessionListPage, SessionError> {
        validate_page_limit(limit, SESSION_LIST_PAGE_LIMIT)?;
        let query = query.map(|value| value.trim().to_lowercase());
        let mut matching = 0_usize;
        let mut page = Vec::with_capacity(limit);
        let mut has_more = false;
        for session in self.document.sessions.values() {
            // A row is searched by what it shows. An unnamed Session shows
            // its first user message, so matching only identity and name
            // would hide exactly the rows a user has to recognize by their
            // content.
            let preview = self.preview(session)?;
            let matches = query.as_ref().is_none_or(|query| {
                session.id.as_str().to_lowercase().contains(query)
                    || session
                        .name
                        .as_ref()
                        .is_some_and(|name| name.to_lowercase().contains(query))
                    || preview
                        .as_ref()
                        .is_some_and(|preview| preview.to_lowercase().contains(query))
            });
            if !matches {
                continue;
            }
            if matching < offset {
                matching += 1;
                continue;
            }
            if page.len() == limit {
                has_more = true;
                break;
            }
            page.push(SessionSummary {
                id: session.id.clone(),
                name: session.name.clone(),
                preview,
                updated_at: session.updated_at,
                active_node: session.active_node.clone(),
                active: session.id == self.document.active_session,
            });
            matching += 1;
        }
        let next_offset = has_more.then_some(offset + page.len());
        Ok(SessionListPage {
            sessions: page,
            next_offset,
        })
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

    /// Whether the active Session has never been used.
    ///
    /// An unused Session is exactly the Session a launch would otherwise
    /// have to publish: one `New` root node whose durable conversation is
    /// still at its initial Surface revision, with no canonical message and
    /// nothing accepted into its Pending Inbound. Startup reuses that Session
    /// instead of publishing another empty one, so repeated launches cannot
    /// accumulate empty rows in `/resume`.
    ///
    /// Pending Inbound is part of the question, not a detail: a message that
    /// was accepted durably but never adopted is work this Session owns, and
    /// composing that lineage again is what adopts it. Treating it as unused
    /// would resurrect a previous launch's prompt inside what the user asked
    /// to be an empty Session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the active identity is not persisted or
    /// its durable conversation cannot be read.
    pub(crate) fn active_is_unused(&self) -> Result<bool, SessionError> {
        let session = self
            .document
            .sessions
            .get(&self.document.active_session)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: self.document.active_session.clone(),
            })?;
        if session.nodes.len() != 1 {
            return Ok(false);
        }
        let Some(node) = session.nodes.values().next() else {
            return Ok(false);
        };
        if node.parent.is_some() || node.origin != SessionNodeOrigin::New {
            return Ok(false);
        }
        let path = self.database_path(&session.id, &node.conversation_id);
        let store = SqliteConversationStore::open(node.conversation_id.clone(), &path)
            .map_err(SessionError::Store)?;
        let head = store.load_head().map_err(SessionError::Store)?;
        if head.revision != SurfaceRevision::INITIAL || !head.active_message_ids.is_empty() {
            return Ok(false);
        }
        let pending = store.load_pending().map_err(SessionError::Store)?;
        Ok(pending.is_empty())
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
        snapshot_of(session)
    }

    /// Returns one bounded page of graph nodes.
    pub(crate) fn node_page(
        &self,
        session_id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<SessionNodePage, SessionError> {
        validate_page_limit(limit, SESSION_TREE_PAGE_LIMIT)?;
        let session =
            self.document
                .sessions
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let nodes = session
            .nodes
            .values()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_offset =
            (offset + nodes.len() < session.nodes.len()).then_some(offset + nodes.len());
        Ok(SessionNodePage { nodes, next_offset })
    }

    /// Returns the active Session node and its intentionally Session-local
    /// state. Current runtime configuration is not part of this durable view.
    pub(crate) fn active_lineage(
        &self,
    ) -> Result<(SessionId, SessionNode, SessionPersistentState), SessionError> {
        active_lineage_of(&self.document)
    }

    /// Returns one named lineage and its Session-local state.
    pub(crate) fn lineage(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<(SessionNode, SessionPersistentState), SessionError> {
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
        Ok((node.clone(), session.state.clone()))
    }

    /// Whether any Session or node in this catalog names `conversation`.
    ///
    /// The catalog is the sole reachability authority for a lineage: a
    /// destination database that `prepare_*` seeded but whose publication
    /// never committed exists on disk and is named by nothing, so it is
    /// neither selectable nor resumable.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "conformance reachability probe")
    )]
    pub(crate) fn names_conversation(&self, conversation: &ConversationId) -> bool {
        self.document
            .sessions
            .values()
            .flat_map(|session| session.nodes.values())
            .any(|node| node.conversation_id == *conversation)
    }

    /// Returns the private database path for a known node.
    pub(crate) fn database_path(
        &self,
        session_id: &SessionId,
        conversation_id: &ConversationId,
    ) -> PathBuf {
        conversation_database_path(&self.root, session_id, conversation_id)
    }

    /// The bounded single-line label an unnamed Session is recognized by:
    /// the first ordinary user message of its root lineage.
    ///
    /// It is derived per page and never stored. The catalog is product
    /// metadata, so caching conversation text in it would create a second
    /// copy of history that could disagree with the conversation itself; a
    /// Session's own durable store is the only authority for what was said
    /// in it. Reading one bounded boundary page per row keeps that honest at
    /// the page limit's cost.
    ///
    /// The root node is the subject on purpose. Branch nodes are seeded
    /// copies of a source lineage, so the row would otherwise change what it
    /// says whenever the active node moves, while the Session it names is
    /// still the same Session that started with the same message.
    fn preview(&self, session: &PersistedSession) -> Result<Option<String>, SessionError> {
        let root = session
            .nodes
            .values()
            .find(|node| node.parent.is_none())
            .ok_or_else(|| SessionError::Catalog {
                detail: format!("Session {} has no root node", session.id),
            })?;
        let path = self.database_path(&session.id, &root.conversation_id);
        let store = SqliteConversationStore::open(root.conversation_id.clone(), &path)
            .map_err(SessionError::Store)?;
        let head = store.load_head().map_err(SessionError::Store)?;
        let page = store
            .load_user_message_boundaries_page(head.revision, 0, 1)
            .map_err(SessionError::Store)?;
        Ok(page
            .boundaries
            .first()
            .and_then(|boundary| preview_of(&boundary.message)))
    }

    /// Atomically names a Session. This touches metadata only.
    ///
    /// Naming never moves, rewrites, or re-identifies anything: the Session
    /// keeps its identity, its graph, and its conversations, and gains only
    /// the label `/resume` shows in place of its first message.
    pub(crate) fn rename(
        &mut self,
        session_id: &SessionId,
        name: &str,
    ) -> Result<SessionSnapshot, SessionError> {
        let name = Some(normalize_name(name)?);
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
        session.state.model = model;
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
        let next = self.build_select_document(session_id, node_id)?;
        self.commit(next)?;
        self.snapshot(session_id)
    }

    /// Validates the catalog publication for a selected Session/node before
    /// the current runtime is quiesced.
    pub(crate) fn preflight_select(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<(), SessionError> {
        validate_document(&self.build_select_document(session_id, node_id)?)
    }

    /// Prepares a new independent Session with a fresh `ConversationId`.
    pub(crate) fn prepare_session(
        &self,
        template: &SessionPersistentState,
        seed: &[MessageBlock],
    ) -> Result<PreparedLineage, SessionError> {
        let (session_id, node_id, conversation_id) = self.allocate_ids();
        self.prepare_session_with_ids(
            template,
            session_id,
            node_id,
            conversation_id,
            &LineageSeed::history(seed.to_vec()),
        )
    }

    /// Prepares a clone from the exact source revision selected by the
    /// caller. The source revision and source message bodies are immutable
    /// inputs to this preparation.
    ///
    /// The clone inherits the source's complete semantic state at that cut,
    /// not only what the Surface still shows there: see
    /// [`HistoricalConversationSnapshot`] and [`lineage_cut`].
    pub(crate) fn prepare_clone_session(
        &self,
        template: &SessionPersistentState,
        source: &HistoricalConversationSnapshot,
    ) -> Result<PreparedLineage, SessionError> {
        let (session_id, node_id, conversation_id) = self.allocate_ids();
        let seed = lineage_cut(&conversation_id, source, None)?;
        self.prepare_session_with_ids(template, session_id, node_id, conversation_id, &seed)
    }

    /// Prepares an independent Session fork and returns the selected original
    /// user prompt for uncommitted editor restoration.
    pub(crate) fn prepare_fork_session(
        &self,
        template: &SessionPersistentState,
        source: &HistoricalConversationSnapshot,
        message_id: &MessageId,
    ) -> Result<
        (
            PreparedLineage,
            Vec<crate::message::types::UserContentBlock>,
        ),
        SessionError,
    > {
        let user = active_user_boundary(source, message_id)?;
        let editor_content = text_only_editor_content(user)?;
        let (session_id, node_id, conversation_id) = self.allocate_ids();
        let seed = lineage_cut(&conversation_id, source, Some(message_id))?;
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
        template: &SessionPersistentState,
        source: &HistoricalConversationSnapshot,
        message_id: &MessageId,
    ) -> Result<
        (
            PreparedLineage,
            Vec<crate::message::types::UserContentBlock>,
        ),
        SessionError,
    > {
        let user = active_user_boundary(source, message_id)?;
        let editor_content = text_only_editor_content(user)?;
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
            let conversation_taken = self
                .document
                .sessions
                .values()
                .flat_map(|session| session.nodes.values())
                .any(|node| node.conversation_id == conversation_id);
            if !node_id_taken && !conversation_taken && !database_path.exists() {
                break (node_id, conversation_id, database_path);
            }
            node_ordinal = node_ordinal.saturating_add(1);
        };
        let seed = lineage_cut(&conversation_id, source, Some(message_id))?;
        initialize_database(&database_path, &conversation_id, &seed)?;
        Ok((
            PreparedLineage {
                session_id: session_id.clone(),
                node_id,
                conversation_id,
                state: template.clone(),
                database_path,
            },
            editor_content,
        ))
    }

    fn prepare_session_with_ids(
        &self,
        template: &SessionPersistentState,
        session_id: SessionId,
        node_id: SessionNodeId,
        conversation_id: ConversationId,
        seed: &LineageSeed,
    ) -> Result<PreparedLineage, SessionError> {
        let database_path = conversation_database_path(&self.root, &session_id, &conversation_id);
        initialize_database(&database_path, &conversation_id, seed)?;
        Ok(PreparedLineage {
            session_id,
            node_id,
            conversation_id,
            state: template.clone(),
            database_path,
        })
    }

    /// Publishes a prepared independent Session and makes it active.
    ///
    /// The `publish_session` process-death boundaries bracket the **catalog
    /// visibility commit** — the atomic rename inside [`Self::commit`] — and
    /// nothing else. The destination database was already seeded by
    /// `prepare_*`, so the two sides are exactly the two durable worlds a
    /// crash can leave: a seeded destination the catalog does not name (the
    /// source lineage is still active and the seed is an inert orphan), or a
    /// catalog that names the complete new lineage. There is no third state.
    pub(crate) fn publish_session(
        &mut self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<SessionSnapshot, SessionError> {
        let session_id = prepared.session_id.clone();
        let next = self.build_session_document(prepared, origin)?;
        crate::runtime::process_death::reach("before:publish_session");
        self.commit(next)?;
        crate::runtime::process_death::reach("after:publish_session");
        self.snapshot(&session_id)
    }

    /// Validates a prepared Session publication before runtime quiescence.
    pub(crate) fn preflight_publish_session(
        &self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<(), SessionError> {
        validate_document(&self.build_session_document(prepared, origin)?)
    }

    /// Publishes a prepared branch node inside an existing Session and makes
    /// it the active node.
    ///
    /// The `publish_node` boundaries bracket the same visibility commit for a
    /// branch node. They are separate from the `publish_session` ones on
    /// purpose: a node publication is a different catalog transaction, with a
    /// different parent linkage and a different active-selection rule, so
    /// proving one atomic says nothing about the other.
    pub(crate) fn publish_node(
        &mut self,
        session_id: &SessionId,
        prepared: &PreparedLineage,
        parent: SessionNodeId,
        origin: SessionNodeOrigin,
    ) -> Result<SessionSnapshot, SessionError> {
        let next = self.build_node_document(session_id, prepared, parent, origin)?;
        crate::runtime::process_death::reach("before:publish_node");
        self.commit(next)?;
        crate::runtime::process_death::reach("after:publish_node");
        self.snapshot(session_id)
    }

    /// Validates a prepared branch-node publication before runtime quiescence.
    pub(crate) fn preflight_publish_node(
        &self,
        session_id: &SessionId,
        prepared: &PreparedLineage,
        parent: SessionNodeId,
        origin: SessionNodeOrigin,
    ) -> Result<(), SessionError> {
        validate_document(&self.build_node_document(session_id, prepared, parent, origin)?)
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

    /// A planned catalog transition that changes nothing.
    ///
    /// An unpublished catalog — a first launch, see
    /// [`Self::create_unpublished`] — still has a pending first write, so
    /// its "unchanged" plan commits the document it was built with. Every
    /// other catalog commits nothing.
    #[must_use]
    pub(crate) fn plan_unchanged(&self) -> PlannedCatalog {
        PlannedCatalog {
            document: self.document.clone(),
            changed: !self.published,
        }
    }

    /// Plans the active selection of an existing Session/node without
    /// publishing it.
    ///
    /// This is [`Self::select`] with the commit removed: the same
    /// validation, the same resulting document, no durable write.
    pub(crate) fn plan_select(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<PlannedCatalog, SessionError> {
        Ok(PlannedCatalog {
            document: self.build_select_document(session_id, node_id)?,
            changed: true,
        })
    }

    /// Plans the publication of a prepared independent Session without
    /// publishing it.
    pub(crate) fn plan_session(
        &self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<PlannedCatalog, SessionError> {
        Ok(PlannedCatalog {
            document: self.build_session_document(prepared, origin)?,
            changed: true,
        })
    }

    /// Commits one planned transition as a single catalog transaction.
    ///
    /// A plan that changes nothing writes nothing: an unchanged document is
    /// not rewritten just because a launch looked at it.
    pub(crate) fn commit_planned(&mut self, planned: PlannedCatalog) -> Result<(), SessionError> {
        if !planned.changed {
            return Ok(());
        }
        self.commit(planned.document)
    }

    fn build_select_document(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<CatalogDocument, SessionError> {
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
        if !session.nodes.contains_key(&selected_node) {
            return Err(SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: selected_node,
            });
        }
        session.active_node = selected_node;
        session.updated_at = Utc::now();
        next.active_session = session_id.clone();
        Ok(next)
    }

    fn build_session_document(
        &self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<CatalogDocument, SessionError> {
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
        let now = Utc::now();
        let node = SessionNode {
            id: prepared.node_id.clone(),
            parent: None,
            conversation_id: prepared.conversation_id.clone(),
            origin,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(prepared.node_id.clone(), node);
        let mut next = self.document.clone();
        next.sessions.insert(
            prepared.session_id.clone(),
            PersistedSession {
                id: prepared.session_id.clone(),
                // Every Session is published unnamed. A generated label —
                // "New session", "Fork of session-3" — is not a name a user
                // chose; it only displaces the first message, which is the
                // one thing that says what the Session is about.
                name: None,
                created_at: now,
                updated_at: now,
                active_node: prepared.node_id.clone(),
                nodes,
                state: prepared.state.clone(),
            },
        );
        next.active_session = prepared.session_id.clone();
        next.next_session_ordinal = next.next_session_ordinal.saturating_add(1);
        next.next_node_ordinal = next.next_node_ordinal.saturating_add(1);
        Ok(next)
    }

    fn build_node_document(
        &self,
        session_id: &SessionId,
        prepared: &PreparedLineage,
        parent: SessionNodeId,
        origin: SessionNodeOrigin,
    ) -> Result<CatalogDocument, SessionError> {
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
        let node = SessionNode {
            id: prepared.node_id.clone(),
            parent: Some(parent),
            conversation_id: prepared.conversation_id.clone(),
            origin,
        };
        session.nodes.insert(prepared.node_id.clone(), node);
        session.active_node = prepared.node_id.clone();
        session.updated_at = Utc::now();
        next.active_session = session_id.clone();
        next.next_node_ordinal = next.next_node_ordinal.saturating_add(1);
        Ok(next)
    }

    fn commit(&mut self, next: CatalogDocument) -> Result<(), SessionError> {
        validate_document(&next)?;
        match self.persist(&next) {
            Ok(()) => {
                self.document = next;
                self.published = true;
                Ok(())
            }
            Err(
                error @ SessionError::CatalogCommit {
                    error: CatalogCommitError::CommittedButDurabilityUncertain { .. },
                },
            ) => {
                // `rename` has already made `next` the visible catalog
                // document. Keep the in-process authority aligned even
                // though the directory durability barrier could not be
                // proven, then surface the distinct post-commit outcome.
                self.document = next;
                self.published = true;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn persist(&self, document: &CatalogDocument) -> Result<(), SessionError> {
        let bytes =
            serde_json::to_vec_pretty(document).map_err(|error| SessionError::CatalogCommit {
                error: CatalogCommitError::NotCommitted {
                    path: self.path.clone(),
                    detail: format!("cannot encode catalog: {error}"),
                },
            })?;
        #[cfg(test)]
        let result = atomic_write(&self.path, &bytes, &self.write_fault);
        #[cfg(not(test))]
        let result = atomic_write(&self.path, &bytes);
        result.map_err(SessionError::from)
    }
}

/// The selected fork/tree boundary, resolved against what the source's
/// Surface actually shows at the selected revision.
///
/// A boundary is a *model-visible* ordinary user message. A message the
/// Surface no longer shows there — one a compaction already retired — is not
/// a branch point the user could have chosen, and a message the Ledger
/// carries but the selected revision predates is not one either.
fn active_user_boundary<'a>(
    source: &'a HistoricalConversationSnapshot,
    message_id: &MessageId,
) -> Result<&'a UserMessageBlock, SessionError> {
    source
        .messages
        .iter()
        .find_map(|message| match message {
            MessageBlock::User(user)
                if user.id == *message_id && user.kind == InboundKind::Message =>
            {
                Some(user)
            }
            _ => None,
        })
        .ok_or_else(|| SessionError::UnknownBoundary {
            message_id: message_id.clone(),
        })
}

/// The native editor restoration contract is currently text-only. Rejecting
/// other canonical user blocks here keeps a fork/tree transition from
/// pretending that an image or file reference is equivalent to a placeholder
/// string. The typed payload remains `Vec<UserContentBlock>` so a structured
/// editor can extend this contract without changing Session lineage rules.
fn text_only_editor_content(
    user: &UserMessageBlock,
) -> Result<Vec<UserContentBlock>, SessionError> {
    if user
        .content
        .iter()
        .any(|content| !matches!(content, UserContentBlock::Text(_)))
    {
        return Err(SessionError::Seed {
            detail: "fork/tree editor restoration currently supports text user content only"
                .to_owned(),
        });
    }
    Ok(user.content.clone())
}

/// Cuts one source lineage at the boundary a `prepare_*` selected, and
/// reconstructs it as a destination-owned [`LineageSeed`].
///
/// The cut is the whole point, and it is taken over the source's *Surface
/// operation history*, not over its final Surface projection. `boundary` is
/// the user message the destination stops before — `None` for a clone, which
/// stops before nothing.
///
/// One forward replay of the source history decides two things at once:
///
/// - which operations the destination inherits. An operation is dropped when
///   it introduces an excluded identity: an `Append` of a message committed
///   at or after the boundary, or a `Replace` whose span holds anything
///   already excluded — a summary of work at or after the boundary is itself
///   work at or after the boundary. A `Replace` whose span is entirely below
///   the boundary is inherited, so a compaction the source already performed
///   over the copied prefix stays performed;
/// - which canonical rows the destination inherits: exactly those the
///   retained operations name. That is the closure the Surface needs — a
///   retained summary drags along the facts it retired — and it is why a
///   compaction that retired a `todo` result carries the result into the
///   destination while leaving it off the destination's Surface.
///
/// Cutting this way is what makes copying *closed* under the operations that
/// follow it. The destination's retained operations are the source's, so the
/// destination's own historical boundaries are the source's boundaries, and
/// a fork of a copy at a copied boundary means what a fork of the source at
/// that boundary means. Rebuilding the destination from the final projection
/// instead — one append per active message — yields the same Surface and a
/// history that never happened, in which a compaction summary appears to
/// predate a user message it actually postdates; forking the copy there then
/// silently carries that user message into the canonical prefix it was
/// supposed to cut before.
///
/// A boundary that excludes everything (a fork at the very first user
/// message) retains no operation and cuts to the empty lineage, which is
/// exactly the fresh conversation such a fork means.
fn lineage_cut(
    destination: &ConversationId,
    source: &HistoricalConversationSnapshot,
    boundary: Option<&MessageId>,
) -> Result<LineageSeed, SessionError> {
    let position = |id: &MessageId| {
        source
            .canonical
            .iter()
            .position(|message| message_id_of(message) == *id)
            .ok_or_else(|| SessionError::Seed {
                detail: format!(
                    "the source Surface history names {id}, which its Ledger does not carry"
                ),
            })
    };
    // A clone stops before nothing, so nothing is committed at or after its
    // boundary.
    let cut_at = match boundary {
        Some(id) => position(id)?,
        None => source.canonical.len(),
    };

    let mut active: Vec<MessageId> = Vec::new();
    let mut excluded: BTreeSet<MessageId> = BTreeSet::new();
    let mut retained: Vec<SurfaceOp> = Vec::new();
    for operation in &source.surface_history {
        let keep = match operation {
            SurfaceOp::Append { message_id } => {
                let below = position(message_id)? < cut_at;
                if !below {
                    excluded.insert(message_id.clone());
                }
                below
            }
            SurfaceOp::Replace {
                start,
                end,
                replacement,
            } => {
                position(replacement)?;
                let span = replaced_span(&active, start, end)?;
                let below = !span.iter().any(|id| excluded.contains(id));
                if !below {
                    excluded.insert(replacement.clone());
                }
                below
            }
        };
        if keep {
            retained.push(operation.clone());
        }
        // The source's own order is tracked whole, retained or not: a later
        // `Replace` names its span in the source's coordinates.
        apply_surface_op(&mut active, operation).map_err(|detail| SessionError::Seed { detail })?;
    }

    // The destination Ledger is exactly what its Surface history names, in
    // source commit order.
    let referenced: BTreeSet<MessageId> = retained
        .iter()
        .flat_map(|operation| operation.message_ids().into_iter().cloned())
        .collect();
    let canonical: Vec<MessageBlock> = source
        .canonical
        .iter()
        .filter(|message| referenced.contains(&message_id_of(message)))
        .cloned()
        .collect();
    remap_seed(destination, &canonical, &retained)
}

/// The identities a `Replace` retires, in the active order it retires them
/// from.
fn replaced_span(
    active: &[MessageId],
    start: &MessageId,
    end: &MessageId,
) -> Result<Vec<MessageId>, SessionError> {
    let from = active
        .iter()
        .position(|id| id == start)
        .ok_or_else(|| SessionError::Seed {
            detail: format!("the source Surface Replace start {start} is not active"),
        })?;
    let to = active
        .iter()
        .position(|id| id == end)
        .ok_or_else(|| SessionError::Seed {
            detail: format!("the source Surface Replace end {end} is not active"),
        })?;
    if to < from {
        return Err(SessionError::Seed {
            detail: format!("the source Surface Replace span {start}..={end} is reversed"),
        });
    }
    Ok(active[from..=to].to_vec())
}

/// Reconstructs a destination seed with destination-owned message and tool
/// identities. Runtime lifecycle identities are not present in this input and
/// therefore cannot leak into the destination.
///
/// `canonical` is the destination's whole Ledger cut and `surface_history` is
/// the operation log that projects it, so the one identity map remaps both:
/// a `Replace` and the retired messages it names receive destination
/// identities that still agree.
pub(crate) fn remap_seed(
    destination: &ConversationId,
    canonical: &[MessageBlock],
    surface_history: &[SurfaceOp],
) -> Result<LineageSeed, SessionError> {
    let messages = canonical;
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

    let canonical = messages
        .iter()
        .map(|message| remap_message(message, &message_ids, &call_ids))
        .collect::<Result<Vec<_>, SessionError>>()?;
    let seeded = |id: &MessageId| {
        message_ids
            .get(id)
            .cloned()
            .ok_or_else(|| SessionError::Seed {
                detail: format!(
                    "the seeded Surface history names {id}, which the seeded Ledger does not carry"
                ),
            })
    };
    let surface_history = surface_history
        .iter()
        .map(|operation| match operation {
            SurfaceOp::Append { message_id } => Ok(SurfaceOp::Append {
                message_id: seeded(message_id)?,
            }),
            SurfaceOp::Replace {
                start,
                end,
                replacement,
            } => Ok(SurfaceOp::Replace {
                start: seeded(start)?,
                end: seeded(end)?,
                replacement: seeded(replacement)?,
            }),
        })
        .collect::<Result<Vec<_>, SessionError>>()?;
    LineageSeed::replayed(canonical, surface_history).map_err(|error| SessionError::Seed {
        detail: error.to_string(),
    })
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

fn snapshot_of(session: &PersistedSession) -> Result<SessionSnapshot, SessionError> {
    let active_node =
        session
            .nodes
            .get(&session.active_node)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
            })?;
    Ok(SessionSnapshot {
        id: session.id.clone(),
        name: session.name.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        active_node: session.active_node.clone(),
        active_conversation_id: active_node.conversation_id.clone(),
        node_count: session.nodes.len(),
    })
}

fn validate_document(document: &CatalogDocument) -> Result<(), SessionError> {
    // An unexpected version is refused, never interpreted. See
    // `SESSION_CATALOG_SCHEMA_VERSION`: the fields of an older catalog decode
    // cleanly and mean something this code does not promise.
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
        validate_active_node(session_id, session)?;
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

fn validate_active_node(
    session_id: &SessionId,
    session: &PersistedSession,
) -> Result<(), SessionError> {
    if !session.nodes.contains_key(&session.active_node) {
        return Err(SessionError::Catalog {
            detail: format!(
                "session {session_id} selects missing node {}",
                session.active_node
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

/// Normalizes one user-supplied Session name into the single bounded line a
/// selector row can show. A name that survives this is exactly what the row
/// displays, so no renderer has to defend itself against a name.
fn normalize_name(name: &str) -> Result<String, SessionError> {
    let name = single_line(name);
    if name.is_empty() || name.chars().count() > SESSION_NAME_LIMIT {
        return Err(SessionError::InvalidName);
    }
    Ok(name)
}

/// Renders one canonical user message as the bounded single line a `/resume`
/// row shows for an unnamed Session. Non-text content contributes nothing:
/// a Session opened with only an image has no first line to show, and saying
/// so with `None` is more honest than inventing one.
fn preview_of(message: &UserMessageBlock) -> Option<String> {
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            UserContentBlock::Text(block) => Some(block.text.as_str()),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let text = single_line(text.as_str());
    if text.is_empty() {
        return None;
    }
    Some(truncate(text, SESSION_PREVIEW_LIMIT))
}

/// Collapses every run of whitespace — line breaks included — into one space
/// and trims the ends.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bounds one already single-line label to `limit` characters, marking the
/// cut so a truncated line never reads as the whole message.
fn truncate(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    let kept = text
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    format!("{}\u{2026}", kept.trim_end())
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
    seed: &LineageSeed,
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
    store.initialize_lineage(seed).map_err(SessionError::Store)
}

fn atomic_write(
    path: &Path,
    bytes: &[u8],
    #[cfg(test)] write_fault: &Arc<Mutex<Option<CatalogWriteFault>>>,
) -> Result<(), CatalogCommitError> {
    let parent = path
        .parent()
        .ok_or_else(|| CatalogCommitError::NotCommitted {
            path: path.to_path_buf(),
            detail: "catalog has no parent".to_owned(),
        })?;
    fs::create_dir_all(parent).map_err(|error| CatalogCommitError::NotCommitted {
        path: parent.to_path_buf(),
        detail: error.to_string(),
    })?;
    let temporary = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| CatalogCommitError::NotCommitted {
            path: temporary.clone(),
            detail: error.to_string(),
        })?;
    file.write_all(bytes)
        .map_err(|error| CatalogCommitError::NotCommitted {
            path: temporary.clone(),
            detail: error.to_string(),
        })?;
    file.sync_all()
        .map_err(|error| CatalogCommitError::NotCommitted {
            path: temporary.clone(),
            detail: error.to_string(),
        })?;
    #[cfg(test)]
    let write_fault = take_write_fault(write_fault);
    #[cfg(test)]
    if write_fault == Some(CatalogWriteFault::BeforeRename) {
        return Err(CatalogCommitError::NotCommitted {
            path: temporary,
            detail: "deterministic fault before catalog visibility rename".to_owned(),
        });
    }
    fs::rename(&temporary, path).map_err(|error| CatalogCommitError::NotCommitted {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    // The rename above is the visibility commit point. Every error after it
    // is therefore a different outcome from a pre-commit write failure.
    #[cfg(test)]
    if write_fault == Some(CatalogWriteFault::AfterRename) {
        return Err(CatalogCommitError::CommittedButDurabilityUncertain {
            path: path.to_path_buf(),
            detail: "deterministic fault after catalog visibility rename".to_owned(),
        });
    }
    let directory = File::open(parent).map_err(|error| {
        CatalogCommitError::CommittedButDurabilityUncertain {
            path: parent.to_path_buf(),
            detail: error.to_string(),
        }
    })?;
    directory.sync_all().map_err(
        |error| CatalogCommitError::CommittedButDurabilityUncertain {
            path: parent.to_path_buf(),
            detail: error.to_string(),
        },
    )?;
    Ok(())
}

fn validate_page_limit(limit: usize, maximum: usize) -> Result<(), SessionError> {
    if limit == 0 || limit > maximum {
        return Err(SessionError::Catalog {
            detail: format!("page limit must be between 1 and {maximum}"),
        });
    }
    Ok(())
}

/// The result classification of the catalog visibility/durability protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogCommitError {
    /// The visibility rename did not complete. The previous catalog remains
    /// authoritative and the in-memory document is unchanged.
    NotCommitted { path: PathBuf, detail: String },
    /// The visibility rename completed, but the parent-directory durability
    /// barrier did not. The new catalog is visible; durability is uncertain.
    CommittedButDurabilityUncertain { path: PathBuf, detail: String },
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogWriteFault {
    BeforeRename,
    AfterRename,
}

#[cfg(test)]
fn take_write_fault(fault: &Arc<Mutex<Option<CatalogWriteFault>>>) -> Option<CatalogWriteFault> {
    fault
        .lock()
        .expect("catalog write fault lock poisoned")
        .take()
}

/// A native Session-domain failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// A storage operation failed.
    Io { path: PathBuf, detail: String },
    /// The catalog publication outcome is explicit: either visibility was not
    /// crossed, or it was crossed but the final durability barrier was not
    /// proven.
    CatalogCommit { error: CatalogCommitError },
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
            Self::CatalogCommit { error } => match error {
                CatalogCommitError::NotCommitted { path, detail } => write!(
                    f,
                    "session catalog publication did not commit at {}: {detail}",
                    path.display()
                ),
                CatalogCommitError::CommittedButDurabilityUncertain { path, detail } => write!(
                    f,
                    "session catalog visibility committed at {}, but durability is uncertain: {detail}",
                    path.display()
                ),
            },
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
                write!(
                    f,
                    "session name must be 1-{SESSION_NAME_LIMIT} characters after \
                     whitespace is collapsed"
                )
            }
        }
    }
}

impl std::error::Error for SessionError {}

impl From<CatalogCommitError> for SessionError {
    fn from(error: CatalogCommitError) -> Self {
        Self::CatalogCommit { error }
    }
}

impl SessionError {
    /// Whether this error crossed the catalog visibility commit point.
    #[must_use]
    pub fn committed(&self) -> bool {
        matches!(
            self,
            Self::CatalogCommit {
                error: CatalogCommitError::CommittedButDurabilityUncertain { .. }
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::{
        CatalogCommitError, HistoricalConversationSnapshot, PreparedLineage, SessionCatalog,
        SessionError, SessionId, SessionNodeOrigin, SessionPersistentState,
    };
    use crate::conversation::{SurfaceRevision, SurfaceSpan};
    use crate::durable::{CompactionCommitInput, ConversationStore, SqliteConversationStore};
    use crate::local_runtime::CurrentRuntimeConfig;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContextKind, InboundKind, MessageBlock,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};
    use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    const CONFIG: &str = r#"{
        "agentId": "agent-a",
        "model": {"model": "provider/model"},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
    }"#;

    fn config() -> CurrentRuntimeConfig {
        CurrentRuntimeConfig::from_jsonc_slice(CONFIG.as_bytes()).expect("valid test config")
    }

    fn state() -> SessionPersistentState {
        SessionPersistentState {
            model: config().model,
        }
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

    fn status(id: &str, value: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![text(value)],
            source: UserSource::Runtime,
            kind: InboundKind::Context(ContextKind::AgentStatus),
            timestamp: None,
        })
    }

    fn source_history() -> Vec<MessageBlock> {
        vec![
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

    fn open_catalog() -> (TempDir, SessionCatalog, CurrentRuntimeConfig) {
        let directory = tempfile::tempdir().expect("temp directory");
        let config = config();
        let catalog = SessionCatalog::create(directory.path(), &state()).expect("catalog");
        (directory, catalog, config)
    }

    fn reopen_catalog(root: &std::path::Path) -> SessionCatalog {
        SessionCatalog::open_existing(root)
            .expect("open catalog")
            .expect("catalog exists")
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

    /// The source lineage as the supervisor reads it: the Surface at the
    /// selected revision *and* the canonical history it was projected from.
    fn lineage_at(
        store: &SqliteConversationStore,
        conversation_id: &ConversationId,
        revision: SurfaceRevision,
    ) -> HistoricalConversationSnapshot {
        HistoricalConversationSnapshot {
            conversation_id: conversation_id.clone(),
            surface_revision: revision,
            messages: store
                .load_surface_snapshot(revision)
                .expect("historical Surface"),
            canonical: store.load_canonical().expect("source canonical history"),
            surface_history: store
                .load_surface_history(revision)
                .expect("source Surface operation history"),
        }
    }

    /// The "unused Session" predicate startup reuses is exactly one `New`
    /// root node whose conversation has no canonical history. Any committed
    /// message disqualifies the Session, and so does a branch node — even a
    /// branch whose own conversation seed is empty.
    #[test]
    fn only_an_untouched_root_session_counts_as_unused() {
        let (_directory, mut catalog, _config) = open_catalog();
        assert!(catalog.active_is_unused().expect("fresh catalog"));

        // A Session-local model choice is metadata, not use.
        catalog
            .persist_active_model(config().model)
            .expect("persist Session model");
        assert!(catalog.active_is_unused().expect("model choice only"));

        // A message that was accepted durably but never adopted is already
        // this Session's work, however empty the canonical history still is.
        let (session_id, node, _) = catalog.active_lineage().expect("root lineage");
        let pending_store = store_for(&catalog, &session_id, &node.conversation_id);
        pending_store
            .accept_inbound(crate::durable::InboundDraft {
                message_id: None,
                source: UserSource::Human,
                kind: InboundKind::Message,
                content: vec![text("pending")],
                timestamp: Utc::now(),
                correlation: None,
            })
            .expect("accept pending inbound");
        assert!(!catalog.active_is_unused().expect("pending inbound"));
        drop(pending_store);

        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        assert!(!catalog.active_is_unused().expect("used root"));

        // Branching at the first user message leaves the active node's own
        // conversation empty, while the Session itself is anything but.
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);
        let (prepared, _) = catalog
            .prepare_tree_node_at_user_message(
                &source_session,
                &state(),
                &source,
                &MessageId::new("source-user-a"),
            )
            .expect("prepare tree node");
        catalog
            .publish_node(
                &source_session,
                &prepared,
                source_node,
                SessionNodeOrigin::New,
            )
            .expect("publish branch node");
        assert!(!catalog.active_is_unused().expect("branched session"));

        // A newly published Session is unused again.
        let prepared = catalog
            .prepare_session(&state(), &[])
            .expect("prepare new session");
        catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .expect("publish new session");
        assert!(catalog.active_is_unused().expect("new session"));
    }

    #[test]
    fn new_and_name_publish_metadata_without_mutating_old_history() {
        let (_directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let before = source_store.load_canonical().expect("source history");

        let prepared = catalog
            .prepare_session(&state(), &[])
            .expect("prepare new session");
        let new_session_id = prepared.session_id.clone();
        let snapshot = catalog
            .publish_session(&prepared, SessionNodeOrigin::New)
            .expect("publish new session");

        assert_eq!(
            snapshot.name, None,
            "a published Session is born unnamed; nothing generates a label for it"
        );
        assert_ne!(snapshot.id, source_session);
        assert_ne!(
            snapshot.active_conversation_id, source_conversation,
            "new Session must own a new ConversationId"
        );
        assert_eq!(
            catalog
                .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("list page")
                .sessions
                .len(),
            2
        );
        assert_eq!(
            source_store
                .load_canonical()
                .expect("source history after new"),
            before
        );

        let renamed = catalog
            .rename(&new_session_id, "  review\n branch  ")
            .expect("rename metadata");
        assert_eq!(
            renamed.name.as_deref(),
            Some("review branch"),
            "a name is normalized into the single line a selector row shows"
        );
        assert!(matches!(
            catalog.rename(&new_session_id, "   "),
            Err(SessionError::InvalidName)
        ));
        assert!(matches!(
            catalog.rename(&new_session_id, &"n".repeat(super::SESSION_NAME_LIMIT + 1)),
            Err(SessionError::InvalidName)
        ));
        let new_nodes = catalog
            .node_page(&new_session_id, 0, super::SESSION_TREE_PAGE_LIMIT)
            .expect("new node page")
            .nodes;
        let new_node = &new_nodes[0];
        let new_store = store_for(&catalog, &new_session_id, &new_node.conversation_id);
        assert!(
            new_store
                .load_canonical()
                .expect("new canonical history")
                .is_empty(),
            "new starts with only the intended empty bootstrap state"
        );
    }

    /// A Session carries no name until a user gives it one, so a `/resume`
    /// row identifies it by what it opened with. Naming replaces that line
    /// and nothing else: the identity, the lineage, and the conversation are
    /// untouched, and the row remains searchable by either.
    #[test]
    fn an_unnamed_session_is_listed_by_its_first_user_message() {
        let (_directory, mut catalog, _config) = open_catalog();
        let row = |catalog: &SessionCatalog| {
            catalog
                .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("session page")
                .sessions
                .into_iter()
                .next()
                .expect("the active Session is listed")
        };

        let empty = row(&catalog);
        assert_eq!(empty.name, None, "a Session is born unnamed");
        assert_eq!(
            empty.preview, None,
            "a Session that was never used has no first message to show either"
        );

        append_history(
            &catalog,
            &[
                user("first", "  restore\n  the auth module  "),
                user("second", "and then the session picker"),
            ],
        );
        let used = row(&catalog);
        assert_eq!(
            used.preview.as_deref(),
            Some("restore the auth module"),
            "the row shows the Session's first user message as one collapsed line"
        );
        assert_eq!(used.name, None);
        assert!(
            !catalog
                .list_page(Some("auth module"), 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("content search")
                .sessions
                .is_empty(),
            "an unnamed row is searchable by the line it shows"
        );

        let session_id = catalog.active_snapshot().expect("active snapshot").id;
        catalog
            .rename(&session_id, "Refactor auth module")
            .expect("name the Session");
        let named = row(&catalog);
        assert_eq!(named.name.as_deref(), Some("Refactor auth module"));
        assert_eq!(
            named.preview.as_deref(),
            Some("restore the auth module"),
            "naming displaces the first message in the row, never in the Session"
        );
        assert_eq!(
            named.id, session_id,
            "a name is metadata; the identity a selection resolves is unchanged"
        );
    }

    /// The derived row line is bounded and text-only: a long first message is
    /// cut with a visible mark, and a Session opened with no text at all has
    /// no line to show rather than an invented one.
    #[test]
    fn a_derived_row_line_is_bounded_and_only_ever_text() {
        let (_directory, catalog, _config) = open_catalog();
        let long = "auth ".repeat(60);
        append_history(&catalog, &[user("long", long.as_str())]);
        let preview = catalog
            .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
            .expect("session page")
            .sessions
            .swap_remove(0)
            .preview
            .expect("a long message still yields a line");
        assert_eq!(preview.chars().count(), super::SESSION_PREVIEW_LIMIT);
        assert!(preview.ends_with('\u{2026}'), "a cut line says it was cut");

        let (_directory, catalog, _config) = open_catalog();
        append_history(
            &catalog,
            &[MessageBlock::User(UserMessageBlock {
                id: MessageId::new("image-only"),
                content: vec![UserContentBlock::Image(
                    crate::message::content::ImageReference {
                        artifact_id: crate::runtime::identity::ArtifactId::new("artifact-1"),
                        alt: None,
                    },
                )],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })],
        );
        assert_eq!(
            catalog
                .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("session page")
                .sessions
                .swap_remove(0)
                .preview,
            None
        );
    }

    #[test]
    fn session_list_projection_is_bounded_searchable_and_continuable() {
        let (_directory, mut catalog, _config) = open_catalog();
        for index in 0..3 {
            let prepared = catalog
                .prepare_session(&state(), &[])
                .expect("prepare paged session");
            let published = catalog
                .publish_session(&prepared, SessionNodeOrigin::New)
                .expect("publish paged session");
            catalog
                .rename(&published.id, &format!("paged session {index}"))
                .expect("name paged session");
        }

        let first = catalog
            .list_page(None, 0, 2)
            .expect("first bounded Session page");
        assert_eq!(first.sessions.len(), 2);
        assert_eq!(first.next_offset, Some(2));
        let second = catalog
            .list_page(None, first.next_offset.expect("continuation"), 2)
            .expect("second bounded Session page");
        assert_eq!(second.sessions.len(), 2);
        assert_eq!(second.next_offset, None);
        let ids = first
            .sessions
            .into_iter()
            .chain(second.sessions)
            .map(|summary| summary.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            ids.len(),
            4,
            "older Sessions remain reachable by continuation"
        );

        let filtered = catalog
            .list_page(Some("paged session 1"), 0, 2)
            .expect("searchable bounded Session page");
        assert_eq!(filtered.sessions.len(), 1);
        assert_eq!(
            filtered.sessions[0].name.as_deref(),
            Some("paged session 1")
        );
        assert!(matches!(
            catalog.list_page(None, 0, 0),
            Err(SessionError::Catalog { .. })
        ));
    }

    #[test]
    fn tree_and_history_projections_are_bounded_and_continuable() {
        let (_directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);
        let (prepared, _) = catalog
            .prepare_tree_node_at_user_message(
                &source_session,
                &state(),
                &source,
                &MessageId::new("source-user-c"),
            )
            .expect("prepare tree node");
        catalog
            .publish_node(
                &source_session,
                &prepared,
                source_node.clone(),
                SessionNodeOrigin::Fork {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: revision,
                    source_user_message: MessageId::new("source-user-c"),
                },
            )
            .expect("publish tree node");

        let node_first = catalog
            .node_page(&source_session, 0, 1)
            .expect("first bounded tree page");
        assert_eq!(node_first.nodes.len(), 1);
        assert_eq!(node_first.next_offset, Some(1));
        let node_second = catalog
            .node_page(
                &source_session,
                node_first.next_offset.expect("tree continuation"),
                1,
            )
            .expect("second bounded tree page");
        assert_eq!(node_second.nodes.len(), 1);
        assert_eq!(node_second.next_offset, None);

        let history_first = source_store
            .load_user_message_boundaries_page(revision, 0, 1)
            .expect("first bounded history page");
        assert_eq!(history_first.boundaries.len(), 1);
        assert_eq!(history_first.next_offset, Some(1));
        let history_second = source_store
            .load_user_message_boundaries_page(
                revision,
                history_first.next_offset.expect("history continuation"),
                1,
            )
            .expect("second bounded history page");
        assert_eq!(history_second.boundaries.len(), 1);
        assert_eq!(history_second.next_offset, None);
    }

    #[test]
    fn accepted_model_configuration_is_catalog_metadata_for_runtime_replacement() {
        let (directory, mut catalog, _config) = open_catalog();
        let mut model = config().model.clone();
        model.model = serde_json::from_value(serde_json::json!("provider/next-model"))
            .expect("model reference");
        catalog
            .persist_active_model(model.clone())
            .expect("persist model metadata");

        let reopened = reopen_catalog(directory.path());
        let (_, _, reopened_state) = reopened.active_lineage().expect("active lineage");
        assert_eq!(reopened_state.model, model);
    }

    #[test]
    fn resume_keeps_explicit_session_model_but_new_catalog_uses_current_default() {
        let directory = tempfile::tempdir().expect("temporary root");
        let first = config().model;
        let mut explicit = first.clone();
        explicit.model = serde_json::from_value(serde_json::json!("provider/explicit"))
            .expect("explicit model reference");
        let mut current = first.clone();
        current.model = serde_json::from_value(serde_json::json!("provider/current"))
            .expect("current model reference");

        let mut catalog = SessionCatalog::create(
            directory.path(),
            &SessionPersistentState {
                model: first.clone(),
            },
        )
        .expect("catalog");
        catalog
            .persist_active_model(explicit.clone())
            .expect("persist explicit Session model");
        let reopened = reopen_catalog(directory.path());
        let (_, _, resumed) = reopened.active_lineage().expect("resumed lineage");
        assert_eq!(resumed.model, explicit);

        let new_directory = tempfile::tempdir().expect("new Session root");
        let fresh = SessionCatalog::create(
            new_directory.path(),
            &SessionPersistentState {
                model: current.clone(),
            },
        )
        .expect("new catalog");
        let (_, _, fresh_state) = fresh.active_lineage().expect("fresh lineage");
        assert_eq!(fresh_state.model, current);
    }

    /// The lineage provenance promise moved, so the persisted version that
    /// gates it moved with it.
    ///
    /// A catalog written before the promise decodes perfectly: same fields,
    /// same `Clone` origin, same destination database. What differs is only
    /// what the destination's retained history *means* — the older seed
    /// flattened the source Surface into one append per active message, so
    /// its recorded branch points are artefacts of the copy. Nothing in the
    /// record distinguishes that from a genuinely copied history, so this
    /// code must refuse the document rather than read a lineage it would then
    /// fork at the wrong boundaries. The published clone below is what such a
    /// catalog holds; only its version is put back.
    #[test]
    fn a_pre_provenance_session_catalog_is_refused_rather_than_reinterpreted() {
        let (directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);
        let clone = catalog
            .prepare_clone_session(&state(), &source)
            .expect("prepare clone");
        catalog
            .publish_session(
                &clone,
                SessionNodeOrigin::Clone {
                    source_session,
                    source_node,
                    source_surface_revision: revision,
                },
            )
            .expect("publish clone");

        // Everything about the document stays as it is; only the version goes
        // back to the one written before copies retained their provenance.
        let path = catalog.path.clone();
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("catalog bytes")).expect("catalog JSON");
        assert_eq!(
            document["schema_version"],
            serde_json::json!(super::SESSION_CATALOG_SCHEMA_VERSION),
            "the published catalog carries the current schema"
        );
        document["schema_version"] = serde_json::json!(super::SESSION_CATALOG_SCHEMA_VERSION - 1);
        fs::write(
            &path,
            serde_json::to_vec(&document).expect("downgraded catalog"),
        )
        .expect("write downgraded catalog");

        let error = SessionCatalog::open_existing(directory.path())
            .expect_err("a pre-provenance catalog must not open");
        let SessionError::Catalog { detail } = error else {
            panic!("a version mismatch is a catalog error");
        };
        assert!(
            detail.contains("unsupported session catalog schema"),
            "the refusal names the schema, not some downstream symptom: {detail}"
        );
    }

    #[test]
    fn catalog_serialization_contains_no_current_runtime_configuration() {
        let directory = tempfile::tempdir().expect("temporary root");
        let state = state();
        let catalog = SessionCatalog::create(directory.path(), &state).expect("catalog");
        let bytes = fs::read(&catalog.path).expect("catalog bytes");
        let json = String::from_utf8(bytes).expect("catalog UTF-8");
        assert!(json.contains("\"state\""));
        assert!(json.contains("\"model\""));
        for forbidden in [
            "agent_id",
            "timezone",
            "context",
            "mcp_servers",
            "native_tools",
            "approval_mode",
            "approvalMode",
            "environment",
            "skills",
            "default_tools",
        ] {
            assert!(
                !json.contains(forbidden),
                "durable Session state must not persist current runtime field {forbidden:?}"
            );
        }
    }

    #[test]
    fn catalog_creation_persists_only_the_supplied_session_state() {
        let directory = tempfile::tempdir().expect("temporary root");
        let state = state();
        let catalog = SessionCatalog::create(directory.path(), &state).expect("catalog");

        let (_, _, persisted) = catalog.active_lineage().expect("active lineage");
        assert_eq!(persisted, state);
    }

    #[test]
    fn catalog_fault_before_rename_keeps_memory_and_file_on_old_document() {
        let (directory, mut catalog, _config) = open_catalog();
        let before = catalog.active_snapshot().expect("initial snapshot");

        catalog.arm_write_fault_before_rename();
        let error = catalog
            .rename(&before.id, "not published")
            .expect_err("the deterministic pre-rename fault must fail");
        assert!(matches!(
            error,
            SessionError::CatalogCommit {
                error: CatalogCommitError::NotCommitted { .. }
            }
        ));
        assert_eq!(
            catalog.active_snapshot().expect("in-memory snapshot"),
            before,
            "a pre-commit failure leaves the in-process document unchanged"
        );
        let reopened = reopen_catalog(directory.path());
        assert_eq!(
            reopened.active_snapshot().expect("reopened snapshot"),
            before
        );

        // The failed metadata mutation did not poison the catalog: absent
        // runtime quiescence, the same attachment remains usable.
        let retried = catalog
            .rename(&before.id, "published after retry")
            .expect("ordinary metadata remains usable after pre-commit failure");
        assert_eq!(retried.name.as_deref(), Some("published after retry"));
    }

    #[test]
    fn catalog_fault_after_rename_keeps_memory_coherent_and_reports_uncertain_durability() {
        let (directory, mut catalog, _config) = open_catalog();
        let before = catalog.active_snapshot().expect("initial snapshot");

        catalog.arm_write_fault_after_rename();
        let error = catalog
            .rename(&before.id, "visible but uncertain")
            .expect_err("the deterministic post-rename fault must fail");
        assert!(matches!(
            error,
            SessionError::CatalogCommit {
                error: CatalogCommitError::CommittedButDurabilityUncertain { .. }
            }
        ));
        let in_memory = catalog
            .active_snapshot()
            .expect("committed in-memory snapshot");
        assert_eq!(in_memory.name.as_deref(), Some("visible but uncertain"));
        let reopened = reopen_catalog(directory.path());
        assert_eq!(
            reopened
                .active_snapshot()
                .expect("reopened snapshot")
                .name
                .as_deref(),
            Some("visible but uncertain"),
            "the file crossed the visibility commit point even though durability was uncertain"
        );
    }

    #[test]
    fn clone_uses_exact_revision_and_isolates_execution_identity_domains() {
        let (_directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let selected_revision = source_store.load_head().expect("source head").revision;
        let source_snapshot = lineage_at(&source_store, &source_conversation, selected_revision);

        let prepared = catalog
            .prepare_clone_session(&state(), &source_snapshot)
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
                &prepared,
                SessionNodeOrigin::Clone {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: selected_revision,
                },
            )
            .expect("publish clone");
        assert_eq!(destination.id, destination_session);
        assert_eq!(destination.active_conversation_id, destination_conversation);
        assert_ne!(source_conversation, destination.active_conversation_id);
    }

    #[test]
    fn failed_clone_and_fork_publication_is_not_visible() {
        let (_directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);

        let clone = catalog
            .prepare_clone_session(&state(), &source)
            .expect("prepare clone");
        catalog.arm_write_fault_before_rename();
        assert!(matches!(
            catalog.publish_session(
                &clone,
                SessionNodeOrigin::Clone {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: revision,
                },
            ),
            Err(SessionError::CatalogCommit {
                error: CatalogCommitError::NotCommitted { .. }
            })
        ));
        assert_eq!(
            catalog
                .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("visible Session page")
                .sessions
                .len(),
            1,
            "failed clone publication does not expose a half-created Session"
        );

        let (fork, _) = catalog
            .prepare_fork_session(&state(), &source, &MessageId::new("source-user-c"))
            .expect("prepare fork");
        catalog.arm_write_fault_before_rename();
        assert!(matches!(
            catalog.publish_session(
                &fork,
                SessionNodeOrigin::Fork {
                    source_session,
                    source_node,
                    source_surface_revision: revision,
                    source_user_message: MessageId::new("source-user-c"),
                },
            ),
            Err(SessionError::CatalogCommit {
                error: CatalogCommitError::NotCommitted { .. }
            })
        ));
        assert_eq!(
            catalog
                .active_snapshot()
                .expect("source remains active")
                .node_count,
            1,
            "failed fork publication does not expose a half-created node"
        );
    }

    #[test]
    fn clone_and_fork_use_retained_surface_revision_after_real_compaction() {
        let (_directory, catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let retained_revision = source_store.load_head().expect("source head").revision;
        source_store
            .commit_compaction(CompactionCommitInput {
                summary: UserMessageBlock {
                    id: MessageId::new("compaction-summary"),
                    content: vec![text("compacted A")],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary,
                    timestamp: None,
                },
                span: SurfaceSpan::new(
                    MessageId::new("source-user-a"),
                    MessageId::new("source-user-a"),
                ),
                expected_revision: retained_revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 64,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 32,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc.with_ymd_and_hms(2026, 8, 21, 12, 0, 0).unwrap(),
            })
            .expect("real compaction commit");
        let retained = lineage_at(&source_store, &source_conversation, retained_revision);
        assert_eq!(retained.messages.len(), history.len());

        let prepared_clone = catalog
            .prepare_clone_session(&state(), &retained)
            .expect("clone retained revision");
        let clone_store = store_for(
            &catalog,
            &prepared_clone.session_id,
            &prepared_clone.conversation_id,
        );
        assert_eq!(
            clone_store.load_canonical().expect("clone history").len(),
            history.len(),
            "clone reads the retained pre-compaction revision rather than the compacted head"
        );

        let (prepared_fork, editor_content) = catalog
            .prepare_fork_session(&state(), &retained, &MessageId::new("source-user-c"))
            .expect("fork retained revision");
        assert_eq!(editor_content, vec![text("C")]);
        let fork_store = store_for(
            &catalog,
            &prepared_fork.session_id,
            &prepared_fork.conversation_id,
        );
        assert_eq!(
            fork_store.load_canonical().expect("fork history").len(),
            history.len() - 1,
            "fork stops immediately before the selected retained user boundary"
        );
    }

    /// A `todo` result committed by the source, exactly as the native tool
    /// publishes one: the complete post-mutation list as the result's
    /// structured content.
    fn todo_result(id: &str, call: &str, subject: &str) -> MessageBlock {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(id),
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![crate::tools::types::ToolResultContent::Json {
                    value: serde_json::json!({
                        "tasks": [{
                            "id": 1,
                            "subject": subject,
                            "status": "pending",
                            "blocked_by": [],
                        }],
                        "next_id": 2,
                    }),
                }],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        })
    }

    /// The assistant turn that called `todo`.
    fn todo_call(id: &str, call: &str) -> MessageBlock {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new(call),
                tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
                name: "todo".to_owned(),
                arguments: serde_json::json!({"action": "create", "subject": "Write the parser"}),
            })],
        })
    }

    /// What a lineage's conversation-owned task list rebuilds to, read the
    /// way a runtime opening that lineage reads it.
    fn todo_list_of(store: &SqliteConversationStore) -> crate::tools::todo::TodoSnapshot {
        crate::tools::todo::ConversationTodoList::rebuilt(
            store.conversation_id().clone(),
            &store
                .load_canonical()
                .expect("destination canonical history"),
        )
        .expect("the destination rebuilds a usable list")
        .committed()
    }

    /// A copy of a conversation means the same thing whether or not the
    /// source has been compacted since.
    ///
    /// This is the lineage invariant, stated on the first piece of
    /// conversation state that is *derived from canonical history* rather
    /// than read off the Surface: the task list. Compaction retires the
    /// `todo` result from the Surface and leaves it canonical, so the source
    /// still has its list — and a copy seeded from the Surface alone would
    /// not. The two clones below bracket exactly that transition, and the
    /// assertion is that they agree with each other and with the source.
    ///
    /// The property is deliberately not "the clone inherits the list". It is
    /// that *compaction is invisible to lineage semantics*: it changes the
    /// context projection, never what copying the conversation means. Any
    /// later conversation-owned state derived from canonical history inherits
    /// that guarantee from the seed rather than having to restate it.
    #[test]
    fn a_clone_means_the_same_before_and_after_the_compaction_that_retires_its_facts() {
        let (_directory, catalog, _config) = open_catalog();
        let history = vec![
            user("source-user-a", "A"),
            todo_call("source-todo-call", "call-todo"),
            todo_result("source-todo-result", "call-todo", "Write the parser"),
            user("source-user-c", "C"),
        ];
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let expected = todo_list_of(&source_store);
        assert_eq!(expected.tasks.len(), 1, "the source committed one task");

        // The clone taken while the `todo` result is still model-visible.
        let before = lineage_at(
            &source_store,
            &source_conversation,
            source_store.load_head().expect("source head").revision,
        );
        let clone_before = catalog
            .prepare_clone_session(&state(), &before)
            .expect("clone before compaction");

        // The compaction retires the whole span the result lives in.
        let head = source_store.load_head().expect("source head");
        source_store
            .commit_compaction(CompactionCommitInput {
                summary: UserMessageBlock {
                    id: MessageId::new("source-compaction-summary"),
                    content: vec![text("earlier work, summarized")],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary,
                    timestamp: None,
                },
                span: SurfaceSpan::new(
                    MessageId::new("source-user-a"),
                    MessageId::new("source-todo-result"),
                ),
                expected_revision: head.revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 64,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 32,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc.with_ymd_and_hms(2026, 8, 26, 12, 0, 0).unwrap(),
            })
            .expect("real compaction commit");
        let compacted = source_store.load_head().expect("compacted head");
        assert!(
            !compacted
                .active_message_ids
                .contains(&MessageId::new("source-todo-result")),
            "the fact the list is derived from is no longer model-visible"
        );
        assert_eq!(
            todo_list_of(&source_store),
            expected,
            "the source itself still has the list it committed"
        );

        // The clone taken from the compacted head.
        let after = lineage_at(&source_store, &source_conversation, compacted.revision);
        let clone_after = catalog
            .prepare_clone_session(&state(), &after)
            .expect("clone after compaction");

        let before_store = store_for(
            &catalog,
            &clone_before.session_id,
            &clone_before.conversation_id,
        );
        let after_store = store_for(
            &catalog,
            &clone_after.session_id,
            &clone_after.conversation_id,
        );
        assert_eq!(
            todo_list_of(&before_store),
            todo_list_of(&after_store),
            "a compaction between two clones cannot change what cloning means"
        );
        assert_eq!(
            todo_list_of(&after_store),
            expected,
            "the clone carries the conversation state the source has, not the \
             subset its Surface still shows"
        );

        // The inherited fact is inherited as the source holds it: canonical,
        // and not model-visible. A destination that put the retired result
        // back on its Surface would be re-showing context the source already
        // summarized away.
        let after_head = after_store.load_head().expect("clone head");
        assert_eq!(
            after_head.active_message_ids.len(),
            after.messages.len(),
            "the clone shows exactly the Surface it was cloned from"
        );
        assert!(
            after_store.load_canonical().expect("clone canonical").len()
                > after_head.active_message_ids.len(),
            "and still carries the canonical facts that Surface no longer shows"
        );
    }

    /// The same invariant on the other lineage constructor: a fork cuts the
    /// canonical history at the boundary it selected, and inherits the
    /// retired facts below that cut.
    #[test]
    fn a_fork_inherits_the_canonical_state_of_the_boundary_it_cuts_at() {
        let (_directory, catalog, _config) = open_catalog();
        let history = vec![
            user("source-user-a", "A"),
            todo_call("source-todo-call", "call-todo"),
            todo_result("source-todo-result", "call-todo", "Write the parser"),
            user("source-user-c", "C"),
        ];
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let expected = todo_list_of(&source_store);

        let head = source_store.load_head().expect("source head");
        source_store
            .commit_compaction(CompactionCommitInput {
                summary: UserMessageBlock {
                    id: MessageId::new("source-compaction-summary"),
                    content: vec![text("earlier work, summarized")],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary,
                    timestamp: None,
                },
                span: SurfaceSpan::new(
                    MessageId::new("source-user-a"),
                    MessageId::new("source-todo-result"),
                ),
                expected_revision: head.revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 64,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 32,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc.with_ymd_and_hms(2026, 8, 26, 12, 0, 0).unwrap(),
            })
            .expect("real compaction commit");

        let compacted = source_store.load_head().expect("compacted head");
        let source = lineage_at(&source_store, &source_conversation, compacted.revision);
        let (prepared, editor_content) = catalog
            .prepare_fork_session(&state(), &source, &MessageId::new("source-user-c"))
            .expect("fork at the surviving boundary");
        assert_eq!(editor_content, vec![text("C")]);

        let fork_store = store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
        assert_eq!(
            todo_list_of(&fork_store),
            expected,
            "the fork inherits the conversation state in effect at its boundary"
        );
        assert_eq!(
            fork_store
                .load_head()
                .expect("fork head")
                .active_message_ids
                .len(),
            1,
            "and shows only the summary the source's Surface shows before that boundary"
        );
    }

    /// A compaction of the span `[A ..= todo_result]`, committed exactly as
    /// the runtime commits one.
    fn compact_the_retired_span(store: &SqliteConversationStore) -> SurfaceRevision {
        let head = store.load_head().expect("source head");
        store
            .commit_compaction(CompactionCommitInput {
                summary: UserMessageBlock {
                    id: MessageId::new("source-compaction-summary"),
                    content: vec![text("earlier work, summarized")],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary,
                    timestamp: None,
                },
                span: SurfaceSpan::new(
                    MessageId::new("source-user-a"),
                    MessageId::new("source-todo-result"),
                ),
                expected_revision: head.revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 64,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 32,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc.with_ymd_and_hms(2026, 8, 26, 12, 0, 0).unwrap(),
            })
            .expect("real compaction commit");
        store.load_head().expect("compacted head").revision
    }

    /// What a lineage carries, in a form two different lineages can be
    /// compared in: identities are lineage-owned and deliberately differ, so
    /// the comparable fact is the content.
    fn shape(message: &MessageBlock) -> String {
        match message {
            MessageBlock::User(user) => {
                let text = user
                    .content
                    .iter()
                    .map(|block| match block {
                        UserContentBlock::Text(block) => block.text.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("user({:?}, {text})", user.kind)
            }
            MessageBlock::Assistant(assistant) => format!("assistant({})", assistant.content.len()),
            MessageBlock::Tool(tool) => format!("tool({})", tool.tool_id),
        }
    }

    fn shapes(messages: &[MessageBlock]) -> Vec<String> {
        messages.iter().map(shape).collect()
    }

    /// What a lineage durably carries, retired facts included.
    fn canonical_shapes(store: &SqliteConversationStore) -> Vec<String> {
        shapes(&store.load_canonical().expect("canonical history"))
    }

    /// What a lineage currently shows the model.
    fn surface_shapes(store: &SqliteConversationStore) -> Vec<String> {
        let head = store.load_head().expect("head");
        shapes(
            &store
                .load_surface_snapshot(head.revision)
                .expect("Surface snapshot"),
        )
    }

    /// The shape of the user message both lineages are branched at.
    const SELECTED_BOUNDARY: &str = "user(Message, C)";

    /// The branch points a lineage reports for its own current head, in the
    /// form two different lineages can be compared in.
    fn reported_boundaries(
        store: &SqliteConversationStore,
    ) -> Vec<(SurfaceRevision, String, MessageId)> {
        let revision = store.load_head().expect("head").revision;
        store
            .load_user_message_boundaries(revision)
            .expect("historical boundaries")
            .into_iter()
            .map(|boundary| {
                (
                    boundary.surface_revision,
                    shape(&MessageBlock::User(boundary.message.clone())),
                    boundary.message.id,
                )
            })
            .collect()
    }

    /// The clone of a compacted source that carries a `todo` result its
    /// Surface no longer shows: the shape every test below branches.
    fn compacted_source_and_its_clone(
        catalog: &SessionCatalog,
    ) -> (
        SqliteConversationStore,
        ConversationId,
        SqliteConversationStore,
        PreparedLineage,
        crate::tools::todo::TodoSnapshot,
    ) {
        let history = vec![
            user("source-user-a", "A"),
            todo_call("source-todo-call", "call-todo"),
            todo_result("source-todo-result", "call-todo", "Write the parser"),
            user("source-user-c", "C"),
        ];
        let (source_conversation, source_session, _source_node) = append_history(catalog, &history);
        let source_store = store_for(catalog, &source_session, &source_conversation);
        let expected = todo_list_of(&source_store);
        let compacted = compact_the_retired_span(&source_store);
        let clone = catalog
            .prepare_clone_session(
                &state(),
                &lineage_at(&source_store, &source_conversation, compacted),
            )
            .expect("clone the compacted source");
        let clone_store = store_for(catalog, &clone.session_id, &clone.conversation_id);
        (
            source_store,
            source_conversation,
            clone_store,
            clone,
            expected,
        )
    }

    /// Copying a lineage is closed under the lineage operations that follow
    /// the copy.
    ///
    /// The two earlier lineage invariants say a copy keeps what the source
    /// currently *means* and currently *shows*. Neither is enough on its own,
    /// because a copy is not a terminal object: the user forks and branches
    /// the copy afterwards, and those operations read the copy's *history*,
    /// not its current state.
    ///
    /// Compaction is where that bites. It makes Surface order and Ledger
    /// order disagree — the summary is the newest canonical row and the
    /// oldest active one — so a copy rebuilt from the final projection alone
    /// would record a history in which the summary predates a user message it
    /// actually postdates. The copy looks identical and branches differently:
    /// forking it at that user message would carry the message itself into
    /// the canonical prefix the fork exists to cut before, and hand the same
    /// message back to the editor as an uncommitted prompt.
    ///
    /// So the invariant is stated on the composition, not on the copy: a
    /// fork taken on a copy, at a boundary the copy itself reports, means
    /// what the same fork taken on the source means.
    #[test]
    fn a_copy_branches_where_its_source_branches() {
        let (_directory, catalog, _config) = open_catalog();
        let (source_store, source_conversation, clone_store, clone, expected) =
            compacted_source_and_its_clone(&catalog);

        // The copy reports the source's branch points, at the source's
        // revisions. A copy rebuilt from the final projection reports `C` at
        // the revision it was re-appended in, which is a branch point the
        // source never had.
        let source_boundaries = reported_boundaries(&source_store);
        let clone_boundaries = reported_boundaries(&clone_store);
        let positions = |boundaries: &[(SurfaceRevision, String, MessageId)]| {
            boundaries
                .iter()
                .map(|(revision, shape, _)| (*revision, shape.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            positions(&clone_boundaries),
            positions(&source_boundaries),
            "the copy offers the branch points the source offers, in the same \
             historical positions"
        );

        // Each lineage is forked at its own reported boundary, through the
        // ordinary path: no revision is hand-constructed here.
        let fork = |store: &SqliteConversationStore,
                    conversation: &ConversationId,
                    boundaries: &[(SurfaceRevision, String, MessageId)]| {
            let (revision, _, message_id) = boundaries
                .iter()
                .find(|(_, shape, _)| shape == SELECTED_BOUNDARY)
                .expect("the selected boundary");
            catalog
                .prepare_fork_session(
                    &state(),
                    &lineage_at(store, conversation, *revision),
                    message_id,
                )
                .expect("fork at the reported boundary")
        };
        let (source_fork, source_editor) =
            fork(&source_store, &source_conversation, &source_boundaries);
        let (clone_fork, clone_editor) =
            fork(&clone_store, &clone.conversation_id, &clone_boundaries);
        let source_fork_store = store_for(
            &catalog,
            &source_fork.session_id,
            &source_fork.conversation_id,
        );
        let clone_fork_store = store_for(
            &catalog,
            &clone_fork.session_id,
            &clone_fork.conversation_id,
        );

        assert_eq!(clone_editor, vec![text("C")]);
        assert_eq!(
            clone_editor, source_editor,
            "the same boundary is restored to the editor from either lineage"
        );
        assert_eq!(
            canonical_shapes(&clone_fork_store),
            canonical_shapes(&source_fork_store),
            "a fork of the copy inherits the canonical prefix a fork of the source \
             inherits"
        );
        assert_eq!(
            surface_shapes(&clone_fork_store),
            surface_shapes(&source_fork_store),
            "and shows what that fork shows"
        );

        // The specific failure the composition exists to catch: the boundary
        // is what the fork cuts *before*, so it can never also be a canonical
        // fact the fork inherits — otherwise the same message is at once
        // committed history and an uncommitted prompt.
        assert!(
            !canonical_shapes(&clone_fork_store)
                .iter()
                .any(|shape| shape == SELECTED_BOUNDARY),
            "a fork cannot inherit the very message it hands back to the editor"
        );
        assert_eq!(
            todo_list_of(&clone_fork_store),
            expected,
            "and still inherits the conversation state in effect at that boundary"
        );
        assert_eq!(
            todo_list_of(&clone_fork_store),
            todo_list_of(&source_fork_store)
        );
    }

    /// A tree node shares the fork's boundary semantics, so it shares the
    /// closure property: branching a copy at a copied boundary means what
    /// branching the source at that boundary means.
    #[test]
    fn a_tree_node_of_a_copy_branches_where_its_source_branches() {
        let (_directory, catalog, _config) = open_catalog();
        let (source_store, source_conversation, clone_store, clone, expected) =
            compacted_source_and_its_clone(&catalog);

        let branch = |store: &SqliteConversationStore,
                      session_id: &SessionId,
                      conversation: &ConversationId| {
            let (revision, _, message_id) = reported_boundaries(store)
                .into_iter()
                .find(|(_, shape, _)| shape == SELECTED_BOUNDARY)
                .expect("the selected boundary");
            catalog
                .prepare_tree_node_at_user_message(
                    session_id,
                    &state(),
                    &lineage_at(store, conversation, revision),
                    &message_id,
                )
                .expect("branch at the reported boundary")
        };
        let (source_session, _, _) = catalog.active_lineage().expect("source lineage");
        let (source_node, source_editor) =
            branch(&source_store, &source_session, &source_conversation);
        let (clone_node, clone_editor) =
            branch(&clone_store, &clone.session_id, &clone.conversation_id);
        assert_eq!(clone_editor, vec![text("C")]);
        assert_eq!(clone_editor, source_editor);

        let source_node_store = store_for(
            &catalog,
            &source_node.session_id,
            &source_node.conversation_id,
        );
        let clone_node_store = store_for(
            &catalog,
            &clone_node.session_id,
            &clone_node.conversation_id,
        );
        assert_eq!(
            canonical_shapes(&clone_node_store),
            canonical_shapes(&source_node_store),
            "a branch of the copy inherits the canonical prefix a branch of the \
             source inherits"
        );
        assert!(
            !canonical_shapes(&clone_node_store)
                .iter()
                .any(|shape| shape == SELECTED_BOUNDARY),
            "a branch cannot inherit the very message it hands back to the editor"
        );
        assert_eq!(
            todo_list_of(&clone_node_store),
            expected,
            "and still inherits the conversation state in effect at that boundary"
        );
    }

    #[test]
    fn fork_seeds_before_user_and_returns_uncommitted_prompt() {
        let (_directory, catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);

        let mut current_session_intent = state();
        current_session_intent.model.model =
            serde_json::from_value(serde_json::json!("provider/current-session-intent"))
                .expect("current model");
        let (prepared, editor_content) = catalog
            .prepare_fork_session(
                &current_session_intent,
                &source,
                &MessageId::new("source-user-c"),
            )
            .expect("prepare fork");
        assert_eq!(
            prepared.state, current_session_intent,
            "lineage history selection cannot restore old node-local control state"
        );
        assert_eq!(editor_content, vec![text("C")]);
        let destination_store =
            store_for(&catalog, &prepared.session_id, &prepared.conversation_id);
        let prefix = destination_store
            .load_canonical()
            .expect("fork canonical prefix");
        assert_eq!(prefix.len(), 3);
        assert!(prefix.iter().all(|message| {
            !matches!(message, MessageBlock::User(user) if user.id == MessageId::new("source-user-c"))
        }));

        let source_after_prepare = source
            .messages
            .iter()
            .map(super::message_id_of)
            .collect::<Vec<_>>();
        assert_eq!(source_after_prepare.len(), 4);
        assert_eq!(
            source_store
                .load_canonical()
                .expect("source remains untouched")
                .len(),
            4
        );
    }

    #[test]
    fn fork_preserves_earlier_status_and_excludes_selected_turn_status() {
        let (_directory, catalog, _config) = open_catalog();
        let history = vec![
            user("source-user-a", "A"),
            status("status-before", "Status1"),
            user("source-user-c", "C"),
            status("status-after", "Status2"),
        ];
        let (source_conversation, source_session, _source_node) =
            append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);

        let (prepared, editor_content) = catalog
            .prepare_fork_session(&state(), &source, &MessageId::new("source-user-c"))
            .expect("prepare fork");
        assert_eq!(editor_content, vec![text("C")]);
        let prefix = store_for(&catalog, &prepared.session_id, &prepared.conversation_id)
            .load_canonical()
            .expect("fork prefix");
        assert_eq!(prefix.len(), 2);
        assert!(matches!(
            &prefix[1],
            MessageBlock::User(message)
                if message.kind == InboundKind::Context(ContextKind::AgentStatus)
                    && message.content == vec![text("Status1")]
        ));
        assert!(prefix.iter().all(|message| {
            !matches!(message, MessageBlock::User(message)
                if message.content == vec![text("C")] || message.content == vec![text("Status2")])
        }));
    }

    #[test]
    fn tree_branch_is_a_distinct_linear_node_and_failed_publication_is_invisible() {
        let (_directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);

        let before_failed = catalog.snapshot(&source_session).expect("source snapshot");
        let failed = catalog
            .prepare_session(&state(), &[])
            .expect("prepare private destination");
        std::fs::remove_file(&failed.database_path).expect("remove private seed");
        assert!(matches!(
            catalog.publish_session(&failed, SessionNodeOrigin::New),
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
                &state(),
                &source,
                &MessageId::new("source-user-c"),
            )
            .expect("prepare tree node");
        let branch_conversation = prepared.conversation_id.clone();
        let snapshot = catalog
            .publish_node(
                &source_session,
                &prepared,
                source_node.clone(),
                SessionNodeOrigin::Fork {
                    source_session: source_session.clone(),
                    source_node,
                    source_surface_revision: revision,
                    source_user_message: MessageId::new("source-user-c"),
                },
            )
            .expect("publish tree node");
        assert_eq!(snapshot.node_count, 2);
        assert_eq!(snapshot.active_conversation_id, branch_conversation);
        let nodes = catalog
            .node_page(&source_session, 0, super::SESSION_TREE_PAGE_LIMIT)
            .expect("tree node page")
            .nodes;
        assert_eq!(snapshot.active_node, nodes[1].id);
        assert_ne!(nodes[0].conversation_id, branch_conversation);
        assert!(
            nodes
                .iter()
                .all(|node| node.conversation_id != source_conversation
                    || node.origin == SessionNodeOrigin::New)
        );
        assert_eq!(
            catalog
                .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("list page")
                .sessions
                .len(),
            1,
            "tree branch stays in one Session"
        );

        // Session and node ordinals are independent domains. A new Session
        // after a tree branch must not reuse the branch's globally unique
        // SessionNodeId.
        let new_session = catalog
            .prepare_session(&state(), &[])
            .expect("prepare new session after tree branch");
        assert_ne!(new_session.node_id, snapshot.active_node);
        catalog
            .publish_session(&new_session, SessionNodeOrigin::New)
            .expect("publish new session after tree branch");
        assert_eq!(
            catalog
                .list_page(None, 0, super::SESSION_LIST_PAGE_LIMIT)
                .expect("list page")
                .sessions
                .len(),
            2
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn tree_nodes_switch_between_independent_lineages_without_rewind() {
        let (_directory, mut catalog, _config) = open_catalog();
        let history = source_history();
        let (source_conversation, source_session, source_node) = append_history(&catalog, &history);
        let source_store = store_for(&catalog, &source_session, &source_conversation);
        let revision = source_store.load_head().expect("source head").revision;
        let source = lineage_at(&source_store, &source_conversation, revision);

        let (prepared_a, _) = catalog
            .prepare_tree_node_at_user_message(
                &source_session,
                &state(),
                &source,
                &MessageId::new("source-user-a"),
            )
            .expect("prepare branch A");
        catalog
            .publish_node(
                &source_session,
                &prepared_a,
                source_node.clone(),
                SessionNodeOrigin::Fork {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: revision,
                    source_user_message: MessageId::new("source-user-a"),
                },
            )
            .expect("publish branch A");
        let (prepared_b, _) = catalog
            .prepare_tree_node_at_user_message(
                &source_session,
                &state(),
                &source,
                &MessageId::new("source-user-c"),
            )
            .expect("prepare branch B");
        catalog
            .publish_node(
                &source_session,
                &prepared_b,
                source_node.clone(),
                SessionNodeOrigin::Fork {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: revision,
                    source_user_message: MessageId::new("source-user-c"),
                },
            )
            .expect("publish branch B");

        let nodes = catalog
            .node_page(&source_session, 0, super::SESSION_TREE_PAGE_LIMIT)
            .expect("all branch nodes")
            .nodes;
        assert_eq!(nodes.len(), 3);
        let branch_a = &nodes[1];
        let branch_b = &nodes[2];
        assert_ne!(branch_a.conversation_id, branch_b.conversation_id);
        let destination_store_a = store_for(&catalog, &source_session, &branch_a.conversation_id);
        let destination_store_b = store_for(&catalog, &source_session, &branch_b.conversation_id);
        destination_store_a
            .append_canonical(&user("branch-a-late", "A later"))
            .expect("mutate branch A");
        destination_store_b
            .append_canonical(&user("branch-b-late", "B later"))
            .expect("mutate branch B");

        let selected_a = catalog
            .select(&source_session, Some(&branch_a.id))
            .expect("select branch A");
        assert_eq!(selected_a.active_conversation_id, branch_a.conversation_id);
        let selected_b = catalog
            .select(&source_session, Some(&branch_b.id))
            .expect("select branch B");
        assert_eq!(selected_b.active_conversation_id, branch_b.conversation_id);
        let selected_a_again = catalog
            .select(&source_session, Some(&branch_a.id))
            .expect("select branch A again");
        assert_eq!(
            selected_a_again.active_conversation_id,
            branch_a.conversation_id
        );

        let ids_a = destination_store_a
            .load_canonical()
            .expect("branch A history")
            .into_iter()
            .map(|message| super::message_id_of(&message))
            .collect::<BTreeSet<_>>();
        let ids_b = destination_store_b
            .load_canonical()
            .expect("branch B history")
            .into_iter()
            .map(|message| super::message_id_of(&message))
            .collect::<BTreeSet<_>>();
        assert!(ids_a.contains(&MessageId::new("branch-a-late")));
        assert!(!ids_a.contains(&MessageId::new("branch-b-late")));
        assert!(ids_b.contains(&MessageId::new("branch-b-late")));
        assert!(!ids_b.contains(&MessageId::new("branch-a-late")));
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
