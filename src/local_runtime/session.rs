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
//!
//! A Session-local current node is graph state, never process-global focus.
//! All catalog operations address identities. The accepted-inbound watermark
//! remains a monotonic usage classifier; unused Sessions are still listed and
//! are never implicitly reused. See docs/durable-sessions.md for ownership.

use crate::durable::JournalStore;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self};

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::durable::{ConversationStoreError, LineageSeed, SqliteConversationStore};
use crate::message::types::{MessageBlock, UserContentBlock, UserMessageBlock};
use crate::model::session::SessionModelConfig;
use crate::runtime::identity::{ConversationId, MessageId};

/// Time one create-pipeline stage when the test-only profiler is enabled, and
/// evaluate the body directly in every other build. The stage selector is only
/// consumed by the test expansion, so no production path carries timing state.
#[cfg(test)]
macro_rules! profile_stage {
    ($select:expr, $body:expr) => {
        create_profile::timed($select, || $body)
    };
}
#[cfg(not(test))]
macro_rules! profile_stage {
    ($select:expr, $body:expr) => {
        $body
    };
}

#[cfg(test)]
pub(crate) mod create_profile;

/// The persisted native session-catalog schema.
///
/// Version 5 reserves node allocations exclusively for Conversation-private
/// state.
/// Version 4 allocations cannot prove this boundary and are refused.
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
/// Version 13 persists the derived display projection (`display_preview`) on
/// each Session, so listing, pagination, and search read catalog metadata
/// only and never open a conversation store. The projection is published by
/// the one-shot display-projection publisher after the first canonical
/// root-lineage ordinary user commit, derived from the frozen seed at
/// clone/fork publication, and backfilled by the explicit idempotent repair
/// seam at reopen/compose/recovery. A version-12 catalog carries no such
/// field and is refused: there is no migration, because the projection must
/// be provably derived from canonical history, not reinterpreted.
///
/// Issue #387 adds a second, independent format boundary at the local root:
/// the private `conversation-reservations/` namespace records consumed
/// `ConversationId`s with exclusive create-new markers. A populated root that
/// predates that namespace is refused at the storage owner because absence of
/// a marker cannot prove an identity was never allocated under the older
/// layout; the old Session-directory scan is not retained as a fallback. The
/// Session Catalog schema is unchanged (still 13) because the catalog's
/// record layout did not change.
///
/// Manual reset of a development runtime root therefore means deleting the
/// **whole runtime root** (the `sessions/` tree *and* the
/// `conversation-reservations/` namespace) and recreating the Sessions. Never
/// delete only `sessions/catalog.json`, and never delete only the reservation
/// namespace: either leaves consumed identities and allocations that the
/// other side can no longer account for. Reset is always an explicit operator
/// action; rustX never deletes or reinterprets old data.
/// Version 9 owns workspace upload allocations and frozen private-copy claims.
/// Version 8 removes global focus and persists only explicit Session inputs, with
/// per-Session settings revisions. Schema 7 materialized model defaults cannot be
/// distinguished from user choices; older development schemas are refused.
/// Version 7 co-locates live Sessions and pending-only frozen cleanup authority,
/// with generation-checked publication. Older development schemas are rejected.
/// Version 15 persists execution ownership generations at the active-node visibility commit.
pub const SESSION_CATALOG_SCHEMA_VERSION: u32 = 18;

/// The largest display name a Session may carry.
pub const SESSION_NAME_LIMIT: usize = 120;

/// UTF-8 byte bound shared by generated-title acceptance, storage and rendering.
pub const SESSION_TITLE_BYTE_LIMIT: usize = 480;

/// Saved automatic display metadata, independent of a user's chosen name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionGeneratedTitle {
    pub text: String,
    pub provenance: SessionTitleProvenance,
}

/// A copied label never claims that generation ran in the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionTitleProvenance {
    Local {
        conversation_id: ConversationId,
        message_id: MessageId,
        call_id: crate::runtime::identity::AuxiliaryCallId,
    },
    Inherited {
        source_session: SessionId,
    },
}

