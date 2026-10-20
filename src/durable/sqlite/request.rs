//! The Request Snapshot domain and the atomic model-turn start transition.

use rusqlite::OptionalExtension;
pub(super) mod contribution;
pub(crate) mod snapshot;

use self::contribution::{
    consume_pending_carryover_tx, read_contribution_head, read_logical_step_progress_tx,
    upsert_contribution_head_tx, validate_contribution_start, validate_pending_carryover_source_tx,
    verify_contribution_start_tx, verify_frozen_contributions_tx,
};
use self::snapshot::{
    encode_snapshot, find_request_start_event, load_request_snapshot, read_manifest_digest,
    read_stored_snapshot_root, reconstruct_snapshot_input, stored_snapshot_digest,
    validate_accepted_continuation_source, validate_snapshot_identity,
};
#[cfg(test)]
use super::RequestStartFaultOperation;
use super::canonical::canonical_observation_batch;
use super::canonical::ledger::{
    append_message_and_surface, ensure_surface_head, load_message, load_message_tx,
    transcript_visible_message, validate_canonical_frontier,
};
use super::journal::{
    ValidatedRequestStart, is_terminal, persist_event_tx, persist_event_tx_deferred,
    persist_event_tx_internal,
};
use super::{
    SqliteConversationStore, content, decode, journal_prefix, manifest, nonnegative,
    observation_receipt, observation_request, observation_request_summary, observation_trace,
    optional_display_layout, projection, reading, seq_to_i64, sequence_from_i64, storage,
};
use crate::durable::{
    ContributionEmissionRecord, ConversationStoreError, ModelTurnStartCommit,
    ModelTurnStartCommitDisposition, RequestSnapshotPage, RequestStore,
};
use crate::events::types::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, contribution_emission_event_id,
};
use crate::message::types::MessageBlock;
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::runtime::identity::{
    AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, RequestId,
};
use crate::runtime::process_death;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl RequestStore for SqliteConversationStore {
    fn attempt_prompt(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError> {
        reading::attempt_prompt(&*self.lock()?, attempt, through)
    }
    fn commit_model_turn_start(
        &self,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<ModelTurnStartCommit, ConversationStoreError> {
        validate_snapshot_identity(snapshot)?;
        snapshot
            .input
            .validate_budget()
            .map_err(ConversationStoreError::InvalidReference)?;
        // The one input validation rule shared by the fresh commit and the
        // idempotent retry (Issue #12, M9b): the exact ordered
        // request-scoped context the caller supplies must equal the frozen
        // `snapshot.request_context_ids` before the store chooses a path or
        // touches durable state. A fresh commit must never be able to append
        // context while persisting a snapshot whose `request_context_ids`
        // disagrees with what it just appended.
        validate_request_context(snapshot, context)?;
        validate_contribution_start(snapshot, context)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("request start transaction: {error}")))?;
        let admission_epoch = self.conn.manifest.capture(&transaction)?;
        let existing: Option<(Option<String>, Option<i64>, String)> = transaction
            .query_row(
                "SELECT CASE WHEN octet_length(snapshot_json)<=67108864 THEN snapshot_json END,started_sequence,CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END FROM request_snapshots WHERE request_id = ?1",
                [snapshot.request_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, read_manifest_digest(row, 2)?)),
            )
            .optional()
            .map_err(|error| storage(format!("request snapshot probe: {error}")))?;
        if let Some((json, started_sequence, integrity)) = existing {
            let json = json.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "frozen request record exceeds byte budget".into(),
                )
            })?;
            if stored_snapshot_digest(
                &json,
                read_stored_snapshot_root(&transaction, &snapshot.request_id)?,
            ) != integrity
            {
                return Err(ConversationStoreError::InvalidReference(
                    "corrupt frozen request manifest".into(),
                ));
            }
            let mut verified = verify_committed_start_tx(
                &transaction,
                &json,
                started_sequence,
                context,
                snapshot,
            )?;
            transaction
                .commit()
                .map_err(|error| storage(format!("request start retry commit: {error}")))?;
            verified.verified_snapshot = Some(Box::new(snapshot.clone()));
            return Ok(verified);
        }
        let projection = projection::current_checkpoint_id(&transaction)?;
        if projection.as_ref() != snapshot.history_cut.checkpoint_id.as_ref() {
            return Err(ConversationStoreError::InvalidReference(
                "actual request would start from a stale Projection Head".into(),
            ));
        }
        if snapshot.history_cut.base_frontier > snapshot.history_cut.ledger {
            return Err(ConversationStoreError::InvalidReference(
                "invalid Request History Cut".into(),
            ));
        }
        if snapshot.identity.retry_number != 0 {
            verify_frozen_contributions_tx(&transaction, snapshot)?;
        }
        // The request-scoped canonical context commits first, inside the
        // same transaction as the snapshot and the start fact: a failure
        // anywhere below rolls all of it back, so request-scoped context
        // can never become canonical without its request starting.
        let selection = manifest::persist(&transaction, &snapshot.input, &self.conn.manifest)?;
        let mut persisted = self.insert_fresh_start_tx(
            &transaction,
            context,
            snapshot,
            timestamp,
            &selection,
            admission_epoch,
        )?;
        process_death::reach("before:commit_model_turn_start");
        transaction
            .commit()
            .map_err(|error| storage(format!("request start commit: {error}")))?;
        process_death::reach("after:commit_model_turn_start");
        selection.commit(&self.conn.manifest);
        persisted.verified_snapshot = Some(Box::new(snapshot.clone()));
        Ok(persisted)
    }
    fn latest_contribution_emission(
        &self,
        producer: &crate::runtime::identity::ContextContributorIdentity,
        key: &str,
    ) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
        #[cfg(test)]
        self.contribution_head_lookup_reads
            .fetch_add(1, Ordering::SeqCst);
        let connection = self.lock()?;
        read_contribution_head(&connection, producer, key)
    }
    fn load_pending_unresolved_output_stream_id(
        &self,
    ) -> Result<Option<PublicationStreamId>, ConversationStoreError> {
        let connection = self.lock()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT pending_unresolved_output_stream_id FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("pending carryover lookup: {error}")))?;
        Ok(value.map(PublicationStreamId::new))
    }
    fn commit_attempt_terminal_with_carryover(
        &self,
        event: RuntimeEventEnvelope,
        pending_source: Option<PublicationStreamId>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        if !is_terminal(&event.event)
            || !matches!(
                event.event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        {
            return Err(ConversationStoreError::InvalidReference(
                "carryover terminal transition accepts only attempt terminal events".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("attempt terminal transaction: {error}")))?;
        #[cfg(test)]
        {
            self.terminal_event_attempts.fetch_add(1, Ordering::SeqCst);
            if Self::consume(&self.fail_terminal_event_remaining) {
                return Err(storage("fault injected: terminal event commit"));
            }
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage("fault injected: event commit"));
            }
        }
        validate_pending_carryover_source_tx(&transaction, &event, pending_source.as_ref())?;
        process_death::reach_event("before:attempt_terminal", &event.event);
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        let source = pending_source.as_ref().map(PublicationStreamId::as_str);
        transaction
            .execute(
                "UPDATE rustx_store SET pending_unresolved_output_stream_id=?1 WHERE id=1",
                [source],
            )
            .map_err(|error| storage(format!("install pending carryover source: {error}")))?;
        process_death::reach("before:commit_attempt_terminal");
        transaction
            .commit()
            .map_err(|error| storage(format!("attempt terminal commit: {error}")))?;
        process_death::reach("after:commit_attempt_terminal");
        Ok(persisted.event)
    }
    fn current_logical_step_progress(&self) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        let value: i64 = connection
            .query_row(
                "SELECT logical_step_sequence FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("Todo progress lookup: {error}")))?;
        nonnegative(value, "Todo progress sequence")
    }
    fn load_request_snapshot(
        &self,
        request_id: &RequestId,
    ) -> Result<RequestSnapshot, ConversationStoreError> {
        #[cfg(test)]
        self.request_snapshot_body_reads
            .fetch_add(1, Ordering::SeqCst);
        {
            let connection = self.lock()?;
            load_request_snapshot(&connection, &self.conversation_id, request_id)
        }
    }
    fn reconstruct_model_request(
        &self,
        request_id: &crate::runtime::identity::RequestId,
    ) -> Result<crate::model::types::ModelRequest, ConversationStoreError> {
        let connection = self.lock()?;
        let snapshot = load_request_snapshot(&connection, &self.conversation_id, request_id)?;
        snapshot
            .reconstruct_with(|reference| {
                if reference.conversation_id != self.conversation_id {
                    return Err(
                        crate::model::snapshot::RequestReconstructionError::Conversation(
                            "foreign frozen Original scope".to_owned(),
                        ),
                    );
                }
                load_message(&connection, &reference.message_id).map_err(|error| {
                    crate::model::snapshot::RequestReconstructionError::Conversation(
                        error.to_string(),
                    )
                })
            })
            .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))
    }
    fn read_request_snapshots(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<RequestSnapshotPage, ConversationStoreError> {
        #[cfg(test)]
        self.request_snapshot_page_reads
            .fetch_add(1, Ordering::SeqCst);
        let connection = self.lock()?;
        let after = seq_to_i64(after_sequence.unwrap_or(0))?;
        let limit = i64::try_from(limit)
            .map_err(|_| storage("request snapshot page limit is too large"))?;
        let mut statement = connection
            .prepare(
                "SELECT request_id,started_sequence FROM request_snapshots
                 WHERE started_sequence IS NOT NULL AND started_sequence > ?1
                 ORDER BY started_sequence, request_id LIMIT ?2",
            )
            .map_err(|error| storage(format!("request snapshot page: {error}")))?;
        let rows = statement
            .query_map(params![after, limit], |row| {
                let request_id: String = row.get(0)?;
                let sequence: i64 = row.get(1)?;
                Ok((request_id, sequence))
            })
            .map_err(|error| storage(format!("request snapshot page query: {error}")))?;
        let rows: Vec<(RequestId, u64)> = rows
            .map(|row| {
                let (request_id, sequence) =
                    row.map_err(|error| storage(format!("request snapshot page row: {error}")))?;
                Ok((RequestId::new(request_id), sequence_from_i64(sequence)?))
            })
            .collect::<Result<_, ConversationStoreError>>()?;
        drop(statement);
        drop(connection);

        let next_sequence = rows.last().map(|(_, sequence)| *sequence);
        let snapshots = rows
            .into_iter()
            .map(|(request_id, _)| self.load_request_snapshot(&request_id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RequestSnapshotPage {
            snapshots,
            next_sequence,
        })
    }
}

pub(super) fn request_start_observation_tx(
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    context: &[MessageBlock],
    snapshot: &RequestSnapshot,
    started: RuntimeEventEnvelope,
    contribution_emissions: Vec<RuntimeEventEnvelope>,
) -> Result<ModelTurnStartCommit, ConversationStoreError> {
    let context_journal_range = Some((
        started.sequence,
        contribution_emissions
            .last()
            .map_or(started.sequence, |event| event.sequence),
    ));
    let context_receipts = canonical_observation_batch(
        transaction,
        conversation,
        &context
            .iter()
            .map(|message| (message.id(), None))
            .collect::<Vec<_>>(),
        context_journal_range,
    )?;
    let mut started = started;
    let status_sources = observation_receipt::status_sources(transaction, conversation, snapshot);
    let shared = observation_receipt::request_start(
        transaction,
        conversation,
        &started,
        &contribution_emissions,
        context_receipts
            .first()
            .map(|receipt| Arc::clone(&receipt.observation)),
        snapshot,
        &status_sources,
    );
    started.observation.clone_from(&shared);
    let mut contribution_emissions = contribution_emissions;
    for emission in &mut contribution_emissions {
        emission.observation.clone_from(&shared);
    }
    Ok(ModelTurnStartCommit {
        verified_snapshot: None,
        status_sources,
        context_receipts,
        started,
        contribution_emissions,
        disposition: ModelTurnStartCommitDisposition::NewlyCommitted,
    })
}

pub(super) fn validate_fresh_input_binding(
    connection: &Connection,
    snapshot: &RequestSnapshot,
    json: &str,
    root: Option<manifest::NodeRef>,
) -> Result<(), ConversationStoreError> {
    let actual: String = connection
        .query_row(
            "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if actual != json || read_stored_snapshot_root(connection, &snapshot.request_id)? != root {
        return Err(ConversationStoreError::InvalidReference(
            "fresh request stored Snapshot/root differs from exact admitted values".into(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Native start inputs plus its transaction-local optional fragment session.
pub(super) fn index_request_read_layouts(
    transaction: &Transaction<'_>,
    scope: &ConversationId,
    snapshot: &RequestSnapshot,
    input: &crate::model::SelectedModelInput,
    selection: &manifest::Selection,
    retry_message_id: Option<&MessageId>,
    context: &[MessageBlock],
    event: &RuntimeEventEnvelope,
) {
    let _ = optional_display_layout(transaction, || {
        if let Some(retry) = retry_message_id {
            content::index_utf8_field(
                transaction,
                "response_identity",
                &format!("request-start:{}", event.sequence),
                1,
                retry.as_str(),
            )?;
        }
        Ok(())
    });
    let _ = optional_display_layout(transaction, || {
        observation_trace::request_core(transaction, event, snapshot)
    });
    let _ = optional_display_layout(transaction, || {
        transaction
            .execute(
                "UPDATE request_snapshots SET observation_source_id=?2 WHERE request_id=?1",
                params![snapshot.request_id.as_str(), event.sequence.to_string()],
            )
            .map_err(|error| storage(error.to_string()))?;
        observation_request_summary::index(transaction, snapshot)?;
        observation_request::index_selection(
            transaction,
            event.sequence,
            scope,
            input,
            snapshot,
            selection,
        )
    });
    let _ = optional_display_layout(transaction, || {
        observation_trace::index_context(transaction, event.sequence, snapshot, context)
    });
    // Catalog identity evidence is independent of optional request-body layouts.
    let _ = optional_display_layout(transaction, || {
        observation_trace::index_catalog(transaction, event.sequence, snapshot)
    });
}

/// Verifies a retried model-turn start against the already-committed durable
/// facts (Issue #12, M9b): the frozen snapshot, the exact ordered
/// request-scoped context committed atomically with it, the request-start
/// event, and the sequence binding must all match exactly.
pub(super) fn verify_committed_start_tx(
    transaction: &rusqlite::Transaction<'_>,
    json: &str,
    started_sequence: Option<i64>,
    context: &[MessageBlock],
    snapshot: &RequestSnapshot,
) -> Result<ModelTurnStartCommit, ConversationStoreError> {
    let Some(started_sequence) = started_sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} exists without a durable start sequence",
            snapshot.request_id
        )));
    };
    let mut stored: RequestSnapshot = decode(json, "request snapshot")?;
    if !stored.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "normalized frozen request metadata contains selected input".into(),
        ));
    }
    stored.input = manifest::load(
        transaction,
        read_stored_snapshot_root(transaction, &snapshot.request_id)?,
    )?;
    if stored != *snapshot {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} was started with different frozen inputs",
            snapshot.request_id
        )));
    }
    // The durable request-start authority binds the exact ordered
    // request-scoped context of that request: the retried start must carry
    // the complete ordered context the original start committed atomically
    // with it. The frozen `MessageId`s prove exact equality — an empty
    // retry, a prefix, a reorder, or an extra message all fail here — and
    // the per-message body check below rejects same-ids/different-body.
    let supplied_ids: Vec<MessageId> = context
        .iter()
        .map(crate::conversation::message_id_of)
        .collect();
    if supplied_ids != stored.request_context_ids {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} was retried with a different request-scoped context",
            snapshot.request_id
        )));
    }
    for message in context {
        let id = crate::conversation::message_id_of(message);
        let stored_message = load_message_tx(transaction, &id)?;
        if stored_message != *message {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request {} context message {id} differs from the committed fact",
                snapshot.request_id
            )));
        }
    }
    let Some(started) = find_request_start_event(transaction, &snapshot.request_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} exists without its request-start fact",
            snapshot.request_id
        )));
    };
    if seq_to_i64(started.sequence)? != started_sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} start sequence disagrees with its event",
            snapshot.request_id
        )));
    }
    validate_request_start_metadata(&stored, &started)?;
    let contribution_emissions =
        verify_contribution_start_tx(transaction, context, &stored, &started)?;
    Ok(ModelTurnStartCommit {
        verified_snapshot: None,
        status_sources: Vec::new(),
        context_receipts: Vec::new(),
        started,
        contribution_emissions,
        disposition: ModelTurnStartCommitDisposition::IdempotentReplay,
    })
}

