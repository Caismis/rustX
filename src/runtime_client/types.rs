//! Runtime Client Protocol: the transport-neutral protocol contract.
//!
//! This module owns the explicit external protocol boundary of Issue #37.
//! It is deliberately **not** the internal runtime fact vocabulary
//! ([`RuntimeEvent`](crate::events::types::RuntimeEvent)) and not the
//! compiled manifest protocol (`crate::protocol`): the Runtime Client
//! projection defines its own external shapes, its own versioning, its own
//! cursor domain, and its own error model. Internal `RuntimeEvent` schema
//! evolution can therefore never break the Runtime Client wire contract by
//! construction.
//!
//! # Protocol version
//!
//! [`RUNTIME_CLIENT_PROTOCOL_VERSION`] is independent from
//! [`EVENT_SCHEMA_VERSION`](crate::events::types::EVENT_SCHEMA_VERSION),
//! [`MANIFEST_SCHEMA_VERSION`](crate::protocol::manifest::MANIFEST_SCHEMA_VERSION),
//! the crate version, and any future Event Journal schema. Attachment
//! initialization performs explicit version negotiation; versions other than
//! [`RUNTIME_CLIENT_PROTOCOL_VERSION`] are rejected explicitly.
//!
//! # Envelope
//!
//! The envelope is a transport-neutral JSON-RPC-style message model:
//!
//! ```text
//! request(id, method + typed params)
//! response(id, result | error)
//! event(cursor + RuntimeClientEvent payload) — no request id exists
//! ```
//!
//! Request ids are scoped to exactly one attachment. Notifications never
//! fabricate request ids: [`RuntimeClientProtocolEvent`] structurally has
//! no `id` field. Every concrete protocol method is client-initiated; the
//! envelope remains structurally capable of peer-initiated requests in a
//! later protocol version.
//!
//! No transport lives here: JSONL/stdio framing is owned by Issue #38 and
//! any WebSocket transport by Issue #36; both consume this semantic layer
//! without redefining it.
//!
//! Native Approval responses are deliberately finite and provider-neutral.
//! They contain no replacement `ToolCall` identity or argument channel; the
//! owning Agent Loop resumes the original prepared invocation only after its
//! own cancellation/start frontier permits it.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::event::RuntimeClientEvent;
use super::snapshot::{
    CapabilityView, RuntimeClientContextView, RuntimeClientSnapshot, RuntimeClientTranscriptCursor,
    RuntimeClientTranscriptPage,
};
use crate::conversation::SurfaceRevision;
use crate::message::types::UserContentBlock;
use crate::model::catalog::ModelCatalogView;
use crate::model::session::{SessionModelConfig, SessionModelView};
use crate::runtime::identity::{
    AgentId, AttemptId, CapabilityRevision, ConversationId, MessageId, ToolExecutionId,
};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::interaction::{InteractionRef, InteractionResponse};
use crate::runtime::types::ApprovalMode;

/// The protocol view of one native Session graph node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionNodeView {
    /// Node identity.
    pub id: String,
    /// Parent node in the same Session graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The independent linear `ConversationId` of this node.
    pub conversation_id: ConversationId,
    /// Product-level origin metadata.
    pub origin: SessionNodeOriginView,
}

/// The protocol view of a `SessionNode` origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionNodeOriginView {
    /// A new empty lineage.
    New,
    /// A clone selected at one exact source revision.
    Clone {
        /// Source Session identity.
        source_session: String,
        /// Source node identity.
        source_node: String,
        /// Source revision selected for the seed.
        source_surface_revision: SurfaceRevision,
    },
    /// A fork selected immediately before one source user message.
    Fork {
        /// Source Session identity.
        source_session: String,
        /// Source node identity.
        source_node: String,
        /// Source revision selected for the seed.
        source_surface_revision: SurfaceRevision,
        /// Selected source user message.
        source_user_message: MessageId,
    },
}

/// The bounded authoritative Runtime Client metadata view of one Session.
///
/// Graph nodes are intentionally returned only through the paged
/// `session_tree_get` projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    /// Session identity.
    pub id: String,
    /// The user-chosen display name, absent until this Session is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Creation instant.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last metadata/active-node publication instant.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Active node identity.
    pub active_node: String,
    /// Conversation identity owned by the active node.
    pub active_conversation_id: ConversationId,
    /// Number of nodes in the Session graph, without embedding the graph.
    pub node_count: usize,
}

/// One bounded row in the `/resume` selector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummaryView {
    /// Session identity.
    pub id: String,
    /// The user-chosen display name, absent until this Session is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The bounded first user message of this Session, which is what an
    /// unnamed row shows instead of a name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Last metadata/active-node publication instant.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Active node identity.
    pub active_node: String,
    /// Whether this is the currently active Session.
    pub active: bool,
}

/// One historical user-message boundary exposed by `/fork` and `/tree`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUserMessageBoundaryView {
    /// Exact source Surface revision.
    pub surface_revision: SurfaceRevision,
    /// Canonical source user message to restore into the editor if selected.
    pub message: crate::message::types::UserMessageBlock,
}

/// Native Session control intent carried from the Runtime Client boundary to
/// the Rust-owned `LocalSessionSupervisor`.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeClientSessionRequest {
    /// Read one bounded, searchable persisted-session page.
    List {
        /// Optional case-insensitive query over Session id/name.
        query: Option<String>,
        /// Number of matching rows already consumed.
        offset: usize,
        /// Requested page size, bounded by the native owner.
        limit: usize,
    },
    /// Read the active Session metadata.
    Get,
    /// Read one bounded active Session graph/history page.
    Tree {
        /// Number of graph nodes already consumed.
        node_offset: usize,
        /// Number of historical boundaries already consumed.
        history_offset: usize,
        /// Requested page size for both projections.
        limit: usize,
    },
    /// Change metadata only.
    Name(String),
    /// Create a new empty Session.
    New,
    /// Select an existing Session/node.
    Select {
        /// Session identity.
        session_id: String,
        /// Optional node; absent selects the Session's active node.
        node_id: Option<String>,
    },
    /// Clone the committed current Surface head.
    Clone,
    /// Fork at an exact historical user boundary into a new Session.
    Fork {
        /// Exact source revision.
        surface_revision: SurfaceRevision,
        /// Source user-message identity.
        message_id: MessageId,
    },
    /// Create a new node inside the active Session at a historical boundary.
    TreeBranch {
        /// Exact source revision.
        surface_revision: SurfaceRevision,
        /// Source user-message identity.
        message_id: MessageId,
    },
}

