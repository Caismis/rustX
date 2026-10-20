//! The Runtime Client event vocabulary.
//!
//! [`RuntimeClientEvent`] is the provider-neutral external event shape of
//! the Runtime Client projection. It is deliberately **not** the internal
//! [`RuntimeEvent`](crate::events::types::RuntimeEvent) vocabulary:
//!
//! - every variant is a deliberate projection with an explicit mapping
//!   policy (PROJECT / FOLD INTO CLIENT STATE ONLY / INTERNAL), defined by
//!   the projection owner;
//! - provider/request mechanics stay internal unless they express a
//!   client-relevant semantic fact;
//! - native, MCP, and Python tool execution converge through one generic
//!   tool-lifecycle shape;
//! - no process ids, no supervisor internals, no MCP SDK objects, no
//!   Python worker internals, and no provider wire objects appear.
//!
//! The internal `RuntimeEvent` schema can therefore evolve without
//! breaking the Runtime Client protocol.
//!
//! Every attempt-scoped event carries its `attempt_id`, so events are
//! self-describing for clients that attach mid-attempt.

use serde::{Deserialize, Serialize};

use crate::observation::status::AgentStatusView;

use crate::observation::capability_source::CapabilityView;

use super::snapshot::{RuntimeClientAgent, RuntimeClientContextView, RuntimeClientJob};

use crate::observation::resource_source::RuntimeClientResourcesView;

use crate::events::types::AttemptLimit;
use crate::message::types::{ContentBlockIndex, MessageBlock, UserMessageBlock};
use crate::model::finish::ModelFinishReason;
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::observation::failure::RuntimeClientAttemptFailure;
use crate::observation::transcript::{
    RuntimeClientTranscriptCursor, RuntimeClientTranscriptInteractionRequested,
    RuntimeClientTranscriptInteractionSettled,
};
use crate::publication::PublicationAudit;
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolExecutionId, ToolId};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::interaction::{InteractionOutcome, InteractionRef};
use crate::runtime::types::CancellationReason;
use crate::tools::types::{ToolExecutionResult, ToolProgress};

