//! Bounded auxiliary audit reconstruction at an exact Journal cut.

use super::super::sqlite_error;

use super::{
    AUXILIARY_AUDIT_RECORD_ENCODED_MAX, AuxiliaryAuditCut, AuxiliaryAuditFact,
    AuxiliaryAuditRecord, AuxiliaryCallAudit, AuxiliaryCallId, AuxiliaryCallStart,
    AuxiliaryRequestAudit, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
    AuxiliaryRequestTerminal, Connection, ConversationId, ConversationStoreError,
    ModelRequestOwner, OptionalExtension, RequestId, RequestRow, RuntimeEvent,
    RuntimeEventEnvelope, Transaction, bounded_encoded_len, call_outcome_for_terminal,
    call_row_query, call_started_event_id, event_at_connection, journal_prefix, open_request_query,
    owner_row_query, params, request_row_query, request_start_matches, require_audit_fact,
    seq_to_i64, sequence_from_i64, terminal_kind, validate_call_start_authority_tx,
    validate_call_start_input, validate_owner_row_tx, validate_request_evidence,
    validate_request_start_input, validate_request_terminal_for_row,
    validate_terminal_envelope_shape,
};

pub(super) fn has_call_journal_fact_through_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
    through: u64,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_auxiliary_call_id_kind_idx
               WHERE json_extract(event_json,'$.event.call_id')=?1
                 AND json_extract(event_json,'$.event.type') IN
                     ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')
                 AND sequence<=?2
             )",
            params![call_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary Call Journal probe: {error}")))
}

pub(super) fn has_call_journal_fact_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<bool, ConversationStoreError> {
    has_call_journal_fact_through_tx(transaction, call_id, journal_prefix(transaction)?)
}

pub(super) fn has_request_journal_fact_through_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
    through: u64,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_request_id_kind_idx
               WHERE json_extract(event_json,'$.event.request_id')=?1
                 AND sequence<=?2
             )",
            params![request_id.as_str(), seq_to_i64(through)?],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary request Journal probe: {error}")))
}

pub(super) fn has_request_terminal_journal_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_request_id_kind_idx
               WHERE json_extract(event_json,'$.event.request_id')=?1
                 AND json_extract(event_json,'$.event.type') IN
                     ('model_request_completed','model_request_failed')
             )",
            [request_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary request terminal probe: {error}")))
}

pub(super) fn has_call_settlement_journal_tx(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<bool, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events INDEXED BY events_auxiliary_call_id_kind_idx
               WHERE json_extract(event_json,'$.event.call_id')=?1
                 AND json_extract(event_json,'$.event.type')='auxiliary_call_settled'
             )",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(&error, format!("auxiliary Call settlement probe: {error}")))
}