/// The current Runtime Client Protocol version.
///
/// Deliberately independent from the internal event schema version, the
/// manifest schema version, and the crate version: representing or changing
/// this version never implies anything about internal schemas.
///
/// Version 7 carries Issue #140's typed compaction-summary metadata: the
/// canonical `InboundKind::CompactionSummary` message kind now transports
/// its validated cumulative file-operation metadata, and clients mirror the
/// payload shape.
///
/// Version 8 carries Issue #144's named-subagent projection: a subagent is
/// identified by `(agent, definition_digest)` instead of a profile name.
/// There is no compatibility decoding of the obsolete shape.
///
/// Version 9 carries Issue #158's `Interrupted` subagent terminal state. The
/// closed Runtime Client lifecycle vocabulary therefore includes an unknown
/// child process/control-plane outcome without relabelling it as a semantic
/// model failure. There is no compatibility decoding of version 8.
///
/// Version 10 carries Issue #146's bounded subagent workspace facts and
/// preserved-worktree handoff metadata. There is no compatibility decoding
/// of version 9.
///
/// Version 11 carries Issue #178's subagent live-activity projection: the
/// subagent view gains the latest-value `observation`, the redacted
/// `execution_profile`, and `started_at`, and its `detail` is now
/// diagnostics-only (a successful child's answer rides only the durable
/// terminal inbound publication). Version 12 carries the shared routed
/// interaction projection: pending interactions are addressed by
/// `InteractionRef` and include source metadata for primary and live child
/// conversations. There is no compatibility decoding of version 11.
///
/// Version 13 carries Issue #187's subagent workspace authority
/// representation: the logical child project workspace is separated from
/// the physical Git worktree ownership facts. The subagent workspace
/// projection is now `logical_workspace` plus a tagged `isolation`
/// (`shared` or `git_worktree` with the source repository root, the
/// repository-relative workspace, the physical worktree root, the base
/// commit, the branch, and the parent dirty fact), and a retained handoff
/// exposes `logical_workspace` and `physical_worktree_root` alongside the
/// branch/base/head/dirty facts. There is no compatibility decoding of
/// version 12.
///
/// Version 14 carries Issue #194's Agent Status contextual annotation
/// projection. The snapshot's latest-only `status` is replaced by the bounded
/// composition window `statuses`; each status opportunity carries the durable
/// identity it was established against (`FreshInbound` the inbound message,
/// `PostToolBatch` its settled tool batch's `transcript_anchor`); and
/// `AgentStatusComposed` carries one complete window transition — the
/// admitted composition plus the `evicted_status_message_id` that admission
/// caused. Together they let a client place every composed status
/// deterministically and reconstruct the same window and placement from a
/// snapshot that it folded from live events, without ever owning retention or
/// inferring a position. There is no compatibility decoding of version 13.
///
/// Version 15 carries the retained-workspace disposal operation and its typed
/// resource lifecycle, including unresolved-preservation projection and the
/// pending partial-settlement outcome. There is no compatibility decoding of
/// version 14.
///
/// Version 16 carries Issue #202's explicit tool outcome certainty: the
/// canonical `ToolExecutionStatus` replaces `interrupted` with
/// `outcome_unknown` (carrying a bounded producer-owned `detail`), and the
/// background terminal state `interrupted` becomes `outcome_unknown`.
/// `timed_out` now means the deadline expired *and* terminal settlement was
/// proven, and every non-success status carries model-facing feedback. There
/// is no compatibility decoding of version 15.
pub const RUNTIME_CLIENT_PROTOCOL_VERSION: u16 = 16;

/// The external cursor of the Runtime Client observation stream.
///
/// A cursor identifies one position in the externally visible
/// `RuntimeClientEvent` sequence: cursor `C` means "every Runtime Client
/// event through event `C` has been applied to the snapshot". It is:
///
/// - monotonic within the Runtime Client observation stream;
/// - **not** an alias of `u64`;
/// - **not** the mailbox [`InboundSequence`](crate::runtime::inbound::InboundSequence);
/// - **not** [`RuntimeEventEnvelope::sequence`](crate::events::types::RuntimeEventEnvelope);
/// - **not** any future Event Journal sequence;
/// - owned by the runtime observation stream, so it survives attachment
///   detach/reconnect;
/// - independent of its numeric representation: protocol versioning never
///   derives anything from cursor encoding.
///
/// Cursor allocation is committed by exactly one linearization owner (the
/// Runtime Client projection) together with event publication; overflow
/// fails explicitly and never wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeClientCursor(u64);

