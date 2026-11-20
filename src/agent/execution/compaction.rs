//! Proactive and overflow-recovery compaction of the attempt conversation as
//! its own independent canonical transition.

use super::request::FrozenCarryover;
use super::terminal::Terminal;
use super::{AgentExecution, DurableFailureKind};
use crate::context::compaction::{
    CompactionAttribution, CompactionExecutionError, execute_compaction_in_operation,
};
use crate::context::engine::{CompactionConstraints, EstimateCorrection};
use crate::context::error::{ContextError, ContextErrorKind};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::MessageBlock;
use crate::model::error::ModelError;
use crate::runtime::identity::{MessageId, TurnId};
use crate::runtime::inbound::FreshInboundTurn;
use crate::runtime::types::RuntimeError;
use crate::tools::types::ModelToolDefinition;

/// The complete frozen primary request inputs relevant to compaction fit.
pub(super) struct CompactionInput<'a> {
    pub(super) request: &'a crate::model::ModelRequest,

    pub(super) tools: &'a [ModelToolDefinition],
    pub(super) estimated_input: u64,
}

/// Native evidence from one committed compaction; missing occupancy is not a
/// zero measurement and cannot establish a rapid refill.
struct CompletedCompaction {
    input_tokens_after: Option<u64>,
    already_recorded: bool,
}

enum BlockingRun {
    Completed(CompletedCompaction),
    Retry,
    Terminal(Terminal),
    Interrupted(Terminal),
}

enum CompetitionWinner {
    Background(Option<crate::context::asynchronous::CompactionDelivery>),
    Blocking(
        Box<Result<crate::context::compaction::GeneratedCompaction, CompactionExecutionError>>,
    ),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompactionNeed {
    Optional,
    Mandatory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompactionOutcome {
    Committed,
    Deferred,
    /// A competing result changed while this blocking scope was being closed.
    Retry,
}

#[derive(Clone, Copy)]
struct LastCompaction {
    accepted_steps: u64,
    input_tokens_before: Option<u64>,
    input_tokens_after: Option<u64>,
    hard_input_limit: Option<u64>,
    mandatory: bool,
}

/// Attempt-local pressure evidence; never persisted or carried to a Session.
/// A rapid cycle has two successive committed compactions with at most one
/// canonically accepted Assistant step between them, and both post-compaction
/// inputs at least 80% of their unchanged hard input limit. Each compaction
/// must also recover less than 5% of that limit, using the exact frozen input
/// estimate and the committed post-compaction occupancy. Two such cycles
/// suppress optional work. Two consecutive mandatory cycles refuse the next
/// mandatory compaction with a diagnostic. Two accepted steps, restored
/// headroom, a meaningful reduction, a changed limit, or missing evidence
/// reset both cycle counts.
/// A no-eligible-history no-op is suppressed only until another canonical
/// Assistant step may make new history eligible; actual optional failures
/// remain suppressed for the entire Attempt.
#[derive(Default)]
pub(super) struct CompactionControl {
    accepted_steps: u64,
    last: Option<LastCompaction>,
    rapid_cycles: u8,
    mandatory_rapid_cycles: u8,
    optional_suppressed: bool,
    skipped_at_step: Option<u64>,
}

impl CompactionControl {
    pub(super) fn accepted_step(&mut self) {
        self.accepted_steps = self.accepted_steps.saturating_add(1);
        if self
            .last
            .is_some_and(|last| self.accepted_steps.saturating_sub(last.accepted_steps) >= 2)
        {
            self.rapid_cycles = 0;
            self.mandatory_rapid_cycles = 0;
        }
    }

    fn record_success(
        &mut self,
        mandatory: bool,
        input_tokens_before: Option<u64>,
        input_tokens_after: Option<u64>,
        hard_input_limit: Option<u64>,
    ) {
        let high_occupancy = |input: Option<u64>, hard: Option<u64>| {
            input.zip(hard).is_some_and(|(input, hard)| {
                hard > 0 && u128::from(input) * 5 >= u128::from(hard) * 4
            })
        };
        let insufficient_gain = |before: Option<u64>, after: Option<u64>, hard: Option<u64>| {
            before
                .zip(after)
                .zip(hard)
                .is_some_and(|((before, after), hard)| {
                    hard > 0
                        && u128::from(before.saturating_sub(after)) * 100 < u128::from(hard) * 5
                })
        };
        let prior = self.last;
        let rapid = prior.is_some_and(|last| {
            last.hard_input_limit == hard_input_limit
                && self.accepted_steps.saturating_sub(last.accepted_steps) <= 1
                && high_occupancy(last.input_tokens_after, last.hard_input_limit)
                && high_occupancy(input_tokens_after, hard_input_limit)
                && insufficient_gain(
                    last.input_tokens_before,
                    last.input_tokens_after,
                    last.hard_input_limit,
                )
                && insufficient_gain(input_tokens_before, input_tokens_after, hard_input_limit)
        });
        if rapid {
            self.rapid_cycles = self.rapid_cycles.saturating_add(1);
            if mandatory && prior.is_some_and(|last| last.mandatory) {
                self.mandatory_rapid_cycles = self.mandatory_rapid_cycles.saturating_add(1);
            } else {
                self.mandatory_rapid_cycles = 0;
            }
        } else {
            self.rapid_cycles = 0;
            self.mandatory_rapid_cycles = 0;
        }
        self.last = Some(LastCompaction {
            accepted_steps: self.accepted_steps,
            input_tokens_before,
            input_tokens_after,
            hard_input_limit,
            mandatory,
        });
    }

    pub(super) fn optional_allowed(&self) -> bool {
        !self.optional_suppressed
            && self.rapid_cycles < 2
            && self.skipped_at_step != Some(self.accepted_steps)
    }

    fn suppress_optional_for_attempt(&mut self) {
        self.optional_suppressed = true;
    }

    fn skip_optional_until_progress(&mut self) {
        self.skipped_at_step = Some(self.accepted_steps);
    }

    fn mandatory_exhausted(&self) -> bool {
        self.mandatory_rapid_cycles >= 2
    }
}

/// These typed pre-commit outcomes leave the old projection authoritative.
/// Optional continuation still requires the complete staged request to pass
/// the ordinary hard admission check; corruption and durability errors never
/// enter this set.
fn deferrable_proactive_failure(kind: ContextErrorKind) -> bool {
    matches!(
        kind,
        ContextErrorKind::SummaryFailed
            | ContextErrorKind::SummaryInputTooLarge
            | ContextErrorKind::SummaryOutputExhausted
            | ContextErrorKind::NoEligibleHistory
            | ContextErrorKind::CannotFit
            | ContextErrorKind::NoProgress
            | ContextErrorKind::SearchIncomplete
            | ContextErrorKind::CompactionBudgetExceeded
            | ContextErrorKind::AuxiliaryBusy
            | ContextErrorKind::AuxiliaryLifecycleBusy
            | ContextErrorKind::AuxiliaryDeadlineExceeded
    )
}

impl AgentExecution<'_> {
    /// A lost competition may re-enter once using the existing logical-step
    /// allowance and its original absolute blocking deadline.
    pub(super) async fn perform_bounded_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        input: &CompactionInput<'_>,
        overflow: Option<&ModelError>,
        estimate_correction: Option<EstimateCorrection>,
        need: CompactionNeed,
    ) -> Result<CompactionOutcome, Terminal> {
        for _ in 0..2 {
            let outcome = self
                .perform_compaction_with_need(
                    must_cover_through,
                    fresh_inbound,
                    input,
                    overflow,
                    estimate_correction,
                    need,
                )
                .await?;
            if outcome != CompactionOutcome::Retry {
                return Ok(outcome);
            }
        }
        Err(Self::context_failure_terminal(&ContextError::new(
            ContextErrorKind::NoProgress,
            "compaction competition changed twice before current admission could be repaired",
        )))
    }