pub(super) fn call_audit_at_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    call_id: &AuxiliaryCallId,
    cut: AuxiliaryAuditCut,
) -> Result<Option<AuxiliaryCallAudit>, ConversationStoreError> {
    let Some(call) = call_row_query(connection, call_id)? else {
        return Ok(None);
    };
    if call.started_sequence > cut.journal {
        return Ok(None);
    }
    let start = event_at_connection(connection, call.started_sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference("auxiliary Call index has no start event".into())
    })?;
    if start.conversation_id != *conversation_id || start.event_id != call_started_event_id(call_id)
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call start belongs to a foreign or noncanonical event".into(),
        ));
    }
    let (
        purpose,
        trigger,
        business_source,
        input,
        semantic_input_fingerprint,
        invocation_fingerprint,
        selected_message_ids,
    ) = match &start.event {
        RuntimeEvent::AuxiliaryCallStarted {
            call_id: event_call,
            purpose,
            trigger,
            business_source,
            input,
            semantic_input_fingerprint,
            invocation_fingerprint,
            selected_message_ids,
        } if event_call == call_id => (
            purpose.clone(),
            trigger.clone(),
            business_source.clone(),
            input.clone(),
            semantic_input_fingerprint.clone(),
            invocation_fingerprint.clone(),
            selected_message_ids.clone(),
        ),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call index names a non-Call-start event".into(),
            ));
        }
    };
    let start_projection = AuxiliaryCallStart {
        call_id: call_id.clone(),
        purpose: purpose.clone(),
        trigger: trigger.clone(),
        business_source: business_source.clone(),
        input: input.clone(),
        semantic_input_fingerprint: semantic_input_fingerprint.clone(),
        invocation_fingerprint: invocation_fingerprint.clone(),
        selected_message_ids: selected_message_ids.clone(),
    };
    validate_call_start_input(&start_projection)?;
    validate_call_start_authority_tx(connection, conversation_id, &start_projection)?;
    require_audit_fact(
        connection,
        call.started_sequence,
        "call_started",
        call_id,
        None,
        None,
    )?;

    let settled_visible = call
        .settled_sequence
        .filter(|sequence| *sequence <= cut.journal);
    let settlement_facts = connection
        .prepare_cached(
            "SELECT sequence FROM events INDEXED BY events_auxiliary_call_id_kind_idx
         WHERE json_extract(event_json,'$.event.call_id')=?1
           AND json_extract(event_json,'$.event.type')='auxiliary_call_settled'
           AND sequence<=?2 ORDER BY sequence LIMIT 2",
        )
        .map_err(|error| sqlite_error(&error, error.to_string()))?
        .query_map(params![call_id.as_str(), seq_to_i64(cut.journal)?], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| sqlite_error(&error, error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    if settlement_facts.len() > 1
        || settlement_facts
            .first()
            .copied()
            .map(sequence_from_i64)
            .transpose()?
            != settled_visible
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index disagrees with its captured Journal facts".into(),
        ));
    }
    let outcome = if let Some(sequence) = settled_visible {
        let settlement = event_at_connection(connection, sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index has no event".into(),
            )
        })?;
        validate_terminal_envelope_shape(&settlement, conversation_id, false)?;
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id: event_call,
            outcome: event_outcome,
            request_evidence,
        } = &settlement.event
        else {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index names another event kind".into(),
            ));
        };
        if event_call != call_id || call.outcome != Some(*event_outcome) {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index disagrees with its event".into(),
            ));
        }
        require_audit_fact(connection, sequence, "call_settled", call_id, None, None)?;
        if open_request_query(connection, call_id)?.is_some() {
            return Err(ConversationStoreError::InvalidReference(
                "a settled auxiliary Call cannot retain an open request".into(),
            ));
        }
        if request_evidence.is_none() {
            super::retry::validate_call_only(connection, call_id, *event_outcome, Some(sequence))?;
        }
        if let Some(evidence) = request_evidence {
            let request =
                request_row_query(connection, "request_id=?1", [evidence.request_id.as_str()])?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call request evidence names a missing request".into(),
                        )
                    })?;
            if request.call_id != *call_id {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call request evidence names another Call".into(),
                ));
            }
            let terminal_sequence = request
                .terminal_sequence
                .filter(|terminal| *terminal <= cut.journal)
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary Call request evidence has no visible request terminal".into(),
                    )
                })?;
            if call.started_sequence >= request.started_sequence
                || request.started_sequence >= terminal_sequence
                || terminal_sequence >= sequence
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call/request start and terminal order is invalid".into(),
                ));
            }
            let terminal =
                event_at_connection(connection, terminal_sequence)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary request terminal index has no Journal fact".into(),
                    )
                })?;
            validate_request_terminal_for_row(conversation_id, &request, &terminal)?;
            if !call_outcome_for_terminal(&terminal.event)
                .is_some_and(|outcomes| outcomes.contains(event_outcome))
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary Call outcome disagrees with its request terminal".into(),
                ));
            }
            require_audit_fact(
                connection,
                terminal_sequence,
                "request_settled",
                call_id,
                Some(&request.request_id),
                Some(request.ordinal),
            )?;
            let owner = owner_row_query(connection, &request.request_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary request terminal has no owner index".into(),
                )
            })?;
            validate_owner_row_tx(connection, conversation_id, &request.request_id, &owner)?;
            if owner.start_sequence != request.started_sequence
                || owner.terminal_sequence != Some(terminal_sequence)
                || owner.terminal_kind != terminal_kind(&terminal.event)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request owner and request rows disagree".into(),
                ));
            }
            validate_request_evidence(evidence, &terminal)?;
        }
        Some(*event_outcome)
    } else {
        None
    };
    if (call.settled_sequence.is_some() && call.outcome.is_none())
        || (call.settled_sequence.is_none() && call.outcome.is_some())
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement sequence/outcome pair is incomplete".into(),
        ));
    }
    Ok(Some(AuxiliaryCallAudit {
        call_id: call_id.clone(),
        purpose,
        trigger,
        business_source,
        input,
        semantic_input_fingerprint,
        invocation_fingerprint,
        selected_message_ids,
        started_sequence: call.started_sequence,
        settled_sequence: settled_visible,
        outcome,
    }))
}

