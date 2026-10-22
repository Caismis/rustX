//! Auxiliary Call/request start and settlement commits; each runs in one write transaction.

use super::super::sqlite_error;

use super::{
    AuxiliaryAuditCut, AuxiliaryAuditFact, AuxiliaryCallId, AuxiliaryCallStart,
    AuxiliaryCallStartCommit, AuxiliaryCommitDisposition, AuxiliaryRequestStart,
    AuxiliaryRequestStartCommit, AuxiliarySettlementCommit, AuxiliaryWriteFailure, Connection,
    ConversationStoreError, DateTime, EVENT_SCHEMA_VERSION, ModelRequestOwner, RequestId,
    RuntimeEvent, RuntimeEventEnvelope, SqliteConversationStore, Transaction, TransactionBehavior,
    Utc, audit_fact_bytes, call_audit_at_tx, call_has_requests_query, call_row_tx,
    call_start_matches, call_started_event_id, event_at_connection, has_call_journal_fact_tx,
    insert_audit_fact_tx, journal_prefix, owner_row_tx, params, persist_event_tx,
    request_audit_at_tx, request_row_tx, request_start_matches, request_started_event_id,
    require_audit_fact, same_terminal_envelope, seq_to_i64, storage, validate_call_start_input,
    validate_request_start_input, validate_settlement_pair, validate_terminal_envelope_shape,
};

pub(super) fn run_auxiliary_write<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, ConversationStoreError>,
) -> Result<T, AuxiliaryWriteFailure> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            AuxiliaryWriteFailure::known(sqlite_error(
                &error,
                format!("auxiliary write transaction: {error}"),
            ))
        })?;
    match operation(&transaction) {
        Ok(value) => transaction.commit().map_or_else(
            |error| {
                Err(AuxiliaryWriteFailure::unknown(storage(format!(
                    "auxiliary write commit: {error}"
                ))))
            },
            |()| Ok(value),
        ),
        Err(error) if transaction.is_autocommit() => {
            // No COMMIT was attempted. SQLite itself rolled this transaction
            // back (e.g. an interrupted INSERT); absence alone is not the proof.
            // Optional writers propagate transaction loss before issuing any
            // later SQL, so no implicit write can escape this original owner.
            Err(AuxiliaryWriteFailure::known(error))
        }
        Err(error) => match transaction.rollback() {
            Ok(()) => Err(AuxiliaryWriteFailure::known(error)),
            Err(rollback) => Err(AuxiliaryWriteFailure::unknown(storage(format!(
                "auxiliary write failed before commit ({error}) and rollback also failed ({rollback})"
            )))),
        },
    }
}