    /// Stage the exact next-start native reminder before pricing activation.
    /// This clone becomes authoritative only after the checkpoint commits.
    fn compaction_installation_input(
        &self,
    ) -> Result<(crate::context::AcceptedContext, Vec<MessageBlock>), ContextError> {
        let mut context = self.logical_step.accepted_context.clone().ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::Internal,
                "compaction has no accepted primary context",
            )
        })?;
        let owner = crate::runtime::identity::ContextContributorIdentity::Native(
            crate::runtime::identity::NativeContextContributor::PermissionReminder,
        );
        let pending = context.user_messages.iter().any(|entry| {
            entry.producer == owner
                && entry
                    .message_id
                    .as_ref()
                    .is_none_or(|id| !self.conversation.ledger().contains(id))
        });
        if !pending {
            context
                .refresh_permission_reminder(&crate::runtime::permission::permission_reminder_text(
                    self.permission_reminder.mode,
                ))
                .map_err(|error| {
                    ContextError::new(ContextErrorKind::CannotFit, error.to_string())
                })?;
        }
        let staged = self.stage_accepted_context(&mut context).map_err(|error| {
            ContextError::new(
                ContextErrorKind::Internal,
                format!("staged context cannot be committed: {error}"),
            )
        })?;
        Ok((context, staged))
    }

    fn install_compaction_context(&mut self, context: crate::context::AcceptedContext) {
        self.logical_step.context_ids = context
            .user_messages
            .iter()
            .filter_map(|entry| entry.message_id.clone())
            .collect();
        self.logical_step.accepted_context = Some(context);
    }

    /// Start a Session-owned proactive operation without retaining this Attempt
    /// as its cancellation parent or installation capability.
    pub(super) async fn start_background_compaction(
        &mut self,
        input: &CompactionInput<'_>,
    ) -> Result<bool, Terminal> {
        let Some(coordinator) = self.asynchronous.clone() else {
            return Ok(false);
        };
        let (_, staged_request_context) = match self.compaction_installation_input() {
            Ok(input) => input,
            Err(error) => return self.background_error(CompactionExecutionError::Context(error)),
        };
        let admission = match coordinator.admit(&self.request.attempt_id) {
            Ok(Some(admission)) => admission,
            Ok(None) => return Ok(true),
            Err(error) => return self.background_error(error),
        };
        let budget = crate::context::compaction_budget::CompactionBudget::asynchronous(
            admission.operation_id.clone(),
            self.context_runtime.monotonic_clock.clone(),
        );
        let operation = match self
            .context_runtime
            .summarizer
            .operation(budget, admission.cancellation.child())
        {
            Ok(operation) => operation,
            Err(error) => {
                let error = CompactionExecutionError::Context(error);
                if let Err(publication) = coordinator.fail_admission(&admission, &error) {
                    return self.background_error(CompactionExecutionError::Context(publication));
                }
                return self.background_error(error);
            }
        };
        let observed = self.reusable_observation().cloned();
        let must_cover = self.continuation_owner.clone();
        let fresh = self.pending_fresh_inbound.clone();
        let anchor = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .and_then(|value| value.anchor.as_ref());
        let carryover = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| anchor.is_some())
            .and_then(FrozenCarryover::admitted);
        let captured = crate::context::compaction::capture_compaction(
            &self.context_runtime,
            self.conversation.revision(),
            &self.request.conversation_id,
            self.store.as_ref(),
            input.tools,
            observed.as_ref(),
            &input.request.effective_system_prompt,
            &CompactionConstraints {
                must_cover_through: must_cover.as_ref(),
                fresh_inbound: fresh.as_ref(),
                protected_context_ids: &self.logical_step.context_ids,
                staged_request_context: &staged_request_context,
                carryover: carryover.as_ref(),
                carryover_anchor: anchor,
                ..CompactionConstraints::default()
            },
            &admission.cancellation,
            CompactionAttribution {
                operation_id: admission.operation_id.clone(),
                attempt_id: None,
                turn_id: None,
            },
            operation.as_ref(),
        )
        .await;
        match captured {
            Ok(captured) => {
                if let Err(error) = coordinator.start(admission.clone(), captured, operation) {
                    if let Err(publication) = coordinator.fail_admission(&admission, &error) {
                        return self
                            .background_error(CompactionExecutionError::Context(publication));
                    }
                    return self.background_error(error);
                }
            }
            Err(error) => {
                operation.close().await;
                if let Err(publication) = coordinator.fail_admission(&admission, &error) {
                    return self.background_error(CompactionExecutionError::Context(publication));
                }
                return self.background_error(error);
            }
        }
        Ok(true)
    }

    fn background_error(&mut self, error: CompactionExecutionError) -> Result<bool, Terminal> {
        match error {
            CompactionExecutionError::Context(error)
                if error.kind == ContextErrorKind::NoEligibleHistory =>
            {
                self.compaction_control.skip_optional_until_progress();
                Ok(true)
            }
            CompactionExecutionError::Context(error)
                if deferrable_proactive_failure(error.kind)
                    || error.kind == ContextErrorKind::Cancelled =>
            {
                self.compaction_control.suppress_optional_for_attempt();
                Ok(true)
            }
            CompactionExecutionError::Context(error)
                if error.kind == ContextErrorKind::AuxiliaryDurabilityFailed =>
            {
                let message = error.message;
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(message.clone());
                Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore { message },
                    },
                })
            }
            CompactionExecutionError::Context(error) => Err(Self::context_failure_terminal(&error)),
            error => {
                let message = format!("independent compaction durability failed: {error:?}");
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(message.clone());
                Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore { message },
                    },
                })
            }
        }
    }

    pub(super) fn settle_background(
        &mut self,
        coordinator: &dyn crate::context::asynchronous::CompactionCoordinator,
        admission: &crate::context::asynchronous::CompactionAdmission,
        outcome: crate::context::asynchronous::CompactionDisposition,
    ) -> Result<(), Terminal> {
        if matches!(
            &outcome,
            crate::context::asynchronous::CompactionDisposition::Defer(_)
        ) {
            self.logical_step.deferred_compaction = Some(admission.operation_id.clone());
        }
        coordinator.settle(admission, outcome).map_err(|error| {
            if error.kind == ContextErrorKind::Cancelled && self.cancellation.is_cancelled() {
                return self.cancelled_terminal();
            }
            let message = format!("compaction status could not be settled: {}", error.message);
            self.durable_failure_kind = Some(DurableFailureKind::Compaction);
            self.durable_failure = Some(message.clone());
            Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            }
        })
    }

    /// Consume exactly one result selected at the current preparation cut.
    #[allow(clippy::too_many_lines)] // Keep installation and its complete settlement table together.
    pub(super) fn consume_background_compaction(
        &mut self,
        delivery: crate::context::asynchronous::CompactionDelivery,
        input: &CompactionInput<'_>,
        correction: Option<EstimateCorrection>,
        mandatory: bool,
    ) -> Result<bool, Terminal> {
        use crate::context::asynchronous::{CompactionDelivery, CompactionDisposition};
        use crate::context::compaction::{
            ActivationOutcome, CurrentInstallation, activate_generated,
        };
        let Some(coordinator) = self.asynchronous.clone() else {
            return Ok(false);
        };
        let ready = match delivery {
            CompactionDelivery::Ready(ready) => ready,
            CompactionDelivery::Failed(error) => {
                return self
                    .background_error(CompactionExecutionError::Context(error))
                    .map(|_| false);
            }
        };
        let (activated_context, staged_request_context) = match self.compaction_installation_input()
        {
            Ok(input) => input,
            Err(error) if deferrable_proactive_failure(error.kind) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Defer(Some(error.message)),
                )?;
                return Ok(false);
            }
            Err(error) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Failed(error.message.clone()),
                )?;
                return Err(Self::context_failure_terminal(&error));
            }
        };
        let must_cover = self.continuation_owner.clone();
        let fresh = self.pending_fresh_inbound.clone();
        let anchor = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .and_then(|value| value.anchor.as_ref());
        let carryover = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| anchor.is_some())
            .and_then(FrozenCarryover::admitted);
        let fallbacks = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| mandatory && anchor.is_some())
            .map_or_default(FrozenCarryover::fallbacks);
        let constraints = CompactionConstraints {
            must_cover_through: must_cover.as_ref(),
            fresh_inbound: fresh.as_ref(),
            protected_context_ids: &self.logical_step.context_ids,
            staged_request_context: &staged_request_context,
            carryover_fallbacks: &fallbacks,
            carryover: carryover.as_ref(),
            carryover_anchor: anchor,
            estimate_correction: correction,
            ..CompactionConstraints::default()
        };
        let signal = self.cancellation.signal();
        let current = CurrentInstallation {
            context: &self.context_runtime,
            request: input.request,
            constraints: &constraints,
            attempt_id: Some(&self.request.attempt_id),
            turn_id: Some(TurnId::new(self.turn.to_string())),
            installer_cancellation: Some(&signal),
        };
        let result = activate_generated(
            &ready.generated,
            &mut self.conversation,
            self.store.as_ref(),
            &ready.admission.cancellation,
            &current,
        );
        match result {
            Ok(ActivationOutcome::Committed(completed)) => {
                let after = match &completed.persisted_event.event {
                    RuntimeEvent::CompactionCompleted { occupancy, .. } => {
                        occupancy.as_ref().map(|value| value.input_tokens)
                    }
                    _ => None,
                };
                if completed.newly_committed {
                    self.record_persisted_event(completed.persisted_event);
                    self.permission_reminder.mark_after_compaction();
                    self.install_compaction_context(activated_context);
                }
                self.retire_continuation();
                self.measurements.withdraw();
                let hard = self
                    .context_runtime
                    .engine
                    .admissible_input_limit(self.compaction_budgets().primary_output_budget)
                    .ok();
                self.compaction_control.record_success(
                    mandatory,
                    Some(input.estimated_input),
                    after,
                    hard,
                );
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Committed,
                )?;
                self.install_committed_carryover(completed.carryover_level)
                    .map_err(|error| Self::context_failure_terminal(&error))?;
                Ok(true)
            }
            Ok(ActivationOutcome::Superseded) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Superseded,
                )?;
                Ok(false)
            }
            Ok(ActivationOutcome::Incomparable) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Incomparable,
                )?;
                Ok(false)
            }
            Ok(ActivationOutcome::Inadmissible(error)) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Defer(Some(error.message)),
                )?;
                Ok(false)
            }
            Err(CompactionExecutionError::HeadChanged) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Defer(None),
                )?;
                Ok(false)
            }
            Err(CompactionExecutionError::Context(error))
                if error.kind == ContextErrorKind::Cancelled =>
            {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Defer(None),
                )?;
                if self.cancellation.is_cancelled() {
                    Err(self.cancelled_terminal())
                } else {
                    Ok(false)
                }
            }
            Err(CompactionExecutionError::Committed { completed, error }) => {
                if completed.newly_committed {
                    self.record_persisted_event(completed.persisted_event);
                    self.permission_reminder.mark_after_compaction();
                    self.install_compaction_context(activated_context);
                }
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Committed,
                )?;
                let message = format!("committed projection could not be installed: {error}");
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(message.clone());
                Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore { message },
                    },
                })
            }
            Err(error) => {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    CompactionDisposition::Failed(format!("{error:?}")),
                )?;
                self.background_error(error).map(|_| false)
            }
        }
    }

    /// Mandatory pressure may join this exact independent operation without
    /// transferring cancellation or refreshing its allowance.
    pub(super) async fn wait_for_background_compaction(
        &mut self,
        input: &CompactionInput<'_>,
        correction: Option<EstimateCorrection>,
    ) -> Result<bool, Terminal> {
        let Some(coordinator) = self.asynchronous.clone() else {
            return Ok(false);
        };
        let Some(admission) = coordinator.active().filter(|work| {
            self.logical_step.deferred_compaction.as_ref() != Some(&work.operation_id)
        }) else {
            return Ok(false);
        };
        let (_, staged_request_context) = match self.compaction_installation_input() {
            Ok(input) => input,
            Err(error) if deferrable_proactive_failure(error.kind) => return Ok(false),
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        let anchor = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .and_then(|value| value.anchor.as_ref());
        let carryover = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| anchor.is_some())
            .and_then(FrozenCarryover::admitted);
        let constraints = CompactionConstraints {
            must_cover_through: self.continuation_owner.as_ref(),
            fresh_inbound: self.pending_fresh_inbound.as_ref(),
            protected_context_ids: &self.logical_step.context_ids,
            staged_request_context: &staged_request_context,
            carryover: carryover.as_ref(),
            carryover_anchor: anchor,
            estimate_correction: correction,
            ..CompactionConstraints::default()
        };
        match coordinator.can_join(
            &admission,
            &self.context_runtime,
            input.request,
            &constraints,
        ) {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => return self.background_error(error).map(|_| false),
        }
        let signal = self.cancellation.signal();
        let delivery = coordinator
            .wait(&admission, &self.request.attempt_id, &signal)
            .await;
        if self.cancellation.is_cancelled() {
            if let Some(crate::context::asynchronous::CompactionDelivery::Ready(ready)) = &delivery
            {
                self.settle_background(
                    coordinator.as_ref(),
                    &ready.admission,
                    crate::context::asynchronous::CompactionDisposition::Defer(None),
                )?;
            }
            return Err(self.cancelled_terminal());
        }
        match delivery {
            Some(delivery) => self.consume_background_compaction(delivery, input, correction, true),
            None => Ok(false),
        }
    }

    /// Runs one compaction: plan, summarize, verify progress and fit, and
    /// commit immutable typed checkpoint content and its durable activation.
    ///
    /// A mandatory compaction failure settles as a runtime compaction failure.
    /// Following provider overflow, the original normalized overflow is
    /// preserved unless the bounded planner reports incomplete search.
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
    /// The optional mode can defer only a classified pre-commit failure.
    /// Request preparation still has to prove that the complete old request
    /// fits before it can pass the normal start gate.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "keep admission, receipt handling, and control transitions in order"
    )]
    pub(super) async fn perform_compaction_with_need(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        input: &CompactionInput<'_>,
        overflow: Option<&ModelError>,
        estimate_correction: Option<EstimateCorrection>,
        need: CompactionNeed,
    ) -> Result<CompactionOutcome, Terminal> {
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled_terminal());
        }
        let operation_id =
            crate::runtime::identity::CompactionOperationId::new(uuid::Uuid::now_v7().to_string());
        let reentry = self.logical_step.compaction_budget.is_some();
        let budget = self.logical_step.compaction_budget.as_ref().map_or_else(
            || self.context_runtime.compaction_budget(operation_id.clone()),
            |previous| previous.next_operation(operation_id.clone()),
        );
        let admission = if reentry {
            budget.charge_recovery(&self.cancellation.signal())
        } else {
            budget.check(&self.cancellation.signal())
        };
        if admission
            .as_ref()
            .is_err_and(|error| error.kind == ContextErrorKind::Cancelled)
        {
            return Err(self.cancelled_terminal());
        }
        self.logical_step.compaction_budget = Some(budget.clone());
        self.emit(RuntimeEvent::CompactionStarted {
            operation_id: operation_id.clone(),
            mode: if overflow.is_some() {
                crate::context::operation::CompactionMode::Overflow
            } else {
                crate::context::operation::CompactionMode::Blocking
            },
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled_compaction(&operation_id));
        }
        if need == CompactionNeed::Mandatory && self.compaction_control.mandatory_exhausted() {
            let error = ContextError::new(
                ContextErrorKind::NoProgress,
                "context refilled rapidly after repeated mandatory compactions; this Attempt cannot make enough progress within the model window",
            );
            return Err(self.compaction_failure(&operation_id, &error, None));
        }
        if let Err(error) = admission {
            return self.compaction_failure_or_defer(&operation_id, &error, overflow, need);
        }
        if let Some(message) = self.durable_failure.clone() {
            return Err(self.compaction_failure(
                &operation_id,
                &ContextError::new(ContextErrorKind::Internal, message),
                overflow,
            ));
        }
        #[cfg(test)]
        let admission_gate = self
            .pauses
            .compaction_admission
            .lock()
            .expect("compaction admission gate")
            .take();
        #[cfg(test)]
        if let Some(gate) = admission_gate {
            gate.enter();
        }
        let blocking_cancellation = self.cancellation.signal().child();
        let operation = match self
            .context_runtime
            .summarizer
            .operation(budget.clone(), blocking_cancellation.clone())
        {
            Ok(operation) => operation,
            Err(error) if error.kind == ContextErrorKind::Cancelled => {
                return Err(self.cancelled_compaction(&operation_id));
            }
            Err(error) => {
                return self.compaction_failure_or_defer(&operation_id, &error, overflow, need);
            }
        };
        let hard_limit = self
            .context_runtime
            .engine
            .admissible_input_limit(self.compaction_budgets().primary_output_budget)
            .ok();
        // Context owns carryover fallback and final fit within this one
        // operation, so an already accepted Summary fragment is never replayed
        // merely because a less detailed carryover is needed.
        let result = if need == CompactionNeed::Mandatory
            && self
                .asynchronous
                .as_ref()
                .and_then(|owner| owner.active())
                .is_some()
        {
            self.run_compaction_competing(
                must_cover_through,
                fresh_inbound,
                input,
                estimate_correction,
                operation.as_ref(),
                &blocking_cancellation,
            )
            .await
        } else {
            self.run_compaction(
                must_cover_through,
                fresh_inbound,
                input,
                estimate_correction,
                need,
                operation.as_ref(),
            )
            .await
            .map(BlockingRun::Completed)
        };
        if matches!(&result, Err(_) | Ok(BlockingRun::Interrupted(_))) {
            blocking_cancellation.cancel();
        }
        operation.close().await;
        match result {
            Ok(BlockingRun::Interrupted(terminal)) => {
                if self.durable_failure_terminal_from_state().is_none() {
                    self.emit(RuntimeEvent::CompactionCancelled { operation_id });
                }
                Err(self
                    .durable_failure_terminal_from_state()
                    .unwrap_or(terminal))
            }
            Ok(BlockingRun::Retry) => Ok(CompactionOutcome::Retry),
            Ok(BlockingRun::Terminal(terminal)) => Err(terminal),
            Ok(BlockingRun::Completed(completed)) => {
                // The typed checkpoint and Projection Head are committed.
                // Complete replay replaces all primary continuation ownership;
                // the old request measurement is no longer evidence for it.
                if !completed.already_recorded {
                    self.retire_continuation();
                    self.measurements.withdraw();
                    if let Some(owner) = &self.asynchronous {
                        owner.head_changed();
                    }
                    self.compaction_control.record_success(
                        need == CompactionNeed::Mandatory,
                        Some(input.estimated_input),
                        completed.input_tokens_after,
                        hard_limit,
                    );
                }
                Ok(CompactionOutcome::Committed)
            }
            Err(error) => self.compaction_failure_or_defer(&operation_id, &error, overflow, need),
        }
    }

    /// The cancellation-aware compaction pipeline: plan, summarize, verify
    /// progress, fit-check, and commit.
    ///
    /// The **semantic commit / linearization point** of compaction is the
    /// single `ConversationStore::commit_projection` transaction. Before it
    /// canonical facts remain immutable. After a successful checkpoint/head
    /// transaction the current durable projection is installed and primary
    /// continuation is cleared. Core inputs retain their generation ownership.
    #[allow(
        clippy::too_many_lines,
        reason = "keep compaction receipt and reconciliation in one ordered path"
    )]
    async fn run_compaction(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        input: &CompactionInput<'_>,
        estimate_correction: Option<EstimateCorrection>,
        need: CompactionNeed,
        operation: &dyn crate::context::summarizer::SummaryOperation,
    ) -> Result<CompletedCompaction, ContextError> {
        let (activated_context, staged_request_context) = self.compaction_installation_input()?;
        let operation_id = operation.budget().operation_id().clone();
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
        let frozen_carryover = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| carryover_anchor.is_some());
        let carryover = frozen_carryover.and_then(FrozenCarryover::admitted);
        let carryover_fallbacks = frozen_carryover
            .filter(|_| need == CompactionNeed::Mandatory)
            .map_or_default(FrozenCarryover::fallbacks);
        // The Context owner publishes only already-persisted chunk facts.
        // Capture the same native observation seam without borrowing the
        // mutable conversation that Context exclusively owns during commit.
        let progress_observer = self.observer;
        let attempt_id = self.request.attempt_id.clone();
        let progress = move |envelope: crate::events::types::RuntimeEventEnvelope| {
            if let Some(observer) = progress_observer {
                observer.observe_event(
                    &attempt_id,
                    &envelope.event,
                    envelope.sequence,
                    envelope.tool_occurrence.as_ref(),
                    envelope.observation.as_deref(),
                );
            }
        };
        let result = execute_compaction_in_operation(
            &mut self.conversation,
            &self.context_runtime,
            &self.request.conversation_id,
            self.store.as_ref(),
            input.tools,
            observed.as_ref(),
            &input.request.effective_system_prompt,
            &CompactionConstraints {
                must_cover_through,
                fresh_inbound,
                protected_context_ids: &self.logical_step.context_ids,
                staged_request_context: &staged_request_context,
                carryover: carryover.as_ref(),
                carryover_fallbacks: &carryover_fallbacks,
                carryover_anchor,
                estimate_correction,
                focus: None,
            },
            &cancellation,
            CompactionAttribution {
                operation_id: operation_id.clone(),
                attempt_id: Some(self.request.attempt_id.clone()),
                turn_id: Some(TurnId::new(self.turn.to_string())),
            },
            operation,
            Some(&progress),
        )
        .await;
        self.finish_blocking_result(result, activated_context)
    }

    /// Race one independently owned background result with the already
    /// admitted blocking generator. Only this Attempt owns the blocking
    /// cancellation and settlement; the background waiter transfers neither.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_compaction_competing(
        &mut self,
        must_cover_through: Option<&MessageId>,
        fresh_inbound: Option<&FreshInboundTurn>,
        input: &CompactionInput<'_>,
        estimate_correction: Option<EstimateCorrection>,
        operation: &dyn crate::context::summarizer::SummaryOperation,
        blocking_cancellation: &crate::runtime::cancellation::CancellationSignal,
    ) -> Result<BlockingRun, ContextError> {
        use crate::context::asynchronous::{CompactionDelivery, CompactionDisposition};
        use crate::context::compaction::{
            ActivationPreview, CurrentInstallation, preview_generated,
        };

        let Some(coordinator) = self.asynchronous.clone() else {
            return self
                .run_compaction(
                    must_cover_through,
                    fresh_inbound,
                    input,
                    estimate_correction,
                    CompactionNeed::Mandatory,
                    operation,
                )
                .await
                .map(BlockingRun::Completed);
        };
        let Some(admission) = coordinator.active().filter(|work| {
            self.logical_step.deferred_compaction.as_ref() != Some(&work.operation_id)
        }) else {
            return self
                .run_compaction(
                    must_cover_through,
                    fresh_inbound,
                    input,
                    estimate_correction,
                    CompactionNeed::Mandatory,
                    operation,
                )
                .await
                .map(BlockingRun::Completed);
        };
        let (activated_context, staged_request_context) = self.compaction_installation_input()?;
        let operation_id = operation.budget().operation_id().clone();
        let attempt_id = self.request.attempt_id.clone();
        let attempt_signal = self.cancellation.signal();
        let store = self.store.clone();
        let owner = match store.projection_owner(Some(&attempt_id)) {
            Ok(owner) => owner,
            Err(error) => {
                return self
                    .finish_blocking_result(
                        Err(CompactionExecutionError::Durable(error.to_string())),
                        activated_context,
                    )
                    .map(BlockingRun::Completed);
            }
        };
        let observed = self.reusable_observation().cloned();
        let frozen_carryover = self.logical_step.frozen_carryover.clone();
        let anchor = frozen_carryover
            .as_ref()
            .and_then(|value| value.anchor.clone());
        let carryover = frozen_carryover
            .as_ref()
            .filter(|_| anchor.is_some())
            .and_then(FrozenCarryover::admitted);
        let carryover_fallbacks = frozen_carryover
            .as_ref()
            .filter(|_| anchor.is_some())
            .map_or_default(FrozenCarryover::fallbacks);
        let protected_ids = self.logical_step.context_ids.clone();
        let constraints = CompactionConstraints {
            must_cover_through,
            fresh_inbound,
            protected_context_ids: &protected_ids,
            staged_request_context: &staged_request_context,
            carryover: carryover.as_ref(),
            carryover_fallbacks: &carryover_fallbacks,
            carryover_anchor: anchor.as_ref(),
            estimate_correction,
            focus: None,
        };
        let captured = match crate::context::compaction::capture_compaction(
            &self.context_runtime,
            self.conversation.revision(),
            &self.request.conversation_id,
            store.as_ref(),
            input.tools,
            observed.as_ref(),
            &input.request.effective_system_prompt,
            &constraints,
            blocking_cancellation,
            CompactionAttribution {
                operation_id: operation_id.clone(),
                attempt_id: Some(attempt_id.clone()),
                turn_id: Some(TurnId::new(self.turn.to_string())),
            },
            operation,
        )
        .await
        {
            Ok(captured) => captured,
            Err(error) => {
                return self
                    .finish_blocking_result(Err(error), activated_context)
                    .map(BlockingRun::Completed);
            }
        };
        let event_observer = self.observer;
        let progress_store = store.clone();
        let progress_attempt = attempt_id.clone();
        let progress = move |envelope| -> Result<(), CompactionExecutionError> {
            let persisted = progress_store
                .append_event(envelope)
                .map_err(|error| CompactionExecutionError::Durable(error.to_string()))?;
            if let Some(observer) = event_observer {
                observer.observe_event(
                    &progress_attempt,
                    &persisted.event,
                    persisted.sequence,
                    persisted.tool_occurrence.as_ref(),
                    persisted.observation.as_deref(),
                );
            }
            Ok(())
        };
        let before_call = || Ok(());
        let mut generation = Box::pin(crate::context::compaction::generate_compaction(
            captured,
            store.as_ref(),
            operation,
            blocking_cancellation,
            &progress,
            &before_call,
        ));
        let mut waiting = Box::pin(coordinator.wait(&admission, &attempt_id, &attempt_signal));
        let winner = tokio::select! {
            biased;
            delivery = &mut waiting => CompetitionWinner::Background(delivery),
            generated = &mut generation => CompetitionWinner::Blocking(Box::new(generated)),
        };
        drop(waiting);
        let generated = match winner {
            CompetitionWinner::Blocking(generated) => *generated,
            CompetitionWinner::Background(Some(CompactionDelivery::Ready(ready))) => {
                let current = CurrentInstallation {
                    context: &self.context_runtime,
                    request: input.request,
                    constraints: &constraints,
                    attempt_id: Some(&attempt_id),
                    turn_id: Some(TurnId::new(self.turn.to_string())),
                    installer_cancellation: Some(&attempt_signal),
                };
                let disposition =
                    match preview_generated(&ready.generated, store.as_ref(), &current) {
                        Ok(ActivationPreview::Eligible(_)) => {
                            // The preview grants nothing. Stop and physically join
                            // our blocking scope before revalidating and committing.
                            blocking_cancellation.cancel();
                            drop(generation);
                            operation.close().await;
                            self.emit(RuntimeEvent::CompactionCancelled { operation_id });
                            if let Some(terminal) = self.durable_failure_terminal_from_state() {
                                let _ = self.settle_background(
                                    coordinator.as_ref(),
                                    &ready.admission,
                                    CompactionDisposition::Defer(None),
                                );
                                return Ok(BlockingRun::Terminal(terminal));
                            }
                            match self.consume_background_compaction(
                                CompactionDelivery::Ready(ready),
                                input,
                                estimate_correction,
                                true,
                            ) {
                                Ok(true) => {
                                    return Ok(BlockingRun::Completed(CompletedCompaction {
                                        input_tokens_after: None,
                                        already_recorded: true,
                                    }));
                                }
                                Ok(false) => return Ok(BlockingRun::Retry),
                                Err(terminal) => return Ok(BlockingRun::Terminal(terminal)),
                            }
                        }
                        Ok(ActivationPreview::Superseded) => CompactionDisposition::Superseded,
                        Ok(ActivationPreview::Incomparable) => CompactionDisposition::Incomparable,
                        Ok(ActivationPreview::Inadmissible(error)) => {
                            CompactionDisposition::Defer(Some(error.message))
                        }
                        Err(CompactionExecutionError::Context(error))
                            if deferrable_proactive_failure(error.kind) =>
                        {
                            CompactionDisposition::Defer(Some(error.message))
                        }
                        Err(CompactionExecutionError::Context(error))
                            if error.kind != ContextErrorKind::AuxiliaryDurabilityFailed =>
                        {
                            CompactionDisposition::Failed(error.message)
                        }
                        Err(error) => {
                            let outcome = CompactionDisposition::Failed(format!("{error:?}"));
                            if let Err(terminal) = self.settle_background(
                                coordinator.as_ref(),
                                &ready.admission,
                                outcome,
                            ) {
                                return Ok(BlockingRun::Interrupted(terminal));
                            }
                            return self
                                .finish_blocking_result(Err(error), activated_context)
                                .map(BlockingRun::Completed);
                        }
                    };
                if let Err(terminal) =
                    self.settle_background(coordinator.as_ref(), &ready.admission, disposition)
                {
                    return Ok(BlockingRun::Interrupted(terminal));
                }
                generation.await
            }
            CompetitionWinner::Background(Some(CompactionDelivery::Failed(error))) => {
                if error.kind == ContextErrorKind::AuxiliaryDurabilityFailed {
                    self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                    self.durable_failure = Some(error.message.clone());
                    return Err(error);
                }
                self.compaction_control.suppress_optional_for_attempt();
                generation.await
            }
            CompetitionWinner::Background(None) => generation.await,
        };
        let result = match generated {
            Ok(generated) => crate::context::compaction::activate_blocking_generated(
                &mut self.conversation,
                store.as_ref(),
                operation,
                blocking_cancellation,
                owner,
                generated,
            ),
            Err(error) => Err(error),
        };
        self.finish_blocking_result(result, activated_context)
            .map(BlockingRun::Completed)
    }

    fn finish_blocking_result(
        &mut self,
        result: Result<crate::context::compaction::ExecutedCompaction, CompactionExecutionError>,
        activated_context: crate::context::AcceptedContext,
    ) -> Result<CompletedCompaction, ContextError> {
        let completed = match result {
            Ok(completed) => completed,
            Err(CompactionExecutionError::Context(error)) => return Err(error),
            Err(CompactionExecutionError::Committed { completed, error }) => {
                if completed.newly_committed {
                    self.record_persisted_event(completed.persisted_event);
                    self.permission_reminder.mark_after_compaction();
                    self.install_compaction_context(activated_context);
                }
                self.install_committed_carryover(completed.carryover_level)?;
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
            Err(CompactionExecutionError::HeadChanged) => {
                let error = "blocking compaction lost its projection head".to_owned();
                self.durable_failure_kind = Some(DurableFailureKind::Compaction);
                self.durable_failure = Some(error.clone());
                return Err(ContextError::new(ContextErrorKind::Internal, error));
            }
        };
        let input_tokens_after = match &completed.persisted_event.event {
            RuntimeEvent::CompactionCompleted { occupancy, .. } => {
                occupancy.as_ref().map(|occupancy| occupancy.input_tokens)
            }
            _ => None,
        };
        if completed.newly_committed {
            self.record_persisted_event(completed.persisted_event);
            // Successful activated compaction arms the next request's sparse
            // permission reminder (Issue #544 §10).
            self.permission_reminder.mark_after_compaction();
            self.install_compaction_context(activated_context);
        }
        self.install_committed_carryover(completed.carryover_level)?;
        Ok(CompletedCompaction {
            input_tokens_after,
            already_recorded: false,
        })
    }

    /// A selected carryover level becomes effective only after the checkpoint
    /// is confirmed, including the confirmed-commit reconciliation path.
    fn install_committed_carryover(
        &mut self,
        level: Option<crate::model::CarryoverDetailLevel>,
    ) -> Result<(), ContextError> {
        let Some(level) = level else {
            return Ok(());
        };
        let valid = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .is_some_and(|carryover| level >= carryover.level);
        if !valid {
            let message = "committed compaction returned an invalid carryover level";
            self.durable_failure_kind = Some(DurableFailureKind::Compaction);
            self.durable_failure = Some(message.to_owned());
            return Err(ContextError::new(ContextErrorKind::Internal, message));
        }
        self.logical_step
            .frozen_carryover
            .as_mut()
            .expect("validated carryover")
            .level = level;
        Ok(())
    }

    /// A failure is optional only before authoritative state changed and only
    /// for the closed set of Summary/service and bounded planning outcomes.
    /// Do not reinterpret cancellation, durable failures, or malformed state.
    fn compaction_failure_or_defer(
        &mut self,
        operation_id: &crate::runtime::identity::CompactionOperationId,
        error: &ContextError,
        overflow: Option<&ModelError>,
        need: CompactionNeed,
    ) -> Result<CompactionOutcome, Terminal> {
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        if self.cancellation.is_cancelled() || error.kind == ContextErrorKind::Cancelled {
            return Err(self.cancelled_compaction(operation_id));
        }
        let eligible = need == CompactionNeed::Optional
            && overflow.is_none()
            && deferrable_proactive_failure(error.kind);
        if eligible && error.kind == ContextErrorKind::NoEligibleHistory {
            self.emit(RuntimeEvent::CompactionSkipped {
                operation_id: operation_id.clone(),
            });
            if let Some(terminal) = self.durable_failure_terminal_from_state() {
                return Err(terminal);
            }
            if self.cancellation.is_cancelled() {
                return Err(self.cancelled_terminal());
            }
            self.compaction_control.skip_optional_until_progress();
            return Ok(CompactionOutcome::Deferred);
        }
        let terminal = self.compaction_failure(operation_id, error, overflow);
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Err(terminal);
        }
        if self.cancellation.is_cancelled() {
            return Err(self.cancelled_terminal());
        }
        if eligible {
            self.compaction_control.suppress_optional_for_attempt();
            Ok(CompactionOutcome::Deferred)
        } else {
            Err(terminal)
        }
    }

    /// Close an admitted Compaction lifecycle before the Attempt cancellation.
    /// A confirmed checkpoint or an already failed durable authority must not
    /// be followed by a contradictory cancellation fact.
    fn cancelled_compaction(
        &mut self,
        operation_id: &crate::runtime::identity::CompactionOperationId,
    ) -> Terminal {
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return terminal;
        }
        self.emit(RuntimeEvent::CompactionCancelled {
            operation_id: operation_id.clone(),
        });
        self.durable_failure_terminal_from_state()
            .unwrap_or_else(|| self.cancelled_terminal())
    }

    /// Emits `CompactionFailed` and constructs the terminal to use unless the
    /// caller classifies this pre-commit failure as optional. After a provider
    /// overflow, the original error remains the final model failure and the
    /// compaction diagnostic remains in the failure event.
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
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return terminal;
        }
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

