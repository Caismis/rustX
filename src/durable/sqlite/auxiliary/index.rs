//! Request-owner and audit-fact index writes performed inside the Journal append transaction.

use super::{
    AUXILIARY_REQUEST_ID_BYTES_MAX, AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX, AuxiliaryAuditCut,
    AuxiliaryCallId, ConversationId, ConversationStoreError, ModelRequestOwner, RequestId,
    RuntimeEvent, RuntimeEventEnvelope, Transaction, bounded_encoded_len, call_row_tx,
    journal_prefix, outcome_name, owner_row_tx, params, request_audit_at_tx, request_row_tx,
    request_snapshot_started_sequence, seq_to_i64, storage, terminal_kind, validate_owner_row_tx,
};

pub(in crate::durable::sqlite) fn load_request_owner_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
) -> Result<Option<ModelRequestOwner>, ConversationStoreError> {
    if let Some(indexed) = owner_row_tx(transaction, request_id)? {
        match &indexed.owner {
            ModelRequestOwner::Agent { .. } => {
                // Preserve the primary native Snapshot validator and its indexed
                // duplicate-fact probe; auxiliary encoded limits do not apply here.
                if crate::durable::sqlite::request::snapshot::find_request_start_event(
                    transaction,
                    request_id,
                )?
                .is_none()
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "request {request_id} has Journal evidence without its request-start owner"
                    )));
                }
                let (_, started) =
                    crate::durable::sqlite::request::snapshot::verified_started_request_tx(
                        transaction,
                        conversation_id,
                        request_id,
                    )?;
                if started.sequence != indexed.start_sequence
                    || started.event.model_request_owner() != Some(&indexed.owner)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "Agent request {request_id} owner start disagrees with its Snapshot index"
                    )));
                }
                let terminal =
                    crate::durable::sqlite::request::snapshot::find_request_outcome_event(
                        transaction,
                        request_id,
                    )?;
                if terminal.as_ref().map(|event| event.sequence) != indexed.terminal_sequence
                    || terminal
                        .as_ref()
                        .and_then(|event| terminal_kind(&event.event))
                        != indexed.terminal_kind
                    || terminal.as_ref().is_some_and(|event| {
                        event.event.model_request_owner() != Some(&indexed.owner)
                    })
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "Agent request {request_id} owner terminal disagrees with its Journal fact"
                    )));
                }
            }
            ModelRequestOwner::Auxiliary { call_id, ordinal } => {
                validate_owner_row_tx(transaction, conversation_id, request_id, &indexed)?;
                let request = request_row_tx(transaction, request_id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} has an owner but no request index"
                    ))
                })?;
                let call = call_row_tx(transaction, call_id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} has no Call authority"
                    ))
                })?;
                if request.call_id != *call_id
                    || request.ordinal != *ordinal
                    || request.started_sequence != indexed.start_sequence
                    || call.started_sequence >= request.started_sequence
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} indexes disagree with its authority"
                    )));
                }
                let _ = request_audit_at_tx(
                    transaction,
                    conversation_id,
                    request_id,
                    AuxiliaryAuditCut {
                        journal: journal_prefix(transaction)?,
                    },
                )?;
            }
        }
        return Ok(Some(indexed.owner));
    }
    if crate::durable::sqlite::request::snapshot::has_request_journal_fact_tx(
        transaction,
        request_id,
    )? || request_snapshot_started_sequence(transaction, request_id)?.is_some()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} has durable evidence without its request-start owner"
        )));
    }
    Ok(None)
}

pub(in crate::durable::sqlite) fn index_event_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &event.event {
        RuntimeEvent::ModelRequestStarted {
            owner, request_id, ..
        } => {
            insert_owner_tx(transaction, request_id, owner, event.sequence)?;
        }
        RuntimeEvent::ModelRequestCompleted {
            owner, request_id, ..
        }
        | RuntimeEvent::ModelRequestFailed {
            owner, request_id, ..
        } => {
            index_terminal_tx(transaction, event, request_id, owner)?;
        }
        RuntimeEvent::AuxiliaryCallStarted { call_id, .. } => {
            bounded_encoded_len(
                event,
                AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
                "auxiliary Call start event",
            )?;
            transaction
                .execute(
                    "INSERT INTO auxiliary_calls(
                        call_id,started_sequence,settled_sequence,outcome
                     ) VALUES(?1,?2,NULL,NULL)",
                    params![call_id.as_str(), seq_to_i64(event.sequence)?],
                )
                .map_err(|error| storage(format!("index auxiliary Call start: {error}")))?;
            insert_audit_fact_tx(
                transaction,
                event.sequence,
                "call_started",
                call_id,
                None,
                None,
            )?;
        }
        RuntimeEvent::AuxiliaryCallSettled {
            call_id, outcome, ..
        } => {
            let updated = transaction
                .execute(
                    "UPDATE auxiliary_calls SET settled_sequence=?1,outcome=?2
                     WHERE call_id=?3 AND settled_sequence IS NULL",
                    params![
                        seq_to_i64(event.sequence)?,
                        outcome_name(*outcome),
                        call_id.as_str(),
                    ],
                )
                .map_err(|error| storage(format!("index auxiliary Call settlement: {error}")))?;
            if updated != 1 {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "auxiliary Call {call_id} already has a settlement"
                )));
            }
            insert_audit_fact_tx(
                transaction,
                event.sequence,
                "call_settled",
                call_id,
                None,
                None,
            )?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn insert_owner_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
    owner: &ModelRequestOwner,
    start_sequence: u64,
) -> Result<(), ConversationStoreError> {
    match owner {
        ModelRequestOwner::Agent { identity } => {
            transaction
                .execute(
                    "INSERT INTO request_owners(
                        request_id,owner_kind,attempt_id,turn_id,retry_number,
                        call_id,auxiliary_ordinal,start_sequence,terminal_sequence,terminal_kind
                     ) VALUES(?1,'agent',?2,?3,?4,NULL,NULL,?5,NULL,NULL)",
                    params![
                        request_id.as_str(),
                        identity.attempt_id.as_str(),
                        identity.turn.as_str(),
                        i64::from(identity.retry_number),
                        seq_to_i64(start_sequence)?,
                    ],
                )
                .map_err(|error| storage(format!("index Agent request owner: {error}")))?;
        }
        ModelRequestOwner::Auxiliary { call_id, ordinal } => {
            if request_id.as_str().is_empty()
                || request_id.as_str().len() > AUXILIARY_REQUEST_ID_BYTES_MAX
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary RequestId must contain 1..={AUXILIARY_REQUEST_ID_BYTES_MAX} bytes"
                )));
            }
            transaction
                .execute(
                    "INSERT INTO request_owners(
                        request_id,owner_kind,attempt_id,turn_id,retry_number,
                        call_id,auxiliary_ordinal,start_sequence,terminal_sequence,terminal_kind
                     ) VALUES(?1,'auxiliary',NULL,NULL,NULL,?2,?3,?4,NULL,NULL)",
                    params![
                        request_id.as_str(),
                        call_id.as_str(),
                        i64::from(*ordinal),
                        seq_to_i64(start_sequence)?,
                    ],
                )
                .map_err(|error| storage(format!("index auxiliary request owner: {error}")))?;
        }
    }
    Ok(())
}

