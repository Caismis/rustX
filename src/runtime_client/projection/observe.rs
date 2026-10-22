//! Admission of native conversation observations: revision fences, native
//! batches and journal facts enter here before folding.
#[cfg(test)]
use super::RuntimeEvent;
#[cfg(test)]
use super::views::is_attempt_terminal;
use super::{
    ConversationObservation, ForegroundToolState, RuntimeClientAttemptFailure, RuntimeClientError,
    RuntimeClientEvent, RuntimeClientProjection, bounded_diagnostic,
};

/// Closed observation classification. Trace invalidation belongs separately to
/// `JournalBatch`, whose source vocabulary is owned by `PendingObservations`.
pub(super) fn invalidates_read_domains(observation: &ConversationObservation) -> bool {
    match observation {
        ConversationObservation::JournalFact(witness) => {
            !matches!(
                witness.owner.as_ref(),
                Some(crate::durable::observation::RequestOwnerKind::Auxiliary)
            ) && witness.auxiliary_call_id.is_none()
        }
        ConversationObservation::JournalSourceReady
        | ConversationObservation::TranscriptSourceReady(_) => false,
        ConversationObservation::CanonicalCommitted { .. }
        | ConversationObservation::AuditSettled { .. }
        | ConversationObservation::PendingReceipt(_) => true,
        #[cfg(test)]
        ConversationObservation::InboundEnqueued(_)
        | ConversationObservation::InboundDrained(_)
        | ConversationObservation::PendingInboundChanged(_) => true,
        #[cfg(test)]
        ConversationObservation::Committed { .. } => true,
        ConversationObservation::InteractionPending { audit, .. }
        | ConversationObservation::InteractionSettled { audit, .. } => audit.is_some(),
        #[cfg(test)]
        ConversationObservation::Event { event, .. } => invalidates_event(event),
        #[cfg(test)]
        ConversationObservation::ManualCompactionEvent { event, .. } => invalidates_event(event),
        ConversationObservation::ObservationBatch(_)
        | ConversationObservation::JournalBatch { .. }
        | ConversationObservation::CollectionProof { .. }
        | ConversationObservation::ToolOccurrenceProof { .. }
        | ConversationObservation::Published { .. } => {
            unreachable!("publication envelopes are classified through their contents")
        }
        #[cfg(test)]
        ConversationObservation::InteractionRemoved { .. } => false,
        ConversationObservation::PendingHead { .. }
        | ConversationObservation::Workflow(_)
        | ConversationObservation::NativeToolResultCommit(_)
        | ConversationObservation::GoalChanged(_)
        | ConversationObservation::Status(_)
        | ConversationObservation::PublicationOpened { .. }
        | ConversationObservation::Publication { .. }
        | ConversationObservation::Background(_)
        | ConversationObservation::ToolProgress { .. }
        | ConversationObservation::SubagentLifecycle { .. }
        | ConversationObservation::SubagentActivity(_)
        | ConversationObservation::Capability { .. }
        | ConversationObservation::Resources { .. }
        | ConversationObservation::AttemptAdmitted { .. }
        | ConversationObservation::SessionModelChanged { .. }
        | ConversationObservation::Shutdown
        | ConversationObservation::DurableFailure { .. }
        | ConversationObservation::DurabilityFailed { .. }
        | ConversationObservation::AdoptionEligibility(_) => false,
    }
}
#[cfg(test)]
pub(super) fn invalidates_event(event: &RuntimeEvent) -> bool {
    match event {
        RuntimeEvent::ModelRequestStarted { owner, .. }
        | RuntimeEvent::ModelRequestCompleted { owner, .. }
        | RuntimeEvent::ModelRequestFailed { owner, .. } => matches!(
            owner,
            crate::model::snapshot::ModelRequestOwner::Agent { .. }
        ),
        RuntimeEvent::AttemptStarted { .. }
        | RuntimeEvent::AttemptCompleted { .. }
        | RuntimeEvent::AttemptCancelled { .. }
        | RuntimeEvent::AttemptFailed { .. }
        | RuntimeEvent::AttemptTimedOut { .. }
        | RuntimeEvent::AttemptLimitExceeded { .. }
        | RuntimeEvent::TurnStarted
        | RuntimeEvent::AssistantMessageCommitted { .. }
        | RuntimeEvent::ToolMessageCommitted { .. }
        | RuntimeEvent::CompactionStarted { .. }
        | RuntimeEvent::CompactionCompleted { .. } => true,
        _ => false,
    }
}

