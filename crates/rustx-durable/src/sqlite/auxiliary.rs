//! SQLite owner of the auxiliary Call/request domain: schema, validation,
//! index writes, commits, confirmations, and bounded audit reads. Every
//! helper runs inside the transaction its caller opened; this module root
//! names the shared vocabulary its children use.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::ConversationStoreError;
use crate::auxiliary::{
    AuxiliaryCallStartCommit, AuxiliaryRequestStartCommit, AuxiliarySettlementCommit,
    AuxiliaryWriteFailure, call_settled_event_id, request_terminal_event_id,
};
use crate::context_resources::ResourceLimits;
use crate::model::snapshot::ModelRequestOwner;
use rustx_types::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx_types::model::auxiliary::{
    AUXILIARY_AUDIT_PAGE_LIMIT_MAX, AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
    AUXILIARY_CALL_START_ENCODED_MAX, AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX,
    AUXILIARY_REQUEST_ID_BYTES_MAX, AUXILIARY_REQUEST_MODEL_BYTES_MAX,
    AUXILIARY_REQUEST_START_ENCODED_MAX, AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
    AuxiliaryAuditCut, AuxiliaryAuditFact, AuxiliaryAuditPage, AuxiliaryAuditRecord,
    AuxiliaryBusinessSource, AuxiliaryCallAudit, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition, AuxiliaryFinishReason,
    AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal, AuxiliaryRequestAudit,
    AuxiliaryRequestEvidence, AuxiliaryRequestStart, AuxiliaryRequestTerminal,
    AuxiliaryUsageCompleteness,
};
use rustx_types::model::generation_evidence::GenerationEvidence;
use rustx_types::model::types::ModelUsage;
use rustx_types::runtime::identity::{
    AuxiliaryCallId, CompactionOperationId, ConversationId, EventId, MessageId, RequestId,
};

use super::journal::persist_event_tx;
use super::{
    SqliteConversationStore, decode, journal_prefix, seq_to_i64, sequence_from_i64, storage,
};

mod audit;
mod authority;
mod commit;
mod confirm;
mod index;
mod read;
mod retry;
mod rows;
mod schema;
mod title;
mod validation;

use audit::{
    AUXILIARY_CALL_REQUESTS_SQL, audit_fact_bytes, audit_record_bytes, audit_total_at_connection,
    call_audit_at_tx, has_call_journal_fact_through_tx, has_call_journal_fact_tx,
    has_call_settlement_journal_tx, has_request_journal_fact_through_tx,
    has_request_terminal_journal_tx, request_audit_at_tx,
};
use authority::validate_call_start_authority_tx;
use commit::settlement_ids;
use index::insert_audit_fact_tx;
use rows::{
    AuditFactRow, RequestRow, audit_fact_row, call_has_requests_query, call_row_query, call_row_tx,
    event_at_connection, next_request_ordinal_tx, open_request_query, open_request_tx,
    owner_row_query, owner_row_tx, request_row_query, request_row_tx, request_snapshot_exists,
    request_snapshot_started_sequence, require_audit_fact, validate_audit_fact_event,
    validate_owner_row_tx, validate_request_terminal_for_row,
};
use schema::AUXILIARY_SEQUENCE_ENCODING_DIGITS_MAX;
use validation::{
    bounded_encoded_len, call_outcome_for_terminal, call_start_matches, call_started_event_id,
    outcome_name, request_start_matches, request_started_event_id, same_terminal_envelope,
    terminal_kind, validate_call_start_input, validate_request_evidence,
    validate_request_start_input, validate_settlement_pair, validate_terminal_envelope_shape,
};

#[cfg(any(test, feature = "test-hooks"))]
#[cfg(any(test, feature = "test-hooks"))]
pub use audit::install_after_request_row_read_hook;
pub use authority::{
    validate_call_event_tx, validate_request_start_event_tx, validate_request_terminal_event_tx,
};
pub use index::{index_event_tx, insert_owner_tx, load_request_owner_tx};
pub use retry::validate_retry_event_tx;
pub use schema::{create_schema, verify_schema};

