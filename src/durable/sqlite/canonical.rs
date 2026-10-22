//! The canonical Message Ledger, Surface, and transcript domain.

#[cfg(test)]
mod tests;

use crate::durable::ProjectionStore;
use rusqlite::OptionalExtension;
use sha2::Digest;
pub(super) mod ledger;
pub(super) mod transcript;

#[cfg(test)]
use self::ledger::append_message_ledger;
use self::ledger::{
    append_seeded_ledger_message, append_surface_op, append_transcript_reference, apply_surface_op,
    ensure_surface_head, load_head, load_message, load_surface_history, load_transcript_reference,
    reconstruct_surface, replay_surface_history, update_surface_head,
    validate_surface_operation_references,
};
use self::transcript::{
    inherited_transcript_through, load_canonical_rows, load_transcript_item, load_transcript_page,
    load_transcript_tools, load_user_message_boundaries, load_user_message_boundaries_page,
};
use super::journal::{
    persist_event_tx, persist_event_tx_deferred, validate_canonical_event_for_message,
};
use super::publication::proposal::record_tool_proposal_dependency;
use super::{
    SqliteConversationStore, append_validation, content, decode, encode, journal_prefix,
    nonnegative, observation, observation_members, observation_occupancy, observation_readiness,
    observation_receipt, observation_state, observation_statistics, observation_tools,
    observation_trace, observation_turn, optional_display_layout, projection, provenance, reading,
    seq_to_i64, storage,
};
use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::durable::{
    CanonicalMessagePage, CanonicalStore, ConversationStoreError, DurableConversationHead,
    LineageSeed, SurfaceUserMessageBoundary, SurfaceUserMessageBoundaryPage,
    TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT, TranscriptCommitReceipt, TranscriptCursor, TranscriptEntry,
    TranscriptItem, TranscriptPage,
};
use crate::events::types::RuntimeEventEnvelope;
#[cfg(test)]
use crate::message::types::AssistantContentBlock;
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::runtime::identity::{ConversationId, EventId, MessageId, ToolCallId};
use crate::runtime::process_death;
use chrono::Utc;
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::Sha256;
use std::collections::BTreeSet;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;