pub(super) fn request_evidence_at_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    request: &RequestRow,
    cut: AuxiliaryAuditCut,
    request_terminal: Option<&RuntimeEventEnvelope>,
) -> Result<Option<AuxiliaryRequestEvidence>, ConversationStoreError> {
    if let Some(evidence) =
        super::retry::request_evidence(connection, conversation_id, request, cut, request_terminal)?
    {
        return Ok(Some(evidence));
    }
    let Some(call) = call_row_query(connection, &request.call_id)? else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request has no Call index".into(),
        ));
    };
    let Some(sequence) = call
        .settled_sequence
        .filter(|sequence| *sequence <= cut.journal)
    else {
        return Ok(None);
    };
    let settlement = event_at_connection(connection, sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index has no Journal event".into(),
        )
    })?;
    validate_terminal_envelope_shape(&settlement, conversation_id, false)?;
    let RuntimeEvent::AuxiliaryCallSettled {
        call_id,
        outcome,
        request_evidence,
        ..
    } = &settlement.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index names a non-settlement event".into(),
        ));
    };
    if call_id != &request.call_id {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request and Call settlement name different Calls".into(),
        ));
    }
    if call.outcome != Some(*outcome) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call settlement index disagrees with its outcome".into(),
        ));
    }
    require_audit_fact(connection, sequence, "call_settled", call_id, None, None)?;
    let Some(evidence) = request_evidence else {
        return Ok(None);
    };
    if evidence.request_id != request.request_id {
        return Ok(None);
    }
    let terminal = request_terminal.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary Call carries request evidence while its request has no terminal".into(),
        )
    })?;
    if !call_outcome_for_terminal(&terminal.event)
        .is_some_and(|outcomes| outcomes.contains(outcome))
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call outcome disagrees with its request terminal".into(),
        ));
    }
    validate_request_evidence(evidence, terminal)?;
    Ok(Some(evidence.clone()))
}

#[cfg(test)]
thread_local! {
    static AFTER_REQUEST_ROW_READ:
        std::cell::RefCell<Option<(RequestId, Box<dyn FnOnce()>)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(in crate::durable::sqlite) fn install_after_request_row_read_hook(
    request_id: RequestId,
    hook: impl FnOnce() + 'static,
) {
    AFTER_REQUEST_ROW_READ.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a request-row read hook is already installed on this thread"
        );
        *slot.borrow_mut() = Some((request_id, Box::new(hook)));
    });
}

#[cfg(test)]
pub(super) fn after_request_row_read(request_id: &RequestId) {
    let hook = AFTER_REQUEST_ROW_READ.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(installed_request_id, _)| installed_request_id == request_id)
        {
            slot.take().map(|(_, hook)| hook)
        } else {
            None
        }
    });
    if let Some(hook) = hook {
        hook();
    }
}