impl RuntimeClientCursor {
    /// Creates a cursor from a raw value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw cursor value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RuntimeClientCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The identity of one Runtime Client attachment.
///
/// Distinct from [`ConversationId`], [`AttemptId`], [`RuntimeClientCursor`],
/// and request ids: one attachment is one client session, and reconnecting
/// always receives a new attachment identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttachmentId(String);

impl AttachmentId {
    /// Creates an attachment identity from a string value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The attachment identity as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AttachmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A request id scoped to exactly one attachment.
///
/// Request ids do not carry across attachments: a fresh attachment starts
/// a fresh request-id scope. The id is opaque correlation data chosen by
/// the client; the runtime never interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates a request id from a raw value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw request id value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One client-initiated Runtime Client protocol request.
///
/// The `method` tag is the stable protocol discriminator; every method
/// carries its typed params. Unknown fields and unknown methods are
/// rejected on deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientRequest {
    /// Negotiate protocol version and admit the attachment.
    Initialize {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The Runtime Client Protocol version the client speaks.
        protocol_version: u16,
    },
    /// Submit one inbound user message.
    ///
    /// The runtime owns authoritative metadata (message identity, inbound
    /// sequencing, timestamp, provenance). A successful response means the
    /// message was accepted/admitted — never that the assistant finished.
    SubmitInbound {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The inbound content blocks.
        content: Vec<UserContentBlock>,
    },
    /// Request cancellation of the current attempt.
    ///
    /// Acceptance is not terminal settlement: actual settlement is owned by
    /// the Agent Loop and observed asynchronously through Runtime Client
    /// events.
    CancelCurrentAttempt {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Manually compact the current canonical context while the runtime is
    /// idle. The response awaits the runtime-owned maintenance operation's
    /// terminal result.
    CompactContext {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Atomically reload the runtime-owned resource/capability generation.
    ReloadResources {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Answer one live native interaction through the runtime-owned
    /// coordinator. The response has no tool-argument replacement channel.
    InteractionRespond {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The full routed interaction identity.
        interaction: InteractionRef,
        /// The finite typed response.
        response: InteractionResponse,
    },
    /// Read the authoritative snapshot and its cursor.
    SnapshotGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read one bounded durable transcript page. `before_cursor` is the
    /// exclusive boundary for older history; omitting it returns the newest
    /// page. To continue walking backward, pass the previous response's
    /// `next_cursor` unchanged. This read does not advance the Runtime Client
    /// observation cursor.
    TranscriptPageGet {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The exclusive older-history boundary returned by the prior page's
        /// `next_cursor`, when walking toward older history.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_cursor: Option<RuntimeClientTranscriptCursor>,
        /// Requested page size.
        limit: usize,
    },
    /// Subscribe to Runtime Client events after a cursor.
    ///
    /// A successful subscription observes every subsequently published
    /// event after `after_cursor`; an unserviceable cursor fails explicitly
    /// with `resync_required` (the stream never silently jumps).
    SubscribeEvents {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The last cursor the client already observed.
        after_cursor: RuntimeClientCursor,
    },
    /// Inspect the active capability projection.
    CapabilityGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read the safe public model catalog: which models and reasoning
    /// profiles this runtime can select.
    ///
    /// This exists so a client never reads `models.jsonc` itself. The result
    /// carries no credential, no adapter internal, and no compat
    /// implementation object.
    ModelCatalogGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read the authoritative session model state.
    ModelGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Replace the authoritative session model configuration.
    ///
    /// This is a whole-state replacement, never a patch. Validation is
    /// transactional: an invalid update changes nothing and publishes
    /// nothing. A valid update may occur while an attempt is running; it
    /// affects **future admissions only**, and the running attempt keeps the
    /// model it froze at its own admission.
    ModelSet {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The complete desired session model configuration.
        config: Box<SessionModelConfig>,
    },
    /// Request a runtime `ApprovalMode` transition.
    ApprovalModeSet {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The latest desired runtime approval mode.
        mode: ApprovalMode,
    },
    /// List persisted native Sessions for `/resume`.
    SessionList {
        /// Attachment-scoped request id.
        id: RequestId,
        /// Optional case-insensitive Session id/name query.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        /// Number of matching rows already consumed.
        offset: usize,
        /// Requested bounded page size.
        limit: usize,
    },
    /// Read the active native Session metadata for `/session`.
    SessionGet {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Read the active Session graph and historical branch boundaries.
    SessionTreeGet {
        /// Attachment-scoped request id.
        id: RequestId,
        /// Number of graph nodes already consumed.
        node_offset: usize,
        /// Number of historical boundaries already consumed.
        history_offset: usize,
        /// Requested bounded page size.
        limit: usize,
    },
    /// Name the active Session. Metadata only: no conversation, lineage, or
    /// identity is touched, and no Session is ever resolved by its name.
    SessionName {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The new bounded single-line display name.
        name: String,
    },
    /// Create and activate a new empty Session.
    SessionNew {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Select and activate an existing Session/node.
    SessionSelect {
        /// Attachment-scoped request id.
        id: RequestId,
        /// Session identity.
        session_id: String,
        /// Optional node; absent means that Session's active node.
        node_id: Option<String>,
    },
    /// Clone the exact current committed canonical Surface head.
    SessionClone {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Fork at an exact historical user-message boundary.
    SessionFork {
        /// Attachment-scoped request id.
        id: RequestId,
        /// Exact source Surface revision.
        surface_revision: SurfaceRevision,
        /// Source user-message identity.
        message_id: MessageId,
    },
    /// Create a new node in the active Session at an exact historical
    /// user-message boundary.
    SessionTreeBranch {
        /// Attachment-scoped request id.
        id: RequestId,
        /// Exact source Surface revision.
        surface_revision: SurfaceRevision,
        /// Source user-message identity.
        message_id: MessageId,
    },
    /// Inspect one background execution.
    BackgroundStatus {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The detached execution identity.
        execution_id: ToolExecutionId,
    },
    /// Request cancellation of one background execution.
    ///
    /// Acceptance and eventual settlement remain distinct: the response
    /// carries the registry snapshot after the request, never the terminal
    /// result.
    BackgroundCancel {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The detached execution identity.
        execution_id: ToolExecutionId,
    },
    /// Inspect one subagent child (Issue #60).
    SubagentStatus {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The subagent identity.
        subagent_id: crate::runtime::identity::SubagentId,
    },
    /// Request cancellation of one subagent child (Issue #60).
    ///
    /// Acceptance and eventual settlement remain distinct: the response
    /// carries the registry snapshot after the intent commit, never the
    /// terminal result.
    SubagentCancel {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The subagent identity.
        subagent_id: crate::runtime::identity::SubagentId,
    },
    /// Dispose the exact retained workspace owned by one terminal subagent.
    /// The request names only the authoritative subagent identity; it never
    /// carries a filesystem path or Git ref.
    SubagentWorkspaceDispose {
        /// Attachment-scoped request id.
        id: RequestId,
        /// The terminal subagent whose retained resource is being disposed.
        subagent_id: crate::runtime::identity::SubagentId,
    },
    /// Release the attachment.
    Detach {
        /// Attachment-scoped request id.
        id: RequestId,
    },
    /// Request local-runtime shutdown.
    ///
    /// Shutdown is not detach: it closes new semantic admission, requests
    /// runtime-owned cancellation, and resolves only after the conversation
    /// runtime reaches quiescence.
    Shutdown {
        /// Attachment-scoped request id.
        id: RequestId,
    },
}

impl RuntimeClientRequest {
    /// The attachment-scoped request id of the request.
    #[must_use]
    pub fn id(&self) -> RequestId {
        match self {
            Self::Initialize { id, .. }
            | Self::SubmitInbound { id, .. }
            | Self::CancelCurrentAttempt { id, .. }
            | Self::CompactContext { id, .. }
            | Self::ReloadResources { id, .. }
            | Self::InteractionRespond { id, .. }
            | Self::SnapshotGet { id, .. }
            | Self::TranscriptPageGet { id, .. }
            | Self::SubscribeEvents { id, .. }
            | Self::CapabilityGet { id, .. }
            | Self::ModelCatalogGet { id, .. }
            | Self::ModelGet { id, .. }
            | Self::ModelSet { id, .. }
            | Self::ApprovalModeSet { id, .. }
            | Self::SessionList { id, .. }
            | Self::SessionGet { id, .. }
            | Self::SessionTreeGet { id, .. }
            | Self::SessionName { id, .. }
            | Self::SessionNew { id, .. }
            | Self::SessionSelect { id, .. }
            | Self::SessionClone { id, .. }
            | Self::SessionFork { id, .. }
            | Self::SessionTreeBranch { id, .. }
            | Self::BackgroundStatus { id, .. }
            | Self::BackgroundCancel { id, .. }
            | Self::SubagentStatus { id, .. }
            | Self::SubagentCancel { id, .. }
            | Self::SubagentWorkspaceDispose { id, .. }
            | Self::Detach { id, .. }
            | Self::Shutdown { id, .. } => *id,
        }
    }

    /// The stable method discriminator.
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Initialize { .. } => "initialize",
            Self::SubmitInbound { .. } => "submit_inbound",
            Self::CancelCurrentAttempt { .. } => "cancel_current_attempt",
            Self::CompactContext { .. } => "compact_context",
            Self::ReloadResources { .. } => "reload_resources",
            Self::InteractionRespond { .. } => "interaction_respond",
            Self::SnapshotGet { .. } => "snapshot_get",
            Self::TranscriptPageGet { .. } => "transcript_page_get",
            Self::SubscribeEvents { .. } => "subscribe_events",
            Self::CapabilityGet { .. } => "capability_get",
            Self::ModelCatalogGet { .. } => "model_catalog_get",
            Self::ModelGet { .. } => "model_get",
            Self::ModelSet { .. } => "model_set",
            Self::ApprovalModeSet { .. } => "approval_mode_set",
            Self::SessionList { .. } => "session_list",
            Self::SessionGet { .. } => "session_get",
            Self::SessionTreeGet { .. } => "session_tree_get",
            Self::SessionName { .. } => "session_name",
            Self::SessionNew { .. } => "session_new",
            Self::SessionSelect { .. } => "session_select",
            Self::SessionClone { .. } => "session_clone",
            Self::SessionFork { .. } => "session_fork",
            Self::SessionTreeBranch { .. } => "session_tree_branch",
            Self::BackgroundStatus { .. } => "background_status",
            Self::BackgroundCancel { .. } => "background_cancel",
            Self::SubagentStatus { .. } => "subagent_status",
            Self::SubagentCancel { .. } => "subagent_cancel",
            Self::SubagentWorkspaceDispose { .. } => "subagent_workspace_dispose",
            Self::Detach { .. } => "detach",
            Self::Shutdown { .. } => "shutdown",
        }
    }

    /// Whether this request crosses the native Session supervisor boundary
    /// and therefore must use the async semantic endpoint.
    #[must_use]
    pub fn is_session_request(&self) -> bool {
        matches!(
            self,
            Self::SessionList { .. }
                | Self::SessionGet { .. }
                | Self::SessionTreeGet { .. }
                | Self::SessionName { .. }
                | Self::SessionNew { .. }
                | Self::SessionSelect { .. }
                | Self::SessionClone { .. }
                | Self::SessionFork { .. }
                | Self::SessionTreeBranch { .. }
        )
    }

    /// Whether this request must run through the async semantic endpoint.
    #[must_use]
    pub fn requires_async(&self) -> bool {
        matches!(
            self,
            Self::CompactContext { .. }
                | Self::ReloadResources { .. }
                | Self::InteractionRespond { .. }
                | Self::SubagentWorkspaceDispose { .. }
                | Self::Shutdown { .. }
        ) || self.is_session_request()
    }

    /// Whether this request changes conversation, runtime, Session, or
    /// lifecycle authority. Read-only inspection attachments reject these
    /// requests before dispatch; protocol reads and `detach` remain allowed.
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            Self::SubmitInbound { .. }
                | Self::CancelCurrentAttempt { .. }
                | Self::CompactContext { .. }
                | Self::ReloadResources { .. }
                | Self::InteractionRespond { .. }
                | Self::ModelSet { .. }
                | Self::ApprovalModeSet { .. }
                | Self::SessionName { .. }
                | Self::SessionNew { .. }
                | Self::SessionSelect { .. }
                | Self::SessionClone { .. }
                | Self::SessionFork { .. }
                | Self::SessionTreeBranch { .. }
                | Self::BackgroundCancel { .. }
                | Self::SubagentCancel { .. }
                | Self::SubagentWorkspaceDispose { .. }
                | Self::Shutdown { .. }
        )
    }

    /// Converts the wire request into the typed native Session control
    /// intent. The request id is intentionally absent from the intent: the
    /// Runtime Client endpoint remains the sole correlation owner.
    #[must_use]
    pub fn session_request(&self) -> Option<RuntimeClientSessionRequest> {
        match self {
            Self::SessionList {
                query,
                offset,
                limit,
                ..
            } => Some(RuntimeClientSessionRequest::List {
                query: query.clone(),
                offset: *offset,
                limit: *limit,
            }),
            Self::SessionGet { .. } => Some(RuntimeClientSessionRequest::Get),
            Self::SessionTreeGet {
                node_offset,
                history_offset,
                limit,
                ..
            } => Some(RuntimeClientSessionRequest::Tree {
                node_offset: *node_offset,
                history_offset: *history_offset,
                limit: *limit,
            }),
            Self::SessionName { name, .. } => Some(RuntimeClientSessionRequest::Name(name.clone())),
            Self::SessionNew { .. } => Some(RuntimeClientSessionRequest::New),
            Self::SessionSelect {
                session_id,
                node_id,
                ..
            } => Some(RuntimeClientSessionRequest::Select {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
            }),
            Self::SessionClone { .. } => Some(RuntimeClientSessionRequest::Clone),
            Self::SessionFork {
                surface_revision,
                message_id,
                ..
            } => Some(RuntimeClientSessionRequest::Fork {
                surface_revision: *surface_revision,
                message_id: message_id.clone(),
            }),
            Self::SessionTreeBranch {
                surface_revision,
                message_id,
                ..
            } => Some(RuntimeClientSessionRequest::TreeBranch {
                surface_revision: *surface_revision,
                message_id: message_id.clone(),
            }),
            _ => None,
        }
    }
}

/// One Runtime Client Protocol response.
///
/// Exactly one of `result`/`error` is present. The request id is echoed
/// from the corresponding request, so responses correlate correctly even
/// under request pipelining.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientResponse {
    /// The echoed attachment-scoped request id.
    pub id: RequestId,
    /// The method result, when the request succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<RuntimeClientResult>,
    /// The typed protocol error, when the request failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RuntimeClientError>,
}

/// The public result of disposing a retained subagent workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeClientSubagentWorkspaceDisposalOutcome {
    /// The retained physical worktree and its exact runtime branch were
    /// removed by this request.
    Disposed,
    /// The same child resource was disposed by an earlier request. No
    /// physical deletion is attempted for this result.
    AlreadyDisposed,
    /// The exact worktree was removed, but branch compare-delete settlement
    /// remains pending and the identity-based request may be retried.
    DisposalPending,
    /// The child has no retained isolated resource to dispose.
    NoRetainedWorkspace,
}

/// The typed success payloads of the Runtime Client protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientResult {
    /// `initialize` succeeded: the attachment is admitted and the initial
    /// authoritative snapshot (linearized with its cursor) is returned.
    Initialized {
        /// The fresh attachment identity.
        attachment_id: AttachmentId,
        /// The conversation this runtime serves.
        conversation_id: ConversationId,
        /// The agent executed by attempts of this runtime.
        agent_id: AgentId,
        /// The authoritative snapshot at the returned cursor.
        snapshot: RuntimeClientSnapshot,
        /// The cursor the snapshot is linearized at.
        cursor: RuntimeClientCursor,
    },
    /// `submit_inbound` succeeded: the message was accepted/admitted.
    InboundAccepted {
        /// The runtime-assigned canonical message identity.
        message_id: MessageId,
        /// The mailbox-assigned inbound sequence.
        inbound_sequence: InboundSequence,
    },
    /// `cancel_current_attempt` succeeded: cancellation was requested. This
    /// is acceptance, never terminal settlement.
    AttemptCancellationAccepted {
        /// The attempt cancellation was requested for.
        attempt_id: AttemptId,
    },
    /// `compact_context` succeeded after the durable summary/Surface commit.
    ContextCompacted {
        /// The authoritative context projection after the commit.
        context: RuntimeClientContextView,
    },
    /// `reload_resources` published a complete new generation.
    ResourcesReloaded {
        /// Published process-local resource generation.
        resource_revision: u64,
        /// Compatible published capability generation.
        capability_revision: CapabilityRevision,
    },
    /// `interaction_respond` succeeded: the coordinator accepted the one
    /// terminal response transition.
    InteractionResponseAccepted {
        /// The full routed interaction identity.
        interaction: InteractionRef,
    },
    /// `snapshot_get` succeeded.
    Snapshot {
        /// The authoritative snapshot at the returned cursor.
        snapshot: RuntimeClientSnapshot,
        /// The cursor the snapshot is linearized at.
        cursor: RuntimeClientCursor,
    },
    /// `transcript_page_get` succeeded.
    TranscriptPage {
        /// The bounded durable transcript page.
        page: RuntimeClientTranscriptPage,
    },
    /// `subscribe_events` succeeded.
    Subscribed {
        /// The cursor the subscription resumes after.
        after_cursor: RuntimeClientCursor,
    },
    /// `capability_get` succeeded: the active capability projection.
    Capability {
        /// The deterministic active capability projection.
        capabilities: CapabilityView,
    },
    /// `model_catalog_get` succeeded: the safe selectable-model view.
    ModelCatalog {
        /// The bounded public catalog view.
        catalog: ModelCatalogView,
    },
    /// `model_get` succeeded: the authoritative session model state.
    Model {
        /// The redacted session model view.
        model: Box<SessionModelView>,
    },
    /// Bounded persisted sessions for `/resume`.
    SessionList {
        /// Session metadata rows.
        sessions: Vec<SessionSummaryView>,
        /// Offset for the next page, when more matching rows exist.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_offset: Option<usize>,
    },
    /// Active native Session metadata for `/session`.
    Session {
        /// The authoritative Session snapshot.
        session: SessionView,
    },
    /// Active Session graph plus historical branch boundaries.
    SessionTree {
        /// The active Session metadata.
        session: SessionView,
        /// One bounded graph-node page.
        nodes: Vec<SessionNodeView>,
        /// Offset for the next graph-node page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_node_offset: Option<usize>,
        /// Branchable user-message boundaries.
        branchable_messages: Vec<SessionUserMessageBoundaryView>,
        /// Offset for the next historical-boundary page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_history_offset: Option<usize>,
    },
    /// A metadata change or a newly selected/created lineage whose catalog
    /// visibility and durability completed normally. Fork/tree may carry a
    /// selected user prompt as transient editor content; it is not canonical
    /// destination history.
    SessionChanged {
        /// The newly authoritative Session snapshot.
        session: SessionView,
        /// Optional uncommitted editor content restored by fork/tree branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        editor_content: Option<Vec<UserContentBlock>>,
        /// Whether the client must reattach to compose the selected lineage.
        restart_required: bool,
    },
    /// A Session transition crossed the catalog visibility commit point, but
    /// the post-rename durability barrier was uncertain. The transition is
    /// authoritative, the current attachment must be replaced, and the
    /// optional editor content is a transient product result rather than
    /// canonical conversation history. The client must restart and refresh
    /// the Session from the new native process before restoring that content.
    SessionCommittedRestartRequired {
        /// The Session snapshot from the committed catalog document.
        session: SessionView,
        /// Optional uncommitted editor content restored by fork/tree branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        editor_content: Option<Vec<UserContentBlock>>,
        /// Bounded diagnostic for the replacement path.
        diagnostic: String,
    },
    /// `model_set` succeeded: the update was applied and published.
    ///
    /// The result carries the *session* state after the update. It never
    /// implies that a running attempt changed model.
    ModelSet {
        /// The redacted session model view after the update.
        model: Box<SessionModelView>,
    },
    /// `approval_mode_set` succeeded.
    ApprovalModeSet {
        /// The authoritative effective mode.
        effective_approval_mode: ApprovalMode,
        /// The desired mode when it is pending reconciliation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending_approval_mode: Option<ApprovalMode>,
        /// The monotonic control-plane revision.
        revision: u64,
    },
    /// `background_status` succeeded.
    BackgroundStatus {
        /// The canonical registry snapshot of the execution.
        execution: RuntimeClientBackgroundExecution,
    },
    /// `background_cancel` succeeded: the request was processed by the
    /// authoritative registry. Acceptance is never terminal settlement.
    BackgroundCancelAccepted {
        /// The registry snapshot after processing the request.
        execution: RuntimeClientBackgroundExecution,
    },
    /// `subagent_status` succeeded (Issue #60).
    SubagentStatus {
        /// The canonical registry snapshot of the child.
        subagent: RuntimeClientSubagent,
    },
    /// `subagent_cancel` succeeded: the intent was committed by the
    /// authoritative registry. Acceptance is never terminal settlement.
    SubagentCancelAccepted {
        /// The registry snapshot after processing the request.
        subagent: RuntimeClientSubagent,
    },
    /// `subagent_workspace_dispose` completed its resource transition or its
    /// deterministic idempotent/no-resource outcome.
    SubagentWorkspaceDisposed {
        /// The authoritative terminal subagent projection after the request.
        subagent: RuntimeClientSubagent,
        /// The physical-resource result, independent of logical lifecycle.
        outcome: RuntimeClientSubagentWorkspaceDisposalOutcome,
    },
    /// `detach` succeeded.
    Detached,
    /// `shutdown` succeeded: the conversation runtime reached quiescence.
    ShutdownCompleted,
}