/// The largest `/resume` row preview derived from a Session's first user
/// message.
const SESSION_PREVIEW_LIMIT: usize = 120;

/// The largest page a native Session list request may return.
pub const SESSION_LIST_PAGE_LIMIT: usize = 32;

/// The largest page a native Session tree/history request may return.
pub const SESSION_TREE_PAGE_LIMIT: usize = 32;

pub mod observation;
pub use crate::runtime::identity::{SessionId, SessionNodeId};

/// Why one `SessionNode` was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum SessionNodeOrigin {
    /// A new empty conversation lineage.
    New,
    /// A clone with structure selected at source Surface revision R.
    /// Temporal provenance was frozen separately at invocation-time native cut C.
    Clone {
        /// Source Session identity.
        source_session: SessionId,
        /// Source node identity.
        source_node: SessionNodeId,
        /// Selected structural source revision R, not an execution timestamp.
        source_surface_revision: SurfaceRevision,
    },
    /// A lineage structurally narrowed at one selected message boundary.
    /// Temporal provenance remains bounded to the captured native cut C.
    Fork {
        /// Source Session identity.
        source_session: SessionId,
        /// Source node identity.
        source_node: SessionNodeId,
        /// Selected structural source revision R, not an execution timestamp.
        source_surface_revision: SurfaceRevision,
        /// Exact selected source message; `side` determines the retained prefix.
        source_message: MessageId,
        /// Exact immutable canonical location, captured by the Fork owner before publication.
        source_locator: crate::observation::content::CanonicalContentLocator,
        /// Explicit retained side of the selected message.
        side: LineageSide,
    },
}

/// Which history prefix a lineage operation retains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineageSide {
    /// Exclude the ordinary User input and restore it to the editor (Retry).
    Before,
    /// Include the completed Assistant response; leave the editor empty.
    After,
}

/// One node in the native Session graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionNode {
    /// Explicit durable publication order, never inferred from UUID bytes.
    pub ordinal: u64,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionSnapshot {
    /// Session identity.
    pub id: SessionId,
    /// The user-defined display name, when this Session has one.
    ///
    /// A Session is born unnamed. The name is display metadata a user
    /// chooses, never an identity: nothing resolves a Session by it, and an
    /// unnamed Session is a complete, ordinary Session.
    pub name: Option<String>,
    /// Automatic label, with local or inherited generation provenance.
    pub generated_title: Option<SessionGeneratedTitle>,
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

/// A bounded page of resume-visible Session summaries.
///
/// The page includes every durable Session, including untouched ones. Usage and
/// client focus do not determine catalog visibility. Every row is projected
/// from catalog metadata alone; no conversation store is opened to build it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListPage {
    /// Rows newest human-message first, with Session identity breaking ties.
    pub sessions: Vec<SessionSummary>,
    /// Offset for the next page, when more matching rows exist.
    pub next_offset: Option<usize>,
}

/// A bounded page of nodes in one Session graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNodePage {
    /// Nodes in explicit node publication-ordinal order.
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

/// What the root lineage's durable history offers as the Session's display
/// projection subject, as read by the explicit repair seam.
///
/// The distinction between the two empty outcomes is load-bearing: a Session
/// whose root lineage has **no** ordinary user boundary yet is a live
/// publication candidate (its first commit is still ahead of it), while a
/// Session whose first boundary exists but renders no text has a settled
/// `None` projection that must never be rewritten or shifted to a later
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DisplayPreviewSubject {
    /// The first ordinary user boundary rendered a bounded single line.
    Derived(String),
    /// The root lineage has no ordinary user boundary at all.
    NoBoundary,
    /// The first ordinary user boundary exists but has no renderable text.
    Unrenderable,
}

