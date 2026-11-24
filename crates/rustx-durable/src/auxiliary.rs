//! Typed semantic-transition results for auxiliary model Calls.
//!
//! The Store owns the transitions. These receipts expose committed Journal
//! facts and bounded audit projections without introducing an executor,
//! scheduler, or retry policy.

use crate::ConversationStoreError;
use rustx_types::events::types::RuntimeEventEnvelope;
use rustx_types::model::auxiliary::{
    AuxiliaryCallAudit, AuxiliaryCommitDisposition, AuxiliaryRequestAudit,
};
use rustx_types::runtime::identity::{AuxiliaryCallId, EventId, RequestId};

/// Native-only permission to bind an automatic Title admission to its exact source.
/// Public for Store implementations; construction remains with the native Session owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxiliaryTitleSourceBinding {
    pub conversation_id: rustx_types::runtime::identity::ConversationId,
    pub message_id: rustx_types::runtime::identity::MessageId,
    pub position: u64,
    pub text: String,
}
impl AuxiliaryTitleSourceBinding {
    #[must_use]
    pub fn new(source: &crate::store::canonical::FirstHumanSource) -> Option<Self> {
        (source.local && source.text.is_some()).then(|| Self {
            conversation_id: source.conversation_id.clone(),
            message_id: source.message_id.clone(),
            position: source.position,
            text: source.text.clone().expect("usable source text"),
        })
    }
}

/// Receipt of one durable auxiliary Call start.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxiliaryCallStartCommit {
    pub disposition: AuxiliaryCommitDisposition,
    pub call: AuxiliaryCallAudit,
    pub event: RuntimeEventEnvelope,
}

/// Receipt of one durable auxiliary physical-request start.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxiliaryRequestStartCommit {
    pub disposition: AuxiliaryCommitDisposition,
    pub request: AuxiliaryRequestAudit,
    pub event: RuntimeEventEnvelope,
}

/// Receipt of one atomic request-terminal and Call settlement.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxiliarySettlementCommit {
    pub disposition: AuxiliaryCommitDisposition,
    pub request: Option<AuxiliaryRequestAudit>,
    pub call: AuxiliaryCallAudit,
    pub request_terminal: Option<RuntimeEventEnvelope>,
    pub call_settled: RuntimeEventEnvelope,
}

/// Receipt of an atomic failed request and scheduled retry; the Call stays active.
#[derive(Debug, Clone, PartialEq)]
pub struct AuxiliaryRetryCommit {
    pub disposition: AuxiliaryCommitDisposition,
    pub request: AuxiliaryRequestAudit,
    pub request_terminal: RuntimeEventEnvelope,
    pub retry_scheduled: RuntimeEventEnvelope,
}

/// Typed outcome of an auxiliary semantic write.
#[derive(Debug)]
pub enum AuxiliaryWriteFailure {
    /// Explicit rollback or SQLite-proven auto-rollback completed, or no write began.
    /// This establishes transaction certainty only; `error` retains its cause.
    KnownNotCommitted { error: ConversationStoreError },
    /// Commit or rollback failed; the physical transaction outcome is unknown.
    Unknown { error: ConversationStoreError },
}

impl AuxiliaryWriteFailure {
    #[must_use]
    pub fn known(error: ConversationStoreError) -> Self {
        Self::KnownNotCommitted { error }
    }

    #[must_use]
    pub fn unknown(error: ConversationStoreError) -> Self {
        Self::Unknown { error }
    }

    /// Returns the underlying Store error without changing commit evidence.
    #[must_use]
    pub fn error(&self) -> &ConversationStoreError {
        match self {
            Self::KnownNotCommitted { error } | Self::Unknown { error } => error,
        }
    }
}

impl core::fmt::Display for AuxiliaryWriteFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::KnownNotCommitted { error } => {
                write!(formatter, "auxiliary write did not commit: {error}")
            }
            Self::Unknown { error } => {
                write!(
                    formatter,
                    "auxiliary write commit outcome is unknown: {error}"
                )
            }
        }
    }
}

impl std::error::Error for AuxiliaryWriteFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error())
    }
}

/// Constructs an uncommitted auxiliary terminal with the canonical event identity.
///
/// Reuse the returned envelope unchanged for write retry or exact confirmation.
/// The Store still validates ownership, evidence and atomic settlement; this
/// constructor does not authorize execution or commit anything.
///
/// # Errors
/// Rejects Agent-owned terminals and nonterminal events.
pub fn auxiliary_settlement_envelope(
    conversation_id: rustx_types::runtime::identity::ConversationId,
    timestamp: chrono::DateTime<chrono::Utc>,
    event: rustx_types::events::types::RuntimeEvent,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let event_id = settlement_event_id(&event)?;
    Ok(RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: rustx_types::events::types::EVENT_SCHEMA_VERSION,
        event_id,
        sequence: 0,
        conversation_id,
        attempt_id: None,
        turn_id: None,
        timestamp,
        event,
    })
}

pub fn settlement_event_id(
    event: &rustx_types::events::types::RuntimeEvent,
) -> Result<rustx_types::runtime::identity::EventId, ConversationStoreError> {
    use crate::model::snapshot::ModelRequestOwner;
    use rustx_types::events::types::RuntimeEvent;
    let value = match event {
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary { .. },
            request_id,
            ..
        } => request_terminal_event_id(request_id, true),
        RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Auxiliary { .. },
            request_id,
            ..
        } => request_terminal_event_id(request_id, false),
        RuntimeEvent::AuxiliaryCallSettled { call_id, .. } => call_settled_event_id(call_id),
        RuntimeEvent::AuxiliaryRetryScheduled { request_id, .. } => retry_event_id(request_id),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary settlement requires an auxiliary request or Call terminal".into(),
            ));
        }
    };
    Ok(value)
}

#[must_use]
pub fn request_terminal_event_id(request_id: &RequestId, completed: bool) -> EventId {
    EventId::new(format!(
        "auxiliary-request-{}:{request_id}",
        if completed { "completed" } else { "failed" }
    ))
}

#[must_use]
pub fn call_settled_event_id(call_id: &AuxiliaryCallId) -> EventId {
    EventId::new(format!("auxiliary-call-settled:{call_id}"))
}

#[must_use]
pub fn retry_event_id(request_id: &RequestId) -> EventId {
    EventId::new(format!("auxiliary-retry-scheduled:{request_id}"))
}
