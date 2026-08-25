//! The canonical runtime event protocol.
//!
//! A [`RuntimeEvent`] is an execution/process fact, never a model-context
//! fact. The absolute invariant is:
//!
//! ```text
//! RuntimeEvent = execution/process fact
//! MessageBlock = model-context fact
//! ```
//!
//! Tool progress is an event, never a message block.
//! Events are append-only and a successful durable append commits before
//! external publication through the durable `ConversationStore`. If a
//! required append fails, the event is not published or fabricated in a
//! local projection. The envelope owns the durable identity and ordering: an
//! explicit schema version, a monotonic sequence, and a stable event id, plus
//! conversation/attempt/turn identity and a UTC timestamp.
//!
//! AG-UI is an output projection of these events and is never the internal
//! representation.
//!
//! ## Attempt settlement
//!
//! A normally settled attempt has exactly one committed terminal event. The
//! terminal events are
//! [`RuntimeEvent::AttemptCompleted`], [`RuntimeEvent::AttemptCancelled`],
//! [`RuntimeEvent::AttemptTimedOut`],
//! [`RuntimeEvent::AttemptLimitExceeded`], and
//! [`RuntimeEvent::AttemptFailed`], and they map one-to-one to
//! [`AttemptOutcome`] variants. A terminal event carries only the data valid
//! for that state: in particular `AttemptCompleted` carries a finish reason
//! and no outcome payload, so a failed/cancelled/timed-out attempt can never
//! be encoded as a completion. Unknown payload fields are rejected. The
//! Agent Loop keeps its execution settlement candidate separately; if the
//! final terminal append fails, no terminal event exists and the result
//! reports the typed durable failure instead of deriving an outcome from a
//! fabricated event.
//!
//! ## Committed messages
//!
//! [`RuntimeEvent::AssistantMessageCommitted`] and
//! [`RuntimeEvent::ToolMessageCommitted`] reference the committed message by
//! its stable [`MessageId`] and never embed the message content. Canonical
//! message content lives only in the durable Message Ledger; the Event
//! Journal records the execution fact. This keeps exactly one authoritative
//! copy of message content.
//!
//! Committed-message events share the `ConversationStore` transaction with the
//! Ledger body they reference. Compaction and request-start facts use the
//! same reference-ordering rule. Persist-before-publish appends the committed
//! envelope before observers or external projections see it.
//!
//! ## What the Event Journal deliberately does not carry
//!
//! High-frequency Assistant streaming content is **not** an Event Journal
//! fact (Issue #108). Assistant text, reasoning, refusal, and tool-call
//! argument increments belong to the durable publication plane
//! ([`crate::publication`]), which owns its own bounded coalescing policy,
//! its own staging rows, and its own terminal marker. The Journal keeps the
//! low-frequency recovery-significant semantic facts only, so its size is
//! O(execution facts) rather than O(provider deltas).
//!
//! The Journal therefore owns exactly two of the three Issue #108
//! linearization points — P ([`RuntimeEvent::ModelRequestCompleted`]) and C
//! ([`RuntimeEvent::AssistantMessageCommitted`]) — while U lives in the
//! publication plane. The required ordering is `P < U < C`.
//!
//! ## Human interaction audit
//!
//! [`RuntimeEvent::InteractionRequested`] and
//! [`RuntimeEvent::InteractionSettled`] (Issue #109) are the low-frequency
//! semantic facts of the Question/Approval plane. They are **audit evidence
//! only**: the pending waiter that they describe is process-owned workflow
//! state and is never reconstructed from them. In particular a historical
//! `InteractionSettled(Approved)` never authorizes a tool execution after a
//! restart. Keypresses, focus changes, editing state, and TUI presentation
//! details are not interaction facts and never enter the Journal.
//!
//! The audit vocabulary itself — [`InteractionSubject`],
//! [`InteractionSettlement`], the argument digest, and the bounded-payload
//! contract every backend enforces — lives in
//! [`crate::events::interaction`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::events::interaction::{InteractionSettlement, InteractionSubject};
use crate::model::error::ModelError;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, InteractionId, MessageId, RequestId, SubagentId,
    ToolCallId, ToolExecutionId, ToolId, TurnId,
};
use crate::runtime::types::{CancellationReason, RuntimeError, TokenMeasurement};
use crate::tools::types::{ToolExecutionResult, ToolProgress};