impl crate::AuxiliaryStore for SqliteConversationStore {
    fn commit_auxiliary_call_start(
        &self,
        start: &rustx_types::model::auxiliary::AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::AuxiliaryCallStartCommit, crate::AuxiliaryWriteFailure> {
        SqliteConversationStore::commit_auxiliary_call_start(self, start, timestamp)
    }

    fn commit_auxiliary_title_call_start(
        &self,
        start: &AuxiliaryCallStart,
        source: &crate::auxiliary::AuxiliaryTitleSourceBinding,
        timestamp: DateTime<Utc>,
    ) -> Result<AuxiliaryCallStartCommit, AuxiliaryWriteFailure> {
        self.commit_auxiliary_call_start_bound(start, timestamp, Some(source))
    }

    fn recover_auxiliary_title_source(
        &self,
        source: &crate::auxiliary::AuxiliaryTitleSourceBinding,
        timestamp: DateTime<Utc>,
    ) -> Result<(), AuxiliaryWriteFailure> {
        self.recover_title_source(source, timestamp)
    }

    fn commit_auxiliary_request_start(
        &self,
        start: &rustx_types::model::auxiliary::AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::AuxiliaryRequestStartCommit, crate::AuxiliaryWriteFailure> {
        SqliteConversationStore::commit_auxiliary_request_start(self, start, timestamp)
    }

    fn commit_auxiliary_retry_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        retry_scheduled: RuntimeEventEnvelope,
    ) -> Result<crate::AuxiliaryRetryCommit, AuxiliaryWriteFailure> {
        self.commit_auxiliary_retry(&request_terminal, &retry_scheduled)
    }

    fn confirm_auxiliary_retry_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        retry_scheduled: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryCommitConfirmation<crate::AuxiliaryRetryCommit>, ConversationStoreError>
    {
        self.confirm_auxiliary_retry(request_terminal, retry_scheduled)
    }

    fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::AuxiliarySettlementCommit, crate::AuxiliaryWriteFailure> {
        SqliteConversationStore::commit_auxiliary_request_settlement(
            self,
            request_terminal,
            call_settled,
        )
    }

    fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::AuxiliarySettlementCommit, crate::AuxiliaryWriteFailure> {
        SqliteConversationStore::commit_auxiliary_call_settlement(self, call_settled)
    }

    fn confirm_auxiliary_call_start(
        &self,
        expected: &rustx_types::model::auxiliary::AuxiliaryCallStart,
    ) -> Result<
        rustx_types::model::auxiliary::AuxiliaryCommitConfirmation<crate::AuxiliaryCallStartCommit>,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_call_start(self, expected)
    }

    fn confirm_auxiliary_request_start(
        &self,
        expected: &rustx_types::model::auxiliary::AuxiliaryRequestStart,
    ) -> Result<
        rustx_types::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::AuxiliaryRequestStartCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_request_start(self, expected)
    }

    fn confirm_auxiliary_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        rustx_types::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::AuxiliarySettlementCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_settlement(self, request_terminal, call_settled)
    }

    fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        rustx_types::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::AuxiliarySettlementCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_call_settlement(self, call_settled)
    }

    fn auxiliary_audit_cut(
        &self,
    ) -> Result<rustx_types::model::auxiliary::AuxiliaryAuditCut, ConversationStoreError> {
        SqliteConversationStore::auxiliary_audit_cut(self)
    }

    fn read_auxiliary_audit(
        &self,
        cut: rustx_types::model::auxiliary::AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<rustx_types::model::auxiliary::AuxiliaryAuditPage, ConversationStoreError> {
        SqliteConversationStore::read_auxiliary_audit(self, cut, after_offset, limit)
    }

    fn auxiliary_audit_total(
        &self,
        cut: rustx_types::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<u64, ConversationStoreError> {
        SqliteConversationStore::auxiliary_audit_total(self, cut)
    }

    fn load_auxiliary_call(
        &self,
        call_id: &rustx_types::runtime::identity::AuxiliaryCallId,
        cut: rustx_types::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<Option<rustx_types::model::auxiliary::AuxiliaryCallAudit>, ConversationStoreError>
    {
        SqliteConversationStore::load_auxiliary_call(self, call_id, cut)
    }

    fn load_auxiliary_submission(
        &self,
        correlation: &str,
    ) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError> {
        self.load_auxiliary_submission(correlation)
    }

    fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: rustx_types::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<Option<rustx_types::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError>
    {
        SqliteConversationStore::load_auxiliary_request(self, request_id, cut)
    }

    fn read_auxiliary_call_requests(
        &self,
        call_id: &rustx_types::runtime::identity::AuxiliaryCallId,
        cut: rustx_types::model::auxiliary::AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<rustx_types::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError>
    {
        SqliteConversationStore::read_auxiliary_call_requests(
            self,
            call_id,
            cut,
            after_started_sequence,
            limit,
        )
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-hooks"))]
pub use read::install_lookup_hook;