impl CanonicalStore for SqliteConversationStore {
    fn transcript_tool_headers(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::NativeToolObservationPage, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_tools::page(
            &connection,
            &self.conversation_id,
            cut,
            locator,
            offset,
            limit,
        )
    }
    fn is_initialized(&self) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        let (initialized, identity): (i64, Option<i64>) = connection
            .prepare_cached("SELECT s.bootstrap_initialized,b.id FROM rustx_store s LEFT JOIN bootstrap_identity b ON b.id=s.id WHERE s.id=1")
            .map_err(|error| storage(format!("bootstrap state: {error}")))?
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| storage(format!("bootstrap state: {error}")))?;
        match (initialized, identity) {
            (0, None) => Ok(false),
            (1, Some(1)) => Ok(true),
            _ => Err(storage(
                "missing or inconsistent immutable bootstrap identity",
            )),
        }
    }
    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError> {
        let bootstrap: Option<(u64, String)> = self
            .lock()?
            .query_row(
                "SELECT message_count,history_digest FROM bootstrap_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap count: {error}")))?;
        let seed = if let Some((count, digest)) = bootstrap {
            if count != messages.len() as u64 || digest != initial_history_digest(messages)? {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
            // Runtime reopening supplies canonical bootstrap bodies, not a new
            // Surface. Reconstruct the original immutable seed, never flatten it.
            let history = self.load_surface_history(SurfaceRevision::new(count))?;
            LineageSeed::replayed(messages.to_vec(), history)?
        } else {
            LineageSeed::history(messages.to_vec())
        };
        let mut inherited = Vec::new();
        let mut after = crate::context::checkpoint::ProjectionActivationId(0);
        loop {
            let page = self.projection_timeline(after, 64)?;
            if page.is_empty() {
                break;
            }
            after = page.last().unwrap().activation.activation;
            inherited.extend(
                page.into_iter()
                    .filter(|p| p.activation.destination_installation.is_some()),
            );
        }
        let seed = seed.with_projections(inherited)?;
        let auxiliary_audits = {
            let connection = self.lock()?;
            projection::load_inherited_auxiliary_audits(&connection)?
        };
        self.initialize_lineage(
            &seed
                .with_completed_responses(self.load_inherited_responses()?)?
                .with_turns(self.load_inherited_turns()?)?
                .with_auxiliary_audits(auxiliary_audits)?,
        )
    }
    #[allow(clippy::too_many_lines)] // One atomic bootstrap owns Ledger, Surface and inherited facts.
    fn initialize_lineage(&self, seed: &LineageSeed) -> Result<(), ConversationStoreError> {
        let validated =
            LineageSeed::replayed(seed.canonical().to_vec(), seed.surface_history().to_vec())?
                .with_completed_responses(seed.completed_responses().to_vec())?
                .with_turns(seed.turns().to_vec())?
                .with_projections(seed.projections().to_vec())?
                .with_auxiliary_audits(seed.auxiliary_audits().to_vec())?
                .with_raw_tool_arguments(seed.raw_tool_arguments().to_vec())?;
        let seed = &validated;
        let messages = seed.canonical();
        // Semantics belong to the Context Engine; persistence has no alternate selection rule.
        let occupancy =
            crate::context::occupancy::inherit(seed.turns(), seed.canonical(), seed.projections());
        let occupancy_json = encode(&occupancy, "bootstrap occupancy")?;
        let history_digest = initial_history_digest(messages)?;
        let occupancy_digest =
            inherited_occupancy_digest(messages.len() as u64, &history_digest, &occupancy_json);
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("initialize transaction: {error}")))?;
        // Fail closed if initialized metadata is missing or damaged. The marker
        // and identity are written in this same transaction, never independently.
        read_inherited_occupancy(&transaction)?;
        let bootstrap: Option<(i64, String, String, String, u64, String)> = transaction
            .query_row(
                "SELECT message_count,history_digest,occupancy_json,occupancy_digest,auxiliary_audit_count,auxiliary_audit_digest FROM bootstrap_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap probe: {error}")))?;
        if let Some((
            count,
            digest,
            retained_occupancy,
            retained_digest,
            auxiliary_audit_count,
            auxiliary_audit_digest,
        )) = bootstrap
        {
            let bootstrap_revision = SurfaceRevision::new(
                u64::try_from(count).map_err(|_| storage("negative bootstrap message count"))?,
            );
            projection::verify_lineage(&transaction, seed.projections())?;
            projection::verify_inherited_auxiliary_audits(&transaction, seed.auxiliary_audits())?;
            projection::verify_auxiliary_lineage_bootstrap(
                &transaction,
                auxiliary_audit_count,
                &auxiliary_audit_digest,
            )?;
            if replay_surface_history(&transaction, bootstrap_revision)?.1 != seed.surface_history()
            {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
            let supplied_count = i64::try_from(messages.len())
                .map_err(|_| storage("initial message count is not representable"))?;
            if retained_occupancy != occupancy_json
                || retained_digest != occupancy_digest
                || count != supplied_count
                || digest != history_digest
                || provenance::responses(&transaction)? != seed.completed_responses()
                || provenance::turns(&transaction)? != seed.turns()
            {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
            provenance::verify_seed_membership(&transaction, seed)?;
        } else {
            let existing: i64 = transaction
                .query_row("SELECT COUNT(*) FROM message_ledger", [], |row| row.get(0))
                .map_err(|error| storage(format!("bootstrap ledger probe: {error}")))?;
            if existing != 0 {
                return Err(storage(
                    "canonical Ledger exists without bootstrap identity",
                ));
            }
            ensure_surface_head(&transaction)?;
            // The two parts of the seed go to their two durable homes, in two
            // passes rather than one, because they are not the same order:
            // the Ledger is written in canonical commit order, and a
            // compaction summary the source produced *last* is the fact its
            // Surface shows *first*. Fusing the passes would force one order
            // on both and reproduce neither.
            let append_terminals =
                |predecessor: Option<&MessageId>| -> Result<(), ConversationStoreError> {
                    for (index, turn) in seed.turns().iter().enumerate() {
                        if turn.process_message_ids.is_empty()
                            && turn.preceding_message_id.as_ref() == predecessor
                        {
                            append_transcript_reference(
                                &transaction,
                                "inherited_turn",
                                &index.to_string(),
                            )?;
                        }
                    }
                    Ok(())
                };
            append_terminals(None)?;
            for message in messages {
                append_seeded_ledger_message(&transaction, message)?;
                append_terminals(Some(&crate::conversation::message_id_of(message)))?;
            }
            for item in seed.raw_tool_arguments() {
                content::index_raw_arguments(&transaction, item)?;
                let message = messages
                    .iter()
                    .find(|message| message.id() == &item.message_id)
                    .ok_or_else(|| storage("raw Tool canonical source missing"))?;
                let position: u64 = transaction
                    .query_row(
                        "SELECT position FROM message_ledger WHERE message_id=?1",
                        [item.message_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|error| storage(error.to_string()))?;
                let _ = optional_display_layout(&transaction, || {
                    observation::index::index_message(&transaction, message, Some(position))?;
                    observation_trace::index_proposals(
                        &transaction,
                        &self.conversation_id,
                        position,
                        message,
                    )?;
                    observation_trace::index_canonical(
                        &transaction,
                        &self.conversation_id,
                        position,
                        message,
                    )
                })?;
            }
            // The seed's Surface history becomes this lineage's own retained
            // operation log, replayed from revision 1. Flattening it into one
            // append per active message would produce the same Surface and a
            // different history, and a later fork or tree of *this* lineage
            // reads the history: its branch points would then be this copy's
            // artefacts rather than the source's real ones.
            let mut active: Vec<MessageId> = Vec::new();
            let mut revision = SurfaceRevision::INITIAL;
            let generation = 0_u64;
            for operation in seed.surface_history() {
                revision = revision.next();
                validate_surface_operation_references(&transaction, operation)?;
                append_surface_op(&transaction, revision, generation, operation)?;
                apply_surface_op(&mut active, operation)?;
            }
            if revision != SurfaceRevision::INITIAL {
                update_surface_head(&transaction, revision, generation)?;
            }
            transaction
                .execute(
                    "INSERT INTO bootstrap_identity(id,message_count,history_digest,turn_count,response_count,occupancy_json,occupancy_digest,provenance_digest,inherited_transcript_through) VALUES(1,?1,?2,?3,?4,?5,?6,?7,(SELECT next_transcript_position FROM rustx_store WHERE id=1))",
                    params![
                        i64::try_from(messages.len()).map_err(|_| storage("initial message count is not representable"))?,
                        history_digest,
                        seq_to_i64(seed.turns().len() as u64)?,
                        seq_to_i64(seed.completed_responses().len() as u64)?,
                        occupancy_json,
                        occupancy_digest,
                        provenance::counts_digest(&history_digest,seq_to_i64(seed.turns().len() as u64)?,seq_to_i64(seed.completed_responses().len() as u64)?)
                    ],
                )
                .map_err(|error| storage(format!("insert bootstrap identity: {error}")))?;
            let _ = optional_display_layout(&transaction, || {
                observation_statistics::seed(&transaction, seed.turns())
            })?;
            let _ = optional_display_layout(&transaction, || {
                observation_occupancy::inherited(&transaction, occupancy.as_ref())
            })?;
            provenance::persist(&transaction, seed)?;
            projection::install_inherited_auxiliary_audits(&transaction, seed.auxiliary_audits())?;
            let (auxiliary_audit_count, auxiliary_audit_digest) =
                projection::auxiliary_lineage_bootstrap(&transaction)?;
            transaction.execute(
                "UPDATE bootstrap_identity SET auxiliary_audit_count=?1,auxiliary_audit_digest=?2 WHERE id=1",
                params![seq_to_i64(auxiliary_audit_count)?, auxiliary_audit_digest],
            ).map_err(|error| storage(format!("install inherited auxiliary audit witness: {error}")))?;

            // The original identity and provenance now exist together. Derived
            // readers validate this same transaction's complete bootstrap shape;
            // no external reader sees the marker before COMMIT.
            projection::install_lineage(&transaction, &self.conversation_id, seed.projections())?;
            transaction
                .execute(
                    "UPDATE rustx_store SET bootstrap_initialized=1 WHERE id=1",
                    [],
                )
                .map_err(|error| storage(format!("mark lineage bootstrap: {error}")))?;
            let _ = optional_display_layout(&transaction, || {
                observation_members::seed_members(&transaction, seed)
            })?;
            let _ = optional_display_layout(&transaction, || {
                observation_turn::seed(&transaction, seed)
            })?;
            for (ordinal, turn) in seed.turns().iter().enumerate() {
                let _ = optional_display_layout(&transaction, || {
                    reading::index_inherited_turn(&transaction, seq_to_i64(ordinal as u64)?, turn)
                })?;
            }
            for (ordinal, turn) in seed.turns().iter().enumerate() {
                if turn.process_message_ids.is_empty() {
                    let id = ordinal.to_string();
                    if let Some(cursor) =
                        load_transcript_reference(&transaction, "inherited_turn", &id)?
                    {
                        let _ = optional_display_layout(&transaction, || {
                            observation::index::index_item(
                                &transaction,
                                "inherited_turn",
                                &id,
                                TranscriptItem::InheritedTurn {
                                    provenance: turn.clone(),
                                },
                                cursor,
                            )
                        })?;
                    }
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("initialize commit: {error}")))
    }
    fn load_inherited_responses(
        &self,
    ) -> Result<Vec<crate::durable::response::CompletedResponseProvenance>, ConversationStoreError>
    {
        let connection = self.lock()?;
        provenance::responses(&connection)
    }
    fn load_inherited_occupancy(
        &self,
    ) -> Result<Option<crate::context::occupancy::ContextOccupancy>, ConversationStoreError> {
        let connection = self.lock()?;
        read_inherited_occupancy(&connection)
    }
    fn load_bootstrap_history(&self) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        let count: Option<i64> = connection
            .query_row(
                "SELECT message_count FROM bootstrap_identity WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap history count: {error}")))?;
        let Some(count) = count else {
            // A low-level non-session composition may be opening a fresh
            // store. ConversationRuntime::initialize will establish the
            // empty bootstrap identity; no historical seed exists yet.
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare(
                "SELECT message_json FROM message_ledger
                 ORDER BY position LIMIT ?1",
            )
            .map_err(|error| storage(format!("bootstrap history query: {error}")))?;
        let rows = statement
            .query_map([count], |row| row.get::<_, String>(0))
            .map_err(|error| storage(format!("bootstrap history rows: {error}")))?;
        rows.map(|row| {
            let json = row.map_err(|error| storage(format!("bootstrap history row: {error}")))?;
            decode(&json, "bootstrap history")
        })
        .collect()
    }
    fn unsettled_canonical_tool_calls(
        &self,
        limit: usize,
    ) -> Result<
        Vec<(crate::message::types::ToolCallOccurrenceRef, ToolCallId)>,
        ConversationStoreError,
    > {
        if limit == 0 || limit > 8192 {
            return Err(crate::context::resources::bound(
                "canonical unsettled occurrence read limit",
            ));
        }
        let connection = self.lock()?;
        let mut query = connection.prepare("SELECT CASE WHEN octet_length(assistant_message_id)<=4096 THEN assistant_message_id END, block_index, CASE WHEN octet_length(call_id)<=4096 THEN call_id END FROM canonical_tool_calls WHERE result_message_id IS NULL ORDER BY assistant_message_id,block_index LIMIT ?1").map_err(|e| storage(e.to_string()))?;
        let rows = query
            .query_map([limit], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| storage(e.to_string()))?;
        rows.map(|row| {
            let (id, block, wire) = row.map_err(|e| storage(e.to_string()))?;
            Ok((
                crate::message::types::ToolCallOccurrenceRef::new(
                    MessageId::new(id.ok_or_else(|| {
                        crate::context::resources::bound(
                            "canonical occurrence identity exceeds bound",
                        )
                    })?),
                    ContentBlockIndex::new(block),
                ),
                ToolCallId::new(wire.ok_or_else(|| {
                    crate::context::resources::bound("canonical wire correlation exceeds bound")
                })?),
            ))
        })
        .collect()
    }
    fn load_head(&self) -> Result<DurableConversationHead, ConversationStoreError> {
        self.read_transaction(|transaction| {
            #[cfg(test)]
            if let Some(probe) = self.head_read_probe.lock().unwrap().take() {
                transaction
                    .query_row(
                        "SELECT schema_version FROM rustx_store WHERE id=1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| storage(error.to_string()))?;
                probe();
            }
            load_head(transaction)
        })
    }
    fn messages_are_human_inputs(&self, ids: &[MessageId]) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        let mut query = connection
            .prepare_cached(
                "SELECT json_extract(message_json, '$.id'),
                    json_extract(message_json, '$.role') = 'user'
                    AND json_extract(message_json, '$.source') = 'human'
                    AND json_extract(message_json, '$.kind') = 'message'
             FROM message_ledger WHERE message_id = ?1",
            )
            .map_err(|error| storage(format!("Ledger input authorship: {error}")))?;
        for id in ids {
            let (stored_id, human): (String, bool) = query
                .query_row([id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(|error| storage(format!("Ledger input authorship: {error}")))?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!("message {id} is unavailable"))
                })?;
            if stored_id != id.as_str() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Ledger row for {id} contains a different message identity"
                )));
            }
            if !human {
                return Ok(false);
            }
        }
        Ok(!ids.is_empty())
    }
    fn load_messages(
        &self,
        ids: &[MessageId],
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        #[cfg(test)]
        self.checkpoint_read_counts[0].fetch_add(ids.len(), Ordering::SeqCst);
        let connection = self.lock()?;
        ids.iter().map(|id| load_message(&connection, id)).collect()
    }
    fn message_transcript_cursor(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let position: Option<i64> = self.lock()?.query_row(
            "SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",
            [message_id.as_str()], |row| row.get(0),
        ).optional().map_err(|error| storage(format!("process control position: {error}")))?;
        position
            .map(|value| nonnegative(value, "process control position").map(TranscriptCursor::new))
            .transpose()
    }
    fn event_transcript_cursor(
        &self,
        event_id: &EventId,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let position: Option<i64> = self.lock()?.query_row(
            "SELECT position FROM transcript_order WHERE reference_kind='attempt_terminal' AND reference_id=?1",
            [event_id.as_str()], |row| row.get(0),
        ).optional().map_err(|error| storage(format!("terminal control position: {error}")))?;
        position
            .map(|value| nonnegative(value, "terminal control position").map(TranscriptCursor::new))
            .transpose()
    }
    fn message_append_revision_by_locator(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError> {
        let connection = self.lock()?;
        let position = content::canonical_position(&connection, locator)?;
        let revision: Option<u64> = connection
            .query_row(
                "SELECT first_append_revision FROM message_ledger WHERE position=?1",
                [position],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(revision.map(SurfaceRevision::new))
    }
    fn message_append_revision(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError> {
        let revision: Option<i64> = self.lock()?.query_row(
            "SELECT revision FROM surface_ops WHERE json_extract(op_json, '$.op') = 'append' AND json_extract(op_json, '$.message_id') = ?1 ORDER BY revision LIMIT 1",
            [message_id.as_str()], |row| row.get(0),
        ).optional().map_err(|error| storage(format!("message append revision: {error}")))?;
        revision
            .map(|value| nonnegative(value, "message append revision").map(SurfaceRevision::new))
            .transpose()
    }
    fn load_surface_history(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceOp>, ConversationStoreError> {
        let connection = self.lock()?;
        load_surface_history(&connection, through)
    }
    fn reconstruct_surface(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationStoreError> {
        #[cfg(test)]
        self.checkpoint_read_counts[1].fetch_add(1, Ordering::SeqCst);
        let connection = self.lock()?;
        reconstruct_surface(&connection, revision)
    }
    fn load_surface_snapshot(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        let ids = reconstruct_surface(&connection, revision)?;
        ids.iter().map(|id| load_message(&connection, id)).collect()
    }
    fn surface_boundary_header_page(
        &self,
        through: SurfaceRevision,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::SurfaceBoundaryHeaderPage, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::surface_boundaries(&connection, through, offset, limit)
    }
    fn surface_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::observation::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError> {
        let connection = self.lock()?;
        let position = content::canonical_position(&connection, locator)?;
        let id:Option<String>=connection.query_row("SELECT message_id FROM message_ledger WHERE ordinary_user=1 AND first_append_revision<=?1 AND position=?2",params![seq_to_i64(boundary_revision.get())?,position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        id.map(MessageId::new).ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "Surface boundary locator/revision mismatch".into(),
            )
        })
    }
    fn canonical_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::observation::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError> {
        let connection = self.lock()?;
        let position = content::canonical_position(&connection, locator)?;
        let id: Option<String> = connection
            .query_row(
                "SELECT message_id FROM message_ledger WHERE first_append_revision<=?1 AND position=?2",
                params![seq_to_i64(boundary_revision.get())?, position],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        id.map(MessageId::new).ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "canonical boundary locator/revision mismatch".into(),
            )
        })
    }
    fn load_user_message_boundaries(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceUserMessageBoundary>, ConversationStoreError> {
        let connection = self.lock()?;
        load_user_message_boundaries(&connection, through)
    }
    fn load_user_message_boundaries_page(
        &self,
        through: SurfaceRevision,
        offset: usize,
        limit: usize,
    ) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
        let connection = self.lock()?;
        load_user_message_boundaries_page(&connection, through, offset, limit)
    }
    fn append_canonical(
        &self,
        message: &MessageBlock,
    ) -> Result<TranscriptCommitReceipt, ConversationStoreError> {
        let mut receipts = append_canonical_messages(self, std::slice::from_ref(message))?;
        Ok(receipts.remove(0))
    }
    fn append_canonical_batch(
        &self,
        messages: &[MessageBlock],
    ) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
        append_canonical_messages(self, messages)
    }
    fn append_canonical_with_event(
        &self,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError> {
        validate_canonical_event_for_message(message, &event.event)?;
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event transaction: {error}")))?;
        ensure_surface_head(&transaction)?;

        let transcript_cursor = validation.append(&transaction, message, None)?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical event journal commit"));
        }
        process_death::reach_event("before:canonical", &event.event);
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        let mut receipt = canonical_observation_receipt(
            &transaction,
            &self.conversation_id,
            message,
            transcript_cursor,
            persisted.event.tool_occurrence.clone(),
        )?;
        set_canonical_range(
            &mut receipt,
            Some((persisted.event.sequence, persisted.event.sequence)),
        );
        persisted.event.observation = if matches!(message, MessageBlock::Tool(_)) {
            observation_receipt::tool_results(
                &transaction,
                &self.conversation_id,
                std::slice::from_ref(&persisted.event),
                std::slice::from_ref(message),
                Arc::clone(&receipt.observation),
            )
        } else {
            observation_receipt::canonical(
                &transaction,
                &self.conversation_id,
                &persisted.event,
                Arc::clone(&receipt.observation),
            )
        };
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach_event("after:canonical", &persisted.event.event);
        Ok((persisted.event, receipt))
    }
    fn append_canonical_batch_with_events(
        &self,
        messages: &[MessageBlock],
        events: &[RuntimeEventEnvelope],
    ) -> Result<(Vec<RuntimeEventEnvelope>, Vec<TranscriptCommitReceipt>), ConversationStoreError>
    {
        if messages.len() != events.len() {
            return Err(storage("canonical event batch lengths differ"));
        }
        for (message, event) in messages.iter().zip(events) {
            validate_canonical_event_for_message(message, &event.event)?;
        }
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event batch transaction: {error}")))?;
        ensure_surface_head(&transaction)?;

        let mut receipts = Vec::with_capacity(messages.len());
        for message in messages {
            receipts.push(validation.append(&transaction, message, None)?);
        }
        let mut persisted = Vec::with_capacity(events.len());
        for event in events {
            #[cfg(test)]
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage(
                    "fault injected: canonical batch event journal commit",
                ));
            }
            persisted.push(
                persist_event_tx_deferred(&transaction, &self.conversation_id, event.clone())?
                    .event,
            );
        }
        let range = persisted
            .first()
            .zip(persisted.last())
            .map(|(first, last)| (first.sequence, last.sequence));
        let receipts = canonical_observation_batch(
            &transaction,
            &self.conversation_id,
            &messages
                .iter()
                .zip(receipts)
                .map(|(message, cursor)| (message.id(), cursor))
                .collect::<Vec<_>>(),
            range,
        )?;
        let shared = receipts.first().and_then(|receipt| {
            observation_receipt::tool_results(
                &transaction,
                &self.conversation_id,
                &persisted,
                messages,
                Arc::clone(&receipt.observation),
            )
        });
        for event in &mut persisted {
            event.observation.clone_from(&shared);
        }
        process_death::reach("before:append_canonical_batch");
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event batch commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach("after:append_canonical_batch");
        Ok((persisted, receipts))
    }
    #[allow(clippy::too_many_lines)]
    fn load_canonical(&self) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        load_canonical_rows(&connection)
    }
    fn load_canonical_page(
        &self,
        after_position: Option<u64>,
        limit: usize,
    ) -> Result<CanonicalMessagePage, ConversationStoreError> {
        let connection = self.lock()?;
        let after = after_position.unwrap_or(0);
        let limit = i64::try_from(limit).map_err(|_| storage("page limit is too large"))?;
        let mut statement = connection
            .prepare("SELECT position,message_json,message_id FROM message_ledger WHERE position > ?1 ORDER BY position LIMIT ?2")
            .map_err(|error| storage(format!("canonical page: {error}")))?;
        let mut rows = statement
            .query(params![after, limit])
            .map_err(|error| storage(format!("canonical page query: {error}")))?;
        let mut messages = Vec::new();
        let mut next = None;
        while let Some(row) = rows
            .next()
            .map_err(|error| storage(format!("canonical page row: {error}")))?
        {
            let position: i64 = row
                .get(0)
                .map_err(|error| storage(format!("canonical position: {error}")))?;
            let json: String = row
                .get(1)
                .map_err(|error| storage(format!("canonical json: {error}")))?;
            let message: MessageBlock = decode(&json, "canonical page")?;
            let position_id = crate::conversation::message_id_of(&message);
            let stored_id: String = row
                .get(2)
                .map_err(|error| storage(format!("canonical message id: {error}")))?;
            if position_id.as_str() != stored_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Ledger page row {stored_id} contains message {position_id}"
                )));
            }
            messages.push(message);
            next =
                Some(u64::try_from(position).map_err(|_| storage("negative canonical position"))?);
        }
        Ok(CanonicalMessagePage {
            messages,
            next_position: next,
        })
    }
    fn load_inspection_seed(
        &self,
    ) -> Result<crate::durable::InspectionSeed, ConversationStoreError> {
        self.read_transaction(|transaction| {
            let head = load_head(transaction)?;
            let ids = reconstruct_surface(transaction, head.revision)?;
            Ok(crate::durable::InspectionSeed {
                messages: ids
                    .iter()
                    .map(|id| load_message(transaction, id))
                    .collect::<Result<_, _>>()?,
                canonical: load_canonical_rows(transaction)?,
                transcript: load_transcript_page(
                    transaction,
                    None,
                    TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT,
                )?,
                journal_through: journal_prefix(transaction)?,
            })
        })
    }
    fn load_transcript_snapshot(
        &self,
        cut: &crate::durable::TranscriptSnapshotCut,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        let connection = self.lock()?;
        let messages = serde_json::to_string(&cut.messages).map_err(|e| storage(e.to_string()))?;
        let publications =
            serde_json::to_string(&cut.publications).map_err(|e| storage(e.to_string()))?;
        // The bootstrap prefix is immutable canonical history. Subsequent
        // canonical membership comes from captured semantic receipts, never
        // the current ledger head. Pending rows are supplied below from C.
        let mut statement = connection
            .prepare(
                "SELECT t.position,t.reference_kind,t.reference_id FROM transcript_order t
             WHERE (t.reference_kind='message' AND EXISTS
               (SELECT 1 FROM message_ledger m WHERE m.message_id=t.reference_id)
               AND (t.position<=?1 OR t.reference_id IN (SELECT value FROM json_each(?2))))
             OR (t.reference_kind='publication_audit' AND
               (t.position<=?1 OR t.reference_id IN (SELECT value FROM json_each(?3))))
             OR (t.reference_kind='inherited_turn' AND t.position<=?1)
             OR (t.reference_kind IN ('interaction_event','attempt_terminal') AND EXISTS
               (SELECT 1 FROM events e WHERE e.event_id=t.reference_id AND e.sequence<=?4))
             ORDER BY t.position DESC LIMIT ?5",
            )
            .map_err(|e| storage(e.to_string()))?;
        let references = statement
            .query_map(
                params![
                    seq_to_i64(cut.bootstrap_through)?,
                    messages,
                    publications,
                    seq_to_i64(cut.journal_through)?,
                    TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT + 1
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|e| storage(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| storage(e.to_string()))?;
        let allowed: BTreeSet<_> = cut.messages.iter().map(MessageId::as_str).collect();
        let mut entries = Vec::new();
        for (position, kind, id) in references {
            let item = load_transcript_item(&connection, &kind, &id)?;
            let mut tool_calls = match &item {
                TranscriptItem::Message {
                    message: MessageBlock::Assistant(assistant),
                } => load_transcript_tools(&connection, assistant)?,
                _ => Vec::new(),
            };
            // Associations are mutable as later Tool results commit. Keep only
            // results whose canonical message belonged to this captured cut.
            for tool in &mut tool_calls {
                let result: Option<(String, i64)> = connection.query_row(
                    "SELECT m.message_id,t.position FROM canonical_tool_calls c
                     JOIN message_ledger m ON m.message_id=c.result_message_id
                     JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=m.message_id
                     WHERE c.assistant_message_id=?1 AND c.block_index=?2",
                    params![tool.message_id.as_str(), tool.block_index.get()],
                    |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e| storage(e.to_string()))?;
                if result.is_some_and(|(id, pos)| {
                    pos > i64::try_from(cut.bootstrap_through).expect("validated transcript prefix")
                        && !allowed.contains(id.as_str())
                }) {
                    tool.result = None;
                }
            }
            entries.push(TranscriptEntry {
                cursor: TranscriptCursor::new(nonnegative(position, "transcript position")?),
                item,
                tool_calls,
            });
        }
        // A later adoption/edit/removal cannot replace the pending body at C.
        for (cursor, message) in &cut.pending {
            entries.retain(|entry| entry.cursor != *cursor);
            entries.push(TranscriptEntry {
                cursor: *cursor,
                item: TranscriptItem::Message {
                    message: MessageBlock::User(message.clone()),
                },
                tool_calls: Vec::new(),
            });
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.cursor));
        let more = entries.len() > TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT;
        entries.truncate(TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT);
        let next_cursor = more.then(|| entries.last().unwrap().cursor);
        entries.reverse();
        Ok(TranscriptPage {
            inherited_through: inherited_transcript_through(&connection)?,
            entries,
            next_cursor,
        })
    }
    fn load_transcript_page(
        &self,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        self.read_transaction(|transaction| load_transcript_page(transaction, before, limit))
    }
}

