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

    /// Atomically binds a native Title source while committing its first Call.
    fn commit_auxiliary_title_call_start(
        &self,
        start: &AuxiliaryCallStart,
        source: &crate::durable::auxiliary::AuxiliaryTitleSourceBinding,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryCallStartCommit, crate::durable::AuxiliaryWriteFailure>;

    /// After the native owner has excluded any previous live owner, settle a
    /// stranded zero-request association. This never replays model execution.
    fn recover_auxiliary_title_source(
        &self,
        source: &crate::durable::auxiliary::AuxiliaryTitleSourceBinding,
        timestamp: DateTime<Utc>,
    ) -> Result<(), crate::durable::AuxiliaryWriteFailure>;

    /// Commits one sequential physical request start under an active Call.
    fn commit_auxiliary_request_start(
        &self,
        start: &AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryRequestStartCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Atomically settles a failed physical request and authorizes its next ordinal.
    fn commit_auxiliary_retry_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        retry_scheduled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliaryRetryCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Confirms the exact pair; absence while the writer may still commit is Unknown.
    fn confirm_auxiliary_retry_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        retry_scheduled: &RuntimeEventEnvelope,
    ) -> Result<
        AuxiliaryCommitConfirmation<crate::durable::AuxiliaryRetryCommit>,
        ConversationStoreError,
    >;

    /// Atomically commits the final open request terminal and Call terminal.
    /// Construct both with [`crate::durable::auxiliary::auxiliary_settlement_envelope`]
    /// and retain them unchanged for retry and confirmation.
    fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>;

    /// Construct the terminal with [`crate::durable::auxiliary::auxiliary_settlement_envelope`].
    /// Settles one active zero-request Call or a backoff whose failed request
    /// and retry are already committed. No physical request or primary Snapshot
    /// is fabricated; a backoff terminal cannot claim completed execution.
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