/// Native display metadata, shared by exact identity reads and catalog rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionSummary {
    /// Native execution ownership generation, encoded as an exact decimal integer.
    /// Changes only when active-node ownership is replaced; never a display timestamp.
    pub ownership_generation: String,
    /// Canonical durable Session cwd, projected without loading a runtime.
    pub cwd: PathBuf,
    /// Session identity.
    pub id: SessionId,
    /// The user-defined display name, when this Session has one.
    pub name: Option<String>,
    /// Automatic label; manual name takes precedence when both are present.
    pub generated_title: Option<SessionGeneratedTitle>,
    /// The first ordinary user message of this Session's root lineage,
    /// bounded to one line. It labels a row without a saved name/title, and
    /// it is the client-facing projection of the persisted
    /// `display_preview`: the catalog stores the derived line so listing,
    /// pagination, and search never open a conversation store. `None` is
    /// legitimate — no ordinary user message exists yet, or the first one
    /// has no renderable text — and yields the client-side identity
    /// fallback.
    pub preview: Option<String>,
    /// Latest committed human-message time, or creation time for a new Session.
    pub updated_at: DateTime<Utc>,
    /// Active node in the session.
    pub active_node: SessionNodeId,
}

/// One user-message boundary the native product exposes for `/fork` and
/// `/tree`. The revision fixes structural selection; temporal evidence is
/// captured separately at the copy operation's native read cut C.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionUserMessageBoundary {
    /// The exact retained Surface revision containing the message.
    pub surface_revision: SurfaceRevision,
    /// The canonical user message at that boundary.
    pub message: UserMessageBlock,
}

/// A source lineage captured at native authority (Surface revision R, read cut C).
///
/// Its structural selection needs three views, which are not the same set of facts:
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
/// records its append boundaries. `projections` independently preserves every
/// eligible historical activation, including distinct activations at one Ledger
/// frontier, so later fork/tree reads retain the source semantics — see [`lineage_cut`].
///
/// Native `read_lineage_cut(R)` freezes authority (R, C): R selects structural
/// history/canonical closure, C selects temporal execution-derived meaning.
/// `prepare_*` may narrow the structural prefix at a boundary, but cannot widen
/// C. The same R read later at a newer C may retain newer terminal facts.
/// This is a lineage input, not a complete execution snapshot: it excludes
/// executable Attempt/request state, Event Journal facts, `RequestSnapshots`,
/// Pending Inbound, and execution-derived Todo reminder progress or heads.
pub use crate::durable::reading::LineageReadCut;

