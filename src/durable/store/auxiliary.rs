//! The auxiliary request ownership and audit contract (Issue #486).
//!
//! Auxiliary Calls and their physical requests are an execution domain of
//! their own: a Call start, its sequential request starts, and their atomic
//! settlement are committed and confirmed here, and their bounded audit is
//! read at an exact Journal cut. Primary Agent request ownership stays with
//! [`super::request::RequestStore`].

use crate::durable::ConversationStoreError;
use crate::events::types::RuntimeEventEnvelope;
use crate::model::auxiliary::{
    AuxiliaryAuditCut, AuxiliaryAuditPage, AuxiliaryCallAudit, AuxiliaryCallStart,
    AuxiliaryCommitConfirmation, AuxiliaryRequestAudit, AuxiliaryRequestStart,
};
use crate::runtime::identity::RequestId;
use chrono::{DateTime, Utc};

/// Auxiliary Call/request commits, confirmations, and bounded audit reads.
#[allow(clippy::missing_errors_doc)]
pub trait AuxiliaryStore: Send + Sync + 'static {
    /// Commits one unique auxiliary Call start with bounded attribution and
    /// exact frozen semantic/invocation fingerprints.
    fn commit_auxiliary_call_start(
        &self,
        start: &AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryCallStartCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Commits one sequential physical request start under an active Call.
    fn commit_auxiliary_request_start(
        &self,
        start: &AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryRequestStartCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Atomically commits the final open request terminal and Call terminal.
    /// Construct both with [`crate::durable::auxiliary::auxiliary_settlement_envelope`]
    /// and retain them unchanged for retry and confirmation.
    fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Construct the terminal with [`crate::durable::auxiliary::auxiliary_settlement_envelope`].
    /// Settles one active zero-request Call. No physical request or primary
    /// Snapshot is fabricated. Intermediate request transitions extend this
    /// contract separately before a settled-request backoff cancellation.
    fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Reads exact authoritative confirmation of one Call start.
    fn confirm_auxiliary_call_start(
        &self,
        expected: &AuxiliaryCallStart,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliaryCallStartCommit>,
        ConversationStoreError,
    >;

    /// Reads exact authoritative confirmation of one request start.
    fn confirm_auxiliary_request_start(
        &self,
        expected: &AuxiliaryRequestStart,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliaryRequestStartCommit>,
        ConversationStoreError,
    >;

    /// Reads exact authoritative confirmation of one atomic settlement pair.
    fn confirm_auxiliary_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliarySettlementCommit>,
        ConversationStoreError,
    >;

    /// Reads exact authoritative confirmation of one Call-only settlement.
    fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliarySettlementCommit>,
        ConversationStoreError,
    >;

    /// Captures the current Journal cut for bounded auxiliary audit reads.
    fn auxiliary_audit_cut(&self) -> Result<AuxiliaryAuditCut, ConversationStoreError>;

    /// Returns an advancing dense-offset page of auxiliary audit facts at `cut`.
    fn read_auxiliary_audit(
        &self,
        cut: AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<AuxiliaryAuditPage, ConversationStoreError>;

    /// Returns the dense auxiliary audit total visible at `cut`.
    fn auxiliary_audit_total(&self, cut: AuxiliaryAuditCut) -> Result<u64, ConversationStoreError>;

    /// Reads one keyed Call audit projection at `cut`.
    fn load_auxiliary_call(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError>;

    /// Reads one keyed physical-request audit projection at `cut`.
    fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: AuxiliaryAuditCut,
    ) -> Result<Option<AuxiliaryRequestAudit>, ConversationStoreError>;

    /// Reads at most `limit` local requests of one Call in start order at `cut`.
    /// Pass the last returned `started_sequence` to advance; an empty page ends
    /// iteration. A supplied cursor must name a visible request start of this
    /// Call. Later facts and inherited source audit never enter this lookup.
    /// Limits use [`crate::model::auxiliary::AUXILIARY_AUDIT_PAGE_LIMIT_MAX`].
    fn read_auxiliary_call_requests(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuxiliaryRequestAudit>, ConversationStoreError>;
}