pub(super) fn reconstruct_request_start_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
    expected_json: Option<&str>,
) -> Result<ModelRequest, ConversationStoreError> {
    let RuntimeEvent::ModelRequestStarted { request_id, .. } = &envelope.event else {
        return Err(ConversationStoreError::InvalidReference(
            "request reference validation requires a ModelRequestStarted fact".into(),
        ));
    };
    let json: Option<String> = transaction
        .query_row(
            "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("request event reference: {error}")))?;
    let Some(json) = json else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start event references missing Request Snapshot {request_id}"
        )));
    };
    if expected_json.is_some_and(|expected| expected != json) {
        return Err(ConversationStoreError::InvalidReference(
            "fresh request stored Snapshot differs from its exact admitted bytes".into(),
        ));
    }
    let snapshot: RequestSnapshot = decode(&json, "request-start snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if snapshot.request_id != *request_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start snapshot identity does not match {request_id}"
        )));
    }
    validate_request_start_metadata(&snapshot, envelope)?;
    let root = read_stored_snapshot_root(transaction, request_id)?;
    let digest: String = transaction
        .query_row(
            "SELECT CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| read_manifest_digest(row, 0),
        )
        .map_err(|error| storage(format!("request reference integrity: {error}")))?;
    if stored_snapshot_digest(&json, root) != digest || !snapshot.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "frozen request manifest integrity mismatch".into(),
        ));
    }
    let mut snapshot = snapshot;
    snapshot.input = manifest::load(transaction, root)?;
    reconstruct_snapshot_input(transaction, &envelope.conversation_id, &snapshot)
}

