//! The SEP-2663 remote task driver of one invocation (Issue #243).
//!
//! `tasks` owns the protocol translation; this module drives one
//! materialized remote task to exactly one terminal rustX result through
//! the shared physical request owner. Nothing here spawns or detaches: every
//! poll wait, `tasks/update`, and cooperative `tasks/cancel` belongs to the
//! owning invocation's operation future, and an admitted effectful request
//! is never replayed.

use std::time::Instant;

use rmcp::model::{
    CancelTaskParams, CancelTaskRequest, ClientRequest, GetTaskParams, GetTaskRequest,
    UpdateTaskParams, UpdateTaskRequest,
};

use crate::tools::executor::ToolExecutionContext;
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

use super::invocation::McpCallIdentity;
use super::outcome::{McpPhysicalOutcome, McpResponseOutcome, is_transport_loss};
use super::request::McpRequestOwnership;
use super::result::{failed_mcp, mcp_empty_terminal, translate_result};
use super::runtime::McpServerRuntime;
#[cfg(test)]
use super::test_sync;
use super::{bound_error, dispatch, mrtr, tasks};

impl McpServerRuntime {
    /// Drives one already-materialized remote MCP task to **exactly one**
    /// terminal rustX `ToolExecutionResult` (Issue #243).
    ///
    /// # The finite state machine
    ///
    /// ```text
    ///                  CreateTaskResult (from tools/call)
    ///                             |
    ///                             v
    ///                     RemoteTaskActive
    ///                             |
    ///          +------------------+------------------+---------------+
    ///          |                  |                  |               |
    ///      tasks/get          tasks/get          tasks/get     cancellation /
    ///      -> working      -> input_required   -> completed     deadline
    ///          |                  |             / failed /          |
    ///          |                  v             / cancelled         v
    ///          |          one runtime-owned          |        no further poll
    ///          |          Questionnaire              |        or update;
    ///          |                  |                  |        cooperative
    ///          |                  v                  |        tasks/cancel
    ///          |            tasks/update             |              |
    ///          |                  |                  v              v
    ///          +--- bounded, cancellation-aware      exactly one terminal
    ///               wait, then tasks/get ---+        ToolExecutionResult
    /// ```
    ///
    /// There is **no `tools/call` here**: the invocation left the round loop
    /// behind when the task was created, so the original call is never
    /// replayed and no second task identity can appear.
    ///
    /// # The poll loop has no task of its own
    ///
    /// Every await below belongs to the operation future of the owning
    /// [`ToolExecutionHandle`](crate::tools::executor::ToolExecutionHandle):
    /// nothing is spawned, nothing is detached, and no timer outlives the
    /// invocation. Termination is the existing contract's — the invocation's
    /// cancellation signal and the generic Issue #204 deadline — which is why
    /// `ttlMs` is read as the server's retention metadata and never as a
    /// second local deadline.
    pub(super) async fn drive_remote_task(
        &self,
        created: &rmcp::model::CreateTaskResult,
        identity: &McpCallIdentity<'_>,
        context: &ToolExecutionContext<'_>,
        generation: u64,
        started: Instant,
    ) -> ToolExecutionResult {
        // Advertisement is authority in both directions. rustX declares the
        // extension per request; a peer that never declared it in its own
        // negotiated capabilities does not acquire the right to hand this
        // invocation a remote lifecycle by simply answering with one.
        if !self.server_tasks {
            return self.task_outcome_unknown(
                &created.task.task_id,
                "the MCP server answered tools/call with a task result without advertising the \
                 io.modelcontextprotocol/tasks extension",
                context,
                started,
                generation,
            );
        }
        let mut task = match tasks::RemoteTask::create(created) {
            Ok(task) => task,
            Err(violation) => {
                return self.task_outcome_unknown(
                    &created.task.task_id,
                    &violation.diagnostic,
                    context,
                    started,
                    generation,
                );
            }
        };
        if let Err(violation) = tasks::RemoteTask::validate_creation(created) {
            return self
                .abandon_active_task(
                    task.task_id(),
                    &violation.diagnostic,
                    context,
                    started,
                    generation,
                )
                .await;
        }
        // All nonterminal exits converge here: addressable abandonment owes
        // at most one cooperative cancellation; unreachable peers cannot carry it.
        let failure = loop {
            // Wait before every poll, including the creation hint.
            // Cancellation observed here means the next `tasks/get` is never dispatched.
            if !wait_before_next_poll(task.poll_interval(), context).await {
                break TaskDriveFailure::Cancelled(
                    "the rustX execution was cancelled while waiting to poll the MCP task"
                        .to_owned(),
                );
            }
            let observation = match self
                .observe_remote_task(&mut task, context, generation)
                .await
            {
                Ok(observation) => observation,
                Err(failure) => break failure,
            };
            match observation {
                // The task finished. The result is projected through exactly
                // the ordinary MCP result path, so `isError`, content, and
                // structured output behave as they do for a direct call — a
                // `completed` task carrying `isError: true` is a completed
                // task with a failed tool, not a failed task.
                tasks::RemoteTaskObservation::Completed(result) => {
                    return translate_result(*result, context, started);
                }
                // The task's own terminal failure, correlated to this task
                // by the protocol. A known terminal outcome, bounded.
                tasks::RemoteTaskObservation::Failed(error) => {
                    return failed_mcp(
                        &format!(
                            "the MCP task failed ({}): {}",
                            self.generation_tag(generation),
                            bound_error(&error.to_string())
                        ),
                        context,
                        started,
                    );
                }
                // A remote protocol fact, never a rustX cancellation:
                // `CancellationReason` is local authority and rustX did not
                // cancel this. The task reached a terminal state carrying no
                // result, which is a known failed outcome of the tool.
                tasks::RemoteTaskObservation::Cancelled => {
                    return failed_mcp(
                        &format!(
                            "the MCP server cancelled the task and it produced no result ({})",
                            self.generation_tag(generation)
                        ),
                        context,
                        started,
                    );
                }
                tasks::RemoteTaskObservation::InputRequired(requests) => {
                    if let Err(failure) = self
                        .answer_task_input(&mut task, &requests, identity, context, generation)
                        .await
                    {
                        break failure;
                    }
                    // A successful update is acknowledged eventually, so the
                    // next observation may still show the request rustX just
                    // answered. It is deduplicated by the answered-key set,
                    // never by a re-ask and never by a second update.
                }
                tasks::RemoteTaskObservation::Working => {}
            }
        };
        match failure {
            TaskDriveFailure::Unreachable(detail) => {
                self.task_outcome_unknown(task.task_id(), &detail, context, started, generation)
            }
            TaskDriveFailure::AbandonAddressable(detail) | TaskDriveFailure::Cancelled(detail) => {
                self.abandon_active_task(task.task_id(), &detail, context, started, generation)
                    .await
            }
        }
    }

