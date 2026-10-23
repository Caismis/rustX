//! The `ProjectionStore` implementation over the projection, compaction, and lineage readers.

#[cfg(test)]
mod tests;

#[cfg(test)]
use super::super::CompactionFaultOperation;
use crate::conversation::SurfaceRevision;
use crate::durable::sqlite::request::snapshot::load_request_snapshot_bounded;
use crate::durable::sqlite::{
    SqliteConversationStore, compaction_resources, lineage, projection, provenance, reading,
    seq_to_i64, storage,
};
use crate::durable::{ConversationStoreError, ProjectionStore};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::runtime::identity::MessageId;
use crate::runtime::process_death;
use rusqlite::OptionalExtension;
use rusqlite::{TransactionBehavior, params};
#[cfg(test)]
use std::sync::atomic::Ordering;

impl ProjectionStore for SqliteConversationStore {
    fn compaction_resource_cut(
        &self,
    ) -> Result<crate::context::resources::ResourceReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| {
            compaction_resources::cut(transaction, &self.conversation_id)
        })
    }
    fn compaction_message_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        ids: Option<&[MessageId]>,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<MessageBlock>, ConversationStoreError> {
        self.read_transaction(|transaction| {
            compaction_resources::messages(
                transaction,
                &self.conversation_id,
                cut,
                after,
                ids,
                limits,
            )
        })
    }
    fn compaction_dependency(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        source: &crate::model::projected::CanonicalMessageRef,
        body_required: bool,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::checkpoint::CanonicalDependency, ConversationStoreError> {
        self.read_transaction(|transaction| {
            compaction_resources::dependency(
                transaction,
                &self.conversation_id,
                cut,
                source,
                body_required,
                limits,
            )
        })
    }
    fn compaction_trajectory_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<
        crate::context::resources::ResourcePage<crate::context::resources::TrajectoryMessage>,
        ConversationStoreError,
    > {
        self.read_transaction(|transaction| {
            compaction_resources::trajectory(transaction, &self.conversation_id, cut, after, limits)
        })
    }
    fn compaction_event_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::JournalFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<RuntimeEventEnvelope>, ConversationStoreError>
    {
        self.read_transaction(|transaction| {
            compaction_resources::events(transaction, &self.conversation_id, cut, after, limits)
        })
    }
    fn inherited_responses_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::durable::response::CompletedResponseProvenance>, ConversationStoreError>
    {
        let connection = self.lock()?;
        provenance::selected_responses(&connection, messages)
    }
    fn load_inherited_turns(
        &self,
    ) -> Result<Vec<crate::durable::reading::TurnReadingProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        reading::inherited_turns(&connection)
    }
    fn inherited_turns_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::durable::reading::TurnReadingProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::selected_turns(&connection, messages)
    }
    fn read_projection_lineage_cut(
        &self,
        ledger: crate::context::resources::LedgerFrontier,
        activation: crate::context::checkpoint::ProjectionActivationId,
    ) -> Result<crate::durable::reading::LineageReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| {
            lineage::read_at(
                transaction,
                &self.conversation_id,
                SurfaceRevision::new(ledger.0),
                Some(activation),
                None,
            )
        })
    }
    fn read_lineage_cut(
        &self,
        revision: SurfaceRevision,
    ) -> Result<crate::durable::reading::LineageReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| {
            lineage::read(transaction, &self.conversation_id, revision)
        })
    }
    fn projected_history(
        &self,
    ) -> Result<crate::durable::projection::HistoricalModelInput, ConversationStoreError> {
        self.read_transaction(|transaction| {
            #[cfg(test)]
            if Self::consume(&self.fail_projection_history_remaining) {
                return Err(storage("fault injected: post-commit hot projection read"));
            }
            projection::selected_history(transaction, &self.conversation_id, &self.conn.manifest)
        })
    }
    fn auxiliary_history(
        &self,
    ) -> Result<
        (
            crate::durable::projection::HistoricalModelInput,
            crate::context::resources::ResourceReadCut,
        ),
        ConversationStoreError,
    > {
        self.read_transaction(|transaction| {
            Ok((
                projection::selected_history(
                    transaction,
                    &self.conversation_id,
                    &self.conn.manifest,
                )?,
                compaction_resources::cut(transaction, &self.conversation_id)?,
            ))
        })
    }
    fn projection_at_journal_cut(
        &self,
        through: u64,
    ) -> Result<Option<crate::durable::projection::ProjectionCheckpoint>, ConversationStoreError>
    {
        self.read_transaction(|transaction| projection::at_journal_cut(transaction, through))
    }
    fn compaction_checkpoint_at(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        id: &crate::runtime::identity::CheckpointId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError> {
        limits.validate()?;
        if cut.conversation_id != self.conversation_id {
            return Err(ConversationStoreError::InvalidReference(
                "foreign checkpoint resource cut".into(),
            ));
        }
        self.read_transaction(|transaction| {
            compaction_resources::validate(transaction, &self.conversation_id, cut, limits)?;
            let bytes:Option<u64>=transaction.query_row(
                "SELECT octet_length(c.candidate_json)+octet_length(a.activation_json)+coalesce(octet_length(c.occupancy_json),0) FROM projection_checkpoints c JOIN projection_activations a ON a.checkpoint_id=c.checkpoint_id WHERE c.checkpoint_id=?1 AND a.activation<=?2",
                params![id.as_str(),seq_to_i64(cut.projection_activation.0)?],|row|row.get(0),
            ).optional().map_err(|e|storage(e.to_string()))?;
            let bytes = bytes.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "checkpoint is absent at Resource Read Cut".into(),
                )
            })?;
            if bytes > limits.record_bytes as u64 || bytes > limits.page_bytes as u64 {
                return Err(crate::context::resources::bound(
                    "checkpoint record exceeds resource materialization bound",
                ));
            }
            projection::checkpoint(transaction, id)
        })
    }
    fn compaction_request(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        request: &crate::runtime::identity::RequestId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::model::RequestSnapshot, ConversationStoreError> {
        limits.validate()?;
        self.read_transaction(|transaction| {
            let bytes: Option<u64> = transaction.query_row("SELECT octet_length(snapshot_json) FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),seq_to_i64(cut.journal.0)?],|row|row.get(0)).optional().map_err(|e|storage(e.to_string()))?;
            if cut.conversation_id != self.conversation_id {
                return Err(ConversationStoreError::InvalidReference(
                    "foreign frozen request Resource Read Cut".into(),
                ));
            }
            let bytes = bytes.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "request is absent at the Resource Read Cut".into(),
                )
            })?;
            if bytes > limits.record_bytes as u64 || bytes > limits.page_bytes as u64 {
                return Err(crate::context::resources::bound(
                    "historical frozen request exceeds materialization bound",
                ));
            }
            load_request_snapshot_bounded(
                transaction,
                &self.conversation_id,
                request,
                limits.record_bytes.min(limits.page_bytes),
            )
        })
    }
    fn projection_head(
        &self,
    ) -> Result<Option<crate::durable::projection::ProjectionCheckpoint>, ConversationStoreError>
    {
        self.read_transaction(|transaction| projection::head(transaction))
    }
    fn projection_checkpoint(
        &self,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError> {
        projection::checkpoint(&*self.lock()?, id)
    }
    fn projection_timeline(
        &self,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<Vec<crate::durable::projection::ProjectionCheckpoint>, ConversationStoreError> {
        self.read_transaction(|transaction| projection::timeline(transaction, after, limit))
    }
    fn projection_timeline_at_journal_cut(
        &self,
        through: u64,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<crate::durable::projection::ProjectionTimelinePage, ConversationStoreError> {
        self.read_transaction(|transaction| {
            projection::timeline_at_journal_cut(transaction, through, after, limit)
        })
    }
    fn projection_checkpoint_at_journal_cut(
        &self,
        through: u64,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<crate::durable::projection::ProjectionCheckpoint, ConversationStoreError> {
        self.read_transaction(|transaction| {
            projection::checkpoint_at_journal_cut(transaction, through, id)
        })
    }
    fn projection_owner(
        &self,
        attempt: Option<&crate::runtime::identity::AttemptId>,
    ) -> Result<crate::durable::projection::ProjectionOwner, ConversationStoreError> {
        projection::owner(&*self.lock()?, attempt)
    }
    fn projection_receipt(
        &self,
        operation: &crate::runtime::identity::CompactionOperationId,
        fingerprint: &str,
    ) -> Result<Option<crate::durable::projection::ProjectionCommitReceipt>, ConversationStoreError>
    {
        projection::receipt(&*self.lock()?, operation, fingerprint)
    }
    fn commit_projection(
        &self,
        input: crate::durable::projection::ProjectionCommitInput,
    ) -> Result<crate::durable::projection::ProjectionCommitReceipt, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| storage(e.to_string()))?;
        let result = projection::commit(&transaction, &self.conversation_id, input, |stage| {
            #[cfg(test)]
            if self.consume_compaction_fault(stage) || Self::consume(&self.fail_event_remaining) {
                return Err(storage("fault injected: checkpoint transaction"));
            }
            #[cfg(not(test))]
            let _ = stage;
            Ok(())
        })?;
        process_death::reach("before:commit_projection");
        transaction.commit().map_err(|e| storage(e.to_string()))?;
        process_death::reach("after:commit_projection");
        // Stage committed Journal facts and release SQLite before returning a receipt.
        drop(connection);
        #[cfg(test)]
        {
            let probe = self
                .projection_commit_return_probe
                .lock()
                .expect("projection return probe")
                .take();
            if let Some(probe) = probe {
                probe();
            }
        }
        #[cfg(test)]
        for (stage, count) in [
            (CompactionFaultOperation::AfterCommitOneHotReadFailure, 1),
            (CompactionFaultOperation::AfterCommitTwoHotReadFailures, 2),
        ] {
            if self.consume_compaction_fault(stage) {
                self.fail_projection_history_remaining
                    .store(count, Ordering::SeqCst);
            }
        }
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterCommit) {
            return Err(storage("fault injected: lost checkpoint commit receipt"));
        }
        Ok(result)
    }
}

impl SqliteConversationStore {
    /// Select projection eligibility at the message boundary inside the same read transaction.
    pub(crate) fn read_message_lineage_cut(
        &self,
        revision: SurfaceRevision,
        message: &MessageId,
        include: bool,
    ) -> Result<crate::durable::reading::LineageReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| {
            lineage::read_at(
                transaction,
                &self.conversation_id,
                revision,
                None,
                Some((message, include)),
            )
        })
    }
}
