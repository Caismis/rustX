//! The Runtime Client event vocabulary (Runtime Client Protocol v2).
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
//! breaking Runtime Client Protocol v2.
//!
//! Every attempt-scoped event carries its `attempt_id`, so events are
//! self-describing for clients that attach mid-attempt.

use serde::{Deserialize, Serialize};

use super::snapshot::{
    AgentStatusView, CapabilityView, RuntimeClientBackgroundExecution, RuntimeClientContextView,
    RuntimeClientSubagent, RuntimeClientTranscriptCursor,
    RuntimeClientTranscriptInteractionRequested, RuntimeClientTranscriptInteractionSettled,
};
use crate::events::types::AttemptLimit;
use crate::message::types::{ContentBlockIndex, MessageBlock, UserMessageBlock};
use crate::model::error::ModelErrorKind;
use crate::model::finish::ModelFinishReason;
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::publication::PublicationAudit;
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolExecutionId, ToolId};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::interaction::{InteractionOutcome, InteractionRequest};
use crate::runtime::types::ApprovalMode;
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::types::{ToolCall, ToolCallStart, ToolExecutionResult, ToolProgress};

/// One externally visible Runtime Client observation.
///
/// The stable `type` discriminator is the protocol contract; unknown
/// fields are rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientEvent {
    /// An attempt started executing.
    ///
    /// The event is self-contained: it carries the immutable model snapshot
    /// the attempt froze at admission, so a continuously subscribed client
    /// knows the attempt's authoritative model without inferring it from
    /// event ordering and without a second `snapshot_get` round trip. The
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
        model: Box<AttemptModelView>,
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
    InteractionPending {
        /// The complete bounded interaction request.
        interaction: InteractionRequest,
    },
    /// A native interaction was removed from the live pending projection.
    InteractionSettled {
        /// The terminal interaction identity.
        interaction_id: crate::runtime::identity::InteractionId,
        /// The exact terminal rendezvous outcome.
        outcome: InteractionOutcome,
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
    /// The authoritative runtime `ApprovalMode` control state changed.
    ApprovalModeChanged {
        /// The mode effective for the current/next attempt boundary.
        effective_approval_mode: ApprovalMode,
        /// The latest desired mode when it is pending reconciliation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending_approval_mode: Option<ApprovalMode>,
        /// The monotonic control-plane revision.
        revision: u64,
    },
    /// A context compaction operation began.
    ContextCompactionStarted {
        /// The owning attempt for automatic compaction; absent for manual
        /// idle maintenance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_id: Option<AttemptId>,
    },
    /// A context compaction operation failed before a semantic commit.
    ContextCompactionFailed {
        /// The owning attempt for automatic compaction; absent for manual
        /// idle maintenance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt_id: Option<AttemptId>,
        /// The runtime-owned failure diagnostic.
        error: String,
    },
    /// A canonical runtime summary and Surface replacement were committed.
    ///
    /// This is the semantic completion fact for clients: the semantic
    /// compaction commit already happened, and the carried snapshot metadata
    /// is sufficient to observe Surface advancement and token-measurement
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
    /// An incremental text delta of one output block.
    AssistantTextDelta {
        /// The attempt streaming the message.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The output block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental text.
        delta: String,
    },
    /// An incremental reasoning delta of one output block.
    AssistantReasoningDelta {
        /// The attempt streaming the message.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The reasoning block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental reasoning text.
        delta: String,
    },
    /// An incremental refusal delta of one output block. Refusal is
    /// preserved as refusal, never flattened into text.
    AssistantRefusalDelta {
        /// The attempt streaming the message.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The refusal block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental refusal text.
        delta: String,
    },

    /// A tool call within the in-flight Assistant message started.
    ToolCallStarted {
        /// The attempt assembling the call.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// The tool-call identity and metadata known at start.
        call: ToolCallStart,
    },
    /// An incremental JSON argument fragment of one tool call.
    ToolCallArgumentsDelta {
        /// The attempt assembling the call.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// The tool-call identity being assembled.
        call_id: ToolCallId,
        /// The incremental JSON argument fragment.
        arguments_delta: String,
    },
    /// A tool call within the in-flight Assistant message finished assembly.
    ToolCallAssembled {
        /// The attempt assembling the call.
        attempt_id: AttemptId,
        /// The provisional message identity.
        message_id: MessageId,
        /// The tool-call content block that completed.
        block_index: ContentBlockIndex,
        /// The fully assembled tool call.
        call: ToolCall,
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
        /// The attempt that streamed the publication.
        attempt_id: AttemptId,
        /// The bounded immutable audit of the settled stream.
        audit: Box<PublicationAudit>,
        /// The durable transcript position of this noncanonical audit.
        transcript_cursor: RuntimeClientTranscriptCursor,
    },

    /// A foreground tool execution started.
    ToolExecutionStarted {
        /// The attempt executing the call.
        attempt_id: AttemptId,
        /// The logical tool-call identity.
        tool_call_id: ToolCallId,
        /// The canonical tool identity.
        tool_id: ToolId,
    },
    /// Bounded structured progress of one tool execution.
    ToolExecutionProgress {
        /// The attempt executing the call.
        attempt_id: AttemptId,
        /// The logical tool-call identity.
        tool_call_id: ToolCallId,
        /// The canonical tool identity.
        tool_id: ToolId,
        /// The detached runtime execution instance for background work;
        /// `None` for foreground executions (no fake id is invented).
        execution_id: Option<ToolExecutionId>,
        /// The bounded structured progress.
        progress: ToolProgress,
    },
    /// A tool execution or pre-tool policy settled with its normalized result
    /// (success, failure, denial, cancellation, timeout, or validation
    /// rejection all settle through this one shape).
    ToolExecutionSettled {
        /// The attempt executing the call.
        attempt_id: AttemptId,
        /// The logical tool-call identity.
        tool_call_id: ToolCallId,
        /// The canonical tool identity.
        tool_id: ToolId,
        /// The normalized execution result.
        result: ToolExecutionResult,
    },

    /// A canonical message was committed to conversation history.
    ///
    /// `attempt_id` is `None` for runtime-admitted messages committed
    /// between attempts (for example a drained inbound batch admitted by
    /// the conversation coordinator). The committed content is the
    /// authoritative canonical block; client projections treat it as
    /// read-only.
    MessageCommitted {
        /// The committing attempt, when one is active.
        attempt_id: Option<AttemptId>,
        /// The committed canonical message.
        message: MessageBlock,
        /// The durable transcript position, absent for hidden Context facts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transcript_cursor: Option<RuntimeClientTranscriptCursor>,
    },

    /// The runtime composed an Agent Status for a fresh inbound turn.
    ///
    /// The carried view derives from the exact composed status used by the
    /// model path; a client never causes a second composition.
    AgentStatusComposed {
        /// The attempt that composed the status.
        attempt_id: AttemptId,
        /// The turn number of the request preparation.
        turn: u32,
        /// The canonical inbound message the status targets.
        target_message_id: MessageId,
        /// The structured status view (sections plus the canonical derived
        /// rendering).
        status: AgentStatusView,
    },

    /// An inbound message was enqueued into the conversation mailbox.
    ///
    /// Human and runtime-originated producers share the one inbound
    /// ordering domain; the event carries the authoritative mailbox
    /// sequence and the canonical message.
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
    BackgroundExecutionUpdated {
        /// The canonical registry snapshot after the transition.
        execution: RuntimeClientBackgroundExecution,
    },

    /// One subagent child transitioned in the authoritative conversation
    /// registry (Issue #60).
    ///
    /// Subagent children are conversation-owned: they survive attempt
    /// termination, client detach, and client reconnect.
    SubagentUpdated {
        /// The canonical registry snapshot after the transition.
        subagent: RuntimeClientSubagent,
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
    /// poll `snapshot_get` to discover an availability transition.
    CapabilityUpdated {
        /// The deterministic active capability projection.
        capabilities: CapabilityView,
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
        model: Box<SessionModelView>,
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
pub enum RuntimeClientOutcome {
    /// The attempt completed.
    Completed {
        /// The normalized finish reason.
        finish_reason: ModelFinishReason,
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
    },
}

/// The normalized client-visible failure of one attempt.
///
/// This is the external projection of the internal [`AttemptFailure`]
/// ([`crate::events::types::AttemptFailure`]): provider-specific fields
/// (such as the raw provider error code) never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeClientAttemptFailure {
    /// A model request exhausted its retry policy.
    Model {
        /// The normalized model error kind.
        kind: ModelErrorKind,
        /// The normalized human-readable message.
        message: String,
        /// The retry hint, when the provider reported one.
        retry_after_ms: Option<u64>,
    },
    /// A runtime failure.
    Runtime {
        /// The normalized runtime error.
        error: RuntimeError,
    },
}