    /// Polls the remote task once and validates the snapshot it answered.
    ///
    /// # Errors
    ///
    /// Returns the failure that ends the drive: a bounded diagnostic about a
    /// task rustX can no longer follow, or a local cancellation that still
    /// owes the server a cooperative `tasks/cancel`.
    async fn observe_remote_task(
        &self,
        task: &mut tasks::RemoteTask,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> Result<tasks::RemoteTaskObservation, TaskDriveFailure> {
        let snapshot = match self.poll_remote_task(task, context, generation).await {
            McpTaskAnswer::Answered(snapshot) => snapshot,
            McpTaskAnswer::Malformed(detail) => {
                return Err(TaskDriveFailure::AbandonAddressable(detail));
            }
            McpTaskAnswer::Cancelled(detail) => {
                return Err(TaskDriveFailure::Cancelled(format!(
                    "{detail}; last observed status {}",
                    task.status()
                )));
            }
            McpTaskAnswer::Unknown(detail) => {
                return Err(TaskDriveFailure::Unreachable(format!(
                    "{detail}; last observed status {}",
                    task.status()
                )));
            }
        };
        // A self-contradictory snapshot fails *this* invocation and nothing
        // else: the peer answered, so the transport is healthy and unrelated
        // calls on this generation stay healthy too.
        task.observe(&snapshot)
            .map_err(|violation| TaskDriveFailure::AbandonAddressable(violation.diagnostic))
    }

    /// Dispatches one `tasks/get` for the invocation's remote task.
    async fn poll_remote_task(
        &self,
        task: &tasks::RemoteTask,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> McpTaskAnswer<rmcp::model::GetTaskResult> {
        let request = ClientRequest::GetTaskRequest(GetTaskRequest::new(GetTaskParams::new(
            task.task_id().to_owned(),
        )));
        self.dispatch_task_request(
            dispatch::GET_TASK_METHOD,
            task.task_id(),
            request,
            McpRequestOwnership::task_control(),
            context,
            generation,
        )
        .await
    }

    /// Turns one task `input_required` snapshot into one runtime-owned
    /// Questionnaire and one `tasks/update` (Issue #243).
    ///
    /// Everything human here is the **existing** #242 machinery: the same
    /// whole-payload translation, the same crate-private
    /// `QuestionnaireRequester`, the same typed answer vocabulary, and the
    /// same `inputResponses` mapping. A task's input request is an MCP
    /// elicitation like any other; nothing about it justifies a second
    /// interaction subsystem.
    #[allow(
        clippy::too_many_lines,
        reason = "one task input round's whole settlement contract stays in one place"
    )]
    async fn answer_task_input(
        &self,
        task: &mut tasks::RemoteTask,
        requests: &rmcp::model::InputRequests,
        identity: &McpCallIdentity<'_>,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> Result<(), TaskDriveFailure> {
        use crate::runtime::interaction::{
            InteractionFailure, InteractionOutcome, InteractionResponse,
        };

        let task_id = task.task_id().to_owned();
        // Nothing here builds a settlement: the driver is the single
        // settlement point, so this function reports only *why* the round
        // could not complete.
        let unsupported =
            |diagnostic: &str| TaskDriveFailure::AbandonAddressable(diagnostic.to_owned());
        // Cancellation observed before publication: no interaction is created
        // at all, so there is no pending prompt to settle and no waiter to
        // race.
        if context.cancellation.is_cancelled() {
            return Err(TaskDriveFailure::Cancelled(
                "the rustX execution was cancelled before the MCP task interaction was published"
                    .to_owned(),
            ));
        }
        // Whole-payload validation before anything is published: a mixed
        // supported/unsupported request set fails here, never half-way
        // through a prompt.
        let plan = match mrtr::plan_round(Some(requests)) {
            Ok(Some(plan)) => plan,
            // Unreachable: an empty request map is refused as a contradictory
            // snapshot before this function is reached.
            Ok(None) => {
                return Err(unsupported(
                    "the MCP task reported input_required with nothing to ask",
                ));
            }
            Err(error) => return Err(unsupported(&error.diagnostic)),
        };
        // The one runtime-owned human-interaction authority — the same
        // crate-private capability the native `ask_user` tool and the
        // synchronous MRTR path consume. A background invocation holds none,
        // and rustX advertised no elicitation capability for it, so a task
        // that asks a background execution for a human fails honestly rather
        // than creating hidden pending UI state.
        let Some(requester) = context.questionnaire_requester() else {
            return Err(unsupported(
                "the MCP task asked for user input, but this execution has no runtime-owned \
                 interaction authority: rustX advertises MCP elicitation only for invocations \
                 that can settle it",
            ));
        };
        let facts = crate::runtime::interaction::QuestionnaireFacts {
            invocation_id: identity.invocation_id.clone(),
            requester: identity.requester(&self.server_id),
            questionnaire: plan.questionnaire().clone(),
        };
        let response = match requester.request_questionnaire(facts).await {
            Ok(InteractionOutcome::Responded {
                response: InteractionResponse::Questionnaire { response },
            }) => response,
            Ok(InteractionOutcome::Responded { response: _ }) => {
                return Err(unsupported(
                    "the MCP elicitation interaction settled with a response of another kind",
                ));
            }
            Ok(InteractionOutcome::Cancelled { reason }) => {
                return Err(TaskDriveFailure::Cancelled(format!(
                    "the MCP task interaction was cancelled ({reason:?})"
                )));
            }
            Ok(InteractionOutcome::DeadlineExpired { kind: _ }) => {
                return Err(TaskDriveFailure::Cancelled(
                    "the execution deadline expired while the MCP task interaction was pending"
                        .to_owned(),
                ));
            }
            Err(failure) => {
                let detail = if failure == InteractionFailure::Unavailable {
                    format!(
                        "the MCP task asked for user input and {failure}, so the call cannot \
                         continue"
                    )
                } else {
                    format!("the MCP elicitation interaction failed: {failure}")
                };
                return Err(unsupported(&detail));
            }
        };
        let input_responses = match plan.responses(&response) {
            Ok(responses) => responses,
            Err(error) => return Err(unsupported(&error)),
        };
        // The keys are recorded **before** the dispatch frontier, so a
        // request rustX has committed to answering is never asked again even
        // if the update itself is never acknowledged. Answering a human twice
        // for one key is not recoverable by retrying.
        if let Err(violation) = task.record_answered(requests.keys().map(String::as_str)) {
            return Err(unsupported(&violation.diagnostic));
        }
        let request = ClientRequest::UpdateTaskRequest(UpdateTaskRequest::new(
            UpdateTaskParams::new(task_id.clone(), input_responses),
        ));
        match self
            .dispatch_task_request::<rmcp::model::TaskAckResult>(
                dispatch::UPDATE_TASK_METHOD,
                &task_id,
                request,
                McpRequestOwnership::task_control(),
                context,
                generation,
            )
            .await
        {
            // The acknowledgement is empty by definition and eventually
            // consistent: it proves the server accepted the responses, never
            // that the next snapshot reflects them.
            McpTaskAnswer::Answered(_) => Ok(()),
            McpTaskAnswer::Cancelled(detail) => Err(TaskDriveFailure::Cancelled(detail)),
            McpTaskAnswer::Malformed(detail) => Err(unsupported(&detail)),
            McpTaskAnswer::Unknown(detail) => Err(TaskDriveFailure::Unreachable(detail)),
        }
    }

