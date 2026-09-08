//! Caller-independent foreground execution and physical settlement authority.
//!
//! Callers own admission, identity, and publication. This owner consumes exactly
//! one executor handle and returns execution truth; it cannot commit history or
//! advance a Workflow graph.

use crate::runtime::MonotonicClock;
use crate::tools::deadline::{
    TOOL_SETTLEMENT_CONTROL_GUARD, ToolCancellationCause, ToolDeadlineKind,
    ToolExecutionDeadlinePolicy, ToolProgressCapability, ToolSettlementCertainty, deadline_after,
};
use crate::tools::executor::{
    ProgressReporter, ToolExecutionContext, ToolExecutor, ToolSettlement,
};
use crate::tools::types::{
    ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolProgress,
};

/// Attempt-frozen native invocation resources. They confer neither canonical
/// history mutation nor model-call authority on a fixed-program caller.
#[derive(Clone)]
pub(crate) struct NativeInvocationServices {
    pub lifecycle: crate::agent::AttemptLifecycle,
    pub runtime: crate::tools::runtime::ConversationToolRuntime,
    pub clock: std::sync::Arc<dyn MonotonicClock>,
    pub leaf_policy: ToolExecutionDeadlinePolicy,
    pub turn: u32,
    pub scheduling: std::sync::Arc<tokio::sync::RwLock<()>>,
    pub descendants: NativeChildScope,
}

/// Local ownership accounting only: no outcomes, graph or physical execution.
/// A composite's control guard starts after these native owners have drained.
#[derive(Clone)]
pub(crate) struct NativeChildScope(tokio::sync::watch::Sender<usize>);

impl Default for NativeChildScope {
    fn default() -> Self {
        Self(tokio::sync::watch::Sender::new(0))
    }
}

impl NativeChildScope {
    pub(crate) fn enter(&self) -> NativeChildLease {
        self.0.send_modify(|active| *active += 1);
        NativeChildLease(self.clone())
    }

    async fn drained(&self) {
        let mut receiver = self.0.subscribe();
        receiver
            .wait_for(|active| *active == 0)
            .await
            .expect("scope owns sender");
    }
}

pub(crate) struct NativeChildLease(NativeChildScope);
impl Drop for NativeChildLease {
    fn drop(&mut self) {
        self.0.0.send_modify(|active| *active -= 1);
    }
}

/// One permission gate over immutable, already prepared invocation facts.
/// The interaction response has no replacement argument or selector channel.
pub(crate) async fn authorize(
    lifecycle: &crate::agent::lifecycle::AttemptLifecycle,
    view: &crate::agent::lifecycle::PreToolView<'_>,
    cancellation: &crate::runtime::cancellation::ExecutionCancellation,
) -> Result<Option<ToolExecutionResult>, crate::runtime::interaction::InteractionFailure> {
    use crate::agent::lifecycle::PreToolDecision;
    use crate::runtime::interaction::{ApprovalDecision, InteractionOutcome, InteractionResponse};
    let cancelled = || terminal(cancellation.native_status(ToolCancellationPhase::BeforeStart));
    if cancellation.is_cancelled() {
        return Ok(Some(cancelled()));
    }
    let policy = lifecycle.pre_tool_policy();
    let raw = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Ok(Some(cancelled())),
        decision = policy.evaluate(view) => decision,
    };
    if cancellation.is_cancelled() {
        return Ok(Some(cancelled()));
    }
    let decision = raw.unwrap_or_else(|error| PreToolDecision::Deny {
        reason: format!("pre-tool policy failed closed: {}", error.message),
    });
    let reason = match decision {
        PreToolDecision::Allow => return Ok(None),
        PreToolDecision::Deny { reason } => reason,
        PreToolDecision::Ask { reason } => {
            let response = lifecycle
                .request_approval(
                    view.attempt_id.clone(),
                    view.approval_facts(reason),
                    cancellation.clone(),
                )
                .await;
            if cancellation.is_cancelled() {
                return Ok(Some(cancelled()));
            }
            match response {
                Ok(InteractionOutcome::DeadlineExpired { .. }) => {
                    return Ok(Some(terminal(ToolExecutionStatus::TimedOut)));
                }
                Ok(InteractionOutcome::Responded {
                    response:
                        InteractionResponse::Approval {
                            decision: ApprovalDecision::Allow,
                        },
                }) => return Ok(None),
                Ok(InteractionOutcome::Responded {
                    response:
                        InteractionResponse::Approval {
                            decision: ApprovalDecision::Deny { reason },
                        },
                }) => reason,
                Ok(
                    InteractionOutcome::Responded { .. } | InteractionOutcome::ReviewInvalidated,
                ) => "approval interaction returned a mismatched response".into(),
                Ok(InteractionOutcome::Cancelled { reason }) => {
                    return Ok(Some(terminal(ToolExecutionStatus::Cancelled {
                        reason,
                        phase: ToolCancellationPhase::BeforeStart,
                    })));
                }
                Err(failure) if failure.is_unavailable() => {
                    "interaction provider unavailable; approval failed closed".into()
                }
                Err(failure) => return Err(failure),
            }
        }
    };
    Ok(Some(terminal(ToolExecutionStatus::Denied { reason })))
}

