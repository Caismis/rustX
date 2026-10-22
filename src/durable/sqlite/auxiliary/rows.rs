//! Typed auxiliary index rows and their bounded queries.

use super::super::sqlite_error;

use super::{
    AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX, AuxiliaryCallId, AuxiliaryCallOutcome,
    AuxiliaryRequestTerminal, Connection, ConversationId, ConversationStoreError,
    ModelRequestOwner, OptionalExtension, RequestId, RuntimeEvent, RuntimeEventEnvelope,
    Transaction, call_settled_event_id, call_started_event_id, decode, request_started_event_id,
    request_terminal_event_id, seq_to_i64, sequence_from_i64, storage, terminal_kind,
    validate_terminal_envelope_shape,
};

#[derive(Debug)]
pub(super) struct CallRow {
    pub(super) started_sequence: u64,
    pub(super) settled_sequence: Option<u64>,
    pub(super) outcome: Option<AuxiliaryCallOutcome>,
}

pub(super) fn call_row_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<Option<CallRow>, ConversationStoreError> {
    call_row_query(transaction, call_id)
}

pub(super) fn call_row_query(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
) -> Result<Option<CallRow>, ConversationStoreError> {
    let row: Option<(i64, Option<i64>, Option<String>)> = connection
        .query_row(
            "SELECT started_sequence,settled_sequence,outcome
             FROM auxiliary_calls WHERE call_id=?1",
            [call_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("auxiliary Call lookup: {error}")))?;
    row.map(|(started_sequence, settled_sequence, outcome)| {
        Ok(CallRow {
            started_sequence: sequence_from_i64(started_sequence)?,
            settled_sequence: settled_sequence.map(sequence_from_i64).transpose()?,
            outcome: outcome
                .map(|value| decode(&format!("\"{value}\""), "auxiliary Call outcome"))
                .transpose()?,
        })
    })
    .transpose()
}

#[derive(Debug)]
pub(super) struct RequestRow {
    pub(super) call_id: AuxiliaryCallId,
    pub(super) ordinal: u32,
    pub(super) request_id: RequestId,
    pub(super) model: String,
    pub(super) started_sequence: u64,
    pub(super) terminal_sequence: Option<u64>,
    pub(super) terminal_kind: Option<AuxiliaryRequestTerminal>,
}

pub(super) fn request_row_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RequestRow>, ConversationStoreError> {
    request_row_query(transaction, "request_id=?1", [request_id.as_str()])
}

pub(super) fn request_row_query<P: rusqlite::Params>(
    connection: &Connection,
    predicate: &str,
    parameters: P,
) -> Result<Option<RequestRow>, ConversationStoreError> {
    let row: Option<(
        String,
        i64,
        String,
        String,
        i64,
        Option<i64>,
        Option<String>,
    )> = connection
        .query_row(
            &format!(
                "SELECT call_id,ordinal,request_id,model,started_sequence,
                        terminal_sequence,terminal_kind
                 FROM auxiliary_requests WHERE {predicate}"
            ),
            parameters,
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("auxiliary request lookup: {error}")))?;
    row.map(
        |(
            call_id,
            ordinal,
            request_id,
            model,
            started_sequence,
            terminal_sequence,
            terminal_kind,
        )| {
            Ok(RequestRow {
                call_id: AuxiliaryCallId::parse(call_id)
                    .map_err(ConversationStoreError::InvalidReference)?,
                ordinal: u32::try_from(ordinal).map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary request ordinal is out of range".into(),
                    )
                })?,
                request_id: RequestId::new(request_id),
                model,
                started_sequence: sequence_from_i64(started_sequence)?,
                terminal_sequence: terminal_sequence.map(sequence_from_i64).transpose()?,
                terminal_kind: terminal_kind
                    .map(|value| decode(&format!("\"{value}\""), "auxiliary request terminal kind"))
                    .transpose()?,
            })
        },
    )
    .transpose()
}

pub(super) fn open_request_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<Option<RequestId>, ConversationStoreError> {
    open_request_query(transaction, call_id)
}

pub(super) fn open_request_query(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
) -> Result<Option<RequestId>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT request_id FROM auxiliary_requests
             WHERE call_id=?1 AND terminal_sequence IS NULL",
            [call_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|value| value.map(RequestId::new))
        .map_err(|error| sqlite_error(&error, format!("auxiliary open request lookup: {error}")))
}

pub(super) fn call_has_requests_query(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM auxiliary_requests WHERE call_id=?1
             )",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| {
            sqlite_error(
                &error,
                format!("auxiliary request-presence lookup: {error}"),
            )
        })
}

