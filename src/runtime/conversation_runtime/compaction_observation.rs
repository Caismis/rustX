//! Durable asynchronous compaction facts published without manual/Attempt ownership.

use super::RuntimeInner;
use crate::context::ContextError;
use crate::events::types::RuntimeEvent;

/// Reconcile only bounded indexed native operation statuses after the prior
/// process has lost ownership. No Summary worker or request is reconstructed.
pub(super) fn reconcile_compaction_restart(
    store: &dyn crate::durable::ConversationStore,
    clock: &dyn crate::runtime::types::RuntimeClock,
) -> Result<(), crate::durable::ConversationStoreError> {
    use crate::context::operation::CompactionOperationPhase;
    let cut = store.compaction_resource_cut()?;
    let statuses = store.latest_compaction_operations(cut.journal.0)?;
    for mut status in statuses
        .into_iter()
        .filter(|status| !status.phase.is_terminal())
    {
        let evidence = store.compaction_restart_evidence(&status.operation_id)?;
        if evidence.accepted_fragments < status.accepted_fragments {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "compaction status exceeds its durable accepted fragments".into(),
            ));
        }
        status.accepted_fragments = evidence.accepted_fragments;
        status.waiting_attempt_id = None;
        if evidence.committed {
            status.phase = CompactionOperationPhase::Committed;
            status.diagnostic = None;
        } else {
            status.phase = CompactionOperationPhase::NoEffect;
            status.diagnostic =
                Some("interrupted by runtime restart; intermediate Summary was not resumed".into());
        }
        store.append_event(crate::events::types::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: clock.now(),
            event: RuntimeEvent::CompactionOperationUpdated { status },
        })?;
    }
    Ok(())
}