/// The current schema version of [`RuntimeEventEnvelope`].
pub const EVENT_SCHEMA_VERSION: u16 = 1;

/// The envelope around every durable runtime event.
#[allow(clippy::struct_field_names)] // `event_id` and `event` are protocol-specified field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEventEnvelope {
    /// Explicit event schema version; never inferred from the crate version.
    pub schema_version: u16,
    /// Stable identity of this event.
    pub event_id: EventId,
    /// Monotonic sequence within the conversation. Allocation is committed
    /// by the native durable `ConversationStore` before publication.
    pub sequence: u64,
    /// The conversation this event belongs to.
    pub conversation_id: ConversationId,
    /// The attempt this event belongs to, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    /// The turn this event belongs to, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// UTC timestamp of the event.
    pub timestamp: DateTime<Utc>,
    /// The typed event payload.
    pub event: RuntimeEvent,
}

/// An execution/process fact produced by the runtime.
///
/// Event payloads are self-describing: they carry the identities they need
/// even when the envelope also carries the enclosing attempt or turn. Unknown
/// payload fields are rejected so stale or contradictory encodings cannot
/// silently deserialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeEvent {
    /// An attempt started executing.
    AttemptStarted {
        /// The attempt identity.
        attempt_id: AttemptId,
    },
    /// The attempt settled by completing; the finish reason explains the
    /// stop. This terminal event never carries a failure outcome.
    AttemptCompleted {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// The normalized provider finish reason.
        finish_reason: ModelFinishReason,
    },
    /// The attempt settled by cancellation.
    AttemptCancelled {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// Why the attempt was cancelled.
        reason: CancellationReason,
    },
    /// The attempt settled because it exceeded its runtime time budget.
    AttemptTimedOut {
        /// The attempt identity.
        attempt_id: AttemptId,
    },
    /// The attempt settled because it exceeded one of its execution limits.
    AttemptLimitExceeded {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// Which limit was exceeded.
        limit: AttemptLimit,
    },
    /// The attempt settled by failure.
    AttemptFailed {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// The normalized failure, preserving a `ModelError` when a model
        /// request exhausted its retry policy.
        error: AttemptFailure,
    },

    /// A turn started.
    TurnStarted,
    /// A turn completed.
    TurnCompleted,

    /// A model request was sent to an adapter.
    ModelRequestStarted {
        /// The exact immutable Request Snapshot this start fact commits.
        request_id: RequestId,
        /// The model selected by the frozen invocation. This is a projection
        /// convenience; the Request Snapshot remains the authority.
        model: String,
    },
    /// A model request completed successfully.
    ///
    /// This is the **P** linearization point of Issue #108: the durable fact
    /// that one exact provider request produced an outcome. It is deliberately
    /// never combined with the publication outcome (U) or with canonical
    /// Assistant acceptance (C); each is a different fact and a crash between
    /// them must stay distinguishable.
    ModelRequestCompleted {
        /// The exact provider request whose outcome this fact records.
        request_id: RequestId,
        /// Why the generation finished.
        finish_reason: ModelFinishReason,
        /// Final usage, when reported.
        usage: Option<ModelUsage>,
    },
    /// A model request failed with a normalized error.
    ModelRequestFailed {
        /// The exact provider request that failed.
        request_id: RequestId,
        /// The normalized model error.
        error: ModelError,
    },
    /// A model request was scheduled for retry.
    ModelRetryScheduled {
        /// Ordinal retry attempt number, starting at 1.
        attempt_number: u32,
        /// Delay before the retry, in milliseconds.
        retry_delay_ms: Option<u64>,
    },

    /// One inbound batch crossed the canonical-adoption linearization point
    /// and became a turn this conversation owes a model answer for.
    ///
    /// This is the durable **answer obligation** of an adopted turn, and the
    /// only durable fact that says "rustX accepted this work". It is committed
    /// inside the adoption transaction itself, so a canonical `UserMessage`
    /// and the obligation to answer it can never disagree.
    ///
    /// The obligation is *consumed* — never re-derived from canonical shape —
    /// by exactly two later facts, whichever commits first:
    ///
    /// ```text
    /// ModelRequestStarted   the turn was carried to the provider; from here
    ///                       the external-outcome plane owns it
    /// attempt terminal      the runtime concluded the turn (completed,
    ///                       cancelled, failed, timed out, limited)
    /// ```
    ///
    /// Recovery therefore continues exactly the turns a live runtime would
    /// still owe an answer for, and supplied bootstrap history — a fork seed,
    /// a persona lineage, a fixture prefix — never acquires an obligation,
    /// because it was never adopted.
    InboundTurnAdopted {
        /// The adopted canonical messages, in inbound sequence order.
        message_ids: Vec<MessageId>,
    },

    /// A complete canonical Assistant message was committed to the Message
    /// Ledger. The event references the message by identity only; message
    /// content is never duplicated into the Event Journal.
    ///
    /// This is the **C** linearization point of Issue #108. For a published
    /// stream the durable store rejects C without U.
    AssistantMessageCommitted {
        /// Identity of the committed message block.
        message_id: MessageId,
    },

    /// Tool execution started.
    ToolExecutionStarted {
        /// Identity of the tool call being executed.
        tool_call_id: ToolCallId,
        /// Identity of the executed tool.
        tool_id: ToolId,
    },
    /// Progress of an in-flight tool execution.
    ToolExecutionProgress {
        /// Identity of the executing tool call.
        tool_call_id: ToolCallId,
        /// Identity of the executed tool.
        tool_id: ToolId,
        /// The detached runtime execution instance for background work,
        /// `None` for foreground executions. No fake execution id is
        /// invented for foreground calls.
        execution_id: Option<ToolExecutionId>,
        /// The bounded structured progress notification.
        progress: ToolProgress,
    },
    /// Tool execution finished and produced a normalized result.
    ToolExecutionCompleted {
        /// Identity of the tool call that finished.
        tool_call_id: ToolCallId,
        /// Identity of the executed tool.
        tool_id: ToolId,
        /// The normalized execution result.
        result: ToolExecutionResult,
    },
    /// Tool execution failed without producing a result.
    ToolExecutionFailed {
        /// Identity of the failed tool call.
        tool_call_id: ToolCallId,
        /// Identity of the executed tool.
        tool_id: ToolId,
        /// Human-readable failure message.
        error: String,
    },
    /// A complete canonical tool message was committed to the Message
    /// Ledger. The event references the message by identity only.
    ToolMessageCommitted {
        /// Identity of the committed message block.
        message_id: MessageId,
        /// Identity of the tool call the committed message answers.
        tool_call_id: ToolCallId,
    },

    /// Context compaction started.
    CompactionStarted,
    /// Context compaction completed: the canonical runtime compaction
    /// summary is committed to the Message Ledger and the new Conversation
    /// Surface revision is established.
    ///
    /// The event is emitted strictly **after** that semantic commit, so it
    /// can never imply success before the state exists.
    CompactionCompleted {
        /// The compaction generation, derived from Conversation Surface
        /// history.
        generation: u64,
        /// The identity of the committed canonical summary message. The
        /// event references it by identity only; summary content lives in
        /// the Message Ledger.
        summary_message_id: MessageId,
        /// The Conversation Surface revision established by the rewrite.
        surface_revision: SurfaceRevision,
        /// The pre-compaction measurement, preserving its provenance.
        tokens_before: TokenMeasurement,
        /// The deterministic estimate of the rebuilt request context.
        estimated_tokens_after: u64,
    },
    /// Context compaction failed.
    CompactionFailed {
        /// Human-readable failure message.
        error: String,
    },
    /// Conversation ownership of one detached background execution
    /// committed durably (Issue #12, M9a).
    ///
    /// This is the **background-start** fact: it commits strictly before the
    /// runner's start gate is released, so no detached external side effect
    /// can begin without durable evidence that this `ToolExecutionId`
    /// existed, which `ToolCall`/tool it belonged to, and that ownership was
    /// committed. Without it, a process restart could not distinguish "a
    /// background execution was owned and never settled" from "no background
    /// execution ever existed", and the deterministic `exec_N` allocator
    /// could reuse an identity that already entered durable authority.
    ///
    /// The fact opens the `background:{execution_id}` durable lifecycle;
    /// [`RuntimeEvent::BackgroundTerminalPublished`] closes it exactly once.
    BackgroundExecutionCommitted {
        /// The detached execution identity allocated by the registry.
        execution_id: ToolExecutionId,
        /// The model-issued tool call the execution belongs to.
        tool_call_id: ToolCallId,
        /// Identity of the executed tool.
        tool_id: ToolId,
        /// The model-facing tool name at dispatch time. Retained so a
        /// recovery-generated terminal notification can name the tool
        /// without consulting the current capability set.
        tool_name: String,
    },
    /// A detached background execution's terminal inbound notification was
    /// durably accepted. The event is committed in the same transaction as
    /// the Pending Inbound row and references that row by `MessageId`; it never
    /// embeds the notification body.
    BackgroundTerminalPublished {
        /// The detached execution identity.
        execution_id: ToolExecutionId,
        /// The pending/canonical `MessageId` of the notification.
        message_id: MessageId,
        /// The terminal state represented by the notification.
        state: BackgroundTerminalState,
    },
    /// Conversation ownership of one asynchronous one-shot subagent child
    /// committed durably (Issue #60).
    ///
    /// This is the **subagent-start** fact: it commits strictly before the
    /// child process receives its delegation, so no child model/tool side
    /// effect can begin without durable evidence that this `SubagentId`
    /// existed, which `ToolCall` delegated it, which child identities it
    /// owns, and that ownership committed. Without it, a process restart
    /// could not distinguish "a child was owned and never settled" from
    /// "no child ever existed", and the conversation-scoped ordinal
    /// allocator could reuse an identity that already entered durable
    /// authority.
    ///
    /// The fact opens the `subagent:{subagent_id}` durable lifecycle;
    /// [`RuntimeEvent::SubagentTerminalPublished`] closes it exactly once.
    /// The fact carries no attempt identity: a committed child deliberately
    /// outlives the attempt that started it.
    SubagentOwnershipCommitted {
        /// The allocated subagent identity.
        subagent_id: SubagentId,
        /// The child agent identity (the provenance of the child's later
        /// model-visible answer).
        child_agent_id: AgentId,
        /// The child's own durable conversation identity.
        child_conversation_id: ConversationId,
        /// The model-issued tool call that delegated the work.
        tool_call_id: ToolCallId,
        /// The child profile identity frozen at start.
        profile: String,
    },
    /// A subagent child's terminal publication was durably accepted. The
    /// event is committed in the same transaction as the Pending Inbound
    /// row and references that row by `MessageId`; it never embeds the
    /// publication body. A successful terminal references the
    /// `UserSource::Agent(child)` result message; every other terminal
    /// references the `UserSource::Runtime` notice.
    SubagentTerminalPublished {
        /// The subagent identity.
        subagent_id: SubagentId,
        /// The child agent identity, restated for a self-describing event.
        /// Durable validation compares it with the exact
        /// `SubagentOwnershipCommitted` fact for `subagent_id`; this repeated
        /// field is not authority by itself.
        child_agent_id: AgentId,
        /// The pending/canonical `MessageId` of the terminal publication.
        message_id: MessageId,
        /// The terminal state represented by the publication.
        state: SubagentTerminalState,
    },

    /// One human interaction was requested (Issue #109).
    ///
    /// This is the **requested** half of the durable interaction audit. It
    /// commits strictly before rustX releases the prompt to a user-facing
    /// client, so no user can be shown a Question or Approval without durable
    /// evidence that the interaction existed, which attempt/turn owned it, and
    /// exactly what was asked.
    ///
    /// The fact opens the `interaction:{interaction_id}` durable lifecycle;
    /// [`RuntimeEvent::InteractionSettled`] closes it exactly once. An
    /// interaction that stays open across a process death is durable evidence
    /// of an unanswered prompt — never an instruction to recreate the waiter.
    InteractionRequested {
        /// The runtime-owned, non-reused interaction identity.
        interaction_id: InteractionId,
        /// The bounded by-value audit subject.
        subject: InteractionSubject,
    },
    /// One human interaction reached its single terminal settlement
    /// (Issue #109).
    ///
    /// For Approval this commits strictly before execution authority
    /// proceeds, so the durable order is always
    ///
    /// ```text
    /// InteractionSettled(Approved) -> ToolExecutionStarted -> external side effect
    /// ```
    ///
    /// The settled fact is audit evidence and nothing more. A historical
    /// `Approved` never grants execution authority to a later process: after a
    /// restart the current runtime must reach a new live approval under
    /// current semantics.
    InteractionSettled {
        /// The interaction whose one terminal transition this fact records.
        interaction_id: InteractionId,
        /// The bounded terminal settlement.
        settlement: InteractionSettlement,
    },
}