    /// Dispatches one SEP-2663 task-control request and reduces its physical
    /// outcome to what the task driver can act on.
    async fn dispatch_task_request<T: tasks::TaskResponse>(
        &self,
        method: &'static str,
        task_id: &str,
        request: ClientRequest,
        ownership: McpRequestOwnership<'_>,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> McpTaskAnswer<T> {
        // Test-only: the deterministic task-request dispatch frontier. A
        // regression parks here, decides a cancellation race, and asserts on
        // what the server actually saw.
        #[cfg(test)]
        test_sync::park_before_task_request(self.server_id.as_str(), method, task_id).await;
        #[cfg(not(test))]
        let _ = task_id;
        // The instant is unused by every branch that survives: a task request
        // never builds a settled result here, because a task-active
        // invocation's certainty rules belong to its driver.
        let started = Instant::now();
        match self
            .dispatch_owned_request(request, ownership, context, generation, started)
            .await
        {
            McpPhysicalOutcome::RefusedByCancellation { reason } => {
                McpTaskAnswer::Cancelled(format!(
                    "{method} was not dispatched because the rustX execution was cancelled ({reason:?})"
                ))
            }
            McpPhysicalOutcome::Refused { detail, .. } => {
                McpTaskAnswer::Unknown(format!("{method} was refused before dispatch: {detail}"))
            }
            McpPhysicalOutcome::Observed {
                response,
                cancellation_won,
                terminated_local_request,
            } => match response {
                McpResponseOutcome::Answered(answered) => match *answered {
                    Ok(result) => match T::decode(result) {
                        Ok(result) => McpTaskAnswer::Answered(result),
                        Err(detail) => McpTaskAnswer::Malformed(format!(
                            "{method}: malformed response: {detail}"
                        )),
                    },
                    // A JSON-RPC error correlated to a task-control request
                    // is a proven answer *to that request* and nothing more:
                    // an expired or unknown task id says the server no longer
                    // has the task, not what the task did.
                    Err(rmcp::service::ServiceError::McpError(error)) => {
                        McpTaskAnswer::Malformed(format!(
                            "{method} was answered with a protocol error: {}",
                            bound_error(&error.to_string())
                        ))
                    }
                    Err(error) => {
                        let detail = format!(
                            "{method} produced no correlated remote response: {}",
                            bound_error(&error.to_string())
                        );
                        if is_transport_loss(&error) && !terminated_local_request {
                            self.note_transport_loss(&detail);
                            McpTaskAnswer::Unknown(detail)
                        } else if cancellation_won {
                            McpTaskAnswer::Cancelled(detail)
                        } else {
                            McpTaskAnswer::Malformed(detail)
                        }
                    }
                },
                McpResponseOutcome::ChannelEnded => {
                    let detail = format!("the MCP transport closed during {method}");
                    if cancellation_won {
                        McpTaskAnswer::Cancelled(detail)
                    } else {
                        self.note_transport_loss(&detail);
                        McpTaskAnswer::Unknown(detail)
                    }
                }
            },
            McpPhysicalOutcome::CancelledPostFrontier { .. } => McpTaskAnswer::Cancelled(format!(
                "the rustX execution was cancelled while {method} was in flight"
            )),
        }
    }