impl SqliteConversationStore {
    pub(in crate::durable::sqlite) fn commit_auxiliary_call_start(
        &self,
        start: &AuxiliaryCallStart,
        timestamp: DateTime<Utc>,
    ) -> Result<AuxiliaryCallStartCommit, AuxiliaryWriteFailure> {
        validate_call_start_input(start).map_err(AuxiliaryWriteFailure::known)?;
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            if let Some(existing) = call_row_tx(transaction, &start.call_id)? {
                let event = event_at_connection(transaction, existing.started_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(format!(
                            "auxiliary Call {} has no start fact",
                            start.call_id
                        ))
                    })?;
                if !call_start_matches(&event, start)
                    || event.event_id != call_started_event_id(&start.call_id)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary Call {} was retried with a conflicting payload",
                        start.call_id
                    )));
                }
                require_audit_fact(
                    transaction,
                    event.sequence,
                    "call_started",
                    &start.call_id,
                    None,
                    None,
                )?;
                let call = call_audit_at_tx(
                    transaction,
                    &self.conversation_id,
                    &start.call_id,
                    AuxiliaryAuditCut {
                        journal: event.sequence,
                    },
                )?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "existing auxiliary Call is missing from its audit index".into(),
                    )
                })?;
                return Ok(AuxiliaryCallStartCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    call,
                    event,
                });
            }
            if has_call_journal_fact_tx(transaction, &start.call_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {} has Journal evidence without a start index",
                    start.call_id
                )));
            }
            let event = RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: call_started_event_id(&start.call_id),
                sequence: 0,
                conversation_id: self.conversation_id.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp,
                event: RuntimeEvent::AuxiliaryCallStarted {
                    call_id: start.call_id.clone(),
                    purpose: start.purpose.clone(),
                    trigger: start.trigger.clone(),
                    business_source: start.business_source.clone(),
                    input: start.input.clone(),
                    semantic_input_fingerprint: start.semantic_input_fingerprint.clone(),
                    invocation_fingerprint: start.invocation_fingerprint.clone(),
                    selected_message_ids: start.selected_message_ids.clone(),
                },
            };
            let persisted = persist_event_tx(transaction, &self.conversation_id, event)?;
            let cut = AuxiliaryAuditCut {
                journal: persisted.event.sequence,
            };
            let call = call_audit_at_tx(transaction, &self.conversation_id, &start.call_id, cut)?
                .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "committed auxiliary Call is missing from its audit index".into(),
                )
            })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::CallStarted(call.clone()),
                persisted.event.sequence,
            )?;
            Ok(AuxiliaryCallStartCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                call,
                event: persisted.event,
            })
        })
    }

    pub(in crate::durable::sqlite) fn commit_auxiliary_request_start(
        &self,
        start: &AuxiliaryRequestStart,
        timestamp: DateTime<Utc>,
    ) -> Result<AuxiliaryRequestStartCommit, AuxiliaryWriteFailure> {
        validate_request_start_input(start).map_err(AuxiliaryWriteFailure::known)?;
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            if let Some(existing) = request_row_tx(transaction, &start.request_id)? {
                let event = event_at_connection(transaction, existing.started_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(format!(
                            "auxiliary request {} has no start fact",
                            start.request_id
                        ))
                    })?;
                if !request_start_matches(&event, start)
                    || event.event_id != request_started_event_id(&start.request_id)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {} was retried with a conflicting payload",
                        start.request_id
                    )));
                }
                require_audit_fact(
                    transaction,
                    event.sequence,
                    "request_started",
                    &start.call_id,
                    Some(&start.request_id),
                    Some(start.ordinal),
                )?;
                let indexed = owner_row_tx(transaction, &start.request_id)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {} is missing its owner index",
                        start.request_id
                    ))
                })?;
                if indexed.owner
                    != (ModelRequestOwner::Auxiliary {
                        call_id: start.call_id.clone(),
                        ordinal: start.ordinal,
                    })
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {} owner index conflicts with its start",
                        start.request_id
                    )));
                }
                let request = request_audit_at_tx(
                    transaction,
                    &self.conversation_id,
                    &start.request_id,
                    AuxiliaryAuditCut {
                        journal: event.sequence,
                    },
                )?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "existing auxiliary request is missing from its audit index".into(),
                    )
                })?;
                return Ok(AuxiliaryRequestStartCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    request,
                    event,
                });
            }
            if crate::durable::sqlite::request::snapshot::has_request_journal_fact_tx(
                transaction,
                &start.request_id,
            )? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {} has Journal evidence without a request-start index",
                    start.request_id
                )));
            }
            let event = RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: request_started_event_id(&start.request_id),
                sequence: 0,
                conversation_id: self.conversation_id.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp,
                event: RuntimeEvent::ModelRequestStarted {
                    owner: ModelRequestOwner::Auxiliary {
                        call_id: start.call_id.clone(),
                        ordinal: start.ordinal,
                    },
                    request_id: start.request_id.clone(),
                    model: start.model.clone(),
                },
            };
            let persisted = persist_event_tx(transaction, &self.conversation_id, event)?;
            transaction
                .execute(
                    "INSERT INTO auxiliary_requests(
                        call_id,ordinal,request_id,model,started_sequence,
                        terminal_sequence,terminal_kind
                     ) VALUES(?1,?2,?3,?4,?5,NULL,NULL)",
                    params![
                        start.call_id.as_str(),
                        i64::from(start.ordinal),
                        start.request_id.as_str(),
                        start.model.as_str(),
                        seq_to_i64(persisted.event.sequence)?,
                    ],
                )
                .map_err(|error| {
                    sqlite_error(&error, format!("index auxiliary request start: {error}"))
                })?;
            insert_audit_fact_tx(
                transaction,
                persisted.event.sequence,
                "request_started",
                &start.call_id,
                Some(&start.request_id),
                Some(start.ordinal),
            )?;
            let cut = AuxiliaryAuditCut {
                journal: persisted.event.sequence,
            };
            let request =
                request_audit_at_tx(transaction, &self.conversation_id, &start.request_id, cut)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "committed auxiliary request is missing from its audit index".into(),
                        )
                    })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::RequestStarted(request.clone()),
                persisted.event.sequence,
            )?;
            Ok(AuxiliaryRequestStartCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                request,
                event: persisted.event,
            })
        })
    }

    pub(in crate::durable::sqlite) fn commit_auxiliary_request_settlement(
        &self,
        request_terminal: RuntimeEventEnvelope,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure> {
        validate_settlement_pair(
            &self.conversation_id,
            &request_terminal,
            &call_settled,
            true,
        )
        .map_err(AuxiliaryWriteFailure::known)?;
        let (request_id, call_id) = settlement_ids(&request_terminal, &call_settled)
            .map_err(AuxiliaryWriteFailure::known)?;
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            let request = request_row_tx(transaction, &request_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary request settlement names unstarted request {request_id}"
                ))
            })?;
            let call = call_row_tx(transaction, &call_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary request settlement names unstarted Call {call_id}"
                ))
            })?;
            if let (Some(request_sequence), Some(call_sequence)) =
                (request.terminal_sequence, call.settled_sequence)
            {
                let stored_request = event_at_connection(transaction, request_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary request terminal index has no Journal fact".into(),
                        )
                    })?;
                let stored_call =
                    event_at_connection(transaction, call_sequence)?.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call settlement index has no Journal fact".into(),
                        )
                    })?;
                if !same_terminal_envelope(&stored_request, &request_terminal)
                    || !same_terminal_envelope(&stored_call, &call_settled)
                {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary request {request_id} settlement was retried with a conflicting payload"
                    )));
                }
                let cut = AuxiliaryAuditCut {
                    journal: journal_prefix(transaction)?,
                };
                let request_audit =
                    request_audit_at_tx(transaction, &self.conversation_id, &request_id, cut)?;
                let call_audit =
                    call_audit_at_tx(transaction, &self.conversation_id, &call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "existing auxiliary Call is missing from its audit index".into(),
                            )
                        })?;
                return Ok(AuxiliarySettlementCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    request: request_audit,
                    call: call_audit,
                    request_terminal: Some(stored_request),
                    call_settled: stored_call,
                });
            }
            if request.terminal_sequence.is_some() || call.settled_sequence.is_some() {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "auxiliary request {request_id} has a partial settlement"
                )));
            }
            let persisted_request =
                persist_event_tx(transaction, &self.conversation_id, request_terminal)?;
            let persisted_call =
                persist_event_tx(transaction, &self.conversation_id, call_settled)?;
            let cut = AuxiliaryAuditCut {
                journal: persisted_call.event.sequence,
            };
            let request_audit =
                request_audit_at_tx(transaction, &self.conversation_id, &request_id, cut)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "committed auxiliary request is missing from its audit index".into(),
                        )
                    })?;
            let call_audit = call_audit_at_tx(transaction, &self.conversation_id, &call_id, cut)?
                .ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "committed auxiliary Call is missing from its audit index".into(),
                )
            })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::RequestSettled(request_audit.clone()),
                persisted_request.event.sequence,
            )?;
            audit_fact_bytes(
                AuxiliaryAuditFact::CallSettled(call_audit.clone()),
                persisted_call.event.sequence,
            )?;
            Ok(AuxiliarySettlementCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                request: Some(request_audit),
                call: call_audit,
                request_terminal: Some(persisted_request.event),
                call_settled: persisted_call.event,
            })
        })
    }

    pub(in crate::durable::sqlite) fn commit_auxiliary_call_settlement(
        &self,
        call_settled: RuntimeEventEnvelope,
    ) -> Result<AuxiliarySettlementCommit, AuxiliaryWriteFailure> {
        validate_terminal_envelope_shape(&call_settled, &self.conversation_id, true)
            .map_err(AuxiliaryWriteFailure::known)?;
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            request_evidence,
            ..
        } = &call_settled.event
        else {
            return Err(AuxiliaryWriteFailure::known(
                ConversationStoreError::InvalidReference(
                    "Call-only settlement requires AuxiliaryCallSettled".into(),
                ),
            ));
        };
        if request_evidence.is_some() {
            return Err(AuxiliaryWriteFailure::known(
                ConversationStoreError::InvalidReference(
                    "Call-only settlement cannot carry request evidence".into(),
                ),
            ));
        }
        let settled_call_id = call_id.clone();
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        run_auxiliary_write(&mut connection, |transaction| {
            let call = call_row_tx(transaction, &settled_call_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {settled_call_id} settled without a start"
                ))
            })?;
            if call_has_requests_query(transaction, &settled_call_id)? {
                return Err(ConversationStoreError::InvalidReference(
                    "Call-only reconciliation requires a zero-request auxiliary Call".into(),
                ));
            }
            if let Some(sequence) = call.settled_sequence {
                let stored = event_at_connection(transaction, sequence)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "auxiliary Call settlement index has no Journal fact".into(),
                    )
                })?;
                if !same_terminal_envelope(&stored, &call_settled) {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "auxiliary Call {settled_call_id} was retried with a conflicting settlement"
                    )));
                }
                let cut = AuxiliaryAuditCut {
                    journal: journal_prefix(transaction)?,
                };
                let call =
                    call_audit_at_tx(transaction, &self.conversation_id, &settled_call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "existing auxiliary Call is missing from its audit index".into(),
                            )
                        })?;
                return Ok(AuxiliarySettlementCommit {
                    disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                    request: None,
                    call,
                    request_terminal: None,
                    call_settled: stored,
                });
            }
            let persisted = persist_event_tx(transaction, &self.conversation_id, call_settled)?;
            let cut = AuxiliaryAuditCut {
                journal: persisted.event.sequence,
            };
            let call = call_audit_at_tx(transaction, &self.conversation_id, &settled_call_id, cut)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "committed auxiliary Call is missing from its audit index".into(),
                    )
                })?;
            audit_fact_bytes(
                AuxiliaryAuditFact::CallSettled(call.clone()),
                persisted.event.sequence,
            )?;
            Ok(AuxiliarySettlementCommit {
                disposition: AuxiliaryCommitDisposition::NewlyCommitted,
                request: None,
                call,
                request_terminal: None,
                call_settled: persisted.event,
            })
        })
    }
}

pub(super) fn settlement_ids(
    request_terminal: &RuntimeEventEnvelope,
    call_settled: &RuntimeEventEnvelope,
) -> Result<(RequestId, AuxiliaryCallId), ConversationStoreError> {
    let (RuntimeEvent::ModelRequestCompleted { request_id, .. }
    | RuntimeEvent::ModelRequestFailed { request_id, .. }) = &request_terminal.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement requires a request terminal".into(),
        ));
    };
    let RuntimeEvent::AuxiliaryCallSettled { call_id, .. } = &call_settled.event else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement requires a Call terminal".into(),
        ));
    };
    Ok((request_id.clone(), call_id.clone()))
}