/// One externally visible Runtime Client observation.
///
/// The stable `type` discriminator is the protocol contract; unknown
/// fields are rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientEvent {
    /// One authoritative transition may carry several finite facts, but owns
    /// exactly one published Cursor and must fold atomically.
    ProjectionTransition {
        events: Vec<RuntimeClientEvent>,
        metadata: super::checkpoint::ProjectionMetadata,
    },
    /// An already folded transition whose growing fields are represented by
    /// a complete finite cut before publication, never rewritten by transport.
    CheckpointAdvanced {
        transition: String,
        checkpoint: Box<super::checkpoint::RuntimeClientCheckpoint>,
    },
    BlockContentAdvanced {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        position: u64,
        attempt_id: AttemptId,
        message_id: MessageId,
        block_index: ContentBlockIndex,
        kind: String,
        base_length: u64,
        end_length: u64,
        content_revision: u64,
        inline_delta: Option<String>,
        content_ref: crate::observation::content::ContentRef,
    },
    /// Native derived read domains at a committed journal cut. Published only
    /// when durable facts change, never for publication text/argument suffixes.
    ReadDomainsUpdated {
        projection_head: Option<crate::observation::head::RuntimeClientProjectionHeadView>,
        transcript: crate::observation::transcript::RuntimeClientTranscriptPage,
        trace: crate::observation::trace::TracePage,
        occupancy: Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
        todos: Option<crate::tools::todo::TodoSnapshot>,
    },
    /// Independent Trace invalidation; reread Trace, not the Session snapshot.
    TraceChanged,
    /// The live runtime's configuration-adoption eligibility changed.
    ///
    /// Published only on an actual change of the native derivation, never for
    /// ordinary streaming. Advisory: `session/adoptConfiguration` revalidates
    /// the real gate at its commit boundary.
    ConfigurationAdoptionEligibilityChanged {
        eligibility: crate::local_runtime::configuration::application::AdoptionEligibility,
    },
    /// Bounded authoritative durable Goal view. Every emission follows a
    /// committed durable transition; there is no activation-only form.
    GoalChanged {
        view: crate::observation::goal::RuntimeClientGoalView,
    },
    /// Complete bounded replacement at one native Workflow revision.
    WorkflowsUpdated {
        workflows: crate::runtime::workflow::read_model::WorkflowSnapshot,
    },
    /// An attempt started executing.
    ///
    /// The event is self-contained: it carries the immutable model snapshot
    /// the attempt froze at admission, so a continuously subscribed client
    /// knows the attempt's authoritative model without inferring it from
    /// event ordering and without a second `checkpoint_get` round trip. The
    /// value is runtime-owned — a client never supplies or derives it — and
    /// it is exactly [`RuntimeClientAttempt::model`]
    /// ([`super::snapshot::RuntimeClientAttempt`]) of the same attempt.
    ///
    /// It is deliberately **not** the session's desired model: while this
    /// attempt runs on model A and the session is switched to model B, this
    /// event keeps reporting A and
    /// [`SessionModelChanged`](Self::SessionModelChanged) reports B.
    AttemptStarted {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// The immutable model snapshot the attempt froze at admission.
        model: Option<Box<AttemptModelView>>,
        /// Native frozen admission evidence, absent when unavailable.
        execution_settings: Option<super::settings::AdmittedSettings>,
    },
    /// The attempt settled. Exactly one terminal settlement exists per
    /// attempt; the outcome is the platform-level settlement, never a
    /// provider-native object.
    AttemptSettled {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// The platform-level settlement.
        outcome: RuntimeClientOutcome,
    },
    /// The current attempt turn count changed.
    AttemptTurnUpdated {
        /// The attempt whose turn count changed.
        attempt_id: AttemptId,
        /// The exact folded number of completed turns.
        turn: u32,
    },
    /// The current attempt's latest normalized model-request usage changed.
    AttemptUsageUpdated {
        /// The attempt whose usage changed.
        attempt_id: AttemptId,
        /// The exact normalized usage folded into the attempt view.
        usage: crate::model::types::ModelUsage,
    },
    /// A native interaction is pending. The request is authoritative runtime
    /// projection state; it is not a client-owned prompt.
    PendingHeadChanged {
        owner: String,
        cut: crate::runtime::identity::ObservationCollectionCut,
        head: Vec<crate::observation::interaction::RuntimeClientInteractionHeader>,
    },
    InteractionPending {
        /// The complete routed interaction projection.
        interaction: crate::observation::interaction::RuntimeClientInteractionHeader,
    },
    /// A native interaction was removed from the live pending projection.
    InteractionSettled {
        /// The terminal routed interaction identity.
        interaction: InteractionRef,
        /// The exact terminal rendezvous outcome.
        outcome: InteractionOutcome,
    },
    /// A child died or otherwise lost its process-owned interaction authority.
    /// This removes the root presentation without inventing a response or a
    /// child settlement outcome.
    InteractionRemoved {
        /// The no-longer-actionable routed identity.
        interaction: InteractionRef,
    },
    /// A durable interaction request audit became visible in the transcript.
    ///
    /// This is historical audit evidence, not a second pending waiter.
    InteractionAuditRequested {
        /// The bounded requested audit projection.
        audit: Box<RuntimeClientTranscriptInteractionRequested>,
        /// The durable transcript position of this audit.
        transcript_cursor: RuntimeClientTranscriptCursor,
    },
    /// A durable interaction settlement audit became visible in the
    /// transcript. It is never actionable after publication.
    InteractionAuditSettled {
        /// The bounded settled audit projection.
        audit: Box<RuntimeClientTranscriptInteractionSettled>,
        /// The durable transcript position of this audit.
        transcript_cursor: RuntimeClientTranscriptCursor,
    },
    /// A context compaction operation began.
    ContextCompactionStarted {
        /// Authoritative context state at this lifecycle boundary.
        context: RuntimeClientContextView,
        /// The owning attempt for automatic compaction; absent for manual
        /// idle maintenance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_id: Option<AttemptId>,
    },
    /// A context compaction operation failed before a semantic commit.
    ContextCompactionFailed {
        /// Authoritative context state at this lifecycle boundary.
        context: RuntimeClientContextView,
        /// The owning attempt for automatic compaction; absent for manual
        /// idle maintenance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_id: Option<AttemptId>,
        /// The runtime-owned failure diagnostic.
        error: String,
    },
    /// A durable typed checkpoint and projection activation were committed.
    ///
    /// This is the semantic completion fact for clients: the semantic
    /// compaction commit already happened, and the carried snapshot metadata
    /// is sufficient to observe projection advancement and token-measurement
    /// provenance without exposing summary text or provider credentials.
    ContextCompacted {
        /// The owning attempt for automatic compaction; absent for manual
        /// idle maintenance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_id: Option<AttemptId>,
        /// The context diagnostics after this committed compaction.
        context: RuntimeClientContextView,
    },

    /// Assembly of a canonical Assistant message began.
    AssistantMessageStarted {
        /// The attempt streaming the message.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
    },
    /// A tool call within the in-flight Assistant message started.
    ToolCallStarted {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        position: u64,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        foreground_position: u64,
        /// The attempt assembling the call.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// The tool-call identity and metadata known at start.
        call: super::snapshot::RuntimeClientToolCall,
    },
    /// A tool call within the in-flight Assistant message finished assembly.
    ToolCallAssembled {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        position: u64,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        foreground_position: u64,
        /// Exact native assembled representation; clients must not reserialize JSON numbers.
        arguments_json: String,
        /// The attempt assembling the call.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The tool-call content block that completed.
        block_index: ContentBlockIndex,
        /// The fully assembled tool call.
        call: super::snapshot::RuntimeClientToolCall,
        arguments_ref: Option<crate::observation::content::ContentRef>,
    },

    /// The in-flight Assistant publication settled without ever becoming a
    /// canonical Assistant message (Issue #108).
    ///
    /// The carried audit is what rustX durably committed **for release**: an
    /// upper bound on what the client may have displayed, never proof that
    /// anything was perceived. Its
    /// [`ProposedToolCall`](crate::publication::PublicationAuditBlock::ProposedToolCall)
    /// entries are model proposals that were never authorized and never
    /// executed — a transcript consumer must present them differently from
    /// the [`ToolExecutionStarted`](Self::ToolExecutionStarted) /
    /// [`ToolExecutionSettled`](Self::ToolExecutionSettled) Tool Plane facts.
    /// The audit is a noncanonical derived transcript item, not a Message
    /// Ledger message or an execution fact.
    AssistantPublicationSettled {
        settlement_head:
            Vec<crate::observation::transcript::RuntimeClientPublicationSettlementRecord>,
        /// The attempt that streamed the publication.
        attempt_id: AttemptId,
        /// The bounded immutable audit of the settled stream.
        audit: Box<PublicationAudit>,
        /// The durable transcript position of this noncanonical audit.
        transcript_cursor: RuntimeClientTranscriptCursor,
    },

    /// The native stream settled, with retained audit content available by bounded reads.
    AssistantPublicationDeferredSettled {
        settlement_head:
            Vec<crate::observation::transcript::RuntimeClientPublicationSettlementRecord>,
        attempt_id: AttemptId,
        settlement: crate::observation::transcript::RuntimeClientPublicationSettlement,
        /// Bounded retained prefix, never a claim that deferred content is empty.
        audit: Option<Box<PublicationAudit>>,
        transcript_cursor: RuntimeClientTranscriptCursor,
        /// Exact immutable source evidence captured by the native audit transaction.
        read_cut: crate::durable::observation::ObservationReadCut,
        content: Vec<crate::observation::cut::ContentBinding>,
        collections: Vec<crate::observation::cut::CollectionFrontier>,
    },

    /// The same provisional block acquired its canonical immutable locator at C.
    ToolOccurrenceRebound {
        attempt_id: AttemptId,
        previous_locator: crate::observation::content::CanonicalContentLocator,
        occurrence: crate::observation::tool::RuntimeClientToolOccurrence,
        metadata_ref: Option<crate::observation::content::ContentRef>,
    },
    /// Canonical Tool result commitment; execution-terminal state remains Journal-owned.
    ToolResultCommitted {
        attempt_id: AttemptId,
        occurrence: crate::observation::tool::RuntimeClientToolOccurrence,
        result: ToolExecutionResult,
        result_deferred: bool,
        result_metadata_ref: Option<crate::observation::content::ContentRef>,
    },

    /// A foreground tool execution started.
    ToolExecutionStarted {
        /// The attempt executing the call.
        attempt_id: AttemptId,
        occurrence: Option<crate::observation::tool::RuntimeClientToolOccurrence>,
        metadata_ref: Option<crate::observation::content::ContentRef>,
        /// The logical tool-call identity.
        tool_call_id: Option<ToolCallId>,
        /// The canonical tool identity.
        tool_id: Option<ToolId>,
    },
    /// Bounded structured progress of one tool execution.
    ToolExecutionProgress {
        /// The attempt executing the call.
        attempt_id: AttemptId,
        occurrence: Option<crate::observation::tool::RuntimeClientToolOccurrence>,
        metadata_ref: Option<crate::observation::content::ContentRef>,
        /// The logical tool-call identity.
        tool_call_id: Option<ToolCallId>,
        /// The canonical tool identity.
        tool_id: Option<ToolId>,
        /// The detached runtime execution instance for background work;
        /// `None` for foreground executions (no fake id is invented).
        execution_id: Option<ToolExecutionId>,
        /// The bounded structured progress.
        progress: ToolProgress,
    },
    /// A native execution-terminal Journal fact settled this execution.
    /// Canonical result commitment travels separately through [`ToolResultCommitted`](Self::ToolResultCommitted).
    ToolExecutionSettled {
        /// The attempt executing the call.
        attempt_id: AttemptId,
        occurrence: Option<crate::observation::tool::RuntimeClientToolOccurrence>,
        metadata_ref: Option<crate::observation::content::ContentRef>,
        /// The logical tool-call identity.
        tool_call_id: Option<ToolCallId>,
        /// The canonical tool identity.
        tool_id: Option<ToolId>,
        /// The normalized execution result.
        result: ToolExecutionResult,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_source: Option<crate::observation::tool::JournalFieldRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_metadata_ref: Option<crate::observation::content::ContentRef>,
    },

    /// A canonical message was committed to conversation history.
    ///
    /// `attempt_id` is `None` for runtime-admitted messages committed
    /// between attempts (for example a drained inbound batch admitted by
    /// the conversation coordinator). The committed content is the
    /// authoritative canonical block; client projections treat it as
    /// read-only.
    MessageDescriptorCommitted {
        attempt_id: Option<AttemptId>,
        descriptor: crate::observation::transcript::RuntimeClientMessageDescriptor,
        head: Vec<crate::observation::transcript::RuntimeClientMessageDescriptor>,
        transcript_cursor: Option<RuntimeClientTranscriptCursor>,
        evicted_message_id: Option<MessageId>,
    },
    MessageCommitted {
        /// Exact display-head eviction; canonical history remains authoritative and readable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evicted_message_id: Option<MessageId>,
        /// The committing attempt, when one is active.
        attempt_id: Option<AttemptId>,
        /// The committed canonical message.
        message: MessageBlock,
        /// The durable transcript position, absent for hidden Context facts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transcript_cursor: Option<RuntimeClientTranscriptCursor>,
    },

    /// The runtime published the Agent Status committed with an existing
    /// primary model turn.
    ///
    /// The carried view derives from the exact composed status used by the
    /// model path; a client never causes a second composition.
    ///
    /// The event describes **one complete transition of the bounded status
    /// window**, not just the new composition: the admission and, when the
    /// window was full, the eviction that admission caused. A client folds
    /// both mechanically. It never applies a retention rule of its own,
    /// because the bound
    /// ([`AGENT_STATUS_WINDOW`](super::AGENT_STATUS_WINDOW)) is projection
    /// policy and is not part of this contract — which is exactly what makes
    /// folding every event through cursor `C` and replacing state from the
    /// authoritative snapshot at cursor `C` produce the same window.
    AgentStatusComposed {
        /// The attempt that composed the status.
        attempt_id: AttemptId,
        /// The turn number of the request preparation.
        turn: u32,
        /// The structured status view (sections plus the canonical derived
        /// rendering).
        status: AgentStatusView,
        /// The composition this admission pushed out of the bounded window,
        /// when the window was already full.
        ///
        /// `None` when nothing left the window — including for a replayed
        /// observation of a composition the window already holds, which
        /// admits nothing and therefore evicts nothing. A client removes
        /// exactly this identity before applying the identity-keyed
        /// admission, so replay stays idempotent and can never displace an
        /// unrelated composition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evicted_status_message_id: Option<MessageId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evicted_status_locator: Option<crate::observation::content::CanonicalContentLocator>,
    },

    /// An inbound message was enqueued into the conversation mailbox.
    ///
    /// Human and runtime-originated producers share the one inbound
    /// ordering domain; the event carries the authoritative mailbox
    /// sequence and the canonical message.
    /// Pending authority changed; reread the snapshot and transcript.
    PendingInboundChanged {
        /// Complete committed pending projection, in durable sequence order.
        pending: Vec<super::snapshot::InboundItemView>,
    },
    InboundEnqueued {
        /// The mailbox-assigned inbound sequence.
        sequence: InboundSequence,
        /// The canonical inbound message.
        message: UserMessageBlock,
        /// The durable transcript position allocated at acceptance, absent
        /// for hidden Context-kind inbound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transcript_cursor: Option<RuntimeClientTranscriptCursor>,
    },
    /// The agent loop committed one finite mailbox drain.
    ///
    /// The drain is authoritative mailbox semantics: every pending item up
    /// to the watermark was consumed together, and post-watermark arrivals
    /// wait for the next drain.
    InboundDrained {
        /// The highest selected inbound sequence.
        watermark: InboundSequence,
        /// The number of drained items.
        count: usize,
        /// The drained message identities in inbound sequence order.
        message_ids: Vec<MessageId>,
    },

    /// One background execution transitioned in the authoritative
    /// conversation registry.
    ///
    /// Background work is conversation-owned: it survives attempt
    /// termination, client detach, and client reconnect.
    /// The native collection source changed outside the loaded display head.
    CollectionChanged {
        domain: String,
        cut: crate::runtime::identity::ObservationCollectionCut,
    },
    JobUpdated {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        position: u64,
        evicted_job_id: Option<crate::runtime::identity::ToolExecutionId>,
        /// The canonical registry snapshot after the transition.
        job: RuntimeClientJob,
    },

    /// One subagent child transitioned in the authoritative conversation
    /// registry (Issue #60).
    ///
    /// Subagent children are conversation-owned: they survive attempt
    /// termination, client detach, and client reconnect.
    AgentUpdated {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        position: u64,
        evicted_agent_id: Option<crate::runtime::identity::AgentId>,
        /// The canonical registry snapshot after the transition.
        agent: Box<RuntimeClientAgent>,
    },

    /// The externally visible capability read model changed (Issue #81).
    ///
    /// This is the one capability observation, published for **both**
    /// kinds of authoritative capability commit:
    ///
    /// - an executable capability activation (a revision swap), and
    /// - an availability-only change (a source became ready or
    ///   unavailable without any change to the committed executable set).
    ///
    /// The carried [`CapabilityView`] is the complete folded projection
    /// after the commit; its `revision` tells the client whether the
    /// executable capability identity changed. A client never needs to
    /// poll `checkpoint_get` to discover an availability transition.
    CapabilityUpdated {
        /// The deterministic active capability projection.
        capabilities: CapabilityView,
    },

    /// The runtime published a new immutable resource generation: the
    /// project context files it loaded **and** the capability generation
    /// they were composed against, as one fact.
    ///
    /// A configuration commit commits both halves together, so it publishes one event
    /// carrying both. Splitting it into a capability event beside a
    /// resource event would allocate two cursors, and an incremental client
    /// that folds events — rather than re-reading `checkpoint_get` — would
    /// then hold the new capability generation beside the resource
    /// generation the same commit retired. That pairing never existed in
    /// the runtime, so it is never published.
    ///
    /// Both halves travel even when only one moved: a configuration commit that changes
    /// only an `AGENTS.md` advances the resource revision and repeats the
    /// unchanged capability view, and a configuration commit that only rebuilds tools
    /// does the converse. The carried revisions, not the event's presence,
    /// tell a client what changed.
    ///
    /// [`Self::CapabilityUpdated`] remains the observation for a capability
    /// commit that is *not* a resource generation — an availability
    /// transition or an activation committed on its own authority.
    ResourceGenerationUpdated {
        plugins: Option<super::settings::EffectivePlugins>,
        model: Option<Box<crate::model::session::SessionModelView>>,
        model_status: super::snapshot::RuntimeClientModelStatus,
        approval_mode: crate::runtime::ApprovalMode,
        /// The active capability projection this generation was composed
        /// against.
        capabilities: CapabilityView,
        /// The active runtime resource projection after the commit.
        resources: RuntimeClientResourcesView,
    },

    /// The authoritative session model configuration changed.
    ///
    /// This is the one model-configuration observation, published on the
    /// same stream and under the same linearization owner as every other
    /// Runtime Client event, so a subscribed client stays synchronized
    /// without polling. It never implies anything about a running attempt:
    /// an already-admitted attempt keeps the model it froze at admission.
    SessionModelChanged {
        /// The redacted session model state after the update.
        model: Option<Box<SessionModelView>>,
        model_status: super::snapshot::RuntimeClientModelStatus,
    },

    /// Runtime drain began and no longer admits new inbound work. Detach
    /// remains available; the correlated shutdown response is not returned
    /// until the current attempt and all other conversation-owned work
    /// settle at quiescence.
    RuntimeShutdown,

    /// The runtime's durable authority failed persistently: it has entered
    /// an explicit degraded state and no new durable admission/execution
    /// work may begin until it is reconstructed. Read-only inspection and
    /// shutdown remain available.
    RuntimeDurabilityFailed {
        /// The operation that failed persistently.
        operation: String,
        /// The human-readable failure diagnostic.
        diagnostic: String,
    },
}