pub(super) fn next_request_ordinal_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<u32, ConversationStoreError> {
    let maximum: Option<i64> = transaction
        .query_row(
            "SELECT max(ordinal) FROM auxiliary_requests WHERE call_id=?1",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary ordinal lookup: {error}")))?;
    maximum.map_or(Ok(0), |value| {
        u32::try_from(value)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary request ordinals are exhausted".into(),
                )
            })
    })
}

pub(super) fn request_snapshot_exists(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM request_snapshots WHERE request_id=?1)",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("primary request lookup: {error}")))
}

#[derive(Debug)]
pub(super) struct OwnerRow {
    pub(super) owner: ModelRequestOwner,
    pub(super) start_sequence: u64,
    pub(super) terminal_sequence: Option<u64>,
    pub(super) terminal_kind: Option<&'static str>,
}

pub(super) fn owner_row_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<OwnerRow>, ConversationStoreError> {
    owner_row_query(transaction, request_id)
}

pub(super) fn owner_row_query(
    transaction: &Connection,
    request_id: &RequestId,
) -> Result<Option<OwnerRow>, ConversationStoreError> {
    let row: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        i64,
        Option<i64>,
        Option<String>,
    )> = transaction
        .query_row(
            "SELECT owner_kind,attempt_id,turn_id,retry_number,call_id,
                    auxiliary_ordinal,start_sequence,terminal_sequence,terminal_kind
             FROM request_owners WHERE request_id=?1",
            [request_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("request owner index lookup: {error}")))?;
    row.map(
        |(
            kind,
            attempt_id,
            turn_id,
            retry_number,
            call_id,
            ordinal,
            start_sequence,
            terminal_sequence,
            terminal_kind,
        )| {
            let owner = match kind.as_str() {
                "agent" => ModelRequestOwner::Agent {
                    identity: crate::model::snapshot::RequestIdentity {
                        attempt_id: crate::runtime::identity::AttemptId::new(
                            attempt_id.ok_or_else(|| {
                                ConversationStoreError::InvalidReference(
                                    "Agent owner index lacks AttemptId".into(),
                                )
                            })?,
                        ),
                        turn: crate::runtime::identity::TurnId::new(turn_id.ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "Agent owner index lacks TurnId".into(),
                            )
                        })?),
                        retry_number: u32::try_from(retry_number.ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "Agent owner index lacks retry ordinal".into(),
                            )
                        })?)
                        .map_err(|_| {
                            ConversationStoreError::InvalidReference(
                                "Agent owner retry ordinal is out of range".into(),
                            )
                        })?,
                    },
                },
                "auxiliary" => ModelRequestOwner::Auxiliary {
                    call_id: AuxiliaryCallId::parse(call_id.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary owner index lacks CallId".into(),
                        )
                    })?)
                    .map_err(ConversationStoreError::InvalidReference)?,
                    ordinal: u32::try_from(ordinal.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary owner index lacks request ordinal".into(),
                        )
                    })?)
                    .map_err(|_| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary owner ordinal is out of range".into(),
                        )
                    })?,
                },
                _ => {
                    return Err(ConversationStoreError::InvalidReference(
                        "request owner index has an unknown owner kind".into(),
                    ));
                }
            };
            let terminal_kind = terminal_kind
                .map(|value| match value.as_str() {
                    "completed" => Ok("completed"),
                    "failed" => Ok("failed"),
                    _ => Err(ConversationStoreError::InvalidReference(
                        "request owner index has an unknown terminal kind".into(),
                    )),
                })
                .transpose()?;
            Ok(OwnerRow {
                owner,
                start_sequence: sequence_from_i64(start_sequence)?,
                terminal_sequence: terminal_sequence.map(sequence_from_i64).transpose()?,
                terminal_kind,
            })
        },
    )
    .transpose()
}

