//! The native first-Human association, independent of consumer attribution.
use super::{
    AuxiliaryBusinessSource, AuxiliaryCallId, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryWriteFailure, Connection, ConversationId, ConversationStoreError, DateTime,
    OptionalExtension, RuntimeEvent, SqliteConversationStore, Transaction, Utc,
    call_has_requests_query, commit, open_request_tx, params, persist_event_tx, storage,
};

pub(super) fn validate_start(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<(), ConversationStoreError> {
    let bound: Option<String> = transaction
        .query_row(
            "SELECT l.message_id FROM auxiliary_title_calls t JOIN message_ledger l ON l.position=t.ledger_position WHERE t.call_sequence=(SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1)",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    let source: Option<(String, bool)> = transaction
        .query_row(
            "SELECT l.message_id,t.consumed FROM auxiliary_title_sources t JOIN message_ledger l ON l.position=t.ledger_position WHERE t.call_sequence=(SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1)",
            [call_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    if bound.as_ref() != source.as_ref().map(|(message, _)| message) {
        return Err(storage(
            "Title Call no longer owns its exact current source association",
        ));
    }
    if let Some((message_id, consumed)) = source {
        let starts = call_has_requests_query(transaction, call_id)?;
        if consumed != starts {
            return Err(storage(format!(
                "Title source {message_id} disagrees with its Call start evidence"
            )));
        }
    }
    Ok(())
}
pub(super) fn consume(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "UPDATE auxiliary_title_sources SET consumed=1 WHERE call_sequence=(SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1)",
            [call_id.as_str()],
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}
pub(super) fn release_zero_request(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<(), ConversationStoreError> {
    if !call_has_requests_query(transaction, call_id)? {
        transaction
            .execute(
                "UPDATE auxiliary_title_sources SET call_sequence=NULL WHERE call_sequence=(SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1) AND consumed=0",
                [call_id.as_str()],
            )
            .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    }
    Ok(())
}

pub(super) fn associate(
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    start: &AuxiliaryCallStart,
    source: &crate::durable::auxiliary::AuxiliaryTitleSourceBinding,
) -> Result<(), ConversationStoreError> {
    if source.conversation_id != *conversation {
        return Err(storage("Title admission belongs to another Conversation"));
    }
    let first = super::super::canonical::first_human::read(transaction, conversation)?
        .ok_or_else(|| storage("Title admission has no first Human"))?;
    if first.message_id != source.message_id
        || first.position != source.position
        || !first.local
        || !first.eligible
        || first.text.as_ref() != Some(&source.text)
        || start.business_source
            != (AuxiliaryBusinessSource::HumanMessage {
                message_id: source.message_id.clone(),
            })
    {
        return Err(storage(
            "Title source is unavailable or differs from its native first Human",
        ));
    }
    let call_sequence: u64 = transaction
        .query_row(
            "SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1",
            [start.call_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    transaction.execute("INSERT INTO auxiliary_title_sources(ledger_position,call_sequence,consumed) VALUES(?1,?2,0) ON CONFLICT(ledger_position) DO UPDATE SET call_sequence=excluded.call_sequence WHERE auxiliary_title_sources.call_sequence IS NULL AND auxiliary_title_sources.consumed=0",params![source.position,call_sequence]).map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO auxiliary_title_calls(call_sequence,ledger_position) VALUES(?1,?2)",
            params![call_sequence, source.position],
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    Ok(())
}
pub(super) fn confirm_association(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
    source: &crate::durable::auxiliary::AuxiliaryTitleSourceBinding,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM auxiliary_title_calls t JOIN message_ledger l ON l.position=t.ledger_position WHERE t.call_sequence=(SELECT started_sequence FROM auxiliary_calls WHERE call_id=?1) AND t.ledger_position=?2 AND l.message_id=?3)",
            params![call_id.as_str(), source.position, source.message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| crate::durable::sqlite::sqlite_error(&error, error.to_string()))?;
    if !exists {
        return Err(storage(
            "Title exact replay lacks its immutable source binding",
        ));
    }
    Ok(())
}
impl SqliteConversationStore {
    pub(super) fn recover_title_source(
        &self,
        source: &crate::durable::auxiliary::AuxiliaryTitleSourceBinding,
        timestamp: DateTime<Utc>,
    ) -> Result<(), AuxiliaryWriteFailure> {
        if self.conversation_id != source.conversation_id {
            return Err(AuxiliaryWriteFailure::known(storage(
                "Title recovery source belongs to another Conversation",
            )));
        }
        let mut connection = self.lock().map_err(AuxiliaryWriteFailure::known)?;
        commit::run_auxiliary_write(&mut connection, |transaction| {
            let call:Option<Option<String>>=transaction.query_row("SELECT c.call_id FROM auxiliary_title_sources s LEFT JOIN auxiliary_calls c ON c.started_sequence=s.call_sequence WHERE s.ledger_position=(SELECT position FROM message_ledger WHERE message_id=?1) AND s.consumed=0 AND s.call_sequence IS NOT NULL",[source.message_id.as_str()],|row|row.get(0)).optional().map_err(|error|crate::durable::sqlite::sqlite_error(&error,error.to_string()))?;
            let Some(call) = call else {
                return Ok(());
            };
            let call = call.ok_or_else(|| storage("Title source Call is unavailable"))?;
            let call_id =
                AuxiliaryCallId::parse(call).map_err(ConversationStoreError::InvalidReference)?;
            if call_has_requests_query(transaction, &call_id)?
                || open_request_tx(transaction, &call_id)?.is_some()
            {
                return Err(storage(
                    "Title zero-start recovery conflicts with request evidence",
                ));
            }
            let settled = crate::durable::auxiliary::auxiliary_settlement_envelope(
                self.conversation_id.clone(),
                timestamp,
                RuntimeEvent::AuxiliaryCallSettled {
                    call_id,
                    outcome: AuxiliaryCallOutcome::Cancelled,
                    request_evidence: None,
                },
            )?;
            persist_event_tx(
                &self.conn.event_guard,
                transaction,
                &self.conversation_id,
                settled,
            )?;
            Ok(())
        })
    }
}