impl RuntimeClientProjection {
    /// Applies one authoritative observation: fold the snapshot read
    /// model, publish the resulting events, and deliver to subscribers.
    ///
    /// This is the one projection application path. Fold, cursor
    /// allocation, replay retention, and delivery share the caller's
    /// synchronization boundary, so no caller may partially apply an
    /// observation.
    #[allow(clippy::too_many_lines)] // Observation folding and publication share one synchronization boundary.
    pub(crate) fn apply(&mut self, observation: ConversationObservation) {
        if let ConversationObservation::TranscriptSourceReady(proof) = observation {
            if self.observe_transcript_source_prefix(proof.prefix, &proof.read_cut) {
                self.read_domains_dirty = true;
                if let Some(staged) = &mut self.staged_publication {
                    staged.2 = true;
                }
                self.publish(RuntimeClientEvent::ProjectionTransition {
                    events: Vec::new(),
                    metadata: self.projection_metadata(),
                });
            }
            return;
        }

        let observation = match observation {
            ConversationObservation::Workflow(mut cuts) => {
                cuts.retain(|source| source.revision() > self.snapshot.workflows.revision);
                if cuts.is_empty() {
                    return;
                }
                if cuts.len() > 1 {
                    for source in cuts {
                        self.apply(ConversationObservation::Workflow(vec![source]));
                    }
                    return;
                }
                ConversationObservation::Workflow(cuts)
            }
            other => other,
        };
        if let ConversationObservation::PendingReceipt(receipt) = observation {
            self.apply_pending_receipt(Some(receipt.as_ref()));
            return;
        }
        if let ConversationObservation::ToolOccurrenceProof {
            occurrence,
            observation,
        } = observation
        {
            let previous = self.tool_occurrence.replace(occurrence);
            self.apply(*observation);
            self.tool_occurrence = previous;
            return;
        }
        if let ConversationObservation::CollectionProof {
            owner,
            cut,
            ordinal,
            observation,
        } = observation
        {
            self.apply_collection_proof(owner, cut, ordinal, observation);
            return;
        }
        if let ConversationObservation::ObservationBatch(observations) = observation {
            self.apply_observation_batch(observations);
            return;
        }
        if self.exhausted {
            return;
        }
        if let ConversationObservation::Published { observation, .. } = observation {
            self.apply(*observation);
            return;
        }
        if let ConversationObservation::JournalBatch {
            through,
            observations,
        } = observation
        {
            self.apply_journal_batch(through, observations);
            return;
        }
        if let ConversationObservation::JournalFact(witness) = &observation
            && ((!self.publication_state.installing_native_batch
                && witness.native_direct
                && !witness.source_unavailable
                && witness.auxiliary_call_id.is_none()
                && !matches!(
                    witness.owner.as_ref(),
                    Some(crate::durable::observation::RequestOwnerKind::Auxiliary)
                ))
                || self.already_installed_tool_start(witness))
        {
            self.prepared_journal.remove(&witness.sequence);
            self.prepared_outcomes.remove(&witness.sequence);
            return;
        }
        if !self.prepare_observation_revision(&observation) {
            return;
        }
        let published = self.fold(observation);
        #[cfg(test)]
        self.probe_publish_enter();
        match published.len() {
            0 => {}
            1 => self.publish(published.into_iter().next().expect("one transition")),
            _ => {
                let metadata = self.projection_metadata();
                self.publish(RuntimeClientEvent::ProjectionTransition {
                    events: published,
                    metadata,
                });
            }
        }
    }