pub(super) fn request_audit_at_tx(
    connection: &Connection,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    cut: AuxiliaryAuditCut,
) -> Result<Option<AuxiliaryRequestAudit>, ConversationStoreError> {
    let request = request_row_query(connection, "request_id=?1", [request_id.as_str()])?;
    let Some(request) = request else {
        return Ok(None);
    };
    if request.started_sequence > cut.journal {
        return Ok(None);
    }
    #[cfg(test)]
    after_request_row_read(request_id);
    let call =
        call_audit_at_tx(connection, conversation_id, &request.call_id, cut)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary request has no visible Call authority".into(),
            )
        })?;
    if call.started_sequence >= request.started_sequence {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request start does not follow its Call start".into(),
        ));
    }
    let start = event_at_connection(connection, request.started_sequence)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary request index has no start event".into(),
        )
    })?;
    let start_input = AuxiliaryRequestStart {
        call_id: request.call_id.clone(),
        ordinal: request.ordinal,
        request_id: request.request_id.clone(),
        model: request.model.clone(),
    };
    validate_request_start_input(&start_input)?;
    if start.conversation_id != *conversation_id || !request_start_matches(&start, &start_input) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request index disagrees with its start event".into(),
        ));
    }
    let owner = owner_row_query(connection, request_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "auxiliary request has no authoritative owner index".into(),
        )
    })?;
    validate_owner_row_tx(connection, conversation_id, request_id, &owner)?;
    if owner.owner
        != (ModelRequestOwner::Auxiliary {
            call_id: request.call_id.clone(),
            ordinal: request.ordinal,
        })
        || owner.start_sequence != request.started_sequence
        || owner.terminal_sequence != request.terminal_sequence
        || owner.terminal_kind
            != request.terminal_kind.map(|kind| match kind {
                AuxiliaryRequestTerminal::Completed => "completed",
                AuxiliaryRequestTerminal::Failed => "failed",
            })
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request owner and request rows disagree".into(),
        ));
    }
    require_audit_fact(
        connection,
        request.started_sequence,
        "request_started",
        &request.call_id,
        Some(request_id),
        Some(request.ordinal),
    )?;

    let terminal_sequence = request
        .terminal_sequence
        .filter(|sequence| *sequence <= cut.journal);
    let mut request_terminal_event = None;
    let mut local_terminal = None;
    let mut normalized_failure_kind = None;
    let mut generation = None;
    let mut usage = None;
    if let Some(sequence) = terminal_sequence {
        let terminal = event_at_connection(connection, sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary request terminal index has no Journal event".into(),
            )
        })?;
        validate_request_terminal_for_row(conversation_id, &request, &terminal)?;
        match &terminal.event {
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id: event_request_id,
                usage: event_usage,
                generation: event_generation,
                ..
            } if call_id == &request.call_id
                && *ordinal == request.ordinal
                && event_request_id == request_id =>
            {
                local_terminal = Some(AuxiliaryRequestTerminal::Completed);
                generation = *event_generation;
                usage.clone_from(event_usage);
            }
            RuntimeEvent::ModelRequestFailed {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id: event_request_id,
                error,
                usage: event_usage,
                generation: event_generation,
            } if call_id == &request.call_id
                && *ordinal == request.ordinal
                && event_request_id == request_id =>
            {
                local_terminal = Some(AuxiliaryRequestTerminal::Failed);
                normalized_failure_kind = Some(error.kind.clone());
                generation = *event_generation;
                usage.clone_from(event_usage);
            }
            _ => {
                return Err(ConversationStoreError::InvalidReference(
                    "auxiliary request terminal disagrees with its start owner".into(),
                ));
            }
        }
        require_audit_fact(
            connection,
            sequence,
            "request_settled",
            &request.call_id,
            Some(request_id),
            Some(request.ordinal),
        )?;
        request_terminal_event = Some(terminal);
    }
    let request_evidence = request_evidence_at_tx(
        connection,
        conversation_id,
        &request,
        cut,
        request_terminal_event.as_ref(),
    )?;
    Ok(Some(AuxiliaryRequestAudit {
        call_id: request.call_id,
        ordinal: request.ordinal,
        request_id: request.request_id,
        model: request.model,
        started_sequence: request.started_sequence,
        terminal_sequence,
        local_terminal,
        normalized_failure_kind,
        generation,
        usage,
        request_evidence,
    }))
}