/// The durable terminal outcome of an asynchronous one-shot subagent child
/// (Issue #60).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentTerminalState {
    /// The child completed and its bounded answer was durably accepted by
    /// the parent conversation with `UserSource::Agent(child)` provenance.
    Succeeded,
    /// The child failed or its process/protocol settlement failed.
    Failed,
    /// Cancellation intent won settlement (explicit cancel or runtime
    /// drain).
    Cancelled,
    /// The owning process restarted while the child was non-terminal: the
    /// child process did not survive the parent and its actual outcome is
    /// **unknown**. Recovery never converts this into a known failure and
    /// never reattaches to or replays the old child.
    Interrupted,
}

/// The durable terminal outcome of a detached background execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTerminalState {
    /// The executor completed successfully.
    Succeeded,
    /// The executor failed.
    Failed,
    /// Cancellation intent won settlement.
    Cancelled,
    /// The owning process restarted while the execution was non-terminal:
    /// the detached task/process did not survive the restart and its actual
    /// external outcome is **unknown** (Issue #12, M9a).
    ///
    /// This is deliberately distinct from [`BackgroundTerminalState::Failed`],
    /// exactly as [`ToolExecutionStatus::Interrupted`] is distinct from
    /// `Failed`: recovery never converts an unknown outcome into a known
    /// failure, and it never re-launches the execution.
    ///
    /// [`ToolExecutionStatus::Interrupted`]: crate::tools::types::ToolExecutionStatus::Interrupted
    Interrupted,
}

