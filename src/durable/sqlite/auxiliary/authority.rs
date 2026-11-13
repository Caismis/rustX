//! Ledger authority and Journal-event validation of auxiliary facts inside the caller's transaction.

use super::super::sqlite_error;

use super::{
    AuxiliaryAuditCut, AuxiliaryBusinessSource, AuxiliaryCallStart, AuxiliaryInputEvidence,
    AuxiliaryRequestStart, CompactionOperationId, Connection, ConversationId,
    ConversationStoreError, MessageId, ModelRequestOwner, ResourceLimits, RuntimeEvent,
    RuntimeEventEnvelope, Transaction, call_audit_at_tx, call_row_tx, call_started_event_id,
    journal_prefix, next_request_ordinal_tx, open_request_tx, owner_row_tx, params, request_row_tx,
    request_snapshot_exists, request_started_event_id, seq_to_i64, validate_call_start_input,
    validate_request_start_input, validate_terminal_envelope_shape,
};

pub(super) fn ledger_frontier(connection: &Connection) -> Result<u64, ConversationStoreError> {
    let value: i64 = connection
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary Ledger frontier: {error}")))?;
    u64::try_from(value)
        .map_err(|_| ConversationStoreError::InvalidReference("Ledger frontier is negative".into()))
}

pub(super) fn require_message_in_ledger(
    connection: &Connection,
    message_id: &MessageId,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM message_ledger
             WHERE message_id=?1 AND position<=?2)",
            params![message_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary MessageId lookup: {error}")))?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary source MessageId {message_id} is not visible at its historical input cut"
        )));
    }
    Ok(())
}

pub(super) fn require_human_message_in_ledger(
    connection: &Connection,
    message_id: &MessageId,
    through: u64,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM message_ledger
               WHERE message_id=?1 AND position<=?2
                 AND json_extract(message_json,'$.role')='user'
                 AND json_extract(message_json,'$.source')='human'
             )",
            params![message_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary HumanMessage lookup: {error}")))?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary HumanMessage source {message_id} is not a visible canonical Human message"
        )));
    }
    Ok(())
}

pub(super) fn require_compaction_operation_through(
    connection: &Connection,
    operation_id: &CompactionOperationId,
    through_journal: u64,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_compaction_operation_kind_idx
               WHERE json_extract(event_json,'$.event.operation_id')=?1
                 AND json_extract(event_json,'$.event.type') IN
                     ('compaction_started','compaction_completed','compaction_failed')
                 AND sequence<=?2
             )",
            params![operation_id.as_str(), seq_to_i64(through_journal)?],
            |row| row.get(0),
        )
        .map_err(|error| {
            sqlite_error(
                &error,
                format!("auxiliary Compaction source lookup: {error}"),
            )
        })?;
    if !exists {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary Compaction source {operation_id} is not visible at the business-source validation frontier"
        )));
    }
    Ok(())
}