    // A worker may prepare this witness before the native owner marks its row
    // direct, then deliver it after that exact occurrence has started. Only
    // that repeated Started fact is redundant; terminal and other Journal facts
    // still fold in their native order.
    pub(super) fn already_installed_tool_start(
        &self,
        witness: &crate::durable::observation::ObservationEventWitness,
    ) -> bool {
        if self.publication_state.installing_native_batch
            || witness.source_unavailable
            || witness.event_kind != "tool_execution_started"
            || witness.sequence > self.journal_through
        {
            return false;
        }
        let Some(native) = &witness.tool_occurrence else {
            return false;
        };
        self.snapshot.attempt.as_ref().is_some_and(|attempt| {
            attempt.attempt_id.as_ref() == witness.attempt_id.as_ref()
                && attempt.foreground.iter().any(|slot| {
                    slot.occurrence.locator
                        == crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                            position: native.ledger_position,
                        }
                        && slot.occurrence.block_index == native.block_index
                        && matches!(
                            slot.state,
                            ForegroundToolState::Running { .. }
                                | ForegroundToolState::Settled { .. }
                        )
                })
        })
    }

    pub(super) fn prepare_observation_revision(
        &mut self,
        observation: &ConversationObservation,
    ) -> bool {
        if invalidates_read_domains(observation) {
            self.read_through = self.journal_through;
            self.read_domains_dirty = true;
            let Some(revision) = self.read_revision.checked_add(1) else {
                self.publication_failed(&RuntimeClientError::ProjectionExhausted);
                return false;
            };
            self.read_revision = revision;
        }
        self.advance_domain_revisions(observation);
        #[cfg(test)]
        if let ConversationObservation::Event { attempt_id, event } = observation
            && is_attempt_terminal(event)
            && let Some(cursor) = self.prepared_terminals.remove(attempt_id)
        {
            self.observe_transcript_position(cursor);
        }
        match observation {
            #[cfg(test)]
            ConversationObservation::InboundEnqueued(item) => {
                if let Some(cursor) = item.transcript_cursor() {
                    self.observe_transcript_position(cursor);
                    self.pending_transcript
                        .insert(item.message().id.clone(), cursor);
                }
            }
            #[cfg(test)]
            ConversationObservation::InboundDrained(batch) => {
                for item in batch.items() {
                    self.pending_transcript.remove(&item.message().id);
                }
            }
            #[cfg(test)]
            ConversationObservation::PendingInboundChanged(items) => {
                self.pending_transcript = items
                    .iter()
                    .filter_map(|item| {
                        item.transcript_cursor()
                            .map(|cursor| (item.message().id.clone(), cursor))
                    })
                    .collect();
            }

            ConversationObservation::InteractionPending {
                audit: Some((_, cursor)),
                ..
            }
            | ConversationObservation::InteractionSettled {
                audit: Some((_, cursor)),
                ..
            } => {
                self.observe_transcript_position(*cursor);
            }
            _ => {}
        }
        true
    }

    pub(super) fn apply_collection_proof(
        &mut self,
        owner: &'static str,
        cut: crate::runtime::identity::ObservationCollectionCut,
        ordinal: Option<u64>,
        observation: Box<ConversationObservation>,
    ) {
        let before_cursor = self.cursor;
        let before_count = self.batch_events.as_ref().map(|batch| batch.0.len());
        let changed = self
            .collection_cuts
            .get(owner)
            .is_none_or(|current| cut.generation > current.generation);
        let observed = self.collection_cuts.entry(owner.into()).or_insert(cut);
        if cut.generation > observed.generation {
            *observed = cut;
        }
        let adjusted = ordinal
            .and_then(|ordinal| ordinal.checked_add(observed.total.saturating_sub(cut.total)))
            .unwrap_or(u64::MAX);
        let previous = self.collection_ordinal.replace(adjusted);
        self.apply(*observation);
        self.collection_ordinal = previous;
        if changed
            && self.cursor == before_cursor
            && self.batch_events.as_ref().map(|batch| batch.0.len()) == before_count
        {
            self.publish(RuntimeClientEvent::CollectionChanged {
                domain: owner.into(),
                cut,
            });
        }
    }

    pub(super) fn apply_observation_batch(&mut self, observations: Vec<ConversationObservation>) {
        if self.batch_events.is_some() {
            for observation in observations {
                self.apply(observation);
            }
            return;
        }
        self.batch_events = Some((Vec::new(), 0, false));
        for observation in observations {
            self.apply(observation);
        }
        let (events, _, overflow) = self.batch_events.take().expect("native transition buffer");
        if overflow && let Some(staged) = &mut self.staged_publication {
            staged.2 = true;
        } else if overflow {
            self.publish(RuntimeClientEvent::CheckpointAdvanced {
                transition: "native_observation_batch".into(),
                checkpoint: Box::new(if let Ok(checkpoint) = self.checkpoint_candidate() {
                    checkpoint
                } else {
                    self.mark_observation_unavailable("native batch display unavailable".into());
                    return;
                }),
            });
        } else if !events.is_empty() {
            self.publish(RuntimeClientEvent::ProjectionTransition {
                events,
                metadata: self.projection_metadata(),
            });
        }
    }

    pub(super) fn apply_journal_batch(
        &mut self,
        through: Option<u64>,
        observations: Vec<ConversationObservation>,
    ) {
        assert!(
            self.batch_events.is_none(),
            "native Journal batches cannot nest"
        );
        self.batch_events = Some((Vec::new(), 0, false));
        for observation in observations {
            self.apply(observation);
        }
        if let Some(through) = through {
            let duplicate = self
                .batch_events
                .as_ref()
                .is_some_and(|batch| batch.0.is_empty())
                && through <= self.journal_through;
            if duplicate {
                self.batch_events = None;
                return;
            }
            self.journal_through = self.journal_through.max(through);
            self.read_through = self.read_through.max(through);
            self.source_state.source_unavailable = false;
            self.source_state.journal_failure = false;
            self.publish(RuntimeClientEvent::TraceChanged);
            let (events, _, overflow) = self.batch_events.take().expect("batch buffer");
            if overflow && let Some(staged) = &mut self.staged_publication {
                staged.2 = true;
                return;
            }
            let event = if overflow {
                RuntimeClientEvent::CheckpointAdvanced {
                    transition: "native_journal_batch".into(),
                    checkpoint: Box::new(if let Ok(checkpoint) = self.checkpoint_candidate() {
                        checkpoint
                    } else {
                        self.mark_observation_unavailable(
                            "native Journal display unavailable".into(),
                        );
                        return;
                    }),
                }
            } else {
                RuntimeClientEvent::ProjectionTransition {
                    events,
                    metadata: self.projection_metadata(),
                }
            };
            self.publish(event);
        } else {
            self.batch_events = None;
            self.source_state.source_unavailable = true;
            self.source_state.journal_failure = true;
            self.read_failure = Some("native Journal observation source unavailable".into());
            self.wake_subscribers();
        }
    }

    pub(crate) fn apply_manual_compaction(
        &mut self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        let Some(batch) = batch else {
            self.mark_observation_unavailable("native manual compaction source unavailable".into());
            return;
        };
        let (crate::durable::observation::NativeObservationTransition::Single(witness)
        | crate::durable::observation::NativeObservationTransition::Compaction(witness)) =
            &batch.transition
        else {
            self.mark_observation_unavailable(
                "invalid native manual compaction source family".into(),
            );
            return;
        };
        self.snapshot.context.manual_compaction =
            Some(super::super::snapshot::RuntimeClientManualCompaction {
                request_id: request.cloned(),
                released: witness.event_kind != "compaction_started",
                error: (witness.event_kind == "compaction_failed")
                    .then(|| {
                        witness
                            .value
                            .get("error")
                            .and_then(serde_json::Value::as_str)
                            .map(bounded_diagnostic)
                    })
                    .flatten(),
            });
        self.apply_native_batch(batch);
    }

    /// Installs only finite source-owned receipts. Source I/O and original body
    /// materialization happened at the native write owner, before this boundary.
    pub(crate) fn apply_native_batch(
        &mut self,
        batch: &crate::durable::observation::NativeObservationBatch,
    ) {
        use crate::durable::observation::NativeObservationTransition;
        self.observe_released_journal_prefix(batch.ready_before);
        let mut observations = Vec::with_capacity(34);
        self.install_native_trace_total(batch.trace_total);
        if let Some(pending) = &batch.pending {
            self.apply_pending_receipt(Some(pending));
        }
        self.prepare_native_canonical(batch, &mut observations);
        let mut witness = |witness: &crate::durable::observation::ObservationEventWitness| {
            let field = match witness.event_kind.as_str() {
                "tool_execution_completed" => Some("result"),
                "tool_execution_failed" => Some("error"),
                _ => None,
            }
            .and_then(|field| {
                witness
                    .fields
                    .iter()
                    .find(|entry| entry.path == ["event", field])
                    .map(|entry| crate::observation::tool::JournalFieldRef {
                        sequence: witness.sequence,
                        path: entry.path.clone(),
                        encoded_bytes: entry.encoded_bytes,
                    })
            });
            self.prepare_journal_fact(witness.sequence, batch.failure.clone(), field);
            self.prepared_outcomes.insert(
                witness.sequence,
                (
                    batch.failure_deferred,
                    batch
                        .failure_sequence
                        .filter(|_| batch.failure_deferred && batch.failure_readable_end > 0)
                        .map(|sequence| crate::observation::tool::JournalFieldRef {
                            sequence,
                            path: vec!["normalized_failure".into()],
                            encoded_bytes: batch.failure_readable_end,
                        }),
                    batch.finish_reason_deferred,
                    batch.finish_reason.as_ref().map(|source| {
                        crate::observation::tool::JournalFieldRef {
                            sequence: source.sequence,
                            path: source.path.clone(),
                            encoded_bytes: source.encoded_bytes,
                        }
                    }),
                ),
            );
            observations.push(ConversationObservation::JournalFact(witness.clone()));
        };
        match &batch.transition {
            NativeObservationTransition::Single(value)
            | NativeObservationTransition::Adoption(value)
            | NativeObservationTransition::Compaction(value) => witness(value),
            NativeObservationTransition::RequestStart {
                started,
                contributions,
                statuses,
                ..
            } => {
                witness(started);
                for contribution in contributions {
                    witness(contribution);
                }
                observations.extend(
                    statuses
                        .iter()
                        .cloned()
                        .map(ConversationObservation::Status),
                );
            }
            NativeObservationTransition::WorkflowTerminal { terminal, output } => {
                witness(terminal);
                witness(output);
            }
            NativeObservationTransition::ToolResults { loaded_results, .. } => {
                observations.extend(
                    loaded_results.iter().cloned().map(|tool| {
                        ConversationObservation::NativeToolResultCommit(Box::new(tool))
                    }),
                );
            }
        }
        if let Some((first, last)) = batch.journal_range {
            self.native_journal_highwater = self.native_journal_highwater.max(last);
            // Native owners may finish their independent callbacks out of J
            // order. Their bounded working heads can install immediately, but
            // only the contiguous prefix authorizes historical source bytes.
            if first <= self.journal_through.saturating_add(1) {
                self.journal_through = self.journal_through.max(last);
                self.read_through = self.journal_through;
            }
        }
        self.record_native_cut(batch.read_cut.clone());
        self.source_state.source_unavailable = false;
        self.publication_state.installing_native_batch = true;
        self.apply(ConversationObservation::ObservationBatch(observations));
        self.publication_state.installing_native_batch = false;
        self.observe_released_transcript_prefix(batch.ready_t_through, &batch.read_cut);
        if batch.source_unavailable || batch.ready_before.is_none() {
            self.mark_observation_unavailable("native Journal display family unavailable".into());
        }
    }

    pub(super) fn prepare_native_canonical(
        &mut self,
        batch: &crate::durable::observation::NativeObservationBatch,
        observations: &mut Vec<ConversationObservation>,
    ) {
        use crate::durable::observation::NativeObservationTransition;
        if let Some(canonical) = &batch.canonical {
            self.install_content_metadata(
                canonical
                    .head
                    .iter()
                    .flat_map(|header| {
                        header
                            .content
                            .iter()
                            .map(|binding| binding.content_ref.clone())
                    })
                    .collect(),
            );
            self.prepare_message_headers(
                canonical
                    .head
                    .iter()
                    .filter_map(|header| {
                        header.identity.as_ref().map(|identity| {
                            (
                                identity.clone(),
                                header.clone(),
                                Some(canonical.read_cut.surface_revision),
                            )
                        })
                    })
                    .collect(),
            );
            observations.push(ConversationObservation::CanonicalCommitted {
                attempt_id: match &batch.transition {
                    NativeObservationTransition::Single(witness)
                    | NativeObservationTransition::Adoption(witness)
                    | NativeObservationTransition::Compaction(witness) => {
                        witness.attempt_id.clone()
                    }
                    NativeObservationTransition::RequestStart { started, .. } => {
                        started.attempt_id.clone()
                    }
                    _ => None,
                },
                receipt: crate::durable::TranscriptCommitReceipt {
                    transcript_cursor: canonical.last_transcript,
                    ledger_position: canonical.last_position,
                    ordinal: canonical.count.saturating_sub(1),
                    observation: canonical.clone(),
                },
            });
        }
    }

    pub(super) fn install_native_trace_total(&mut self, total: Option<u64>) {
        let Some(total) = total else {
            self.mark_observation_unavailable("native Trace count source unavailable".into());
            return;
        };
        let loaded = self.snapshot.trace.records.len() as u64;
        let previous = self
            .source_frontiers
            .get("trace.records")
            .map_or(0, |frontier| frontier.total);
        let total = total.max(previous);
        let mut frontier = super::super::trace::checkpoint_frontier(
            total,
            loaded,
            self.snapshot.trace.next_cursor.as_ref(),
        );
        if loaded < total {
            frontier.next_offset = Some(0);
        }
        self.source_frontiers
            .insert("trace.records".into(), frontier);
    }

    pub(crate) fn prepare_journal_fact(
        &mut self,
        sequence: u64,
        failure: Option<RuntimeClientAttemptFailure>,
        result: Option<crate::observation::tool::JournalFieldRef>,
    ) {
        self.prepared_journal.insert(sequence, (failure, result));
    }

    /// Called under the host lock after reading one coherent native cut.
    /// A complete replacement advances one Runtime cursor even when the
    /// independent native Workflow revision has skipped intermediate cuts.
    pub(crate) fn fold_workflows(
        &mut self,
        mut workflows: crate::runtime::workflow::read_model::WorkflowSnapshot,
    ) {
        for pending in &self.snapshot.pending_interactions {
            let Some(node) = pending.workflow_node.as_deref() else {
                continue;
            };
            let reason = match pending.kind {
                crate::observation::interaction::RuntimeClientInteractionKind::Review => {
                    crate::runtime::workflow::read_model::WorkflowWait::Review
                }
                crate::observation::interaction::RuntimeClientInteractionKind::Approval => {
                    crate::runtime::workflow::read_model::WorkflowWait::Approval
                }
                crate::observation::interaction::RuntimeClientInteractionKind::Questionnaire => {
                    crate::runtime::workflow::read_model::WorkflowWait::Questionnaire
                }
            };
            if let Some(run) = workflows
                .runs
                .iter_mut()
                .find(|run| run.id == node.block.run)
                && let Some(instance) = run.instances.iter_mut().find(|instance| {
                    instance.block == node.block
                        && instance.node.as_ref() == Some(&node.node)
                        && instance.visit == Some(node.visit)
                })
            {
                // Correlation never interprets or changes native execution
                // state. A queued older request cannot turn a settled node
                // back into an actionable wait.
                if instance.state
                    == (crate::runtime::workflow::read_model::WorkflowState::Waiting { reason })
                {
                    instance.interaction = Some(pending.interaction.clone());
                }
            }
        }
        let previous = self.snapshot.workflows.revision.0;
        if workflows.revision.0 < previous || workflows == self.snapshot.workflows {
            return;
        }
        self.snapshot.workflows.clone_from(&workflows);
        self.publish(RuntimeClientEvent::WorkflowsUpdated { workflows });
    }

    /// The current revision of the domain owning `domain`.
    pub(super) fn domain_revision(&self, domain: &str) -> u64 {
        let owner = domain.split('.').next().unwrap_or(domain);
        self.domain_revisions.get(owner).copied().unwrap_or(0)
    }

    pub(super) fn advance_domain_revisions(&mut self, observation: &ConversationObservation) {
        let owners: &[&'static str] = match observation {
            ConversationObservation::CanonicalCommitted { .. } => {
                &["messages", "transcript", "attempt", "todos"]
            }
            #[cfg(test)]
            ConversationObservation::Committed { .. } => {
                &["messages", "transcript", "attempt", "todos"]
            }
            ConversationObservation::PublicationOpened { .. }
            | ConversationObservation::Publication { .. } => &["attempt"],
            ConversationObservation::AuditSettled { .. } => &["attempt", "transcript"],
            ConversationObservation::PendingReceipt(_) => &["inbound", "transcript"],
            #[cfg(test)]
            ConversationObservation::InboundEnqueued(_)
            | ConversationObservation::InboundDrained(_)
            | ConversationObservation::PendingInboundChanged(_) => &["inbound", "transcript"],
            ConversationObservation::PendingHead { .. }
            | ConversationObservation::InteractionPending { .. }
            | ConversationObservation::InteractionSettled { .. } => {
                &["pending_interactions", "transcript"]
            }
            #[cfg(test)]
            ConversationObservation::InteractionRemoved { .. } => {
                &["pending_interactions", "transcript"]
            }
            ConversationObservation::NativeToolResultCommit(_) => &["attempt.foreground"],
            ConversationObservation::GoalChanged(_) => &["goal"],
            ConversationObservation::Background(_) => &["jobs"],
            ConversationObservation::SubagentLifecycle { .. }
            | ConversationObservation::SubagentActivity(_) => &["agents"],
            ConversationObservation::Capability { .. } => &["capabilities"],
            ConversationObservation::Resources { .. } => &[
                "resources",
                "capabilities",
                "model",
                "effective_plugins",
                "model_catalog",
            ],
            ConversationObservation::SessionModelChanged { .. } => &["model", "capabilities"],
            ConversationObservation::Status(_) => &["statuses"],
            #[cfg(test)]
            ConversationObservation::ManualCompactionEvent { .. } => &["context"],
            ConversationObservation::ToolProgress { .. }
            | ConversationObservation::AttemptAdmitted { .. } => &["attempt"],
            #[cfg(test)]
            ConversationObservation::Event { event, .. }
                if event.is_auxiliary_model_request() || event.auxiliary_call_id().is_some() =>
            {
                &["auxiliary"]
            }
            ConversationObservation::JournalFact(witness)
                if witness.auxiliary_call_id.is_some()
                    || matches!(
                        witness.owner.as_ref(),
                        Some(crate::durable::observation::RequestOwnerKind::Auxiliary)
                    ) =>
            {
                &["auxiliary"]
            }
            _ => &["attempt", "context", "trace"],
        };
        for owner in owners {
            let revision = self.domain_revisions.entry(owner).or_default();
            let Some(next) = revision.checked_add(1) else {
                self.publication_failed(&RuntimeClientError::ProjectionExhausted);
                return;
            };
            *revision = next;
            if *owner == "jobs" {
                let keys: Vec<_> = self
                    .domain_sources
                    .range(owner.to_string()..format!("{owner}/"))
                    .filter(|(key, _)| key.split('.').next() == Some(*owner))
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in keys {
                    self.domain_sources.get_mut(&key).unwrap().2 = *revision;
                }
            }
        }
    }
}
