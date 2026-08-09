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
//! Streaming model deltas and tool progress are events, never message blocks.
//! Events are append-only and persist before external publication in
//! production (a later milestone). The envelope owns the durable identity and
//! ordering: an explicit schema version, a monotonic sequence, and a stable
//! event id, plus conversation/attempt/turn identity and a UTC timestamp.
//!
//! AG-UI is an output projection of these events and is never the internal
//! representation.
//!
//! ## Attempt settlement
//!
//! Exactly one terminal event settles an attempt. The terminal events are
//! [`RuntimeEvent::AttemptCompleted`], [`RuntimeEvent::AttemptCancelled`],
//! [`RuntimeEvent::AttemptTimedOut`],
//! [`RuntimeEvent::AttemptLimitExceeded`], and
//! [`RuntimeEvent::AttemptFailed`], and they map one-to-one to
//! [`AttemptOutcome`] variants. A terminal event carries only the data valid
//! for that state: in particular `AttemptCompleted` carries a finish reason
//! and no outcome payload, so a failed/cancelled/timed-out attempt can never
//! be encoded as a completion. Unknown payload fields are rejected.
//!
//! ## Committed messages
//!
//! [`RuntimeEvent::AgentMessageCommitted`] and
//! [`RuntimeEvent::ToolMessageCommitted`] reference the committed message by
//! its stable [`MessageId`] and never embed the message content. Canonical
//! message content lives only in the durable Message Ledger (M8); the Event
//! Journal records the execution fact. This keeps exactly one authoritative
//! copy of message content.
//!
//! A committed-message event must not be emitted before the corresponding
//! `MessageBlock` has been durably committed to the Message Ledger. Message
//! Ledger persistence and Event Journal persistence are separate durable
//! operations unless a backend provides a shared atomic transaction; M8 owns
//! the atomicity or crash-reconciliation boundary between these stores. If a
//! crash occurs after the `MessageBlock` is durably committed but before the
//! corresponding committed-message event is appended, recovery must
//! recognize and reconcile that state rather than treating the message as
//! absent or duplicating its content.
//!
//! Persist-before-publish applies to `RuntimeEvent` publication only:
//! append the event durably before publishing it externally. It does not by
//! itself provide a transaction with the Message Ledger.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::message::types::ContentBlockIndex;
use crate::model::error::ModelError;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, ToolCallId, ToolExecutionId, ToolId, TurnId,
};
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::types::{ToolCall, ToolCallStart, ToolExecutionResult, ToolProgress};

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
    /// by the future event writer before publication.
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
        /// The model identifier requested.
        model: String,
    },
    /// A model request completed successfully.
    ModelRequestCompleted {
        /// Why the generation finished.
        finish_reason: ModelFinishReason,
        /// Final usage, when reported.
        usage: Option<ModelUsage>,
    },
    /// A model request failed with a normalized error.
    ModelRequestFailed {
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

    /// Assembly of a canonical agent message started.
    AgentMessageStarted {
        /// The message identity being assembled.
        message_id: MessageId,
    },
    /// A text delta of one output block of the in-flight agent message.
    AgentTextDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The output block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental text.
        delta: String,
    },
    /// A reasoning delta of one output block of the in-flight agent message.
    AgentReasoningDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The reasoning block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental reasoning text.
        delta: String,
    },
    /// A refusal delta of one output block of the in-flight agent message.
    ///
    /// Refusal is preserved as refusal, never flattened into text, so the
    /// completed message assembles a `RefusalBlock`.
    AgentRefusalDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The refusal block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental refusal text.
        delta: String,
    },
    /// A tool call within the in-flight agent message started.
    ToolCallStarted {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// The tool call identity, without streamed arguments yet.
        call: ToolCallStart,
    },
    /// An argument delta of an in-flight tool call.
    ToolCallArgumentsDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// Identity of the tool call being assembled.
        call_id: ToolCallId,
        /// The incremental JSON argument fragment.
        arguments_delta: String,
    },
    /// A tool call within the in-flight agent message completed.
    ToolCallCompleted {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The tool-call content block that completed.
        block_index: ContentBlockIndex,
        /// The fully assembled tool call.
        call: ToolCall,
    },
    /// A complete canonical agent message was committed to the Message
    /// Ledger. The event references the message by identity only; message
    /// content is never duplicated into the Event Journal.
    AgentMessageCommitted {
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

    /// Context compaction started (compaction itself is a later milestone).
    CompactionStarted,
    /// Context compaction completed.
    CompactionCompleted,
    /// Context compaction failed.
    CompactionFailed {
        /// Human-readable failure message.
        error: String,
    },
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
/// The relationship to terminal runtime events is one-to-one: each terminal
/// [`RuntimeEvent`] maps to exactly one [`AttemptOutcome`] variant via
/// [`AttemptOutcome::from_terminal_event`], and no non-terminal event maps
/// to an outcome. The Agent Loop (M3) consumes this platform-level
/// projection.
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
    use crate::runtime::types::CancellationReason;
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
            RuntimeEvent::AgentMessageCommitted {
                message_id: crate::runtime::identity::MessageId::new("msg-1"),
            },
            RuntimeEvent::CompactionCompleted,
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