fn validate_preceding_call(
    connection: &Connection,
    conversation: &ConversationId,
    operation: &CompactionOperationId,
    preceding: Option<&crate::model::auxiliary::AuxiliaryCallRef>,
    through_journal: u64,
) -> Result<(), ConversationStoreError> {
    use rusqlite::OptionalExtension;
    let latest: Option<String> = connection.query_row(
        "SELECT json_extract(event_json,'$.event.call_id') FROM events INDEXED BY events_auxiliary_compaction_kind_idx WHERE json_extract(event_json,'$.event.business_source.operation_id')=?1 AND json_extract(event_json,'$.event.type')='auxiliary_call_started' AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",
        params![operation.as_str(), seq_to_i64(through_journal)?], |row| row.get(0),
    ).optional().map_err(|error| sqlite_error(&error, error.to_string()))?;
    match (latest, preceding) {
        (None, None) => Ok(()),
        (Some(latest), Some(reference))
            if latest == reference.call_id.as_str()
                && reference.conversation_id == *conversation
                && reference.through_journal > 0
                && reference.through_journal <= through_journal =>
        {
            let settled: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM auxiliary_calls WHERE call_id=?1 AND settled_sequence IS NOT NULL AND settled_sequence<=?2)",
                params![reference.call_id.as_str(), seq_to_i64(reference.through_journal)?], |row| row.get(0),
            ).map_err(|error| sqlite_error(&error, error.to_string()))?;
            if settled {
                Ok(())
            } else {
                Err(ConversationStoreError::InvalidReference(
                    "preceding Compaction Call is not settled at its declared cut".into(),
                ))
            }
        }
        _ => Err(ConversationStoreError::InvalidReference(
            "preceding Compaction Call must name the latest settled Call of this operation".into(),
        )),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Validate the complete frozen Call authority at its admission frontier"
)]
pub(super) fn validate_call_start_authority_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    start: &AuxiliaryCallStart,
    through_journal: u64,
) -> Result<(), ConversationStoreError> {
    validate_call_start_input(start)?;
    match &start.input {
        AuxiliaryInputEvidence::Historical { cut }
        | AuxiliaryInputEvidence::Compaction { cut, .. } => {
            #[cfg(test)]
            let _probe = super::super::start_probe::activate(
                start.call_id.as_str(),
                super::super::AuxiliaryStartProbe::HistoricalCut,
            );
            if cut.conversation_id != *conversation_id {
                return Err(ConversationStoreError::ConversationIdMismatch {
                    stored: conversation_id.clone(),
                    requested: cut.conversation_id.clone(),
                });
            }
            if cut.journal.0 > through_journal {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary historical input cut is after its Call admission frontier".into(),
                ));
            }
            crate::durable::sqlite::compaction_resources::validate(
                connection,
                conversation_id,
                cut,
                ResourceLimits::default(),
            )?;
            for message_id in &start.selected_message_ids {
                require_message_in_ledger(connection, message_id, cut.ledger.0)?;
            }
        }
        AuxiliaryInputEvidence::Temporary => {
            if !start.selected_message_ids.is_empty() {
                return Err(ConversationStoreError::InvalidReference(
                    "temporary auxiliary input cannot name selected historical MessageIds".into(),
                ));
            }
        }
    }
    if let AuxiliaryInputEvidence::Compaction {
        cut,
        previous_summary: Some(previous),
        ..
    } = &start.input
    {
        let AuxiliaryBusinessSource::Compaction { operation_id, .. } = &start.business_source
        else {
            return Err(ConversationStoreError::InvalidReference(
                "compaction input requires a Compaction owner".into(),
            ));
        };
        super::super::projection::auxiliary_lineage::validate_previous_summary(
            connection,
            conversation_id,
            &start.call_id,
            operation_id,
            previous,
            cut,
            through_journal,
        )?;
    }
    // Business causality and input selection have independent coordinates.
    // A newly admitted operation may intentionally consume an older input cut.
    match &start.business_source {
        AuxiliaryBusinessSource::Independent | AuxiliaryBusinessSource::Business { .. } => {}
        AuxiliaryBusinessSource::Submission { identity } => {
            if identity.encoding != 1 || identity.correlation.as_str().len() > 128 {
                return Err(ConversationStoreError::InvalidReference(
                    "unsupported auxiliary submission encoding or correlation".into(),
                ));
            }
        }
        AuxiliaryBusinessSource::Compaction {
            operation_id,
            preceding_call,
        } => {
            #[cfg(test)]
            let _probe = super::super::start_probe::activate(
                start.call_id.as_str(),
                super::super::AuxiliaryStartProbe::CompactionFrontier,
            );
            require_compaction_operation_through(connection, operation_id, through_journal)?;
            validate_preceding_call(
                connection,
                conversation_id,
                operation_id,
                preceding_call.as_ref(),
                through_journal,
            )?;
        }
        AuxiliaryBusinessSource::HumanMessage { message_id } => {
            require_human_message_in_ledger(connection, message_id, ledger_frontier(connection)?)?;
        }
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn validate_call_event_tx(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &envelope.event {
        RuntimeEvent::AuxiliaryCallStarted {
            call_id,
            purpose,
            trigger,
            business_source,
            input,
            semantic_input_fingerprint,
            invocation_fingerprint,
            selected_message_ids,
        } => {
            if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call facts must not carry an Attempt or Turn envelope".into(),
                ));
            }
            if envelope.event_id != call_started_event_id(call_id) {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call start has a noncanonical event identity".into(),
                ));
            }
            let start = AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose: purpose.clone(),
                trigger: trigger.clone(),
                business_source: business_source.clone(),
                input: input.clone(),
                semantic_input_fingerprint: semantic_input_fingerprint.clone(),
                invocation_fingerprint: invocation_fingerprint.clone(),
                selected_message_ids: selected_message_ids.clone(),
            };
            validate_call_start_authority_tx(
                transaction,
                &envelope.conversation_id,
                &start,
                journal_prefix(transaction)?,
            )?;
        }
        RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            outcome,
            request_evidence,
        } => {
            if request_evidence.is_none() {
                super::retry::validate_call_only(transaction, call_id, *outcome, None)?;
            }
            validate_terminal_envelope_shape(envelope, &envelope.conversation_id, true)?;
            let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} settled without a start"
                ))
            })?;
            if call.settled_sequence.is_some() {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "auxiliary Call {call_id} is already terminal"
                )));
            }
            if open_request_tx(transaction, call_id)?.is_some() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} cannot settle while a request is open"
                )));
            }
        }
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "Call validation received a non-Call fact".into(),
            ));
        }
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn validate_request_start_event_tx(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request start must not carry an Attempt or Turn envelope".into(),
        ));
    }
    let RuntimeEvent::ModelRequestStarted {
        owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
        request_id,
        model,
    } = &envelope.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request validation requires an auxiliary-owned start".into(),
        ));
    };
    let start = AuxiliaryRequestStart {
        call_id: call_id.clone(),
        ordinal: *ordinal,
        request_id: request_id.clone(),
        model: model.clone(),
    };
    validate_request_start_input(&start)?;
    if envelope.event_id != request_started_event_id(request_id) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request start has a noncanonical event identity".into(),
        ));
    }
    let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} requires active Call {call_id}"
        ))
    })?;
    let _ = call_audit_at_tx(
        transaction,
        &envelope.conversation_id,
        call_id,
        AuxiliaryAuditCut {
            journal: call.started_sequence,
        },
    )?
    .ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} has no authoritative Call start"
        ))
    })?;
    if call.settled_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "auxiliary Call {call_id} is terminal"
        )));
    }
    if let Some(open) = open_request_tx(transaction, call_id)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary Call {call_id} already has open request {open}"
        )));
    }
    let expected = next_request_ordinal_tx(transaction, call_id)?;
    if *ordinal != expected {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request ordinal {ordinal} is not the required next ordinal {expected}"
        )));
    }
    if *ordinal >= 3 {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call confirmed-start limit is three".into(),
        ));
    }
    if *ordinal > 0 {
        let scheduled: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM auxiliary_retries WHERE call_id=?1 AND next_ordinal=?2)",
            params![call_id.as_str(), ordinal], |row| row.get(0),
        ).map_err(|error| sqlite_error(&error, error.to_string()))?;
        if !scheduled {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary next request lacks committed retry authorization".into(),
            ));
        }
    }
    super::title::validate_start(transaction, call_id)?;
    if request_snapshot_exists(transaction, request_id)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} conflicts with a primary Request Snapshot"
        )));
    }
    Ok(())
}

