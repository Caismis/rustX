//! The deterministic agent execution loop (M3 + M4 context integration).
//!
//! The loop turns one canonical `ModelEvent` stream into an attempt
//! execution:
//!
//! ```text
//! input
//!  ↓
//! ConversationState (Message Ledger + Conversation Surface) + pending
//! FreshInboundTurn
//!  ↓
//! Context Assembly (native + extension + deferred post-tool proposals)
//!  ↓
//! PreStepPolicy → Enter | Reject(reason)
//!  ↓
//! transient staged context / PreparedModelTurn (nothing commits)
//!  ↓
//! cancellation-vs-start arbitration
//!    ├─ cancel: discard staged request
//!    └─ start: fused durable model-turn-start transaction
//!  ↓
//! provider invocation
//!  ↓
//! canonical model events
//!  ↓
//! message assembly + RuntimeEvent emission
//!  ↓
//! tool calls (if requested): resolve, execute, record
//!  ↓
//! tool batch structural settlement → ToolResultObserver pass →
//! Agent-Loop-owned deferred context buffer
//!  ↓
//! TurnCompleted → safe boundary → one finite inbound mailbox drain
//!  ↓
//! continuation (or proactive compaction / compact-and-retry on overflow)
//!  ↓
//! terminal settlement candidate → durable terminal RuntimeEvent when commit succeeds
//! ```
//!
//! Ownership: the loop owns execution semantics, message assembly, tool
//! execution, continuation state, cancellation observation, context assembly,
//! request admission/snapshots, fresh-inbound lifecycle, safe-boundary inbound
//! consumption, and lifecycle-extension coordination. The durable
//! `ConversationStore` owns canonical facts and the Event Journal; the loop
//! keeps only bounded active execution state and the current conversation
//! working set. The adapter owns provider protocol translation only. No
//! provider protocol concept appears in this module.
//!
//! The typed lifecycle seams of Issue #56 live on the required immutable
//! [`AttemptLifecycle`]: exactly one [`PreStepPolicy`] evaluation per primary
//! step and exactly one [`ToolResultObserver`] pass per structurally settled
//! tool batch. Neither seam receives canonical-history, tool-identity,
//! cancellation, or terminal authority, and neither can create a second
//! context-admission path: deferred post-tool proposals re-enter the same
//! Context Assembly → pre-step policy → admission pipeline as every other
//! proposal.
//!
//! [`PreStepPolicy`]: super::lifecycle::PreStepPolicy
//! [`ToolResultObserver`]: super::lifecycle::ToolResultObserver
//!
//! The context path is mandatory: every normal `AgentExecution` is
//! constructed with a [`ContextRuntime`], and there is exactly one normal
//! execution model — no no-context/unbounded mode and no Agent Status
//! disable flag. Agent Status is composed whenever a pending
//! [`FreshInboundTurn`] exists and is consumed by the first successful model
//! invocation that observes it. The conversation inbound mailbox is owned by
//! the conversation tool runtime: the loop drains exactly
//! `tool_runtime.mailbox()` at every safe boundary, so background terminal
//! notifications always enter the same mailbox the Agent Loop drains.
//! Cancellation is a generic Agent Loop invariant for every execution:
//! observable cancellation is checked before every model turn begins.

use futures_util::StreamExt;

use crate::capabilities::AttemptCapabilityLease;
use crate::context::compaction::{
    CompactionAttribution, CompactionExecutionError, execute_compaction,
};
use crate::context::engine::CompactionConstraints;
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
use crate::context::tokens::ProviderObservedInput;
use crate::context::{
    AcceptedContext, ContextRuntime, ContributorInputSnapshot, DeferredContextProposal,
    MAX_DEFERRED_CONTEXT_PROPOSALS, MAX_PROPOSALS_PER_CONTRIBUTOR, render_effective_system_prompt,
    validate_user_message_proposal,
};
use crate::conversation::{ConversationError, ConversationState, PreparedCanonicalCommit};
use crate::durable::ConversationStore;
use crate::events::types::{
    AttemptFailure, AttemptOutcome, EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope,
};
use crate::message::types::{AssistantMessageBlock, MessageBlock, ToolMessageBlock};
use crate::model::adapter::ModelEventStream;
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::session::AttemptModelSnapshot;
use crate::model::snapshot::{RequestIdentity, RequestSnapshot};
use crate::model::types::{ModelRequest, ModelUsage};
use crate::publication::{
    CoalescePolicy, PublicationClock, PublicationCoalescer, PublicationFrame, PublicationPayload,
    PublicationStreamStart, SystemPublicationClock,
};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, RequestId,
    ToolCallId, ToolId, TurnId,
};
use crate::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger, MailboxError};
use crate::runtime::interaction::{ApprovalDecision, InteractionOutcome, InteractionResponse};
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::background::BackgroundDispatchOutcome;
use crate::tools::executor::{
    PreflightOutcome, PreparedInvocation, ProgressReporter, ToolExecutionContext, ToolRegistry,
};
use crate::tools::runtime::ConversationToolRuntime;
use crate::tools::types::{
    ToolCall, ToolConcurrencyPolicy, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
    ToolInvocationMode, ToolOrigin, ToolProgress,
};

use super::assembly::ModelEventAssembler;
use super::cancellation::{AgentCancellation, StartAdjudication};
use super::lifecycle::{
    AttemptLifecycle, ObservedToolInvocation, PreStepBatch, PreStepDecision, PreToolDecision,
    PreToolView, ToolResultObservation,
};
use super::observer::{AgentExecutionObserver, AgentStatusObservation};
use super::state::{ExecutionState, ExecutionStateMachine};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// The bounded M4 retry policy for `ContextWindowExceeded`.
///
/// This is the only retry policy the loop implements: one compaction, one
/// retry. No generic backoff, rate-limit, timeout, transport, or provider
/// fallback retry exists.
pub const MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN: u32 = 1;

/// Everything the loop needs to know about one attempt.
#[derive(Debug, PartialEq)]
pub struct AgentExecutionRequest {
    /// The agent being executed.
    pub agent_id: AgentId,
    /// The conversation the attempt belongs to.
    pub conversation_id: ConversationId,
    /// The attempt identity reported by attempt-level events.
    pub attempt_id: AttemptId,
    /// The one canonical conversation state the attempt takes ownership of.
    ///
    /// Ownership transfers into the attempt: while the attempt runs it is
    /// the single mutable conversation authority, and settlement transfers
    /// the state back to the host through
    /// [`AgentExecutionResult::conversation`]. There is deliberately no
    /// clone-based `initial_messages` API, so two independently mutable
    /// conversation copies are not representable.
    pub conversation: ConversationState,
    /// The explicit execution trigger of the attempt's first model turn.
    ///
    /// Fresh inbound identity is explicit execution state, never inferred
    /// from message role or history shape. [`InitialTurnTrigger::FreshInbound`]
    /// makes Agent Status and fresh-inbound validation mandatory; omitting a
    /// status or a fresh turn is impossible, so the trigger can never
    /// silently suppress Agent Status.
    pub initial_turn_trigger: InitialTurnTrigger,
    /// The per-execution/conversation IANA timezone metadata used by the
    /// temporal Agent Status section, when known. The process/system local
    /// timezone is never consulted.
    pub timezone: Option<Tz>,
    /// The one immutable model ownership object of the attempt.
    ///
    /// The loop receives exactly this instead of independent
    /// mutable-looking model fields. Every model turn of the attempt — the
    /// first request, every tool→model continuation, every context-overflow
    /// retry, every proactive-compaction continuation, and every compaction
    /// summary — uses this frozen snapshot and never reads live session
    /// model state.
    pub model: AttemptModelSnapshot,
}

/// The durable authority stage that failed while an attempt was executing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableFailureKind {
    /// A canonical Message Ledger/Surface commit failed.
    CanonicalCommit,
    /// The immutable request-start snapshot/event transaction failed.
    RequestStart,
    /// The atomic compaction transition failed.
    Compaction,
    /// A standalone or terminal Event Journal append failed.
    EventJournal,
    /// A durable publication-plane transition failed: a stream could not
    /// open, frames could not be staged before release, the publication
    /// terminal could not commit, or an audit could not terminalize.
    Publication,
}

impl DurableFailureKind {
    /// The stable coordinator diagnostic name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalCommit => "canonical_commit",
            Self::RequestStart => "request_start",
            Self::Compaction => "compaction",
            Self::EventJournal => "event_journal",
            Self::Publication => "publication",
        }
    }
}

/// The deterministic result of one attempt execution.
///
/// The `outcome` field is the execution state-machine settlement candidate;
/// it remains meaningful when the final terminal Event Journal append fails,
/// in which case no terminal fact is published and `durable_failure_kind` is
/// `EventJournal`.
///
/// `conversation` is the bounded current working state handed back to the
/// host: active Ledger bodies and the current Conversation Surface. The
/// complete append-only Ledger and historical Surface revisions remain in the
/// durable `ConversationStore` and are read there on demand.
#[derive(Debug, PartialEq)]
pub struct AgentExecutionResult {
    /// The executed attempt.
    pub attempt_id: AttemptId,
    /// The platform outcome of the execution settlement candidate. For a
    /// normally settled attempt it is also the outcome of the one committed
    /// terminal event; a terminal Event Journal failure can leave that event
    /// absent while this value remains available to the coordinator.
    pub outcome: AttemptOutcome,
    /// The terminal state-machine settlement that produced the terminal
    /// event: [`ExecutionState::Completed`] for successful settlement and
    /// [`ExecutionState::Failed`] for failure and cancellation settlement.
    pub terminal_state: ExecutionState,
    /// The bounded current conversation read model, transferred back to the
    /// runtime. Historical durable facts remain in `ConversationStore`.
    pub conversation: ConversationState,
    /// The durable-authority failure the attempt encountered, when one
    /// caused or contributed to its failure settlement (Issue #63).
    ///
    /// The coordinator uses this to keep its durability-health state
    /// honest: after an active-attempt durable canonical-write failure the
    /// runtime must not return to a false healthy state and admit further
    /// work as though storage were fine. The in-memory conversation state
    /// handed back stayed consistent with the durable Ledger — a failed
    /// durable commit installed nothing — but the durable authority itself
    /// is marked failed.
    pub durable_failure: Option<String>,
    /// The typed durable stage associated with [`Self::durable_failure`].
    pub durable_failure_kind: Option<DurableFailureKind>,
}

impl AgentExecutionResult {
    /// The hot committed message bodies available in the settled attempt, in
    /// current Surface order. After a durable restart this is the current
    /// Surface working set; use the `ConversationStore` for Ledger commit order
    /// and retired history.
    ///
    /// This explicitly enumerates the hot Ledger read model. The engine's
    /// normal projection and compaction paths never enumerate the durable
    /// historical Ledger; callers needing retired history page the store.
    #[must_use]
    pub fn messages(&self) -> &[MessageBlock] {
        self.conversation.ledger().audit_records()
    }

    /// The active ordered message identities of the settled Conversation
    /// Surface.
    #[must_use]
    pub fn active_ids(&self) -> &[MessageId] {
        self.conversation.active_ids()
    }
}

/// One agent attempt execution.
///
/// The loop borrows the model adapter, the immutable tool registry, the
/// attempt cancellation signal, and owns the attempt capability lease, the
/// mandatory M4 context runtime, the conversation tool runtime (whose
/// canonical mailbox the loop drains), the execution state machine, the
/// bounded current read model, the retained continuation state, and the
/// pending fresh inbound trigger.
pub struct AgentExecution<'a> {
    request: AgentExecutionRequest,
    capability: AttemptCapabilityLease,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a ConversationToolRuntime,
    /// The conversation-level durable authority. Tool execution receives
    /// only the mailbox capability; canonical/request/event durability is
    /// owned by the conversation execution plane.
    store: std::sync::Arc<dyn ConversationStore>,
    state: ExecutionStateMachine,
    /// The attempt's owned conversation state: the single mutable
    /// conversation authority for the attempt's lifetime.
    conversation: ConversationState,
    /// The durable-authority failure encountered by this attempt, when
    /// any (Issue #63): carried into [`AgentExecutionResult`] so the
    /// coordinator never returns to a false healthy durability state
    /// after an active-attempt durable failure, regardless of how the
    /// terminal outcome itself is classified.
    durable_failure: Option<String>,
    durable_failure_kind: Option<DurableFailureKind>,
    pending_continuation: Option<ProviderContinuationState>,
    /// The committed Assistant message that established the pending
    /// continuation, when one is pending.
    continuation_owner: Option<MessageId>,
    /// The pending fresh inbound turn: `Some` until a successful model
    /// invocation has observed it. One pending fresh inbound turn produces
    /// at most one Agent Status snapshot per request preparation.
    pending_fresh_inbound: Option<FreshInboundTurn>,
    context_runtime: ContextRuntime,
    /// The transient accepted context for the current admitted primary step.
    /// It is retained across an overflow compaction/retry and discarded only
    /// when the next primary step begins.
    accepted_context: Option<AcceptedContext>,
    /// The one required immutable lifecycle configuration of the attempt.
    /// It carries the attempt's single pre-step policy owner and its
    /// identity-registered tool-result observers; the inert configuration is
    /// the identity.
    lifecycle: AttemptLifecycle,
    /// The Agent-Loop-owned staging buffer of deferred context.
    ///
    /// It is **not** canonical history and **not** a second transcript: a
    /// staged proposal is a transient value that becomes a conversation fact
    /// only after the next Context Assembly, the pre-step policy, and the
    /// admission boundary accept it. The buffer is filled after a tool batch
    /// reaches structural settlement, in canonical `(ToolCall batch position,
    /// producer identity, proposal FIFO)` order, and drained by the very next
    /// primary step's assembly, so it never accumulates across turns.
    ///
    /// Each entry carries the trusted producer identity the Agent Loop stamped
    /// from the observer's registration. The buffer records *timing*; the
    /// identity records *ownership*, and Context Assembly derives lane and
    /// provenance from the identity alone.
    deferred_context: Vec<DeferredContextProposal>,
    /// Per-attempt context-generation allocator owned by the Agent Loop.
    context_generation_serial: u64,
    observed: Option<ProviderObservedInput>,
    last_request_fingerprint: Option<u64>,
    /// The exact request identity of the in-flight provider request. The
    /// provider outcome fact (P) and the publication stream both name it, so
    /// neither can be attributed to a different request of the same turn.
    last_request_id: Option<RequestId>,
    /// The bounded publication policy of the attempt.
    publication_policy: CoalescePolicy,
    /// The monotonic clock the publication latency policy reads.
    publication_clock: std::sync::Arc<dyn PublicationClock>,
    /// The open, not-yet-settled publication stream of the in-flight model
    /// request. Exactly one stream is open at a time: a stream settles — as
    /// canonical, unaccepted, or incomplete — before the next one opens.
    publication: Option<OpenPublication>,
    /// Set when an attempted audit settlement failed. The stream remains in
    /// durable authority for startup recovery, but the Agent Loop must not
    /// retry the transition in the same control-flow turn and accidentally
    /// continue into a second request.
    publication_settlement_failed: bool,
    /// The optional live observation seam: when attached, every emitted
    /// runtime fact, every committed canonical message, and every composed
    /// Agent Status is observed at its commit linearization point. The
    /// durable Event Journal remains the historical authority regardless of
    /// attachment.
    observer: Option<&'a dyn AgentExecutionObserver>,
    /// Test-only control point parked at the turn-continuation boundary:
    /// after a completed turn (and all its mailbox drain/append work)
    /// returned "continue", before the generic cancellation check of the
    /// next model turn; never present outside `#[cfg(test)]`. The `Mutex`
    /// keeps an execution with an installed pause `Sync` (the pause holds
    /// a `std::sync::mpsc::Receiver`), so host-driven attempt tasks can be
    /// spawned across threads in tests.
    #[cfg(test)]
    continuation_pause: std::sync::Mutex<Option<test_sync::ContinuationBoundaryPause>>,
    /// Test-only control point at the one M9b model-turn start boundary:
    /// immediately before the cancellation-vs-start arbitration, and inside
    /// the arbitration critical section immediately before the durable
    /// start commit. No request-scoped context, Request Snapshot, start
    /// fact, or provider request exists while the first phase is parked;
    /// while the second phase is parked the arbitration holds the start
    /// gate, so a concurrent cancellation provably blocks behind it.
    #[cfg(test)]
    start_boundary_pause: std::sync::Mutex<Option<test_sync::StartBoundaryPause>>,
    /// Test-only control point after a foreground tool-start fact and before
    /// the next sibling's start frontier advances. This makes cancellation
    /// during a parallel batch deterministic without changing production
    /// scheduling.
    #[cfg(test)]
    tool_start_pause: std::sync::Mutex<Option<test_sync::ToolStartPause>>,
    turn: u32,
    terminal_emitted: bool,
}

/// The open publication stream of one in-flight model request.
///
/// The stream owns the release plane of exactly one provider request: the
/// bounded coalescer that decides when a frame exists, and the durable facts
/// of whether U has committed. It is deliberately not the assembler: the
/// assembler builds the canonical message, this builds what rustX committed
/// for release.
struct OpenPublication {
    /// The frozen identity, pinned to the exact request generation.
    start: PublicationStreamStart,
    /// The bounded coalescer of this stream.
    coalescer: PublicationCoalescer,
    /// Whether the publication terminal transaction (U) committed.
    terminal_committed: bool,
}

/// How one model stream of a turn ended.
enum StreamTerminal {
    Completed {
        finish_reason: ModelFinishReason,
        usage: Option<ModelUsage>,
    },
    Failed {
        error: ModelError,
    },
}

/// One completed model invocation: the provisional message identity, the
/// assembler holding the provisional stream content, and the stream
/// terminal.
///
/// The three pieces travel together: an overflow retry replaces the whole
/// invocation, so provisional output and tool calls of the failed request
/// are never committed under the retry's message identity.
struct ModelInvocation {
    message_id: MessageId,
    assembler: ModelEventAssembler,
    terminal: StreamTerminal,
}

/// The fully prepared, not-yet-started model turn (Issue #12, M9b).
///
/// Every fallible input of one actual model request is resolved: the
/// request-scoped context is validated against the conversation state, the
/// Effective System Prompt, the projection, the frozen Request Snapshot,
/// and the exact provider-neutral request are computed from the staged
/// view, and any compaction the request requires already committed as its
/// own independent canonical transition. Nothing in this value is durable
/// or provider-visible yet: it becomes observable only through the one
/// cancellation-vs-start arbitration in [`AgentExecution::start_model_turn`],
/// and is discarded without a trace when cancellation wins there.
struct PreparedModelTurn {
    /// The validated, not-yet-committed request-scoped context commits, in
    /// canonical append order.
    context: Vec<PreparedCanonicalCommit>,
    /// The frozen snapshot of this actual request.
    snapshot: RequestSnapshot,
    /// The exact provider-neutral request of the staged projection.
    request: ModelRequest,
    /// The staged projection's fingerprint, used to match a later
    /// provider-observed input measurement to exactly this request context.
    fingerprint: u64,
}

/// The intermediate staged view of one model turn: scratch conversation,
/// Effective System Prompt, projection, frozen snapshot, and request — all
/// transient, nothing committed.
struct StagedModelTurn {
    /// The staged projection (over the scratch conversation that includes
    /// the not-yet-committed request-scoped context).
    projection: ContextProjection,
    /// The staged Effective System Prompt.
    effective_system_prompt: String,
    /// The frozen snapshot of this actual request.
    snapshot: RequestSnapshot,
    /// The exact provider-neutral request of the staged projection.
    request: ModelRequest,
}

/// The committed result of one successful compaction: the derived record
/// plus the measurements the completion event reports.
struct CompletedCompaction;

/// The terminal outcome of the whole attempt.
enum Terminal {
    Completed { finish_reason: ModelFinishReason },
    Cancelled { reason: CancellationReason },
    Failed { failure: AttemptFailure },
}

impl Terminal {
    /// Projects the execution state-machine settlement candidate without
    /// requiring a durable terminal event to exist.
    fn outcome(&self) -> AttemptOutcome {
        match self {
            Self::Completed { finish_reason } => AttemptOutcome::Completed {
                finish_reason: finish_reason.clone(),
            },
            Self::Cancelled { reason } => AttemptOutcome::Cancelled { reason: *reason },
            Self::Failed { failure } => AttemptOutcome::Failed {
                error: failure.clone(),
            },
        }
    }
}

/// A canonical commit failure in the attempt loop.
///
/// The prepare → durable → install canonical-commit seam (Issue #63, Finding
/// 2) has two fallible phases: the in-memory preparation/validation (a
/// [`ConversationError`]) and the durable Message Ledger append (an
/// [`ConversationStoreError`]). The install phase is infallible after both.
#[derive(Debug)]
enum CanonicalCommitError {
    Conversation(ConversationError),
    Durable(crate::durable::inbox::ConversationStoreError),
}

impl core::fmt::Display for CanonicalCommitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Conversation(error) => write!(f, "{error}"),
            Self::Durable(error) => write!(f, "{error}"),
        }
    }
}

impl From<ConversationError> for CanonicalCommitError {
    fn from(error: ConversationError) -> Self {
        Self::Conversation(error)
    }
}

impl<'a> AgentExecution<'a> {
    /// Creates an attempt execution over the given adapter, the owned attempt
    /// capability lease, the cancellation signal, the mandatory M4 context
    /// runtime, and the conversation tool runtime.
    ///
    /// The attempt capability lease is moved into the execution and pins the
    /// immutable capability snapshot (revision, `ToolRegistry` handle, Skill
    /// catalog, environment identities, and the effective `ToolEnvironment`)
    /// for the complete lifetime of this attempt: every model/tool cycle
    /// inside the attempt uses exactly that snapshot and never re-discovers
    /// Skills. Constructor failure drops the owned lease, and successful
    /// execution settlement drops it with the consumed execution. The
    /// execution cannot be constructed without a lease — there is no
    /// capability-free constructor.
    ///
    /// The conversation tool runtime binds the conversation identity, the
    /// canonical inbound mailbox, and the background registry together:
    /// the attempt must belong to the same conversation, otherwise the
    /// execution is rejected structurally. The loop drains exactly
    /// `tool_runtime.mailbox()` at every safe boundary, so background
    /// terminal notifications always enter the mailbox the Agent Loop
    /// drains.
    ///
    /// The context runtime is required: there is exactly one normal
    /// execution model — canonical history is always projected through the
    /// context engine, and Agent Status is composed whenever a pending fresh
    /// inbound turn exists. There is no no-context mode and no Agent Status
    /// disable flag.
    ///
    /// The [`AttemptLifecycle`] is required for the same reason: the loop
    /// always evaluates exactly one pre-step policy and always runs exactly
    /// one tool-result observation pass, so no code path branches on whether
    /// a seam is attached. [`AttemptLifecycle::inert`] is the identity
    /// configuration.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::ConversationMismatch`] when the request's
    /// conversation differs from the conversation tool runtime's
    /// conversation (and therefore its canonical mailbox).
    pub fn new(
        request: AgentExecutionRequest,
        capability: AttemptCapabilityLease,
        cancellation: &'a AgentCancellation,
        context_runtime: ContextRuntime,
        tool_runtime: &'a ConversationToolRuntime,
        lifecycle: AttemptLifecycle,
    ) -> Result<Self, MailboxError> {
        if tool_runtime.conversation_id() != &request.conversation_id {
            return Err(MailboxError::ConversationMismatch {
                expected: request.conversation_id.clone(),
                actual: tool_runtime.conversation_id().clone(),
            });
        }
        let store = tool_runtime.durable_store();
        Self::new_bound(
            request,
            capability,
            cancellation,
            context_runtime,
            tool_runtime,
            store,
            lifecycle,
        )
    }