pub(super) fn validate_request_start_metadata(
    snapshot: &RequestSnapshot,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::ModelRequestStarted { request_id, model } = &envelope.event else {
        return Err(ConversationStoreError::InvalidReference(
            "request snapshot is paired with a non-request-start event".to_owned(),
        ));
    };
    if request_id != &snapshot.request_id
        || model != &snapshot.invocation.model
        || envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
        || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start fact does not exactly identify Request Snapshot {}",
            snapshot.request_id
        )));
    }
    Ok(())
}

/// Validates that the exact ordered request-scoped context supplied for a
/// model-turn start equals the frozen `request_context_ids` of its snapshot
/// (Issue #12, M9b).
///
/// This is the one input validation rule shared by the fresh commit and the
/// idempotent retry: the complete ordered `MessageId` set must match before
/// the store chooses a path or touches durable state, so a fresh commit can
/// never persist a snapshot that disagrees with the request-scoped context it
/// appends atomically. The frozen ids prove exact ordered equality — an empty
/// set, a prefix, a reorder, or an extra message all fail here — and the
/// retry path additionally proves per-message body equality against the
/// already-committed fact.
pub(super) fn validate_request_context(
    snapshot: &RequestSnapshot,
    context: &[MessageBlock],
) -> Result<(), ConversationStoreError> {
    let supplied_ids: Vec<MessageId> = context
        .iter()
        .map(crate::conversation::message_id_of)
        .collect();
    if supplied_ids != snapshot.request_context_ids {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} supplied a request-scoped context different from its frozen snapshot",
            snapshot.request_id
        )));
    }
    Ok(())
}

