//! SQLite owner of the auxiliary Call/request domain: schema, validation,
//! index writes, commits, confirmations, and bounded audit reads. Every
//! helper runs inside the transaction its caller opened; this module root
//! names the shared vocabulary its children use.
#![allow(clippy::too_many_lines, clippy::type_complexity)]

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::context::resources::ResourceLimits;
use crate::durable::ConversationStoreError;
use crate::durable::auxiliary::{
    AuxiliaryCallStartCommit, AuxiliaryRequestStartCommit, AuxiliarySettlementCommit,
    AuxiliaryWriteFailure, call_settled_event_id, request_terminal_event_id,
};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::model::auxiliary::{
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
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::snapshot::ModelRequestOwner;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
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
mod rows;
mod schema;
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

#[cfg(test)]
pub(in crate::durable::sqlite) use audit::install_after_request_row_read_hook;
pub(in crate::durable::sqlite) use authority::{
    validate_call_event_tx, validate_request_start_event_tx, validate_request_terminal_event_tx,
};
pub(in crate::durable::sqlite) use index::{index_event_tx, load_request_owner_tx};
pub(in crate::durable::sqlite) use schema::{create_schema, verify_schema};

impl crate::durable::AuxiliaryStore for SqliteConversationStore {
    fn commit_auxiliary_call_start(
        &self,
        start: &crate::model::auxiliary::AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryCallStartCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_call_start(self, start, timestamp)
    }

    fn commit_auxiliary_request_start(
        &self,
        start: &crate::model::auxiliary::AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<crate::durable::AuxiliaryRequestStartCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_request_start(self, start, timestamp)
    }

    fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_request_settlement(
            self,
            request_terminal,
            call_settled,
        )
    }

    fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<crate::durable::AuxiliarySettlementCommit, crate::durable::AuxiliaryWriteFailure>
    {
        SqliteConversationStore::commit_auxiliary_call_settlement(self, call_settled)
    }

    fn confirm_auxiliary_call_start(
        &self,
        expected: &crate::model::auxiliary::AuxiliaryCallStart,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliaryCallStartCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_call_start(self, expected)
    }

    fn confirm_auxiliary_request_start(
        &self,
        expected: &crate::model::auxiliary::AuxiliaryRequestStart,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliaryRequestStartCommit,
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
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliarySettlementCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_settlement(self, request_terminal, call_settled)
    }

    fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<
        crate::model::auxiliary::AuxiliaryCommitConfirmation<
            crate::durable::AuxiliarySettlementCommit,
        >,
        ConversationStoreError,
    > {
        SqliteConversationStore::confirm_auxiliary_call_settlement(self, call_settled)
    }

    fn auxiliary_audit_cut(
        &self,
    ) -> Result<crate::model::auxiliary::AuxiliaryAuditCut, ConversationStoreError> {
        SqliteConversationStore::auxiliary_audit_cut(self)
    }

    fn read_auxiliary_audit(
        &self,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
        after_offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::model::auxiliary::AuxiliaryAuditPage, ConversationStoreError> {
        SqliteConversationStore::read_auxiliary_audit(self, cut, after_offset, limit)
    }

    fn auxiliary_audit_total(
        &self,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<u64, ConversationStoreError> {
        SqliteConversationStore::auxiliary_audit_total(self, cut)
    }

    fn load_auxiliary_call(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<Option<crate::model::auxiliary::AuxiliaryCallAudit>, ConversationStoreError> {
        SqliteConversationStore::load_auxiliary_call(self, call_id, cut)
    }

    fn load_auxiliary_request(
        &self,
        request_id: &RequestId,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
    ) -> Result<Option<crate::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError>
    {
        SqliteConversationStore::load_auxiliary_request(self, request_id, cut)
    }

    fn read_auxiliary_call_requests(
        &self,
        call_id: &crate::runtime::identity::AuxiliaryCallId,
        cut: crate::model::auxiliary::AuxiliaryAuditCut,
        after_started_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<crate::model::auxiliary::AuxiliaryRequestAudit>, ConversationStoreError> {
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
