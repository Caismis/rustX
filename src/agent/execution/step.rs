//! The primary execution loop and its logical-step lifecycle: the attempt
//! loop, one logical model step with its retry state machine, turn
//! completion (P -> U -> C ordering, Workflow terminal protocol, tool
//! dispatch), and the mailbox safe boundary between turns.

use super::recovery::SemanticGenerationRecovery;
use super::recovery::{SemanticGenerationAnomaly, transient_retry_delay_ms};
use super::request::{FrozenCarryover, FrozenLogicalInput};
use super::stream::{ModelInvocation, StreamTerminal};
use super::terminal::{Terminal, TerminalCarryoverDecision, TerminalCommitState};
use super::{
    AgentExecution, AgentExecutionResult, DurableFailureKind,
    MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN, MAX_TRANSIENT_MODEL_RETRIES_PER_LOGICAL_STEP,
};
use crate::context::AcceptedContext;
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::model::assembly::ModelEventAssembler;
use crate::model::error::ModelErrorKind;
use crate::model::snapshot::RequestIdentity;
use crate::runtime::identity::MessageId;
use crate::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger};
use crate::runtime::types::RuntimeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogicalModelStepState {
    NotStarted,
    Unresolved,
    CanonicalAccepted,
}

/// The working set of one logical primary step: everything that is reset
/// together when the next logical step begins and reused by every actual
/// request (initial, transient retry, overflow or corrective retry) of the
/// current one.
pub(super) struct LogicalStep {
    /// Workflow correction frozen for this logical generation, including recovery versions.
    pub(super) frozen_workflow_feedback: Option<String>,
    /// The semantic-generation recovery state of the current logical model
    /// step: its one spent corrective-generation budget, shared by every
    /// anomaly class, and — while a corrective generation is pending — the
    /// one bounded corrective hint every actual request of that generation
    /// carries.
    ///
    /// This is noncanonical request-only state. It is frozen into the exact
    /// `RequestSnapshot` of every actual request that used it — durable
    /// audit evidence, so a historical request stays byte-for-byte
    /// reconstructible — but it never enters canonical Assistant/User
    /// history and never survives the corrective generation as semantic
    /// context. The discarded generation itself is never stored here at
    /// all.
    pub(super) semantic_recovery: SemanticGenerationRecovery,
    /// The transient accepted context for the current admitted logical model
    /// step. It is retained across every actual-request retry and discarded
    /// only when the next logical step begins.
    pub(super) accepted_context: Option<AcceptedContext>,
    /// The one publication-audit carryover frozen for the current logical
    /// primary step. It is never reloaded for a retry.
    pub(super) frozen_carryover: Option<FrozenCarryover>,
    /// Latest authorized immutable semantic version of the current logical generation.
    pub(super) current_input: Option<Box<FrozenLogicalInput>>,
    /// The current logical generation's accepted Context identities, exactly once.
    pub(super) context_ids: Vec<MessageId>,
    /// The last request identity that durably crossed model start in the
    /// current logical step. It is the input to the shared live/recovery
    /// carryover selector.
    pub(super) last_started_request: Option<RequestIdentity>,
    /// Whether the current logical step has started model execution without
    /// reaching canonical Assistant acceptance yet.
    pub(super) model_step: LogicalModelStepState,
    /// A terminal cannot be recorded until publication evidence and the
    /// carryover source-selection decision are durably ready.
    pub(super) terminal_commit_state: TerminalCommitState,
    /// The producer decision frozen after the current unresolved step's audit
    /// settles. `Some(None)` is meaningful: the new unresolved step replaces
    /// any older pending source with an explicit clear.
    pub(super) terminal_carryover_decision: TerminalCarryoverDecision,
}