pub(super) fn event_at_connection(
    connection: &Connection,
    sequence: u64,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let row: Option<(i64, String, i64, String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT length(CAST(event_json AS BLOB)),
                    event_id,schema_version,conversation_id,attempt_id,turn_id
             FROM events WHERE sequence=?1",
            [seq_to_i64(sequence)?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("auxiliary event lookup: {error}")))?;
    let Some((
        encoded_bytes,
        stored_event_id,
        stored_schema_version,
        stored_conversation_id,
        stored_attempt_id,
        stored_turn_id,
    )) = row
    else {
        return Ok(None);
    };
    if u64::try_from(encoded_bytes)
        .map_err(|_| storage("auxiliary event encoded length is negative"))?
        > AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX as u64
    {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary source event exceeds its encoded byte bound".into(),
        ));
    }
    let json: String = connection
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [seq_to_i64(sequence)?],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary bounded event read: {error}")))?;
    if json.len() > AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary source event exceeds its encoded byte bound".into(),
        ));
    }
    let event: RuntimeEventEnvelope = decode(&json, "auxiliary event")?;
    if event.sequence != sequence
        || event.event_id.as_str() != stored_event_id.as_str()
        || i64::from(event.schema_version) != stored_schema_version
        || event.conversation_id.as_str() != stored_conversation_id.as_str()
        || event
            .attempt_id
            .as_ref()
            .map(crate::runtime::identity::AttemptId::as_str)
            != stored_attempt_id.as_deref()
        || event
            .turn_id
            .as_ref()
            .map(crate::runtime::identity::TurnId::as_str)
            != stored_turn_id.as_deref()
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary event payload disagrees with its Journal row".into(),
        ));
    }
    Ok(Some(event))
}

#[derive(Debug)]
pub(super) struct AuditFactRow {
    pub(super) sequence: u64,
    pub(super) audit_offset: u64,
    pub(super) kind: String,
    pub(super) call_id: AuxiliaryCallId,
    pub(super) request_id: Option<RequestId>,
    pub(super) ordinal: Option<u32>,
}

pub(super) fn audit_fact_row(
    connection: &Connection,
    sequence: u64,
) -> Result<Option<AuditFactRow>, ConversationStoreError> {
    let row: Option<(i64, i64, String, String, Option<String>, Option<i64>)> = connection
        .query_row(
            "SELECT sequence,audit_offset,kind,call_id,request_id,ordinal
             FROM auxiliary_audit_facts WHERE sequence=?1",
            [seq_to_i64(sequence)?],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("auxiliary audit fact lookup: {error}")))?;
    row.map(
        |(sequence, audit_offset, kind, call_id, request_id, ordinal)| {
            Ok(AuditFactRow {
                sequence: sequence_from_i64(sequence)?,
                audit_offset: u64::try_from(audit_offset).map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary audit offset is negative".into(),
                    )
                })?,
                kind,
                call_id: AuxiliaryCallId::parse(call_id)
                    .map_err(ConversationStoreError::InvalidReference)?,
                request_id: request_id.map(RequestId::new),
                ordinal: ordinal
                    .map(|value| {
                        u32::try_from(value).map_err(|_| {
                            ConversationStoreError::InvalidReference(
                                "auxiliary audit ordinal is out of range".into(),
                            )
                        })
                    })
                    .transpose()?,
            })
        },
    )
    .transpose()
}

pub(super) fn validate_audit_fact_event(
    event: &RuntimeEventEnvelope,
    row: &AuditFactRow,
) -> Result<(), ConversationStoreError> {
    if event.sequence != row.sequence {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary audit sequence disagrees with its Journal fact".into(),
        ));
    }
    match (&event.event, row.kind.as_str()) {
        (
            RuntimeEvent::AuxiliaryCallStarted { call_id, .. }
            | RuntimeEvent::AuxiliaryCallSettled { call_id, .. },
            "call_started" | "call_settled",
        ) => {
            let expected_kind = if matches!(event.event, RuntimeEvent::AuxiliaryCallStarted { .. })
            {
                "call_started"
            } else {
                "call_settled"
            };
            if row.kind != expected_kind
                || &row.call_id != call_id
                || row.request_id.is_some()
                || row.ordinal.is_some()
                || event.event_id
                    != if expected_kind == "call_started" {
                        call_started_event_id(call_id)
                    } else {
                        call_settled_event_id(call_id)
                    }
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call audit payload disagrees with its Journal fact".into(),
                ));
            }
        }
        (
            RuntimeEvent::ModelRequestStarted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                ..
            },
            "request_started",
        ) => {
            if &row.call_id != call_id
                || row.request_id.as_ref() != Some(request_id)
                || row.ordinal != Some(*ordinal)
                || event.event_id != request_started_event_id(request_id)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request-start audit payload disagrees with its Journal fact".into(),
                ));
            }
        }
        (
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                ..
            }
            | RuntimeEvent::ModelRequestFailed {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                ..
            },
            "request_settled",
        ) => {
            let completed = matches!(event.event, RuntimeEvent::ModelRequestCompleted { .. });
            if &row.call_id != call_id
                || row.request_id.as_ref() != Some(request_id)
                || row.ordinal != Some(*ordinal)
                || event.event_id != request_terminal_event_id(request_id, completed)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request-terminal audit payload disagrees with its Journal fact"
                        .into(),
                ));
            }
        }
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary audit kind disagrees with its Journal fact".into(),
            ));
        }
    }
    if row.audit_offset == 0 {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary audit offset must be positive".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_audit_fact(
    connection: &Connection,
    sequence: u64,
    kind: &str,
    call_id: &AuxiliaryCallId,
    request_id: Option<&RequestId>,
    ordinal: Option<u32>,
) -> Result<u64, ConversationStoreError> {
    let row = audit_fact_row(connection, sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary {kind} evidence is missing from its transactional audit index"
        ))
    })?;
    if row.kind != kind
        || &row.call_id != call_id
        || row.request_id.as_ref() != request_id
        || row.ordinal != ordinal
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary {kind} audit payload disagrees with its mandatory identity"
        )));
    }
    let event = event_at_connection(connection, sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "auxiliary {kind} audit row has no Journal fact"
        ))
    })?;
    validate_audit_fact_event(&event, &row)?;
    Ok(row.audit_offset)
}