impl SqliteConversationStore {
    /// Advances the concrete Todo reminder progress coordinate for one fresh
    /// logical primary request. Overflow retries carry a different request
    /// identity but do not represent new semantic progress, so they reuse the
    /// current sequence. This value is durable only as part of the enclosing
    /// model-turn-start transaction.
    pub(super) fn advance_logical_step_progress_tx(
        transaction: &rusqlite::Transaction<'_>,
        snapshot: &RequestSnapshot,
    ) -> Result<u64, ConversationStoreError> {
        let current = read_logical_step_progress_tx(transaction)?;
        if snapshot.identity.retry_number != 0 {
            return Ok(current);
        }
        let next = current
            .checked_add(1)
            .ok_or(ConversationStoreError::SequenceExhausted)?;
        transaction
            .execute(
                "UPDATE rustx_store SET logical_step_sequence=?1 WHERE id=1",
                [seq_to_i64(next)?],
            )
            .map_err(|error| storage(format!("update Todo progress sequence: {error}")))?;
        Ok(next)
    }
    /// Inserts the fresh model-turn start facts into `transaction`: the
    /// request-scoped canonical context, the frozen snapshot, the
    /// `ModelRequestStarted` event, and the sequence binding (Issue #12,
    /// M9b).
    #[allow(clippy::too_many_lines)] // One atomic start transition, contribution settlement and native reference validation.
    pub(super) fn insert_fresh_start_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
        selection: &manifest::Selection,
        admission_epoch: u64,
    ) -> Result<crate::durable::ModelTurnStartCommit, ConversationStoreError> {
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::BeforeContextAppend) {
            return Err(storage(
                "fault injected: before request-start context append",
            ));
        }
        if !context.is_empty() {
            ensure_surface_head(transaction)?;
        }
        for message in context {
            if transcript_visible_message(message) {
                return Err(ConversationStoreError::InvalidReference(
                    "request-scoped context must use a hidden Context-kind User message".to_owned(),
                ));
            }
            append_message_and_surface(transaction, message)?;
        }
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterContextAppend) {
            return Err(storage(
                "fault injected: after request-start context append",
            ));
        }
        validate_canonical_frontier(transaction, snapshot.canonical_frontier)?;
        validate_accepted_continuation_source(transaction, &self.conversation_id, snapshot)?;
        let warm = projection::validate_warm_selection(
            transaction,
            &self.conversation_id,
            snapshot,
            &self.conn.manifest,
            admission_epoch,
        )?;
        if !warm {
            projection::validate_request_selection(transaction, &self.conversation_id, snapshot)?;
        }
        // Retry belongs to the Attempt's original adoption, independent of the
        // selected model context, media transformations or later steering.
        // next_event_sequence is the inclusive committed J frontier in this
        // same transaction, before the new RequestStarted row is appended.
        let through = journal_prefix(transaction)?;
        let retry_message_id =
            reading::attempt_prompt(transaction, &snapshot.identity.attempt_id, through)?;
        let json = encode_snapshot(snapshot)?;
        let manifest_digest = stored_snapshot_digest(&json, selection.root);
        transaction
            .execute(
                "INSERT INTO request_snapshots(request_id,canonical_frontier,snapshot_json,manifest_digest,started_sequence,retry_message_id,retry_ledger_position,previous_request_id,input_root,input_root_digest,input_count) VALUES(?1,?2,?3,?4,NULL,?5,(SELECT position FROM message_ledger WHERE message_id=?5),(SELECT request_id FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence IS NOT NULL ORDER BY started_sequence DESC LIMIT 1),?6,?7,?8)",
                params![
                    snapshot.request_id.as_str(),
                    seq_to_i64(snapshot.canonical_frontier.0)?,
                    json,
                    manifest_digest,
                    retry_message_id.as_ref().map(MessageId::as_str),
                    selection.root.map(|r|r.id),
                    selection.root.as_ref().map(|r|r.digest.as_slice()),
                    selection.root.map_or(0,|r|r.count),
                ],
            )
            .map_err(|error| storage(format!("insert request snapshot: {error}")))?;
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterSnapshotInsert) {
            return Err(storage(
                "fault injected: after request-start snapshot insert",
            ));
        }
        let event = RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: Some(snapshot.identity.attempt_id.clone()),
            turn_id: Some(snapshot.identity.turn.clone()),
            timestamp,
            event: RuntimeEvent::ModelRequestStarted {
                request_id: snapshot.request_id.clone(),
                model: snapshot.invocation.model.clone(),
            },
        };
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage(
                "fault injected: request-start event journal commit",
            ));
        }
        // Validate the original stored R and its immutable C dependencies once,
        // after insertion (including any native SQLite trigger effects).
        validate_fresh_input_binding(transaction, snapshot, &json, selection.root)?;
        let input = if warm && self.conn.manifest.epoch.load(Ordering::Acquire) == admission_epoch {
            None
        } else {
            Some(reconstruct_request_start_reference(
                transaction,
                &event,
                Some(&json),
            )?)
        };
        let validated_epoch = self.conn.manifest.epoch.load(Ordering::Acquire);
        if let Some(input) = &input {
            for association in &snapshot.tool_associations {
                let source = &association.source;
                if source.assistant.conversation_id != self.conversation_id {
                    return Err(ConversationStoreError::InvalidReference(
                        "foreign request Tool source".into(),
                    ));
                }
                let original = load_message(transaction, &source.assistant.message_id)?;
                if !source.assistant.matches(&original) {
                    return Err(ConversationStoreError::InvalidReference(
                        "changed request Tool source".into(),
                    ));
                }
                let MessageBlock::Assistant(assistant) = original else {
                    return Err(ConversationStoreError::InvalidReference(
                        "request Tool source is not Assistant".into(),
                    ));
                };
                let Some(crate::message::types::AssistantContentBlock::ToolCall(physical)) =
                    assistant.content.get(source.block.get() as usize)
                else {
                    return Err(ConversationStoreError::InvalidReference(
                        "request Tool source occurrence is missing".into(),
                    ));
                };
                let crate::model::projected::ModelMessageView::Assistant(projected) =
                    input.messages[association.call.message.item_index as usize].view()
                else {
                    unreachable!("frozen association validated");
                };
                let crate::message::types::AssistantContentBlock::ToolCall(call) =
                    &projected[association.call.block.0 as usize]
                else {
                    unreachable!("frozen association validated");
                };
                if call.tool_id != physical.tool_id {
                    return Err(ConversationStoreError::InvalidReference(
                        "request Tool source names a different physical Tool".into(),
                    ));
                }
            }
        }
        let persisted = persist_event_tx_internal(
            transaction,
            &self.conversation_id,
            event,
            false,
            Some(ValidatedRequestStart {
                transaction,
                snapshot,
            }),
        )?;
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterCompletionInsert) {
            return Err(storage("fault injected: after request-start event insert"));
        }
        transaction
            .execute(
                "UPDATE request_snapshots SET started_sequence=?1 WHERE request_id=?2",
                params![
                    seq_to_i64(persisted.event.sequence)?,
                    snapshot.request_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("bind request start sequence: {error}")))?;
        let source_input = input.as_ref().map_or_else(
            || snapshot.input.messages(),
            |r| {
                crate::model::SelectedInputManifest::from_messages(
                    &self.conversation_id,
                    &r.messages,
                )
                .messages()
            },
        );
        index_request_read_layouts(
            transaction,
            &self.conversation_id,
            snapshot,
            &source_input,
            selection,
            retry_message_id.as_ref(),
            context,
            &persisted.event,
        );
        // Todo progress is a conversation-owned semantic coordinate, not a
        // Surface append count. Advance it only for the first request of a
        // logical primary step and do so after this start's request context
        // and ModelRequestStarted fact have staged. The enclosing transaction
        // makes the resulting value the exact origin of any Todo emission
        // committed below; a failed or cancelled start rolls it back.
        Self::advance_logical_step_progress_tx(transaction, snapshot)?;
        // contribution emissions are semantic observations of the logical
        // step, not one emission per transport attempt. A transient or
        // overflow retry may carry the same status metadata in its snapshot
        // when the canonical status message remains on the frozen Surface,
        // but only the initial request owns the durable emission facts.
        let contribution_emissions = if snapshot.identity.retry_number == 0 {
            self.insert_contribution_emissions_tx(transaction, snapshot, timestamp)?
        } else {
            Vec::new()
        };
        consume_pending_carryover_tx(transaction, snapshot)?;
        let result = request_start_observation_tx(
            transaction,
            &self.conversation_id,
            context,
            snapshot,
            persisted.event,
            contribution_emissions,
        )?;
        validate_fresh_input_binding(transaction, snapshot, &json, selection.root)?;
        if self.conn.manifest.epoch.load(Ordering::Acquire) != validated_epoch {
            reconstruct_request_start_reference(transaction, &result.started, Some(&json))?;
        }
        Ok(result)
    }
    pub(super) fn insert_contribution_emissions_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        let logical_step_origin = read_logical_step_progress_tx(transaction)?;
        let mut persisted_emissions = Vec::new();
        for contribution in &snapshot.contributions {
            for emission in &contribution.emissions {
                let emission_event = RuntimeEventEnvelope {
                    tool_occurrence: None,
                    observation: None,
                    schema_version: EVENT_SCHEMA_VERSION,
                    event_id: contribution_emission_event_id(
                        &snapshot.request_id,
                        &contribution.producer,
                        emission,
                    ),
                    sequence: 0,
                    conversation_id: self.conversation_id.clone(),
                    attempt_id: Some(snapshot.identity.attempt_id.clone()),
                    turn_id: Some(snapshot.identity.turn.clone()),
                    timestamp,
                    event: RuntimeEvent::ContextContributionEmitted {
                        producer: contribution.producer.clone(),
                        request_id: snapshot.request_id.clone(),
                        message_id: contribution.message_id.clone(),
                        emission: emission.clone(),
                        logical_step_origin,
                    },
                };
                let persisted =
                    persist_event_tx_deferred(transaction, &self.conversation_id, emission_event)?;
                #[cfg(test)]
                if self.consume_request_start_fault(
                    RequestStartFaultOperation::AfterContributionEventInsert,
                ) {
                    return Err(storage(
                        "fault injected: after contribution emission event insert",
                    ));
                }
                upsert_contribution_head_tx(transaction, &persisted.event)?;
                #[cfg(test)]
                if self.consume_request_start_fault(
                    RequestStartFaultOperation::AfterContributionHeadUpsert,
                ) {
                    return Err(storage(
                        "fault injected: after contribution emission head upsert",
                    ));
                }
                persisted_emissions.push(persisted.event);
            }
        }
        Ok(persisted_emissions)
    }
}
