//! The durable publication plane: streams, staged frames, terminals, and audits.

use rusqlite::OptionalExtension;
pub(super) mod proposal;

use self::proposal::{
    load_publication_proposal_owners, validate_audit_proposal_ownership,
    validate_canonical_tool_proposals, validate_proposal_frames,
};
use super::canonical::ledger::{append_transcript_reference, ensure_surface_head};
use super::canonical::{canonical_observation_receipt, set_canonical_range};
use super::journal::{persist_event_tx, validate_canonical_event_for_message};
use super::request::snapshot::require_started_request_tx;
use super::{
    SqliteConversationStore, append_validation, content, decode, encode, nonnegative, observation,
    observation_readiness, observation_receipt, observation_trace, optional_display_layout,
    seq_to_i64, storage,
};
use crate::durable::{
    ConversationStoreError, PublicationStore, TranscriptCommitReceipt, TranscriptCursor,
};
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::MessageBlock;
use crate::model::snapshot::RequestSnapshot;
use crate::publication::{
    PublicationAudit, PublicationFrame, PublicationSettlement, PublicationStreamRecord,
    PublicationStreamStart, consolidate_audit_content,
};
use crate::runtime::identity::{
    AttemptId, ConversationId, MessageId, PublicationStreamId, RequestId, TurnId,
};
use crate::runtime::process_death;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use std::sync::Arc;

