//! Finite display identities and previews over original native turn owners.
use crate::conversation::SurfaceRevision;
use crate::durable::TranscriptCursor;
use crate::durable::reading::ConversationReadCut;
use crate::durable::reading::TurnProcessOutcome;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{AttemptId, ConversationId, MessageId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedTurnIdentity {
    pub conversation_id: ConversationId,
    pub attempt_id: Option<AttemptId>,
    pub attempt_id_source: Option<AttemptIdentitySource>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedTurn {
    pub id: ObservedTurnIdentity,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub ordinal: u64,
    pub cursor: Option<TranscriptCursor>,
    #[schemars(length(max = 50))]
    pub prompt: String,
    #[schemars(length(max = 120))]
    pub response: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedTurnPage {
    pub cut: ConversationReadCut,
    pub total: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    #[schemars(length(max = 64))]
    pub turns: Vec<ObservedTurn>,
}

/// Derived response view over local execution or inherited lineage provenance.
/// Canonical content remains in the Message Ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedResponseView {
    pub closing_message_id: Option<MessageId>,
    pub closing_locator: super::content::CanonicalContentLocator,
    pub retry_locator: Option<super::content::CanonicalContentLocator>,
    pub deferred_fields: Vec<Vec<String>>,
    /// Original execution owner, including for inherited historical responses.
    pub origin: ObservedResponseOrigin,
    /// Durable completion timestamp; never a browser receipt time.
    pub completed_at: chrono::DateTime<chrono::Utc>,
    /// Immutable first Surface revision containing the closing response.
    pub surface_revision: SurfaceRevision,
    /// Exact adopted ordinary input, when this Attempt has a replayable input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_message_id: Option<MessageId>,
    /// All actual requests in the Attempt, only when every usage is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<crate::durable::response::CompletedResponseTiming>,
    /// Distinct provider-facing models of the Attempt's actual requests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}
/// Immutable original field selection; caller source cut supplies read authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttemptIdentitySource {
    Journal {
        field: super::tool::JournalFieldRef,
        domain: String,
    },
    Inherited {
        kind: InheritedAttemptIdentityKind,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        ordinal: u64,
        domain: String,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        encoded_bytes: u64,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InheritedAttemptIdentityKind {
    Turn,
    Response,
}
/// Finite execution provenance. A deferred original identity remains available
/// through the source-owned response metadata domain at its immutable cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedResponseOrigin {
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub attempt_id: Option<AttemptId>,
    pub attempt_id_source: Option<AttemptIdentitySource>,
    pub closing_message_id: Option<MessageId>,
}
/// One native process owner shared by live, successful and unsuccessful Turns.
/// Repeated on each exact member so bounded pages are independently resolvable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnProcessView {
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub attempt_id: Option<AttemptId>,
    pub attempt_id_source: Option<AttemptIdentitySource>,
    /// Original terminal Journal identity, absent while running or inherited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<crate::runtime::identity::EventId>,
    /// Control sorts immediately before this immutable native transcript position.
    /// No-content terminal Attempts use their terminal event position.
    pub control_cursor: super::transcript::RuntimeClientTranscriptCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_message_id: Option<MessageId>,
    /// Exact final canonical identity, present only for a proven completed response.
    pub final_message_locator: Option<super::content::CanonicalContentLocator>,
    pub final_message_id_deferred: bool,
    /// Original canonical metadata retains identities omitted from the inline view.
    pub final_message_id_source: Option<super::content::ContentRef>,
    pub message_count: u32,
    pub tool_call_count: u32,
    pub outcome: TurnProcessOutcome,
    /// Normalized terminal failure, retained at its durable transcript position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<super::failure::RuntimeClientAttemptFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) fn terminal_turn(
    event: crate::events::types::RuntimeEventEnvelope,
    cursor: super::transcript::RuntimeClientTranscriptCursor,
) -> Result<TurnProcessView, String> {
    use crate::events::types::RuntimeEvent;
    let failure = match &event.event {
        RuntimeEvent::AttemptFailed { error, .. } => {
            Some(super::failure::finite_attempt_failure(error).0)
        }
        _ => None,
    };
    let outcome = match event.event {
        RuntimeEvent::AttemptCancelled { .. } => TurnProcessOutcome::Cancelled,
        RuntimeEvent::AttemptFailed { .. } => TurnProcessOutcome::Failed,
        RuntimeEvent::AttemptTimedOut { .. } => TurnProcessOutcome::TimedOut,
        RuntimeEvent::AttemptLimitExceeded { .. } => TurnProcessOutcome::LimitExceeded,
        _ => return Err("invalid terminal Attempt reference".into()),
    };
    Ok(TurnProcessView {
        conversation_id: event.conversation_id,
        attempt_id: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() <= 512)
            .cloned(),
        attempt_id_source: event
            .attempt_id
            .as_ref()
            .filter(|id| id.as_str().len() > 512)
            .map(|id| crate::durable::sqlite::journal_attempt_source(event.sequence, id))
            .transpose()
            .map_err(|_| "terminal owner field unavailable")?,
        event_id: Some(event.event_id),
        control_cursor: cursor,
        final_message_id: None,
        final_message_locator: None,
        final_message_id_deferred: false,
        final_message_id_source: None,
        message_count: 0,
        tool_call_count: 0,
        outcome,
        failure,
        started_at: None,
        ended_at: Some(event.timestamp),
    })
}
