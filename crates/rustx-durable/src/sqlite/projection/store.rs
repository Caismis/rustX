//! The `ProjectionStore` implementation over the projection, compaction, and lineage readers.

use crate::sqlite::native_request;

#[cfg(any(test, feature = "test-hooks"))]
use super::super::CompactionFaultOperation;
use crate::process_death;
use crate::sqlite::request::snapshot::load_request_snapshot_bounded;
use crate::sqlite::{
    SqliteConversationStore, compaction_resources, lineage, projection, provenance, reading,
    seq_to_i64, storage,
};
use crate::{ConversationStoreError, ProjectionStore};
use rusqlite::OptionalExtension;
use rusqlite::{TransactionBehavior, params};
use rustx_conversation::SurfaceRevision;
use rustx_types::events::types::RuntimeEventEnvelope;
use rustx_types::message::types::MessageBlock;
use rustx_types::runtime::identity::MessageId;
#[cfg(any(test, feature = "test-hooks"))]
use std::sync::atomic::Ordering;

impl ProjectionStore for SqliteConversationStore {
    fn compaction_resource_cut(
        &self,
    ) -> Result<rustx_types::context::resources::ResourceReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| {
            compaction_resources::cut(transaction, &self.conversation_id)
        })
    }
    fn compaction_message_page(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        after: rustx_types::context::resources::LedgerFrontier,
        ids: Option<&[MessageId]>,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::context_resources::ResourcePage<MessageBlock>, ConversationStoreError> {
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
        cut: &rustx_types::context::resources::ResourceReadCut,
        source: &rustx_types::model::projected::CanonicalMessageRef,
        body_required: bool,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<rustx_types::context::checkpoint::CanonicalDependency, ConversationStoreError> {
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
        cut: &rustx_types::context::resources::ResourceReadCut,
        after: rustx_types::context::resources::LedgerFrontier,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<
        crate::context_resources::ResourcePage<crate::context_resources::TrajectoryMessage>,
        ConversationStoreError,
    > {
        self.read_transaction(|transaction| {
            compaction_resources::trajectory(transaction, &self.conversation_id, cut, after, limits)
        })
    }
    fn compaction_event_page(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        after: rustx_types::context::resources::JournalFrontier,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::context_resources::ResourcePage<RuntimeEventEnvelope>, ConversationStoreError>
    {
        self.read_transaction(|transaction| {
            compaction_resources::events(transaction, &self.conversation_id, cut, after, limits)
        })
    }
    fn inherited_responses_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::response::CompletedResponseProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::selected_responses(&connection, messages)
    }
    fn load_inherited_turns(
        &self,
    ) -> Result<Vec<crate::reading::TurnReadingProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        reading::inherited_turns(&connection)
    }
    fn inherited_turns_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<crate::reading::TurnReadingProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::selected_turns(&connection, messages)
    }
    fn read_projection_lineage_cut(
        &self,
        ledger: rustx_types::context::resources::LedgerFrontier,
        activation: rustx_types::context::checkpoint::ProjectionActivationId,
    ) -> Result<crate::reading::LineageReadCut, ConversationStoreError> {
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
    ) -> Result<crate::reading::LineageReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| {
            lineage::read(transaction, &self.conversation_id, revision)
        })
    }
    fn projected_history(
        &self,
    ) -> Result<crate::projection::HistoricalModelInput, ConversationStoreError> {
        self.read_transaction(|transaction| {
            #[cfg(any(test, feature = "test-hooks"))]
            if Self::consume(&self.fail_projection_history_remaining)
                || self
                    .fail_projected_history_when
                    .lock()
                    .expect("projection read fault")
                    .as_ref()
                    .is_some_and(|probe| probe())
            {
                return Err(storage("fault injected: post-commit hot projection read"));
            }
            projection::selected_history(transaction, &self.conversation_id, &self.conn.manifest)
        })
    }
    fn auxiliary_history(
        &self,
    ) -> Result<
        (
            crate::projection::HistoricalModelInput,
            rustx_types::context::resources::ResourceReadCut,
        ),
        ConversationStoreError,
    > {
        self.read_transaction(|transaction| {
            projection::auxiliary_history(transaction, &self.conversation_id, &self.conn.manifest)
        })
    }

    fn projection_at_journal_cut(
        &self,
        through: u64,
    ) -> Result<Option<crate::projection::ProjectionCheckpoint>, ConversationStoreError> {
        self.read_transaction(|transaction| projection::at_journal_cut(transaction, through))
    }
    fn compaction_checkpoint_at(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        id: &rustx_types::runtime::identity::CheckpointId,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::projection::ProjectionCheckpoint, ConversationStoreError> {
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
                return Err(crate::context_resources::bound(
                    "checkpoint record exceeds resource materialization bound",
                ));
            }
            projection::checkpoint(transaction, id)
        })
    }
    fn compaction_request(
        &self,
        cut: &rustx_types::context::resources::ResourceReadCut,
        request: &rustx_types::runtime::identity::RequestId,
        limits: crate::context_resources::ResourceLimits,
    ) -> Result<crate::model::snapshot::RequestSnapshot, ConversationStoreError> {
        limits.validate()?;
        self.read_transaction(|transaction| {
            let sql = format!(
                "SELECT {} FROM request_snapshots LEFT JOIN request_snapshot_payloads USING(started_sequence) WHERE started_sequence=(SELECT start_sequence FROM request_owners WHERE request_id=?1) AND started_sequence<=?2",
                native_request::MATERIALIZED_BYTES_SQL
            );
            let bytes: Option<u64> = transaction
                .query_row(
                    &sql,
                    params![
                        request.as_str(),
                        seq_to_i64(cut.journal.0)?,
                        native_request::NORMALIZATION_OVERHEAD
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| storage(e.to_string()))?;
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
                return Err(crate::context_resources::bound(
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
    ) -> Result<Option<crate::projection::ProjectionCheckpoint>, ConversationStoreError> {
        self.read_transaction(|transaction| projection::head(transaction))
    }
    fn projection_checkpoint(
        &self,
        id: &rustx_types::runtime::identity::CheckpointId,
    ) -> Result<crate::projection::ProjectionCheckpoint, ConversationStoreError> {
        projection::checkpoint(&*self.lock()?, id)
    }
    fn projection_timeline(
        &self,
        after: rustx_types::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<Vec<crate::projection::ProjectionCheckpoint>, ConversationStoreError> {
        self.read_transaction(|transaction| projection::timeline(transaction, after, limit))
    }
    fn projection_timeline_at_journal_cut(
        &self,
        through: u64,
        after: rustx_types::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<crate::projection::ProjectionTimelinePage, ConversationStoreError> {
        self.read_transaction(|transaction| {
            projection::timeline_at_journal_cut(transaction, through, after, limit)
        })
    }
    fn projection_checkpoint_at_journal_cut(
        &self,
        through: u64,
        id: &rustx_types::runtime::identity::CheckpointId,
    ) -> Result<crate::projection::ProjectionCheckpoint, ConversationStoreError> {
        self.read_transaction(|transaction| {
            projection::checkpoint_at_journal_cut(transaction, through, id)
        })
    }
    fn projection_owner(
        &self,
        attempt: Option<&rustx_types::runtime::identity::AttemptId>,
    ) -> Result<crate::projection::ProjectionOwner, ConversationStoreError> {
        projection::owner(&*self.lock()?, attempt)
    }
    fn projection_receipt(
        &self,
        operation: &rustx_types::runtime::identity::CompactionOperationId,
        fingerprint: &str,
    ) -> Result<Option<crate::projection::ProjectionCommitReceipt>, ConversationStoreError> {
        projection::receipt(&*self.lock()?, operation, fingerprint)
    }
    fn compaction_restart_evidence(
        &self,
        operation: &rustx_types::runtime::identity::CompactionOperationId,
    ) -> Result<crate::projection::CompactionRestartEvidence, ConversationStoreError> {
        self.read_transaction(|transaction| {
            projection::restart_evidence(transaction, &self.conversation_id, operation)
        })
    }
    fn commit_projection(
        &self,
        input: crate::projection::ProjectionCommitInput,
    ) -> Result<crate::projection::ProjectionCommitReceipt, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| storage(e.to_string()))?;
        let result = projection::commit(
            &self.conn.event_guard,
            &transaction,
            &self.conversation_id,
            input,
            |stage| {
                #[cfg(any(test, feature = "test-hooks"))]
                if self.consume_compaction_fault(stage) || Self::consume(&self.fail_event_remaining)
                {
                    return Err(storage("fault injected: checkpoint transaction"));
                }
                #[cfg(not(test))]
                let _ = stage;
                Ok(())
            },
        )?;
        process_death::reach("before:commit_projection");
        transaction.commit().map_err(|e| storage(e.to_string()))?;
        process_death::reach("after:commit_projection");
        // Stage committed Journal facts and release SQLite before returning a receipt.
        drop(connection);
        #[cfg(any(test, feature = "test-hooks"))]
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
        #[cfg(any(test, feature = "test-hooks"))]
        for (stage, count) in [
            (CompactionFaultOperation::AfterCommitOneHotReadFailure, 1),
            (CompactionFaultOperation::AfterCommitTwoHotReadFailures, 2),
        ] {
            if self.consume_compaction_fault(stage) {
                self.fail_projection_history_remaining
                    .store(count, Ordering::SeqCst);
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterCommit) {
            return Err(storage("fault injected: lost checkpoint commit receipt"));
        }
        Ok(result)
    }
}

impl SqliteConversationStore {
    /// Select projection eligibility at the message boundary inside the same read transaction.
    pub fn read_message_lineage_cut(
        &self,
        revision: SurfaceRevision,
        message: &MessageId,
        include: bool,
    ) -> Result<crate::reading::LineageReadCut, ConversationStoreError> {
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