pub(crate) fn terminal(status: ToolExecutionStatus) -> ToolExecutionResult {
    ToolExecutionResult {
        status,
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// Facts selected by the invocation owner, independently of caller framing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InvocationFact {
    Deadline { kind: ToolDeadlineKind },
    CancellationRequested { cause: ToolCancellationCause },
    SettlementObserved { certainty: ToolSettlementCertainty },
    SettlementControlFailed { reason: String },
}

/// Native execution facts for callers without canonical `ToolCall` framing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeInvocationFact {
    Prepared {
        tool_name: String,
        arguments_digest: String,
    },
    Started,
    Progress {
        progress: ToolProgress,
    },
    Lifecycle {
        fact: InvocationFact,
    },
    Completed {
        status: ToolExecutionStatus,
    },
}

/// Frozen policy and clock authority for one foreground invocation.
pub(crate) struct ForegroundInvocation<'a> {
    pub clock: &'a dyn MonotonicClock,
    pub policy: ToolExecutionDeadlinePolicy,
    pub registration: crate::tools::deadline::ForegroundPolicy,
    #[cfg(test)]
    pub deadline_armed: Option<&'a (dyn Fn(u64) + Send + Sync)>,
    #[cfg(test)]
    pub cancellation_won: Option<&'a (dyn Fn() + Send + Sync)>,
    #[cfg(test)]
    pub completion_won: Option<&'a (dyn Fn() + Send + Sync)>,
}

enum Winner {
    Cancellation(ToolCancellationCause),
    Deadline(ToolDeadlineKind),
    Physical(ToolExecutionResult),
}

struct Progress<'a> {
    downstream: &'a dyn ProgressReporter,
    clock: &'a dyn MonotonicClock,
    liveness: Option<tokio::sync::watch::Sender<u64>>,
}

impl ProgressReporter for Progress<'_> {
    fn report(&self, progress: ToolProgress) {
        // Stamp genuine executor evidence before any observer can delay it.
        if let Some(sender) = &self.liveness {
            sender.send_replace(self.clock.now_millis());
        }
        self.downstream
            .report(crate::tools::limits::bound_tool_progress(progress));
    }
}

