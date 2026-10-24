//! Folding of native Journal facts, Tool witnesses, compaction and Attempt lifecycle.
use super::{
    AttemptId, AttemptModelView, ForegroundSettlement, ForegroundToolState, RuntimeClientAttempt,
    RuntimeClientAttemptPhase, RuntimeClientCompactionView, RuntimeClientEvent,
    RuntimeClientOutcome, RuntimeClientProjection, RuntimeEvent, ToolCallId, ToolExecutionResult,
    ToolExecutionStatus, ToolId, arguments_of, client_failure, foreground_slot_for_event,
    update_stored_tool_slots,
};

impl RuntimeClientProjection {
    #[allow(clippy::too_many_lines)] // Exhaustive owner-scoped Journal folding.
    pub(super) fn fold_journal_fact(
        &mut self,
        witness: crate::durable::observation::ObservationEventWitness,
    ) -> Vec<RuntimeClientEvent> {
        if witness.auxiliary_call_id.is_some()
            || matches!(
                witness.owner.as_ref(),
                Some(crate::durable::observation::RequestOwnerKind::Auxiliary)
            )
        {
            self.prepared_journal.remove(&witness.sequence);
            self.prepared_outcomes.remove(&witness.sequence);
            let total = self
                .auxiliary_audit_total
                .unwrap_or_else(|| self.domain_revision("auxiliary"))
                .saturating_add(1);
            if total > 9_007_199_254_740_991 {
                self.mark_observation_unavailable(
                    "auxiliary audit total exceeds the protocol integer domain".into(),
                );
                return Vec::new();
            }
            self.auxiliary_audit_total = Some(total);
            return vec![RuntimeClientEvent::CollectionChanged {
                domain: crate::runtime_client::auxiliary::DOMAIN.into(),
                cut: crate::runtime::identity::ObservationCollectionCut {
                    generation: self.domain_revision("auxiliary"),
                    total,
                },
            }];
        }
        if !self.publication_state.installing_native_batch && witness.native_direct {
            self.prepared_journal.remove(&witness.sequence);
            self.prepared_outcomes.remove(&witness.sequence);
            return Vec::new();
        }
        if witness.source_unavailable {
            self.mark_observation_unavailable("native Journal display family unavailable".into());
        }
        self.install_native_trace_total(witness.trace_total);
        let outcome_source = self
            .prepared_outcomes
            .remove(&witness.sequence)
            .unwrap_or_default();
        if let Some(cursor) = witness.transcript_cursor {
            self.observe_transcript_position(cursor);
        }
        let prepared = self
            .prepared_journal
            .remove(&witness.sequence)
            .unwrap_or_default();
        if let Some(reading) = &witness.occupancy {
            return self.fold_compaction_reading(witness.attempt_id.as_ref(), reading);
        }
        if witness.event_kind == "compaction_completed" {
            self.mark_observation_unavailable(
                "native compaction reading source unavailable".into(),
            );
            return Vec::new();
        }
        let Some(attempt) = witness.attempt_id.as_ref() else {
            if let Ok(event) =
                crate::observation::structural_json::decode::<RuntimeEvent>(witness.value)
                && matches!(
                    event,
                    RuntimeEvent::CompactionStarted { .. }
                        | RuntimeEvent::CompactionCompleted { .. }
                        | RuntimeEvent::CompactionFailed { .. }
                )
            {
                return self.fold_compaction_event(None, &event);
            }
            return Vec::new();
        };
        // Request terminal witnesses deliberately retain only the finite owner
        // kind, not the full primary RequestIdentity required by RuntimeEvent.
        // Consume their bounded Usage directly instead of treating the reduced
        // witness as a complete event.
        if matches!(
            witness.owner,
            Some(crate::durable::observation::RequestOwnerKind::Agent)
        ) && witness.event_kind == "model_request_completed"
        {
            let usage = witness.value.get("usage").cloned().and_then(|value| {
                crate::observation::bounded::decode_domain_value::<crate::model::ModelUsage>(value)
                    .ok()
            });
            return self.fold_request_usage(attempt, usage.as_ref());
        }
        self.tool_occurrence.clone_from(&witness.tool_occurrence);
        if witness.event_kind == "attempt_failed" {
            return prepared.0.map_or_else(Vec::new, |error| {
                self.settle_attempt(
                    attempt,
                    RuntimeClientOutcome::Failed {
                        error,
                        error_deferred: outcome_source.0,
                        error_source: outcome_source.1,
                    },
                )
            });
        }
        if matches!(
            witness.event_kind.as_str(),
            "tool_execution_started" | "tool_execution_completed" | "tool_execution_failed"
        ) {
            return self.fold_tool_witness(attempt, &witness, prepared.1);
        }
        if let Ok(event) =
            crate::observation::structural_json::decode::<RuntimeEvent>(witness.value)
        {
            if matches!(
                event,
                RuntimeEvent::CompactionStarted { .. }
                    | RuntimeEvent::CompactionCompleted { .. }
                    | RuntimeEvent::CompactionFailed { .. }
            ) {
                return self.fold_compaction_event(Some(attempt), &event);
            }
            if let RuntimeEvent::AttemptCompleted { finish_reason, .. } = event {
                return self.settle_attempt(
                    attempt,
                    RuntimeClientOutcome::Completed {
                        finish_reason,
                        finish_reason_deferred: outcome_source.2,
                        finish_reason_source: outcome_source.3,
                    },
                );
            }
            self.fold_event(attempt, &event)
        } else {
            Vec::new()
        }
    }