    /// Settles an invocation whose remote task rustX will no longer drive,
    /// after asking the server cooperatively to stop it (Issue #243).
    ///
    /// # Why the acknowledgement proves nothing
    ///
    /// `tasks/cancel` is *cooperative*: SEP-2663 defines its result as an
    /// empty acknowledgement, and an implementation may accept it while the
    /// task keeps running, or having already finished. It is therefore
    /// remote **control**, never remote **evidence**, and it can never turn
    /// this settlement into a proven `Cancelled` or a proven no-effect. It is
    /// also distinct from `notifications/cancelled`, which cancels one
    /// in-flight JSON-RPC request and says nothing about the task at all —
    /// the in-flight poll's own request-level cancellation already happened
    /// inside [`Self::dispatch_owned_request`].
    ///
    /// The invocation therefore settles as `OutcomeUnknown`: the remote task
    /// may continue independently on the server, and rustX does not pretend
    /// otherwise.
    async fn abandon_active_task(
        &self,
        task_id: &str,
        detail: &str,
        context: &ToolExecutionContext<'_>,
        started: Instant,
        generation: u64,
    ) -> ToolExecutionResult {
        if !self.server_tasks {
            return self.task_outcome_unknown(
                task_id,
                &format!("{detail}; no tasks/cancel: Tasks was not negotiated"),
                context,
                started,
                generation,
            );
        }
        if let Some(reason) = self.unusable_reason() {
            return self.task_outcome_unknown(
                task_id,
                &format!("{detail}; no tasks/cancel: {reason}"),
                context,
                started,
                generation,
            );
        }
        let request = ClientRequest::CancelTaskRequest(CancelTaskRequest::new(
            CancelTaskParams::new(task_id.to_owned()),
        ));
        // Exactly one cooperative cancellation per invocation, dispatched
        // through the one task-request owner so its own local HTTP
        // participant is terminated and proven released before this
        // invocation reports.
        let requested = match self
            .dispatch_task_request::<rmcp::model::TaskAckResult>(
                dispatch::CANCEL_TASK_METHOD,
                task_id,
                request,
                McpRequestOwnership::task_cancellation(),
                context,
                generation,
            )
            .await
        {
            McpTaskAnswer::Answered(_) => {
                "the server acknowledged the cooperative tasks/cancel, which does not prove the \
                 remote task stopped"
            }
            McpTaskAnswer::Cancelled(_)
            | McpTaskAnswer::Unknown(_)
            | McpTaskAnswer::Malformed(_) => {
                "the cooperative tasks/cancel produced no acknowledgement"
            }
        };
        self.task_outcome_unknown(
            task_id,
            &format!("{detail}; {requested}"),
            context,
            started,
            generation,
        )
    }