pub(super) fn index_terminal_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
    request_id: &RequestId,
    owner: &ModelRequestOwner,
) -> Result<(), ConversationStoreError> {
    let kind = terminal_kind(&event.event).expect("terminal shape checked");
    let indexed = owner_row_tx(transaction, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "request {request_id} terminal has no authoritative owner"
        ))
    })?;
    if indexed.owner != *owner || indexed.terminal_sequence.is_some() {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "request {request_id} terminal conflicts with its authoritative owner state"
        )));
    }
    let updated = transaction
        .execute(
            "UPDATE request_owners
             SET terminal_sequence=?1,terminal_kind=?2
             WHERE request_id=?3 AND terminal_sequence IS NULL",
            params![seq_to_i64(event.sequence)?, kind, request_id.as_str(),],
        )
        .map_err(|error| storage(format!("index request terminal: {error}")))?;
    if updated != 1 {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "request {request_id} already has a terminal"
        )));
    }
    if let ModelRequestOwner::Auxiliary { call_id, ordinal } = owner {
        let request = request_row_tx(transaction, request_id)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "auxiliary request {request_id} terminal has no request index"
            ))
        })?;
        if request.call_id != *call_id || request.ordinal != *ordinal {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary request {request_id} terminal disagrees with its request index"
            )));
        }
        let updated = transaction
            .execute(
                "UPDATE auxiliary_requests
                 SET terminal_sequence=?1,terminal_kind=?2
                 WHERE request_id=?3 AND terminal_sequence IS NULL",
                params![seq_to_i64(event.sequence)?, kind, request_id.as_str(),],
            )
            .map_err(|error| storage(format!("index auxiliary request terminal: {error}")))?;
        if updated != 1 {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "auxiliary request {request_id} already has a terminal"
            )));
        }
        insert_audit_fact_tx(
            transaction,
            event.sequence,
            "request_settled",
            call_id,
            Some(request_id),
            Some(*ordinal),
        )?;
    }
    Ok(())
}

pub(super) fn next_audit_offset(
    transaction: &Transaction<'_>,
) -> Result<u64, ConversationStoreError> {
    let maximum: i64 = transaction
        .query_row(
            "SELECT coalesce(max(audit_offset),0) FROM auxiliary_audit_facts
             INDEXED BY auxiliary_audit_offset_idx",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("auxiliary audit frontier lookup: {error}")))?;
    let maximum =
        u64::try_from(maximum).map_err(|_| storage("auxiliary audit frontier is negative"))?;
    maximum
        .checked_add(1)
        .ok_or_else(|| storage("auxiliary audit offset exhausted"))
}

pub(super) fn insert_audit_fact_tx(
    transaction: &Transaction<'_>,
    sequence: u64,
    kind: &str,
    call_id: &AuxiliaryCallId,
    request_id: Option<&RequestId>,
    ordinal: Option<u32>,
) -> Result<u64, ConversationStoreError> {
    let audit_offset = next_audit_offset(transaction)?;
    transaction
        .execute(
            "INSERT INTO auxiliary_audit_facts(
                sequence,audit_offset,kind,call_id,request_id,ordinal
             ) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                seq_to_i64(sequence)?,
                i64::try_from(audit_offset)
                    .map_err(|_| storage("auxiliary audit offset overflow"))?,
                kind,
                call_id.as_str(),
                request_id.map(RequestId::as_str),
                ordinal.map(i64::from),
            ],
        )
        .map_err(|error| storage(format!("insert auxiliary audit fact: {error}")))?;
    Ok(audit_offset)
}