/// The typed protocol-visible errors of the Runtime Client protocol.
///
/// Every protocol-visible failure maps to one category; provider SDK
/// errors and internal synchronization failures are never exposed as
/// protocol structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientError {
    /// The negotiated protocol version is not supported by this runtime.
    UnsupportedProtocolVersion {
        /// The version this runtime speaks.
        supported: u16,
        /// The version the client requested.
        requested: u16,
    },
    /// A control attachment is already active: the host allows one control
    /// attachment per live runtime instance, and a second control attach
    /// never evicts the active one. Read-only inspection attachments use a
    /// separate admission lane.
    AttachmentInUse {
        /// The identity of the active attachment.
        existing_attachment_id: AttachmentId,
    },
    /// The request arrived without an admitted attachment.
    NotAttached,
    /// The request payload is malformed or contradictory.
    InvalidRequest {
        /// Human-readable detail.
        message: String,
    },
    /// No attempt is currently cancellable.
    NoCurrentAttempt,
    /// Resource reload was refused because semantic work owns the Session.
    ResourceReloadBusy {
        /// Bounded machine-readable busy category.
        reason: String,
    },
    /// The interaction was no longer pending. Duplicate, stale, pre-crash,
    /// and post-quiescent responses all use this bounded contract.
    InteractionNotPending {
        /// The stale routed interaction identity.
        interaction: InteractionRef,
    },
    /// The response was typed but invalid for the pending interaction.
    InteractionInvalidResponse {
        /// The bounded validation diagnostic.
        message: String,
    },
    /// The response left the interaction pending map, but its durable settled
    /// fact could not commit (Issue #109). The interaction is settled
    /// fail-closed and the decision granted no authority; the client is told
    /// rather than shown an acceptance the durable audit does not support.
    InteractionAuditFailed {
        /// The routed interaction whose settlement was not durably recorded.
        interaction: InteractionRef,
    },
    /// The runtime has not been activated for an `ApprovalMode` update.
    ApprovalModeInactive,
    /// The runtime's durability authority rejected an `ApprovalMode` update.
    ApprovalModeDurabilityFailed {
        /// The bounded durability diagnostic.
        message: String,
    },
    /// The referenced background execution does not exist in the
    /// authoritative conversation registry.
    UnknownBackgroundExecution {
        /// The referenced execution identity.
        execution_id: ToolExecutionId,
    },
    /// The referenced subagent child does not exist in the authoritative
    /// conversation registry (Issue #60).
    UnknownSubagent {
        /// The referenced subagent identity.
        subagent_id: crate::runtime::identity::SubagentId,
    },
    /// The Runtime Client requested disposal, but the recorded handoff and
    /// current Git registration did not prove one exact runtime-owned
    /// resource. The operation failed closed before destructive mutation.
    SubagentWorkspaceOwnershipMismatch {
        /// The retained child whose resource could not be proven.
        subagent_id: crate::runtime::identity::SubagentId,
        /// The bounded proof diagnostic.
        message: String,
    },
    /// The requested cursor is not serviceable by the bounded projection
    /// replay buffer; the client must take a fresh snapshot from the runtime
    /// projection. The durable Event Journal is not represented by this
    /// cursor.
    ResyncRequired {
        /// The cursor the client asked to resume after.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The runtime is shutting down and no longer admits inbound work.
    RuntimeShutdown,
    /// The operation is impossible in the current authoritative runtime
    /// state.
    InvalidState {
        /// Human-readable detail.
        message: String,
    },
    /// A model configuration update could not be resolved.
    ///
    /// The update was rejected as a whole: no session state changed and no
    /// model-configuration event was published. The message never carries a
    /// credential.
    InvalidModelConfiguration {
        /// Human-readable detail.
        message: String,
    },
    /// The Runtime Client observation stream is exhausted.
    ProjectionExhausted,
    /// A runtime-internal failure prevented the operation; no provider
    /// detail is exposed.
    RuntimeFailure {
        /// Human-readable detail.
        message: String,
    },
    /// A native Session operation failed without changing the authoritative
    /// active selection.
    SessionFailure {
        /// Bounded product-level diagnostic.
        message: String,
    },
    /// The native Session owner has crossed a terminal replacement boundary.
    /// The current attachment must be closed and replaced; this is not an
    /// ordinary recoverable Session failure.
    SessionRestartRequired {
        /// Bounded replacement diagnostic.
        message: String,
    },
}

