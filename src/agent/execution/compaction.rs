//! Proactive and overflow-recovery compaction of the attempt conversation as
//! its own independent canonical transition.

use super::request::FrozenCarryover;
use super::terminal::Terminal;
use super::{AgentExecution, DurableFailureKind};
use crate::context::compaction::{
    CompactionAttribution, CompactionExecutionError, execute_compaction,
};
use crate::context::engine::{CompactionConstraints, EstimateCorrection};
use crate::context::error::{ContextError, ContextErrorKind};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::MessageBlock;
use crate::model::error::ModelError;
use crate::runtime::identity::{MessageId, TurnId};
use crate::runtime::inbound::FreshInboundTurn;
use crate::runtime::types::RuntimeError;

/// The committed result of one successful compaction: the derived record
/// plus the measurements the completion event reports.
struct CompletedCompaction;

impl AgentExecution<'_> {
    /// Runs one compaction: plan, summarize, verify progress and fit, and
    /// commit immutable typed checkpoint content and its durable activation.
    ///
    /// `overflow` distinguishes the two callers: a proactive compaction
    /// failure settles as `AttemptFailed(Runtime(ContextCompactionFailed))`,
    /// while a compaction after a context overflow preserves the normalized
    /// overflow as the final model failure (`AttemptFailed(Model(overflow))`)
    /// with the compaction diagnostic carried by `CompactionFailed.error`.
    ///
    /// The compaction planning receives the pending fresh inbound turn (so
    /// unobserved fresh inbound can never be retired) and the exact Effective
    /// System Prompt of this admitted request preparation.
    ///
    /// Cancellation is observed before planning and raced against the summary.
    /// The shared gate separately arbitrates checkpoint-commit authorization:
    /// cancellation before the grant prevents commit; a granted transaction may
    /// settle after cancellation. Each subsequent actual model start has its own gate.
    ///
    /// `staged_request_context` carries the **staged, not-yet-committed**
    /// request-scoped context (Issue #12, M9b): when the compaction is
    /// decided during model-turn preparation, the context it must make room
    /// for is not yet on the canonical Surface (it commits only inside the
    /// durable model-turn start transaction). Every compaction candidate is
    /// evaluated against the exact hypothetical post-compaction request —
    /// the compiled historical projection plus this overlay plus the Effective System
    /// Prompt plus tools — through the same token estimator, never as a
    /// scalar token delta. Independent compactions pass an empty overlay.
    ///
    /// This is also the **one** ownership path that invalidates the opaque
    /// provider continuation: a successful default Summary projection transition
    /// invalidates it exactly once, immediately after the semantic commit.
    /// A failed or cancelled compaction never clears it.
    pub(super) async fn perform_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        effective_system_prompt: &str,
        overflow: Option<&ModelError>,
        staged_request_context: &[MessageBlock],
        estimate_correction: Option<EstimateCorrection>,
    ) -> Result<(), Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        let operation_id =
            crate::runtime::identity::CompactionOperationId::new(uuid::Uuid::now_v7().to_string());
        self.emit(RuntimeEvent::CompactionStarted {
            operation_id: operation_id.clone(),
        });
        if let Some(message) = self.durable_failure.clone() {
            return Err(self.compaction_failure(
                &operation_id,
                &ContextError::new(ContextErrorKind::Internal, message),
                overflow,
            ));
        }
        loop {
            match self
                .run_compaction(
                    &operation_id,
                    must_cover_through,
                    fresh_inbound,
                    effective_system_prompt,
                    staged_request_context,
                    estimate_correction,
                )
                .await
            {
                Ok(_completed) => {
                    // The typed checkpoint and Projection Head are committed.
                    // Complete replay replaces all primary continuation ownership;
                    // the old request measurement is no longer evidence for it.
                    self.retire_continuation();
                    self.measurements.withdraw();
                    // `commit_projection` persisted and returned the exact
                    // completion fact before this branch became observable.
                    break;
                }
                // Carryover is auxiliary. A full/reduced/metadata-only value
                // may make a candidate fail the hard fit, but that must never
                // turn a committed checkpoint into a terminal CannotFit.
                Err(error) if error.kind == ContextErrorKind::CannotFit => {
                    let can_degrade = self
                        .logical_step
                        .frozen_carryover
                        .as_mut()
                        .is_some_and(FrozenCarryover::degrade);
                    if can_degrade {
                        continue;
                    }
                    return Err(self.compaction_failure(&operation_id, &error, overflow));
                }
                // Cancellation never becomes a compaction failure: no
                // `CompactionFailed` event is emitted and the attempt settles
                // cancelled.
                Err(error) if error.kind == ContextErrorKind::Cancelled => {
                    return Err(Terminal::Cancelled {
                        reason: self.cancellation.reason(),
                    });
                }
                Err(error) => return Err(self.compaction_failure(&operation_id, &error, overflow)),
            }
        }
        Ok(())
    }

    /// The cancellation-aware compaction pipeline: plan, summarize, verify
    /// progress, fit-check, and commit.
    ///
    /// The **semantic commit / linearization point** of compaction is the
    /// single `ConversationStore::commit_projection` transaction. Before it
    /// canonical facts remain immutable. After a successful checkpoint/head
    /// transaction the current durable projection is installed and primary
    /// continuation is cleared. Core inputs retain their generation ownership.
    async fn run_compaction(
        &mut self,
        operation_id: &crate::runtime::identity::CompactionOperationId,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        effective_system_prompt: &str,
        staged_request_context: &[MessageBlock],
        estimate_correction: Option<EstimateCorrection>,
    ) -> Result<CompletedCompaction, ContextError> {
        let tools = self.tool_registry().model_definitions();
        let cancellation = self.cancellation.signal();
        // Resolved before the exclusive borrow of the conversation: the
        // gate reads the attempt's request identity, which the compaction
        // is about to be handed mutable access to.
        let observed = self.reusable_observation().cloned();
        let carryover_anchor = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .and_then(|carryover| carryover.anchor.as_ref());
        let carryover = if carryover_anchor.is_some() {
            self.logical_step
                .frozen_carryover
                .as_ref()
                .and_then(FrozenCarryover::admitted)
        } else {
            None
        };
        let result = execute_compaction(
            &mut self.conversation,
            &self.context_runtime,
            &self.request.conversation_id,
            self.store.as_ref(),
            &tools,
            observed.as_ref(),
            effective_system_prompt,
            &CompactionConstraints {
                must_cover_through,
                fresh_inbound,
                protected_context_ids: &self.logical_step.context_ids,
                staged_request_context,
                carryover: carryover.as_ref(),
                carryover_anchor,
                estimate_correction,
            },
            &cancellation,
            CompactionAttribution {
                operation_id: operation_id.clone(),
                attempt_id: Some(self.request.attempt_id.clone()),
                turn_id: Some(TurnId::new(self.turn.to_string())),
            },
        )
        .await;
        let completed = match result {
            Ok(completed) => completed,
            Err(CompactionExecutionError::Context(error)) => return Err(error),
            Err(CompactionExecutionError::Committed { completed, error }) => {
                if completed.newly_committed {
                    self.record_persisted_event(completed.persisted_event);
                }
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(format!(
                    "committed projection could not be installed: {error}"
                ));
                return Err(ContextError::new(ContextErrorKind::Internal, error));
            }
            Err(CompactionExecutionError::Durable(error)) => {
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(format!(
                    "the compaction transition cannot be committed durably: {error}"
                ));
                return Err(ContextError::new(ContextErrorKind::Internal, error));
            }
        };
        if completed.newly_committed {
            self.record_persisted_event(completed.persisted_event);
        }
        Ok(CompletedCompaction)
    }

    /// Emits `CompactionFailed` with the diagnostic and returns the
    /// compaction terminal for this caller.
    ///
    /// A proactive compaction failure is an actual compaction pipeline
    /// failure and settles as
    /// `AttemptFailed(Runtime(ContextCompactionFailed { message }))`; after a
    /// context overflow the original normalized overflow is preserved as the
    /// final model failure with the compaction diagnostic carried by
    /// `CompactionFailed.error`. Neither path becomes a generic context
    /// preparation failure.
    fn compaction_failure(
        &mut self,
        operation_id: &crate::runtime::identity::CompactionOperationId,
        error: &ContextError,
        overflow: Option<&ModelError>,
    ) -> Terminal {
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return terminal;
        }

        self.emit(RuntimeEvent::CompactionFailed {
            operation_id: operation_id.clone(),
            error: error.message.clone(),
        });
        // An unfinished planner search is an independent runtime failure,
        // not evidence that the provider's overflow remains irrecoverable.
        match overflow.filter(|_| error.kind != ContextErrorKind::SearchIncomplete) {
            Some(overflow) => Terminal::Failed {
                failure: AttemptFailure::Model {
                    error: overflow.clone(),
                },
            },
            None => Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContextCompactionFailed {
                        message: error.message.clone(),
                    },
                },
            },
        }
    }

    pub(super) fn compaction_budgets(&self) -> crate::context::CompactionBudgets {
        self.context_runtime.compaction_budgets
    }
}