pub(super) fn request_id_from_event(event: &RuntimeEvent) -> Option<&RequestId> {
    match event {
        RuntimeEvent::ModelRequestStarted { request_id, .. }
        | RuntimeEvent::ModelRequestCompleted { request_id, .. }
        | RuntimeEvent::ModelRequestFailed { request_id, .. } => Some(request_id),
        _ => None,
    }
}

pub(super) fn validate_request_terminal_for_row(
    conversation_id: &ConversationId,
    request: &RequestRow,
    terminal: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    validate_terminal_envelope_shape(terminal, conversation_id, false)?;
    let (owner, request_id, kind) = match &terminal.event {
        RuntimeEvent::ModelRequestCompleted {
            owner, request_id, ..
        } => (owner, request_id, AuxiliaryRequestTerminal::Completed),
        RuntimeEvent::ModelRequestFailed {
            owner, request_id, ..
        } => (owner, request_id, AuxiliaryRequestTerminal::Failed),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary request terminal names a non-terminal event".into(),
            ));
        }
    };
    if request_id != &request.request_id
        || owner
            != &(ModelRequestOwner::Auxiliary {
                call_id: request.call_id.clone(),
                ordinal: request.ordinal,
            })
        || request.terminal_kind != Some(kind)
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request terminal disagrees with its request index".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_owner_row_tx(
    transaction: &Connection,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    indexed: &OwnerRow,
) -> Result<(), ConversationStoreError> {
    let start = event_at_connection(transaction, indexed.start_sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "request {request_id} owner index has no authoritative start fact"
        ))
    })?;
    if start.conversation_id != *conversation_id
        || request_id_from_event(&start.event) != Some(request_id)
        || start.event.model_request_owner() != Some(&indexed.owner)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} owner index disagrees with its start fact"
        )));
    }
    if let ModelRequestOwner::Auxiliary { .. } = indexed.owner
        && start.event_id != request_started_event_id(request_id)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request {request_id} start has a noncanonical event identity"
        )));
    }
    match (indexed.terminal_sequence, indexed.terminal_kind) {
        (None, None) => {}
        (Some(sequence), Some(kind)) => {
            let terminal = event_at_connection(transaction, sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "request {request_id} owner terminal index has no Journal fact"
                ))
            })?;
            if terminal.conversation_id != *conversation_id
                || request_id_from_event(&terminal.event) != Some(request_id)
                || terminal.event.model_request_owner() != Some(&indexed.owner)
                || terminal_kind(&terminal.event) != Some(kind)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request {request_id} owner terminal disagrees with its Journal fact"
                )));
            }
            if let ModelRequestOwner::Auxiliary { call_id, ordinal } = &indexed.owner {
                require_audit_fact(
                    transaction,
                    sequence,
                    "request_settled",
                    call_id,
                    Some(request_id),
                    Some(*ordinal),
                )?;
            }
        }
        _ => {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request {request_id} owner terminal sequence/kind pair is incomplete"
            )));
        }
    }
    if let ModelRequestOwner::Auxiliary { call_id, ordinal } = &indexed.owner {
        require_audit_fact(
            transaction,
            indexed.start_sequence,
            "request_started",
            call_id,
            Some(request_id),
            Some(*ordinal),
        )?;
    }
    Ok(())
}

pub(super) fn request_snapshot_started_sequence(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<u64>, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT started_sequence FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map_err(|error| sqlite_error(&error, format!("request snapshot start lookup: {error}")))?
        .flatten()
        .map(sequence_from_i64)
        .transpose()
}