impl PublicationStore for SqliteConversationStore {
    fn open_publication_stream(
        &self,
        start: &PublicationStreamStart,
    ) -> Result<(), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication open transaction: {error}")))?;
        // The first open is the publication generation admission boundary.
        // Prove the complete Request Snapshot/start-event identity before an
        // idempotent reopen probe or any publication row can be written.
        let (snapshot, _, _) =
            require_started_request_tx(&transaction, &self.conversation_id, &start.request_id)?;
        validate_publication_generation(&snapshot, start)?;
        if let Some(existing) = read_publication_stream(&transaction, &start.stream_id)? {
            // Re-opening is idempotent only for the identical frozen identity:
            // a stream may never be re-associated with another request,
            // attempt, turn, or provisional message.
            if existing.start != *start {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "publication stream {} is already open under a different request generation",
                    start.stream_id
                )));
            }
            if existing.settlement.is_some() {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "publication stream {} already settled and cannot reopen",
                    start.stream_id
                )));
            }
            return Ok(());
        }
        transaction
            .execute(
                "INSERT INTO publication_streams(stream_id,attempt_id,turn_id,request_id,message_id,next_frame_sequence,terminal_sequence,settlement)
                 VALUES(?1,?2,?3,?4,?5,0,NULL,NULL)",
                params![
                    start.stream_id.as_str(),
                    start.attempt_id.as_str(),
                    start.turn_id.as_str(),
                    start.request_id.as_str(),
                    start.message_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("open publication stream: {error}")))?;
        process_death::reach("before:open_publication_stream");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication open commit: {error}")))?;
        process_death::reach("after:open_publication_stream");
        Ok(())
    }
    fn stage_publication_frames(
        &self,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError> {
        if frames.is_empty() {
            return Err(ConversationStoreError::PublicationViolation(
                "a publication staging transaction must carry at least one frame".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication staging transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_frames_remaining) {
            return Err(storage("fault injected: publication staging commit"));
        }
        let (stream, receipts) = stage_frames_tx(&transaction, frames, &self.conversation_id)?;
        if stream.terminal_sequence.is_some() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {} already reached its terminal boundary",
                stream.start.stream_id
            )));
        }
        process_death::reach("before:stage_publication_frames");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication staging commit: {error}")))?;
        process_death::reach("after:stage_publication_frames");
        Ok(receipts)
    }
    fn commit_publication_terminal(
        &self,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError> {
        if frames.is_empty() {
            return Err(ConversationStoreError::PublicationViolation(
                "the publication terminal transaction must carry a final frame".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication terminal transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_terminal_remaining) {
            return Err(storage("fault injected: publication terminal commit"));
        }
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        // U may never precede P: prove the exact provider outcome before the
        // frame/proposal staging transaction does any durable work.
        if !request_outcome_is_durable(
            &transaction,
            &self.conversation_id,
            &snapshot,
            completed_sequence,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication terminal for {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        let (stream, receipts) = stage_frames_tx(&transaction, frames, &self.conversation_id)?;
        if stream.start.stream_id != *stream_id {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "terminal frames belong to publication stream {}, not {stream_id}",
                stream.start.stream_id
            )));
        }
        if stream.terminal_sequence.is_some() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already committed its terminal boundary"
            )));
        }
        let terminal_sequence = frames
            .last()
            .map(|frame| frame.sequence)
            .ok_or_else(|| storage("terminal frames are empty"))?;
        // The final frame and the terminal marker share this one transaction.
        transaction
            .execute(
                "UPDATE publication_streams SET terminal_sequence=?1 WHERE stream_id=?2",
                params![seq_to_i64(terminal_sequence)?, stream_id.as_str()],
            )
            .map_err(|error| storage(format!("commit publication terminal: {error}")))?;
        process_death::reach("before:commit_publication_terminal");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication terminal commit: {error}")))?;
        process_death::reach("after:commit_publication_terminal");
        Ok(receipts)
    }
    // Keep native acceptance, event/staging and COMMIT in one auditable boundary.
    #[allow(clippy::too_many_lines)]
    fn commit_canonical_publication(
        &self,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError> {
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical publication transaction: {error}")))?;
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        validate_canonical_publication_event(
            &self.conversation_id,
            &stream.start,
            message,
            &event,
        )?;
        validate_canonical_tool_proposals(&transaction, stream_id, message)?;
        if let Some(settlement) = stream.settlement {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already settled as {}; canonical acceptance is permanently forbidden",
                settlement.as_str()
            )));
        }
        // C may never precede U for a published stream.
        if stream.terminal_sequence.is_none() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} has no durable publication terminal; canonical acceptance requires U"
            )));
        }
        if !matches!(message, MessageBlock::Assistant(_)) {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} requires an Assistant message"
            )));
        }
        if !request_outcome_is_durable(
            &transaction,
            &self.conversation_id,
            &snapshot,
            completed_sequence,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        ensure_surface_head(&transaction)?;

        let transcript_cursor =
            validation
                .append(&transaction, message, None)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "canonical publication must receive a transcript cursor".to_owned(),
                    )
                })?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical publication commit"));
        }
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        content::settle_canonical(&transaction, stream_id, message)?;
        rebind_published_read_layouts(&transaction, message, &persisted.event)?;
        // The canonical transition clears the lifecycle staging of the stream:
        // the Ledger is now the long-term authority and no transient frame
        // survives it.
        clear_publication_staging(&transaction, stream_id)?;
        // Proposal ownership remains as one bounded `(stream_id, call_id)`
        // fact after C. Recovery ToolResult repair and later Tool Plane
        // transitions must still resolve the exact accepted proposal rather
        // than falling back to a bare call id.
        transaction
            .execute(
                "UPDATE publication_proposals SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication proposals: {error}")))?;
        transaction
            .execute(
                "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication: {error}")))?;
        let mut receipt = canonical_observation_receipt(
            &transaction,
            &self.conversation_id,
            message,
            Some(transcript_cursor),
            persisted.event.tool_occurrence.clone(),
        )?;
        process_death::reach("before:commit_canonical_publication");
        let sequence = persisted.event.sequence;
        set_canonical_range(&mut receipt, Some((sequence, sequence)));
        persisted.event.observation = observation_receipt::canonical(
            &transaction,
            &self.conversation_id,
            &persisted.event,
            Arc::clone(&receipt.observation),
        );
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical publication commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach("after:commit_canonical_publication");
        Ok((persisted.event, receipt))
    }
    fn terminalize_publication_audit(
        &self,
        stream_id: &PublicationStreamId,
        timestamp: DateTime<Utc>,
    ) -> Result<(PublicationAudit, crate::durable::PublicationAuditReceipt), ConversationStoreError>
    {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication audit transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_audit_remaining) {
            return Err(storage("fault injected: publication audit commit"));
        }
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        if let Some(settlement) = stream.settlement {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already settled as {}",
                settlement.as_str()
            )));
        }
        if stream.terminal_sequence.is_some()
            && !request_outcome_is_durable(
                &transaction,
                &self.conversation_id,
                &snapshot,
                completed_sequence,
            )?
        {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication audit for {stream_id} has U without the exact successful provider outcome"
            )));
        }
        // The kind is derived from durable evidence, never supplied: U
        // present means the released output was complete but never accepted;
        // U absent means publication never reached its own terminal boundary.
        let kind = stream.audit_kind();
        let executed: Option<String> = transaction
            .query_row(
                "SELECT call_id FROM publication_proposals WHERE stream_id=?1 AND executed=1 LIMIT 1",
                [stream_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("publication proposal execution probe: {error}")))?;
        if let Some(call_id) = executed {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} cannot terminalize as an audit: proposal {call_id} already has a Tool Plane execution fact"
            )));
        }
        let frames = read_publication_frames(&transaction, stream_id)?;
        let content = consolidate_audit_content(&frames);
        validate_audit_proposal_ownership(&transaction, stream_id, &content)?;
        let audit = PublicationAudit {
            stream_id: stream_id.clone(),
            attempt_id: stream.start.attempt_id.clone(),
            turn_id: stream.start.turn_id.clone(),
            request_id: stream.start.request_id.clone(),
            message_id: stream.start.message_id.clone(),
            kind,
            content,
            settled_at: timestamp,
        };
        let _ =
            optional_display_layout(&transaction, || content::settle_audit(&transaction, &audit))?;
        let settlement = PublicationSettlement::from(kind);
        transaction
            .execute(
                "INSERT INTO publication_audits(stream_id,audit_json) VALUES(?1,?2)",
                params![stream_id.as_str(), encode(&audit, "publication audit")?],
            )
            .map_err(|error| storage(format!("insert publication audit: {error}")))?;
        let transcript_cursor =
            append_transcript_reference(&transaction, "publication_audit", stream_id.as_str())?;
        // Consolidation replaces the transient frames: the durable footprint
        // of a settled audit is one bounded object, never O(frames) rows.
        clear_publication_staging(&transaction, stream_id)?;
        transaction
            .execute(
                "UPDATE publication_proposals SET settlement=?1 WHERE stream_id=?2",
                params![settlement.as_str(), stream_id.as_str()],
            )
            .map_err(|error| storage(format!("ban audited tool proposals: {error}")))?;
        transaction
            .execute(
                "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                params![settlement.as_str(), stream_id.as_str()],
            )
            .map_err(|error| storage(format!("settle publication audit: {error}")))?;
        let receipt = audit_observation_receipt(
            &transaction,
            &self.conversation_id,
            &audit,
            transcript_cursor,
        )?;
        process_death::reach("before:terminalize_publication_audit");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication audit commit: {error}")))?;
        process_death::reach("after:terminalize_publication_audit");
        Ok((audit, receipt))
    }
    fn load_unsettled_publication_streams(
        &self,
    ) -> Result<Vec<PublicationStreamRecord>, ConversationStoreError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT stream_id,attempt_id,turn_id,request_id,message_id,terminal_sequence,settlement
                 FROM publication_streams WHERE settlement IS NULL ORDER BY stream_id",
            )
            .map_err(|error| storage(format!("unsettled publication query: {error}")))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|error| storage(format!("unsettled publication rows: {error}")))?;
        let mut records = Vec::new();
        for row in rows {
            let row =
                row.map_err(|error| storage(format!("unsettled publication row: {error}")))?;
            records.push(publication_record(row)?);
        }
        Ok(records)
    }
    fn load_publication_audit(
        &self,
        stream_id: &PublicationStreamId,
    ) -> Result<Option<PublicationAudit>, ConversationStoreError> {
        let connection = self.lock()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT audit_json FROM publication_audits WHERE stream_id=?1",
                [stream_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("publication audit read: {error}")))?;
        json.map(|json| decode(&json, "publication audit"))
            .transpose()
    }
}