/// One Runtime Client event pushed on the observation stream.
///
/// The envelope carries the cursor the event was published at, so a client
/// resuming after a cursor always knows the exact stream position of every
/// observation. Notifications structurally carry no request id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientProtocolEvent {
    /// The cursor this event was published at.
    pub cursor: RuntimeClientCursor,
    /// The typed external event payload.
    pub event: RuntimeClientEvent,
}

// Re-exported for use by the public protocol docs.
pub use super::snapshot::{
    RuntimeClientBackgroundExecution, RuntimeClientSubagent, RuntimeClientSubagentWorkspace,
    RuntimeClientWorkspaceHandoff, RuntimeClientWorkspaceIsolation,
};

#[cfg(test)]
mod tests {
    use super::{
        RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor, RuntimeClientError,
        RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse,
        RuntimeClientResult, RuntimeClientSubagent, SessionView,
    };
    use chrono::{DateTime, Utc};

    use crate::events::types::EVENT_SCHEMA_VERSION;
    use crate::message::content::TextBlock;
    use crate::message::types::UserContentBlock;
    use crate::runtime::identity::{AttemptId, ConversationId, InteractionId, ToolExecutionId};
    use crate::runtime::interaction::{ApprovalDecision, InteractionRef, InteractionResponse};
    use crate::runtime_client::event::RuntimeClientEvent;