impl ForegroundInvocation<'_> {
    /// The caller has committed its start frontier. Construction and the first
    /// operation poll still independently reject observable cancellation.
    #[allow(clippy::too_many_lines)] // Keep the single arbitration/settlement state machine linear.
    pub(crate) async fn execute(
        &self,
        executor: &dyn ToolExecutor,
        invocation: ToolInvocation,
        progress_capability: ToolProgressCapability,
        context: ToolExecutionContext<'_>,
    ) -> (ToolExecutionResult, Vec<InvocationFact>) {
        let descendants = context
            .subagent_context()
            .and_then(|context| context.native.as_ref())
            .map(|services| services.descendants.clone());
        let started = self.clock.now_millis();
        let policy = self.registration.resolve(self.policy);
        let hard_deadline = policy.hard_deadline_millis(started);
        #[cfg(test)]
        if let Some(armed) = self.deadline_armed {
            armed(hard_deadline);
        }
        let idle = policy.effective_idle_liveness(progress_capability);
        let (sender, mut receiver) = tokio::sync::watch::channel(started);
        let progress = Progress {
            downstream: context.progress,
            clock: self.clock,
            liveness: idle.map(|_| sender),
        };
        let ancestor = context.cancellation.clone();
        let (trigger, cancellation) = ancestor.child_execution();
        let context = context.reborrow(cancellation, &progress);
        let handle = crate::tools::executor::start_tool_execution(executor, invocation, context);
        let completion = handle.completion;
        let settlement = handle.settlement;
        tokio::pin!(completion);
        tokio::pin!(settlement);
        let idle_wait = async {
            let Some(idle) = idle else {
                return std::future::pending::<()>().await;
            };
            let mut observed = *receiver.borrow_and_update();
            loop {
                tokio::select! {
                    biased;
                    () = self.clock.wait_until_millis(deadline_after(observed, idle)) => {
                        let latest = *receiver.borrow_and_update();
                        if latest == observed { break; }
                        observed = latest;
                    },
                    changed = receiver.changed() => {
                        if changed.is_err() { std::future::pending::<()>().await; }
                        observed = *receiver.borrow_and_update();
                    },
                }
            }
        };
        // Sole terminal winner: cancellation, hard, idle, physical completion.
        // The pinned settlement plane retains operation ownership after select.
        let winner = tokio::select! {
            biased;
            () = ancestor.cancelled() => {
                let reason = ancestor.native_cause();
                #[cfg(test)]
                if let Some(hook) = self.cancellation_won { hook(); }
                Winner::Cancellation(reason)
            },
            () = self.clock.wait_until_millis(hard_deadline) => Winner::Deadline(ToolDeadlineKind::Hard),
            () = idle_wait => Winner::Deadline(ToolDeadlineKind::Idle),
            result = completion.as_mut() => {
                #[cfg(test)]
                if let Some(hook) = self.completion_won { hook(); }
                Winner::Physical(result)
            },
        };
        let mut facts = Vec::new();
        let cause = match winner {
            Winner::Physical(mut result) => {
                if let ToolExecutionStatus::Cancelled { phase, .. } = &mut result.status {
                    *phase = ToolCancellationPhase::DuringExecution;
                }
                return (result, facts);
            }
            Winner::Cancellation(cause) => cause,
            Winner::Deadline(kind) => {
                facts.push(InvocationFact::Deadline { kind });
                ToolCancellationCause::Deadline(kind)
            }
        };
        trigger.cancel(cause);
        facts.push(InvocationFact::CancellationRequested { cause });
        let guard = deadline_after(self.clock.now_millis(), TOOL_SETTLEMENT_CONTROL_GUARD);
        let guard_wait = async {
            match self.registration {
                crate::tools::deadline::ForegroundPolicy::Leaf => {
                    self.clock.wait_until_millis(guard).await;
                }
                crate::tools::deadline::ForegroundPolicy::Composite { .. } => {
                    if let Some(scope) = descendants {
                        scope.drained().await;
                    }
                    // Once native child ownership is gone, the composite's
                    // own control plane has the same finite guard as a leaf.
                    self.clock
                        .wait_until_millis(deadline_after(
                            self.clock.now_millis(),
                            TOOL_SETTLEMENT_CONTROL_GUARD,
                        ))
                        .await;
                }
            }
        };
        let evidence = tokio::select! {
            biased;
            evidence = settlement.as_mut() => Some(evidence),
            () = guard_wait => None,
        };
        let result = match evidence {
            Some(ToolSettlement::Confirmed(mut result)) => {
                facts.push(InvocationFact::SettlementObserved {
                    certainty: ToolSettlementCertainty::Confirmed,
                });
                if matches!(result.status, ToolExecutionStatus::Cancelled { .. }) {
                    result.status = match cause {
                        ToolCancellationCause::Attempt(reason) => ToolExecutionStatus::Cancelled {
                            reason,
                            phase: ToolCancellationPhase::DuringExecution,
                        },
                        ToolCancellationCause::Deadline(_) => ToolExecutionStatus::TimedOut,
                    };
                }
                result
            }
            Some(ToolSettlement::Unconfirmed { detail }) => {
                facts.push(InvocationFact::SettlementObserved {
                    certainty: ToolSettlementCertainty::Unconfirmed,
                });
                unknown(detail)
            }
            None => {
                let reason = "the executor's settlement control plane did not return within the guard window after the cancellation request; this is an executor settlement-contract violation, not proof about the physical operation".to_owned();
                facts.push(InvocationFact::SettlementControlFailed {
                    reason: reason.clone(),
                });
                unknown(reason)
            }
        };
        (result, facts)
    }
}

fn unknown(detail: String) -> ToolExecutionResult {
    terminal(ToolExecutionStatus::OutcomeUnknown { detail })
}