/// The platform-level settlement outcome of one attempt.
///
/// Provider finish reasons, runtime cancellation, timeout, limit
/// exhaustion, and runtime failure are distinct and never collapsed into
/// one string. A normalized model failure exposes its kind, message, and
/// retry hint — never the provider-specific error code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientOutcome {
    /// The attempt completed.
    Completed {
        /// The normalized finish reason.
        finish_reason: ModelFinishReason,
        /// The original reason exceeds its finite inline preview.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        finish_reason_deferred: bool,
        /// Exact allowlisted reason bytes at the published native cut.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finish_reason_source: Option<crate::observation::tool::JournalFieldRef>,
    },
    /// The attempt was cancelled.
    Cancelled {
        /// Why the attempt was cancelled.
        reason: CancellationReason,
    },
    /// The attempt exceeded its runtime time budget.
    TimedOut,
    /// The attempt exceeded one of its execution limits.
    LimitExceeded {
        /// Which limit was exceeded.
        limit: AttemptLimit,
    },
    /// The attempt failed.
    Failed {
        /// The normalized client-visible failure.
        error: RuntimeClientAttemptFailure,
        /// The normalized diagnostic exceeds its finite inline preview.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        error_deferred: bool,
        /// Normalized diagnostic source; never a raw provider error.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_source: Option<crate::observation::tool::JournalFieldRef>,
    },
}

impl RuntimeClientEvent {
    /// The ordered facts belonging to one complete native cursor transition.
    #[must_use]
    pub fn facts(&self) -> &[Self] {
        match self {
            Self::ProjectionTransition { events, .. } => events,
            event => std::slice::from_ref(event),
        }
    }
}