pub(super) fn canonical_observation_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    message: &MessageBlock,
    transcript_cursor: Option<TranscriptCursor>,
    _tool_occurrence: Option<crate::events::types::ToolObservationOccurrence>,
) -> Result<TranscriptCommitReceipt, ConversationStoreError> {
    canonical_observation_receipt_for_id(
        connection,
        conversation,
        message.id(),
        transcript_cursor,
        None,
    )
}

pub(super) fn canonical_observation_receipt_for_id(
    connection: &Connection,
    conversation: &ConversationId,
    message_id: &MessageId,
    transcript_cursor: Option<TranscriptCursor>,
    _tool_occurrence: Option<crate::events::types::ToolObservationOccurrence>,
) -> Result<TranscriptCommitReceipt, ConversationStoreError> {
    let mut receipts = canonical_observation_batch(
        connection,
        conversation,
        &[(message_id, transcript_cursor)],
        None,
    )?;
    Ok(receipts.remove(0))
}

pub(super) fn set_canonical_range(
    receipt: &mut TranscriptCommitReceipt,
    range: Option<(u64, u64)>,
) {
    Arc::make_mut(&mut receipt.observation).journal_range = range;
}

pub(super) fn canonical_observation_batch(
    connection: &Connection,
    conversation: &ConversationId,
    items: &[(&MessageId, Option<TranscriptCursor>)],
    journal_range: Option<(u64, u64)>,
) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let through = journal_prefix(connection)?;
    let mut source_unavailable = false;
    let mut positions = Vec::with_capacity(items.len());
    for (id, _) in items {
        let position: u64 = connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        source_unavailable |= connection.execute("UPDATE observation_headers SET journal_position=?1,header_json=CAST(json_set(header_json,'$.transcript_cursor',transcript_position) AS BLOB) WHERE ledger_position=?2",params![through,position]).is_err();
        positions.push(position);
    }
    let first_position = *positions.first().expect("nonempty canonical batch");
    let last_position = *positions.last().expect("nonempty canonical batch");
    let read_cut = observation::cut(connection, conversation)?;
    let head = canonical_display_head(connection, conversation, &read_cut, &mut source_unavailable);
    let scalars = complete_canonical_scalars(connection, &read_cut).unwrap_or_else(|_| {
        source_unavailable = true;
        CanonicalSourceScalars::default()
    });
    let batch = Arc::new(crate::durable::CanonicalObservationBatch {
        ready_through: observation_readiness::frontier(connection, 0, through).ok(),
        ready_t_through: observation_readiness::transcript_frontier(
            connection,
            0,
            read_cut.read.transcript,
        )
        .ok(),
        source_unavailable,
        todos: scalars.todos,
        read_cut,
        journal_range,
        first_position,
        last_position,
        count: items.len() as u64,
        first_transcript: items.iter().filter_map(|(_, cursor)| *cursor).min(),
        last_transcript: items.iter().filter_map(|(_, cursor)| *cursor).max(),
        transcript_count: items.iter().filter(|(_, cursor)| cursor.is_some()).count() as u64,
        head,
        human_activity_at: scalars.human_activity_at,
        has_opening_user: scalars.has_opening_user,
        opening_preview: scalars.opening_preview,
    });
    Ok(items
        .iter()
        .zip(positions)
        .enumerate()
        .map(
            |(ordinal, ((_, transcript_cursor), ledger_position))| TranscriptCommitReceipt {
                transcript_cursor: *transcript_cursor,
                ledger_position,
                ordinal: ordinal as u64,
                observation: Arc::clone(&batch),
            },
        )
        .collect())
}