    /// Settles an invocation whose remote task was materialized by a round
    /// that local cancellation had already won against.
    ///
    /// The task exists — the server said so — and rustX never polls it, so
    /// this is [`Self::abandon_active_task`] with the one difference that an
    /// unusable task id cannot be cancelled cooperatively at all.
    pub(super) async fn abandon_remote_task(
        &self,
        created: &rmcp::model::CreateTaskResult,
        context: &ToolExecutionContext<'_>,
        started: Instant,
        generation: u64,
    ) -> ToolExecutionResult {
        let detail = "the rustX execution was cancelled and the MCP server answered the same \
                      round by creating a task, which rustX will not drive";
        match tasks::RemoteTask::create(created) {
            Ok(task) => {
                self.abandon_active_task(task.task_id(), detail, context, started, generation)
                    .await
            }
            Err(violation) => self.task_outcome_unknown(
                &created.task.task_id,
                &format!("{detail}; {}", violation.diagnostic),
                context,
                started,
                generation,
            ),
        }
    }

    /// The one terminal status of an invocation whose remote task rustX
    /// cannot follow to a terminal protocol state.
    ///
    /// A materialized task **is** remote work in progress, so every outcome
    /// other than the task's own terminal state leaves the external result
    /// genuinely unknown — including a cancellation rustX itself decided.
    /// Reporting a proven `Cancelled` there would claim the server stopped,
    /// which no part of the Tasks protocol can establish.
    fn task_outcome_unknown(
        &self,
        task_id: &str,
        detail: &str,
        context: &ToolExecutionContext<'_>,
        started: Instant,
        generation: u64,
    ) -> ToolExecutionResult {
        mcp_empty_terminal(
            ToolExecutionStatus::OutcomeUnknown {
                detail: bound_error(&format!(
                    "the MCP tool materialized remote task {} and its outcome could not be \
                     established ({}): {detail}",
                    bounded_task_id(task_id),
                    self.generation_tag(generation)
                )),
            },
            context,
            started,
        )
    }
}