/// The normalized failure of an attempt.
///
/// An attempt that fails because a model request exhausted its retry policy
/// preserves the normalized [`ModelError`]; other failures are runtime
/// failures. This keeps provider failure information intact without creating
/// a runtime-to-model dependency: the model layer remains below this event
/// layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttemptFailure {
    /// A model request exhausted its retry policy.
    Model {
        /// The normalized model error, preserved for diagnostics and
        /// retry/termination reasoning.
        error: ModelError,
    },
    /// A runtime failure.
    Runtime {
        /// The runtime error.
        error: RuntimeError,
    },
}

/// The platform-level outcome of an attempt.
///
/// Provider finish reasons, runtime cancellation, timeout, limit exhaustion,
/// and runtime failure are distinct and are never collapsed into one string.
/// When a terminal runtime event is durably committed, it maps one-to-one to
/// an [`AttemptOutcome`] variant via
/// [`AttemptOutcome::from_terminal_event`], and no non-terminal event maps to
/// an outcome. The Agent Loop (M3) also reports an execution settlement
/// candidate separately when the required terminal append fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// The attempt completed; the provider finish reason explains the stop.
    Completed {
        /// The normalized provider finish reason.
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
        /// The normalized failure.
        error: AttemptFailure,
    },
}

impl AttemptOutcome {
    /// Maps a terminal attempt event to its one-to-one platform outcome.
    ///
    /// The mapping is total on the five terminal events and returns `None`
    /// for every non-terminal event, freezing the invariant that exactly one
    /// terminal event settles an attempt with exactly one outcome.
    #[must_use]
    pub fn from_terminal_event(event: &RuntimeEvent) -> Option<AttemptOutcome> {
        match event {
            RuntimeEvent::AttemptCompleted { finish_reason, .. } => {
                Some(AttemptOutcome::Completed {
                    finish_reason: finish_reason.clone(),
                })
            }
            RuntimeEvent::AttemptCancelled { reason, .. } => {
                Some(AttemptOutcome::Cancelled { reason: *reason })
            }
            RuntimeEvent::AttemptTimedOut { .. } => Some(AttemptOutcome::TimedOut),
            RuntimeEvent::AttemptLimitExceeded { limit, .. } => {
                Some(AttemptOutcome::LimitExceeded { limit: *limit })
            }
            RuntimeEvent::AttemptFailed { error, .. } => Some(AttemptOutcome::Failed {
                error: error.clone(),
            }),
            _ => None,
        }
    }
}