    /// Completes construction over the full authority obtained from the
    /// conversation tool runtime's single composition binding. This helper
    /// is private so callers cannot pair an arbitrary store with a mailbox.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::ConversationMismatch`] when the request and
    /// tool runtime belong to different conversations, and
    /// [`MailboxError::Durable`] when the supplied authority belongs to a
    /// different conversation, cannot load its current head, or cannot
    /// initialize the standalone fixture history.
    fn new_bound(
        request: AgentExecutionRequest,
        capability: AttemptCapabilityLease,
        cancellation: &'a AgentCancellation,
        context_runtime: ContextRuntime,
        tool_runtime: &'a ConversationToolRuntime,
        store: std::sync::Arc<dyn ConversationStore>,
        lifecycle: AttemptLifecycle,
    ) -> Result<Self, MailboxError> {
        if tool_runtime.conversation_id() != &request.conversation_id {
            return Err(MailboxError::ConversationMismatch {
                expected: request.conversation_id.clone(),
                actual: tool_runtime.conversation_id().clone(),
            });
        }
        if store.conversation_id() != &request.conversation_id {
            return Err(MailboxError::Durable(
                crate::durable::ConversationStoreError::ConversationIdMismatch {
                    stored: store.conversation_id().clone(),
                    requested: request.conversation_id.clone(),
                },
            ));
        }
        let snapshot = capability.snapshot();
        if snapshot.conversation_id() != tool_runtime.conversation_id()
            || snapshot.workspace_root() != tool_runtime.workspace().root()
        {
            return Err(MailboxError::CapabilityOwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: tool_runtime.conversation_id().clone(),
                capability_workspace: snapshot.workspace_root().to_path_buf(),
                runtime_workspace: tool_runtime.workspace().root().to_path_buf(),
            });
        }
        let mut request = request;
        let conversation = core::mem::take(&mut request.conversation);
        // Standalone execution fixtures may provide a fresh store rather than
        // constructing a ConversationRuntime first. Initialize that store
        // once from the supplied current facts; normal runtime construction
        // has already established the immutable bootstrap identity.
        let head = store
            .load_head()
            .map_err(|error| MailboxError::Durable(error.clone()))?;
        if head.revision == crate::conversation::SurfaceRevision::INITIAL
            && head.active_message_ids.is_empty()
        {
            store
                .initialize(conversation.ledger().audit_records())
                .map_err(MailboxError::Durable)?;
        }
        Ok(Self {
            conversation,
            request,
            capability,
            cancellation,
            tool_runtime,
            store,
            state: ExecutionStateMachine::new(),
            durable_failure: None,
            durable_failure_kind: None,
            pending_continuation: None,
            continuation_owner: None,
            pending_fresh_inbound: None,
            context_runtime,
            accepted_context: None,
            lifecycle,
            deferred_context: Vec::new(),
            context_generation_serial: 0,
            observed: None,
            last_request_fingerprint: None,
            last_request_id: None,
            publication_policy: CoalescePolicy::default(),
            publication_clock: std::sync::Arc::new(SystemPublicationClock::new()),
            publication: None,
            publication_settlement_failed: false,
            observer: None,
            #[cfg(test)]
            continuation_pause: std::sync::Mutex::new(None),
            #[cfg(test)]
            start_boundary_pause: std::sync::Mutex::new(None),
            #[cfg(test)]
            tool_start_pause: std::sync::Mutex::new(None),
            turn: 0,
            terminal_emitted: false,
        })
    }

    /// Attaches the live observation seam.
    ///
    /// The observer is read-only: it observes emitted runtime facts,
    /// committed canonical messages, and composed Agent Status at their
    /// commit linearization points, and it never influences execution. It
    /// must be attached before [`AgentExecution::run`] to observe the whole
    /// attempt; attaching is optional, and the durable Event Journal remains
    /// available for historical reads regardless.
    pub fn observe(&mut self, observer: &'a dyn AgentExecutionObserver) {
        self.observer = Some(observer);
    }

    /// Installs the deterministic publication policy and clock of one attempt
    /// (Issue #108 regressions).
    ///
    /// Production always uses the default bounded policy and the monotonic
    /// system clock. A test installs an explicit byte threshold and a manually
    /// advanced clock so a coalescing or latency regression is decided by the
    /// policy alone and never by wall-clock timing.
    #[cfg(test)]
    pub(crate) fn install_publication_policy(
        &mut self,
        policy: CoalescePolicy,
        clock: std::sync::Arc<dyn PublicationClock>,
    ) {
        self.publication_policy = policy;
        self.publication_clock = clock;
    }

    /// Installs the test-only start-boundary pause (Issue #12, M9b
    /// deterministic race tests).
    #[cfg(test)]
    pub(crate) fn install_start_boundary_pause(&mut self, pause: test_sync::StartBoundaryPause) {
        *self
            .start_boundary_pause
            .lock()
            .expect("start boundary pause lock") = Some(pause);
    }

    /// Installs the test-only foreground tool-start pause for one attempt.
    #[cfg(test)]
    pub(crate) fn install_tool_start_pause(&mut self, pause: test_sync::ToolStartPause) {
        *self.tool_start_pause.lock().expect("tool start pause lock") = Some(pause);
    }

    /// Runs the attempt to its execution settlement candidate.
    ///
    /// The execution state machine is the settlement authority: the machine
    /// is settled (`complete()` for success, `fail()` for failure and
    /// cancellation) immediately before the single attempt terminal
    /// `RuntimeEvent` is attempted. A terminal Event Journal append failure
    /// does not fabricate an event; the result reports that durable failure
    /// separately.
    ///
    /// # Panics
    ///
    /// Panics only when the loop violates its own state-machine invariants;
    /// a durable terminal write failure is an explicit result, not a panic.
    pub async fn run(mut self) -> AgentExecutionResult {
        self.emit(RuntimeEvent::AttemptStarted {
            attempt_id: self.request.attempt_id.clone(),
        });
        let terminal = if let Err(error) = self.state.start() {
            Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            }
        } else if let Some(message) = self.durable_failure.clone() {
            Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            }
        } else {
            // The attempt's explicit fresh inbound trigger is pending until
            // the first successful model invocation observes it. A pure
            // continuation attempt has no pending trigger: the trigger makes
            // the execution mode explicit, so Agent Status can never be
            // silently suppressed.
            self.pending_fresh_inbound = match &self.request.initial_turn_trigger {
                InitialTurnTrigger::FreshInbound(fresh) => Some(fresh.clone()),
                InitialTurnTrigger::Continuation => None,
            };
            let mut terminal = None;
            while terminal.is_none() {
                // Generic Agent Loop cancellation checkpoint:
                // observable cancellation is checked before every model
                // turn begins — the first turn, every continuation
                // after a foreground tool turn, and every continuation
                // caused by a drained inbound batch. This is an
                // intentional pre-1.0 Agent Loop contract refinement:
                // mailbox attachment, mailbox contents, the context
                // runtime, and the provider protocol do not control
                // generic cancellation timing. When cancellation wins
                // here, no `TurnStarted`, no `ModelRequestStarted`, and
                // no adapter invocation happen for the next turn.
                //
                // The check never replaces a terminal outcome already
                // selected at a mailbox safe boundary: a successful
                // no-tool turn whose empty mailbox snapshot settled the
                // attempt as Completed exits this loop before the check
                // runs again, so a later cancellation or enqueue never
                // reopens or reclassifies the completed attempt.
                if self.cancellation.is_cancelled() {
                    terminal = Some(Terminal::Cancelled {
                        reason: self.cancellation.reason(),
                    });
                    break;
                }
                terminal = self.run_turn().await;
                // TEST-ONLY continuation boundary: the previous turn is
                // structurally complete (every tool result and every
                // mailbox drain/append of that turn is done) and the
                // loop is about to check cancellation again before the
                // next model turn. Tests park here to make cancellation
                // observable deterministically between turns.
                #[cfg(test)]
                if terminal.is_none()
                    && let Some(pause) = &self
                        .continuation_pause
                        .lock()
                        .expect("continuation pause lock")
                        .as_ref()
                {
                    pause.park_at_continuation_boundary();
                }
            }
            terminal.expect("the attempt must settle")
        };
        self.settle(&terminal);
        self.emit_terminal(&terminal);
        let outcome = terminal.outcome();
        AgentExecutionResult {
            attempt_id: self.request.attempt_id,
            outcome,
            terminal_state: self.state.state(),
            conversation: self.conversation,
            durable_failure: self.durable_failure,
            durable_failure_kind: self.durable_failure_kind,
        }
    }

    /// Settles the execution state machine for the computed terminal
    /// outcome, immediately before the terminal event is emitted.
    fn settle(&mut self, terminal: &Terminal) {
        let settlement = match terminal {
            Terminal::Completed { .. } => self.state.complete(),
            Terminal::Cancelled { .. } | Terminal::Failed { .. } => self.state.fail(),
        };
        settlement.expect("the execution state machine must accept the settlement");
    }

    /// Executes one turn and settles its publication stream exactly once.
    ///
    /// This is the one mutual-exclusion point of the three publication
    /// settlements. A turn that reached canonical acceptance already cleared
    /// its stream through the compound C transition; every other exit —
    /// cancellation, model failure, structural assembly rejection, preflight
    /// rejection, a durable failure — leaves the stream open here and it
    /// terminalizes as an audit. Canonical acceptance and audit
    /// terminalization can therefore never both happen for one stream.
    async fn run_turn(&mut self) -> Option<Terminal> {
        let terminal = self.run_turn_body().await;
        if !self.publication_settlement_failed
            && let Err(error) = self.settle_publication_audit()
        {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        terminal
    }

    /// Executes one turn: one model request, its tool calls, and their
    /// results. Returns the terminal outcome when the attempt settled.
    async fn run_turn_body(&mut self) -> Option<Terminal> {
        self.turn += 1;
        // A new primary step gets one fresh assembly generation. An overflow
        // retry stays inside this function and therefore retains the accepted
        // context below.
        self.accepted_context = None;
        let assistant_message_id = RequestIdentity {
            attempt_id: self.request.attempt_id.clone(),
            turn: TurnId::new(self.turn.to_string()),
            retry_number: 0,
        }
        .provisional_message_id();
        self.emit(RuntimeEvent::TurnStarted);
        if let Some(message) = self.durable_failure.clone() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        }

        let prepared = match self.prepare_model_turn().await {
            Ok(prepared) => prepared,
            Err(terminal) => return Some(terminal),
        };
        // The one cancellation-vs-start arbitration of this model turn:
        // the provider is invoked only after the durable start commit won.
        let request = match self.start_model_turn(prepared) {
            Ok(request) => request,
            Err(terminal) => return Some(terminal),
        };
        let mut invocation = match self
            .consume_invocation(request, &assistant_message_id)
            .await
        {
            Ok(invocation) => invocation,
            Err(terminal) => return Some(terminal),
        };

        // M4 bounded compact-and-retry: a recoverable context overflow does
        // not settle the attempt. The execution state remains an active
        // model-running state; no state-machine settlement and no attempt
        // terminal event are produced between the overflow and the retry.
        //
        // The retry budget is per model turn: `overflow_retries` is
        // turn-local, so every turn is entitled to its own
        // `MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN` retries and the
        // budget never persists across turns. The retry path is single-shot:
        // a retry that overflows again settles the attempt, so there is no
        // second retry inside any individual turn.
        let overflow_retries: u32 = 0;
        if let StreamTerminal::Failed { error } = &invocation.terminal
            && error.kind == ModelErrorKind::ContextWindowExceeded
            && overflow_retries < MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN
        {
            let retry_number = overflow_retries + 1;
            let overflow_error = error.clone();
            match self
                .retry_after_overflow(&overflow_error, retry_number)
                .await
            {
                Ok(retry_invocation) => {
                    // The successful retry replaces the complete invocation:
                    // the provisional identity, the assembler (and therefore
                    // the provisional content and tool calls of the failed
                    // request), and the terminal.
                    invocation = retry_invocation;
                }
                Err(terminal) => return Some(terminal),
            }
        }

        match invocation.terminal {
            StreamTerminal::Failed { error } => Some(Terminal::Failed {
                failure: AttemptFailure::Model { error },
            }),
            StreamTerminal::Completed {
                finish_reason,
                usage,
            } => {
                self.complete_turn(
                    invocation.message_id,
                    finish_reason,
                    usage,
                    invocation.assembler,
                )
                .await
            }
        }
    }

    /// Settles one completed model stream: assembly, usage folding,
    /// continuation retention, message commit, tool execution, and the
    /// turn-completion event. Returns the terminal outcome when the attempt
    /// settled.
    #[allow(clippy::too_many_lines)] // One P -> U -> C ordering, one place.
    async fn complete_turn(
        &mut self,
        assistant_message_id: MessageId,
        finish_reason: ModelFinishReason,
        usage: Option<ModelUsage>,
        assembler: ModelEventAssembler,
    ) -> Option<Terminal> {
        let turn_assembly = match assembler.finish(&finish_reason, usage) {
            Ok(assembly) => assembly,
            Err(error) => {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
        };
        // The pending fresh inbound turn is consumed by the first successful
        // model invocation, including a successful ToolCalls response: the
        // model has already observed the inbound turn, so the following
        // tool-only continuation carries no Agent Status unless a new
        // mailbox batch is drained later.
        self.pending_fresh_inbound = None;
        // The model request completion is reported with the canonical
        // final usage: the terminal event's reported usage, else the
        // latest usage update, never a sum of snapshots.
        let reported_usage = turn_assembly.usage;
        let Some(request_id) = self.last_request_id.clone() else {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: "a model stream completed with no started request".to_owned(),
                    },
                },
            });
        };
        // P: the provider outcome of this exact request becomes durable.
        self.emit(RuntimeEvent::ModelRequestCompleted {
            request_id,
            finish_reason: finish_reason.clone(),
            usage: reported_usage.clone(),
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Some(terminal);
        }
        // U: P is durable and provider completion is structurally accepted,
        // so the remaining publication payload and the publication terminal
        // marker commit in one transaction — and only then is the final
        // buffered payload released. This is the intentional tail-latency
        // cost of correct ordering; with nothing buffered, a terminal-only
        // frame commits and no visible text waits.
        if let Err(error) = self.commit_publication_terminal() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // A provider-reported input measurement applies only to the
        // exact projection the completed request used.
        if let Some(usage) = &reported_usage
            && let Some(fingerprint) = self.last_request_fingerprint.take()
        {
            self.observed = Some(ProviderObservedInput {
                fingerprint,
                input_tokens: usage.input_tokens,
            });
        }
        self.pending_continuation = turn_assembly.continuation;
        if self.pending_continuation.is_some() {
            self.continuation_owner = Some(assistant_message_id.clone());
        } else {
            self.continuation_owner = None;
        }
        let has_tool_calls = !turn_assembly.tool_calls.is_empty();
        if let Err(error) = self.state.model_finished(has_tool_calls) {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // M5 preflight boundary: every model-issued tool call of the turn
        // must resolve structurally (registry identity, execution-policy
        // resolution, runtime metadata extraction, business argument
        // validation) before the Assistant message is committed. An impossible
        // canonical identity mismatch or unregistered tool is a
        // runtime/model-stream contract failure and the Assistant message is
        // never committed. Business JSON Schema validation failures are
        // normal rejected result slots and do not fail the attempt.
        let preflight = match self.preflight_tool_calls(&turn_assembly.tool_calls) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
        };
        if let Err(error) =
            self.commit_assistant_message(&assistant_message_id, &turn_assembly.content)
        {
            return Some(self.commit_failure_terminal(
                "the assembled Assistant message cannot be committed",
                error,
            ));
        }
        if !has_tool_calls {
            self.emit(RuntimeEvent::TurnCompleted);
            if let Some(terminal) = self.durable_failure_terminal_from_state() {
                return Some(terminal);
            }
            // Safe boundary for a completed no-tool turn: the attempt may
            // settle only when the boundary snapshot observes no eligible
            // inbound work. A drained batch keeps the attempt running for
            // one further model turn, so a pending inbound message prevents
            // a successful Stop from settling before it is observed.
            return match self.safe_boundary_drain() {
                Ok(true) => None,
                Ok(false) => Some(Terminal::Completed { finish_reason }),
                Err(terminal) => Some(terminal),
            };
        }
        // The entire tool-result batch is structurally settled exactly once
        // inside `execute_tools` before this point returns: every logical
        // call of the committed batch receives exactly one canonical
        // attempt-facing result slot, committed in original model call
        // order. Attempt cancellation can still settle the attempt as
        // cancelled after the structurally complete result batch is
        // committed; no next model turn starts after cancellation.
        let settled = match self
            .execute_tools(&turn_assembly.tool_calls, preflight)
            .await
        {
            Ok(settled) => settled,
            Err(error) => {
                return Some(
                    self.commit_failure_terminal("a tool result cannot be committed", error),
                );
            }
        };
        // Cancellation observed before the observation phase begins wins
        // immediately: the batch is already structurally settled, so there is
        // no useful deferred model context to produce and no observation runs.
        if self.cancellation.is_cancelled() {
            return Some(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The immutable tool-result observation pass runs only here, after
        // the complete owning batch is structurally settled. Observer failure
        // therefore cannot split the batch or prevent a committed Assistant
        // tool-call message from receiving its complete canonical result
        // batch.
        if let Err(terminal) = self.run_tool_result_observations(&settled).await {
            return Some(terminal);
        }
        if let Err(error) = self.state.tools_finished() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // Safe boundary after a structurally complete tool turn: every
        // foreground call of the turn executed and every ToolMessage was
        // committed before this point. One finite mailbox drain may attach
        // an inbound batch to the continuation; the drain never splits the
        // tool-result batch.
        self.safe_boundary_drain().err()
    }

    /// Preflights every model-issued tool call of the turn.
    ///
    /// An impossible canonical identity mismatch or unregistered tool is a
    /// runtime/model-stream contract failure; business schema violations are
    /// normal [`PreflightOutcome::Rejected`] result slots.
    fn preflight_tool_calls(
        &self,
        calls: &[ToolCall],
    ) -> Result<Vec<PreflightOutcome>, RuntimeError> {
        let mut outcomes = Vec::with_capacity(calls.len());
        for call in calls {
            match self.tool_registry().preflight(call) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    return Err(match error {
                        crate::tools::executor::ToolPreflightError::UnknownTool { name } => {
                            RuntimeError::UnknownTool { name }
                        }
                        crate::tools::executor::ToolPreflightError::IdentityMismatch {
                            id,
                            name,
                        } => RuntimeError::ContractViolation {
                            message: format!(
                                "tool call identity mismatch: id {id} and name {name:?}                                      do not resolve to the same registered tool"
                            ),
                        },
                    });
                }
            }
        }
        Ok(outcomes)
    }

    /// The mailbox-specific safe boundary: exactly one finite inbound
    /// snapshot after the current turn is structurally complete.
    ///
    /// This function is inbound-boundary semantics only, separate from the
    /// generic Agent Loop cancellation checkpoint (which lives in `run()`
    /// before every model turn). The conversation tool runtime owns the one
    /// canonical mailbox/durable inbox of the conversation; the loop selects
    /// and adopts exactly that boundary, so background terminal
    /// notifications always enter the same durable path the Agent Loop
    /// adopts. With no pending items the snapshot observes no state and the
    /// function returns `Ok(false)`.
    ///
    /// Cancellation wins before selection: when cancellation is already
    /// observable, no selection/adoption happens, all pending items stay
    /// durably pending, and the attempt settles cancelled. Otherwise one
    /// finite watermark is selected and atomically adopted into the durable
    /// canonical ledger, then the complete batch is appended synchronously
    /// as distinct canonical `UserMessageBlock` values in inbound sequence
    /// order — the batch is never partially consumed and never requeued. The
    /// whole adopted batch becomes one new [`FreshInboundTurn`] in sequence
    /// order, so the next model request receives exactly one Agent Status
    /// snapshot targeting the final adopted message (the highest-sequence
    /// item).
    ///
    /// Returns `Ok(true)` when one complete batch was adopted, `Ok(false)`
    /// when the snapshot observed an empty inbox, and the attempt terminal
    /// when cancellation was observable before the snapshot.
    fn safe_boundary_drain(&mut self) -> Result<bool, Terminal> {
        let mailbox = self.tool_runtime.mailbox();
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // Selection freezes the finite watermark (non-destructive): an
        // acceptance that linearizes after this point can never join the
        // selected batch. A durable selection failure is a
        // durable-authority failure (Issue #63): the settled result carries
        // it to the coordinator.
        let batch = match mailbox.select_pending_batch() {
            Ok(Some(batch)) => batch,
            Ok(None) => return Ok(false),
            Err(error) => {
                return Err(self.durable_failure_terminal(
                    "the durable pending inbound inbox cannot be selected",
                    &error,
                ));
            }
        };
        // Prepare the canonical transition **before** the durable adoption
        // commit: validate every fallible in-memory condition now, so the
        // post-commit installation is infallible (Finding 2). The prepared
        // values bind each exact drained message. A validation failure leaves
        // every item pending and adopts nothing.
        let mut prepared = Vec::with_capacity(batch.items().len());
        for item in batch.items() {
            let block = crate::durable::inbox::canonical_block(item.message());
            match self.conversation.prepare_commit(&block) {
                Ok(commit) => prepared.push(commit),
                Err(error) => {
                    return Err(Terminal::Failed {
                        failure: AttemptFailure::Runtime {
                            error: RuntimeError::ContractViolation {
                                message: format!(
                                    "a drained inbound message cannot be prepared: {error}"
                                ),
                            },
                        },
                    });
                }
            }
        }
        let fresh = FreshInboundTurn::new(
            prepared
                .iter()
                .map(|commit| commit.message_id().clone())
                .collect(),
        )
        .map_err(|error| Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContractViolation {
                    message: format!(
                        "a selected inbound batch cannot form a fresh inbound turn: {error}"
                    ),
                },
            },
        })?;
        // Canonical adoption: the durable ledger append and the pending
        // removal commit in one transaction. A durable adoption failure is
        // a durable-authority failure (Issue #63): the settled result
        // carries it to the coordinator.
        if let Err(error) = mailbox.adopt_pending_batch(&batch) {
            return Err(
                self.durable_failure_terminal("a selected inbound batch cannot be adopted", &error)
            );
        }
        for commit in prepared {
            // Infallible: every adopted identity was validated above under
            // exclusive ownership of the conversation state.
            let block = commit.message().clone();
            self.conversation.install_prepared(commit);
            if let Some(observer) = self.observer {
                observer.observe_committed(&self.request.attempt_id, &block);
            }
        }
        self.pending_fresh_inbound = Some(fresh);
        Ok(true)
    }

    /// Prepares the next model turn without committing anything (Issue #12,
    /// M9b).
    ///
    /// This is the fallible preparation half of every primary model request:
    /// the current Surface flows through Context Assembly and the context
    /// engine into a staged projection, and the staged view is compiled into
    /// the frozen Request Snapshot and the exact provider-neutral request.
    /// The pending fresh inbound turn (when one exists) is sampled into one
    /// native context fact for the admitted primary step, together with the
    /// deferred post-tool proposals staged by the previous structurally
    /// settled tool batch. That accepted generation is reused throughout
    /// proactive compaction and overflow retry.
    ///
    /// The exact structure is:
    ///
    /// ```text
    /// Context Assembly (native + extension + deferred post-tool)
    ///     ↓ final immutable AcceptedContext
    /// PreStepPolicy → Enter | Reject(reason)
    ///     ↓
    /// stage request-scoped context (validate only — nothing commits)
    ///     ↓
    /// staged Effective System Prompt → staged projection
    ///     ↓ (reservation-aware compaction when the staged input overflows)
    /// PreparedModelTurn: validated context commits + RequestSnapshot + request
    /// ```
    ///
    /// A rejection, a policy failure, or a preparation failure leaves no
    /// canonical context, no Surface advancement, no frozen snapshot, and no
    /// provider request. Nothing request-scoped commits here: the one
    /// cancellation-vs-start arbitration in [`AgentExecution::start_model_turn`]
    /// decides whether this prepared turn ever starts. An overflow retry
    /// never re-enters this block: it reuses the already-admitted context
    /// generation, so the policy is evaluated exactly once per primary step.
    ///
    /// The generic cancellation check at the top is a cheap fast path that
    /// avoids wasted preparation work; it is **not** the cancellation
    /// arbitration (a cancellation that lands after it is decided by the
    /// start gate, exactly like one landing during assembly).
    #[allow(clippy::too_many_lines)]
    async fn prepare_model_turn(&mut self) -> Result<PreparedModelTurn, Terminal> {
        if let Some(message) = self.durable_failure.clone() {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        }
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        if self.accepted_context.is_some() {
            return Err(Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::Internal,
                "a primary model turn is prepared exactly once per turn",
            )));
        }
        // Compact already-committed history before staging this step's
        // dynamic context. This keeps an unobserved fresh inbound batch and
        // its newly staged Runtime fact from blocking a
        // complete-message compaction candidate that belongs to older
        // history. A second fit check below still rejects a request whose
        // newly staged context cannot fit on its own. This compaction is
        // independent conversation-history maintenance with its own
        // canonical commit; it reserves no staged tokens.
        let baseline_prompt = self.effective_system_prompt()?;
        let baseline_projection = self.current_projection(&baseline_prompt)?;
        if self
            .context_runtime
            .engine
            .should_compact(
                &baseline_projection,
                self.compaction_budgets().primary_output_budget,
            )
            .map_err(|error| Self::context_failure_terminal(&error))?
        {
            let must_cover = self.continuation_owner.clone();
            let fresh = self.pending_fresh_inbound.clone();
            self.perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                &baseline_prompt,
                None,
                &[],
            )
            .await?;
        }
        let status = match self.compose_status() {
            Ok(status) => status,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        let input = match self.contributor_input_snapshot() {
            Ok(input) => input,
            Err(terminal) => return Err(terminal),
        };
        // The deferred proposals of the previous structurally settled tool
        // batch enter the *same* final transient batch as every other
        // proposal, and are laned and given provenance by their producer
        // identity, not by their timing. There is exactly one
        // model-visible dynamic-context admission path: an observer has no
        // privileged committer role and cannot bypass the policy below.
        let deferred = core::mem::take(&mut self.deferred_context);
        let mut native = self.context_runtime.native_system.clone();
        native.agent_status = status;
        let accepted = self
            .context_runtime
            .assembly
            .assemble(&input, &native, &deferred)
            .await
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    error.to_string(),
                ))
            })?;
        // The typed pre-step policy boundary (Issue #56). It observes the
        // complete final proposal batch and returns Enter/Reject only. It
        // owns no cancellation, allocates no identity, and commits nothing;
        // a rejection settles the attempt strictly before the start
        // arbitration in `start_model_turn`.
        let policy = self.lifecycle.pre_step_policy();
        let decision = {
            let batch = PreStepBatch {
                attempt_id: &self.request.attempt_id,
                conversation_id: &self.request.conversation_id,
                turn: self.turn,
                surface_revision: self.conversation.revision(),
                context: &accepted,
            };
            policy.evaluate(&batch).await
        };
        match decision {
            Ok(PreStepDecision::Enter) => {}
            Ok(PreStepDecision::Reject { reason }) => {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::PreStepRejected { reason },
                    },
                });
            }
            Err(error) => {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::PreStepPolicyFailed {
                            message: error.message,
                        },
                    },
                });
            }
        }
        // Stage the request-scoped context: generation, canonical
        // identities, and in-memory validation only. Nothing commits until
        // the start arbitration wins.
        let staged_context = self.stage_context(accepted)?;
        let mut staged = self.stage_model_turn(0, &staged_context)?;
        let budgets = self.compaction_budgets();
        let should_compact = match self
            .context_runtime
            .engine
            .should_compact(&staged.projection, budgets.primary_output_budget)
        {
            Ok(value) => value,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        if should_compact {
            // The staged context is not yet canonical, so this compaction
            // plans over the committed Surface and evaluates every candidate
            // against the exact hypothetical post-compaction request: the
            // rewritten Surface plus the staged request-scoped context plus
            // the Effective System Prompt plus tools. The rewrite must leave
            // room for the context that the start transaction will append on
            // top of it. The compaction itself is an independent canonical
            // commit of conversation-history maintenance; if cancellation
            // wins the start arbitration afterwards, the compaction remains
            // valid and the staged context is discarded without a trace.
            let must_cover = self.continuation_owner.clone();
            let fresh = self.pending_fresh_inbound.clone();
            // `perform_compaction` owns the post-surface-rewrite
            // invalidation of the opaque provider continuation; no caller
            // clears it a second time.
            self.perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                &staged.effective_system_prompt,
                None,
                &staged_context,
            )
            .await?;
            // Restage over the rewritten Surface: the staged context blocks
            // are unchanged, the Surface revision and projection are not.
            staged = self.stage_model_turn(0, &staged_context)?;
            if !self
                .context_runtime
                .engine
                .fits_under_soft_limit(&staged.projection, budgets.primary_output_budget)
                .map_err(|error| Self::context_failure_terminal(&error))?
            {
                return Err(Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::CannotFit,
                    "the staged model turn still exceeds the soft input limit after compaction",
                )));
            }
        }
        self.finalize_model_turn(&staged_context, staged)
    }

    /// Composes the Agent Status value of the pending fresh inbound
    /// turn, sampling the runtime clock exactly once.
    ///
    /// With no pending fresh inbound turn there is no Agent Status. With a
    /// pending turn, the turn is validated against canonical history and the
    /// final message's persisted timestamp drives `inbound_message_time`;
    /// the composer produces the structured sections and the canonical
    /// renderer produces the bounded text that Context Assembly admits as a
    /// canonical Runtime context fact.
    ///
    /// # Errors
    ///
    /// Returns a context error for a fresh-inbound contract violation
    /// (`MalformedHistory`) or a failing status section provider
    /// (`StatusFailed`).
    fn compose_status(&self) -> Result<Option<String>, ContextError> {
        let Some(fresh) = &self.pending_fresh_inbound else {
            return Ok(None);
        };
        let active = self.conversation.active_messages().map_err(|error| {
            ContextError::new(ContextErrorKind::MalformedHistory, error.to_string())
        })?;
        fresh.validate_against(&active).map_err(|error| {
            ContextError::new(
                ContextErrorKind::MalformedHistory,
                format!("pending fresh inbound turn is inconsistent: {error}"),
            )
        })?;
        let target_message_id = fresh.last_message_id().clone();
        let inbound_message_time = self.inbound_time_of(&target_message_id).ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::MalformedHistory,
                format!(
                    "pending fresh inbound message {target_message_id} has no persisted timestamp"
                ),
            )
        })?;
        let context = crate::context::status::AgentStatusRenderContext {
            inbound_message_time,
            timezone: self.request.timezone,
            background: self.tool_runtime.background().active_snapshot(),
        };
        let status = self.context_runtime.status_composer.compose(&context)?;
        if let Some(observer) = self.observer {
            observer.observe_status(&AgentStatusObservation {
                attempt_id: self.request.attempt_id.clone(),
                turn: self.turn,
                target_message_id: target_message_id.clone(),
                status: status.clone(),
            });
        }
        Ok(Some(crate::context::status::render_agent_status(&status)))
    }

    /// The persisted timestamp of one committed inbound message.
    fn inbound_time_of(&self, message_id: &MessageId) -> Option<DateTime<Utc>> {
        match self.conversation.ledger().get(message_id) {
            Some(MessageBlock::User(user)) => user.timestamp,
            _ => None,
        }
    }

    /// Builds the staged view of one actual model request: a scratch
    /// conversation (the current durable head plus the not-yet-committed
    /// request-scoped context), its Effective System Prompt, the staged
    /// projection, the frozen Request Snapshot, and the exact
    /// provider-neutral request.
    ///
    /// Everything produced here is transient preparation: the scratch
    /// conversation shares no state with the canonical conversation, and
    /// nothing commits until the start arbitration wins.
    fn stage_model_turn(
        &self,
        retry_number: u32,
        staged_context: &[MessageBlock],
    ) -> Result<StagedModelTurn, Terminal> {
        let active = self.conversation.active_messages().map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        let mut scratch = ConversationState::from_durable_head(
            active,
            self.conversation.active_ids().to_vec(),
            self.conversation.revision(),
            self.conversation.surface().compaction_generation(),
        )
        .map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        for block in staged_context {
            // Infallible in practice: the staged messages were validated
            // when they were produced; a rejection is an internal contract
            // violation.
            scratch.commit(block.clone()).map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    format!("staged context cannot enter the scratch conversation: {error}"),
                ))
            })?;
        }
        let sections = self
            .accepted_context
            .as_ref()
            .map_or(&[][..], |accepted| accepted.system_sections.as_slice());
        let effective_system_prompt = render_effective_system_prompt(sections);
        let projection = self
            .context_runtime
            .engine
            .build_projection(
                &scratch,
                &self.tool_registry().model_definitions(),
                self.observed.as_ref(),
                &effective_system_prompt,
            )
            .map_err(|error| Self::context_failure_terminal(&error))?;
        let request = self.model_request_from_projection(&projection);
        let accepted = self.accepted_context.as_ref().ok_or_else(|| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::Internal,
                "model request built before context staging",
            ))
        })?;
        let primary = self.request.model.primary();
        let snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: self.request.attempt_id.clone(),
                turn: TurnId::new(self.turn.to_string()),
                retry_number,
            },
            projection.surface_revision,
            effective_system_prompt.clone(),
            accepted.system_sections.clone(),
            self.context_runtime.resource_revision,
            request.invocation.clone(),
            primary.context_window(),
            primary.reasoning_profile().cloned(),
            primary.reasoning_enabled(),
            request.tools.clone(),
            self.capability.snapshot().revision(),
            accepted.generation.clone(),
            request.continuation.clone(),
            staged_context
                .iter()
                .map(crate::conversation::message_id_of)
                .collect(),
        );
        Ok(StagedModelTurn {
            projection,
            effective_system_prompt,
            snapshot,
            request,
        })
    }

    /// Freezes one staged model turn into its final prepared value: every
    /// staged context block is validated against the canonical conversation
    /// state, so the post-commit installation is infallible (Issue #63,
    /// Finding 2). Nothing commits here.
    fn finalize_model_turn(
        &mut self,
        staged_context: &[MessageBlock],
        staged: StagedModelTurn,
    ) -> Result<PreparedModelTurn, Terminal> {
        let mut context = Vec::with_capacity(staged_context.len());
        for block in staged_context {
            context.push(self.conversation.prepare_commit(block).map_err(|error| {
                Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation {
                            message: format!("staged context cannot be validated: {error}"),
                        },
                    },
                }
            })?);
        }
        Ok(PreparedModelTurn {
            context,
            snapshot: staged.snapshot,
            request: staged.request,
            fingerprint: staged.projection.fingerprint(),
        })
    }

    /// The one cancellation-vs-start linearization point of every model
    /// turn (Issue #12, M9b): the first turn, every tool→model
    /// continuation, every recovered continuation, and every overflow retry
    /// reach exactly this boundary.
    ///
    /// The attempt's model-turn start gate serializes the cancellation
    /// observation against the durable start commit
    /// (`ConversationStore::commit_model_turn_start`: the request-scoped
    /// context, the immutable Request Snapshot, and the exact
    /// `ModelRequestStarted` fact in one transaction). Exactly one side
    /// wins:
    ///
    /// - cancellation linearized first ⇒ the prepared turn is discarded:
    ///   no provider invocation, no `ModelRequestStarted`, no started
    ///   Request Snapshot, no request-scoped context commit, and no start
    ///   claim left behind;
    /// - the durable start commit succeeded ⇒ the request has durably
    ///   started; the validated context installs infallibly, the start fact
    ///   is recorded, and only then may the provider be invoked. A
    ///   cancellation that raced the commit is post-start cancellation of
    ///   the now-started request;
    /// - the durable start commit failed ⇒ a durable-authority failure
    ///   (Issue #63): no provider invocation, no start fact, no partial
    ///   request-owned state, and the coordinator learns the durable
    ///   failure kind.
    fn start_model_turn(&mut self, prepared: PreparedModelTurn) -> Result<ModelRequest, Terminal> {
        #[cfg(test)]
        if let Some(pause) = self
            .start_boundary_pause
            .lock()
            .expect("start boundary pause lock")
            .as_ref()
        {
            pause.park_before_start_arbitration();
        }
        let store = std::sync::Arc::clone(&self.store);
        let arbitration = self.cancellation.arbitrate_model_turn_start(|| {
            #[cfg(test)]
            if let Some(pause) = self
                .start_boundary_pause
                .lock()
                .expect("start boundary pause lock")
                .as_ref()
            {
                pause.park_before_start_commit();
            }
            let context: Vec<MessageBlock> = prepared
                .context
                .iter()
                .map(|commit| commit.message().clone())
                .collect();
            store.commit_model_turn_start(&context, &prepared.snapshot, Utc::now())
        });
        let started = match arbitration {
            Ok(StartAdjudication::CancelledBeforeStart) => {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            Ok(StartAdjudication::Started(started)) => started,
            Err(error) => {
                // The durable start transaction failed: no start fact, no
                // request-scoped context, and no provider invocation
                // exist. This is a durable-authority failure (Issue #63),
                // never a context-preparation failure.
                self.durable_failure_kind = Some(DurableFailureKind::RequestStart);
                self.durable_failure = Some(format!(
                    "request start could not be committed durably: {error}"
                ));
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: format!(
                                "request start could not be committed durably: {error}"
                            ),
                        },
                    },
                });
            }
        };
        // The durable start commit won: the request-scoped context installs
        // infallibly (validated at preparation, still exact), the start fact
        // is recorded, and only after this point may the provider be
        // invoked.
        for commit in prepared.context {
            let block = commit.message().clone();
            self.conversation.install_prepared(commit);
            if let Some(observer) = self.observer {
                observer.observe_committed(&self.request.attempt_id, &block);
            }
        }
        self.record_persisted_event(started);
        self.last_request_fingerprint = Some(prepared.fingerprint);
        self.last_request_id = Some(prepared.snapshot.request_id.clone());
        let reconstructed = self
            .store
            .reconstruct_model_request(&prepared.snapshot.request_id)
            .map_err(|error| {
                self.durable_failure_kind = Some(DurableFailureKind::RequestStart);
                self.durable_failure = Some(format!(
                    "durable request reconstruction failed after start: {error}"
                ));
                Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: format!(
                                "durable request reconstruction failed after start: {error}"
                            ),
                        },
                    },
                }
            })?;
        if reconstructed != prepared.request {
            self.durable_failure_kind = Some(DurableFailureKind::RequestStart);
            self.durable_failure = Some(
                "durable request reconstruction differs from the live provider-neutral request"
                    .to_owned(),
            );
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore {
                        message:
                            "durable request reconstruction differs from the live provider-neutral request"
                                .to_owned(),
                    },
                },
            });
        }
        Ok(prepared.request)
    }

    /// The current finite projection of the Conversation Surface, or the
    /// terminal the attempt must settle with when the context plane failed.
    fn current_projection(
        &self,
        effective_system_prompt: &str,
    ) -> Result<ContextProjection, Terminal> {
        self.context_runtime
            .engine
            .build_projection(
                &self.conversation,
                &self.tool_registry().model_definitions(),
                self.observed.as_ref(),
                effective_system_prompt,
            )
            .map_err(|error| Self::context_failure_terminal(&error))
    }

    /// Renders the exact request-time Effective System Prompt solely from
    /// the attempt-pinned resource generation and accepted request sections.
    fn effective_system_prompt(&self) -> Result<String, Terminal> {
        if let Some(accepted) = &self.accepted_context {
            return Ok(render_effective_system_prompt(&accepted.system_sections));
        }
        let sections = self
            .context_runtime
            .assembly
            .system_sections(&self.context_runtime.native_system)
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    error.to_string(),
                ))
            })?;
        Ok(render_effective_system_prompt(&sections))
    }

    /// Creates the finite immutable input visible to all contributors.
    fn contributor_input_snapshot(&self) -> Result<ContributorInputSnapshot, Terminal> {
        let active = self.conversation.active_messages().map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        let claimed_ids = self
            .pending_fresh_inbound
            .as_ref()
            .map_or_else(Vec::new, |fresh| fresh.message_ids().to_vec());
        let claimed_inbound = active
            .iter()
            .filter(|message| {
                let id = crate::conversation::message_id_of(message);
                claimed_ids.iter().any(|claimed| claimed == &id)
            })
            .cloned()
            .collect();
        Ok(ContributorInputSnapshot {
            attempt_id: self.request.attempt_id.clone(),
            conversation_id: self.request.conversation_id.clone(),
            turn: self.turn,
            surface_revision: self.conversation.revision(),
            surface_ids: self.conversation.active_ids().to_vec(),
            claimed_inbound,
            workspace_root: self.capability.snapshot().workspace_root().to_path_buf(),
            capability_revision: self.capability.snapshot().revision(),
        })
    }

    /// The only dynamic-context staging path (Issue #12, M9b). Core assigns
    /// the generation, provenance, semantic kind, and canonical `MessageId`;
    /// contributors have no access to any of those authorities.
    ///
    /// Staging is transient preparation only: it validates every staged
    /// message in a scratch conversation (which makes identity allocation
    /// observe the already-staged messages) and records the accepted
    /// generation, but commits nothing. The staged blocks become canonical
    /// exactly when the model-turn start arbitration commits them inside
    /// the durable start transaction; if cancellation wins there, they are
    /// discarded without a trace and history never claims the model
    /// observed them.
    fn stage_context(
        &mut self,
        mut accepted: AcceptedContext,
    ) -> Result<Vec<MessageBlock>, Terminal> {
        self.context_generation_serial = self
            .context_generation_serial
            .checked_add(1)
            .expect("context generation cannot overflow");
        accepted.generation.id = self.context_generation_serial;
        let namespace = format!("{}-turn-{}", self.request.attempt_id, self.turn);
        let active = self.conversation.active_messages().map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        let mut scratch = ConversationState::from_durable_head(
            active,
            self.conversation.active_ids().to_vec(),
            self.conversation.revision(),
            self.conversation.surface().compaction_generation(),
        )
        .map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        let mut staged = Vec::with_capacity(accepted.user_messages.len());
        for context in &accepted.user_messages {
            let id = scratch.allocate_context_message_id(&namespace);
            let block = MessageBlock::User(crate::message::types::UserMessageBlock {
                id,
                content: context.content.clone(),
                source: context.source.clone(),
                kind: crate::message::types::InboundKind::Context(context.kind),
                timestamp: None,
            });
            scratch
                .commit(block.clone())
                .map_err(|error| Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation {
                            message: format!("staged context cannot be committed: {error}"),
                        },
                    },
                })?;
            staged.push(block);
        }
        self.accepted_context = Some(accepted);
        Ok(staged)
    }

    fn compaction_budgets(&self) -> crate::context::CompactionBudgets {
        self.context_runtime.compaction_budgets
    }

    /// The immutable `ToolRegistry` handle of the pinned capability snapshot.
    fn tool_registry(&self) -> &ToolRegistry {
        self.capability.snapshot().tool_registry()
    }

    /// The `AttemptFailed` terminal of a durable-authority failure: the
    /// durable Message Ledger / Pending Inbound Inbox rejected a required
    /// durable operation of the active attempt. The failure is recorded on
    /// the execution so the settled result carries it to the coordinator
    /// (Issue #63): after an active-attempt durable failure the runtime
    /// must not return to a false healthy state.
    fn durable_failure_terminal(
        &mut self,
        context: &str,
        error: &dyn core::fmt::Display,
    ) -> Terminal {
        let message = format!("{context}: {error}");
        self.durable_failure_kind = Some(DurableFailureKind::CanonicalCommit);
        self.durable_failure = Some(message.clone());
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::DurableStore { message },
            },
        }
    }

    fn durable_failure_terminal_from_state(&self) -> Option<Terminal> {
        self.durable_failure
            .clone()
            .map(|message| Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            })
    }

    /// Converts an already-recorded Event Journal failure into the canonical
    /// commit error used by the tool-batch path. This keeps the active attempt
    /// from continuing into a semantic write after an event could not be
    /// persisted.
    fn durable_failure_commit_error(&self) -> Option<CanonicalCommitError> {
        self.durable_failure.clone().map(|message| {
            CanonicalCommitError::Durable(crate::durable::inbox::ConversationStoreError::Storage(
                message,
            ))
        })
    }

    /// Maps one canonical commit failure to its honest terminal: a durable
    /// Message Ledger failure is a durable-authority failure (recorded for
    /// the coordinator), while an in-memory validation failure is a
    /// contract violation and never touches the durability record.
    fn commit_failure_terminal(&mut self, context: &str, error: CanonicalCommitError) -> Terminal {
        match error {
            CanonicalCommitError::Durable(error) => self.durable_failure_terminal(context, &error),
            CanonicalCommitError::Conversation(error) => Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: format!("{context}: {error}"),
                    },
                },
            },
        }
    }

    /// The `AttemptFailed` terminal of a context-plane failure that occurred
    /// while preparing model context **before any compaction began**: an
    /// invalid pending fresh-inbound state discovered during status
    /// composition or projection preparation, a failing Agent Status section
    /// provider, or a projection preparation failure that is not itself a
    /// compaction operation. These are never mislabeled as compaction
    /// failures: [`RuntimeError::ContextCompactionFailed`] is reserved for an
    /// actual proactive compaction pipeline failure.
    fn context_failure_terminal(error: &ContextError) -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::ContextPreparationFailed {
                    message: error.message.clone(),
                },
            },
        }
    }

    /// Runs one compaction: plan, summarize, verify progress and fit, and
    /// commit the canonical summary plus the Surface rewrite.
    ///
    /// `overflow` distinguishes the two callers: a proactive compaction
    /// failure settles as `AttemptFailed(Runtime(ContextCompactionFailed))`,
    /// while a compaction after a context overflow preserves the normalized
    /// overflow as the final model failure (`AttemptFailed(Model(overflow))`)
    /// with the compaction diagnostic carried by `CompactionFailed.error`.
    ///
    /// The compaction planning receives the pending fresh inbound turn (so
    /// unobserved fresh inbound can never be retired) and the exact Effective
    /// System Prompt of this admitted request preparation.
    ///
    /// Cancellation is observed before the compaction, raced (biased)
    /// against the pending summary, checked again before the semantic
    /// commit, and checked again before any retry by the callers: once
    /// cancellation is observable, no summary, no Ledger append, no Surface
    /// rewrite, and no retry progress may begin, and the pending summary
    /// future is dropped.
    ///
    /// `staged_request_context` carries the **staged, not-yet-committed**
    /// request-scoped context (Issue #12, M9b): when the compaction is
    /// decided during model-turn preparation, the context it must make room
    /// for is not yet on the canonical Surface (it commits only inside the
    /// durable model-turn start transaction). Every compaction candidate is
    /// evaluated against the exact hypothetical post-compaction request —
    /// the rewritten Surface plus this overlay plus the Effective System
    /// Prompt plus tools — through the same token estimator, never as a
    /// scalar token delta. Independent compactions pass an empty overlay.
    ///
    /// This is also the **one** ownership path that invalidates the opaque
    /// provider continuation: a successful incompatible Surface rewrite
    /// invalidates it exactly once, immediately after the semantic commit.
    /// A failed or cancelled compaction never clears it.
    async fn perform_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        effective_system_prompt: &str,
        overflow: Option<&ModelError>,
        staged_request_context: &[MessageBlock],
    ) -> Result<(), Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        self.emit(RuntimeEvent::CompactionStarted);
        if let Some(message) = self.durable_failure.clone() {
            return Err(self.compaction_failure(
                &ContextError::new(ContextErrorKind::Internal, message),
                overflow,
            ));
        }
        match self
            .run_compaction(
                must_cover_through,
                fresh_inbound,
                effective_system_prompt,
                staged_request_context,
            )
            .await
        {
            Ok(_completed) => {
                // The semantic commit already happened: the summary is a
                // Ledger fact and the new Surface revision exists. The
                // opaque provider continuation is now known to be
                // incompatible, and this is the single place that discards
                // it. The observed provider measurement belonged to the old
                // request context and is dropped with it.
                self.pending_continuation = None;
                self.continuation_owner = None;
                self.observed = None;
                // `commit_compaction` persisted and returned the exact
                // completion fact before this branch became observable.
            }
            // Cancellation never becomes a compaction failure: no
            // `CompactionFailed` event is emitted and the attempt settles
            // cancelled.
            Err(error) if error.kind == ContextErrorKind::Cancelled => {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            Err(error) => return Err(self.compaction_failure(&error, overflow)),
        }
        Ok(())
    }

    /// The cancellation-aware compaction pipeline: plan, summarize, verify
    /// progress, fit-check, and commit.
    ///
    /// The **semantic commit / linearization point** of compaction is the
    /// single `ConversationStore::commit_compaction` transaction. Before it
    /// the old Ledger, the old Surface, and the old continuation semantics are
    /// authoritative; after it the canonical
    /// `User(Runtime / CompactionSummary)` message exists in the Ledger, a
    /// new Surface revision exists in which that summary replaces the
    /// selected active span, every covered Ledger fact remains intact and
    /// addressable, and the provider continuation is known to be
    /// incompatible.
    ///
    /// Cancellation is observed before the compaction, raced (biased)
    /// against the pending summary, and checked again immediately before the
    /// semantic commit: once cancellation is observable, no summary, no
    /// canonical summary append, and no Surface rewrite happen, so no
    /// half-committed state can exist.
    async fn run_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        effective_system_prompt: &str,
        staged_request_context: &[MessageBlock],
    ) -> Result<CompletedCompaction, ContextError> {
        let tools = self.tool_registry().model_definitions();
        let cancellation = self.cancellation.signal();
        let result = execute_compaction(
            &mut self.conversation,
            &self.context_runtime,
            &self.request.conversation_id,
            self.store.as_ref(),
            &tools,
            self.observed.as_ref(),
            effective_system_prompt,
            &CompactionConstraints {
                must_cover_through,
                fresh_inbound,
                staged_request_context,
            },
            &cancellation,
            CompactionAttribution {
                attempt_id: Some(self.request.attempt_id.clone()),
                turn_id: Some(TurnId::new(self.turn.to_string())),
            },
        )
        .await;
        let completed = match result {
            Ok(completed) => completed,
            Err(CompactionExecutionError::Context(error)) => return Err(error),
            Err(CompactionExecutionError::Durable(error)) => {
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(format!(
                    "the compaction transition cannot be committed durably: {error}"
                ));
                return Err(ContextError::new(ContextErrorKind::Internal, error));
            }
        };
        // The committed runtime summary is a canonical Ledger fact, observed
        // at exactly the commit linearization point like every other commit.
        if let Some(observer) = self.observer {
            observer.observe_committed(&self.request.attempt_id, &completed.summary_block);
        }
        self.record_persisted_event(completed.persisted_event);
        Ok(CompletedCompaction)
    }

    /// Emits `CompactionFailed` with the diagnostic and returns the
    /// compaction terminal for this caller.
    ///
    /// A proactive compaction failure is an actual compaction pipeline
    /// failure and settles as
    /// `AttemptFailed(Runtime(ContextCompactionFailed { message }))`; after a
    /// context overflow the original normalized overflow is preserved as the
    /// final model failure with the compaction diagnostic carried by
    /// `CompactionFailed.error`. Neither path becomes a generic context
    /// preparation failure.
    fn compaction_failure(
        &mut self,
        error: &ContextError,
        overflow: Option<&ModelError>,
    ) -> Terminal {
        self.emit(RuntimeEvent::CompactionFailed {
            error: error.message.clone(),
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return terminal;
        }
        match overflow {
            Some(overflow) => Terminal::Failed {
                failure: AttemptFailure::Model {
                    error: overflow.clone(),
                },
            },
            None => Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContextCompactionFailed {
                        message: error.message.clone(),
                    },
                },
            },
        }
    }

    /// Consumes one model invocation: sends the request, assembles the
    /// provisional stream content under the given provisional identity, and
    /// returns the complete invocation (identity + assembler + terminal).
    async fn consume_invocation(
        &mut self,
        request: ModelRequest,
        provisional_message_id: &MessageId,
    ) -> Result<ModelInvocation, Terminal> {
        // The provider binding comes from the attempt's frozen snapshot: the
        // loop never resolves an adapter and never observes a later session
        // model change.
        let mut stream = self
            .request
            .model
            .primary()
            .adapter()
            .stream(request, self.cancellation.model_cancellation());
        let mut assembler = ModelEventAssembler::new();
        let terminal = match self
            .consume_model_stream(&mut assembler, provisional_message_id, &mut stream)
            .await
        {
            Ok(stream_terminal) => stream_terminal,
            Err(terminal) => return Err(terminal),
        };
        Ok(ModelInvocation {
            message_id: provisional_message_id.clone(),
            assembler,
            terminal,
        })
    }

    /// The bounded compact-and-retry path after a context overflow.
    ///
    /// The compaction must retire the continuation-owning turn completely
    /// (the constraint is passed to the context engine), the pending
    /// continuation is then invalidated, and the retry request uses the
    /// smaller projection with its own deterministic retry-specific
    /// provisional/committed message identity
    /// `{attempt}-agent-{turn}-retry-{retry_number}`.
    ///
    /// The retry is not a new admitted dynamic-context step. It reuses the
    /// already accepted context generation, status fact, Skill system section,
    /// and contributor output; only the Surface revision and request identity
    /// may change because compaction happened.
    ///
    /// The retry returns the complete retry invocation — provisional
    /// identity, assembler, and terminal together — so a successful retry
    /// replaces the failed invocation wholesale and the failed request's
    /// provisional content and tool calls are never committed or executed.
    ///
    /// If the retry also overflows, no second compaction occurs: the attempt
    /// settles with the second overflow error as its final model failure.
    async fn retry_after_overflow(
        &mut self,
        overflow_error: &ModelError,
        retry_number: u32,
    ) -> Result<ModelInvocation, Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The abandoned publication is a durable predecessor of this retry.
        // It must settle before compaction, a retry snapshot/start commit, or
        // the second adapter invocation can occur.
        if let Err(error) = self.settle_publication_audit() {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        let effective_system_prompt = self.effective_system_prompt()?;
        let must_cover = self.continuation_owner.clone();
        // ContextWindowExceeded is a rejected provider request, not evidence
        // that a successful model invocation observed the fresh inbound
        // batch. Keep the pending constraint through overflow compaction;
        // the retry reuses the already-admitted context generation without
        // rerunning assembly.
        let fresh = self.pending_fresh_inbound.clone();
        match self
            .perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                &effective_system_prompt,
                Some(overflow_error),
                &[],
            )
            .await
        {
            Ok(()) => {}
            Err(terminal) => return Err(terminal),
        }
        // The cancellation check here is a cheap fast path only: it avoids
        // emitting `ModelRetryScheduled` once cancellation is already
        // observable. The retry's actual cancellation arbitration is the
        // same `start_model_turn` gate every other model request uses.
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The successful compaction already invalidated the incompatible
        // opaque provider continuation exactly once, inside
        // `perform_compaction`; this path never clears it a second time.
        self.emit(RuntimeEvent::ModelRetryScheduled {
            attempt_number: retry_number,
            retry_delay_ms: None,
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        let retry_message_id = RequestIdentity {
            attempt_id: self.request.attempt_id.clone(),
            turn: TurnId::new(self.turn.to_string()),
            retry_number,
        }
        .provisional_message_id();
        // The retry is another actual provider request: it stages no new
        // request-scoped context (the original request's start transaction
        // already committed it), freezes its own Request Snapshot, and
        // passes through the one cancellation-vs-start arbitration exactly
        // like the first request of the turn.
        let staged = match self.stage_model_turn(retry_number, &[]) {
            Ok(staged) => staged,
            Err(terminal) => return Err(terminal),
        };
        let prepared = match self.finalize_model_turn(&[], staged) {
            Ok(prepared) => prepared,
            Err(terminal) => return Err(terminal),
        };
        let request = match self.start_model_turn(prepared) {
            Ok(request) => request,
            Err(terminal) => return Err(terminal),
        };
        self.consume_invocation(request, &retry_message_id).await
    }

    /// Consumes one model stream: emits runtime events for non-terminal
    /// model facts, feeds the assembler, and validates the canonical stream
    /// contract. Returns the stream terminal, or the attempt terminal when
    /// the attempt must settle before the stream finished.
    async fn consume_model_stream(
        &mut self,
        assembler: &mut ModelEventAssembler,
        assistant_message_id: &MessageId,
        stream: &mut ModelEventStream,
    ) -> Result<StreamTerminal, Terminal> {
        let mut stream_terminal = None;
        loop {
            // A quiet provider must not hold committed-for-release payload
            // hostage: while payload is buffered, the coalescer-owned
            // absolute deadline competes with the next provider chunk. With
            // an empty buffer there is nothing to flush and the loop simply
            // awaits the provider. Later chunks cannot restart that deadline.
            let next = if self.has_buffered_publication() {
                let latency_wait = self
                    .publication
                    .as_ref()
                    .and_then(|publication| publication.coalescer.latency_wait())
                    .expect("a buffered publication has an owned latency deadline");
                tokio::select! {
                    biased;
                    event = stream.next() => event,
                    () = self.cancellation.cancelled() => {
                        return Err(self.cancelled_terminal());
                    }
                    () = latency_wait => {
                        self.flush_publication();
                        if let Some(terminal) = self.durable_failure_terminal_from_state() {
                            return Err(terminal);
                        }
                        continue;
                    }
                }
            } else {
                stream.next().await
            };
            let Some(event) = next else { break };
            if stream_terminal.is_none() && self.cancellation.is_cancelled() {
                return Err(Terminal::Cancelled {
                    reason: self.cancellation.reason(),
                });
            }
            if let Err(error) = assembler.push(&event) {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
            match &event {
                ModelEvent::Completed {
                    finish_reason,
                    usage,
                } => {
                    stream_terminal = Some(StreamTerminal::Completed {
                        finish_reason: finish_reason.clone(),
                        usage: usage.clone(),
                    });
                }
                ModelEvent::Failed { error } => {
                    if let Some(request_id) = self.last_request_id.clone() {
                        self.emit(RuntimeEvent::ModelRequestFailed {
                            request_id,
                            error: error.clone(),
                        });
                    }
                    if let Some(terminal) = self.durable_failure_terminal_from_state() {
                        return Err(terminal);
                    }
                    stream_terminal = Some(StreamTerminal::Failed {
                        error: error.clone(),
                    });
                }
                ModelEvent::Started => {
                    // The provider stream physically began: the publication
                    // stream opens durably before any of its output can be
                    // staged, so a crash always leaves either no stream at
                    // all or a stream recovery can classify.
                    self.open_publication(assistant_message_id);
                    if let Some(terminal) = self.durable_failure_terminal_from_state() {
                        return Err(terminal);
                    }
                }
                _ => {
                    if stream_terminal.is_none() {
                        self.publish_model_event(&event);
                        if let Some(terminal) = self.durable_failure_terminal_from_state() {
                            return Err(terminal);
                        }
                    }
                }
            }
        }
        let Some(stream_terminal) = stream_terminal else {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: "model stream ended without a terminal event".to_owned(),
                    },
                },
            });
        };
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        Ok(stream_terminal)
    }

    /// Executes the turn's tool calls through deterministic scheduling
    /// phases and commits the structurally complete result batch.
    ///
    /// Scheduling interprets [`ToolConcurrencyPolicy`] per registered tool:
    /// a `Sequential` invocation is an exclusive scheduling barrier, and
    /// adjacent `Parallel` invocations execute concurrently as one group.
    /// A background call is settled for the originating attempt when its
    /// background dispatch is accepted, not when the detached work
    /// terminates, so a sequential background call blocks later scheduling
    /// only through its dispatch-acceptance point.
    ///
    /// The structural invariant: once the valid Assistant tool-call message is
    /// committed, its entire tool-result batch is settled exactly once.
    /// Every call slot receives exactly one attempt-facing result
    /// (success/failure/cancellation/timeout/validation rejection/accepted
    /// background), canonical `ToolMessageBlock`s are committed in original
    /// model call order, and the batch never splits on cancellation. If
    /// attempt cancellation wins during the batch: in-flight cancellable
    /// foreground executions observe the attempt signal and physically
    /// settle, unstarted calls receive cancelled result slots, committed
    /// background executions stay conversation-owned, prepared-but-
    /// uncommitted dispatches roll back, and the complete batch commits in
    /// call order.
    ///
    /// The returned [`SettledCall`] values are immutable copies of exactly
    /// the committed facts, in canonical call order. They are the only input
    /// of the tool-result observation pass, which runs strictly after this
    /// function returns and therefore cannot influence structural settlement.
    #[allow(clippy::too_many_lines)] // one coherent scheduling/commit pipeline
    async fn execute_tools(
        &mut self,
        calls: &[ToolCall],
        preflight: Vec<PreflightOutcome>,
    ) -> Result<Vec<SettledCall>, CanonicalCommitError> {
        let mut slots: Vec<CallSlot> = calls
            .iter()
            .cloned()
            .zip(preflight)
            .map(|(call, outcome)| match outcome {
                PreflightOutcome::Ready(prepared) => CallSlot {
                    call,
                    tool_id: prepared.invocation.tool_id.clone(),
                    origin: prepared.origin.clone(),
                    prepared: Some(prepared),
                    result: None,
                    started: false,
                    progress: Vec::new(),
                },
                PreflightOutcome::Rejected {
                    tool_id,
                    origin,
                    error,
                } => CallSlot {
                    call,
                    tool_id,
                    origin,
                    prepared: None,
                    result: Some(failed_result(&error)),
                    started: false,
                    progress: Vec::new(),
                },
            })
            .collect();
        // Resolve every ready call through the one pre-tool policy boundary
        // before scheduling any executor. This is a deliberately strong
        // parallel-batch contract: all policy/interaction decisions for the
        // committed batch settle in canonical call order before the existing
        // sequential/parallel start frontier advances. The original
        // PreparedInvocation remains in every slot unchanged.
        self.resolve_pre_tool_decisions(&mut slots).await;
        let mut index = 0;
        while index < slots.len() {
            if self.cancellation.is_cancelled() {
                break;
            }
            match group_at(&slots, index) {
                Group::Trivial => {
                    index += 1;
                }
                Group::Sequential => {
                    if slots[index].result.is_none() {
                        slots[index].started = true;
                        self.emit(RuntimeEvent::ToolExecutionStarted {
                            tool_call_id: slots[index].call.id.clone(),
                            tool_id: slots[index].tool_id.clone(),
                        });
                        #[cfg(test)]
                        self.park_after_tool_start();
                        if let Some(error) = self.durable_failure_commit_error() {
                            return Err(error);
                        }
                        let invocation = slots[index]
                            .prepared
                            .as_ref()
                            .expect("unsettled slots are preflighted")
                            .invocation
                            .clone();
                        let (_, result, progress) = self.run_single_call(index, invocation).await;
                        slots[index].result = Some(result);
                        slots[index].progress = progress;
                    }
                    index += 1;
                }
                Group::Parallel => {
                    // Execution-start facts are emitted before any future is
                    // created, so the loop owns `&mut self` emission and the
                    // shared `&self` borrows of the spawned futures never
                    // conflict.
                    let end = parallel_group_end(&slots, index);
                    for slot in &mut slots[index..end] {
                        // Cancellation can be requested by a synchronous
                        // start-event observer while this group is being
                        // announced. Stop the logical start frontier at the
                        // first observed cancellation; the remaining slots
                        // are filled by the canonical cancellation pass and
                        // never acquire an execution future.
                        if self.cancellation.is_cancelled() {
                            break;
                        }
                        if slot.result.is_none() {
                            slot.started = true;
                            self.emit(RuntimeEvent::ToolExecutionStarted {
                                tool_call_id: slot.call.id.clone(),
                                tool_id: slot.tool_id.clone(),
                            });
                            #[cfg(test)]
                            self.park_after_tool_start();
                        }
                    }
                    if let Some(error) = self.durable_failure_commit_error() {
                        return Err(error);
                    }
                    let mut futures = futures_util::stream::FuturesUnordered::new();
                    for (slot_index, slot) in slots[index..end].iter().enumerate() {
                        if slot.started {
                            let invocation = slot
                                .prepared
                                .as_ref()
                                .expect("unsettled slots are preflighted")
                                .invocation
                                .clone();
                            futures.push(Box::pin(
                                self.run_single_call(index + slot_index, invocation),
                            ));
                        }
                    }
                    let mut remaining = futures.len();
                    if remaining > 0 {
                        loop {
                            if self.cancellation.is_cancelled() {
                                break;
                            }
                            tokio::select! {
                                biased;
                                () = self.cancellation.cancelled() => break,
                                Some((slot_index, result, progress)) = futures.next() => {
                                    slots[slot_index].result = Some(result);
                                    slots[slot_index].progress = progress;
                                    remaining -= 1;
                                    if remaining == 0 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // After cancellation wins, every in-flight foreground
                    // execution still settles: executors observe the attempt
                    // signal in their context and must settle their external
                    // work. The futures are awaited to completion, never
                    // dropped with external work abandoned.
                    while let Some((slot_index, result, progress)) = futures.next().await {
                        slots[slot_index].result = Some(result);
                        slots[slot_index].progress = progress;
                    }
                    index = end;
                }
            }
        }
        // Cancellation fill: every not-yet-started foreground call receives
        // a cancelled result slot, so the committed batch covers every
        // logical call exactly once.
        if self.cancellation.is_cancelled() {
            for slot in &mut slots {
                if slot.result.is_none() {
                    slot.result = Some(cancelled_result(self.cancellation.reason()));
                }
            }
        }
        // Canonical batch commit in original model call order. Progress
        // facts precede their completion event; the completion events
        // themselves are committed in canonical order regardless of physical
        // completion order.
        //
        // The **structural settlement point of the whole batch** is the one
        // atomic `commit_tool_result_batch` call: either every logical call
        // of the committed Assistant tool-call message owns exactly one
        // canonical `ToolMessage` (in original model call order) or none of
        // them becomes canonical. A durable failure of one member can never
        // leave a partial batch behind. The settled facts collected here are
        // immutable copies of exactly what was committed, and they are the
        // only input of the observation pass.
        let mut blocks = Vec::with_capacity(slots.len());
        let mut result_slots = Vec::with_capacity(slots.len());
        for (batch_position, slot) in slots.iter().enumerate() {
            let result = slot.result.clone().expect("every call slot settles");
            for event in &slot.progress {
                self.emit(event.clone());
                if let Some(error) = self.durable_failure_commit_error() {
                    return Err(error);
                }
            }
            if slot.started {
                self.emit(RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: slot.call.id.clone(),
                    tool_id: slot.tool_id.clone(),
                    result: result.clone(),
                });
                if let Some(error) = self.durable_failure_commit_error() {
                    return Err(error);
                }
            }
            let block = MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!(
                    "{}-tool-{}-{}",
                    self.request.attempt_id, self.turn, slot.call.id
                )),
                tool_call_id: slot.call.id.clone(),
                tool_id: slot.tool_id.clone(),
                result: result.clone(),
            });
            blocks.push(block);
            result_slots.push((batch_position, slot, result));
        }
        self.commit_tool_result_batch(&blocks)?;
        let settled = result_slots
            .into_iter()
            .map(|(batch_position, slot, result)| SettledCall {
                batch_position,
                call_id: slot.call.id.clone(),
                tool_id: slot.tool_id.clone(),
                origin: slot.origin.clone(),
                // The invocation facts are copied from the one
                // `PreparedInvocation` that executed, and the canonical
                // identity/origin are the slot's, so no second stored identity
                // can disagree with the registry. A preflight-rejected call
                // never resolved an invocation and exposes none.
                invocation: slot
                    .prepared
                    .as_ref()
                    .map(|prepared| ObservedToolInvocation {
                        tool_id: slot.tool_id.clone(),
                        origin: slot.origin.clone(),
                        mode: prepared.invocation.mode,
                        arguments: prepared.invocation.arguments.clone(),
                    }),
                result,
            })
            .collect();
        self.emit(RuntimeEvent::TurnCompleted);
        if let Some(error) = self.durable_failure_commit_error() {
            return Err(error);
        }
        Ok(settled)
    }

    /// Resolves the typed pre-tool decision of every preflight-ready call.
    ///
    /// The policy sees only an immutable view of the exact registry-resolved
    /// facts. `Ask` delegates the wait to the attempt's concrete native
    /// interaction binding, while the Agent Loop remains the owner of
    /// execution and cancellation. A provider-unavailable interaction and a
    /// policy error fail closed as `Denied` only when no post-await
    /// cancellation is observable; an owner cancellation produces the
    /// existing cancelled result slot and closes the later tool-start
    /// frontier.
    async fn resolve_pre_tool_decisions(&self, slots: &mut [CallSlot]) {
        let policy = self.lifecycle.pre_tool_policy();
        for slot in slots {
            if slot.result.is_some() {
                continue;
            }
            if self.cancellation.is_cancelled() {
                slot.result = Some(cancelled_result(self.cancellation.reason()));
                continue;
            }
            let prepared = slot
                .prepared
                .as_ref()
                .expect("a policy decision requires a preflight-ready invocation");
            let invocation = &prepared.invocation;
            let view = PreToolView {
                conversation_id: &self.request.conversation_id,
                attempt_id: &self.request.attempt_id,
                turn: self.turn,
                call_id: &slot.call.id,
                tool_id: &invocation.tool_id,
                tool_name: &invocation.tool_name,
                origin: &prepared.origin,
                mode: invocation.mode,
                arguments: &invocation.arguments,
                approval_policy: prepared.approval,
            };
            // A policy future is allowed to settle, but its result is not
            // consumed after cancellation becomes observable at this
            // extension boundary. This single checkpoint applies uniformly
            // to Allow, Deny, Ask, and policy errors.
            let raw_decision = policy.evaluate(&view).await;
            if self.cancellation.is_cancelled() {
                slot.result = Some(cancelled_result(self.cancellation.reason()));
                continue;
            }
            let decision = match raw_decision {
                Ok(decision) => decision,
                Err(error) => PreToolDecision::Deny {
                    reason: format!("pre-tool policy failed closed: {}", error.message),
                },
            };
            let resolution = match decision {
                PreToolDecision::Allow => PreToolResolution::Allow,
                PreToolDecision::Deny { reason } => PreToolResolution::Denied(reason),
                PreToolDecision::Ask { reason } => {
                    let facts = view.approval_facts(reason);
                    let outcome = self
                        .lifecycle
                        .request_approval(
                            self.request.attempt_id.clone(),
                            facts,
                            self.cancellation.execution_cancellation(),
                        )
                        .await;
                    // The interaction terminal winner owns the rendezvous,
                    // but it never grants execution authority. Apply the
                    // same post-await cancellation precedence before the
                    // Answered/Deny/Unavailable value is consumed.
                    if self.cancellation.is_cancelled() {
                        PreToolResolution::Cancelled(self.cancellation.reason())
                    } else {
                        match outcome {
                            InteractionOutcome::Answered { response } => match response {
                                InteractionResponse::Approval { decision } => match decision {
                                    ApprovalDecision::Allow => PreToolResolution::Allow,
                                    ApprovalDecision::Deny { reason } => {
                                        PreToolResolution::Denied(reason)
                                    }
                                },
                                InteractionResponse::Question { .. } => PreToolResolution::Denied(
                                    "approval interaction returned a Question response".to_owned(),
                                ),
                            },
                            InteractionOutcome::Cancelled { reason } => {
                                PreToolResolution::Cancelled(reason)
                            }
                            InteractionOutcome::Unavailable => PreToolResolution::Denied(
                                "interaction provider unavailable; approval failed closed"
                                    .to_owned(),
                            ),
                        }
                    }
                }
            };
            slot.result = match resolution {
                PreToolResolution::Allow => None,
                PreToolResolution::Denied(reason) => Some(denied_result(&reason)),
                PreToolResolution::Cancelled(reason) => Some(cancelled_result(reason)),
            };
        }
    }

    /// Runs the immutable tool-result observation pass of one structurally
    /// settled tool batch and stages its bounded deferred context proposals.
    ///
    /// Invocation order is the canonical `ToolCall` batch order (never
    /// physical completion order) and, within one call, the registered
    /// observers' logical identity order. The staged proposals therefore keep
    /// the canonical deferred order
    /// `(ToolCall batch position, producer identity, proposal FIFO)`, with no
    /// registration-order term.
    ///
    /// # The transaction boundary
    ///
    /// Every observer return value is validated **before** anything is staged:
    /// its proposal count against the established
    /// [`MAX_PROPOSALS_PER_CONTRIBUTOR`] bound, the running total against
    /// [`MAX_DEFERRED_CONTEXT_PROPOSALS`], and every proposal body against the
    /// same bounded content contract Context Assembly applies. An oversized
    /// observation can therefore never be appended to the attempt buffer and
    /// discovered one step later.
    ///
    /// The pass is transactional: any failure — a failing observer, a bound
    /// violation, invalid content, or observable cancellation — discards every
    /// proposal of this pass *and* clears the attempt's deferred buffer, so no
    /// partial deferred state survives. The already-committed Assistant
    /// tool-call message and its complete canonical `ToolMessage` batch are
    /// untouched either way; they were committed before this function is
    /// called.
    ///
    /// # Cancellation precedence
    ///
    /// The observer receives no cancellation handle; the loop keeps
    /// cancellation ownership. A bounded observation that is already running
    /// settles rather than being dropped, but observable cancellation is
    /// checked before each observer starts and again once it settles, before
    /// its return value is consumed. Once cancellation is observable, no later
    /// observer starts and neither an observer's success nor its failure can
    /// decide the terminal outcome.
    async fn run_tool_result_observations(
        &mut self,
        settled: &[SettledCall],
    ) -> Result<(), Terminal> {
        let observers = self.lifecycle.tool_result_observers().to_vec();
        if observers.is_empty() {
            return Ok(());
        }
        match self.observe_settled_batch(&observers, settled).await {
            Ok(staged) => {
                self.deferred_context.extend(staged);
                Ok(())
            }
            Err(terminal) => {
                // Nothing of this pass was staged, and no earlier pass may
                // survive a failed or cancelled one: the attempt settles
                // terminally, and it settles with an empty deferred buffer.
                self.deferred_context.clear();
                Err(terminal)
            }
        }
    }

    /// The pass body: one observation per (settled call, bound observer),
    /// validated at its transaction boundary and accumulated into a
    /// pass-local buffer that is never visible to the attempt on failure.
    ///
    /// Cancellation is checked at two points around every observation, and
    /// both belong to the Agent Loop alone:
    ///
    /// ```text
    /// cancellation check          ← an observer never starts after this
    /// await observer
    /// cancellation check          ← wins over the observer's Ok *or* Err
    /// consume result, validate, stage
    /// ```
    ///
    /// An in-flight bounded observation is allowed to settle rather than being
    /// dropped, but once cancellation is observable it decides the terminal
    /// outcome: a later observer never starts, and an observer's failure can
    /// no longer become `ToolResultObservationFailed`.
    async fn observe_settled_batch(
        &self,
        observers: &[super::lifecycle::RegisteredToolResultObserver],
        settled: &[SettledCall],
    ) -> Result<Vec<DeferredContextProposal>, Terminal> {
        let already_staged = self.deferred_context.len();
        let mut staged: Vec<DeferredContextProposal> = Vec::new();
        for call in settled {
            for registered in observers {
                if self.cancellation.is_cancelled() {
                    return Err(self.cancelled_terminal());
                }
                let observation = ToolResultObservation {
                    attempt_id: &self.request.attempt_id,
                    conversation_id: &self.request.conversation_id,
                    turn: self.turn,
                    batch_position: call.batch_position,
                    call_id: &call.call_id,
                    tool_id: &call.tool_id,
                    origin: &call.origin,
                    invocation: call.invocation.as_ref(),
                    result: &call.result,
                };
                let observed = registered
                    .observer()
                    .observe_tool_result(&observation)
                    .await;
                // Cancellation that became observable while this bounded
                // observation was in flight wins here, before the return value
                // is consumed. This is why an observer that fails during an
                // already-cancelled attempt cannot convert cancellation into
                // `ToolResultObservationFailed`.
                if self.cancellation.is_cancelled() {
                    return Err(self.cancelled_terminal());
                }
                let proposals = observed.map_err(|error| Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ToolResultObservationFailed {
                            message: error.message,
                        },
                    },
                })?;
                // The transaction boundary. Nothing below has touched the
                // attempt's deferred buffer yet, and nothing will until the
                // whole pass validates.
                if proposals.len() > MAX_PROPOSALS_PER_CONTRIBUTOR {
                    return Err(Self::deferred_rejected(format!(
                        "deferred-context producer {:?} returned {} proposals for call {}, \
                         above the bounded proposal limit {MAX_PROPOSALS_PER_CONTRIBUTOR}",
                        registered.producer(),
                        proposals.len(),
                        call.call_id
                    )));
                }
                if already_staged + staged.len() + proposals.len() > MAX_DEFERRED_CONTEXT_PROPOSALS
                {
                    return Err(Self::deferred_rejected(format!(
                        "the observation pass of turn {} would stage more than \
                         {MAX_DEFERRED_CONTEXT_PROPOSALS} deferred proposals",
                        self.turn
                    )));
                }
                for proposal in proposals {
                    validate_user_message_proposal(&proposal).map_err(|error| {
                        Self::deferred_rejected(format!(
                            "deferred-context producer {:?} proposed invalid context for \
                             call {}: {error}",
                            registered.producer(),
                            call.call_id
                        ))
                    })?;
                    staged.push(DeferredContextProposal {
                        // The producer comes from the binding, never from the
                        // observer's return value — and it is still only a
                        // reference until Context Assembly resolves it.
                        producer: registered.producer().clone(),
                        proposal,
                    });
                }
            }
        }
        Ok(staged)
    }

    /// The attempt's cancellation terminal, sampled at an observation
    /// checkpoint.
    fn cancelled_terminal(&self) -> Terminal {
        Terminal::Cancelled {
            reason: self.cancellation.reason(),
        }
    }

    /// The terminal of a deferred batch rejected at the transaction boundary.
    fn deferred_rejected(message: String) -> Terminal {
        Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::DeferredContextRejected { message },
            },
        }
    }

    /// Runs one logical tool call of the batch.
    ///
    /// Foreground invocations race against attempt cancellation (biased):
    /// when cancellation is already observable, no new execution progress
    /// begins, and an in-flight execution settles by observing the attempt
    /// signal in its context. Background invocations are dispatched through
    /// the conversation background registry's ownership commit. The slot
    /// index is returned so physical completion order can be recorded while
    /// canonical results remain model-call ordered.
    async fn run_single_call(
        &self,
        call_index: usize,
        invocation: ToolInvocation,
    ) -> (usize, ToolExecutionResult, Vec<RuntimeEvent>) {
        let (result, progress) = match invocation.mode {
            ToolInvocationMode::Foreground => self.run_foreground(&invocation).await,
            ToolInvocationMode::Background => (self.dispatch_background(&invocation), Vec::new()),
        };
        (call_index, result, progress)
    }

    #[cfg(test)]
    fn park_after_tool_start(&self) {
        if let Some(pause) = self
            .tool_start_pause
            .lock()
            .expect("tool start pause lock")
            .as_ref()
        {
            pause.park();
        }
    }

    /// Runs one foreground invocation against attempt cancellation.
    ///
    /// The execution receives an `ExecutionCancellation` view of the
    /// attempt's signal in its context. Native foreground work derives child
    /// signals from that view, so observable attempt cancellation physically
    /// reaches the subordinate operation without handing it cancellation
    /// authority over the attempt. Cancelled results produced while attempt
    /// cancellation is observable are normalized to the attempt's reason.
    async fn run_foreground(
        &self,
        invocation: &ToolInvocation,
    ) -> (ToolExecutionResult, Vec<RuntimeEvent>) {
        let executor = self.tool_registry().executor(&invocation.tool_id);
        let buffer =
            ForegroundProgressBuffer::new(invocation.call_id.clone(), invocation.tool_id.clone());
        let context = ToolExecutionContext::new(
            &self.request.conversation_id,
            None,
            self.cancellation.execution_cancellation(),
            self.tool_runtime.workspace(),
            &buffer,
            self.tool_runtime.artifacts(),
            self.tool_runtime.tool_output(),
            self.capability.snapshot().effective_environment(),
        );
        let context = match self.lifecycle.native_question_requester(
            self.request.attempt_id.clone(),
            self.cancellation.execution_cancellation(),
            self.turn,
        ) {
            Some(requester) => context.with_question_requester(requester),
            None => context,
        };
        let future = executor.execute(invocation.clone(), context);
        tokio::pin!(future);
        let mut result = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => future.as_mut().await,
            result = future.as_mut() => result,
        };
        if self.cancellation.is_cancelled()
            && matches!(result.status, ToolExecutionStatus::Cancelled { .. })
        {
            result.status = ToolExecutionStatus::Cancelled {
                reason: self.cancellation.reason(),
            };
        }
        let progress_events = buffer.take();
        (result, progress_events)
    }

    /// Dispatches one background invocation through the conversation-owned
    /// background registry.
    ///
    /// The dispatch is two-stage: prepare allocates the deterministic
    /// execution id and parks the runner behind its commit gate; commit
    /// performs the final attempt-cancellation checkpoint and produces the
    /// accepted result exactly when conversation ownership commits. A
    /// rolled-back dispatch produces a cancelled result slot for the
    /// originating attempt, never a detached execution.
    fn dispatch_background(&self, invocation: &ToolInvocation) -> ToolExecutionResult {
        let executor = self.tool_registry().executor(&invocation.tool_id);
        // The attempt's effective ToolEnvironment is captured at prepare
        // time, before the background ownership commit: the detached
        // execution retains exactly this immutable environment for its
        // whole lifetime, even after this attempt terminates and later
        // revisions activate.
        let environment = self.capability.snapshot().effective_environment().clone();
        let Some(mcp_leases) = self.capability.mcp_leases() else {
            return failed_result("the admitted MCP generation is already physically retired");
        };
        match self
            .tool_runtime
            .background()
            .prepare_dispatch_with_mcp_leases(invocation, &executor, environment, mcp_leases)
        {
            Ok(prepared) => {
                match self
                    .tool_runtime
                    .background()
                    .commit_dispatch(prepared, &self.cancellation.signal())
                {
                    Ok(BackgroundDispatchOutcome::Accepted { result, .. }) => result,
                    // A rolled-back dispatch (attempt cancellation won at
                    // the ownership boundary) or a refused commit (the
                    // owning conversation runtime is not activated) both
                    // leave no detached execution; the originating attempt
                    // observes a cancelled result slot.
                    Ok(BackgroundDispatchOutcome::RolledBack)
                    | Err(
                        crate::tools::background::BackgroundDispatchError::ConversationInactive {
                            ..
                        },
                    ) => cancelled_result(self.cancellation.reason()),
                    Err(error) => failed_result(&error.to_string()),
                }
            }
            Err(error) => failed_result(&error.to_string()),
        }
    }

    /// Builds the canonical request from one finite context projection.
    ///
    /// Every projected item is already a complete canonical message, so
    /// there is nothing to materialize: the projection's messages *are* the
    /// request messages. The exact Effective System Prompt is carried as a
    /// provider-neutral request value and adapters only translate it.
    fn model_request_from_projection(&self, projection: &ContextProjection) -> ModelRequest {
        let primary = self.request.model.primary();
        // Tool definitions are compiled only for a model whose effective
        // capabilities include tool calls: a text-only model is usable, it
        // simply never receives runtime tool definitions.
        let tools = if primary.capabilities().tool_calls {
            self.tool_registry().model_definitions()
        } else {
            Vec::new()
        };
        ModelRequest {
            invocation: primary.invocation_config(),
            messages: projection.messages.clone(),
            tools,
            effective_system_prompt: projection.effective_system_prompt.clone(),
            continuation: self.pending_continuation.clone(),
        }
    }

    /// Commits **C**: the assembled Assistant message joins canonical history
    /// as one compound durable transition with its publication stream.
    ///
    /// The transition validates that the exact stream is publication-complete,
    /// appends the canonical Ledger fact, advances the Surface, records
    /// `AssistantMessageCommitted`, and clears the stream's publication
    /// staging — all in one transaction. `ModelRequestCompleted` is
    /// deliberately not part of it: provider completion remains an external
    /// execution fact even when canonicalization later fails.
    fn commit_assistant_message(
        &mut self,
        message_id: &MessageId,
        content: &[crate::message::types::AssistantContentBlock],
    ) -> Result<(), CanonicalCommitError> {
        let block = MessageBlock::Assistant(AssistantMessageBlock {
            id: message_id.clone(),
            content: content.to_vec(),
        });
        let Some(publication) = self.publication.as_ref() else {
            return Err(CanonicalCommitError::Durable(
                crate::durable::ConversationStoreError::PublicationViolation(format!(
                    "Assistant message {message_id} has no open publication stream"
                )),
            ));
        };
        let stream_id = publication.start.stream_id.clone();
        let prepared = self.conversation.prepare_commit(&block)?;
        let event = Self::canonical_event(&block).expect("an Assistant commit has its event");
        let persisted = self
            .store
            .commit_canonical_publication(
                &stream_id,
                prepared.message(),
                self.event_envelope(event),
            )
            .map_err(CanonicalCommitError::Durable)?;
        // The stream settled canonically: the Ledger is the long-term
        // authority and no audit may ever be created for it.
        self.publication = None;
        self.conversation.install_prepared(prepared);
        if let Some(observer) = self.observer {
            observer.observe_committed(&self.request.attempt_id, &block);
        }
        self.record_persisted_event(persisted);
        Ok(())
    }

    /// Commits one complete `ToolResult` batch atomically.
    ///
    /// The whole batch prepares (validates) first, then appends to the
    /// durable Message Ledger in **one** transaction, and only then installs
    /// each member in memory. A durable failure of any member appends and
    /// installs none of them, so a partial tool-result group can never become
    /// canonical — the prior review's tool-batch atomicity requirement.
    fn commit_tool_result_batch(
        &mut self,
        blocks: &[MessageBlock],
    ) -> Result<(), CanonicalCommitError> {
        let mut prepared = Vec::with_capacity(blocks.len());
        for block in blocks {
            prepared.push(self.conversation.prepare_commit(block)?);
        }
        let events = blocks
            .iter()
            .map(|block| {
                Self::canonical_event(block).ok_or_else(|| {
                    CanonicalCommitError::Durable(
                        crate::durable::inbox::ConversationStoreError::InvalidReference(
                            "a ToolResult batch contains a non-tool message".to_owned(),
                        ),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let envelopes = events
            .into_iter()
            .map(|event| self.event_envelope(event))
            .collect::<Vec<_>>();
        let persisted_events = self
            .store
            .append_canonical_batch_with_events(blocks, &envelopes)
            .map_err(CanonicalCommitError::Durable)?;
        for (prepared, block) in prepared.into_iter().zip(blocks) {
            self.conversation.install_prepared(prepared);
            if let Some(observer) = self.observer {
                observer.observe_committed(&self.request.attempt_id, block);
            }
        }
        for event in persisted_events {
            self.record_persisted_event(event);
        }
        Ok(())
    }

    /// Opens the publication stream of one model request.
    ///
    /// The stream is pinned to the exact request generation that started it:
    /// attempt, turn, request, and provisional message identity are frozen
    /// here, so a resource, Skill, or Tool configuration edit that lands
    /// during streaming can never re-associate this stream with a newer
    /// generation, and recovery classifies it from these identities alone.
    fn open_publication(&mut self, message_id: &MessageId) {
        let Some(request_id) = self.last_request_id.clone() else {
            self.record_publication_failure(
                "a publication stream cannot open without a started request",
            );
            return;
        };
        let start = PublicationStreamStart {
            stream_id: PublicationStreamId::for_request(&self.request.attempt_id, message_id),
            attempt_id: self.request.attempt_id.clone(),
            turn_id: TurnId::new(self.turn.to_string()),
            request_id,
            message_id: message_id.clone(),
        };
        if let Err(error) = self.store.open_publication_stream(&start) {
            self.record_publication_failure(&format!(
                "the publication stream could not be opened durably: {error}"
            ));
            return;
        }
        let coalescer = PublicationCoalescer::new(
            start.stream_id.clone(),
            message_id.clone(),
            self.publication_policy,
            std::sync::Arc::clone(&self.publication_clock),
        );
        if let Some(observer) = self.observer {
            observer.observe_publication_opened(&self.request.attempt_id, &start);
        }
        self.publication = Some(OpenPublication {
            start,
            coalescer,
            terminal_committed: false,
        });
    }

    /// The publication payload of one non-terminal model event, when the
    /// event carries user-facing semantic output.
    ///
    /// Usage and provider continuation state are request bookkeeping, not
    /// released output, so they produce no publication frame.
    fn publication_payload(event: &ModelEvent) -> Option<PublicationPayload> {
        match event {
            ModelEvent::TextDelta { block_index, text } => Some(PublicationPayload::TextSuffix {
                block_index: *block_index,
                suffix: text.clone(),
            }),
            ModelEvent::ReasoningDelta { block_index, text } => {
                Some(PublicationPayload::ReasoningSuffix {
                    block_index: *block_index,
                    suffix: text.clone(),
                })
            }
            ModelEvent::RefusalDelta { block_index, text } => {
                Some(PublicationPayload::RefusalSuffix {
                    block_index: *block_index,
                    suffix: text.clone(),
                })
            }
            ModelEvent::ToolCallStarted { block_index, call } => {
                Some(PublicationPayload::ProposedToolCallStarted {
                    block_index: *block_index,
                    call: call.clone(),
                })
            }
            ModelEvent::ToolCallArgumentsDelta {
                block_index,
                call_id,
                arguments_delta,
            } => Some(PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: *block_index,
                call_id: call_id.clone(),
                suffix: arguments_delta.clone(),
            }),
            ModelEvent::ToolCallCompleted { block_index, call } => {
                Some(PublicationPayload::ProposedToolCallCompleted {
                    block_index: *block_index,
                    call: call.clone(),
                })
            }
            ModelEvent::Started
            | ModelEvent::UsageUpdate { .. }
            | ModelEvent::ContinuationState { .. }
            | ModelEvent::Completed { .. }
            | ModelEvent::Failed { .. } => None,
        }
    }

    /// Buffers one non-terminal model event into the publication coalescer
    /// and performs the bounded flush the policy requires.
    fn publish_model_event(&mut self, event: &ModelEvent) {
        let Some(payload) = Self::publication_payload(event) else {
            return;
        };
        let Some(publication) = self.publication.as_mut() else {
            self.record_publication_failure(
                "a model delta arrived with no open publication stream",
            );
            return;
        };
        let must_flush = publication.coalescer.push(payload);
        if must_flush || publication.coalescer.latency_elapsed() {
            self.flush_publication();
        }
    }

    /// Stages the buffered publication payload durably and only then releases
    /// it.
    ///
    /// The order is the whole point: nothing reaches a user-facing Runtime
    /// Client before its staging transaction committed.
    fn flush_publication(&mut self) {
        let Some(publication) = self.publication.as_mut() else {
            return;
        };
        let frames = publication.coalescer.take_frames();
        if frames.is_empty() {
            return;
        }
        if let Err(error) = self.store.stage_publication_frames(&frames) {
            self.record_publication_failure(&format!(
                "publication frames could not be staged before release: {error}"
            ));
            return;
        }
        self.release_publication(&frames);
    }

    /// Commits **U** — the final publication frame and the publication
    /// terminal marker in one transaction — and only then releases the final
    /// buffered payload.
    ///
    /// P must already be durable; the durable store enforces that. When no
    /// visible payload remains, a terminal-only frame still commits and no
    /// visible text is delayed.
    fn commit_publication_terminal(&mut self) -> Result<(), RuntimeError> {
        let Some(publication) = self.publication.as_mut() else {
            return Ok(());
        };
        if publication.terminal_committed {
            return Ok(());
        }
        let stream_id = publication.start.stream_id.clone();
        let frames = publication.coalescer.take_terminal_frames();
        if let Err(error) = self.store.commit_publication_terminal(&stream_id, &frames) {
            let message =
                format!("the publication terminal could not be committed durably: {error}");
            self.record_publication_failure(&message);
            return Err(RuntimeError::DurableStore { message });
        }
        if let Some(publication) = self.publication.as_mut() {
            publication.terminal_committed = true;
        }
        self.release_publication(&frames);
        Ok(())
    }

    /// Releases already-committed frames to the live observation seam.
    fn release_publication(&self, frames: &[PublicationFrame]) {
        let Some(observer) = self.observer else {
            return;
        };
        for frame in frames {
            observer.observe_publication(&self.request.attempt_id, frame);
        }
    }

    /// Settles a still-open publication stream as an audit.
    ///
    /// The audit kind is derived by the durable store from P/U evidence
    /// alone, so this path can never mislabel an Incomplete publication as
    /// Unaccepted (or the reverse) no matter which control-flow exit reached
    /// it. Canonical acceptance of the stream becomes permanently forbidden.
    fn settle_publication_audit(&mut self) -> Result<(), RuntimeError> {
        let Some(stream_id) = self
            .publication
            .as_ref()
            .map(|publication| publication.start.stream_id.clone())
        else {
            return Ok(());
        };
        match self
            .store
            .terminalize_publication_audit(&stream_id, Utc::now())
        {
            Ok(audit) => {
                // Only remove the in-memory owner after the durable
                // settlement committed. On failure it remains the one
                // recoverable unsettled stream.
                self.publication.take();
                if let Some(observer) = self.observer {
                    observer.observe_publication_settled(&self.request.attempt_id, &audit);
                }
                Ok(())
            }
            Err(error) => {
                let message =
                    format!("the publication audit could not be terminalized durably: {error}");
                self.record_publication_failure(&message);
                self.publication_settlement_failed = true;
                Err(RuntimeError::DurableStore { message })
            }
        }
    }

    /// Whether the open publication stream is holding buffered payload.
    fn has_buffered_publication(&self) -> bool {
        self.publication
            .as_ref()
            .is_some_and(|publication| !publication.coalescer.is_empty())
    }

    /// Records a publication-plane durability failure exactly once.
    ///
    /// A publication failure is a durable-authority failure like any other:
    /// the attempt must not report a healthy durability state afterwards.
    fn record_publication_failure(&mut self, message: &str) {
        if self.durable_failure.is_none() {
            self.durable_failure_kind = Some(DurableFailureKind::Publication);
            self.durable_failure = Some(message.to_owned());
        }
    }

    fn canonical_event(block: &MessageBlock) -> Option<RuntimeEvent> {
        match block {
            MessageBlock::Assistant(message) => Some(RuntimeEvent::AssistantMessageCommitted {
                message_id: message.id.clone(),
            }),
            MessageBlock::Tool(message) => Some(RuntimeEvent::ToolMessageCommitted {
                message_id: message.id.clone(),
                tool_call_id: message.tool_call_id.clone(),
            }),
            MessageBlock::User(_) => None,
        }
    }

    fn event_envelope(&self, event: RuntimeEvent) -> RuntimeEventEnvelope {
        // Attempt lifecycle facts are attempt-level facts, not events in the
        // model turn that just completed. Keeping their turn identity empty
        // lets the durable Journal enforce turn terminality without treating
        // the enclosing attempt lifecycle as a contradictory turn fact.
        let turn_id = if matches!(
            &event,
            RuntimeEvent::AttemptStarted { .. }
                | RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptFailed { .. }
        ) {
            None
        } else {
            Some(TurnId::new(self.turn.to_string()))
        };
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: self.request.conversation_id.clone(),
            attempt_id: Some(self.request.attempt_id.clone()),
            turn_id,
            timestamp: Utc::now(),
            event,
        }
    }

    fn record_persisted_event(&self, envelope: RuntimeEventEnvelope) {
        let event = envelope.event;
        if let Some(observer) = self.observer {
            observer.observe_event(&self.request.attempt_id, &event);
        }
    }

    fn emit(&mut self, event: RuntimeEvent) {
        debug_assert!(
            !self.terminal_emitted,
            "no runtime events may follow the terminal event"
        );
        if self.durable_failure.is_some() {
            return;
        }
        match self.store.append_event(self.event_envelope(event)) {
            Ok(envelope) => self.record_persisted_event(envelope),
            Err(error) => {
                self.durable_failure_kind = Some(DurableFailureKind::EventJournal);
                self.durable_failure = Some(format!(
                    "runtime event could not be persisted before publication: {error}"
                ));
            }
        }
    }

    fn emit_terminal(&mut self, terminal: &Terminal) {
        let event = match terminal {
            Terminal::Completed { finish_reason } => RuntimeEvent::AttemptCompleted {
                attempt_id: self.request.attempt_id.clone(),
                finish_reason: finish_reason.clone(),
            },
            Terminal::Cancelled { reason } => RuntimeEvent::AttemptCancelled {
                attempt_id: self.request.attempt_id.clone(),
                reason: *reason,
            },
            Terminal::Failed { failure } => RuntimeEvent::AttemptFailed {
                attempt_id: self.request.attempt_id.clone(),
                error: failure.clone(),
            },
        };
        debug_assert!(!self.terminal_emitted, "exactly one terminal event");
        self.terminal_emitted = true;
        match self.store.append_event(self.event_envelope(event)) {
            Ok(envelope) => self.record_persisted_event(envelope),
            Err(error) => {
                // The execution state machine has settled, but the durable
                // Event Journal has not. The uncommitted candidate must never
                // enter the persisted-event projection or observer stream.
                self.durable_failure_kind = Some(DurableFailureKind::EventJournal);
                self.durable_failure = Some(format!(
                    "terminal event could not be persisted before publication: {error}"
                ));
            }
        }
    }
}

/// One result slot of a committed tool-call batch, preallocated in model
/// call order so completion timing can never influence message identities or
/// canonical ordering.
struct CallSlot {
    call: ToolCall,
    /// The canonical registry-resolved tool identity of the call. Both
    /// preflight outcomes carry it, so a rejected slot is identified by the
    /// registry's resolution rather than by the raw model-issued value.
    tool_id: ToolId,
    /// The canonical registry-resolved typed origin of the tool.
    origin: ToolOrigin,
    prepared: Option<PreparedInvocation>,
    result: Option<ToolExecutionResult>,
    started: bool,
    progress: Vec<RuntimeEvent>,
}

/// The immutable facts of one settled call of a structurally settled batch.
///
/// These are copies of exactly what was committed as canonical history, kept
/// in canonical model call order. They are the only input the tool-result
/// observation pass receives, so an observer can neither reach the live call
/// slot nor influence the committed result.
struct SettledCall {
    batch_position: usize,
    call_id: ToolCallId,
    tool_id: ToolId,
    origin: ToolOrigin,
    /// The immutable invocation facts, absent exactly when preflight rejected
    /// the call before invocation resolution.
    invocation: Option<ObservedToolInvocation>,
    result: ToolExecutionResult,
}

/// The result of one pre-tool policy/interaction boundary.
enum PreToolResolution {
    /// The existing Tool Plane start frontier may consider the call.
    Allow,
    /// The call receives a policy-denied result slot; no executor starts.
    Denied(String),
    /// The owner cancellation closed the start frontier.
    Cancelled(CancellationReason),
}

/// One deterministic scheduling phase of a tool-call batch.
enum Group {
    /// The slot is already settled (validation rejection); no barrier.
    Trivial,
    /// An exclusive scheduling barrier.
    Sequential,
    /// Adjacent parallel invocations executing concurrently.
    Parallel,
}

/// The scheduling phase beginning at `index`: a `Sequential` invocation is
/// an exclusive barrier; adjacent `Parallel` invocations form one group.
fn group_at(slots: &[CallSlot], index: usize) -> Group {
    let Some(prepared) = slots[index].prepared.as_ref() else {
        return Group::Trivial;
    };
    match prepared.concurrency {
        ToolConcurrencyPolicy::Sequential => Group::Sequential,
        ToolConcurrencyPolicy::Parallel => Group::Parallel,
    }
}

/// The exclusive end of the parallel group beginning at `index`: every
/// adjacent `Parallel` invocation forms one group.
fn parallel_group_end(slots: &[CallSlot], index: usize) -> usize {
    let mut end = index + 1;
    while end < slots.len()
        && slots[end]
            .prepared
            .as_ref()
            .is_some_and(|candidate| candidate.concurrency == ToolConcurrencyPolicy::Parallel)
    {
        end += 1;
    }
    end
}

/// A failed tool result.
fn failed_result(error: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: error.to_owned(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// A policy-denied result is a normal structural Tool Plane result, distinct
/// from executor failure. It occupies exactly one canonical call slot and
/// carries no execution-start fact.
fn denied_result(reason: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Denied {
            reason: reason.to_owned(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// A cancelled tool result carrying the attempt cancellation reason.
fn cancelled_result(reason: CancellationReason) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Cancelled { reason },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// The bounded foreground progress buffer of one active tool call.
///
/// One foreground invocation owns exactly one buffer. The executor's
/// progress reports are normalized through the one shared UTF-8-safe bound
/// ([`bound_tool_progress`], the same normalization the background registry
/// uses) and retained under the explicit tool-plane cardinality bound
/// [`MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`]. Once the bound is reached,
/// the first `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1` observations are
/// pinned and the final slot tracks the newest observation, so retained
/// progress always ends with the most recent executor state and the buffer
/// never exceeds the bound while the executor is still running.
///
/// The buffer is transient current-execution state: only the retained
/// observations become canonical `ToolExecutionProgress` Event Journal facts
/// at batch commit, before their completion event. Coalesced observations
/// never cross the durable execution-fact commit point.
struct ForegroundProgressBuffer {
    call_id: ToolCallId,
    tool_id: ToolId,
    events: std::sync::Mutex<Vec<RuntimeEvent>>,
}

impl ForegroundProgressBuffer {
    /// An empty bounded buffer for one foreground invocation.
    fn new(call_id: ToolCallId, tool_id: ToolId) -> Self {
        Self {
            call_id,
            tool_id,
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Drains the retained progress events in observation order, newest
    /// last. Called exactly once, when the invocation structurally settles.
    fn take(&self) -> Vec<RuntimeEvent> {
        std::mem::take(&mut *self.events.lock().expect("progress buffer lock"))
    }

    /// The number of retained observations; test-only invariant probe.
    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.events.lock().expect("progress buffer lock").len()
    }
}

impl ProgressReporter for ForegroundProgressBuffer {
    fn report(&self, progress: ToolProgress) {
        let bounded = crate::tools::limits::bound_tool_progress(progress);
        let event = RuntimeEvent::ToolExecutionProgress {
            tool_call_id: self.call_id.clone(),
            tool_id: self.tool_id.clone(),
            execution_id: None,
            progress: bounded,
        };
        let mut events = self.events.lock().expect("progress buffer lock");
        if events.len() < crate::tools::limits::MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
            events.push(event);
        } else {
            // At capacity the earliest observations are pinned and the final
            // slot tracks the newest observation: first
            // `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1` progress events,
            // then the latest progress event.
            *events
                .last_mut()
                .expect("the foreground progress bound is positive") = event;
        }
    }
}

/// Test-only synchronization for in-crate unit tests.
///
/// [`ContinuationBoundaryPause`] parks the execution at the turn-continuation
/// boundary — after a completed turn (including every mailbox drain/append
/// of that turn) returned "continue", before the generic
/// cancellation-before-next-turn check — so a unit test can make
/// cancellation observable deterministically between turns, without timing
/// assumptions.
///
/// The pause signals `reached` through a watch (observed with `wait_for`)
/// and blocks the execution task on a `std` channel until the test
/// releases it, so the controlling test must run on a multi-threaded
/// runtime. This hook exists only under `#[cfg(test)]`.
#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::mpsc;

    use tokio::sync::watch;

    /// A test-only control point at the turn-continuation boundary.
    ///
    /// The execution parks here exactly when a completed turn returned
    /// "continue" — the turn is structurally complete and every mailbox
    /// drain/append of that turn is done — before the generic
    /// cancellation-before-next-turn check runs. A unit test can therefore
    /// make cancellation observable deterministically after one turn
    /// completed but before another starts, without timing assumptions.
    ///
    /// The pause signals `reached` through a watch (observed with
    /// `wait_for`) and blocks the execution task on a `std` channel until
    /// the test releases it, so the controlling test must run on a
    /// multi-threaded runtime. This hook exists only under `#[cfg(test)]`.
    #[derive(Debug)]
    pub(super) struct ContinuationBoundaryPause {
        reached: watch::Sender<bool>,
        release: mpsc::Receiver<()>,
    }

    impl ContinuationBoundaryPause {
        /// Creates the pause and its observation/release handles.
        #[must_use]
        pub(super) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
            let (reached, reached_rx) = watch::channel(false);
            let (release_tx, release_rx) = mpsc::channel();
            (
                Self {
                    reached,
                    release: release_rx,
                },
                reached_rx,
                release_tx,
            )
        }

        /// Signals that the turn boundary was reached, then blocks until
        /// the test releases the execution.
        pub(super) fn park_at_continuation_boundary(&self) {
            self.reached.send_replace(true);
            let _ = self.release.recv();
        }
    }

    /// A test-only control point after one `ToolExecutionStarted` fact and
    /// before the next sibling is announced. The test can request attempt or
    /// runtime cancellation while the exact start frontier is parked.
    #[derive(Debug)]
    pub(crate) struct ToolStartPause {
        reached: watch::Sender<bool>,
        release: mpsc::Receiver<()>,
    }

    impl ToolStartPause {
        /// Creates the pause and its observation/release handles.
        #[must_use]
        pub(crate) fn install() -> (Self, watch::Receiver<bool>, mpsc::Sender<()>) {
            let (reached, reached_rx) = watch::channel(false);
            let (release_tx, release_rx) = mpsc::channel();
            (
                Self {
                    reached,
                    release: release_rx,
                },
                reached_rx,
                release_tx,
            )
        }

        /// Signals the first announced tool start and blocks the loop until
        /// the test releases the frontier.
        pub(super) fn park(&self) {
            self.reached.send_replace(true);
            let _ = self.release.recv();
        }
    }

    /// A test-only control point at the one M9b model-turn start boundary.
    ///
    /// Two independent phases cover the deterministic race matrix:
    ///
    /// - `pre_start`: parks immediately before the cancellation-vs-start
    ///   arbitration, after every fallible preparation step completed. No
    ///   request-scoped context, Request Snapshot, start fact, or provider
    ///   request exists while parked, so a cancellation issued while parked
    ///   provably linearizes before the arbitration.
    /// - `pre_commit`: parks **inside** the arbitration critical section
    ///   (the attempt's start gate is held), immediately before the durable
    ///   start commit. A cancellation issued while parked provably blocks
    ///   behind the gate and is therefore post-start once the commit
    ///   succeeds.
    ///
    /// Each phase counts its parks on a watch channel and blocks the
    /// execution task on a `std` channel until the test releases that exact
    /// park, so multi-request tests address each park by ordinal. This hook
    /// exists only under `#[cfg(test)]`.
    #[derive(Debug)]
    pub(crate) struct StartBoundaryPause {
        pre_start: Option<PhasePause>,
        pre_commit: Option<PhasePause>,
    }

    /// One parked phase of the start boundary.
    #[derive(Debug)]
    struct PhasePause {
        reached: watch::Sender<u32>,
        release: mpsc::Receiver<()>,
        parks: std::sync::atomic::AtomicU32,
    }

    /// The test-side control of one installed phase.
    #[derive(Debug)]
    pub(crate) struct PhasePauseControl {
        reached: watch::Receiver<u32>,
        release: mpsc::Sender<()>,
    }

    impl PhasePauseControl {
        /// Waits until the phase parked the `ordinal`-th time (1-based).
        pub(crate) async fn await_park(&mut self, ordinal: u32) {
            self.reached
                .wait_for(|parks| *parks >= ordinal)
                .await
                .expect("start boundary pause channel stays open");
        }

        /// Releases the currently parked execution.
        pub(crate) fn release(&self) {
            let _ = self.release.send(());
        }
    }

    impl PhasePause {
        fn install() -> (Self, PhasePauseControl) {
            let (reached, reached_rx) = watch::channel(0_u32);
            let (release_tx, release_rx) = mpsc::channel();
            (
                Self {
                    reached,
                    release: release_rx,
                    parks: std::sync::atomic::AtomicU32::new(0),
                },
                PhasePauseControl {
                    reached: reached_rx,
                    release: release_tx,
                },
            )
        }

        fn park(&self) {
            let parks = self.parks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            self.reached.send_replace(parks);
            let _ = self.release.recv();
        }
    }

    impl StartBoundaryPause {
        /// Creates the pause with the selected phases and their test-side
        /// controls.
        #[must_use]
        pub(crate) fn install(
            pre_start: bool,
            pre_commit: bool,
        ) -> (Self, Option<PhasePauseControl>, Option<PhasePauseControl>) {
            let (pre_start_pause, pre_start_control) = if pre_start {
                let (pause, control) = PhasePause::install();
                (Some(pause), Some(control))
            } else {
                (None, None)
            };
            let (pre_commit_pause, pre_commit_control) = if pre_commit {
                let (pause, control) = PhasePause::install();
                (Some(pause), Some(control))
            } else {
                (None, None)
            };
            (
                Self {
                    pre_start: pre_start_pause,
                    pre_commit: pre_commit_pause,
                },
                pre_start_control,
                pre_commit_control,
            )
        }

        /// Parks immediately before the cancellation-vs-start arbitration.
        pub(super) fn park_before_start_arbitration(&self) {
            if let Some(phase) = &self.pre_start {
                phase.park();
            }
        }

        /// Parks inside the arbitration critical section, immediately
        /// before the durable start commit (the start gate is held).
        pub(super) fn park_before_start_commit(&self) {
            if let Some(phase) = &self.pre_commit {
                phase.park();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::conversation::ConversationState;
    use crate::durable::inbox::ConversationStore;
    use crate::events::types::{AttemptOutcome, RuntimeEvent};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use futures_util::future::BoxFuture;
    use tokio::sync::watch;

    use crate::agent::observer::{AgentExecutionObserver, AgentStatusObservation};
    use crate::message::types::{
        ContentBlockIndex, ContextKind, InboundKind, MessageBlock, UserContentBlock,
        UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelEventStream};
    use crate::model::chat_protocol;
    use crate::model::error::ModelErrorKind;
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, MessageId, RequestId, ToolCallId, ToolId,
    };
    use crate::runtime::inbound::InitialTurnTrigger;
    use crate::runtime::types::CancellationReason;
    use crate::scripted_suites::common::{tool_runtime, tool_runtime_with_store};
    use crate::scripted_suites::support::model::scripted_session_model;
    use crate::tools::executor::{ProgressReporter, ToolExecutor, ToolRegistry};
    use crate::tools::limits::{
        MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL, MAX_PROGRESS_MESSAGE_BYTES,
    };
    use crate::tools::types::{
        ToolCall, ToolCallStart, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy,
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolOrigin, ToolReplayPolicy,
    };

    use super::{
        AgentExecution, AgentExecutionRequest, ForegroundProgressBuffer,
        test_sync::{ContinuationBoundaryPause, StartBoundaryPause},
    };
    use crate::agent::cancellation::AgentCancellation;
    use crate::context::ContextRuntime;

    /// An empty bounded foreground progress buffer for one scripted call.
    fn foreground_buffer() -> ForegroundProgressBuffer {
        ForegroundProgressBuffer::new(ToolCallId::new("call-1"), ToolId::new("tool-1"))
    }

    /// Reports one numbered progress observation through the reporter seam,
    /// exactly as an executor would.
    fn report_progress(buffer: &ForegroundProgressBuffer, index: usize) {
        buffer.report(crate::tools::types::ToolProgress {
            message: Some(format!("progress {index}")),
            completed: None,
            total: None,
        });
    }

    /// Drains the retained progress messages in observation order.
    fn retained_messages(buffer: &ForegroundProgressBuffer) -> Vec<String> {
        buffer
            .take()
            .iter()
            .map(|event| match event {
                RuntimeEvent::ToolExecutionProgress { progress, .. } => {
                    progress.message.clone().expect("numbered progress message")
                }
                other => {
                    panic!("the foreground progress buffer retains only progress events: {other:?}")
                }
            })
            .collect()
    }

    /// Exact bound: reporting exactly `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`
    /// observations retains every one of them in observation order, and the
    /// retained count never exceeds the bound while reporting.
    #[test]
    fn foreground_progress_buffer_retains_the_exact_bound() {
        let buffer = foreground_buffer();
        for index in 0..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
            report_progress(&buffer, index);
            assert!(
                buffer.retained_len() <= MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
                "the retained count never exceeds the bound"
            );
        }
        let messages = retained_messages(&buffer);
        assert_eq!(messages.len(), MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL);
        for (index, message) in messages.iter().enumerate() {
            assert_eq!(message, &format!("progress {index}"));
        }
    }

    /// One over the bound: the first `MAX - 1` observations are pinned and
    /// the final slot tracks the newest observation, so the retained count
    /// stays exactly at the bound and ends with the latest progress.
    #[test]
    fn foreground_progress_buffer_one_over_the_bound_keeps_first_prefix_plus_latest() {
        let buffer = foreground_buffer();
        for index in 0..=MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
            report_progress(&buffer, index);
            assert!(
                buffer.retained_len() <= MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
                "the retained count never exceeds the bound"
            );
        }
        let messages = retained_messages(&buffer);
        let mut expected: Vec<String> = (0..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1)
            .map(|index| format!("progress {index}"))
            .collect();
        expected.push(format!(
            "progress {MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL}"
        ));
        assert_eq!(
            messages, expected,
            "the overflow policy is deterministic: first MAX-1 pinned, newest last"
        );
    }

    /// Flood: a misbehaving executor reporting ten times the bound never
    /// grows the buffer past the bound; the retained prefix stays the
    /// earliest observations and the final slot is the newest one.
    #[test]
    fn foreground_progress_buffer_flood_never_exceeds_the_bound() {
        const FLOOD: usize = MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL * 10;
        let buffer = foreground_buffer();
        for index in 0..FLOOD {
            report_progress(&buffer, index);
            assert!(
                buffer.retained_len() <= MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL,
                "the bound holds during reporting, not only after settlement"
            );
        }
        let messages = retained_messages(&buffer);
        assert_eq!(messages.len(), MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL);
        for (index, message) in messages[..MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1]
            .iter()
            .enumerate()
        {
            assert_eq!(message, &format!("progress {index}"));
        }
        assert_eq!(
            messages.last().expect("retained progress"),
            &format!("progress {}", FLOOD - 1),
            "the final retained observation is the newest executor state"
        );
    }

    /// The canonical shared normalization (message bytes, finite values)
    /// applies as part of bounded retention; the cardinality bound never
    /// bypasses or duplicates it.
    #[test]
    fn foreground_progress_buffer_applies_canonical_normalization_before_retention() {
        let buffer = foreground_buffer();
        buffer.report(crate::tools::types::ToolProgress {
            message: Some("x".repeat(MAX_PROGRESS_MESSAGE_BYTES + 10)),
            completed: Some(f64::NAN),
            total: Some(f64::INFINITY),
        });
        let events = buffer.take();
        assert_eq!(events.len(), 1);
        let RuntimeEvent::ToolExecutionProgress { progress, .. } = &events[0] else {
            panic!("the buffer retains a progress event");
        };
        assert_eq!(
            progress.message.as_deref().expect("message").len(),
            MAX_PROGRESS_MESSAGE_BYTES
        );
        assert_eq!(progress.completed, None, "non-finite values are dropped");
        assert_eq!(progress.total, None, "non-finite values are dropped");
    }

    /// A scripted model adapter: each invocation pops the next event script
    /// and yields it synchronously, recording every request.
    struct ScriptedAdapter {
        scripts: Mutex<VecDeque<Vec<ModelEvent>>>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl ScriptedAdapter {
        fn new(scripts: Vec<Vec<ModelEvent>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into()),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn request_count(&self) -> usize {
            self.requests
                .lock()
                .expect("scripted adapter request lock")
                .len()
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests
                .lock()
                .expect("scripted adapter request lock")
                .clone()
        }
    }

    impl ModelAdapter for ScriptedAdapter {
        fn protocol(&self) -> ModelProtocol {
            chat_protocol()
        }

        fn stream(
            &self,
            request: ModelRequest,
            _cancellation: CancellationSignal,
        ) -> ModelEventStream {
            self.requests
                .lock()
                .expect("scripted adapter request lock")
                .push(request);
            let script = self
                .scripts
                .lock()
                .expect("scripted adapter script lock")
                .pop_front()
                .unwrap_or_default();
            Box::pin(futures_util::stream::iter(script))
        }
    }

    use crate::publication::{PublicationFrame, PublicationStreamStart};

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<RuntimeEvent>>,
        /// The released publication frames, in release order. Every frame
        /// here is already durably committed for release.
        frames: Mutex<Vec<PublicationFrame>>,
        /// The audits of every stream that settled without canonical
        /// acceptance.
        audits: Mutex<Vec<crate::publication::PublicationAudit>>,
    }

    impl AgentExecutionObserver for RecordingObserver {
        fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
            self.events
                .lock()
                .expect("observer event lock")
                .push(event.clone());
        }

        fn observe_committed(&self, _attempt_id: &AttemptId, _block: &MessageBlock) {}

        fn observe_status(&self, _observation: &AgentStatusObservation) {}

        fn observe_publication_opened(
            &self,
            _attempt_id: &AttemptId,
            _start: &PublicationStreamStart,
        ) {
        }

        fn observe_publication(&self, _attempt_id: &AttemptId, frame: &PublicationFrame) {
            self.frames
                .lock()
                .expect("observer frame lock")
                .push(frame.clone());
        }

        fn observe_publication_settled(
            &self,
            _attempt_id: &AttemptId,
            audit: &crate::publication::PublicationAudit,
        ) {
            self.audits
                .lock()
                .expect("observer audit lock")
                .push(audit.clone());
        }
    }

    /// A contributor whose bounded work is explicitly held at an awaited
    /// boundary. The test releases it only after cancellation is observable,
    /// proving that the final admission check—not the contributor's private
    /// synchronization—decides whether transient proposals become facts.
    struct AwaitingContributor {
        entered: watch::Sender<bool>,
        release: watch::Receiver<bool>,
        invocations: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::context::ContextContributor for AwaitingContributor {
        fn contribute<'a>(
            &'a self,
            _input: &'a crate::context::ContributorInputSnapshot,
        ) -> BoxFuture<
            'a,
            Result<Vec<crate::context::ContextProposal>, crate::context::ContextAssemblyError>,
        > {
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.send_replace(true);
            let mut release = self.release.clone();
            Box::pin(async move {
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("contributor release channel stays open");
                Ok(vec![crate::context::ContextProposal::UserMessage(
                    crate::context::UserMessageProposal {
                        content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                            text: "awaited proposal".to_owned(),
                        })],
                    },
                )])
            })
        }
    }

    /// An instant fake executor returning one fixed successful result.
    struct InstantTool;

    impl InstantTool {
        fn definition(id: &str, name: &str) -> ToolDefinition {
            ToolDefinition {
                id: ToolId::new(id),
                name: name.to_owned(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
                execution_policy: ToolExecutionPolicy::ForegroundOnly,
                concurrency_policy: ToolConcurrencyPolicy::Sequential,
                approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Builtin,
            }
        }
    }

    impl ToolExecutor for InstantTool {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: crate::tools::executor::ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            Box::pin(async {
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                }
            })
        }
    }

    /// One attempt request bound to a scripted adapter through a real
    /// catalog resolution: the binding still requires an explicit endpoint
    /// and credential, exactly as in production.
    fn request(adapter: &Arc<ScriptedAdapter>) -> AgentExecutionRequest {
        let adapter: Arc<dyn ModelAdapter> = adapter.clone();
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id: ConversationId::new("conv-1"),
            attempt_id: AttemptId::new("attempt-1"),
            conversation: ConversationState::new(),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            timezone: None,
            model: scripted_session_model(adapter).snapshot(),
        }
    }

    /// A deterministic context runtime derived from the same immutable model
    /// snapshot as the execution. The window is far larger than any scripted
    /// request, so no compaction ever triggers in these tests.
    fn runtime(adapter: &Arc<ScriptedAdapter>) -> ContextRuntime {
        ContextRuntime::for_attempt(
            crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(crate::context::DefaultTokenEstimator),
            crate::context::AgentStatusComposer::default(),
            &request(adapter).model,
        )
        .expect("valid context runtime")
    }

    fn inbound_message(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse fixed timestamp")
                    .with_timezone(&chrono::Utc),
            ),
        }
    }

    /// One turn of a single tool call, scripted as canonical events.
    fn tool_call_script(call: &ToolCall) -> Vec<ModelEvent> {
        vec![
            ModelEvent::Started,
            ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
            ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                arguments_delta: "{}".to_owned(),
            },
            ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            },
        ]
    }

    /// The exact expected trace: one completed tool turn, then the generic
    /// pre-next-turn cancellation checkpoint settles the attempt cancelled
    /// before any second model turn.
    fn expected_trace() -> Vec<crate::events::types::RuntimeEvent> {
        use crate::events::types::RuntimeEvent;
        vec![
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt-1"),
            },
            RuntimeEvent::TurnStarted,
            RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                model: "scripted".to_owned(),
            },
            // Assistant streaming assembly is no longer an Event Journal
            // fact (Issue #108): it lives in the durable publication plane.
            RuntimeEvent::ModelRequestCompleted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            },
            RuntimeEvent::AssistantMessageCommitted {
                message_id: MessageId::new("attempt-1-agent-1"),
            },
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
            },
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            },
            RuntimeEvent::ToolMessageCommitted {
                message_id: MessageId::new("attempt-1-tool-1-call-1"),
                tool_call_id: ToolCallId::new("call-1"),
            },
            RuntimeEvent::TurnCompleted,
            RuntimeEvent::AttemptCancelled {
                attempt_id: AttemptId::new("attempt-1"),
                reason: CancellationReason::UserRequested,
            },
        ]
    }

    /// Reads committed Event Journal facts through bounded pages for tests
    /// that explicitly audit the complete attempt history.
    fn event_history(store: &dyn ConversationStore) -> Vec<RuntimeEvent> {
        const PAGE_SIZE: usize = 32;
        let mut cursor = None;
        let mut events = Vec::new();
        loop {
            let page = store.read_events(cursor, PAGE_SIZE).expect("event page");
            if page.events.is_empty() {
                break;
            }
            events.extend(page.events.into_iter().map(|envelope| envelope.event));
            cursor = page.next_sequence;
        }
        events
    }

    /// Reads retained Request Snapshots through bounded pages for tests that
    /// explicitly audit historical request facts.
    fn request_snapshot_history(
        store: &dyn ConversationStore,
    ) -> Vec<crate::model::RequestSnapshot> {
        const PAGE_SIZE: usize = 32;
        let mut cursor = None;
        let mut snapshots = Vec::new();
        loop {
            let page = store
                .read_request_snapshots(cursor, PAGE_SIZE)
                .expect("request snapshot page");
            if page.snapshots.is_empty() {
                break;
            }
            snapshots.extend(page.snapshots);
            cursor = page.next_sequence;
        }
        snapshots
    }

    /// Spawns the controller that parks until the continuation boundary,
    /// makes cancellation observable there, and releases the execution.
    fn boundary_controller(
        mut reached_rx: tokio::sync::watch::Receiver<bool>,
        release_tx: std::sync::mpsc::Sender<()>,
        cancellation: AgentCancellation,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            reached_rx
                .wait_for(|reached| *reached)
                .await
                .expect("continuation boundary reached");
            cancellation.cancel();
            release_tx.send(()).expect("release the execution");
        })
    }

    /// Builds the attempt capability lease over the given tool registry and
    /// conversation tool runtime: empty Skill set, base environment, and a
    /// private environment store. Returns the store guard, the coordinator,
    /// and the pinned lease.
    async fn capability_lease(
        tools: ToolRegistry,
        tool_runtime: &crate::tools::runtime::ConversationToolRuntime,
    ) -> (
        tempfile::TempDir,
        crate::capabilities::CapabilityCoordinator,
        crate::capabilities::AttemptCapabilityLease,
    ) {
        let tools = std::sync::Arc::new(tools);
        let dir = tempfile::tempdir().expect("temp dir");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: tools,
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::default(),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("env-store"),
            },
        )
        .expect("capability coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let lease = coordinator.acquire_attempt_lease();
        (dir, coordinator, lease)
    }

    #[tokio::test]
    async fn capability_lease_owner_matches_runtime_before_execution() {
        let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, coordinator, lease) = capability_lease(ToolRegistry::new(), &tool_runtime).await;
        assert_eq!(coordinator.active_attempts(), 1);

        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("matching capability owner is accepted");

        assert_eq!(adapter.request_count(), 0, "construction is pre-execution");
        drop(execution);
        assert_eq!(coordinator.active_attempts(), 0);
    }

    /// The provider boundary is strictly after the durable request-start
    /// transaction. A deterministic request-start fault therefore leaves no
    /// started snapshot and the adapter is never called.
    #[tokio::test]
    async fn provider_is_not_invoked_before_durable_request_start_commit() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }]]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new("conv-1"))
                .expect("in-memory store"),
        );
        store.arm_request_start_fault_script([
            crate::durable::sqlite::RequestStartFaultOperation::BeforeContextAppend,
        ]);
        let tool_runtime = tool_runtime_with_store("conv-1", Some(store.clone()));
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("execution construction")
        .run()
        .await;

        assert_eq!(
            adapter.request_count(),
            0,
            "provider starts after durable commit"
        );
        assert!(result.durable_failure.is_some());
        assert!(
            store
                .read_request_snapshots(None, 32)
                .expect("request snapshots")
                .snapshots
                .is_empty()
        );
        assert!(
            !store
                .read_events(None, 32)
                .expect("event journal")
                .events
                .iter()
                .any(|envelope| matches!(envelope.event, RuntimeEvent::ModelRequestStarted { .. }))
        );
    }

    /// A generic provider request-size failure is terminal request ownership,
    /// not conversation-history pressure. Once an adapter normalizes it as
    /// `InvalidRequest`, the Agent Loop emits no compaction lifecycle and
    /// performs no summary/retry invocation.
    #[tokio::test]
    async fn invalid_request_size_failure_never_compacts_or_retries() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Failed {
            error: crate::model::error::ModelError {
                kind: ModelErrorKind::InvalidRequest,
                message: "Request exceeds the maximum size of 32 MB".to_owned(),
                retry_after_ms: None,
                provider_code: Some("request_too_large".to_owned()),
            },
        }]]));
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("execution construction")
        .run()
        .await;

        assert_eq!(
            adapter.request_count(),
            1,
            "no summary request or overflow retry is issued"
        );
        assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
        let events = tool_runtime
            .durable_store()
            .read_events(None, 128)
            .expect("event journal")
            .events;
        assert!(
            !events.iter().any(|event| matches!(
                event.event,
                RuntimeEvent::CompactionStarted
                    | RuntimeEvent::CompactionCompleted { .. }
                    | RuntimeEvent::CompactionFailed { .. }
            )),
            "generic request-size failure produces no compaction lifecycle"
        );
        assert!(
            !result.messages().iter().any(|message| matches!(
                message,
                MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
            )),
            "no durable compaction summary is created"
        );
    }

    /// A long scripted tool loop grows the durable authorities while the
    /// active execution retains only its current working state. Event and
    /// Request Snapshot inspection deliberately walks the store in bounded
    /// pages; the settlement result has no historical trace collections to
    /// retain or transfer.
    #[tokio::test]
    async fn long_attempt_history_is_durable_and_boundedly_inspectable() {
        const TOOL_TURNS: usize = 40;
        const PAGE_SIZE: usize = 7;

        let mut scripts = Vec::with_capacity(TOOL_TURNS + 1);
        for turn in 0..TOOL_TURNS {
            let call = ToolCall {
                id: ToolCallId::new(format!("call-{turn}")),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
                arguments: serde_json::json!({}),
            };
            scripts.push(tool_call_script(&call));
        }
        scripts.push(vec![
            ModelEvent::Started,
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "final".to_owned(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ]);

        let adapter = Arc::new(ScriptedAdapter::new(scripts));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new("conv-1"))
                .expect("in-memory store"),
        );
        let tool_runtime = tool_runtime_with_store("conv-1", Some(store.clone()));
        let mut tools = ToolRegistry::new();
        tools
            .register(
                InstantTool::definition("tool-alpha", "alpha"),
                Arc::new(InstantTool),
            )
            .expect("register scripted tool");
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await;

        assert!(matches!(
            result.outcome,
            AttemptOutcome::Completed {
                finish_reason: ModelFinishReason::Stop
            }
        ));
        assert_eq!(adapter.request_count(), TOOL_TURNS + 1);

        assert_bounded_event_history(store.as_ref(), PAGE_SIZE, TOOL_TURNS);
        assert_bounded_request_history(
            store.as_ref(),
            PAGE_SIZE,
            TOOL_TURNS + 1,
            &adapter.requests(),
        );
    }

    fn assert_bounded_event_history(
        store: &dyn ConversationStore,
        page_size: usize,
        minimum_event_count: usize,
    ) {
        let mut cursor = None;
        let mut sequences = Vec::new();
        let mut pages = 0;
        let mut last_event = None;
        loop {
            let page = store
                .read_events(cursor, page_size)
                .expect("event journal page");
            assert!(page.events.len() <= page_size);
            if page.events.is_empty() {
                break;
            }
            pages += 1;
            sequences.extend(page.events.iter().map(|event| event.sequence));
            last_event = page.events.last().map(|event| event.event.clone());
            cursor = page.next_sequence;
        }
        assert!(pages > 1, "the journal must be inspected in pages");
        assert!(!sequences.is_empty());
        assert!(
            sequences.len() > minimum_event_count,
            "the long attempt must commit more events than tool turns"
        );
        assert!(sequences.windows(2).all(|window| window[0] < window[1]));
        assert!(matches!(
            store
                .read_events(sequences.last().copied(), page_size)
                .expect("terminal journal page")
                .events
                .as_slice(),
            []
        ));
        assert!(matches!(
            last_event,
            Some(RuntimeEvent::AttemptCompleted { .. })
        ));
    }

    fn assert_bounded_request_history(
        store: &dyn ConversationStore,
        page_size: usize,
        expected_count: usize,
        provider_requests: &[ModelRequest],
    ) {
        let mut cursor = None;
        let mut snapshots = Vec::new();
        let mut pages = 0;
        loop {
            let page = store
                .read_request_snapshots(cursor, page_size)
                .expect("request snapshot page");
            assert!(page.snapshots.len() <= page_size);
            if page.snapshots.is_empty() {
                break;
            }
            pages += 1;
            snapshots.extend(page.snapshots);
            cursor = page.next_sequence;
        }
        assert!(pages > 1, "request history must be inspected in pages");
        assert_eq!(snapshots.len(), expected_count);
        assert!(
            snapshots
                .windows(2)
                .all(|window| window[0].request_id != window[1].request_id)
        );
        assert!(snapshots.iter().enumerate().all(|(index, snapshot)| {
            snapshot.identity.turn.as_str() == (index + 1).to_string()
                && snapshot.identity.retry_number == 0
        }));
        for (snapshot, request) in snapshots.iter().zip(provider_requests.iter()) {
            assert_eq!(
                store
                    .reconstruct_model_request(&snapshot.request_id)
                    .expect("historical request reconstruction"),
                *request
            );
        }
    }

    /// A terminal Event Journal append is a required durable publication.
    /// If it fails, the execution settlement candidate remains available to
    /// the caller, but neither the local event projection nor an observer may
    /// fabricate the uncommitted terminal fact.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_event_append_failure_never_fabricates_terminal_fact() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![
            ModelEvent::Started,
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            },
            ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ]]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new("conv-1"))
                .expect("in-memory store"),
        );
        store.arm_fail_next_terminal_event();
        let tool_runtime = tool_runtime_with_store("conv-1", Some(store.clone()));
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let observer = RecordingObserver::default();
        let mut execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("execution construction");
        execution.observe(&observer);

        let result = execution.run().await;

        assert!(matches!(
            result.outcome,
            AttemptOutcome::Completed {
                finish_reason: ModelFinishReason::Stop
            }
        ));
        assert_eq!(
            result.durable_failure_kind,
            Some(super::DurableFailureKind::EventJournal)
        );
        assert!(result.durable_failure.is_some());
        let events = event_history(store.as_ref());
        assert!(
            !events.iter().any(|event| matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )),
            "an uncommitted terminal candidate never enters the local projection"
        );
        assert!(
            !observer
                .events
                .lock()
                .expect("observer event lock")
                .iter()
                .any(|event| matches!(
                    event,
                    RuntimeEvent::AttemptCompleted { .. }
                        | RuntimeEvent::AttemptCancelled { .. }
                        | RuntimeEvent::AttemptTimedOut { .. }
                        | RuntimeEvent::AttemptLimitExceeded { .. }
                        | RuntimeEvent::AttemptFailed { .. }
                )),
            "publication follows the durable Event Journal commit"
        );
        let persisted = store.read_events(None, 64).expect("event journal").events;
        assert!(
            persisted
                .iter()
                .any(|envelope| matches!(envelope.event, RuntimeEvent::AttemptStarted { .. }))
        );
        assert!(
            !persisted.iter().any(|envelope| matches!(
                envelope.event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )),
            "the failed terminal transaction leaves no durable terminal fact"
        );
        assert_eq!(store.terminal_event_attempts(), 1);
    }

    #[tokio::test]
    async fn capability_lease_rejects_different_conversation_before_execution() {
        let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let owner_runtime = tool_runtime("conv-1");
        let (_dir, coordinator, lease) =
            capability_lease(ToolRegistry::new(), &owner_runtime).await;
        assert_eq!(coordinator.active_attempts(), 1);
        let other_dir = tempfile::tempdir().expect("other runtime directory");
        std::fs::create_dir_all(other_dir.path().join("workspace")).expect("other workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("conv-2"),
            other_dir.path().join("workspace"),
            other_dir.path().join("artifacts"),
        )
        .expect("other tool runtime");
        let mut other_request = request(&adapter);
        other_request.conversation_id = ConversationId::new("conv-2");

        let result = AgentExecution::new(
            other_request,
            lease,
            &cancellation,
            runtime(&adapter),
            &other_runtime,
            crate::agent::AttemptLifecycle::inert(),
        );

        assert!(matches!(
            result,
            Err(crate::runtime::inbound::MailboxError::CapabilityOwnershipMismatch { .. })
        ));
        assert_eq!(
            adapter.request_count(),
            0,
            "rejection precedes model requests"
        );
        assert_eq!(coordinator.active_attempts(), 0);
    }

    #[tokio::test]
    async fn capability_lease_rejects_different_workspace_before_execution() {
        let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let owner_runtime = tool_runtime("conv-1");
        let (_dir, coordinator, lease) =
            capability_lease(ToolRegistry::new(), &owner_runtime).await;
        assert_eq!(coordinator.active_attempts(), 1);
        let other_dir = tempfile::tempdir().expect("other workspace directory");
        std::fs::create_dir_all(other_dir.path().join("workspace")).expect("other workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("conv-1"),
            other_dir.path().join("workspace"),
            other_dir.path().join("artifacts"),
        )
        .expect("other tool runtime");

        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &other_runtime,
            crate::agent::AttemptLifecycle::inert(),
        );

        assert!(matches!(
            result,
            Err(crate::runtime::inbound::MailboxError::CapabilityOwnershipMismatch { .. })
        ));
        assert_eq!(
            adapter.request_count(),
            0,
            "rejection precedes model requests"
        );
        assert_eq!(coordinator.active_attempts(), 0);
    }

    /// Issue #12 (M9b), cancellation wins before start: Context Assembly is
    /// allowed to finish while cancellation is observable, but the one
    /// model-turn start gate is the linearization point. The execution parks
    /// immediately before the arbitration; the cancellation fully completes
    /// while parked (provably linearized first); the released arbitration
    /// observes it and discards the prepared turn: no provider request, no
    /// `ModelRequestStarted`, no started Request Snapshot, and no
    /// request-scoped context commit.
    ///
    /// `TurnStarted` is present: it records that the loop began preparing
    /// the logical turn, which genuinely happened; it implies no request,
    /// no context commit, and no provider side effect. The attempt terminal
    /// is unique and last.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_before_start_arbitration_commits_nothing() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }]]));
        let (request, runtime, invocation_count) = counting_context_request(&adapter);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
        let mut pre_start = pre_start.expect("pre-start phase installed");
        let controller_cancellation = cancellation.clone();
        let controller = tokio::spawn(async move {
            pre_start.await_park(1).await;
            controller_cancellation.cancel();
            // The cancellation fully completed while the execution was
            // parked before the arbitration: it provably linearized first.
            assert!(controller_cancellation.is_cancelled());
            pre_start.release();
        });
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let mut execution = AgentExecution::new(
            request,
            lease,
            &cancellation,
            runtime,
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.install_start_boundary_pause(pause);
        let result = execution.run().await;
        controller.await.expect("cancellation controller");
        let store = tool_runtime.durable_store();
        let snapshots = request_snapshot_history(store.as_ref());
        let events = event_history(store.as_ref());

        assert_eq!(
            invocation_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the finite contributor ran once and was never rerun"
        );
        assert!(matches!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ));
        assert!(snapshots.is_empty(), "no started Request Snapshot exists");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. })),
            "no request-start fact exists"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::TurnStarted)),
            "TurnStarted records that turn preparation began"
        );
        assert!(
            matches!(events.last(), Some(RuntimeEvent::AttemptCancelled { .. }))
                && events
                    .iter()
                    .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
                    .count()
                    == 1,
            "the cancellation terminal is unique and last: {events:?}"
        );
        assert_eq!(
            result.conversation.revision(),
            crate::conversation::SurfaceRevision::INITIAL
        );
        assert!(
            result.messages().is_empty(),
            "no request-scoped context was committed"
        );
        assert_eq!(
            adapter.request_count(),
            0,
            "the provider request never started"
        );
    }

    /// Issue #12 (M9b): the exact arbitration race, start winner. The
    /// execution is parked **inside** the arbitration — the start gate is
    /// held, the cancellation check has passed, and the durable commit is
    /// pending. A concurrent canceller's `cancel()` provably cannot complete
    /// while the gate is held; once the commit is released the start fact
    /// is durable, the provider request starts, and the late cancellation
    /// settles that started request as post-start cancellation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_commit_wins_the_arbitration_race_against_cancellation() {
        let adapter = Arc::new(ParkedUntilCancelledAdapter::default());
        let adapter_dyn: Arc<dyn ModelAdapter> = adapter.clone();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, _, pre_commit) = StartBoundaryPause::install(false, true);
        let mut pre_commit = pre_commit.expect("pre-commit phase installed");
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let mut execution = AgentExecution::new(
            request_dyn(&adapter_dyn),
            lease,
            &cancellation,
            runtime_dyn(&adapter_dyn),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.install_start_boundary_pause(pause);
        let canceller = cancellation.clone();
        let controller = tokio::spawn(async move {
            pre_commit.await_park(1).await;
            // Release the parked commit, then cancel: the cancel can only
            // complete after the gate is dropped at the end of the durable
            // start commit, so cancellation provably linearizes after the
            // start fact. The parked provider stream then makes the attempt
            // outcome deterministic regardless of the post-commit schedule.
            pre_commit.release();
            canceller.cancel();
        });
        let result = execution.run().await;
        controller.await.expect("canceller controller");

        assert_eq!(
            adapter.request_count(),
            1,
            "the provider request started exactly once"
        );
        let events = event_history(tool_runtime.durable_store().as_ref());
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
                .count(),
            1,
            "exactly one start fact: the durable commit won the race"
        );
        assert_eq!(
            request_snapshot_history(tool_runtime.durable_store().as_ref()).len(),
            1,
            "exactly one started Request Snapshot"
        );
        assert!(
            matches!(
                result.outcome,
                crate::events::types::AttemptOutcome::Cancelled {
                    reason: CancellationReason::UserRequested
                }
            ),
            "the post-start cancellation settles the started request"
        );
        assert!(
            matches!(events.last(), Some(RuntimeEvent::AttemptCancelled { .. }))
                && events
                    .iter()
                    .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
                    .count()
                    == 1,
            "the cancellation terminal is unique and last: {events:?}"
        );
        assert!(
            !result
                .messages()
                .iter()
                .any(|message| matches!(message, MessageBlock::Assistant(_))),
            "the cancelled in-flight request commits no assistant message"
        );
    }

    /// Issue #12 (M9b): a durable failure **inside** the fused start
    /// transaction — after the request-scoped context append — rolls the
    /// whole transaction back: no context, no snapshot, no
    /// `ModelRequestStarted`, no provider invocation. The attempt settles
    /// with the honest durable-store failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_start_commit_after_context_append_rolls_back_everything() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }]]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new("conv-1"))
                .expect("store"),
        );
        store.arm_request_start_fault_script([
            crate::durable::sqlite::RequestStartFaultOperation::AfterContextAppend,
        ]);
        let mut assembly = crate::context::ContextAssembly::new();
        assembly
            .register_extension(
                "static.test",
                Some("package-v1".to_owned()),
                Arc::new(StaticContributor),
            )
            .expect("register static contributor");
        let request = request(&adapter);
        let runtime = ContextRuntime::for_attempt_with_assembly(
            crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(crate::context::DefaultTokenEstimator),
            crate::context::AgentStatusComposer::default(),
            assembly,
            &request.model,
        )
        .expect("valid context runtime");
        let tool_runtime = tool_runtime_with_store("conv-1", Some(store.clone()));
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = AgentExecution::new(
            request,
            lease,
            &cancellation,
            runtime,
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await;

        assert!(
            matches!(
                &result.outcome,
                crate::events::types::AttemptOutcome::Failed { error }
                    if format!("{error:?}").contains("request start could not be committed durably")
            ),
            "the attempt settles with the honest durable-store failure: {:?}",
            result.outcome
        );
        assert_eq!(
            adapter.request_count(),
            0,
            "the provider request never started"
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .is_empty(),
            "no started Request Snapshot exists"
        );
        let journal = store.read_events(None, 128).expect("journal").events;
        assert!(
            !journal.iter().any(|envelope| matches!(
                envelope.event,
                crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
            )),
            "no request-start fact exists"
        );
        assert!(
            store.load_canonical().expect("canonical").is_empty(),
            "the staged context rolled back with the start transaction"
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .expect("snapshots")
                .snapshots
                .is_empty(),
            "no snapshot exists"
        );
        assert!(
            matches!(
                journal.last().map(|envelope| &envelope.event),
                Some(crate::events::types::RuntimeEvent::AttemptFailed { .. })
            ),
            "the attempt settles with the durable failure terminal"
        );
    }

    /// Issue #12 (M9b): tool continuation reaches the same gate. The first
    /// request starts and completes a tool call; the second request is
    /// parked immediately before its start arbitration, and cancelling
    /// there is again cancellation-before-start: no second provider request,
    /// no second `ModelRequestStarted`, while the first request's durable
    /// facts (including the Tool message) remain.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_before_continuation_start_stops_the_tool_turn() {
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![
            tool_call_script(&call),
            vec![ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }],
        ]));
        let mut tools = ToolRegistry::new();
        tools
            .register(
                InstantTool::definition("tool-alpha", "alpha"),
                std::sync::Arc::new(InstantTool),
            )
            .expect("register tool");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
        let mut pre_start = pre_start.expect("pre-start phase installed");
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let mut execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.install_start_boundary_pause(pause);
        let controller_cancellation = cancellation.clone();
        let controller = tokio::spawn(async move {
            // Request #1 reaches its start arbitration: let it start and
            // complete (including the tool execution).
            pre_start.await_park(1).await;
            pre_start.release();
            // The tool continuation reaches the same gate for request #2:
            // cancel before that start.
            pre_start.await_park(2).await;
            controller_cancellation.cancel();
            pre_start.release();
        });
        let result = execution.run().await;
        controller.await.expect("controller task");
        let events = event_history(tool_runtime.durable_store().as_ref());

        assert_eq!(
            adapter.request_count(),
            1,
            "the second provider request never started"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
                .count(),
            1,
            "exactly one ModelRequestStarted: the continuation start never committed"
        );
        assert!(
            result
                .messages()
                .iter()
                .any(|message| matches!(message, MessageBlock::Tool(_))),
            "the first request's Tool message remains a durable fact"
        );
        assert!(
            matches!(
                result.outcome,
                crate::events::types::AttemptOutcome::Cancelled {
                    reason: CancellationReason::UserRequested
                }
            ),
            "the continuation settles cancelled before its start"
        );
        assert!(
            matches!(events.last(), Some(RuntimeEvent::AttemptCancelled { .. }))
                && events
                    .iter()
                    .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
                    .count()
                    == 1,
            "the cancellation terminal is unique and last: {events:?}"
        );
    }

    /// Issue #12 (M9b): an inert lifecycle and an attached-but-empty
    /// lifecycle (a counting `AlwaysEnter` policy plus the native
    /// no-deferred-context observer) reach the same start boundary and
    /// produce the same never-started outcome.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inert_and_attached_lifecycles_share_the_start_boundary() {
        for attached in [false, true] {
            let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }]]));
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
            let mut pre_start = pre_start.expect("pre-start phase installed");
            // Each iteration gets its own in-memory durable authority; the
            // default temp-dir store would be shared across iterations.
            let tool_runtime = tool_runtime_with_store(
                "conv-1",
                Some(Arc::new(
                    crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                        "conv-1",
                    ))
                    .expect("store"),
                )),
            );
            let (_dir, _coordinator, lease) =
                capability_lease(ToolRegistry::new(), &tool_runtime).await;
            let evaluations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let lifecycle = if attached {
                crate::agent::AttemptLifecycle::inert()
                    .with_pre_step_policy(Arc::new(CountingEnterPolicy {
                        evaluations: Arc::clone(&evaluations),
                    }))
                    .with_native_tool_result_observer(Arc::new(crate::agent::NoDeferredContext))
                    .expect("native observer binding")
            } else {
                crate::agent::AttemptLifecycle::inert()
            };
            let mut execution = AgentExecution::new(
                request(&adapter),
                lease,
                &cancellation,
                runtime(&adapter),
                &tool_runtime,
                lifecycle,
            )
            .expect("conversation identity matches the tool runtime");
            execution.install_start_boundary_pause(pause);
            let controller_cancellation = cancellation.clone();
            let controller = tokio::spawn(async move {
                pre_start.await_park(1).await;
                controller_cancellation.cancel();
                pre_start.release();
            });
            let result = execution.run().await;
            controller.await.expect("controller task");

            assert!(
                matches!(
                    result.outcome,
                    crate::events::types::AttemptOutcome::Cancelled {
                        reason: CancellationReason::UserRequested
                    }
                ),
                "attached={attached}: cancellation before start settles cancelled"
            );
            assert_eq!(
                adapter.request_count(),
                0,
                "attached={attached}: the provider request never started"
            );
            assert!(
                request_snapshot_history(tool_runtime.durable_store().as_ref()).is_empty(),
                "attached={attached}: no started Request Snapshot"
            );
            assert!(
                result.messages().is_empty(),
                "attached={attached}: no request-scoped context committed"
            );
            if attached {
                assert!(
                    evaluations.load(std::sync::atomic::Ordering::SeqCst) >= 1,
                    "the attached policy was genuinely evaluated"
                );
            }
        }
    }

    /// A request + context runtime with one counting test contributor that
    /// returns a single User-context proposal.
    fn counting_context_request(
        adapter: &Arc<ScriptedAdapter>,
    ) -> (
        AgentExecutionRequest,
        ContextRuntime,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let invocation_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_for_contributor = Arc::clone(&invocation_count);
        let mut assembly = crate::context::ContextAssembly::new();
        assembly
            .register_extension(
                "test.context",
                Some("package-v1".to_owned()),
                Arc::new(move |_: &crate::context::ContributorInputSnapshot| {
                    count_for_contributor.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(vec![crate::context::ContextProposal::UserMessage(
                        crate::context::UserMessageProposal {
                            content: vec![UserContentBlock::Text(
                                crate::message::content::TextBlock {
                                    text: "proposal exists before start".to_owned(),
                                },
                            )],
                        },
                    )])
                }),
            )
            .expect("register test contributor");
        let request = request(adapter);
        let runtime = ContextRuntime::for_attempt_with_assembly(
            crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(crate::context::DefaultTokenEstimator),
            crate::context::AgentStatusComposer::default(),
            assembly,
            &request.model,
        )
        .expect("valid context runtime");
        (request, runtime, invocation_count)
    }

    /// An adapter whose stream parks until the invocation's cancellation
    /// signal fires, then fails with `Cancelled` — the execution-level
    /// analogue of the scripted suites' `FakeStep::ParkUntilCancelled`.
    #[derive(Default)]
    struct ParkedUntilCancelledAdapter {
        requests: Mutex<Vec<ModelRequest>>,
    }

    impl ParkedUntilCancelledAdapter {
        fn request_count(&self) -> usize {
            self.requests.lock().expect("request lock").len()
        }
    }

    impl ModelAdapter for ParkedUntilCancelledAdapter {
        fn protocol(&self) -> ModelProtocol {
            chat_protocol()
        }

        fn stream(
            &self,
            request: ModelRequest,
            cancellation: CancellationSignal,
        ) -> ModelEventStream {
            self.requests.lock().expect("request lock").push(request);
            Box::pin(futures_util::stream::once(async move {
                cancellation.cancelled().await;
                ModelEvent::Failed {
                    error: crate::model::error::ModelError {
                        kind: ModelErrorKind::Cancelled,
                        message: "cancelled".to_owned(),
                        retry_after_ms: None,
                        provider_code: None,
                    },
                }
            }))
        }
    }

    /// A contributor returning one static User-context proposal.
    struct StaticContributor;

    impl crate::context::ContextContributor for StaticContributor {
        fn contribute<'a>(
            &'a self,
            _input: &'a crate::context::ContributorInputSnapshot,
        ) -> BoxFuture<
            'a,
            Result<Vec<crate::context::ContextProposal>, crate::context::ContextAssemblyError>,
        > {
            Box::pin(async {
                Ok(vec![crate::context::ContextProposal::UserMessage(
                    crate::context::UserMessageProposal {
                        content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                            text: "staged context".to_owned(),
                        })],
                    },
                )])
            })
        }
    }

    /// A pre-step policy that counts its evaluations, then always enters.
    struct CountingEnterPolicy {
        evaluations: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::agent::PreStepPolicy for CountingEnterPolicy {
        fn evaluate<'a>(
            &'a self,
            _batch: &'a crate::agent::PreStepBatch<'a>,
        ) -> BoxFuture<'a, Result<crate::agent::PreStepDecision, crate::agent::LifecycleError>>
        {
            self.evaluations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(crate::agent::PreStepDecision::Enter) })
        }
    }

    /// A [`request`] variant over a `dyn` adapter handle.
    fn request_dyn(adapter: &Arc<dyn ModelAdapter>) -> AgentExecutionRequest {
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id: ConversationId::new("conv-1"),
            attempt_id: AttemptId::new("attempt-1"),
            conversation: ConversationState::new(),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            timezone: None,
            model: scripted_session_model(adapter.clone()).snapshot(),
        }
    }

    /// A [`runtime`] variant over a `dyn` adapter handle.
    fn runtime_dyn(adapter: &Arc<dyn ModelAdapter>) -> ContextRuntime {
        ContextRuntime::for_attempt(
            crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(crate::context::DefaultTokenEstimator),
            crate::context::AgentStatusComposer::default(),
            &request_dyn(adapter).model,
        )
        .expect("valid context runtime")
    }

    /// Cancellation may become observable while a contributor is awaiting
    /// bounded work. The contributor still settles its transient proposal,
    /// then the one start arbitration linearizes cancellation before any
    /// canonical context, Surface revision, snapshot, or provider request
    /// exists (Issue #12, M9b).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_during_awaited_context_assembly_commits_nothing() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }]]));
        let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (entered, mut entered_rx) = watch::channel(false);
        let (release, release_rx) = watch::channel(false);
        let mut assembly = crate::context::ContextAssembly::new();
        assembly
            .register_extension(
                "awaited.test",
                Some("package-v1".to_owned()),
                Arc::new(AwaitingContributor {
                    entered,
                    release: release_rx,
                    invocations: Arc::clone(&invocations),
                }),
            )
            .expect("register awaited contributor");
        let request = request(&adapter);
        let runtime = ContextRuntime::for_attempt_with_assembly(
            crate::context::SessionContextPolicy {
                reserve_tokens: 0,
                keep_recent_tokens: 0,
                summary_output_cap: None,
            },
            Arc::new(crate::context::DefaultTokenEstimator),
            crate::context::AgentStatusComposer::default(),
            assembly,
            &request.model,
        )
        .expect("valid context runtime");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let controller_cancellation = cancellation.clone();
        let controller = tokio::spawn(async move {
            entered_rx
                .wait_for(|entered| *entered)
                .await
                .expect("awaited contributor entered");
            controller_cancellation.cancel();
            release.send_replace(true);
        });
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let result = AgentExecution::new(
            request,
            lease,
            &cancellation,
            runtime,
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await;
        controller.await.expect("cancellation controller");
        let snapshots = request_snapshot_history(tool_runtime.durable_store().as_ref());

        assert_eq!(
            invocations.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the awaited contributor is invoked once"
        );
        assert!(matches!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ));
        assert!(snapshots.is_empty());
        assert_eq!(
            result.conversation.revision(),
            crate::conversation::SurfaceRevision::INITIAL
        );
        assert!(result.messages().is_empty());
        assert_eq!(adapter.request_count(), 0);
    }

    /// Once the model-turn start has committed, a provider failure does not
    /// roll back the canonical context fact, its Surface revision, or the
    /// frozen provider-neutral request boundary.
    #[tokio::test]
    async fn post_start_provider_failure_preserves_historical_facts() {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Failed {
            error: crate::model::error::ModelError {
                kind: ModelErrorKind::ProviderError,
                message: "provider failed after start".to_owned(),
                retry_after_ms: None,
                provider_code: None,
            },
        }]]));
        let mut request = request(&adapter);
        let inbound = inbound_message("fresh-1", "fresh input");
        let fresh = crate::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("fresh-1")])
            .expect("fresh trigger");
        request.conversation = ConversationState::from_messages(vec![MessageBlock::User(inbound)])
            .expect("canonical inbound history");
        request.initial_turn_trigger = InitialTurnTrigger::FreshInbound(fresh);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) =
            capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let result = AgentExecution::new(
            request,
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await;
        let snapshots = request_snapshot_history(tool_runtime.durable_store().as_ref());

        assert!(matches!(
            result.outcome,
            crate::events::types::AttemptOutcome::Failed { .. }
        ));
        assert_eq!(adapter.request_count(), 1);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(result.conversation.revision().get(), 2);
        assert!(result.messages().iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user)
                    if user.kind == InboundKind::Context(ContextKind::AgentStatus)
            )
        }));
        let reconstructed = tool_runtime
            .durable_store()
            .reconstruct_model_request(&snapshots[0].request_id)
            .expect("historical reconstruction");
        assert_eq!(reconstructed, adapter.requests()[0]);
    }

    /// The generic turn-boundary invariant with no mailbox attached: turn 1
    /// completes with a tool call and its result, the test control point
    /// makes cancellation observable after the turn (and all of its work)
    /// completed but before the next turn begins, and the generic
    /// pre-next-turn checkpoint settles the attempt cancelled — the second
    /// model turn never starts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_at_turn_boundary_stops_next_model_request_without_mailbox() {
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![tool_call_script(&call)]));
        let mut tools = ToolRegistry::new();
        tools
            .register(
                InstantTool::definition("tool-alpha", "alpha"),
                std::sync::Arc::new(InstantTool),
            )
            .expect("register tool");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
        let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

        let tool_runtime = tool_runtime("conv-1");
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution
            .continuation_pause
            .lock()
            .expect("continuation pause lock")
            .replace(pause);
        let result = execution.run().await;
        controller.await.expect("controller task");
        let events = event_history(tool_runtime.durable_store().as_ref());

        assert_eq!(
            adapter.request_count(),
            1,
            "exactly one model request total: the second model turn never begins"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, crate::events::types::RuntimeEvent::TurnStarted))
                .count(),
            1,
            "exactly one TurnStarted total"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
                    )
                })
                .count(),
            1,
            "exactly one ModelRequestStarted total"
        );
        assert_eq!(
            events,
            expected_trace(),
            "the exact trace ends with the single AttemptCancelled terminal event"
        );
        assert_eq!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            }
        );
    }

    /// The drain+append commit-point interleaving on the generic boundary
    /// hook: a tool turn completes, the safe boundary atomically drains
    /// batch A and appends it to canonical history, the continuation
    /// boundary control point makes cancellation observable there, and
    /// after the release the generic pre-next-turn checkpoint prevents any
    /// second model turn — mailbox commit semantics and generic Agent Loop
    /// cancellation compose.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_after_drain_append_stops_before_the_next_turn() {
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![tool_call_script(&call)]));
        let mut tools = ToolRegistry::new();
        tools
            .register(
                InstantTool::definition("tool-alpha", "alpha"),
                std::sync::Arc::new(InstantTool),
            )
            .expect("register tool");
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new("conv-1"))
                .expect("in-memory store"),
        );
        let tool_runtime = tool_runtime_with_store("conv-1", Some(store.clone()));
        let mailbox = tool_runtime.mailbox();
        mailbox
            .enqueue(inbound_message("msg-a", "A"))
            .expect("enqueue A before the attempt");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, reached_rx, release_tx) = ContinuationBoundaryPause::install();
        let controller = boundary_controller(reached_rx, release_tx, cancellation.clone());

        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution
            .continuation_pause
            .lock()
            .expect("continuation pause lock")
            .replace(pause);
        let result = execution.run().await;
        controller.await.expect("controller task");
        let events = event_history(store.as_ref());

        assert_eq!(
            adapter.request_count(),
            1,
            "no next model turn begins after the drained batch is committed"
        );
        assert_eq!(
            events,
            expected_trace(),
            "the exact trace ends with the single AttemptCancelled terminal event"
        );
        assert_eq!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            }
        );
        let committed: Vec<&MessageBlock> = result
            .messages()
            .iter()
            .filter(|block| {
                matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-a"))
            })
            .collect();
        assert_eq!(
            committed.len(),
            1,
            "the drained batch appears exactly once in canonical history"
        );
        assert!(
            mailbox.select_pending_batch().expect("select").is_none(),
            "the adopted batch is consumed from the durable inbox and never requeued"
        );
    }

    /// A parking background executor: its returned future reports entry,
    /// waits for durable release state, and then settles with a fixed result.
    struct ParkingBackgroundTool {
        definition: ToolDefinition,
        started: tokio::sync::watch::Sender<bool>,
        release: tokio::sync::watch::Sender<bool>,
    }

    impl ParkingBackgroundTool {
        fn new() -> (
            Self,
            tokio::sync::watch::Receiver<bool>,
            tokio::sync::watch::Sender<bool>,
        ) {
            let (started, started_rx) = tokio::sync::watch::channel(false);
            let (release, _release_rx) = tokio::sync::watch::channel(false);
            (
                Self {
                    definition: ToolDefinition {
                        id: ToolId::new("tool-bg"),
                        name: "bg".to_owned(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type": "object"}),
                        execution_policy: ToolExecutionPolicy::ModelSelectable,
                        concurrency_policy: ToolConcurrencyPolicy::Sequential,
                        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
                        replay_policy: ToolReplayPolicy::Never,
                        origin: ToolOrigin::Builtin,
                    },
                    started,
                    release: release.clone(),
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for ParkingBackgroundTool {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: crate::tools::executor::ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            let started = self.started.clone();
            let mut release = self.release.subscribe();
            Box::pin(async move {
                started.send_replace(true);
                release
                    .wait_for(|released| *released)
                    .await
                    .expect("release channel stays open");
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                }
            })
        }
    }

    /// The outer liveness guard of the deterministic mailbox-boundary test.
    ///
    /// Nothing in that test's ordering proof depends on this value: every
    /// semantic step is an exact handshake. It bounds only total wall time
    /// so a genuine regression fails with a message instead of hanging a
    /// CI job, and it is deliberately far larger than any scheduling delay
    /// a loaded runner can produce.
    const LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    /// Exact deterministic proof for the background terminal inbound.
    ///
    /// The production finite-watermark contract under test (Issue #63):
    ///
    /// ```text
    /// once a safe-boundary selection froze its finite watermark, an inbound
    /// whose durable acceptance linearizes after that selection can never
    /// join the selected batch — it belongs to the next safe boundary
    /// ```
    ///
    /// The test constructs the exact happens-before chain using only the
    /// test-only continuation-boundary pause and the parking background
    /// executor; no sleep or scheduler-timing assumption participates in the
    /// proof:
    ///
    /// ```text
    /// [human] durably accepted before the attempt starts (sequence 1)
    /// turn 1 (continuation) dispatches the parking background tool
    /// turn 1 completes → safe-boundary selection/adoption #1 commits
    ///   [human] (sequence 1) — the first finite batch is frozen forever
    /// Agent Loop parks at the continuation boundary before turn 2
    /// background runner settled → terminal durably accepted (sequence 2)
    ///   — provably after adoption #1 (the loop is parked)
    /// Agent Loop released → turn 2 request observes [human], no terminal
    /// turn 2 completes → adoption #2 commits [terminal] (sequence 2)
    /// turn 3 request observes the terminal exactly once
    /// ```
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn terminal_inbound_after_snapshot_can_never_join_the_first_batch() {
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new("conv-1"))
                .expect("in-memory store"),
        );
        let tool_runtime = tool_runtime_with_store("conv-1", Some(store.clone()));
        let mailbox = tool_runtime.mailbox();
        mailbox
            .enqueue(inbound_message("msg-human", "hello"))
            .expect("accept the human message");

        let call = ToolCall {
            id: ToolCallId::new("call-bg"),
            tool_id: ToolId::new("tool-bg"),
            name: "bg".to_owned(),
            arguments: serde_json::json!({"execution_mode": "background"}),
        };
        let adapter = Arc::new(ScriptedAdapter::new(vec![
            tool_call_script(&call),
            vec![
                ModelEvent::Started,
                ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "turn two".to_owned(),
                },
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                },
            ],
            vec![
                ModelEvent::Started,
                ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "turn three".to_owned(),
                },
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                },
            ],
        ]));
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let mut tools = ToolRegistry::new();
        tools
            .register(tool.definition.clone(), Arc::new(tool))
            .expect("register bg tool");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let background = tool_runtime.background().clone();
        let (pause, mut pause_reached, pause_release) = ContinuationBoundaryPause::install();
        let controller = tokio::spawn(async move {
            // 1. The detached background runner is provably started.
            tokio::time::timeout(LIVENESS_GUARD, started.wait_for(|started| *started))
                .await
                .expect("bg runner start wait exceeded liveness guard")
                .expect("bg runner started");
            // 2. Turn 1 completed and adoption #1 committed [human]; the
            //    loop is parked before turn 2, so no later selection can
            //    run until it is released.
            tokio::time::timeout(LIVENESS_GUARD, pause_reached.wait_for(|reached| *reached))
                .await
                .expect("continuation wait exceeded liveness guard")
                .expect("continuation boundary reached");
            // 3. Settle the runner: its terminal inbound is durably accepted
            //    after adoption #1 already froze the first finite batch. The
            //    registry terminal observation proves the durable enqueue
            //    completed (finish publishes before notifying state).
            release.send_replace(true);
            background
                .wait_until_terminal(&crate::runtime::identity::ToolExecutionId::new("exec_1"))
                .await
                .expect("the terminal state is durably published");
            // 4. Release the boundary twice: turn 2 runs and adopts
            //    [terminal] at its own safe boundary; turn 3 then observes
            //    the terminal as its fresh inbound.
            pause_release
                .send(())
                .expect("release the first continuation boundary");
            pause_release
                .send(())
                .expect("release the second continuation boundary");
        });
        let (_dir, _coordinator, lease) = capability_lease(tools, &tool_runtime).await;
        let execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution
            .continuation_pause
            .lock()
            .expect("continuation pause lock")
            .replace(pause);
        // Outer liveness guards only: every step above is an exact
        // handshake, so these bounds never participate in the proof — they
        // exist so a regression fails loudly instead of hanging.
        let _result = tokio::time::timeout(LIVENESS_GUARD, execution.run())
            .await
            .expect("the attempt terminates");
        tokio::time::timeout(LIVENESS_GUARD, controller)
            .await
            .expect("the controller terminates")
            .expect("controller task");

        let requests = adapter.requests.lock().expect("requests lock").clone();
        assert_eq!(
            requests.len(),
            3,
            "the constructed ordering is exactly: human turn, terminal turn, stop turn"
        );
        let second_request = &requests[1];
        assert!(
            second_request.messages.iter().any(|message| {
                matches!(message, MessageBlock::User(user) if user.id == MessageId::new("msg-human"))
            }),
            "the human message joins the second request"
        );
        assert!(
            !second_request.messages.iter().any(|message| {
                matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")
            }),
            "the terminal can never appear in the first drained batch"
        );
        assert!(
            requests[2]
                .messages
                .iter()
                .any(|message| matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")),
            "the terminal inbound waits for the next drained batch"
        );
        let terminal_occurrences = requests
            .iter()
            .flat_map(|request| &request.messages)
            .filter(|message| {
                matches!(message, MessageBlock::User(user) if user.id.as_str() == "background-exec_1-terminal")
            })
            .count();
        assert_eq!(
            terminal_occurrences, 1,
            "the terminal inbound is drained and committed exactly once"
        );
        assert!(
            mailbox.select_pending_batch().expect("select").is_none(),
            "the durable inbox is drained"
        );
    }
}
