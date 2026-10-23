//! Atomic intermediate request settlement. The persisted delay is audit data,
//! never a substitute for the live executor's monotonic deadline authority.
use super::{
    AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX, AuxiliaryAuditCut, AuxiliaryCallId,
    AuxiliaryCallOutcome, AuxiliaryCommitConfirmation, AuxiliaryCommitDisposition,
    AuxiliaryRequestEvidence, AuxiliaryWriteFailure, Connection, ConversationId,
    ConversationStoreError, EVENT_SCHEMA_VERSION, ModelRequestOwner, OptionalExtension, RequestId,
    RequestRow, RuntimeEvent, RuntimeEventEnvelope, SqliteConversationStore, Transaction,
    bounded_encoded_len, call_row_tx, commit, event_at_connection, params, persist_event_tx,
    request_audit_at_tx, request_row_query, request_row_tx, require_audit_fact,
    same_terminal_envelope, seq_to_i64, sequence_from_i64, storage, validate_request_evidence,
    validate_terminal_envelope_shape,
};
use crate::durable::AuxiliaryRetryCommit;
use crate::durable::auxiliary::retry_event_id;

fn pair<'a>(
    conversation: &ConversationId,
    terminal: &RuntimeEventEnvelope,
    retry: &'a RuntimeEventEnvelope,
) -> Result<(&'a AuxiliaryCallId, &'a RequestId), ConversationStoreError> {
    validate_terminal_envelope_shape(terminal, conversation, false)?;
    bounded_encoded_len(
        retry,
        AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
        "auxiliary retry",
    )?;
    let RuntimeEvent::AuxiliaryRetryScheduled {
        call_id,
        request_id,
        next_ordinal,
        delay_ms,
        request_evidence,
    } = &retry.event
    else {
        return Err(storage("retry settlement requires AuxiliaryRetryScheduled"));
    };
    if retry.conversation_id != *conversation
        || retry.schema_version != EVENT_SCHEMA_VERSION
        || retry.attempt_id.is_some()
        || retry.turn_id.is_some()
        || retry.tool_occurrence.is_some()
        || retry.observation.is_some()
        || retry.event_id != retry_event_id(request_id)
        || !(1..3).contains(next_ordinal)
        || *delay_ms > 60_000
    {
        return Err(storage("invalid bounded auxiliary retry envelope"));
    }
    if !matches!(&terminal.event, RuntimeEvent::ModelRequestFailed { owner: ModelRequestOwner::Auxiliary {call_id: owner_call,ordinal}, request_id: failed, .. } if owner_call == call_id && failed == request_id && ordinal.checked_add(1) == Some(*next_ordinal))
    {
        return Err(storage("retry does not match its preceding failed request"));
    }
    validate_request_evidence(request_evidence, terminal)?;
    Ok((call_id, request_id))
}

pub(in crate::durable::sqlite) fn validate_retry_event_tx(
    transaction: &Transaction<'_>,
    retry: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::AuxiliaryRetryScheduled {
        call_id,
        request_id,
        ..
    } = &retry.event
    else {
        return Err(storage("expected retry event"));
    };
    let request =
        request_row_tx(transaction, request_id)?.ok_or_else(|| storage("retry has no request"))?;
    let terminal = request
        .terminal_sequence
        .and_then(|seq| event_at_connection(transaction, seq).transpose())
        .transpose()?
        .ok_or_else(|| storage("retry has no settled request"))?;
    pair(&retry.conversation_id, &terminal, retry)?;
    let call = call_row_tx(transaction, call_id)?.ok_or_else(|| storage("retry has no Call"))?;
    if call.settled_sequence.is_some() || request.call_id != *call_id {
        return Err(storage("retry Call is terminal or ownership disagrees"));
    }
    Ok(())
}

fn receipt(
    connection: &Connection,
    conversation: &ConversationId,
    terminal: &RuntimeEventEnvelope,
    retry: &RuntimeEventEnvelope,
) -> Result<Option<AuxiliaryRetryCommit>, ConversationStoreError> {
    let (_, request_id) = pair(conversation, terminal, retry)?;
    let sequence: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM auxiliary_retries WHERE request_id=?1",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let Some(sequence) = sequence else {
        return Ok(None);
    };
    let stored_retry = event_at_connection(connection, sequence_from_i64(sequence)?)?
        .ok_or_else(|| storage("retry index missing its Journal event"))?;
    let request = request_row_query(connection, "request_id=?1", [request_id.as_str()])?
        .ok_or_else(|| storage("retry request missing"))?;
    let stored_terminal = event_at_connection(
        connection,
        request
            .terminal_sequence
            .ok_or_else(|| storage("retry request terminal missing"))?,
    )?
    .ok_or_else(|| storage("retry request terminal fact missing"))?;
    if !same_terminal_envelope(&stored_terminal, terminal)
        || !same_terminal_envelope(&stored_retry, retry)
    {
        return Err(storage(
            "retry confirmation conflicts with exact committed payload",
        ));
    }
    require_audit_fact(
        connection,
        stored_retry.sequence,
        "retry_scheduled",
        &request.call_id,
        Some(request_id),
        Some(request.ordinal),
    )?;
    let audit = request_audit_at_tx(
        connection,
        conversation,
        request_id,
        AuxiliaryAuditCut {
            journal: stored_retry.sequence,
        },
    )?
    .ok_or_else(|| storage("retry request audit missing"))?;
    Ok(Some(AuxiliaryRetryCommit {
        disposition: AuxiliaryCommitDisposition::IdempotentReplay,
        request: audit,
        request_terminal: stored_terminal,
        retry_scheduled: stored_retry,
    }))
}