/// Which attempt execution limit was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptLimit {
    /// The maximum turn count was exceeded.
    MaxTurns,
    /// The maximum tool-call count was exceeded.
    MaxToolCalls,
    /// The maximum runtime duration was exceeded.
    MaxRuntimeSeconds,
}

#[cfg(test)]
mod tests {
    use super::{AttemptFailure, AttemptLimit, AttemptOutcome, RuntimeEvent, RuntimeEventEnvelope};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::finish::ModelFinishReason;
    use crate::runtime::identity::{AttemptId, ConversationId, EventId, ToolCallId, ToolId};
    use crate::runtime::types::{CancellationReason, TokenMeasurement, TokenMeasurementSource};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};
    use chrono::{DateTime, TimeZone, Utc};

    fn example_envelope() -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new("evt-1"),
            sequence: 1,
            conversation_id: ConversationId::new("conv-1"),
            attempt_id: Some(AttemptId::new("attempt-1")),
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt-1"),
            },
        }
    }

    /// The envelope round-trips with deterministic serialization.
    #[test]
    fn envelope_round_trip() {
        let envelope = example_envelope();
        let first = serde_json::to_string(&envelope).expect("serialize envelope");
        let second = serde_json::to_string(&envelope).expect("serialize envelope again");
        assert_eq!(first, second, "serialization must be deterministic");
        let decoded: RuntimeEventEnvelope =
            serde_json::from_str(&first).expect("deserialize envelope");
        assert_eq!(decoded, envelope);
    }

    /// The envelope carries explicit schema version and sequence.
    #[test]
    fn envelope_carries_schema_version_and_sequence() {
        let value = serde_json::to_value(example_envelope()).expect("serialize envelope");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["sequence"], 1);
        assert_eq!(value["event"]["type"], "attempt_started");
    }

    /// The timestamp uses a stable UTC representation.
    #[test]
    fn envelope_timestamp_is_utc_rfc3339() {
        let envelope = example_envelope();
        let value = serde_json::to_value(&envelope).expect("serialize envelope");
        let text = value["timestamp"].as_str().expect("timestamp string");
        assert_eq!(text, "2026-08-07T12:00:00Z");
        let parsed: DateTime<Utc> =
            serde_json::from_value(value["timestamp"].clone()).expect("parse timestamp");
        assert_eq!(parsed, envelope.timestamp);
    }

    /// Attempt outcomes are typed and never collapsed into strings.
    #[test]
    fn attempt_outcome_round_trip() {
        let outcomes = [
            AttemptOutcome::Completed {
                finish_reason: ModelFinishReason::Stop,
            },
            AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            },
            AttemptOutcome::TimedOut,
            AttemptOutcome::LimitExceeded {
                limit: AttemptLimit::MaxTurns,
            },
            AttemptOutcome::Failed {
                error: AttemptFailure::Model {
                    error: ModelError {
                        kind: ModelErrorKind::RateLimit,
                        message: "retries exhausted".to_owned(),
                        retry_after_ms: None,
                        provider_code: None,
                    },
                },
            },
        ];
        for outcome in outcomes {
            let json = serde_json::to_string(&outcome).expect("serialize outcome");
            let decoded: AttemptOutcome = serde_json::from_str(&json).expect("deserialize outcome");
            assert_eq!(decoded, outcome);
        }
    }

    /// Event discriminators are stable protocol strings.
    #[test]
    fn event_discriminators_are_stable() {
        let event = RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call_01"),
            tool_id: ToolId::new("tool-list"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 5,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        };
        let value = serde_json::to_value(event).expect("serialize event");
        assert_eq!(value["type"], "tool_execution_completed");
    }

    /// Exactly one terminal event settles an attempt, and each terminal
    /// event maps one-to-one to an `AttemptOutcome`.
    #[test]
    fn terminal_events_map_one_to_one_to_outcomes() {
        let attempt_id = AttemptId::new("attempt-1");
        let pairs = [
            (
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt_id.clone(),
                    finish_reason: ModelFinishReason::Stop,
                },
                AttemptOutcome::Completed {
                    finish_reason: ModelFinishReason::Stop,
                },
            ),
            (
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt_id.clone(),
                    reason: CancellationReason::UserRequested,
                },
                AttemptOutcome::Cancelled {
                    reason: CancellationReason::UserRequested,
                },
            ),
            (
                RuntimeEvent::AttemptTimedOut {
                    attempt_id: attempt_id.clone(),
                },
                AttemptOutcome::TimedOut,
            ),
            (
                RuntimeEvent::AttemptLimitExceeded {
                    attempt_id: attempt_id.clone(),
                    limit: AttemptLimit::MaxToolCalls,
                },
                AttemptOutcome::LimitExceeded {
                    limit: AttemptLimit::MaxToolCalls,
                },
            ),
            (
                RuntimeEvent::AttemptFailed {
                    attempt_id: attempt_id.clone(),
                    error: AttemptFailure::Runtime {
                        error: crate::runtime::types::RuntimeError::Internal {
                            message: "boom".to_owned(),
                        },
                    },
                },
                AttemptOutcome::Failed {
                    error: AttemptFailure::Runtime {
                        error: crate::runtime::types::RuntimeError::Internal {
                            message: "boom".to_owned(),
                        },
                    },
                },
            ),
        ];
        for (event, expected) in pairs {
            assert_eq!(
                AttemptOutcome::from_terminal_event(&event),
                Some(expected),
                "terminal event must map to its outcome"
            );
        }
    }

    /// Non-terminal events never map to an outcome.
    #[test]
    fn non_terminal_events_map_to_no_outcome() {
        let non_terminal = [
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("attempt-1"),
            },
            RuntimeEvent::TurnStarted,
            RuntimeEvent::AssistantMessageCommitted {
                message_id: crate::runtime::identity::MessageId::new("msg-1"),
            },
            RuntimeEvent::CompactionCompleted {
                generation: 1,
                summary_message_id: crate::runtime::identity::MessageId::new("conv-summary-1"),
                surface_revision: crate::conversation::SurfaceRevision::new(4),
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 50,
            },
        ];
        for event in non_terminal {
            assert_eq!(
                AttemptOutcome::from_terminal_event(&event),
                None,
                "non-terminal event must not map to an outcome"
            );
        }
    }

    /// A contradictory terminal encoding is impossible by construction: a
    /// completed attempt carries only a finish reason (no outcome payload),
    /// and the old outcome-bearing encoding fails to deserialize.
    #[test]
    fn contradictory_terminal_encodings_are_impossible() {
        let event = RuntimeEvent::AttemptCompleted {
            attempt_id: AttemptId::new("attempt-1"),
            finish_reason: ModelFinishReason::Stop,
        };
        let value = serde_json::to_value(&event).expect("serialize event");
        assert_eq!(value["type"], "attempt_completed");
        assert_eq!(value["finish_reason"], serde_json::json!({"type": "stop"}));
        assert!(
            value.get("outcome").is_none(),
            "AttemptCompleted must not carry an outcome payload"
        );

        let contradictory = r#"{
            "type": "attempt_completed",
            "attempt_id": "attempt-1",
            "outcome": {"type": "failed", "error": {"type": "internal", "message": "boom"}}
        }"#;
        assert!(
            serde_json::from_str::<RuntimeEvent>(contradictory).is_err(),
            "outcome-bearing completion must be rejected"
        );
    }

    /// An attempt failing from exhausted retries preserves the normalized
    /// model error without degrading it to a runtime error string.
    #[test]
    fn attempt_failure_preserves_model_error() {
        let error = ModelError {
            kind: ModelErrorKind::RateLimit,
            message: "retries exhausted".to_owned(),
            retry_after_ms: Some(5_000),
            provider_code: Some("rate_limit_exceeded".to_owned()),
        };
        let event = RuntimeEvent::AttemptFailed {
            attempt_id: AttemptId::new("attempt-1"),
            error: AttemptFailure::Model {
                error: error.clone(),
            },
        };
        let json = serde_json::to_string(&event).expect("serialize event");
        let decoded: RuntimeEvent = serde_json::from_str(&json).expect("deserialize event");
        assert_eq!(decoded, event);
        assert!(matches!(
            AttemptOutcome::from_terminal_event(&decoded),
            Some(AttemptOutcome::Failed {
                error: AttemptFailure::Model { error: ref model_error }
            }) if model_error == &error
        ));
    }

    /// With two parallel tool calls completing in reversed order, every
    /// completion event remains attributable to its originating call.
    #[test]
    fn parallel_tool_completions_remain_attributable() {
        let call_a = ToolCallId::new("call_a");
        let call_b = ToolCallId::new("call_b");
        let tool_a = ToolId::new("tool-alpha");
        let tool_b = ToolId::new("tool-beta");

        let events = [
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_a.clone(),
                tool_id: tool_a.clone(),
            },
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_b.clone(),
                tool_id: tool_b.clone(),
            },
            // B completes before A: completion order is reversed.
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call_b.clone(),
                tool_id: tool_b.clone(),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 20,
                    exit_code: Some(0),
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            },
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call_a.clone(),
                tool_id: tool_a.clone(),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 40,
                    exit_code: Some(0),
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            },
        ];
        let mut completed: Vec<(ToolCallId, u64)> = Vec::new();
        for event in &events {
            if let RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                result,
            } = event
            {
                let expected_tool = if tool_call_id == &call_a {
                    &tool_a
                } else {
                    &tool_b
                };
                assert_eq!(tool_id, expected_tool, "tool identity must match the call");
                completed.push((tool_call_id.clone(), result.duration_ms));
            }
        }
        assert_eq!(
            completed,
            vec![(call_b, 20), (call_a, 40)],
            "completions remain attributable despite reversed order"
        );
    }
}