    pub(super) fn fold_request_usage(
        &mut self,
        attempt_id: &AttemptId,
        usage: Option<&crate::model::ModelUsage>,
    ) -> Vec<RuntimeClientEvent> {
        if let Some(usage) = usage
            && let Some(attempt) = &mut self.snapshot.attempt
            && attempt.attempt_id.as_ref() == Some(attempt_id)
        {
            attempt.last_usage = Some(usage.clone());
            return vec![RuntimeClientEvent::AttemptUsageUpdated {
                attempt_id: attempt_id.clone(),
                usage: usage.clone(),
            }];
        }
        Vec::new()
    }

    pub(super) fn start_tool_witness(
        &mut self,
        attempt: &AttemptId,
        occurrence: Option<&crate::observation::tool::RuntimeClientToolOccurrence>,
    ) {
        if let Some(occurrence) = occurrence {
            update_stored_tool_slots(&mut self.snapshot, occurrence, |slot| {
                if matches!(slot.state, ForegroundToolState::Assembled { .. }) {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments_of(&slot.state),
                        progress: None,
                    };
                }
            });
        }
        if let Some(slot) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|current| current.attempt_id.as_ref() == Some(attempt))
            .and_then(|current| {
                current
                    .foreground
                    .iter_mut()
                    .find(|slot| Some(&slot.occurrence) == occurrence)
            })
            && matches!(slot.state, ForegroundToolState::Assembled { .. })
        {
            slot.state = ForegroundToolState::Running {
                arguments: arguments_of(&slot.state),
                progress: None,
            };
        }
    }

    pub(super) fn fold_tool_witness(
        &mut self,
        attempt: &AttemptId,
        witness: &crate::durable::observation::ObservationEventWitness,
        source: Option<crate::observation::tool::JournalFieldRef>,
    ) -> Vec<RuntimeClientEvent> {
        let value = &witness.value;
        let tool_call_id = value
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| id.len() <= 256)
            .map(ToolCallId::new);
        let tool_id = value
            .get("tool_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| id.len() <= 256)
            .map(ToolId::new);
        let occurrence = self.tool_display_occurrence();
        let metadata_ref = self.tool_display_metadata();
        if witness.event_kind == "tool_execution_started" {
            self.start_tool_witness(attempt, occurrence.as_ref());
            return vec![RuntimeClientEvent::ToolExecutionStarted {
                attempt_id: attempt.clone(),
                occurrence,
                metadata_ref,
                tool_call_id,
                tool_id,
            }];
        }
        let Some(mut result) = crate::observation::tool::tool_witness_result(witness) else {
            return Vec::new();
        };
        let mut result_metadata_ref = None;
        let result_source = source;
        if let Some(slot) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|current| current.attempt_id.as_ref() == Some(attempt))
            .and_then(|current| {
                current
                    .foreground
                    .iter_mut()
                    .find(|slot| Some(&slot.occurrence) == occurrence.as_ref())
            })
        {
            if matches!(slot.state, ForegroundToolState::Settled { .. }) {
                return Vec::new();
            }
            if let Some(committed) = &slot.committed_result {
                result.content.clone_from(&committed.content);
                result.artifacts.clone_from(&committed.artifacts);
                result.deliveries.clone_from(&committed.deliveries);
                result.workflow.clone_from(&committed.workflow);
                result.managed_output.clone_from(&committed.managed_output);
                result_metadata_ref.clone_from(&slot.result_metadata_ref);
            }
            slot.state = ForegroundToolState::Settled {
                arguments: arguments_of(&slot.state),
                result: result.clone(),
            };
            slot.result_source.clone_from(&result_source);
        }
        if let Some(occurrence) = occurrence.as_ref() {
            update_stored_tool_slots(&mut self.snapshot, occurrence, |slot| {
                let mut settled = result.clone();
                if let Some(committed) = &slot.committed_result {
                    settled.content.clone_from(&committed.content);
                    settled.artifacts.clone_from(&committed.artifacts);
                    settled.deliveries.clone_from(&committed.deliveries);
                    settled.workflow.clone_from(&committed.workflow);
                    settled.managed_output.clone_from(&committed.managed_output);
                }
                crate::observation::tool::defer_result(&mut settled);
                slot.state = ForegroundToolState::Settled {
                    arguments: arguments_of(&slot.state),
                    result: settled,
                };
                slot.result_source.clone_from(&result_source);
            });
        }
        vec![RuntimeClientEvent::ToolExecutionSettled {
            attempt_id: attempt.clone(),
            occurrence,
            metadata_ref,
            tool_call_id,
            tool_id,
            result,
            result_source,
            result_metadata_ref,
        }]
    }

    /// The explicit `RuntimeEvent` mapping policy of the Runtime Client
    /// protocol.
    ///
    /// Classification (see the module documentation):
    ///
    /// - PROJECT: attempt lifecycle/settlement, foreground tool lifecycle,
    ///   and progress. Streaming assistant output and tool-call assembly are
    ///   no longer `RuntimeEvent` facts at all: they arrive as durably
    ///   committed publication frames (Issue #108) and fold through
    ///   [`RuntimeClientProjection::fold_publication_frame`];
    /// - PROJECT: turn counting and final request usage, carrying the exact
    ///   values folded into the attempt view;
    /// - TRACE: request boundaries and retries remain internal; `JournalBatch`
    ///   independently invalidates the represented durable Trace cut;
    /// - PROJECT: compaction start/failure and committed completion, carrying
    ///   attempt attribution when automatic and no attempt identity when
    ///   manual.
    // The mapping table is one explicit classification policy; identical
    // `Vec::new()` bodies mark intentionally distinct classes (the remaining
    // fold-only/internal observations) that must remain separately
    // documented.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    pub(super) fn fold_event(
        &mut self,
        attempt_id: &AttemptId,
        event: &RuntimeEvent,
    ) -> Vec<RuntimeClientEvent> {
        match event {
            RuntimeEvent::ModelRequestStarted {
                owner: crate::model::snapshot::ModelRequestOwner::Auxiliary { .. },
                ..
            }
            | RuntimeEvent::ModelRequestCompleted {
                owner: crate::model::snapshot::ModelRequestOwner::Auxiliary { .. },
                ..
            }
            | RuntimeEvent::ModelRequestFailed {
                owner: crate::model::snapshot::ModelRequestOwner::Auxiliary { .. },
                ..
            }
            | RuntimeEvent::AuxiliaryCallStarted { .. }
            | RuntimeEvent::AuxiliaryCallSettled { .. }
            | RuntimeEvent::AuxiliaryRetryScheduled { .. } => Vec::new(),
            RuntimeEvent::AttemptStarted { .. } => {
                let model = self.frozen_attempt_model(attempt_id);
                let execution_settings = self.frozen_execution_settings(attempt_id);
                self.snapshot.attempt = Some(RuntimeClientAttempt {
                    attempt_id: Some(attempt_id.clone()),
                    attempt_id_source: None,
                    phase: RuntimeClientAttemptPhase::Running,
                    turn: 0,
                    last_usage: None,
                    in_flight: None,
                    foreground: Vec::new(),
                    model: model.clone(),
                    execution_settings: execution_settings.clone(),
                });
                // The Agent Status window is deliberately untouched: a status
                // composed by an earlier attempt is a historical fact of this
                // conversation, not attempt-scoped live state, so a new
                // attempt neither retracts nor relocates it.
                //
                // The published event carries the same frozen model the
                // attempt read model carries, so an incremental subscriber
                // and a snapshot reader agree without inference.
                vec![RuntimeClientEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                    model,
                    execution_settings,
                }]
            }
            RuntimeEvent::AttemptCompleted { finish_reason, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Completed {
                    finish_reason_deferred: false,
                    finish_reason_source: None,
                    finish_reason: finish_reason.clone(),
                },
            ),
            RuntimeEvent::AttemptCancelled { reason, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Cancelled { reason: *reason },
            ),
            RuntimeEvent::AttemptTimedOut { .. } => {
                self.settle_attempt(attempt_id, RuntimeClientOutcome::TimedOut)
            }
            RuntimeEvent::AttemptLimitExceeded { limit, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::LimitExceeded { limit: *limit },
            ),
            RuntimeEvent::AttemptFailed { error, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Failed {
                    error_deferred: false,
                    error_source: None,
                    error: client_failure(error),
                },
            ),
            RuntimeEvent::TurnStarted => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                {
                    attempt.turn = attempt.turn.saturating_add(1);
                    return vec![RuntimeClientEvent::AttemptTurnUpdated {
                        attempt_id: attempt_id.clone(),
                        turn: attempt.turn,
                    }];
                }
                Vec::new()
            }
            // Request mechanics are private. The JournalBatch owner publishes
            // one independent Trace invalidation for the represented cut.
            RuntimeEvent::TurnCompleted
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::ModelRetryScheduled { .. } => Vec::new(),
            RuntimeEvent::ContextContributionEmitted { .. } => Vec::new(),
            RuntimeEvent::ModelRequestCompleted { usage, .. } => {
                self.fold_request_usage(attempt_id, usage.as_ref())
            }
            // The durable Journal event carries identity only; the projection
            // already receives the canonical body from the commit
            // observation and never copies body content into the Journal.
            RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. } => Vec::new(),
            // The adopted turn reaches a client as the committed `UserMessage`
            // it names. The obligation itself is durable recovery authority,
            // never a client-facing execution fact.
            RuntimeEvent::InboundTurnAdopted { .. } => Vec::new(),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                    && let Some(slot) = foreground_slot_for_event(
                        &mut attempt.foreground,
                        tool_call_id,
                        self.tool_occurrence.as_ref(),
                    )
                    && matches!(&slot.state, ForegroundToolState::Assembled { .. })
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments_of(&slot.state),
                        progress: None,
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionStarted {
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: crate::observation::tool::inline_call_id(tool_call_id),
                    tool_id: crate::observation::tool::inline_tool_id(tool_id),
                }]
            }
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id,
                tool_id,
                execution_id,
                progress,
            } => {
                if execution_id.is_some() {
                    return Vec::new();
                }
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id.as_ref() == Some(attempt_id)
                    && let Some(slot) = foreground_slot_for_event(
                        &mut attempt.foreground,
                        tool_call_id,
                        self.tool_occurrence.as_ref(),
                    )
                    && let ForegroundToolState::Running { arguments, .. } = &slot.state
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments.clone(),
                        progress: Some(progress.clone()),
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionProgress {
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: crate::observation::tool::inline_call_id(tool_call_id),
                    tool_id: crate::observation::tool::inline_tool_id(tool_id),
                    execution_id: execution_id.clone(),
                    progress: progress.clone(),
                }]
            }
            // A deadline fact is durable liveness *intent* evidence only
            // (Issue #204): the terminal `ToolExecutionCompleted` carries the
            // client-facing settlement, so the projection has nothing to add.
            // The cancellation-request, settlement-observation, and
            // settlement-control-plane-failure facts are the same kind of
            // durable evidence.
            RuntimeEvent::ToolExecutionDeadlineFired { .. }
            | RuntimeEvent::ToolExecutionCancellationRequested { .. }
            | RuntimeEvent::ToolExecutionSettlementObserved { .. }
            | RuntimeEvent::ToolExecutionSettlementControlFailed { .. } => Vec::new(),
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                result,
            } => {
                if self.settle_foreground(attempt_id, tool_call_id, None, result.clone())
                    == ForegroundSettlement::AlreadySettled
                {
                    return Vec::new();
                }
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    result_source: None,
                    result_metadata_ref: None,
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: crate::observation::tool::inline_call_id(tool_call_id),
                    tool_id: crate::observation::tool::inline_tool_id(tool_id),
                    result: result.clone(),
                }]
            }
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                tool_id,
                error,
            } => {
                let result = ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Failed {
                        error: error.clone(),
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                };
                if self.settle_foreground(attempt_id, tool_call_id, None, result.clone())
                    == ForegroundSettlement::AlreadySettled
                {
                    return Vec::new();
                }
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    result_source: None,
                    result_metadata_ref: None,
                    attempt_id: attempt_id.clone(),
                    occurrence: self.tool_display_occurrence(),
                    metadata_ref: self.tool_display_metadata(),
                    tool_call_id: crate::observation::tool::inline_call_id(tool_call_id),
                    tool_id: crate::observation::tool::inline_tool_id(tool_id),
                    result,
                }]
            }
            RuntimeEvent::CompactionStarted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::CompactionFailed { .. } => {
                self.fold_compaction_event(Some(attempt_id), event)
            }
            // The background/subagent ownership/terminal publication facts
            // are durable execution facts; the client projection learns the
            // resulting snapshot and inbound message through its native
            // background/subagent/mailbox/message projections.
            RuntimeEvent::BackgroundExecutionCommitted { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::AgentActivationAdmission { .. }
            | RuntimeEvent::SubagentOwnershipCommitted { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::SubagentPhysicalSettlementProven { .. } => Vec::new(),
            // The interaction requested/settled facts are durable audit
            // evidence (Issue #109). The client already learns the live
            // pending/settled transitions from the coordinator's own
            // observation seam, so replaying the Journal fact here would
            // publish the same prompt twice under two different vocabularies.
            RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. } => {
                Vec::new()
            }
            // Goal journal facts are audit-only. The authoritative bounded
            // view enters through GoalChanged, never replay.
            RuntimeEvent::Goal { .. } => Vec::new(),
            // Workflow lifecycle/join facts are best-effort observability;
            // the successful child value and native terminal pair are the
            // separate durable authority. The Runtime Client continues to
            // project the bounded parent Tool call/result and does not expose
            // workflow-local values or child transcripts as a second
            // conversation surface.
            RuntimeEvent::WorkflowStarted { .. }
            | RuntimeEvent::WorkflowBlockStarted { .. }
            | RuntimeEvent::WorkflowBlockSettled { .. }
            | RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
            | RuntimeEvent::WorkflowLoopIterationSettled { .. }
            | RuntimeEvent::WorkflowLoopExited { .. }
            | RuntimeEvent::NativeToolInvocation { .. }
            | RuntimeEvent::WorkflowNodeStarted { .. }
            | RuntimeEvent::WorkflowNodeWaiting { .. }
            | RuntimeEvent::WorkflowNodeSettled { .. }
            | RuntimeEvent::WorkflowAgentAdmitted { .. }
            | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
            | RuntimeEvent::WorkflowBranchSelected { .. }
            | RuntimeEvent::WorkflowParallelAdmitted { .. }
            | RuntimeEvent::WorkflowParallelSettled { .. }
            | RuntimeEvent::WorkflowCompleted { .. }
            | RuntimeEvent::WorkflowFailed { .. }
            | RuntimeEvent::WorkflowCancelled { .. } => Vec::new(),
        }
    }

    pub(super) fn tool_display_occurrence(
        &self,
    ) -> Option<crate::observation::tool::RuntimeClientToolOccurrence> {
        let native = self.tool_occurrence.as_ref()?;
        Some(crate::observation::tool::RuntimeClientToolOccurrence {
            locator: crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                position: native.ledger_position,
            },
            block_index: native.block_index,
        })
    }

    pub(super) fn tool_display_metadata(&self) -> Option<crate::observation::content::ContentRef> {
        let occurrence = self.tool_display_occurrence()?;
        self.snapshot
            .attempt
            .as_ref()?
            .foreground
            .iter()
            .find(|slot| slot.occurrence == occurrence)
            .and_then(|slot| slot.metadata_ref.clone())
    }

    /// Folds the shared automatic/manual compaction lifecycle. Manual
    /// maintenance carries no attempt identity; both paths update the exact
    /// same context read model and publish the same client vocabulary.
    pub(super) fn fold_compaction_event(
        &mut self,
        attempt_id: Option<&AttemptId>,
        event: &RuntimeEvent,
    ) -> Vec<RuntimeClientEvent> {
        match event {
            RuntimeEvent::CompactionStarted { .. } => {
                self.snapshot.context.compaction_error = None;
                self.snapshot.context.compaction_in_progress = true;
                vec![RuntimeClientEvent::ContextCompactionStarted {
                    context: self.snapshot.context.clone(),
                    attempt_id: attempt_id.cloned(),
                }]
            }
            RuntimeEvent::CompactionFailed { error, .. } => {
                self.snapshot.context.compaction_error = Some(error.clone());
                self.snapshot.context.compaction_in_progress = false;
                vec![RuntimeClientEvent::ContextCompactionFailed {
                    context: self.snapshot.context.clone(),
                    attempt_id: attempt_id.cloned(),
                    error: error.clone(),
                }]
            }
            #[cfg(test)]
            RuntimeEvent::CompactionCompleted {
                generation,
                checkpoint_id,
                activation_id,
                tokens_before,
                estimated_tokens_after,
                occupancy,
                ..
            } => self.fold_compaction_completed(
                attempt_id,
                &RuntimeClientCompactionView {
                    generation: *generation,
                    checkpoint_id: checkpoint_id.clone(),
                    activation_id: *activation_id,
                    tokens_before: *tokens_before,
                    estimated_tokens_after: *estimated_tokens_after,
                },
                occupancy
                    .as_ref()
                    .map(|value| {
                        crate::observation::occupancy::RuntimeClientContextOccupancy::from_native(
                            value, None,
                        )
                    })
                    .as_ref(),
            ),
            _ => unreachable!("only compaction events enter the compaction fold"),
        }
    }

    pub(super) fn fold_compaction_reading(
        &mut self,
        attempt_id: Option<&AttemptId>,
        reading: &crate::durable::observation::OccupancyObservation,
    ) -> Vec<RuntimeClientEvent> {
        // The original completion owns both the generation and its eligible
        // projection head. A later asynchronous read may refine other domains,
        // but cannot be required to make this published completion coherent.
        self.snapshot.context.projection_head = Some(reading.projection_head.clone());
        self.fold_compaction_completed(attempt_id, &reading.compaction, reading.value.as_ref())
    }

    pub(super) fn fold_compaction_completed(
        &mut self,
        attempt_id: Option<&AttemptId>,
        compaction: &RuntimeClientCompactionView,
        occupancy: Option<&crate::observation::occupancy::RuntimeClientContextOccupancy>,
    ) -> Vec<RuntimeClientEvent> {
        self.snapshot.context.compaction_in_progress = false;
        self.snapshot.context.occupancy = occupancy.cloned();
        self.snapshot.context.compaction_count = self
            .snapshot
            .context
            .compaction_count
            .checked_add(1)
            .expect("Runtime Client compaction count cannot overflow");
        self.snapshot.context.latest_compaction = Some(compaction.clone());
        vec![RuntimeClientEvent::ContextCompacted {
            attempt_id: attempt_id.cloned(),
            context: self.snapshot.context.clone(),
        }]
    }

    /// Folds one terminal attempt settlement: the attempt view settles and
    /// exactly one terminal client event is published.
    pub(super) fn settle_attempt(
        &mut self,
        attempt_id: &AttemptId,
        outcome: RuntimeClientOutcome,
    ) -> Vec<RuntimeClientEvent> {
        if let Some(attempt) = &mut self.snapshot.attempt
            && attempt.attempt_id.as_ref() == Some(attempt_id)
        {
            attempt.phase = RuntimeClientAttemptPhase::Settled {
                outcome: outcome.clone(),
            };
            self.snapshot.context.compaction_in_progress = false;
        }
        vec![RuntimeClientEvent::AttemptSettled {
            attempt_id: attempt_id.clone(),
            outcome,
        }]
    }

    /// Ensures the attempt view exists with a running phase (used by unit
    /// tests applying raw event sequences without prior admission).
    pub(super) fn ensure_attempt(&mut self, attempt_id: &AttemptId) {
        if self
            .snapshot
            .attempt
            .as_ref()
            .is_none_or(|attempt| attempt.attempt_id.as_ref() != Some(attempt_id))
        {
            let model = self.frozen_attempt_model(attempt_id);
            self.snapshot.attempt = Some(RuntimeClientAttempt {
                attempt_id: Some(attempt_id.clone()),
                attempt_id_source: None,
                phase: RuntimeClientAttemptPhase::Running,
                turn: 0,
                last_usage: None,
                in_flight: None,
                foreground: Vec::new(),
                model,
                execution_settings: None,
            });
        }
    }

    /// The model view an attempt froze at admission.
    ///
    /// Rebuilding the attempt view (for example when the loop's
    /// `AttemptStarted` arrives after admission) must never lose the frozen
    /// model: while the same attempt is described, the already-frozen view
    /// wins over live session state.
    pub(super) fn frozen_execution_settings(
        &self,
        attempt_id: &AttemptId,
    ) -> Option<super::super::settings::AdmittedSettings> {
        self.snapshot
            .attempt
            .as_ref()
            .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
            .and_then(|attempt| attempt.execution_settings.clone())
    }

    pub(super) fn frozen_attempt_model(
        &self,
        attempt_id: &AttemptId,
    ) -> Option<Box<AttemptModelView>> {
        match self
            .snapshot
            .attempt
            .as_ref()
            .filter(|attempt| attempt.attempt_id.as_ref() == Some(attempt_id))
        {
            Some(attempt) => attempt.model.clone(),
            None => None,
        }
    }

    pub(super) fn install_todo_head(&mut self, todos: &crate::durable::NativeTodoObservation) {
        self.snapshot.todos = Some(todos.snapshot.clone());
        self.source_frontiers.insert(
            "todos.tasks".into(),
            crate::observation::cut::CollectionFrontier {
                domain: "todos.tasks".into(),
                revision: todos.source_ledger_position,
                total: todos.total,
                loaded: todos.loaded,
                next_offset: todos.next_offset,
            },
        );
    }
}
