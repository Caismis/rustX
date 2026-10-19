//! Typed semantic-transition results for auxiliary model Calls.
//!
//! The Store owns the transitions. These receipts expose committed Journal
//! facts and bounded audit projections without introducing an executor,
//! scheduler, or retry policy.

use crate::durable::inbox::ConversationStoreError;
use crate::events::types::RuntimeEventEnvelope;
use crate::model::auxiliary::{
    AuxiliaryCallAudit, AuxiliaryCommitDisposition, AuxiliaryRequestAudit,
};

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
    /// An actual rollback completed, or validation failed before SQL opened.
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