pub(in crate::durable::sqlite) const AUXILIARY_AUDIT_FRONTIER_SQL: &str =
    "SELECT sequence,audit_offset FROM auxiliary_audit_facts
     INDEXED BY auxiliary_audit_sequence_idx
     WHERE sequence<=?1 ORDER BY sequence DESC LIMIT 1";

pub(in crate::durable::sqlite) const AUXILIARY_CALL_REQUESTS_SQL: &str =
    "SELECT sequence,request_id,ordinal FROM auxiliary_audit_facts
     INDEXED BY auxiliary_audit_call_idx
     WHERE call_id=?1 AND sequence>?2 AND sequence<=?3 AND kind='request_started'
     ORDER BY sequence LIMIT ?4";

pub(in crate::durable::sqlite) const AUXILIARY_AUTHORITATIVE_FRONTIER_SQL: &str = "
SELECT max(sequence) FROM (
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='auxiliary_call_started'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='model_request_started'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='model_request_completed'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type')='model_request_failed'
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')
      )
    UNION ALL
    SELECT max(sequence) AS sequence
    FROM events INDEXED BY events_auxiliary_fact_type_idx
    WHERE sequence<=?1
      AND json_extract(event_json,'$.event.type') IN ('auxiliary_call_settled','auxiliary_retry_scheduled')
      AND (
          (json_extract(event_json,'$.event.type') IN
              ('model_request_started','model_request_completed','model_request_failed')
           AND json_extract(event_json,'$.event.owner.type')='auxiliary')
          OR json_extract(event_json,'$.event.type') IN
              ('auxiliary_call_started','auxiliary_call_settled','auxiliary_retry_scheduled')
      )
)";

pub(super) fn audit_total_at_connection(
    connection: &Connection,
    cut: AuxiliaryAuditCut,
) -> Result<u64, ConversationStoreError> {
    let current = journal_prefix(connection)?;
    if cut.journal > current {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary audit cut is in the future".into(),
        ));
    }
    let journal = seq_to_i64(cut.journal)?;
    let derived: Option<(i64, i64)> = connection
        .query_row(AUXILIARY_AUDIT_FRONTIER_SQL, [journal], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(|error| sqlite_error(&error, format!("auxiliary audit total lookup: {error}")))?;
    let authoritative: Option<i64> = connection
        .query_row(AUXILIARY_AUTHORITATIVE_FRONTIER_SQL, [journal], |row| {
            row.get(0)
        })
        .map_err(|error| {
            sqlite_error(&error, format!("auxiliary authoritative frontier: {error}"))
        })?;
    let authoritative = authoritative.map(sequence_from_i64).transpose()?;
    match (derived, authoritative) {
        (None, None) => Ok(0),
        (Some((derived_sequence, audit_offset)), Some(authoritative_sequence))
            if sequence_from_i64(derived_sequence)? == authoritative_sequence =>
        {
            u64::try_from(audit_offset).map_err(|_| {
                ConversationStoreError::InvalidReference("auxiliary audit total is negative".into())
            })
        }
        _ => Err(ConversationStoreError::InvalidReference(
            "auxiliary audit frontier disagrees with authoritative Journal facts".into(),
        )),
    }
}

pub(super) fn audit_record_bytes(
    record: &AuxiliaryAuditRecord,
) -> Result<(), ConversationStoreError> {
    bounded_encoded_len(
        record,
        AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
        "auxiliary audit record",
    )?;
    Ok(())
}

pub(super) fn audit_fact_bytes(
    fact: AuxiliaryAuditFact,
    journal_sequence: u64,
) -> Result<(), ConversationStoreError> {
    audit_record_bytes(&AuxiliaryAuditRecord {
        audit_offset: u64::MAX,
        journal_sequence,
        fact,
    })
}