/// Deliberate Session-owned intent. Effective configuration is never persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionPersistentState {
    pub cwd: PathBuf,
    /// Explicit initial selection. Omission delegates to the native configuration
    /// owner at creation; clients must not copy a projected default into this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SessionModelConfig>,
}
impl SessionPersistentState {
    #[must_use]
    pub fn from_input(input: &super::configuration::SessionConfigInput) -> Self {
        Self {
            cwd: input.cwd.clone(),
            model: input.model.clone(),
        }
    }
    #[must_use]
    pub fn input(&self) -> super::configuration::SessionConfigInput {
        super::configuration::SessionConfigInput {
            cwd: self.cwd.clone(),
            model: self.model.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredIdentities {
    sessions: BTreeSet<SessionId>,
    nodes: BTreeSet<SessionNodeId>,
    conversations: BTreeSet<ConversationId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct CatalogDocument {
    retired: RetiredIdentities,
    upload_preparations: BTreeMap<SessionId, Vec<PathBuf>>,
    generation: u64,
    deletions: BTreeMap<SessionId, deletion::DeletionRecord>,
    schema_version: u32,
    next_session_ordinal: u64,
    next_node_ordinal: u64,
    sessions: BTreeMap<SessionId, PersistedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct PersistedSession {
    ownership_generation: u64,
    ordinal: u64,
    uploads: uploads::UploadRegistry,
    id: SessionId,
    /// Absent until a user names this Session; see [`SessionSnapshot::name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    // Required even when null: falsely current pre-title documents are refused.
    #[serde(deserialize_with = "Option::deserialize")]
    generated_title: Option<SessionGeneratedTitle>,
    /// Session-owned derived display metadata: the bounded first-ordinary-user-message
    /// line [`SessionSummary::preview`] projects.
    ///
    /// Display precedence is: the explicit [`PersistedSession::name`] when one
    /// exists, then generated title, this projection and client-side identity.
    /// It is *derived* display state, never canonical: nothing feeds it to the
    /// model, resolves identity by it, or lets recovery depend on it, and the
    /// root conversation's own durable store remains the only authority for
    /// what was said.
    ///
    /// Publication has exactly three seams. Clone/fork destinations carry the
    /// value derived from their frozen seed at publication. A fresh Session is
    /// born with `None` and the one-shot display-projection publisher fills it
    /// after the first canonical root-lineage ordinary user commit. The
    /// explicit idempotent repair seam (reopen/compose/recovery) backfills a
    /// missing value from the root store. `None` is therefore legitimate
    /// state — no ordinary user message exists yet, or the first one has no
    /// renderable text — and must never trigger rewrites or shift the subject
    /// to a later message. Publishing bumps the catalog generation like any
    /// commit but, like `commit_uploads`, never touches `updated_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_preview: Option<String>,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_prompt_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    active_node: SessionNodeId,
    nodes: BTreeMap<SessionNodeId, SessionNode>,
    /// Only intentionally Session-local choices are persisted here.
    state: SessionPersistentState,
    settings_revision: u64,
}

mod artifacts;
mod catalog_commit;
mod creation;
mod invalidations;
mod lineage;
mod metadata;
mod projection;
mod title;
pub use catalog_commit::CatalogCommitError;
#[cfg(test)]
use catalog_commit::CatalogWriteFault;
use catalog_commit::{validate_document, validate_id};
#[cfg(test)]
pub(crate) use creation::fail_next_database_initializations;
pub(crate) use invalidations::SessionSummaryInvalidations;
pub(crate) use lineage::PreparedLineage;
#[cfg(test)]
pub(crate) use lineage::{lineage_cut, remap_seed};
pub(crate) use projection::preview_of;
pub(crate) mod deletion;
pub mod uploads;

/// The native durable `SessionCatalog` and graph authority.
#[derive(Debug, Clone)]
pub struct SessionCatalog {
    identities: Arc<dyn crate::runtime::identity::UuidV7Generator>,
    product: crate::runtime::local_storage::ProductRoot,
    root: PathBuf,
    path: PathBuf,
    document: CatalogDocument,
    node_observation_index: Arc<BTreeMap<SessionId, Vec<SessionNodeId>>>,
    /// Whether `document` has ever been written to `path`.
    ///
    /// A first launch composes against a catalog that exists only in
    /// memory (see [`Self::create_unpublished`]), so an unpublished catalog
    /// has a pending first write even when nothing about the launch changed
    /// it: [`Self::plan_unchanged`] plans that write, and it lands in the
    /// same single startup transaction as every other catalog decision.
    published: bool,
    lifecycle: Option<std::sync::Arc<crate::runtime::local_storage::ProductController>>,
    #[cfg(test)]
    write_fault: Arc<Mutex<Option<CatalogWriteFault>>>,
    #[cfg(test)]
    preparation_cleanup_fault: Arc<std::sync::atomic::AtomicBool>,
    /// Test-only gate parked between the Surface-head observation and the
    /// acceptance-watermark read of `is_unused`, so a race regression can
    /// replay the exact adoption interleaving against the classifier.
    #[cfg(test)]
    classification_gate: Option<Arc<crate::runtime::conversation_runtime::Gate>>,
    /// The post-commit summary invalidation log every display-projection
    /// publication records into, shared by every clone of this catalog (read
    /// snapshots included) so there is exactly one log per product root.
    summary_invalidations: Arc<SessionSummaryInvalidations>,
}

impl SessionCatalog {
    pub(crate) fn is_published(&self) -> bool {
        self.published
    }
    pub(crate) fn empty(
        controller: &crate::runtime::local_storage::ProductController,
    ) -> Result<Self, SessionError> {
        let root = controller.root().join("sessions");
        fs::create_dir_all(&root).map_err(|e| SessionError::Catalog {
            detail: e.to_string(),
        })?;
        let mut catalog = Self {
            identities: Arc::new(crate::runtime::identity::SystemUuidV7Generator),
            product: (**controller).clone(),
            path: root.join("catalog.json"),
            root,
            node_observation_index: Arc::default(),
            document: CatalogDocument {
                retired: RetiredIdentities::default(),
                generation: 0,
                deletions: BTreeMap::new(),
                upload_preparations: BTreeMap::new(),
                schema_version: SESSION_CATALOG_SCHEMA_VERSION,
                next_session_ordinal: 1,
                next_node_ordinal: 1,
                sessions: BTreeMap::new(),
            },
            published: false,
            lifecycle: None,
            #[cfg(test)]
            write_fault: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            preparation_cleanup_fault: Arc::default(),
            #[cfg(test)]
            classification_gate: None,
            summary_invalidations: Arc::default(),
        };
        catalog.commit(catalog.document.clone())?;
        Ok(catalog)
    }
    pub(crate) fn controller(
        &self,
    ) -> Result<std::sync::Arc<crate::runtime::local_storage::ProductController>, SessionError>
    {
        self.lifecycle.clone().ok_or_else(|| SessionError::Catalog {
            detail: "storage recovery requires ProductController".into(),
        })
    }
    pub(crate) fn plan_schedule_read(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
        catalog: Arc<tokio::sync::Mutex<Self>>,
    ) -> Result<super::session_controller::ScheduleReadPlan, SessionError> {
        if self.document.deletions.contains_key(id) {
            return Err(SessionError::DeletingSession {
                session_id: id.clone(),
            });
        }
        let node = self
            .document
            .sessions
            .get(id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: id.clone(),
            })?
            .nodes
            .get(node)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: id.clone(),
                node_id: node.clone(),
            })?;
        Ok(super::session_controller::ScheduleReadPlan {
            catalog,
            session: id.clone(),
            node: node.id.clone(),
            conversation_id: node.conversation_id.clone(),
            database_path: self.database_path(id, &node.conversation_id),
            controller: self.controller()?,
        })
    }
    pub(crate) fn validate_schedule_read(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
        conversation: &ConversationId,
    ) -> Result<(), SessionError> {
        if self.document.deletions.contains_key(id) {
            return Err(SessionError::DeletingSession {
                session_id: id.clone(),
            });
        }
        let bound = self
            .document
            .sessions
            .get(id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: id.clone(),
            })?
            .nodes
            .get(node)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: id.clone(),
                node_id: node.clone(),
            })?;
        if &bound.conversation_id != conversation {
            return Err(SessionError::Catalog {
                detail: "scheduled read destination binding changed".into(),
            });
        }
        Ok(())
    }
    pub(crate) fn acquire_read(
        &self,
        id: &SessionId,
        selected: Option<&SessionNodeId>,
    ) -> Result<super::session_controller::SessionReadAccess, SessionError> {
        if self.document.deletions.contains_key(id) {
            return Err(SessionError::DeletingSession {
                session_id: id.clone(),
            });
        }
        let session =
            self.document
                .sessions
                .get(id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: id.clone(),
                })?;
        let node_id = selected.unwrap_or(&session.active_node);
        let node = session
            .nodes
            .get(node_id)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: id.clone(),
                node_id: node_id.clone(),
            })?;
        let path = self.database_path(id, &node.conversation_id);
        let allocation =
            crate::runtime::local_storage::ConversationAccess::existing_for_controller(
                self.controller()?,
                path.parent().expect("allocation"),
            )
            .map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })?;
        Ok(super::session_controller::SessionReadAccess {
            conversation_id: node.conversation_id.clone(),
            database_path: path,
            allocation: Arc::new(allocation),
        })
    }
    pub(crate) fn read_origin(
        &self,
        id: &SessionId,
        node_id: &SessionNodeId,
    ) -> Result<super::session_controller::SessionReadOrigin, SessionError> {
        if self.document.deletions.contains_key(id) {
            return Err(SessionError::DeletingSession {
                session_id: id.clone(),
            });
        }
        let node = self
            .document
            .sessions
            .get(id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: id.clone(),
            })?
            .nodes
            .get(node_id)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: id.clone(),
                node_id: node_id.clone(),
            })?;
        let SessionNodeOrigin::Fork {
            source_session,
            source_node,
            source_surface_revision,
            source_locator,
            side,
            ..
        } = &node.origin
        else {
            return Err(SessionError::Catalog {
                detail: "Node has no immutable fork origin".into(),
            });
        };
        if !matches!(source_locator, crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position } if *position > 0)
        {
            return Err(SessionError::Catalog {
                detail: "Fork origin requires a finite native ledger position".into(),
            });
        }
        Ok(super::session_controller::SessionReadOrigin {
            source_session: source_session.clone(),
            source_node: source_node.clone(),
            source_surface_revision: *source_surface_revision,
            source_locator: source_locator.clone(),
            side: *side,
        })
    }
    pub(crate) fn acquire_session(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
    ) -> Result<super::session_controller::SessionAccess, SessionError> {
        let (node, settings) = self.lineage(id, node)?;
        let path = self.database_path(id, &node.conversation_id);
        let controller = self
            .lifecycle
            .clone()
            .ok_or_else(|| SessionError::Catalog {
                detail: "allocation admission requires retained ProductController".into(),
            })?;
        let allocation =
            crate::runtime::local_storage::ConversationAccess::existing_for_controller(
                controller,
                path.parent().expect("allocation"),
            )
            .map_err(|e| SessionError::Catalog {
                detail: e.to_string(),
            })?;
        Ok(super::session_controller::SessionAccess {
            session: self.snapshot(id)?,
            settings_revision: self.settings_revision(id)?,
            database_path: path,
            node,
            settings,
            allocation: std::sync::Arc::new(allocation),
        })
    }
    pub(crate) fn inspect_deletion(
        &self,
        id: &SessionId,
    ) -> std::io::Result<super::session_deletion::DeletionTargetSnapshot> {
        super::session_deletion::DeletionTargetSnapshot::inspect(self.product.root(), id)
    }

    fn inspect_store(
        &self,
        id: ConversationId,
        path: &Path,
    ) -> Result<SqliteConversationStore, ConversationStoreError> {
        use crate::runtime::local_storage::ConversationAccess;
        self.product
            .confined(path)
            .map_err(|e| ConversationStoreError::Storage(e.to_string()))?;
        let access =
            ConversationAccess::existing(&self.product, path.parent().expect("allocation"))
                .map_err(|e| ConversationStoreError::Storage(e.to_string()))?;
        SqliteConversationStore::open_existing(id, path)
            .map(|store| store.with_lifecycle(std::sync::Arc::new(access)))
    }

    /// Recover graph-owned stores before startup performs read-only selection.
    /// Child references come only from existing typed durable ownership commits.
    pub(crate) fn recover_session_storage(
        &self,
        session_id: &SessionId,
        guard: &std::sync::Arc<crate::runtime::local_storage::ProductController>,
    ) -> Result<(), SessionError> {
        let record =
            self.document
                .sessions
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let mut pending: Vec<_> = std::iter::once((session_id, record))
            .flat_map(|(session, record)| {
                record.nodes.values().map(|node| {
                    (
                        node.conversation_id.clone(),
                        self.database_path(session, &node.conversation_id),
                    )
                })
            })
            .collect();
        let mut seen = BTreeSet::new();
        while let Some((id, path)) = pending.pop() {
            validate_id(id.as_str(), "conversation")?;
            if !seen.insert(id.clone()) {
                return Err(SessionError::Catalog {
                    detail: "ambiguous or cyclic durable ownership".into(),
                });
            }
            let store = SqliteConversationStore::recover_existing(id, &path, guard)
                .map_err(SessionError::Store)?;
            let mut cursor = None;
            loop {
                let page = store
                    .read_events(cursor, 256)
                    .map_err(SessionError::Store)?;
                if page.events.is_empty() {
                    break;
                }
                cursor = page.next_sequence;
                for event in page.events {
                    if let crate::events::types::RuntimeEvent::SubagentOwnershipCommitted {
                        child_conversation_id,
                        ..
                    } = event.event
                    {
                        validate_id(child_conversation_id.as_str(), "child conversation")?;
                        let path = crate::runtime::subagent::child_conversation_store_path(
                            guard.root(),
                            session_id,
                            &child_conversation_id,
                        );
                        pending.push((child_conversation_id, path));
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn ownership_nodes(&self) -> BTreeMap<SessionId, Vec<SessionNode>> {
        self.document
            .sessions
            .iter()
            .map(|(id, session)| (id.clone(), session.nodes.values().cloned().collect()))
            .collect()
    }

    pub(crate) fn retain_lifecycle(
        &mut self,
        guard: std::sync::Arc<crate::runtime::local_storage::ProductController>,
    ) {
        self.lifecycle = Some(guard);
    }

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
        if !runtime_root.exists() {
            return Ok(None);
        }
        let root = crate::runtime::local_storage::ProductRoot::existing(runtime_root).map_err(
            |error| SessionError::Io {
                path: runtime_root.to_path_buf(),
                detail: error.to_string(),
            },
        )?;
        Self::read_under_guard(&root)
    }

    pub(crate) fn read_under_guard(
        guard: &crate::runtime::local_storage::ProductRoot,
    ) -> Result<Option<Self>, SessionError> {
        let root = guard.root().join("sessions");
        let path = guard
            .confined(&root.join("catalog.json"))
            .map_err(|error| SessionError::Io {
                path: root.clone(),
                detail: error.to_string(),
            })?;
        if !path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(&path).map_err(|error| SessionError::Io {
            path: path.clone(),
            detail: error.to_string(),
        })?;
        let envelope: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })?;
        if envelope
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(SESSION_CATALOG_SCHEMA_VERSION))
        {
            return Err(SessionError::Catalog { detail: "unsupported session catalog schema; CFG3 requires UUIDv7 identities and Session-owned intent; old development schemas are refused".into() });
        }
        let document: CatalogDocument =
            serde_json::from_slice(&bytes).map_err(|error| SessionError::Catalog {
                detail: format!("cannot decode {}: {error}", path.display()),
            })?;
        validate_document(&document)?;
        Ok(Some(Self {
            identities: Arc::new(crate::runtime::identity::SystemUuidV7Generator),
            product: guard.clone(),
            root,
            path,
            node_observation_index: Arc::new(observation::node_index(&document)),
            document,
            published: true,
            lifecycle: None,
            #[cfg(test)]
            write_fault: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            preparation_cleanup_fault: Arc::default(),
            #[cfg(test)]
            classification_gate: None,
            summary_invalidations: Arc::default(),
        }))
    }
}