impl RuntimeInner {
    /// Publish the initial native lifecycle and status as one durable fact pair.
    pub(super) fn publish_compaction_admission(
        &self,
        status: crate::context::operation::CompactionOperationStatus,
    ) -> Result<(), crate::context::compaction::CompactionExecutionError> {
        let timestamp = self.clock.now();
        let envelope = |event| crate::events::types::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(""),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: None,
            turn_id: None,
            timestamp,
            event,
        };
        let started = envelope(RuntimeEvent::CompactionStarted {
            operation_id: status.operation_id.clone(),
            mode: crate::context::operation::CompactionMode::Proactive,
        });
        let update = envelope(RuntimeEvent::CompactionOperationUpdated { status });
        let (started, update) = self
            .store
            .append_compaction_admission(started, update)
            .map_err(|error| {
                crate::context::compaction::CompactionExecutionError::Durable(error.to_string())
            })?;
        self.publish_async_compaction_receipt(&started);
        self.publish_async_compaction_receipt(&update);
        Ok(())
    }

    pub(super) fn publish_compaction_status(
        &self,
        status: crate::context::operation::CompactionOperationStatus,
    ) -> Result<(), ContextError> {
        status.validate().map_err(|message| {
            ContextError::new(crate::context::ContextErrorKind::Internal, message)
        })?;
        let persisted = self
            .store
            .append_event(crate::events::types::RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                event_id: crate::runtime::identity::EventId::new(""),
                sequence: 0,
                conversation_id: self.conversation_id.clone(),
                attempt_id: None,
                turn_id: None,
                timestamp: self.clock.now(),
                event: RuntimeEvent::CompactionOperationUpdated { status },
            })
            .map_err(|error| {
                ContextError::new(
                    crate::context::ContextErrorKind::AuxiliaryDurabilityFailed,
                    format!("independent compaction state could not be persisted: {error}"),
                )
            })?;
        self.publish_async_compaction_receipt(&persisted);
        Ok(())
    }

    pub(super) fn publish_async_compaction_receipt(
        &self,
        persisted: &crate::events::types::RuntimeEventEnvelope,
    ) {
        let Some(fanout) = self.pending.get() else {
            return;
        };
        let Some(batch) = persisted.observation.as_deref() else {
            fanout.source_failed();
            return;
        };
        if !fanout.native_batch(batch).is_installed() {
            fanout.source_failed();
            return;
        }
        if let Some(canonical) = &batch.canonical
            && self
                .store
                .mark_canonical_transcript_ready(canonical.first_position, canonical.last_position)
                .is_err()
        {
            fanout.source_failed();
            return;
        }
        if let Some((first, last)) = batch.journal_range {
            match self.store.mark_observation_direct(first, last) {
                Ok(()) => fanout.source_ready(last),
                Err(_) => fanout.source_failed(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::checkpoint::{CheckpointCandidate, CheckpointItem, CheckpointMessage};
    use crate::context::operation::{
        CompactionMode, CompactionOperationPhase, CompactionOperationStatus,
    };
    use crate::durable::{
        CanonicalStore, ConversationStore, JournalStore, ObservationStore, ProjectionStore,
        SqliteConversationStore,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::projected::CanonicalMessageRef;
    use crate::runtime::identity::{
        CheckpointId, CompactionOperationId, ConversationId, EventId, MessageId,
    };
    use crate::runtime::types::{SystemClock, TokenMeasurement, TokenMeasurementSource};

    fn envelope(
        store: &SqliteConversationStore,
        event: RuntimeEvent,
    ) -> crate::events::types::RuntimeEventEnvelope {
        crate::events::types::RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            event,
        }
    }

    fn status(
        id: &str,
        generation: u64,
        phase: CompactionOperationPhase,
    ) -> CompactionOperationStatus {
        CompactionOperationStatus {
            operation_id: CompactionOperationId::new(id),
            admission_generation: generation,
            triggering_attempt_id: None,
            waiting_attempt_id: None,
            phase,
            accepted_fragments: 0,
            diagnostic: None,
        }
    }

    fn admit(store: &SqliteConversationStore, value: CompactionOperationStatus) {
        let started = envelope(
            store,
            RuntimeEvent::CompactionStarted {
                operation_id: value.operation_id.clone(),
                mode: CompactionMode::Proactive,
            },
        );
        let status = envelope(
            store,
            RuntimeEvent::CompactionOperationUpdated { status: value },
        );
        store.append_compaction_admission(started, status).unwrap();
    }

    fn latest(store: &SqliteConversationStore) -> Vec<CompactionOperationStatus> {
        let cut = store.compaction_resource_cut().unwrap();
        store.latest_compaction_operations(cut.journal.0).unwrap()
    }

    fn message() -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new("recovery-source"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "source".into(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    #[test]
    fn restart_terminalizes_bounded_nonterminal_work_without_replay() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[message()]).unwrap();
        admit(
            &store,
            status("generating", 1, CompactionOperationPhase::Generating),
        );
        admit(
            &store,
            status("ready", 2, CompactionOperationPhase::Generating),
        );
        store
            .append_event(envelope(
                &store,
                RuntimeEvent::CompactionOperationUpdated {
                    status: status("ready", 2, CompactionOperationPhase::Ready),
                },
            ))
            .unwrap();
        reconcile_compaction_restart(&store, &SystemClock).unwrap();
        let recovered = latest(&store);
        assert_eq!(recovered.len(), 2);
        assert!(
            recovered
                .iter()
                .all(|item| item.phase == CompactionOperationPhase::NoEffect)
        );
        assert!(recovered.iter().all(|item| {
            item.diagnostic
                .as_deref()
                .is_some_and(|value| value.contains("not resumed"))
        }));
        let events = store.read_events(None, 64).unwrap().events;
        assert!(events.iter().all(|item| !matches!(
            item.event,
            RuntimeEvent::CompactionCompleted { .. } | RuntimeEvent::AuxiliaryCallStarted { .. }
        )));
        reconcile_compaction_restart(&store, &SystemClock).unwrap();
        assert_eq!(
            store.read_events(None, 64).unwrap().events,
            events,
            "recovery is idempotent"
        );
        admit(
            &store,
            status("next", 3, CompactionOperationPhase::Generating),
        );
        assert_eq!(latest(&store).last().unwrap().admission_generation, 3);
    }

    #[test]
    fn restart_recovers_committed_status_from_the_actual_atomic_receipt() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let original = message();
        store.initialize(std::slice::from_ref(&original)).unwrap();
        let initial = status(
            "committed-before-status",
            1,
            CompactionOperationPhase::Generating,
        );
        admit(&store, initial.clone());
        let cut = store.compaction_resource_cut().unwrap();
        let candidate = CheckpointCandidate {
            checkpoint_id: CheckpointId::new("restart-checkpoint"),
            expected_head: None,
            base_frontier: cut.ledger,
            read_cut: cut,
            items: vec![CheckpointItem {
                message: CheckpointMessage::Original(CanonicalMessageRef::of(
                    store.conversation_id(),
                    &original,
                )),
                call_sources: Vec::new(),
                result_call: None,
            }],
            prefix_coverage: None,
            summary_metadata: None,
            summary_generation: None,
        };
        let operation = initial.operation_id.clone();
        let signal = crate::runtime::CancellationSignal::new();
        let committed = store
            .commit_projection(crate::durable::projection::ProjectionCommitInput {
                grant: signal
                    .authorize_compaction_commit(operation.clone())
                    .unwrap(),
                operation_id: operation,
                candidate,
                prefix_proof: None,
                owner: store.projection_owner(None).unwrap(),
                tokens_before: TokenMeasurement {
                    input_tokens: 10,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 9,
                occupancy: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
            })
            .unwrap();
        assert_eq!(
            latest(&store)[0].phase,
            CompactionOperationPhase::Generating
        );
        reconcile_compaction_restart(&store, &SystemClock).unwrap();
        assert_eq!(latest(&store)[0].phase, CompactionOperationPhase::Committed);
        assert_eq!(
            store.projection_head().unwrap().unwrap(),
            committed.checkpoint
        );
        let events = store.read_events(None, 64).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|item| matches!(item.event, RuntimeEvent::CompactionCompleted { .. }))
                .count(),
            1
        );
        reconcile_compaction_restart(&store, &SystemClock).unwrap();
        assert_eq!(store.read_events(None, 64).unwrap().events, events);
    }
}