impl SqliteConversationStore {
    pub(super) fn commit_auxiliary_retry(
        &self,
        terminal: &RuntimeEventEnvelope,
        retry: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryRetryCommit, AuxiliaryWriteFailure> {
        pair(&self.conversation_id, terminal, retry).map_err(AuxiliaryWriteFailure::known)?;
        if terminal.sequence != 0 || retry.sequence != 0 {
            return Err(AuxiliaryWriteFailure::known(storage(
                "new retry settlement requires unallocated Journal sequences",
            )));
        }

        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        commit::run_auxiliary_write(&mut connection, |transaction| {
            if let Some(existing) = receipt(transaction, &self.conversation_id, terminal, retry)? {
                return Ok(existing);
            }
            persist_event_tx(transaction, &self.conversation_id, terminal.clone())?;
            persist_event_tx(transaction, &self.conversation_id, retry.clone())?;
            let mut result = receipt(transaction, &self.conversation_id, terminal, retry)?
                .ok_or_else(|| storage("committed retry receipt missing"))?;
            result.disposition = AuxiliaryCommitDisposition::NewlyCommitted;
            Ok(result)
        })
    }
    pub(super) fn confirm_auxiliary_retry(
        &self,
        terminal: &RuntimeEventEnvelope,
        retry: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliaryRetryCommit>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        Ok(
            receipt(&transaction, &self.conversation_id, terminal, retry)?.map_or(
                AuxiliaryCommitConfirmation::Unknown,
                AuxiliaryCommitConfirmation::ConfirmedCommitted,
            ),
        )
    }
}

pub(super) fn request_evidence(
    connection: &Connection,
    conversation: &ConversationId,
    request: &RequestRow,
    cut: AuxiliaryAuditCut,
    terminal: Option<&RuntimeEventEnvelope>,
) -> Result<Option<AuxiliaryRequestEvidence>, ConversationStoreError> {
    let sequence: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM auxiliary_retries WHERE request_id=?1 AND sequence<=?2",
            params![request.request_id.as_str(), seq_to_i64(cut.journal)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let Some(sequence) = sequence else {
        return Ok(None);
    };
    let retry = event_at_connection(connection, sequence_from_i64(sequence)?)?
        .ok_or_else(|| storage("retry evidence event missing"))?;
    let terminal = terminal.ok_or_else(|| storage("retry evidence has no terminal"))?;
    pair(conversation, terminal, &retry)?;
    require_audit_fact(
        connection,
        retry.sequence,
        "retry_scheduled",
        &request.call_id,
        Some(&request.request_id),
        Some(request.ordinal),
    )?;
    let RuntimeEvent::AuxiliaryRetryScheduled {
        request_evidence, ..
    } = retry.event
    else {
        unreachable!()
    };
    Ok(Some(request_evidence))
}

/// A Call-only terminal after requests is a backoff cancellation/failure, not a
/// way to discard final request evidence or fabricate accepted completion.
pub(super) fn validate_call_only(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
    outcome: AuxiliaryCallOutcome,
    settled_sequence: Option<u64>,
) -> Result<(), ConversationStoreError> {
    let last: Option<(String,u32)> = connection.query_row(
        "SELECT request_id,ordinal FROM auxiliary_requests WHERE call_id=?1 ORDER BY ordinal DESC LIMIT 1",
        [call_id.as_str()],|row|Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|error|storage(error.to_string()))?;
    let Some((request_id, ordinal)) = last else {
        return Ok(());
    };
    let scheduled:Option<u64>=connection.query_row(
        "SELECT sequence FROM auxiliary_retries WHERE call_id=?1 AND request_id=?2 AND next_ordinal=?3",
        params![call_id.as_str(),request_id,ordinal+1],|row|row.get(0),
    ).optional().map_err(|error|storage(error.to_string()))?;
    if outcome == AuxiliaryCallOutcome::Completed
        || scheduled
            .is_none_or(|scheduled| settled_sequence.is_some_and(|terminal| scheduled >= terminal))
    {
        return Err(storage(
            "Call-only terminal requires a preceding committed retry and a non-success outcome",
        ));
    }
    Ok(())
}