pub(in crate::durable::sqlite) fn validate_request_terminal_event_tx(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    validate_terminal_envelope_shape(envelope, &envelope.conversation_id, true)?;
    let (RuntimeEvent::ModelRequestCompleted {
        owner, request_id, ..
    }
    | RuntimeEvent::ModelRequestFailed {
        owner, request_id, ..
    }) = &envelope.event
    else {
        unreachable!("terminal validation received another fact")
    };
    let ModelRequestOwner::Auxiliary { call_id, ordinal } = owner else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary terminal validation received an Agent owner".into(),
        ));
    };
    let request = request_row_tx(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request terminal names unstarted request {request_id}"
        ))
    })?;
    if &request.call_id != call_id || request.ordinal != *ordinal {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request terminal for {request_id} disagrees with its authoritative owner"
        )));
    }
    if request.terminal_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "auxiliary request {request_id} already has a terminal"
        )));
    }
    let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} has no Call start"
        ))
    })?;
    if call.settled_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "auxiliary Call {call_id} is already terminal"
        )));
    }
    let indexed = owner_row_tx(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} has no owner index"
        ))
    })?;
    if indexed.owner != *owner
        || indexed.start_sequence != request.started_sequence
        || indexed.terminal_sequence.is_some()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request terminal for {request_id} disagrees with its owner index"
        )));
    }
    Ok(())
}