#[derive(Default)]
pub(super) struct CanonicalSourceScalars {
    pub(super) human_activity_at: Option<chrono::DateTime<Utc>>,
    pub(super) has_opening_user: bool,
    pub(super) opening_preview: Option<String>,
    pub(super) todos: Option<crate::durable::NativeTodoObservation>,
}

pub(super) fn complete_canonical_scalars(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
) -> Result<CanonicalSourceScalars, ConversationStoreError> {
    let activity: Option<(i64,u32)> = connection.query_row(
        "SELECT human_activity_seconds,human_activity_nanos FROM message_ledger INDEXED BY message_ledger_human_activity_idx WHERE human_activity_at IS NOT NULL AND position<=?1 ORDER BY human_activity_seconds DESC,human_activity_nanos DESC LIMIT 1",
        [cut.ledger_through], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|error| storage(error.to_string()))?;
    let opening: Option<Option<String>> = connection.query_row(
        "SELECT opening_preview FROM message_ledger INDEXED BY message_ledger_opening_idx WHERE ordinary_user=1 AND position<=?1 ORDER BY position LIMIT 1",
        [cut.ledger_through], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    Ok(CanonicalSourceScalars {
        human_activity_at: activity
            .map(|(seconds, nanos)| {
                chrono::DateTime::from_timestamp(seconds, nanos)
                    .ok_or_else(|| storage("native human activity timestamp unavailable"))
            })
            .transpose()?,
        has_opening_user: opening.is_some(),
        opening_preview: opening.flatten(),
        todos: observation_state::todo_head(connection, 0, cut.ledger_through)?,
    })
}

pub(super) fn canonical_display_head(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &crate::durable::observation::ObservationReadCut,
    source_unavailable: &mut bool,
) -> Vec<crate::durable::observation::MessageHeader> {
    match observation::messages(
        connection,
        conversation,
        cut,
        Some(cut.ledger_through.saturating_sub(8)),
        8,
    ) {
        Ok(page) if !*source_unavailable => page.headers,
        _ => {
            *source_unavailable = true;
            Vec::new()
        }
    }
}

pub(super) fn append_canonical_messages(
    store: &SqliteConversationStore,
    messages: &[MessageBlock],
) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let mut connection = store.lock()?;
    let mut validation = append_validation::Batch::begin(&store.conn.append_validation);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("canonical transaction: {error}")))?;
    ensure_surface_head(&transaction)?;

    let mut receipts = Vec::with_capacity(messages.len());
    for message in messages {
        if let MessageBlock::Tool(tool) = message {
            record_tool_proposal_dependency(
                &transaction,
                &tool.tool_call_id,
                None,
                None,
                Some(&tool.tool_id),
                "canonical ToolMessage",
                true,
                Some(&tool.occurrence.assistant_message_id),
            )?;
        }
        receipts.push(validation.append(&transaction, message, None)?);
    }
    let receipts = canonical_observation_batch(
        &transaction,
        &store.conversation_id,
        &messages
            .iter()
            .zip(receipts)
            .map(|(message, cursor)| (message.id(), cursor))
            .collect::<Vec<_>>(),
        None,
    )?;
    transaction
        .commit()
        .map_err(|error| storage(format!("canonical commit: {error}")))?;
    validation.committed(&connection);
    Ok(receipts)
}