    /// The Runtime Client protocol version is a distinct constant from the
    /// internal event schema version: representing or changing one never
    /// implies anything about the other.
    #[test]
    fn protocol_version_is_independent_from_event_schema_version() {
        let _ = EVENT_SCHEMA_VERSION;
        assert_eq!(RUNTIME_CLIENT_PROTOCOL_VERSION, 16);
        // Structural independence: no Runtime Client protocol type carries
        // a `schema_version` field, and serialized requests never embed it.
        let request = RuntimeClientRequest::Initialize {
            id: super::RequestId::new(1),
            protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
        };
        let value = serde_json::to_value(request).expect("serialize request");
        assert!(value.get("schema_version").is_none());
        assert!(value.get("event_schema_version").is_none());
    }

    /// The Runtime Client subagent projection carries the complete closed
    /// lifecycle vocabulary on the wire, including the unknown-outcome
    /// `Interrupted` terminal state, plus the Issue #178 observation-plane
    /// fields and the Issue #187 logical/physical workspace projection.
    #[test]
    fn interrupted_subagent_projection_serializes_as_the_current_wire_state() {
        let subagent = RuntimeClientSubagent {
            subagent_id: crate::runtime::identity::SubagentId::new("subagent-1"),
            child_agent_id: crate::runtime::identity::AgentId::new("agent-child"),
            child_conversation_id: ConversationId::new("conversation-child"),
            agent: "conformance".to_owned(),
            definition_digest: "sha256:definition".to_owned(),
            state: crate::runtime::subagent::SubagentState::Interrupted,
            detail: Some("child outcome unknown".to_owned()),
            observation: crate::runtime::subagent::SubagentObservation::default(),
            execution_profile: None,
            started_at: DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
                .expect("timestamp")
                .with_timezone(&Utc),
            workspace: super::RuntimeClientSubagentWorkspace {
                logical_workspace: std::path::PathBuf::from("<shared-workspace>"),
                isolation: super::RuntimeClientWorkspaceIsolation::Shared,
                resource_state: crate::runtime::subagent::SubagentWorkspaceResourceState::None,
                handoff: None,
            },
        };

        let value = serde_json::to_value(&subagent).expect("serialize subagent projection");
        assert_eq!(value["state"], "interrupted");
        assert_eq!(value["agent"], "conformance");
        assert_eq!(value["definition_digest"], "sha256:definition");
        assert_eq!(value["observation"]["revision"], 0);
        assert_eq!(
            value["observation"]["activity"]["type"],
            "awaiting_activity"
        );
        assert_eq!(value["started_at"], "2026-09-02T10:00:00Z");
        let decoded: RuntimeClientSubagent =
            serde_json::from_value(value).expect("deserialize subagent projection");
        assert_eq!(decoded, subagent);
    }

