//! Exact authoritative confirmation of previously attempted auxiliary commits.

use super::{
    AuxiliaryAuditCut, AuxiliaryCallStart, AuxiliaryCallStartCommit, AuxiliaryCommitConfirmation,
    AuxiliaryCommitDisposition, AuxiliaryRequestStart, AuxiliaryRequestStartCommit,
    AuxiliarySettlementCommit, ConversationStoreError, RuntimeEvent, RuntimeEventEnvelope,
    SqliteConversationStore, call_audit_at_tx, call_row_query, call_row_tx, call_start_matches,
    call_started_event_id, event_at_connection, has_call_journal_fact_tx,
    has_call_settlement_journal_tx, has_request_terminal_journal_tx, journal_prefix,
    request_audit_at_tx, request_row_tx, request_start_matches, request_started_event_id,
    same_terminal_envelope, settlement_ids, storage, validate_call_start_input,
    validate_request_start_input, validate_settlement_pair, validate_terminal_envelope_shape,
};

impl SqliteConversationStore {
    pub(in crate::durable::sqlite) fn confirm_auxiliary_call_start(
        &self,
        expected: &AuxiliaryCallStart,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliaryCallStartCommit>, ConversationStoreError> {
        validate_call_start_input(expected)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary Call start: {error}")))?;
        let Some(call) = call_row_query(&transaction, &expected.call_id)? else {
            if has_call_journal_fact_tx(&transaction, &expected.call_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {} has Journal evidence without its start index",
                    expected.call_id
                )));
            }
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let event = event_at_connection(&transaction, call.started_sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary Call index has no authoritative start".into(),
            )
        })?;
        if !call_start_matches(&event, expected)
            || event.event_id != call_started_event_id(&expected.call_id)
        {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary Call {} confirmation conflicts with its committed payload",
                expected.call_id
            )));
        }
        let audit = call_audit_at_tx(
            &transaction,
            &self.conversation_id,
            &expected.call_id,
            AuxiliaryAuditCut {
                journal: event.sequence,
            },
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "existing auxiliary Call is missing from its audit index".into(),
            )
        })?;
        Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
            AuxiliaryCallStartCommit {
                disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                call: audit,
                event,
            },
        ))
    }

    pub(in crate::durable::sqlite) fn confirm_auxiliary_request_start(
        &self,
        expected: &AuxiliaryRequestStart,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliaryRequestStartCommit>, ConversationStoreError>
    {
        validate_request_start_input(expected)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary request start: {error}")))?;
        let Some(request) = request_row_tx(&transaction, &expected.request_id)? else {
            if crate::durable::sqlite::request::snapshot::has_request_journal_fact_tx(
                &transaction,
                &expected.request_id,
            )? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary request {} has Journal evidence without its start index",
                    expected.request_id
                )));
            }
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let event =
            event_at_connection(&transaction, request.started_sequence)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "auxiliary request index has no authoritative start".into(),
                )
            })?;
        if !request_start_matches(&event, expected)
            || event.event_id != request_started_event_id(&expected.request_id)
        {
            return Err(ConversationStoreError::InvalidReference(format!(
                "auxiliary request {} confirmation conflicts with its committed payload",
                expected.request_id
            )));
        }
        let audit = request_audit_at_tx(
            &transaction,
            &self.conversation_id,
            &expected.request_id,
            AuxiliaryAuditCut {
                journal: event.sequence,
            },
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "existing auxiliary request is missing from its audit index".into(),
            )
        })?;
        Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
            AuxiliaryRequestStartCommit {
                disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                request: audit,
                event,
            },
        ))
    }

    pub(in crate::durable::sqlite) fn confirm_auxiliary_settlement(
        &self,
        request_terminal: &RuntimeEventEnvelope,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliarySettlementCommit>, ConversationStoreError>
    {
        validate_settlement_pair(&self.conversation_id, request_terminal, call_settled, false)?;
        let (request_id, call_id) = settlement_ids(request_terminal, call_settled)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary settlement: {error}")))?;
        let request = request_row_tx(&transaction, &request_id)?;
        let call = call_row_tx(&transaction, &call_id)?;
        match (
            request.and_then(|request| request.terminal_sequence),
            call.and_then(|call| call.settled_sequence),
        ) {
            (None, None) => {
                if has_request_terminal_journal_tx(&transaction, &request_id)?
                    || has_call_settlement_journal_tx(&transaction, &call_id)?
                {
                    return Err(ConversationStoreError::InvalidReference(
                        "auxiliary settlement Journal evidence lacks its transactional indexes"
                            .into(),
                    ));
                }
                Ok(AuxiliaryCommitConfirmation::Unknown)
            }
            (Some(request_sequence), Some(call_sequence)) => {
                let stored_request = event_at_connection(&transaction, request_sequence)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary request settlement index has no Journal fact".into(),
                        )
                    })?;
                let stored_call =
                    event_at_connection(&transaction, call_sequence)?.ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "auxiliary Call settlement index has no Journal fact".into(),
                        )
                    })?;
                validate_terminal_envelope_shape(&stored_request, &self.conversation_id, false)?;
                validate_terminal_envelope_shape(&stored_call, &self.conversation_id, false)?;
                if !same_terminal_envelope(&stored_request, request_terminal)
                    || !same_terminal_envelope(&stored_call, call_settled)
                {
                    return Err(ConversationStoreError::InvalidReference(
                        "auxiliary settlement confirmation conflicts with committed facts".into(),
                    ));
                }
                let cut = AuxiliaryAuditCut {
                    journal: journal_prefix(&transaction)?,
                };
                let request_audit =
                    request_audit_at_tx(&transaction, &self.conversation_id, &request_id, cut)?;
                let call_audit =
                    call_audit_at_tx(&transaction, &self.conversation_id, &call_id, cut)?
                        .ok_or_else(|| {
                            ConversationStoreError::InvalidReference(
                                "existing auxiliary Call is missing from its audit index".into(),
                            )
                        })?;
                Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
                    AuxiliarySettlementCommit {
                        disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                        request: request_audit,
                        call: call_audit,
                        request_terminal: Some(stored_request),
                        call_settled: stored_call,
                    },
                ))
            }
            _ => Err(ConversationStoreError::InvalidReference(
                "auxiliary settlement has partial durable evidence".into(),
            )),
        }
    }

    pub(in crate::durable::sqlite) fn confirm_auxiliary_call_settlement(
        &self,
        call_settled: &RuntimeEventEnvelope,
    ) -> Result<AuxiliaryCommitConfirmation<AuxiliarySettlementCommit>, ConversationStoreError>
    {
        validate_terminal_envelope_shape(call_settled, &self.conversation_id, false)?;
        let RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            request_evidence,
            ..
        } = &call_settled.event
        else {
            return Err(ConversationStoreError::InvalidReference(
                "Call-only confirmation requires AuxiliaryCallSettled".into(),
            ));
        };
        if request_evidence.is_some() {
            return Err(ConversationStoreError::InvalidReference(
                "Call-only confirmation cannot carry request evidence".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(format!("confirm auxiliary Call settlement: {error}")))?;
        let Some(call) = call_row_tx(&transaction, call_id)? else {
            if has_call_journal_fact_tx(&transaction, call_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "auxiliary Call {call_id} has Journal evidence without its start index"
                )));
            }
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let Some(sequence) = call.settled_sequence else {
            return Ok(AuxiliaryCommitConfirmation::Unknown);
        };
        let stored = event_at_connection(&transaction, sequence)?.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "auxiliary Call settlement index has no Journal fact".into(),
            )
        })?;
        validate_terminal_envelope_shape(&stored, &self.conversation_id, false)?;
        if !same_terminal_envelope(&stored, call_settled) {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary Call settlement confirmation conflicts with committed facts".into(),
            ));
        }
        let audit = call_audit_at_tx(
            &transaction,
            &self.conversation_id,
            call_id,
            AuxiliaryAuditCut {
                journal: journal_prefix(&transaction)?,
            },
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "existing auxiliary Call is missing from its audit index".into(),
            )
        })?;
        Ok(AuxiliaryCommitConfirmation::ConfirmedCommitted(
            AuxiliarySettlementCommit {
                disposition: AuxiliaryCommitDisposition::IdempotentReplay,
                request: None,
                call: audit,
                request_terminal: None,
                call_settled: stored,
            },
        ))
    }
}