/// Reads one publication stream record, when it exists.
pub(super) fn read_publication_stream(
    transaction: &Connection,
    stream_id: &PublicationStreamId,
) -> Result<Option<PublicationStreamRecord>, ConversationStoreError> {
    let row = transaction
        .query_row(
            "SELECT stream_id,attempt_id,turn_id,request_id,message_id,terminal_sequence,settlement
             FROM publication_streams WHERE stream_id=?1",
            [stream_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("publication stream read: {error}")))?;
    row.map(publication_record).transpose()
}

pub(super) fn require_publication_stream(
    transaction: &Connection,
    stream_id: &PublicationStreamId,
) -> Result<PublicationStreamRecord, ConversationStoreError> {
    read_publication_stream(transaction, stream_id)?.ok_or_else(|| {
        ConversationStoreError::PublicationViolation(format!(
            "publication stream {stream_id} was never opened"
        ))
    })
}

pub(super) type PublicationStreamRow = (
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
);

pub(super) fn rebind_published_read_layouts(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let _ = optional_display_layout(transaction, || {
        observation_trace::settle_fields(transaction, event)
    })?;
    rebind_canonical_display(transaction, message)
}

pub(super) fn rebind_canonical_display(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    // Rebind deferred Tool arguments to their retained original raw encoding at C.
    let position: i64 = transaction
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message.id().as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let _ = optional_display_layout(transaction, || {
        observation::index::index_message(
            transaction,
            message,
            Some(nonnegative(position, "header ledger position")?),
        )
    })?;
    Ok(())
}