/// A native Session-domain failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The Session was logically deleted and cleanup is still pending.
    DeletingSession { session_id: SessionId },
    /// A concurrent settings publication won the compare-and-swap.
    StaleSettings { expected: u64, actual: u64 },
    /// A storage operation failed.
    Io { path: PathBuf, detail: String },
    /// The catalog publication outcome is explicit: either visibility was not
    /// crossed, or it was crossed but the final durability barrier was not
    /// proven.
    CatalogCommit { error: CatalogCommitError },
    /// Private cleanup failed; retain both the operation and cleanup diagnostics.
    PreparationCleanupPending {
        operation: Box<SessionError>,
        cleanup: Box<SessionError>,
    },
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
            Self::StaleSettings { expected, actual } => write!(
                f,
                "stale Session settings revision {expected}; current is {actual}"
            ),
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
            Self::PreparationCleanupPending { operation, cleanup } => write!(
                f,
                "{operation}; private preparation cleanup pending: {cleanup}"
            ),
            Self::Catalog { detail } => write!(f, "session catalog: {detail}"),
            Self::Store(error) => write!(f, "conversation seed: {error}"),
            Self::Seed { detail } => write!(f, "conversation seed: {detail}"),
            Self::DeletingSession { session_id } => {
                write!(f, "Session {session_id} is deleted with cleanup pending")
            }
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
pub(crate) mod tests;
