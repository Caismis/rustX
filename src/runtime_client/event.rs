//! The Runtime Client event vocabulary (Runtime Client Protocol v1).
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
//! breaking Runtime Client Protocol v1.
//!
//! Every attempt-scoped event carries its `attempt_id`, so events are
//! self-describing for clients that attach mid-attempt.

use serde::{Deserialize, Serialize};

use super::snapshot::{AgentStatusView, CapabilityView, RuntimeClientBackgroundExecution};
use crate::events::types::AttemptLimit;
use crate::message::types::{ContentBlockIndex, MessageBlock, UserMessageBlock};
use crate::model::error::ModelErrorKind;
use crate::model::finish::ModelFinishReason;
use crate::model::session::SessionModelView;
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolExecutionId, ToolId};
use crate::runtime::inbound::InboundSequence;
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
    AttemptStarted {
        /// The attempt identity.
        attempt_id: AttemptId,
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

    /// Assembly of a canonical agent message began.
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

    /// A tool call within the in-flight agent message started.
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
    /// A tool call within the in-flight agent message finished assembly.
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
    /// A tool execution settled with its normalized result (success,
    /// failure, cancellation, timeout, or validation rejection all settle
    /// through this one shape).
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

    /// The active capability set was activated (a revision swap).
    CapabilityPublished {
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

    /// The runtime accepted a shutdown request and no longer admits
    /// inbound work. Detach remains available; the current attempt
    /// continues to its settlement.
    RuntimeShutdown,
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