pub(super) fn audit_observation_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    audit: &PublicationAudit,
    cursor: TranscriptCursor,
) -> Result<crate::durable::PublicationAuditReceipt, ConversationStoreError> {
    let read_cut = observation::cut(connection, conversation)?;
    let summary = observation::audit_summary(audit);
    let _ = optional_display_layout(connection, || {
        observation::index::index_audit(connection, audit, cursor)?;
        connection.execute("UPDATE observation_headers SET journal_position=?1 WHERE reference_kind='publication_audit' AND reference_id=?2",params![read_cut.read.journal,audit.stream_id.as_str()]).map_err(|error|storage(error.to_string()))?;
        Ok(())
    })?;
    let bytes:Option<Vec<u8>>=connection.query_row("SELECT header_json FROM observation_headers WHERE reference_kind='publication_audit' AND reference_id=?1",[audit.stream_id.as_str()],|row|row.get(0)).optional().ok().flatten();
    let header = bytes
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| crate::durable::observation::TranscriptHeader {
            tool_calls: Vec::new(),
            publication_settlement: Some(
                crate::observation::transcript::RuntimeClientPublicationSettlementRecord {
                    settlement: summary.clone().into(),
                    transcript_cursor: cursor.into(),
                    read_cut: read_cut.clone(),
                },
            ),
            cursor,
            reference_kind: "publication_audit".into(),
            reference_id: format!("transcript:{}", cursor.get()),
            message: None,
            display: None,
            content: Vec::new(),
            collections: Vec::new(),
            deferred: true,
        });
    Ok(crate::durable::PublicationAuditReceipt {
        ready_through: observation_readiness::frontier(connection, 0, read_cut.read.journal).ok(),
        ready_t_through: observation_readiness::transcript_frontier(
            connection,
            0,
            read_cut.read.transcript,
        )
        .ok(),
        summary,
        transcript_cursor: cursor,
        header,
        read_cut,
    })
}

pub(super) fn publication_record(
    row: PublicationStreamRow,
) -> Result<PublicationStreamRecord, ConversationStoreError> {
    let (stream_id, attempt_id, turn_id, request_id, message_id, terminal_sequence, settlement) =
        row;
    let settlement = settlement
        .map(|value| {
            PublicationSettlement::parse(&value).ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "publication stream {stream_id} has an unknown settlement {value}"
                ))
            })
        })
        .transpose()?;
    let terminal_sequence = terminal_sequence
        .map(|value| nonnegative(value, "publication terminal sequence"))
        .transpose()?;
    Ok(PublicationStreamRecord {
        start: PublicationStreamStart {
            stream_id: PublicationStreamId::new(stream_id),
            attempt_id: AttemptId::new(attempt_id),
            turn_id: TurnId::new(turn_id),
            request_id: RequestId::new(request_id),
            message_id: MessageId::new(message_id),
        },
        terminal_sequence,
        settlement,
    })
}

/// Stages one contiguous run of frames onto an open unsettled stream and
/// returns the prior stream and finite per-frame committed source evidence.
pub(super) fn stage_frames_tx(
    transaction: &Transaction<'_>,
    frames: &[PublicationFrame],
    conversation_id: &ConversationId,
) -> Result<
    (
        PublicationStreamRecord,
        Vec<crate::publication::PublicationFrameReceipt>,
    ),
    ConversationStoreError,