#[cfg(test)]
mod tests {
    use super::{CompactionControl, ContextErrorKind, deferrable_proactive_failure};

    #[test]
    fn rapid_optional_refills_suppress_only_after_two_cycles_and_reset_on_progress() {
        let mut control = CompactionControl::default();
        control.record_success(false, Some(83), Some(80), Some(100));
        control.accepted_step();
        control.record_success(false, Some(93), Some(90), Some(100));
        assert!(control.optional_allowed());
        control.accepted_step();
        control.record_success(false, Some(88), Some(85), Some(100));
        assert!(!control.optional_allowed());
        assert!(!control.mandatory_exhausted());

        control.accepted_step();
        assert!(!control.optional_allowed());
        control.accepted_step();
        assert!(control.optional_allowed());
        assert!(!control.mandatory_exhausted());

        control.suppress_optional_for_attempt();
        control.accepted_step();
        control.accepted_step();
        assert!(
            !control.optional_allowed(),
            "a service failure stays suppressed for the Attempt"
        );
        assert!(
            CompactionControl::default().optional_allowed(),
            "a new Attempt starts fresh"
        );
    }

    #[test]
    fn no_eligible_history_only_suppresses_until_new_accepted_history() {
        let mut control = CompactionControl::default();
        control.skip_optional_until_progress();
        assert!(!control.optional_allowed());
        assert!(!control.mandatory_exhausted());
        control.accepted_step();
        assert!(control.optional_allowed());
        control.suppress_optional_for_attempt();
        control.skip_optional_until_progress();
        control.accepted_step();
        assert!(
            !control.optional_allowed(),
            "a prior optional failure remains suppressed"
        );
    }