pub(super) fn ordinary_user_by_id(
    connection: &Connection,
    id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .prepare_cached("SELECT ordinary_user FROM message_ledger WHERE message_id=?1")
        .map_err(|error| storage(error.to_string()))?
        .query_row([id.as_str()], |row| row.get(0))
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(|error| storage(error.to_string()))
}

/// Seed irrelevant text-Assistant history for read-cost regressions without
/// reconstructing and validating the whole growing Surface on every append.
/// Native Ledger, Surface and Journal writers still own every persisted fact.
#[cfg(test)]
use crate::durable::ConversationStore;

#[cfg(test)]
pub(crate) fn append_assistant_history_fixture(
    store: &SqliteConversationStore,
    history: &[(
        crate::message::types::AssistantMessageBlock,
        RuntimeEventEnvelope,
    )],
) -> Result<(), ConversationStoreError> {
    let mut connection = store.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("Assistant history fixture: {error}")))?;
    ensure_surface_head(&transaction)?;
    let mut head = load_head(&transaction)?;
    for (assistant, event) in history {
        if !assistant
            .content
            .iter()
            .all(|block| matches!(block, AssistantContentBlock::Text(_)))
        {
            return Err(storage("the Assistant history fixture accepts only text"));
        }
        let message = MessageBlock::Assistant(assistant.clone());
        validate_canonical_event_for_message(&message, &event.event)?;
        append_message_ledger(&transaction, &message, None)?;
        head.active_message_ids.push(assistant.id.clone());
        head.revision = head.revision.next();
        append_surface_op(
            &transaction,
            head.revision,
            head.compaction_generation,
            &SurfaceOp::Append {
                message_id: assistant.id.clone(),
            },
        )?;
        persist_event_tx(&transaction, store.conversation_id(), event.clone())?;
    }
    update_surface_head(&transaction, head.revision, head.compaction_generation)?;
    // Verify the same final durable invariants as ordinary appends, once.
    load_head(&transaction)?;
    transaction
        .commit()
        .map_err(|error| storage(format!("Assistant history fixture commit: {error}")))
}