> {
    let stream_id = &frames[0].stream_id;
    if frames.iter().any(|frame| frame.stream_id != *stream_id) {
        return Err(ConversationStoreError::PublicationViolation(
            "one publication transaction may only stage frames of one stream".to_owned(),
        ));
    }
    let stream = require_publication_stream(transaction, stream_id)?;
    if let Some(settlement) = stream.settlement {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "publication stream {stream_id} already settled as {}",
            settlement.as_str()
        )));
    }
    let mut next: i64 = transaction
        .query_row(
            "SELECT next_frame_sequence FROM publication_streams WHERE stream_id=?1",
            [stream_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("publication sequence read: {error}")))?;
    let mut owners = load_publication_proposal_owners(transaction, stream_id)?;

    // Preflight the complete transaction before inserting a frame or changing
    // an ownership row. In particular, a suffix/completion cannot become an
    // orphaned audit proposal merely because it appeared in a later batch.
    for frame in frames {
        if frame.message_id != stream.start.message_id {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication frame of {stream_id} names a foreign message identity"
            )));
        }
        let expected = nonnegative(next, "publication frame sequence")?;
        if frame.sequence != expected {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication frame of {stream_id} is sequence {} where {expected} was required",
                frame.sequence
            )));
        }
        next = next
            .checked_add(1)
            .ok_or_else(|| storage("publication frame sequence overflow"))?;
    }
    validate_proposal_frames(stream_id, frames, &mut owners)?;

    let mut receipts = Vec::with_capacity(frames.len());
    for frame in frames {
        let receipt = stage_frame_tx(transaction, conversation_id, &stream.start, frame)?;
        receipts.push(receipt);
    }
    for owner in owners {
        match owner.persisted_state {
            None => {
                transaction
                    .execute(
                        "INSERT INTO publication_proposals(stream_id,call_id,block_index,tool_id,tool_name,state,executed,settlement)
                         VALUES(?1,?2,?3,?4,?5,?6,0,NULL)",
                        params![
                            stream_id.as_str(),
                            owner.call_id.as_str(),
                            i64::from(owner.block_index.get()),
                            owner.tool_id.as_str(),
                            owner.name,
                            owner.state.as_str(),
                        ],
                    )
                    .map_err(|error| storage(format!("register tool proposal: {error}")))?;
            }
            Some(previous) if previous != owner.state => {
                transaction
                    .execute(
                        "UPDATE publication_proposals SET state=?1 WHERE stream_id=?2 AND call_id=?3",
                        params![owner.state.as_str(), stream_id.as_str(), owner.call_id.as_str()],
                    )
                    .map_err(|error| storage(format!("advance tool proposal state: {error}")))?;
            }
            Some(_) => {}
        }
    }
    transaction
        .execute(
            "UPDATE publication_streams SET next_frame_sequence=?1 WHERE stream_id=?2",
            params![next, stream_id.as_str()],
        )
        .map_err(|error| storage(format!("advance publication sequence: {error}")))?;
    Ok((stream, receipts))
}

pub(super) fn stage_frame_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    start: &PublicationStreamStart,
    frame: &PublicationFrame,
) -> Result<crate::publication::PublicationFrameReceipt, ConversationStoreError> {
    let stream_id = &start.stream_id;
    let failed:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM publication_frames INDEXED BY publication_frames_unindexed_idx WHERE stream_id=?1 AND observation_indexed=0)",[stream_id.as_str()],|row|row.get(0)).unwrap_or(true);
    let receipt = (!failed)
        .then(|| {
            optional_display_layout(transaction, || {
                content::index_frame(transaction, frame)?;
                content::frame_receipt(transaction, conversation_id, start, frame)
            })
        })
        .transpose()?
        .flatten()
        .unwrap_or_else(|| crate::publication::PublicationFrameReceipt {
            unavailable: true,
            stream_id: frame.stream_id.clone(),
            sequence: frame.sequence,
            content: Vec::new(),
            suffix_bytes: None,
            block_ordinal: None,
            block_total: 0,
            tool_ordinal: None,
            tool_total: 0,
        });
    transaction
            .execute(
                "INSERT INTO publication_frames(stream_id,sequence,frame_json,observation_indexed) VALUES(?1,?2,?3,?4)",
                params![
                    stream_id.as_str(),
                    seq_to_i64(frame.sequence)?,
                    encode(frame, "publication frame")?,
                    !receipt.unavailable
                ],
            )
            .map_err(|error| storage(format!("stage publication frame: {error}")))?;
    Ok(receipt)
}