/// What one SEP-2663 task-control request produced for the task driver.
enum McpTaskAnswer<T> {
    /// The peer answered this request id with a result.
    Answered(T),
    /// A correlated response has the wrong method-specific shape or is an error.
    Malformed(String),
    /// Local cancellation intent decided this request. The remote task is
    /// still the server's, so the driver asks it cooperatively to stop.
    Cancelled(String),
    /// No usable correlated answer exists, and no further protocol step can
    /// establish the task's outcome.
    Unknown(String),
}

/// Why one remote task drive ended without a terminal task state.
///
/// The driver alone settles, after at most one cancellation for an addressable task.
enum TaskDriveFailure {
    /// Local continuation failed but the task remains addressable.
    AbandonAddressable(String),
    /// Transport or generation cannot carry further traffic.
    Unreachable(String),
    /// Local cancellation or a deadline decided it.
    Cancelled(String),
}

/// The cancellation-aware wait before every `tasks/get` poll.
///
/// Returns `false` when local cancellation won the wait, which is the task's
/// **poll dispatch frontier**: the next `tasks/get` is then never dispatched.
///
/// The wait is deliberately a plain `select!` on this invocation's own
/// cancellation signal and one timer owned by this stack frame. Nothing is
/// spawned, nothing is detached, and the timer cannot outlive the operation
/// future — a server's `pollIntervalMs` can therefore never defeat local
/// cancellation, however large it is.
async fn wait_before_next_poll(
    interval: std::time::Duration,
    context: &ToolExecutionContext<'_>,
) -> bool {
    tokio::select! {
        biased;
        () = context.cancellation.cancelled() => false,
        () = async {
            match tokio::time::Instant::now().checked_add(interval) {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        } => true,
    }
}

/// A remote task id trimmed for a diagnostic, so a hostile id cannot inflate
/// one.
fn bounded_task_id(task_id: &str) -> String {
    const LIMIT: usize = 64;
    if task_id.chars().count() <= LIMIT {
        return format!("{task_id:?}");
    }
    format!("{:?}…", task_id.chars().take(LIMIT).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::wait_before_next_poll;
    use crate::runtime::types::CancellationReason;
    use crate::runtime::{CancellationSignal, ExecutionCancellation};
    use crate::tools::mcp::tasks;
    use crate::tools::mcp::test_context::{NoProgress, context, runtime};

    #[tokio::test(start_paused = true)]
    async fn mcp_tasks_poll_wait_preserves_hints_and_is_interruptible() {
        use futures_util::FutureExt as _;
        use std::time::Duration;
        let (_directory, runtime) = runtime("conv_d291e9cf-f815-71c6-a54a-0fcd24614d46");
        let context = context(&runtime, None, &NoProgress);
        for (hint, millis) in [
            (None, 500),
            (Some(0), 25),
            (Some(500), 500),
            (Some(60_000), 60_000),
        ] {
            let wait = wait_before_next_poll(tasks::poll_interval(hint), &context);
            tokio::pin!(wait);
            assert!(wait.as_mut().now_or_never().is_none());
            tokio::time::advance(Duration::from_millis(millis - 1)).await;
            assert!(wait.as_mut().now_or_never().is_none());
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(wait.await);
        }
        let signal = CancellationSignal::new();
        let mut context = context;
        context.cancellation =
            ExecutionCancellation::detached(signal.clone(), CancellationReason::UserRequested);
        let wait = wait_before_next_poll(tasks::poll_interval(Some(u64::MAX)), &context);
        tokio::pin!(wait);
        assert!(wait.as_mut().now_or_never().is_none());
        signal.cancel();
        assert!(!wait.await);
    }
}