pub(super) const INHERITED_OCCUPANCY_SQL: &str = "SELECT s.bootstrap_initialized,b.message_count,b.history_digest,b.occupancy_json,b.occupancy_digest FROM rustx_store s LEFT JOIN bootstrap_identity b ON b.id=s.id WHERE s.id=1";

pub(super) fn inherited_occupancy_digest(
    count: u64,
    history_digest: &str,
    occupancy: &str,
) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(b"rustx-inherited-occupancy-v1\n");
    hasher.update(count.to_be_bytes());
    hasher.update(history_digest.as_bytes());
    hasher.update(occupancy.as_bytes());
    let mut digest = String::new();
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

pub(super) fn read_inherited_occupancy(
    connection: &Connection,
) -> Result<Option<crate::context::occupancy::ContextOccupancy>, ConversationStoreError> {
    let (initialized, count, history, json, digest): (
        i64,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(INHERITED_OCCUPANCY_SQL, [], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|error| storage(format!("bootstrap occupancy: {error}")))?;
    let (count, history, json, digest) = match (initialized, count, history, json, digest) {
        (0, None, None, None, None) => return Ok(None),
        (1, Some(count), Some(history), Some(json), Some(digest)) if count >= 0 => (
            u64::try_from(count).map_err(|_| storage("negative bootstrap message count"))?,
            history,
            json,
            digest,
        ),
        _ => {
            return Err(storage(
                "missing or inconsistent immutable bootstrap metadata",
            ));
        }
    };
    if digest != inherited_occupancy_digest(count, &history, &json) {
        return Err(storage("inconsistent immutable bootstrap occupancy"));
    }
    let reading: Option<crate::context::occupancy::ContextOccupancy> =
        decode(&json, "bootstrap occupancy")?;
    if let Some(reading) = &reading {
        reading.validate()?;
    }
    Ok(reading)
}

pub(super) fn initial_history_digest(
    messages: &[MessageBlock],
) -> Result<String, ConversationStoreError> {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(b"rustx-bootstrap-v2\n");
    for message in messages {
        let bytes = serde_json::to_vec(message)
            .map_err(|error| storage(format!("serialize bootstrap: {error}")))?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let mut digest = String::new();
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    Ok(digest)
}

impl SqliteConversationStore {
    #[cfg(test)]
    pub(crate) fn canonical_scalar_sources_match(
        &self,
        expected: &crate::durable::CanonicalObservationBatch,
    ) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, &expected.read_cut)?;
        let actual = complete_canonical_scalars(&connection, &expected.read_cut)?;
        Ok(actual.human_activity_at == expected.human_activity_at
            && actual.has_opening_user == expected.has_opening_user
            && actual.opening_preview == expected.opening_preview
            && actual.todos == expected.todos)
    }
}