pub(super) fn read_publication_frames(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<Vec<PublicationFrame>, ConversationStoreError> {
    let mut statement = transaction
        .prepare("SELECT frame_json FROM publication_frames WHERE stream_id=?1 ORDER BY sequence")
        .map_err(|error| storage(format!("publication frame query: {error}")))?;
    let rows = statement
        .query_map([stream_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(|error| storage(format!("publication frame rows: {error}")))?;
    let mut frames = Vec::new();
    for row in rows {
        let json = row.map_err(|error| storage(format!("publication frame row: {error}")))?;
        frames.push(decode(&json, "publication frame")?);
    }
    Ok(frames)
}

pub(super) fn clear_publication_staging(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "DELETE FROM publication_frames WHERE stream_id=?1",
            [stream_id.as_str()],
        )
        .map_err(|error| storage(format!("clear publication staging: {error}")))?;
    Ok(())
}

/// Whether **P** is durable for one exact provider request.
pub(super) fn request_outcome_is_durable(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    snapshot: &RequestSnapshot,
    completed_sequence: Option<u64>,
) -> Result<bool, ConversationStoreError> {
    // The caller has already validated this exact immutable row and its start
    // in this transaction, before generation/envelope checks. Reuse its P marker.
    let request_id = &snapshot.request_id;
    let Some(completed_sequence) = completed_sequence else {
        return Ok(false);
    };
    let event_json: String = transaction
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [seq_to_i64(completed_sequence)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("request outcome lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "request {request_id} completed marker has no Event Journal fact"
            ))
        })?;
    let event: RuntimeEventEnvelope = decode(&event_json, "request outcome")?;
    if event.sequence != completed_sequence
        || event.conversation_id != *conversation_id
        || event.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
        || event.turn_id.as_ref() != Some(&snapshot.identity.turn)
        || !matches!(
            &event.event,
            RuntimeEvent::ModelRequestCompleted {
                request_id: completed_request,
                ..
            } if completed_request == request_id
        )
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} completed marker does not identify its exact successful provider outcome"
        )));
    }
    Ok(true)
}

/// Validates the complete immutable identity of a publication stream against
/// the Request Snapshot that owns it.
pub(super) fn validate_publication_generation(
    snapshot: &RequestSnapshot,
    start: &PublicationStreamStart,
) -> Result<(), ConversationStoreError> {
    let expected_stream = PublicationStreamId::for_request(
        &snapshot.identity.attempt_id,
        &snapshot.provisional_message_id,
    );
    if start.request_id != snapshot.request_id
        || start.attempt_id != snapshot.identity.attempt_id
        || start.turn_id != snapshot.identity.turn
        || start.message_id != snapshot.provisional_message_id
        || start.stream_id != expected_stream
    {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "publication stream {} does not identify the exact Request Snapshot generation {}",
            start.stream_id, snapshot.request_id
        )));
    }
    Ok(())
}

/// Validates C's event envelope and message identity against the frozen
/// publication generation before the compound Ledger/Surface/Journal
/// transaction can mutate anything.
pub(super) fn validate_canonical_publication_event(
    conversation_id: &ConversationId,
    start: &PublicationStreamStart,
    message: &MessageBlock,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    validate_canonical_event_for_message(message, &event.event)?;
    if !matches!(message, MessageBlock::Assistant(_))
        || !matches!(
            &event.event,
            RuntimeEvent::AssistantMessageCommitted { message_id }
                if message_id == &start.message_id
        )
        || crate::conversation::message_id_of(message) != start.message_id
        || event.conversation_id != *conversation_id
        || event.attempt_id.as_ref() != Some(&start.attempt_id)
        || event.turn_id.as_ref() != Some(&start.turn_id)
    {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "canonical acceptance of publication stream {} does not identify its exact Assistant generation",
            start.stream_id
        )));
    }
    Ok(())
}
