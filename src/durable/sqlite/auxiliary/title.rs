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
            "SELECT message_id FROM auxiliary_title_calls WHERE call_id=?1",
            [call_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    let source: Option<(String, bool)> = transaction
        .query_row(
            "SELECT message_id,consumed FROM auxiliary_title_sources WHERE call_id=?1",
            [call_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage(error.to_string()))?;
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
            "UPDATE auxiliary_title_sources SET consumed=1 WHERE call_id=?1",
            [call_id.as_str()],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn release_zero_request(
    transaction: &Transaction<'_>,
    call_id: &AuxiliaryCallId,
) -> Result<(), ConversationStoreError> {
    if !call_has_requests_query(transaction, call_id)? {
        transaction
            .execute(
                "UPDATE auxiliary_title_sources SET call_id=NULL WHERE call_id=?1 AND consumed=0",
                [call_id.as_str()],
            )
            .map_err(|error| storage(error.to_string()))?;
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
    transaction.execute("INSERT INTO auxiliary_title_sources(message_id,call_id,consumed) VALUES(?1,?2,0) ON CONFLICT(message_id) DO UPDATE SET call_id=excluded.call_id WHERE auxiliary_title_sources.call_id IS NULL AND auxiliary_title_sources.consumed=0",params![source.message_id.as_str(),start.call_id.as_str()]).map_err(|error|storage(error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO auxiliary_title_calls(call_id,message_id) VALUES(?1,?2)",
            params![start.call_id.as_str(), source.message_id.as_str()],
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok(())
}
pub(super) fn confirm_association(
    connection: &Connection,
    call_id: &AuxiliaryCallId,
    source: &crate::durable::auxiliary::AuxiliaryTitleSourceBinding,
) -> Result<(), ConversationStoreError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM auxiliary_title_calls WHERE call_id=?1 AND message_id=?2)",
            params![call_id.as_str(), source.message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
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
            let call:Option<String>=transaction.query_row("SELECT call_id FROM auxiliary_title_sources WHERE message_id=?1 AND consumed=0 AND call_id IS NOT NULL",[source.message_id.as_str()],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            let Some(call) = call else {
                return Ok(());
            };
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
            persist_event_tx(transaction, &self.conversation_id, settled)?;
            Ok(())
        })
    }
}
