//! Typed semantic-transition results for auxiliary model Calls.
//!
//! The Store owns the transitions. These receipts expose committed Journal
//! facts and bounded audit projections without introducing an executor,
//! scheduler, or retry policy.

use crate::durable::ConversationStoreError;
use crate::events::types::RuntimeEventEnvelope;
use crate::model::auxiliary::{
    AuxiliaryCallAudit, AuxiliaryCommitDisposition, AuxiliaryRequestAudit,
};
use crate::runtime::identity::{AuxiliaryCallId, EventId, RequestId};

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
    pub(crate) fn known(error: ConversationStoreError) -> Self {
        Self::KnownNotCommitted { error }
    }

    #[must_use]
    pub(crate) fn unknown(error: ConversationStoreError) -> Self {
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
    conversation_id: crate::runtime::identity::ConversationId,
    timestamp: chrono::DateTime<chrono::Utc>,
    event: crate::events::types::RuntimeEvent,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let event_id = settlement_event_id(&event)?;
    Ok(RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
        event_id,
        sequence: 0,
        conversation_id,
        attempt_id: None,
        turn_id: None,
        timestamp,
        event,
    })
}

pub(crate) fn settlement_event_id(
    event: &crate::events::types::RuntimeEvent,
) -> Result<crate::runtime::identity::EventId, ConversationStoreError> {
    use crate::events::types::RuntimeEvent;
    use crate::model::ModelRequestOwner;
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
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary settlement requires an auxiliary request or Call terminal".into(),
            ));
        }
    };
    Ok(value)
}

pub(crate) fn request_terminal_event_id(request_id: &RequestId, completed: bool) -> EventId {
    EventId::new(format!(
        "auxiliary-request-{}:{request_id}",
        if completed { "completed" } else { "failed" }
    ))
}

pub(crate) fn call_settled_event_id(call_id: &AuxiliaryCallId) -> EventId {
    EventId::new(format!("auxiliary-call-settled:{call_id}"))
}