impl LogicalStep {
    /// A fresh logical step carrying the Workflow correction frozen for it.
    pub(super) fn begin(frozen_workflow_feedback: Option<String>) -> Self {
        Self {
            frozen_workflow_feedback,
            semantic_recovery: SemanticGenerationRecovery::default(),
            accepted_context: None,
            frozen_carryover: None,
            current_input: None,
            context_ids: Vec::new(),
            last_started_request: None,
            model_step: LogicalModelStepState::NotStarted,
            terminal_commit_state: TerminalCommitState::Ready,
            terminal_carryover_decision: TerminalCarryoverDecision::Undecided,
        }
    }
}
impl<'a> AgentExecution<'a> {
    /// Runs the attempt to its execution settlement candidate.
    ///
    /// The execution state machine is the settlement authority: the machine
    /// is settled (`complete()` for success, `fail()` for failure and
    /// cancellation) immediately before the single attempt terminal
    /// `RuntimeEvent` is attempted. A terminal Event Journal append failure
    /// does not fabricate an event; the result reports that durable failure
    /// separately.
    ///
    /// # Panics
    ///
    /// Panics only when the loop violates its own state-machine invariants;
    /// a durable terminal write failure is an explicit result, not a panic.
    #[must_use]
    pub fn run(self) -> futures_util::future::BoxFuture<'a, AgentExecutionResult> {
        // Keep the retained frozen working set off each caller's async stack.
        Box::pin(self.run_owned())
    }

    async fn run_owned(mut self) -> AgentExecutionResult {
        self.emit(RuntimeEvent::AttemptStarted {
            attempt_id: self.request.attempt_id.clone(),
        });
        let terminal = if let Err(error) = self.state.start() {
            Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            }
        } else if let Some(message) = self.durable_failure.clone() {
            Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            }
        } else {
            // The attempt's explicit fresh inbound trigger is pending until
            // the first successful model invocation observes it. A pure
            // continuation attempt has no pending trigger: the trigger makes
            // the execution mode explicit.
            self.pending_fresh_inbound = match &self.request.initial_turn_trigger {
                InitialTurnTrigger::FreshInbound(fresh) => Some(fresh.clone()),
                InitialTurnTrigger::Continuation => None,
            };
            let mut terminal = None;
            while terminal.is_none() {
                // Generic Agent Loop cancellation checkpoint:
                // observable cancellation is checked before every model
                // turn begins — the first turn, every continuation
                // after a foreground tool turn, and every continuation
                // caused by a drained inbound batch. This is an
                // intentional pre-1.0 Agent Loop contract refinement:
                // mailbox attachment, mailbox contents, the context
                // runtime, and the provider protocol do not control
                // generic cancellation timing. When cancellation wins
                // here, no `TurnStarted`, no `ModelRequestStarted`, and
                // no adapter invocation happen for the next turn.
                //
                // The check never replaces a terminal outcome already
                // selected at a mailbox safe boundary: a successful
                // no-tool turn whose empty mailbox snapshot settled the
                // attempt as Completed exits this loop before the check
                // runs again, so a later cancellation or enqueue never
                // reopens or reclassifies the completed attempt.
                if self.cancellation.is_cancelled() {
                    terminal = Some(Terminal::Cancelled {
                        reason: self.cancellation.reason(),
                    });
                    break;
                }
                if self.cancellation.interaction_failed() {
                    terminal = Some(Self::interaction_failure_terminal());
                    break;
                }
                terminal = self.run_turn().await;
                if terminal.is_none() && self.cancellation.interaction_failed() {
                    terminal = Some(Self::interaction_failure_terminal());
                }
                // TEST-ONLY continuation boundary: the previous turn is
                // structurally complete (every tool result and every
                // mailbox drain/append of that turn is done) and the
                // loop is about to check cancellation again before the
                // next model turn. Tests park here to make cancellation
                // observable deterministically between turns.
                #[cfg(test)]
                if terminal.is_none()
                    && let Some(pause) = &self
                        .pauses
                        .continuation_pause
                        .lock()
                        .expect("continuation pause lock")
                        .as_ref()
                {
                    pause.park_at_continuation_boundary();
                }
            }
            terminal.expect("the attempt must settle")
        };
        self.settle(&terminal);
        if let Some(changes) = &self.workspace_changes {
            changes.finish().await;
        }
        self.emit_terminal(&terminal);
        let outcome = terminal.outcome();
        AgentExecutionResult {
            attempt_id: self.request.attempt_id,
            outcome,
            terminal_state: self.state.state(),
            conversation: self.conversation,
            durable_failure: self.durable_failure,
            durable_failure_kind: self.durable_failure_kind,
        }
    }

    /// Executes one turn and settles its publication stream exactly once.
    ///
    /// This is the one mutual-exclusion point of the three publication
    /// settlements. A turn that reached canonical acceptance already cleared
    /// its stream through the compound C transition; every other exit —
    /// cancellation, model failure, structural assembly rejection, preflight
    /// rejection, a durable failure — leaves the stream open here and it
    /// terminalizes as an audit. Canonical acceptance and audit
    /// terminalization can therefore never both happen for one stream.
    async fn run_turn(&mut self) -> Option<Terminal> {
        let terminal = self.run_turn_body().await;
        if !self.publication_settlement_failed
            && let Err(error) = self.settle_publication_audit()
        {
            self.logical_step.terminal_commit_state = TerminalCommitState::Blocked;
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        if terminal.is_some()
            && matches!(
                self.logical_step.model_step,
                LogicalModelStepState::Unresolved
            )
        {
            let Some(last_started_request) = self.logical_step.last_started_request.as_ref() else {
                self.logical_step.terminal_commit_state = TerminalCommitState::Blocked;
                let message =
                    "an unresolved logical model step has no durable last-started request";
                self.durable_failure_kind = Some(DurableFailureKind::Publication);
                self.durable_failure = Some(message.to_owned());
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: message.to_owned(),
                        },
                    },
                });
            };
            match crate::publication::select_unresolved_output_source(last_started_request, |id| {
                self.store.load_publication_audit(id)
            }) {
                Ok(source) => {
                    self.logical_step.terminal_carryover_decision =
                        TerminalCarryoverDecision::Replace(source);
                }
                Err(error) => {
                    self.logical_step.terminal_commit_state = TerminalCommitState::Blocked;
                    let message = format!(
                        "unresolved-output carryover source selection failed before attempt terminal: {error}"
                    );
                    self.durable_failure_kind = Some(DurableFailureKind::Publication);
                    self.durable_failure = Some(message.clone());
                    return Some(Terminal::Failed {
                        failure: AttemptFailure::Runtime {
                            error: RuntimeError::DurableStore { message },
                        },
                    });
                }
            }
        }
        terminal
    }

    /// Executes one turn: one model request, its tool calls, and their
    /// results. Returns the terminal outcome when the attempt settled.
    #[allow(clippy::too_many_lines)] // one explicit logical-step retry state machine
    async fn run_turn_body(&mut self) -> Option<Terminal> {
        self.turn += 1;
        // A new logical primary step gets one fresh assembly generation. Every
        // actual request of that step — initial, transient retry, or
        // post-compaction retry — consumes the one shared request ordinal
        // below. Retries never re-enter dynamic context admission.
        // The semantic corrective-generation budget, any unresolved corrective
        // hint, the frozen carryover and input, and the terminal readiness of
        // the step that just ended are all replaced here, so none of them may
        // cross this boundary.
        self.logical_step = LogicalStep::begin(self.workflow_output_feedback.take());
        self.context_runtime.engine.clear_media();
        self.emit(RuntimeEvent::TurnStarted);
        if let Some(message) = self.durable_failure.clone() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        }

        let mut next_request_ordinal = 0_u32;
        let mut transient_retries = 0_u32;
        let mut overflow_retries = 0_u32;
        let prepared = match self.prepare_model_turn(next_request_ordinal).await {
            Ok(prepared) => prepared,
            Err(terminal) => return Some(terminal),
        };
        let started = match self.start_actual_model_request(prepared, &mut next_request_ordinal) {
            Ok(started) => started,
            Err(terminal) => return Some(terminal),
        };
        let mut invocation = match self.consume_invocation(started).await {
            Ok(invocation) => invocation,
            Err(terminal) => return Some(terminal),
        };

        loop {
            let ModelInvocation {
                message_id,
                assembler,
                terminal,
            } = invocation;
            match terminal {
                StreamTerminal::Completed => {
                    // The semantic generation boundary. This stream produced
                    // a canonical model outcome, so a corrective generation —
                    // if this was one — has resolved and its hint must not
                    // reach the tool-result continuation that follows.
                    self.logical_step.semantic_recovery.resolve();
                    return self.complete_turn(message_id, assembler).await;
                }
                StreamTerminal::Failed { error } => {
                    let Some(failed_request_id) = self.last_request_id.clone() else {
                        return Some(Terminal::Failed {
                            failure: AttemptFailure::Runtime {
                                error: RuntimeError::ContractViolation {
                                    message: "a model failure has no started request".to_owned(),
                                },
                            },
                        });
                    };

                    // A semantic generation anomaly — a malformed tool
                    // proposal, a degenerate channel, an exhausted
                    // generation budget — is a model-generation defect, not
                    // a transport defect: the physical generation is
                    // discarded whole and the same logical step gets one
                    // corrective *generation*, carrying bounded corrective
                    // context. The three classes share this one budget, so a
                    // second anomaly of any class terminates the step. The
                    // budget and the corrective context are owned together
                    // by the logical step (`SemanticGenerationRecovery`), so
                    // they can neither reset nor be reset by the transient
                    // and overflow budgets tracked alongside them — and an
                    // actual-request retry inside the corrective generation
                    // is not a second authorization.
                    if let Some(anomaly) = SemanticGenerationAnomaly::classify(&error)
                        && self
                            .logical_step
                            .semantic_recovery
                            .authorize(anomaly, error.message.clone())
                    {
                        let retry_number = next_request_ordinal;
                        match self
                            .regenerate_after_semantic_anomaly(
                                &failed_request_id,
                                retry_number,
                                &mut next_request_ordinal,
                            )
                            .await
                        {
                            Ok(retry_invocation) => {
                                invocation = retry_invocation;
                                continue;
                            }
                            Err(terminal) => return Some(terminal),
                        }
                    }

                    // Context overflow has its own semantic recovery boundary:
                    // estimator correction, compaction, fit validation, then
                    // one new frozen request state. It does not reset or
                    // consume the generic transient budget.
                    if error.kind == ModelErrorKind::ContextWindowExceeded
                        && overflow_retries < MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN
                    {
                        overflow_retries += 1;
                        let retry_number = next_request_ordinal;
                        match self
                            .retry_after_overflow(
                                &error,
                                &failed_request_id,
                                retry_number,
                                &mut next_request_ordinal,
                            )
                            .await
                        {
                            Ok(retry_invocation) => {
                                invocation = retry_invocation;
                                continue;
                            }
                            Err(terminal) => return Some(terminal),
                        }
                    }

                    if error.kind != ModelErrorKind::ContextWindowExceeded
                        && error.retry_disposition
                            == crate::model::error::ModelRetryDisposition::Transient
                        && transient_retries < MAX_TRANSIENT_MODEL_RETRIES_PER_LOGICAL_STEP
                    {
                        transient_retries += 1;
                        let retry_number = next_request_ordinal;
                        let retry_delay_ms =
                            transient_retry_delay_ms(transient_retries, error.retry_after_ms);
                        // Freeze the absolute deadline BEFORE the schedule
                        // commits: once `ModelRetryScheduled` is durable, its
                        // committed delay and the backoff deadline are one
                        // atomic fact, so a clock advance ordered after the
                        // commit always covers the deadline.
                        let retry_deadline_ms = self
                            .monotonic_clock
                            .now_millis()
                            .saturating_add(retry_delay_ms);
                        if let Err(terminal) = self.schedule_transient_retry(
                            failed_request_id,
                            retry_number,
                            retry_delay_ms,
                        ) {
                            return Some(terminal);
                        }
                        if let Err(terminal) = self.wait_for_retry_backoff(retry_deadline_ms).await
                        {
                            return Some(terminal);
                        }
                        match self
                            .retry_frozen_model_turn(retry_number, &mut next_request_ordinal)
                            .await
                        {
                            Ok(retry_invocation) => {
                                invocation = retry_invocation;
                                continue;
                            }
                            Err(terminal) => return Some(terminal),
                        }
                    }

                    return Some(Terminal::Failed {
                        failure: AttemptFailure::Model { error },
                    });
                }
            }
        }
    }

    /// Settles one completed model stream: assembly, usage folding,
    /// continuation retention, message commit, tool execution, and the
    /// turn-completion event. Returns the terminal outcome when the attempt
    /// settled.
    #[allow(clippy::too_many_lines)] // One P -> U -> C ordering, one place.
    async fn complete_turn(
        &mut self,
        assistant_message_id: MessageId,
        assembler: ModelEventAssembler,
    ) -> Option<Terminal> {
        let turn_assembly = match assembler.finish() {
            Ok(assembly) => assembly,
            Err(error) => {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
        };
        // The pending fresh inbound turn is consumed by the first successful
        // model invocation, including a successful ToolCalls response: the
        // model has already observed the inbound turn, so the following
        // tool-only continuation carries no Agent Status unless a new
        // mailbox batch is drained later.
        self.pending_fresh_inbound = None;
        // The model request completion is reported with the canonical
        // final usage: the terminal event's reported usage, else the
        // latest usage update, never a sum of snapshots.
        let reported_usage = turn_assembly.usage;
        let Some(request_id) = self.last_request_id.clone() else {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message: "a model stream completed with no started request".to_owned(),
                    },
                },
            });
        };
        let Some(owner) = self.exact_started_request_owner(&request_id) else {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation {
                        message:
                            "a model stream completed without the matching durable start identity"
                                .to_owned(),
                    },
                },
            });
        };
        // P: the provider outcome of this exact request becomes durable.
        // This downstream copy is derived only after canonical assembly has
        // validated and retained the event's terminal fact.
        let finish_reason = turn_assembly.finish_reason.clone();
        let generation = self.settle_generation_evidence();
        self.emit(RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id,
            finish_reason: finish_reason.clone(),
            usage: reported_usage.clone(),
            generation,
        });
        if let Some(terminal) = self.durable_failure_terminal_from_state() {
            return Some(terminal);
        }
        // U: P is durable and provider completion is structurally accepted,
        // so the remaining publication payload and the publication terminal
        // marker commit in one transaction — and only then is the final
        // buffered payload released. This is the intentional tail-latency
        // cost of correct ordering; with nothing buffered, a terminal-only
        // frame commits and no visible text waits.
        if let Err(error) = self.commit_publication_terminal() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // A provider-reported input measurement is authoritative for the
        // exact projection the completed request used, and remains
        // authoritative for the prefix it covers of every request context
        // that extends it. Both facts travel with the measurement; the
        // engine decides which one applies.
        if let Some(usage) = &reported_usage {
            self.record_provider_observation(usage);
        }
        self.pending_continuation = turn_assembly.continuation;
        if self.pending_continuation.is_some() {
            self.continuation_owner = Some(assistant_message_id.clone());
        } else {
            self.retire_continuation();
        }
        let has_tool_calls = !turn_assembly.tool_calls.is_empty();
        if let Err(error) = self.state.model_finished(has_tool_calls) {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // A Workflow-owned Agent has one reserved terminal protocol. It is
        // handled before ordinary Tool Plane preflight so a provider-shaped
        // `workflow_output` call can never acquire a ToolExecutionId, reach
        // an executor, or create a ToolResult. A mixed or duplicate turn is
        // rejected as one structural unit: no ordinary side effect and no
        // output commit are allowed from that turn.
        if let Some(workflow_output) = self.workflow_output.clone() {
            let protocol_calls = turn_assembly
                .tool_calls
                .iter()
                .filter(|call| call.name == crate::runtime::workflow::WORKFLOW_OUTPUT_TOOL_NAME)
                .collect::<Vec<_>>();
            if !protocol_calls.is_empty() {
                if protocol_calls.len() != 1 || turn_assembly.tool_calls.len() != 1 {
                    self.reject_workflow_output_turn(
                        "workflow_output must be the sole tool-shaped call in an Assistant turn",
                        true,
                    );
                    return None;
                }
                if self.cancellation.is_cancelled() {
                    let reason = self.cancellation.reason();
                    let _ = workflow_output.cancel(reason);
                    return Some(Terminal::Cancelled { reason });
                }
                match workflow_output.submit(protocol_calls[0].arguments.clone()) {
                    crate::runtime::workflow::WorkflowOutputSubmission::Committed => {
                        self.retire_continuation();
                        self.logical_step.model_step = LogicalModelStepState::CanonicalAccepted;
                        if let Err(error) = self.state.tools_finished() {
                            return Some(Terminal::Failed {
                                failure: AttemptFailure::Runtime { error },
                            });
                        }
                        self.emit(RuntimeEvent::TurnCompleted);
                        return Some(Terminal::Completed { finish_reason });
                    }
                    crate::runtime::workflow::WorkflowOutputSubmission::Invalid(feedback) => {
                        self.reject_workflow_output_turn(&feedback, true);
                        return None;
                    }
                    crate::runtime::workflow::WorkflowOutputSubmission::Stale => {
                        self.reject_workflow_output_turn(
                            "workflow_output arrived after the AgentRun terminal state",
                            true,
                        );
                        return None;
                    }
                }
            }
            if !has_tool_calls {
                // Ordinary final Assistant text is not a successful
                // Workflow Agent result. Keep the AgentRun alive for a
                // normal corrective model turn, carrying only bounded
                // request-time feedback; no assistant text enters history.
                self.reject_workflow_output_turn(
                    "Workflow Agent execution must terminate with workflow_output(value), not ordinary Assistant text",
                    false,
                );
                return None;
            }
        }
        // M5 preflight boundary: every model-issued tool call of the turn
        // must resolve structurally (registry identity, execution-policy
        // resolution, runtime metadata extraction, business argument
        // validation) before the Assistant message is committed. An impossible
        // canonical identity mismatch or unregistered tool is a
        // runtime/model-stream contract failure and the Assistant message is
        // never committed. Business JSON Schema validation failures are
        // normal rejected result slots and do not fail the attempt.
        let preflight = match self.preflight_tool_calls(&turn_assembly.tool_calls) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                return Some(Terminal::Failed {
                    failure: AttemptFailure::Runtime { error },
                });
            }
        };
        if let Err(error) =
            self.commit_assistant_message(&assistant_message_id, &turn_assembly.content)
        {
            return Some(self.commit_failure_terminal(
                "the assembled Assistant message cannot be committed",
                error,
            ));
        }
        // C succeeded: the logical model step is no longer unresolved, even
        // if a later tool/result phase causes the attempt itself to fail.
        self.logical_step.model_step = LogicalModelStepState::CanonicalAccepted;
        self.continuation_source = if self.pending_continuation.is_some() {
            let input = self
                .logical_step
                .current_input
                .as_ref()
                .expect("accepted output has frozen actual request");
            let output = self
                .conversation
                .ledger()
                .get(&assistant_message_id)
                .expect("accepted Assistant is committed");
            Some(crate::runtime::continuation::AcceptedOutputContinuationSource::from_accepted_output(
                input.snapshot.request_id.clone(), &self.request.conversation_id,
                &input.request.messages, output,
            ).expect("canonical accepted output is Assistant"))
        } else {
            None
        };
        if !has_tool_calls {
            self.emit(RuntimeEvent::TurnCompleted);
            if let Some(terminal) = self.durable_failure_terminal_from_state() {
                return Some(terminal);
            }
            // Safe boundary for a completed no-tool turn: the attempt may
            // settle only when the boundary snapshot observes no eligible
            // inbound work. A drained batch keeps the attempt running for
            // one further model turn, so a pending inbound message prevents
            // a successful Stop from settling before it is observed.
            return match self.safe_boundary_drain() {
                Ok(true) => None,
                Ok(false) => Some(Terminal::Completed { finish_reason }),
                Err(terminal) => Some(terminal),
            };
        }
        // The entire tool-result batch is structurally settled exactly once
        // inside `execute_tools` before this point returns: every logical
        // call of the committed batch receives exactly one canonical
        // attempt-facing result slot, committed in original model call
        // order. Attempt cancellation can still settle the attempt as
        // cancelled after the structurally complete result batch is
        // committed; no next model turn starts after cancellation.
        let settled = match self
            .execute_tools(
                &turn_assembly.tool_calls,
                turn_assembly
                    .content
                    .iter()
                    .enumerate()
                    .filter(|(_, block)| {
                        matches!(
                            block,
                            crate::message::types::AssistantContentBlock::ToolCall(_)
                        )
                    })
                    .map(|(index, _)| {
                        crate::message::types::ToolCallOccurrenceRef::new(
                            assistant_message_id.clone(),
                            crate::message::types::ContentBlockIndex::new(
                                u32::try_from(index).expect("canonical block index"),
                            ),
                        )
                    })
                    .collect(),
                preflight,
            )
            .await
        {
            Ok(settled) => settled,
            Err(error) => {
                return Some(
                    self.commit_failure_terminal("a tool result cannot be committed", error),
                );
            }
        };
        // Cancellation observed before the observation phase begins wins
        // immediately: the batch is already structurally settled, so there is
        // no useful deferred model context to produce and no observation runs.
        if self.cancellation.is_cancelled() {
            return Some(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // The immutable tool-result observation pass runs only here, after
        // the complete owning batch is structurally settled. Observer failure
        // therefore cannot split the batch or prevent a committed Assistant
        // tool-call message from receiving its complete canonical result
        // batch.
        if let Err(terminal) = self.run_tool_result_observations(&settled).await {
            return Some(terminal);
        }
        if let Err(error) = self.state.tools_finished() {
            return Some(Terminal::Failed {
                failure: AttemptFailure::Runtime { error },
            });
        }
        // Safe boundary after a structurally complete tool turn: every
        // foreground call of the turn executed and every ToolMessage was
        // committed before this point. One finite mailbox drain may attach
        // an inbound batch to the continuation; the drain never splits the
        // tool-result batch.
        self.safe_boundary_drain().err()
    }

    /// Retains bounded request-time feedback for a Workflow Agent protocol
    /// correction and, for a tool-shaped turn, completes the Agent Loop's
    /// ordinary structural tool phase without dispatching anything.
    fn reject_workflow_output_turn(&mut self, feedback: &str, had_tool_calls: bool) {
        self.workflow_output_feedback = Some(crate::runtime::subagent::bound_utf8(
            feedback.to_owned(),
            2048,
        ));
        self.retire_continuation();
        self.logical_step.model_step = LogicalModelStepState::CanonicalAccepted;
        if had_tool_calls {
            let _ = self.state.tools_finished();
        }
        self.emit(RuntimeEvent::TurnCompleted);
    }

    /// Relinquish native state, accepted-output provenance and its planning
    /// anchor together. This never changes canonical accepted output.
    pub(super) fn retire_continuation(&mut self) {
        self.pending_continuation = None;
        self.continuation_source = None;
        self.continuation_owner = None;
    }

    /// The mailbox-specific safe boundary: exactly one finite inbound
    /// snapshot after the current turn is structurally complete.
    ///
    /// This function is inbound-boundary semantics only, separate from the
    /// generic Agent Loop cancellation checkpoint (which lives in `run()`
    /// before every model turn). The conversation tool runtime owns the one
    /// canonical mailbox/durable inbox of the conversation; the loop selects
    /// and adopts exactly that boundary, so background terminal
    /// notifications always enter the same durable path the Agent Loop
    /// adopts. With no pending items the snapshot observes no state and the
    /// function returns `Ok(false)`.
    ///
    /// Cancellation wins before selection: when cancellation is already
    /// observable, no selection/adoption happens, all pending items stay
    /// durably pending, and the attempt settles cancelled. Otherwise one
    /// finite watermark is selected and atomically adopted into the durable
    /// canonical ledger, then the complete batch is appended synchronously
    /// as distinct canonical `UserMessageBlock` values in inbound sequence
    /// order — the batch is never partially consumed and never requeued. The
    /// whole adopted batch becomes one new [`FreshInboundTurn`] in sequence
    /// order, so the next model request receives exactly one Agent Status
    /// snapshot targeting the final adopted message (the highest-sequence
    /// item).
    ///
    /// Returns `Ok(true)` when one complete batch was adopted, `Ok(false)`
    /// when the snapshot observed an empty inbox, and the attempt terminal
    /// when cancellation was observable before the snapshot.
    fn safe_boundary_drain(&mut self) -> Result<bool, Terminal> {
        let mailbox = self.tool_runtime.mailbox();
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        // Selection freezes the finite watermark (non-destructive): an
        // acceptance that linearizes after this point can never join the
        // selected batch. A durable selection failure is a
        // durable-authority failure (Issue #63): the settled result carries
        // it to the coordinator.
        let batch = match mailbox.select_pending_batch() {
            Ok(Some(batch)) => batch,
            Ok(None) => return Ok(false),
            Err(error) => {
                return Err(self.durable_failure_terminal(
                    "the durable pending inbound inbox cannot be selected",
                    &error,
                ));
            }
        };
        // Prepare the canonical transition **before** the durable adoption
        // commit: validate every fallible in-memory condition now, so the
        // post-commit installation is infallible (Finding 2). These checks
        // validate stable identities; committed receipts supply the content. A validation failure leaves
        // every item pending and adopts nothing.
        for item in batch.items() {
            let block = crate::durable::inbox::canonical_block(item.message());
            match self.conversation.prepare_commit(&block) {
                Ok(_) => {}
                Err(error) => {
                    return Err(Terminal::Failed {
                        failure: AttemptFailure::Runtime {
                            error: RuntimeError::ContractViolation {
                                message: format!(
                                    "a drained inbound message cannot be prepared: {error}"
                                ),
                            },
                        },
                    });
                }
            }
        }
        let adopted = mailbox
            .adopt_pending_batch(&batch, Some(self.request.attempt_id.clone()))
            .map_err(|error| {
                self.durable_failure_terminal("a selected inbound batch cannot be adopted", &error)
            })?;
        let adoption = adopted.adoption;
        let canonical_receipts = adopted.canonical_receipts;
        let adopted = adopted.items;
        if adopted.is_empty() {
            return Ok(false);
        }
        let prepared: Vec<_> = adopted
            .iter()
            .map(|item| {
                self.conversation
                    .prepare_commit(&crate::durable::inbox::canonical_block(item.message()))
                    .expect("adopted identity was validated under exclusive ownership")
            })
            .collect();
        let fresh = FreshInboundTurn::new(
            adopted
                .iter()
                .map(|item| item.message().id.clone())
                .collect(),
        )
        .expect("nonempty ordered committed identities");
        for ((commit, _item), receipt) in
            prepared.into_iter().zip(&adopted).zip(&canonical_receipts)
        {
            // Infallible: every adopted identity was validated above under
            // exclusive ownership of the conversation state.
            let block = commit.message().clone();
            self.conversation.install_prepared(commit);
            if let Some(observer) = self.observer {
                observer.observe_committed(&self.request.attempt_id, &block, receipt);
            }
        }
        // The adoption fact is published after its canonical messages, with
        // its exact Journal sequence. Trace projects it as the ledger's User
        // record, and the observation queue holds its receipt until exactly
        // this publication releases it.
        if let (Some(observer), Some(adoption)) = (self.observer, adoption.as_ref()) {
            observer.observe_event(
                &self.request.attempt_id,
                &adoption.event,
                adoption.sequence,
                adoption.tool_occurrence.as_ref(),
                adoption.observation.as_deref(),
            );
        }
        self.pending_fresh_inbound = Some(fresh);
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