    /// Requests serialize deterministically with their method discriminator
    /// and round-trip exactly.
    #[test]
    fn requests_round_trip_deterministically() {
        let request = RuntimeClientRequest::Initialize {
            id: super::RequestId::new(7),
            protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
        };
        let first = serde_json::to_string(&request).expect("serialize");
        let second = serde_json::to_string(&request).expect("serialize again");
        assert_eq!(first, second);
        let value: serde_json::Value = serde_json::from_str(&first).expect("parse");
        assert_eq!(value["method"], "initialize");
        assert_eq!(value["id"], 7);
        let decoded: RuntimeClientRequest =
            serde_json::from_str(&first).expect("deserialize request");
        assert_eq!(decoded, request);

        let request = RuntimeClientRequest::SessionTreeBranch {
            id: super::RequestId::new(12),
            surface_revision: crate::conversation::SurfaceRevision::new(7),
            message_id: crate::runtime::identity::MessageId::new("user-c"),
        };
        let value = serde_json::to_value(&request).expect("serialize session request");
        assert_eq!(value["method"], "session_tree_branch");
        assert_eq!(value["surface_revision"], 7);
        assert!(request.is_session_request());
        let decoded: RuntimeClientRequest =
            serde_json::from_value(value).expect("deserialize session request");
        assert_eq!(decoded, request);

        let request = RuntimeClientRequest::SubmitInbound {
            id: super::RequestId::new(9),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hello".to_owned(),
            })],
        };
        let value = serde_json::to_value(&request).expect("serialize");
        assert_eq!(value["method"], "submit_inbound");
        let decoded: RuntimeClientRequest =
            serde_json::from_value(value).expect("deserialize request");
        assert_eq!(decoded, request);
    }

    /// The interaction response is a typed decision only: its wire shape has
    /// no field through which a client could replace the resolved tool
    /// arguments.
    #[test]
    fn interaction_response_request_round_trips_without_replacement_arguments() {
        let request = RuntimeClientRequest::InteractionRespond {
            id: super::RequestId::new(11),
            interaction: InteractionRef::new(
                ConversationId::new("conversation-1"),
                InteractionId::new("attempt-1-interaction-1"),
            ),
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Deny {
                    reason: "human denied".to_owned(),
                },
            },
        };
        let value = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(value["method"], "interaction_respond");
        assert_eq!(value["interaction"]["conversation_id"], "conversation-1");
        assert_eq!(
            value["interaction"]["interaction_id"],
            "attempt-1-interaction-1"
        );
        assert_eq!(value["response"]["type"], "approval");
        assert!(value["response"].get("arguments").is_none());
        let decoded: RuntimeClientRequest =
            serde_json::from_value(value).expect("deserialize request");
        assert_eq!(decoded, request);
    }

    /// Unknown methods and unknown request fields are rejected explicitly.
    #[test]
    fn unknown_methods_and_fields_are_rejected() {
        let unknown_method = r#"{"method": "future_method", "id": 1}"#;
        assert!(serde_json::from_str::<RuntimeClientRequest>(unknown_method).is_err());
        let interaction_publish =
            r#"{"method": "interaction_publish", "id": 1, "conversation_id": "forged"}"#;
        assert!(
            serde_json::from_str::<RuntimeClientRequest>(interaction_publish).is_err(),
            "Runtime Client exposes response control only; publication is runtime-owned"
        );
        let unknown_field = r#"{"method": "snapshot_get", "id": 1, "extra": true}"#;
        assert!(serde_json::from_str::<RuntimeClientRequest>(unknown_field).is_err());
    }

    /// Responses echo the request id and carry exactly one of result/error.
    #[test]
    fn responses_correlate_request_ids() {
        let response = RuntimeClientResponse {
            id: super::RequestId::new(42),
            result: Some(RuntimeClientResult::Detached),
            error: None,
        };
        let value = serde_json::to_value(&response).expect("serialize");
        assert_eq!(value["id"], 42);
        assert!(value.get("error").is_none());
        let decoded: RuntimeClientResponse = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, response);
    }

    /// A committed-but-uncertain Session transition is a success payload with
    /// a distinct wire discriminator, not a generic Session failure. Its
    /// transient editor content survives the JSON round trip unchanged.
    #[test]
    fn committed_session_transition_round_trips_with_editor_payload() {
        let now = chrono::Utc::now();
        let result = RuntimeClientResult::SessionCommittedRestartRequired {
            session: SessionView {
                id: "session-2".to_owned(),
                name: None,
                created_at: now,
                updated_at: now,
                active_node: "node-2".to_owned(),
                active_conversation_id: ConversationId::new("conversation-2"),
                node_count: 1,
            },
            editor_content: Some(vec![UserContentBlock::Text(TextBlock {
                text: "fork-draft-exact-7f3b".to_owned(),
            })]),
            diagnostic: "catalog visibility committed; durability uncertain".to_owned(),
        };
        let value = serde_json::to_value(&result).expect("serialize transition result");
        assert_eq!(value["type"], "session_committed_restart_required");
        assert_eq!(value["editor_content"][0]["type"], "text");
        let decoded: RuntimeClientResult =
            serde_json::from_value(value).expect("deserialize transition result");
        assert_eq!(decoded, result);
    }

    /// Notifications structurally carry no request id: an event envelope
    /// serializes as cursor + typed payload only.
    #[test]
    fn notifications_never_fabricate_request_ids() {
        let notification = RuntimeClientProtocolEvent {
            cursor: RuntimeClientCursor::new(5),
            event: RuntimeClientEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt-1"),
                model: Box::new(attempt_model_view("acme/model-a")),
            },
        };
        let value = serde_json::to_value(&notification).expect("serialize");
        assert!(value.get("id").is_none());
        assert_eq!(value["cursor"], 5);
        assert_eq!(value["event"]["type"], "attempt_started");
        let decoded: RuntimeClientProtocolEvent =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, notification);
    }

    /// `attempt_started` is self-contained: the frozen attempt model travels
    /// with the event, so a continuously subscribed client never has to
    /// infer the active attempt's model or take a second snapshot.
    #[test]
    fn attempt_started_carries_the_frozen_attempt_model() {
        let event = RuntimeClientEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-a"),
            model: Box::new(attempt_model_view("acme/model-a")),
        };
        let value = serde_json::to_value(&event).expect("serialize");
        assert_eq!(value["type"], "attempt_started");
        assert_eq!(value["model"]["primary"]["model"], "acme/model-a");
        assert_eq!(value["model"]["summary"]["mode"], "session");
        let decoded: RuntimeClientEvent = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded, event);
    }

    /// The redacted attempt model view of one model reference.
    fn attempt_model_view(reference: &str) -> crate::model::session::AttemptModelView {
        use crate::model::catalog::{ModelCapabilities, ModelRef};
        use crate::model::invocation::{ModelInvocationView, RequestParams};
        use crate::model::session::{AttemptModelView, SummaryModelView};
        use crate::model::types::ModelProtocol;

        let capabilities = ModelCapabilities::text_only(true, false);
        AttemptModelView {
            primary: ModelInvocationView {
                model: ModelRef::parse(reference).expect("a valid model reference"),
                protocol: ModelProtocol::OpenAiChatCompletions,
                context_window: 128_000,
                model_max_output_tokens: 4096,
                max_output_tokens: 4096,
                reasoning_profile: None,
                reasoning_enabled: false,
                request_params: RequestParams::new(),
                capabilities: capabilities.clone(),
                declared_capabilities: capabilities,
            },
            summary: SummaryModelView::Session,
        }
    }

    /// Typed errors carry structured categories, never free-form strings
    /// only, and round-trip exactly.
    #[test]
    fn typed_errors_round_trip() {
        let cases = [
            RuntimeClientError::UnsupportedProtocolVersion {
                supported: 6,
                requested: 10,
            },
            RuntimeClientError::NoCurrentAttempt,
            RuntimeClientError::InteractionNotPending {
                interaction: InteractionRef::new(
                    ConversationId::new("conversation-1"),
                    InteractionId::new("attempt-1-interaction-1"),
                ),
            },
            RuntimeClientError::InteractionInvalidResponse {
                message: "bounded".to_owned(),
            },
            RuntimeClientError::UnknownBackgroundExecution {
                execution_id: ToolExecutionId::new("exec_1"),
            },
            RuntimeClientError::ResyncRequired {
                after_cursor: RuntimeClientCursor::new(3),
                earliest_serviceable: RuntimeClientCursor::new(100),
            },
            RuntimeClientError::RuntimeShutdown,
            RuntimeClientError::SessionFailure {
                message: "destination publication failed".to_owned(),
            },
            RuntimeClientError::SessionRestartRequired {
                message: "the old attachment must be replaced".to_owned(),
            },
        ];
        for error in cases {
            let value = serde_json::to_value(&error).expect("serialize");
            let decoded: RuntimeClientError = serde_json::from_value(value).expect("deserialize");
            assert_eq!(decoded, error);
        }
    }

    /// The cursor is a distinct newtype: it is not `u64`, not the mailbox
    /// `InboundSequence`, and not the internal event sequence.
    #[test]
    fn cursor_is_a_distinct_domain() {
        let cursor = RuntimeClientCursor::new(7);
        assert_eq!(cursor.get(), 7);
        assert_eq!(cursor.to_string(), "7");
        let value = serde_json::to_value(cursor).expect("serialize");
        assert_eq!(value, serde_json::json!(7));
    }
}