    #[test]
    fn mandatory_pressure_gets_three_committed_attempts_before_rapid_escalation() {
        let mut control = CompactionControl::default();
        for index in 0..3 {
            control.record_success(true, Some(93), Some(90), Some(100));
            assert_eq!(control.mandatory_exhausted(), index == 2);
            if index != 2 {
                control.accepted_step();
            }
        }
        assert!(!control.optional_allowed());
        control.accepted_step();
        control.accepted_step();
        assert!(!control.mandatory_exhausted());
        assert!(control.optional_allowed());
    }

    #[test]
    fn restored_headroom_missing_measurements_and_changed_limits_reset_refill_evidence() {
        for (input, hard) in [
            (Some(79), Some(100)),
            (None, Some(100)),
            (Some(90), None),
            (Some(90), Some(101)),
        ] {
            let mut control = CompactionControl::default();
            control.record_success(true, Some(93), Some(90), Some(100));
            control.record_success(true, Some(93), Some(90), Some(100));
            control.record_success(true, Some(93), input, hard);
            assert!(!control.mandatory_exhausted());
            assert!(control.optional_allowed());
        }
    }

    #[test]
    fn meaningful_reduction_resets_cycles_even_with_high_fixed_occupancy() {
        for recovered in [5, 15, 20] {
            let mut control = CompactionControl::default();
            for _ in 0..4 {
                control.record_success(true, Some(100), Some(100 - recovered), Some(100));
                control.accepted_step();
                assert!(control.optional_allowed(), "recovered {recovered}% of H");
                assert!(!control.mandatory_exhausted());
            }
        }
        let mut control = CompactionControl::default();
        control.record_success(true, None, Some(90), Some(100));
        control.record_success(true, Some(93), Some(90), Some(100));
        control.record_success(true, Some(93), Some(90), Some(100));
        assert!(
            !control.mandatory_exhausted(),
            "unknown gain cannot certify a rapid cycle"
        );
    }

    #[test]
    fn only_classified_precommit_failures_are_deferrable() {
        for kind in [
            ContextErrorKind::SummaryFailed,
            ContextErrorKind::SummaryInputTooLarge,
            ContextErrorKind::SummaryOutputExhausted,
            ContextErrorKind::NoEligibleHistory,
            ContextErrorKind::CannotFit,
            ContextErrorKind::NoProgress,
            ContextErrorKind::SearchIncomplete,
            ContextErrorKind::CompactionBudgetExceeded,
            ContextErrorKind::AuxiliaryBusy,
            ContextErrorKind::AuxiliaryLifecycleBusy,
            ContextErrorKind::AuxiliaryDeadlineExceeded,
        ] {
            assert!(deferrable_proactive_failure(kind));
        }
        for kind in [
            ContextErrorKind::Cancelled,
            ContextErrorKind::AuxiliaryDurabilityFailed,
            ContextErrorKind::AuxiliarySupervisionFailed,
            ContextErrorKind::AuxiliaryNotStarted,
            ContextErrorKind::SummaryFormatChanged,
            ContextErrorKind::MalformedHistory,
            ContextErrorKind::Internal,
        ] {
            assert!(!deferrable_proactive_failure(kind));
        }
    }
}
