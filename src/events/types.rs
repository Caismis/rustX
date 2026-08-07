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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::message::types::{AgentMessageBlock, ToolMessageBlock};
use crate::model::error::ModelError;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, ToolCallId, ToolId, TurnId,
};
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::types::{ToolCall, ToolExecutionResult};

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
/// even when the envelope also carries the enclosing attempt or turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// An attempt started executing.
    AttemptStarted {
        /// The attempt identity.
        attempt_id: AttemptId,
    },
    /// An attempt completed with a typed outcome.
    AttemptCompleted {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// The platform-level outcome.
        outcome: AttemptOutcome,
    },
    /// An attempt failed with a runtime error.
    AttemptFailed {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// The runtime error.
        error: RuntimeError,
    },
    /// An attempt was cancelled.
    AttemptCancelled {
        /// The attempt identity.
        attempt_id: AttemptId,
        /// Why the attempt was cancelled.
        reason: CancellationReason,
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
    /// A text delta of the in-flight agent message.
    AgentTextDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The incremental text.
        delta: String,
    },
    /// A reasoning delta of the in-flight agent message.
    AgentReasoningDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The incremental reasoning text.
        delta: String,
    },
    /// A tool call within the in-flight agent message started.
    ToolCallStarted {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The tool call being assembled.
        call: ToolCall,
    },
    /// An argument delta of an in-flight tool call.
    ToolCallArgumentsDelta {
        /// The message identity being assembled.
        message_id: MessageId,
        /// Identity of the tool call being assembled.
        call_id: ToolCallId,
        /// The incremental JSON argument fragment.
        arguments_delta: String,
    },
    /// A tool call within the in-flight agent message completed.
    ToolCallCompleted {
        /// The message identity being assembled.
        message_id: MessageId,
        /// The completed tool call.
        call: ToolCall,
    },
    /// A complete canonical agent message was committed to the history.
    AgentMessageCommitted {
        /// The committed message block.
        message: AgentMessageBlock,
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
        /// A progress notification.
        progress: String,
    },
    /// Tool execution finished and produced a normalized result.
    ToolExecutionCompleted {
        /// The normalized execution result.
        result: ToolExecutionResult,
    },
    /// Tool execution failed without producing a result.
    ToolExecutionFailed {
        /// Identity of the failed tool call.
        tool_call_id: ToolCallId,
        /// Human-readable failure message.
        error: String,
    },
    /// A complete canonical tool message was committed to the history.
    ToolMessageCommitted {
        /// The committed message block.
        message: ToolMessageBlock,
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

/// The platform-level outcome of an attempt.
///
/// Provider finish reasons, runtime cancellation, timeout, limit exhaustion,
/// and runtime failure are distinct and are never collapsed into one string.
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
    /// The attempt failed with a runtime error.
    Failed {
        /// The runtime error.
        error: RuntimeError,
    },
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
    use super::{AttemptLimit, AttemptOutcome, RuntimeEvent, RuntimeEventEnvelope};
    use crate::model::finish::ModelFinishReason;
    use crate::runtime::identity::{AttemptId, ConversationId, EventId};
    use crate::runtime::types::CancellationReason;
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
                error: crate::runtime::types::RuntimeError::Internal {
                    message: "boom".to_owned(),
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
            result: crate::tools::types::ToolExecutionResult {
                status: crate::tools::types::ToolExecutionStatus::Success,
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
}
