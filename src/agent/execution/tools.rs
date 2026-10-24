//! Tool execution of one committed Assistant tool-call batch: preflight,
//! pre-tool decisions, deterministic sequential/parallel scheduling,
//! foreground and background dispatch, ordered structural settlement, and
//! the tool-result observation pass.

use super::AgentExecution;
use super::terminal::{CanonicalCommitError, Terminal};
use crate::agent::lifecycle::{ObservedToolInvocation, PreToolView, ToolResultObservation};
use crate::agent::observer::AgentExecutionObserver;
use crate::context::{
    DeferredContextProposal, MAX_DEFERRED_CONTEXT_PROPOSALS, MAX_PROPOSALS_PER_CONTRIBUTOR,
    PostToolBatchOpportunity, validate_user_message_proposal,
};
use crate::durable::TranscriptCursor;
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::{MessageBlock, ToolMessageBlock};
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolId};
use crate::runtime::interaction::InteractionFailure;
use crate::runtime::types::{CancellationReason, RuntimeError};
use crate::tools::background::BackgroundDispatchOutcome;
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{
    PreflightOutcome, PreparedInvocation, ProgressReporter, ToolExecutionContext, ToolRegistry,
};
use crate::tools::types::{
    ToolCall, ToolCancellationPhase, ToolConcurrencyPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolProgress,
};
use futures_util::StreamExt;
use std::sync::Arc;

/// The attempt-local `PostToolBatch` status opportunity together with the
/// durable transcript position frozen when it was established.
///
/// The eligibility marker and its placement fact are one value because one
/// event establishes both — the canonical `ToolResult` batch commit — and
/// they must never be able to drift apart. [`PostToolBatchOpportunity`]
/// stays the pure eligibility marker the status modules see; the cursor
/// beside it is an ordering fact, not a presentation instruction, and no
/// layer below the Runtime Client interprets it.
///
/// Freezing here rather than at observation time is the whole point. A
/// composed status is semantically determined at the primary-step
/// preparation that consumed this opportunity, but it is not *observed*
/// until the durable model-turn-start commit has landed. Inbound acceptance
/// is an independent durable boundary and may legally commit in that window;
/// a placement derived afterwards from "the newest durable position" would
/// follow that unrelated inbound turn instead of the tool batch that caused
/// the composition.
pub(super) struct PendingPostToolBatch {
    /// The eligibility marker consumed by the next primary-step preparation.
    pub(super) opportunity: PostToolBatchOpportunity,
    /// The durable position of the canonical `ToolResult` batch that
    /// established the marker; `None` when that batch committed no visible
    /// transcript item.
    pub(super) transcript_anchor: Option<TranscriptCursor>,
}

/// One result slot of a committed tool-call batch, preallocated in model
/// call order so completion timing can never influence message identities or
/// canonical ordering.
struct CallSlot {
    call: ToolCall,
    /// The canonical registry-resolved tool identity of the call. Both
    /// preflight outcomes carry it, so a rejected slot is identified by the
    /// registry's resolution rather than by the raw model-issued value.
    tool_id: ToolId,
    /// The canonical registry-resolved typed origin of the tool.
    origin: ToolOrigin,
    prepared: Option<PreparedInvocation>,
    result: Option<ToolExecutionResult>,
    /// The authoritative per-call executor-start frontier. Once true, this
    /// call can never receive a `BeforeStart` cancellation result.
    executor_started: bool,
    /// The retained execution facts of the call (bounded progress
    /// observations, then the deadline intent and cancellation/settlement
    /// control-plane facts when a non-physical winner drove a settlement
    /// phase), committed ahead of the call's terminal completion event.
    execution_facts: Vec<RuntimeEvent>,
}

/// The immutable facts of one settled call of a structurally settled batch.
///
/// These are copies of exactly what was committed as canonical history, kept
/// in canonical model call order. They are the only input the tool-result
/// observation pass receives, so an observer can neither reach the live call
/// slot nor influence the committed result.
pub(super) struct SettledCall {
    batch_position: usize,
    call_id: ToolCallId,
    tool_id: ToolId,
    origin: ToolOrigin,
    /// The immutable invocation facts, absent exactly when preflight rejected
    /// the call before invocation resolution.
    invocation: Option<ObservedToolInvocation>,
    result: ToolExecutionResult,
}

/// One deterministic scheduling phase of a tool-call batch.
enum Group {
    /// The slot is already settled (validation rejection); no barrier.
    Trivial,
    /// An exclusive scheduling barrier.
    Sequential,
    /// Adjacent parallel invocations executing concurrently.
    Parallel,
}

/// The scheduling phase beginning at `index`: a `Sequential` invocation is
/// an exclusive barrier; adjacent `Parallel` invocations form one group.
fn group_at(slots: &[CallSlot], index: usize) -> Group {
    let Some(prepared) = slots[index].prepared.as_ref() else {
        return Group::Trivial;
    };
    match prepared.concurrency {
        ToolConcurrencyPolicy::Sequential => Group::Sequential,
        ToolConcurrencyPolicy::Parallel => Group::Parallel,
    }
}

/// The exclusive end of the parallel group beginning at `index`: every
/// adjacent `Parallel` invocation forms one group.
fn parallel_group_end(slots: &[CallSlot], index: usize) -> usize {
    let mut end = index + 1;
    while end < slots.len()
        && slots[end]
            .prepared
            .as_ref()
            .is_some_and(|candidate| candidate.concurrency == ToolConcurrencyPolicy::Parallel)
    {
        end += 1;
    }
    end
}

/// A failed tool result.
fn failed_result(error: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Failed {
            error: error.to_owned(),
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

/// A cancelled tool result carrying the attempt cancellation reason.
fn cancelled_result(reason: CancellationReason) -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Cancelled {
            reason,
            phase: ToolCancellationPhase::BeforeStart,
        },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

/// The bounded foreground progress buffer of one active tool call.
///
/// One foreground invocation owns exactly one buffer. The executor's
/// progress reports are normalized through the one shared UTF-8-safe bound
/// ([`bound_tool_progress`], the same normalization the background registry
/// uses) and retained under the explicit tool-plane cardinality bound
/// [`MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`]. Once the bound is reached,
/// the first `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1` observations are
/// pinned and the final slot tracks the newest observation, so retained
/// progress always ends with the most recent executor state and the buffer
/// never exceeds the bound while the executor is still running.
///
/// The buffer is transient current-execution state: only the retained
/// observations become canonical `ToolExecutionProgress` Event Journal facts
/// at batch commit, before their completion event. Coalesced observations
/// never cross the durable execution-fact commit point.
struct ForegroundProgressBuffer {
    call_id: ToolCallId,
    tool_id: ToolId,
    events: std::sync::Mutex<Vec<RuntimeEvent>>,
}

impl ForegroundProgressBuffer {
    /// An empty bounded buffer for one foreground invocation.
    fn new(call_id: ToolCallId, tool_id: ToolId) -> Self {
        Self {
            call_id,
            tool_id,
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Drains the retained progress events in observation order, newest
    /// last. Called exactly once, when the invocation structurally settles.
    pub(super) fn take(&self) -> Vec<RuntimeEvent> {
        std::mem::take(&mut *self.events.lock().expect("progress buffer lock"))
    }

    /// The number of retained observations; test-only invariant probe.
    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.events.lock().expect("progress buffer lock").len()
    }
}

impl ProgressReporter for ForegroundProgressBuffer {
    fn report(&self, progress: ToolProgress) {
        let bounded = crate::tools::limits::bound_tool_progress(progress);
        let event = RuntimeEvent::ToolExecutionProgress {
            tool_call_id: self.call_id.clone(),
            tool_id: self.tool_id.clone(),
            execution_id: None,
            progress: bounded,
        };
        let mut events = self.events.lock().expect("progress buffer lock");
        if events.len() < crate::tools::limits::MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL {
            events.push(event);
        } else {
            // At capacity the earliest observations are pinned and the final
            // slot tracks the newest observation: first
            // `MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL - 1` progress events,
            // then the latest progress event.
            *events
                .last_mut()
                .expect("the foreground progress bound is positive") = event;
        }
    }
}

/// The progress fanout of one foreground invocation (Issue #178).
///
/// One executor report feeds two consumers of the same bounded value:
///
/// - the durable [`ForegroundProgressBuffer`], unchanged: the retained
///   observations commit as canonical `ToolExecutionProgress` Event
///   Journal facts at batch settlement; and
/// - the attempt's live observation seam, when installed, as one
///   disposable, latest-value, read-only observation emitted while the
///   tool still executes. This half never enters the Event Journal, never
///   blocks the loop, and is never execution evidence.
///
/// The Tool Plane learns nothing about observers or subagents: the fanout
/// is a plain [`ProgressReporter`], so no executor or
/// `ToolExecutionContext` signature changes.
struct ForegroundProgressFanout<'a> {
    /// The durable buffer of the invocation.
    buffer: &'a ForegroundProgressBuffer,
    /// The attempt's live observation seam, when installed.
    observer: Option<&'a dyn AgentExecutionObserver>,
    /// The owning attempt.
    attempt_id: &'a AttemptId,
}

impl ProgressReporter for ForegroundProgressFanout<'_> {
    fn report(&self, progress: ToolProgress) {
        // Normalized once: the buffer's own normalization is idempotent, so
        // both consumers observe the same bounded value and the durable
        // path is byte-identical to the un-fanned-out one.
        let bounded = crate::tools::limits::bound_tool_progress(progress);
        // The shared native driver already stamped genuine liveness before
        // this caller-specific observation can delay it.
        self.buffer.report(bounded.clone());
        if let Some(observer) = self.observer {
            observer.observe_tool_progress(
                self.attempt_id,
                &self.buffer.call_id,
                &self.buffer.tool_id,
                &bounded,
            );
        }
    }
}

impl AgentExecution<'_> {
    /// Preflights every model-issued tool call of the turn.
    ///
    /// An impossible canonical identity mismatch or unregistered tool is a
    /// runtime/model-stream contract failure; business schema violations are
    /// normal [`PreflightOutcome::Rejected`] result slots.
    pub(super) fn preflight_tool_calls(
        &self,
        calls: &[ToolCall],
    ) -> Result<Vec<PreflightOutcome>, RuntimeError> {
        let mut outcomes = Vec::with_capacity(calls.len());
        for call in calls {
            match self.tool_registry().preflight(call) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    return Err(match error {
                        crate::tools::executor::ToolPreflightError::UnknownTool { name } => {
                            RuntimeError::UnknownTool { name }
                        }
                        crate::tools::executor::ToolPreflightError::IdentityMismatch {
                            id,
                            name,
                        } => RuntimeError::ContractViolation {
                            message: format!(
                                "tool call identity mismatch: id {id} and name {name:?}                                      do not resolve to the same registered tool"
                            ),
                        },
                    });
                }
            }
        }
        Ok(outcomes)
    }

    /// The immutable `ToolRegistry` handle of the pinned capability snapshot.
    pub(super) fn tool_registry(&self) -> &ToolRegistry {
        &self.admitted_tools
    }

    /// Executes the turn's tool calls through deterministic scheduling
    /// phases and commits the structurally complete result batch.
    ///
    /// Scheduling interprets [`ToolConcurrencyPolicy`] per registered tool:
    /// a `Sequential` invocation is an exclusive scheduling barrier, and
    /// adjacent `Parallel` invocations execute concurrently as one group.
    /// A background call is settled for the originating attempt when its
    /// background dispatch is accepted, not when the detached work
    /// terminates, so a sequential background call blocks later scheduling
    /// only through its dispatch-acceptance point.
    ///
    /// The structural invariant: once the valid Assistant tool-call message is
    /// committed, its entire tool-result batch is settled exactly once.
    /// Every call slot receives exactly one attempt-facing result
    /// (success/failure/cancellation/timeout/validation rejection/accepted
    /// background), canonical `ToolMessageBlock`s are committed in original
    /// model call order, and the batch never splits on cancellation. If
    /// attempt cancellation wins during the batch: in-flight cancellable
    /// foreground executions observe the attempt signal and physically
    /// settle, unstarted calls receive cancelled result slots, committed
    /// background executions stay conversation-owned, prepared-but-
    /// uncommitted dispatches roll back, and the complete batch commits in
    /// call order.
    ///
    /// The returned [`SettledCall`] values are immutable copies of exactly
    /// the committed facts, in canonical call order. They are the only input
    /// of the tool-result observation pass, which runs strictly after this
    /// function returns and therefore cannot influence structural settlement.
    /// Executes one batch and settles the conversation-owned task list with
    /// it.
    ///
    /// A `todo` call mutates only *staged* list state
    /// ([`crate::tools::todo::ConversationTodoList`]), owned by the
    /// [`TodoBatch`] opened here and reachable only through the writer this
    /// batch hands its own invocations. So the in-memory list is settled by
    /// exactly the same event that makes the batch's snapshots durable: the
    /// atomic `ToolResult` batch commit. What it installs is what *this*
    /// batch's own canonical results published, never whatever happened to be
    /// staged, and every other way out — a durable failure while announcing a
    /// call, a rejected canonical append, a panic — drops the token and
    /// discards the batch's provisional list. The authority a later rebuild
    /// reads back from canonical history and the authority this process holds
    /// therefore cannot disagree.
    ///
    /// The list refuses to hand out a second concurrent batch. A batch that
    /// cannot open one runs without a writer: its `todo` calls settle as
    /// ordinary rejected results and the list stays exactly where canonical
    /// history left it. That is the correct failure — the alternative is a
    /// batch mutating a list it cannot publish.
    pub(super) async fn execute_tools(
        &mut self,
        calls: &[ToolCall],
        occurrences: Vec<crate::message::types::ToolCallOccurrenceRef>,
        preflight: Vec<PreflightOutcome>,
    ) -> Result<Vec<SettledCall>, CanonicalCommitError> {
        // `None` is either "this composition has no Todo extension" or "the
        // list already has an open batch". Both mean the same thing to this
        // batch: it runs without a Todo writer, so a `todo` call (which a
        // Todo-free composition cannot even make — the Tool is not
        // registered) settles as an ordinary rejected result and no list is
        // touched.
        let batch = self
            .tool_runtime
            .todos()
            .filter(|_| {
                self.capability
                    .snapshot()
                    .resolved_profile()
                    .is_some_and(|profile| profile.extensions.todo().is_some())
            })
            .and_then(crate::tools::todo::ConversationTodoList::open_batch);
        self.execute_tools_staged(calls, occurrences, preflight, batch)
            .await
    }

    /// The batch itself: schedule, settle, and commit every call.
    #[allow(clippy::too_many_lines)] // one coherent scheduling/commit pipeline
    async fn execute_tools_staged(
        &mut self,
        calls: &[ToolCall],
        occurrences: Vec<crate::message::types::ToolCallOccurrenceRef>,
        preflight: Vec<PreflightOutcome>,
        todos: Option<crate::tools::todo::TodoBatch>,
    ) -> Result<Vec<SettledCall>, CanonicalCommitError> {
        let todo_writer = todos.as_ref().map(crate::tools::todo::TodoBatch::writer);
        let mut slots: Vec<CallSlot> = calls
            .iter()
            .cloned()
            .zip(preflight)
            .map(|(call, outcome)| match outcome {
                PreflightOutcome::Ready(prepared) => CallSlot {
                    call,
                    tool_id: prepared.invocation.tool_id.clone(),
                    origin: prepared.origin.clone(),
                    prepared: Some(prepared),
                    result: None,
                    executor_started: false,
                    execution_facts: Vec::new(),
                },
                PreflightOutcome::Rejected {
                    tool_id,
                    origin,
                    error,
                } => CallSlot {
                    call,
                    tool_id,
                    origin,
                    prepared: None,
                    result: Some(failed_result(&error)),
                    executor_started: false,
                    execution_facts: Vec::new(),
                },
            })
            .collect();
        // Resolve every ready call through the one pre-tool policy boundary
        // before scheduling any executor. This is a deliberately strong
        // parallel-batch contract: all policy/interaction decisions for the
        // committed batch settle in canonical call order before the existing
        // sequential/parallel start frontier advances. The original
        // PreparedInvocation remains in every slot unchanged.
        self.resolve_pre_tool_decisions(&mut slots)
            .await
            .map_err(CanonicalCommitError::Interaction)?;
        if self.cancellation.interaction_failed() {
            return Err(CanonicalCommitError::Interaction(
                InteractionFailure::ControlLost,
            ));
        }
        let mut index = 0;
        while index < slots.len() {
            if self.cancellation.is_cancelled() {
                break;
            }
            match group_at(&slots, index) {
                Group::Trivial => {
                    index += 1;
                }
                Group::Sequential => {
                    if slots[index].result.is_none() {
                        // This assignment is the per-call executor-start
                        // frontier. It is set before the existing start fact
                        // and before constructing the executor future; once
                        // crossed, cancellation must be classified as
                        // `DuringExecution` for this slot.
                        slots[index].executor_started = true;
                        self.emit(RuntimeEvent::ToolExecutionStarted {
                            tool_call_id: slots[index].call.id.clone(),
                            tool_id: slots[index].tool_id.clone(),
                        });
                        #[cfg(test)]
                        self.park_after_tool_start();
                        if let Some(error) = self.durable_failure_commit_error() {
                            return Err(error);
                        }
                        let prepared = slots[index]
                            .prepared
                            .as_ref()
                            .expect("unsettled slots are preflighted");
                        let invocation = prepared.invocation.clone();
                        let progress_capability = prepared.progress_capability;
                        let (_, result, facts) = self
                            .run_single_call(
                                index,
                                invocation,
                                progress_capability,
                                todo_writer.clone(),
                                occurrences[index].clone(),
                            )
                            .await;
                        slots[index].result = Some(result);
                        slots[index].execution_facts = facts;
                        if self.cancellation.interaction_failed() {
                            return Err(CanonicalCommitError::Interaction(
                                InteractionFailure::ControlLost,
                            ));
                        }
                    }
                    index += 1;
                }
                Group::Parallel => {
                    // Execution-start facts are emitted before any future is
                    // created, so the loop owns `&mut self` emission and the
                    // shared `&self` borrows of the spawned futures never
                    // conflict.
                    let end = parallel_group_end(&slots, index);
                    for slot in &mut slots[index..end] {
                        // Cancellation can be requested by a synchronous
                        // start-event observer while this group is being
                        // announced. Stop the logical start frontier at the
                        // first observed cancellation; the remaining slots
                        // are filled by the canonical cancellation pass and
                        // never acquire an execution future.
                        if self.cancellation.is_cancelled() {
                            break;
                        }
                        if slot.result.is_none() {
                            // The slot crosses its authoritative start
                            // frontier before the existing projection event.
                            // A synchronous observer may cancel here; this
                            // slot is still an in-flight cancellation, while
                            // later siblings remain eligible for BeforeStart.
                            slot.executor_started = true;
                            self.emit(RuntimeEvent::ToolExecutionStarted {
                                tool_call_id: slot.call.id.clone(),
                                tool_id: slot.tool_id.clone(),
                            });
                            #[cfg(test)]
                            self.park_after_tool_start();
                        }
                    }
                    if let Some(error) = self.durable_failure_commit_error() {
                        return Err(error);
                    }
                    let mut futures = futures_util::stream::FuturesUnordered::new();
                    for (slot_index, slot) in slots[index..end].iter().enumerate() {
                        if slot.executor_started {
                            let prepared = slot
                                .prepared
                                .as_ref()
                                .expect("unsettled slots are preflighted");
                            futures.push(Box::pin(self.run_single_call(
                                index + slot_index,
                                prepared.invocation.clone(),
                                prepared.progress_capability,
                                todo_writer.clone(),
                                occurrences[index + slot_index].clone(),
                            )));
                        }
                    }
                    let mut remaining = futures.len();
                    if remaining > 0 {
                        loop {
                            if self.cancellation.is_cancelled() {
                                break;
                            }
                            tokio::select! {
                                biased;
                                () = self.cancellation.cancelled() => break,
                                Some((slot_index, result, facts)) = futures.next() => {
                                    slots[slot_index].result = Some(result);
                                    slots[slot_index].execution_facts = facts;
                                    remaining -= 1;
                                    if remaining == 0 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // After cancellation wins, every in-flight foreground
                    // execution still settles: executors observe the attempt
                    // signal in their context and must settle their external
                    // work, and the lifecycle's settlement-authority wait —
                    // bounded against a contract-violating executor by the
                    // settlement control-plane guard (Issue #204) — commits
                    // `OutcomeUnknown` for an executor whose settlement never
                    // confirms, so this drain is finite. The futures are
                    // awaited to completion, never dropped with external
                    // work abandoned.
                    while let Some((slot_index, result, facts)) = futures.next().await {
                        slots[slot_index].result = Some(result);
                        slots[slot_index].execution_facts = facts;
                    }
                    if self.cancellation.interaction_failed() {
                        return Err(CanonicalCommitError::Interaction(
                            InteractionFailure::ControlLost,
                        ));
                    }
                    index = end;
                }
            }
        }
        if self.cancellation.interaction_failed() {
            return Err(CanonicalCommitError::Interaction(
                InteractionFailure::ControlLost,
            ));
        }
        // Cancellation fill: every not-yet-started foreground call receives
        // a cancelled result slot, so the committed batch covers every
        // logical call exactly once.
        if self.cancellation.is_cancelled() {
            for slot in &mut slots {
                if slot.result.is_none() {
                    slot.result = Some(cancelled_result(self.cancellation.reason()));
                }
            }
        }
        // Canonical batch commit in original model call order. Each call's
        // retained execution facts (progress observations, then the deadline
        // intent and cancellation/settlement control-plane facts when a
        // non-physical winner drove a settlement phase) precede their
        // completion event; the completion events themselves are committed
        // in canonical order regardless of physical completion order.
        //
        // The **structural settlement point of the whole batch** is the one
        // atomic `commit_tool_result_batch` call: either every logical call
        // of the committed Assistant tool-call message owns exactly one
        // canonical `ToolMessage` (in original model call order) or none of
        // them becomes canonical. A durable failure of one member can never
        // leave a partial batch behind. The settled facts collected here are
        // immutable copies of exactly what was committed, and they are the
        // only input of the observation pass.
        let mut blocks = Vec::with_capacity(slots.len());
        let mut result_slots = Vec::with_capacity(slots.len());
        for (batch_position, slot) in slots.iter().enumerate() {
            let result = slot.result.clone().expect("every call slot settles");
            for event in &slot.execution_facts {
                self.emit(event.clone());
                if let Some(error) = self.durable_failure_commit_error() {
                    return Err(error);
                }
            }
            if slot.executor_started {
                self.emit(RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: slot.call.id.clone(),
                    tool_id: slot.tool_id.clone(),
                    result: result.clone(),
                });
                if let Some(error) = self.durable_failure_commit_error() {
                    return Err(error);
                }
            }
            let block = MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new(format!(
                    "{}-tool-{}-{}",
                    self.request.attempt_id,
                    self.turn,
                    occurrences[batch_position].block_index.get()
                )),
                occurrence: occurrences[batch_position].clone(),
                tool_call_id: slot.call.id.clone(),
                tool_id: slot.tool_id.clone(),
                result: result.clone(),
            });
            blocks.push(block);
            result_slots.push((batch_position, slot, result));
        }
        let batch_anchor = self.commit_tool_result_batch(&blocks)?;
        // The batch is durable, so the snapshots its `todo` results carry
        // are durable: this is the one point where the conversation's list
        // may move, and it moves to exactly what these committed blocks
        // published — never to a stage this batch did not write.
        //
        // Settling is the whole decision: the batch installs what its own
        // committed results published, or moves nothing when they published
        // none. There is no third outcome to branch on and no failure to
        // report — a batch that reaches here still holds the list it opened,
        // which is what makes `open_batch`'s exclusivity worth having.
        if let Some(todos) = todos {
            todos.settle(&blocks);
        }
        // The complete canonical ToolResult batch, not an individual tool
        // completion, establishes one attempt-local PostToolBatch
        // opportunity for the next primary step that already exists.
        //
        // The batch's own durable position is frozen with the marker, here,
        // at the commit that allocated it. That is the linearization point at
        // which "after this tool batch" becomes a determined fact; a status
        // composed from this opportunity keeps it no matter what else the
        // conversation durably accepts before the composition is observed.
        self.pending_post_tool_batch = Some(PendingPostToolBatch {
            opportunity: PostToolBatchOpportunity,
            transcript_anchor: batch_anchor,
        });
        // FND-06 can kill a real process in the narrow window after the
        // canonical batch commit and before the next primary-step
        // preparation. The marker is deliberately process-local; this hook
        // proves recovery does not synthesize it from durable tool history.
        crate::runtime::process_death::reach("after:post_tool_batch_marker");
        let settled = result_slots
            .into_iter()
            .map(|(batch_position, slot, result)| SettledCall {
                batch_position,
                call_id: slot.call.id.clone(),
                tool_id: slot.tool_id.clone(),
                origin: slot.origin.clone(),
                // The invocation facts are copied from the one
                // `PreparedInvocation` that executed, and the canonical
                // identity/origin are the slot's, so no second stored identity
                // can disagree with the registry. A preflight-rejected call
                // never resolved an invocation and exposes none.
                invocation: slot
                    .prepared
                    .as_ref()
                    .map(|prepared| ObservedToolInvocation {
                        tool_id: slot.tool_id.clone(),
                        origin: slot.origin.clone(),
                        mode: prepared.invocation.mode,
                        arguments: prepared.invocation.arguments.clone(),
                    }),
                result,
            })
            .collect();
        self.emit(RuntimeEvent::TurnCompleted);
        if let Some(error) = self.durable_failure_commit_error() {
            return Err(error);
        }
        Ok(settled)
    }

    /// Resolves the typed pre-tool decision of every preflight-ready call.
    ///
    /// The policy sees only an immutable view of the exact registry-resolved
    /// facts. `Ask` delegates the wait to the attempt's concrete native
    /// interaction binding, while the Agent Loop remains the owner of
    /// execution and cancellation. A provider-unavailable interaction and a
    /// policy error fail closed as `Denied` only when no post-await
    /// cancellation is observable; an owner cancellation produces the
    /// existing cancelled result slot and closes the later tool-start
    /// frontier.
    async fn resolve_pre_tool_decisions(
        &self,
        slots: &mut [CallSlot],
    ) -> Result<(), InteractionFailure> {
        for slot in slots {
            if slot.result.is_some() {
                continue;
            }
            if self.cancellation.is_cancelled() {
                slot.result = Some(cancelled_result(self.cancellation.reason()));
                continue;
            }
            let prepared = slot
                .prepared
                .as_ref()
                .expect("a policy decision requires a preflight-ready invocation");
            let invocation = &prepared.invocation;
            let view = PreToolView {
                conversation_id: &self.request.conversation_id,
                execution: crate::runtime::interaction::InteractionExecution::Attempt {
                    attempt_id: self.request.attempt_id.clone(),
                    turn: self.turn,
                },
                invocation_id: &invocation.id,
                tool_id: &invocation.tool_id,
                tool_name: &invocation.tool_name,
                origin: &prepared.origin,
                mode: invocation.mode,
                arguments: &invocation.arguments,
                audit_arguments: &slot.call.arguments,
                approval_policy: prepared.approval,
            };
            slot.result = crate::tools::invocation::authorize(
                &self.lifecycle,
                &view,
                &self.cancellation.execution_cancellation(),
            )
            .await?;
        }
        Ok(())
    }

    /// Runs the immutable tool-result observation pass of one structurally
    /// settled tool batch and stages its bounded deferred context proposals.
    ///
    /// Invocation order is the canonical `ToolCall` batch order (never
    /// physical completion order) and, within one call, the registered
    /// observers' logical identity order. The staged proposals therefore keep
    /// the canonical deferred order
    /// `(ToolCall batch position, producer identity, proposal FIFO)`, with no
    /// registration-order term.
    ///
    /// # The transaction boundary
    ///
    /// Every observer return value is validated **before** anything is staged:
    /// its proposal count against the established
    /// [`MAX_PROPOSALS_PER_CONTRIBUTOR`] bound, the running total against
    /// [`MAX_DEFERRED_CONTEXT_PROPOSALS`], and every proposal body against the
    /// same bounded content contract Context Assembly applies. An oversized
    /// observation can therefore never be appended to the attempt buffer and
    /// discovered one step later.
    ///
    /// The pass is transactional: any failure — a failing observer, a bound
    /// violation, invalid content, or observable cancellation — discards every
    /// proposal of this pass *and* clears the attempt's deferred buffer, so no
    /// partial deferred state survives. The already-committed Assistant
    /// tool-call message and its complete canonical `ToolMessage` batch are
    /// untouched either way; they were committed before this function is
    /// called.
    ///
    /// # Cancellation precedence
    ///
    /// The observer receives no cancellation handle; the loop keeps
    /// cancellation ownership. A bounded observation that is already running
    /// settles rather than being dropped, but observable cancellation is
    /// checked before each observer starts and again once it settles, before
    /// its return value is consumed. Once cancellation is observable, no later
    /// observer starts and neither an observer's success nor its failure can
    /// decide the terminal outcome.
    pub(super) async fn run_tool_result_observations(
        &mut self,
        settled: &[SettledCall],
    ) -> Result<(), Terminal> {
        let observers = self.lifecycle.tool_result_observers().to_vec();
        if observers.is_empty() {
            return Ok(());
        }
        match self.observe_settled_batch(&observers, settled).await {
            Ok(staged) => {
                self.deferred_context.extend(staged);
                Ok(())
            }
            Err(terminal) => {
                // Nothing of this pass was staged, and no earlier pass may
                // survive a failed or cancelled one: the attempt settles
                // terminally, and it settles with an empty deferred buffer.
                self.deferred_context.clear();
                Err(terminal)
            }
        }
    }

    /// The pass body: one observation per (settled call, bound observer),
    /// validated at its transaction boundary and accumulated into a
    /// pass-local buffer that is never visible to the attempt on failure.
    ///
    /// Cancellation is checked at two points around every observation, and
    /// both belong to the Agent Loop alone:
    ///
    /// ```text
    /// cancellation check          ← an observer never starts after this
    /// await observer
    /// cancellation check          ← wins over the observer's Ok *or* Err
    /// consume result, validate, stage
    /// ```
    ///
    /// An in-flight bounded observation is allowed to settle rather than being
    /// dropped, but once cancellation is observable it decides the terminal
    /// outcome: a later observer never starts, and an observer's failure can
    /// no longer become `ToolResultObservationFailed`.
    async fn observe_settled_batch(
        &self,
        observers: &[crate::agent::lifecycle::RegisteredToolResultObserver],
        settled: &[SettledCall],
    ) -> Result<Vec<DeferredContextProposal>, Terminal> {
        let already_staged = self.deferred_context.len();
        let mut staged: Vec<DeferredContextProposal> = Vec::new();
        for call in settled {
            for registered in observers {
                if self.cancellation.is_cancelled() {
                    return Err(self.cancelled_terminal());
                }
                let observation = ToolResultObservation {
                    attempt_id: &self.request.attempt_id,
                    conversation_id: &self.request.conversation_id,
                    turn: self.turn,
                    batch_position: call.batch_position,
                    call_id: &call.call_id,
                    tool_id: &call.tool_id,
                    origin: &call.origin,
                    invocation: call.invocation.as_ref(),
                    result: &call.result,
                };
                let observed = registered
                    .observer()
                    .observe_tool_result(&observation)
                    .await;
                // Cancellation that became observable while this bounded
                // observation was in flight wins here, before the return value
                // is consumed. This is why an observer that fails during an
                // already-cancelled attempt cannot convert cancellation into
                // `ToolResultObservationFailed`.
                if self.cancellation.is_cancelled() {
                    return Err(self.cancelled_terminal());
                }
                let proposals = observed.map_err(|error| Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ToolResultObservationFailed {
                            message: error.message,
                        },
                    },
                })?;
                // The transaction boundary. Nothing below has touched the
                // attempt's deferred buffer yet, and nothing will until the
                // whole pass validates.
                if proposals.len() > MAX_PROPOSALS_PER_CONTRIBUTOR {
                    return Err(Self::deferred_rejected(format!(
                        "deferred-context producer {:?} returned {} proposals for call {}, \
                         above the bounded proposal limit {MAX_PROPOSALS_PER_CONTRIBUTOR}",
                        registered.producer(),
                        proposals.len(),
                        call.call_id
                    )));
                }
                if already_staged + staged.len() + proposals.len() > MAX_DEFERRED_CONTEXT_PROPOSALS
                {
                    return Err(Self::deferred_rejected(format!(
                        "the observation pass of turn {} would stage more than \
                         {MAX_DEFERRED_CONTEXT_PROPOSALS} deferred proposals",
                        self.turn
                    )));
                }
                for proposal in proposals {
                    validate_user_message_proposal(proposal.message()).map_err(|error| {
                        Self::deferred_rejected(format!(
                            "deferred-context producer {:?} proposed invalid context for \
                             call {}: {error}",
                            registered.producer(),
                            call.call_id
                        ))
                    })?;
                    staged.push(DeferredContextProposal {
                        // The producer comes from the binding, never from the
                        // observer's return value — and it is still only a
                        // reference until Context Assembly resolves it.
                        producer: registered.producer().clone(),
                        proposal,
                    });
                }
            }
        }
        Ok(staged)
    }

    /// Runs one logical tool call of the batch.
    ///
    /// Foreground invocations race against attempt cancellation (biased):
    /// when cancellation is already observable, no new execution progress
    /// begins, and an in-flight execution settles by observing the attempt
    /// signal in its context. Background invocations are dispatched through
    /// the conversation background registry's ownership commit. The slot
    /// index is returned so physical completion order can be recorded while
    /// canonical results remain model-call ordered. The progress capability
    /// is the executor's declaration frozen into the admitted invocation at
    /// preflight (Issue #204); background dispatch has no idle watchdog and
    /// ignores it.
    async fn run_single_call(
        &self,
        call_index: usize,
        invocation: ToolInvocation,
        progress_capability: ToolProgressCapability,
        todos: Option<crate::tools::todo::TodoWriter>,
        occurrence: crate::message::types::ToolCallOccurrenceRef,
    ) -> (usize, ToolExecutionResult, Vec<RuntimeEvent>) {
        let (result, facts) = match invocation.mode {
            ToolInvocationMode::Foreground => {
                self.run_foreground(&invocation, progress_capability, todos, &occurrence)
                    .await
            }
            // A detached execution outlives this batch, so it is handed no
            // task-list authority: nothing it does can land in the snapshot
            // this batch is about to commit.
            ToolInvocationMode::Background => {
                (self.dispatch_background(&invocation).await, Vec::new())
            }
        };
        (call_index, result, facts)
    }

    #[cfg(test)]
    fn park_after_tool_start(&self) {
        if let Some(pause) = self
            .pauses
            .tool_start_pause
            .lock()
            .expect("tool start pause lock")
            .as_ref()
        {
            pause.park();
        }
    }

    #[cfg(test)]
    fn park_after_tool_cancellation_settlement(&self) {
        if let Some(pause) = self
            .pauses
            .tool_cancellation_settlement_pause
            .lock()
            .expect("tool cancellation settlement pause lock")
            .as_ref()
        {
            pause.park();
        }
    }

    #[cfg(test)]
    fn park_after_tool_physical_settlement(&self) {
        if let Some(pause) = self
            .pauses
            .tool_physical_settlement_pause
            .lock()
            .expect("tool physical settlement pause lock")
            .as_ref()
        {
            pause.park();
        }
    }

    /// Caller resources and canonical observation surround the native driver.
    #[allow(clippy::too_many_lines)] // Caller context and canonical fact projection stay together.
    async fn run_foreground(
        &self,
        invocation: &ToolInvocation,
        progress_capability: ToolProgressCapability,
        todos: Option<crate::tools::todo::TodoWriter>,
        occurrence: &crate::message::types::ToolCallOccurrenceRef,
    ) -> (ToolExecutionResult, Vec<RuntimeEvent>) {
        use crate::tools::invocation::InvocationFact;
        let call_id = invocation
            .id
            .canonical_call_id()
            .expect("Agent-owned invocation");
        let executor = self.tool_registry().executor(&invocation.tool_id);
        let buffer = ForegroundProgressBuffer::new(call_id.clone(), invocation.tool_id.clone());
        let progress = ForegroundProgressFanout {
            buffer: &buffer,
            observer: self.observer,
            attempt_id: &self.request.attempt_id,
        };
        let execution_cancellation = self.cancellation.execution_cancellation();
        let context = ToolExecutionContext::new(
            &self.request.conversation_id,
            None,
            execution_cancellation.clone(),
            self.tool_runtime.workspace(),
            &progress,
            self.tool_runtime.artifacts(),
            self.tool_runtime.tool_output(),
            self.capability.snapshot().effective_environment(),
        );
        let context = match self.lifecycle.native_questionnaire_requester(
            self.request.attempt_id.clone(),
            execution_cancellation,
            self.turn,
        ) {
            Some(requester) => context.with_questionnaire_requester(requester),
            None => context,
        };
        let mut context = context;
        context.schedule = Some(Box::new(crate::scheduler::control::ScheduleToolContext {
            control: self.tool_runtime.schedule().clone(),
            grant: &self.schedule_authorization,
            command_id: {
                use sha2::{Digest, Sha256};
                format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&(&self.request.conversation_id, occurrence))
                            .expect("native canonical Tool occurrence")
                    )
                )
            },
        }));
        context.model_invocation = Some(self.request.model.primary());
        if let Some(goal) = self.tool_runtime.goal() {
            context.goal = Some(Box::new(crate::goal::GoalToolContext {
                domain: goal.clone(),
                creation_authorization: &self.goal_creation_authorization,
                mailbox: self.tool_runtime.mailbox(),
            }));
        }
        // The task-list authority of *this* batch, named rather than
        // ambient: an invocation that is not part of a batch never receives
        // one, and therefore never writes provisional list state.
        let context = match todos {
            Some(todos) => context.with_todos(todos),
            None => context,
        };
        // The attempt's *own* immutable generation, named rather than
        // ambient. A registered executor never reaches runtime-current
        // resources, so a reload that commits a newer generation mid-attempt
        // cannot be observed by this invocation.
        let context = match &self.subagent_context {
            Some(subagent) => {
                let mut subagent = subagent.clone();
                subagent.native = Some(Arc::new(
                    crate::tools::invocation::NativeInvocationServices {
                        lifecycle: self.lifecycle.clone(),
                        runtime: self.tool_runtime.clone(),
                        clock: self.monotonic_clock.clone(),
                        leaf_policy: self.tool_deadline_policy,
                        execution: crate::runtime::interaction::InteractionExecution::Attempt {
                            attempt_id: self.request.attempt_id.clone(),
                            turn: self.turn,
                        },
                        scheduling: Arc::new(tokio::sync::RwLock::new(())),
                        descendants: crate::tools::invocation::NativeChildScope::default(),
                    },
                ));
                context.with_subagent_context(subagent)
            }
            None => context,
        };

        #[cfg(test)]
        let armed = |deadline| {
            if let Some(signal) = self
                .pauses
                .tool_deadline_armed
                .lock()
                .expect("deadline signal")
                .as_ref()
            {
                signal.armed(deadline);
            }
        };
        #[cfg(test)]
        let cancellation_won = || self.park_after_tool_cancellation_settlement();
        #[cfg(test)]
        let completion_won = || self.park_after_tool_physical_settlement();
        let driver = crate::tools::invocation::ForegroundInvocation {
            clock: &*self.monotonic_clock,
            policy: self.tool_deadline_policy,
            registration: self.tool_registry().foreground_policy(&invocation.tool_id),
            #[cfg(test)]
            deadline_armed: Some(&armed),
            #[cfg(test)]
            cancellation_won: Some(&cancellation_won),
            #[cfg(test)]
            completion_won: Some(&completion_won),
        };
        let (result, facts) = driver
            .execute(&*executor, invocation.clone(), progress_capability, context)
            .await;
        let mut events = buffer.take();
        events.extend(facts.into_iter().map(|fact| match fact {
            InvocationFact::Deadline { kind } => RuntimeEvent::ToolExecutionDeadlineFired {
                tool_call_id: call_id.clone(),
                tool_id: invocation.tool_id.clone(),
                kind,
            },
            InvocationFact::CancellationRequested { cause } => {
                RuntimeEvent::ToolExecutionCancellationRequested {
                    tool_call_id: call_id.clone(),
                    tool_id: invocation.tool_id.clone(),
                    cause,
                }
            }
            InvocationFact::SettlementObserved { certainty } => {
                RuntimeEvent::ToolExecutionSettlementObserved {
                    tool_call_id: call_id.clone(),
                    tool_id: invocation.tool_id.clone(),
                    certainty,
                }
            }
            InvocationFact::SettlementControlFailed { reason } => {
                RuntimeEvent::ToolExecutionSettlementControlFailed {
                    tool_call_id: call_id.clone(),
                    tool_id: invocation.tool_id.clone(),
                    reason,
                }
            }
        }));
        (result, events)
    }

    /// Dispatches one background invocation through the conversation-owned
    /// background registry.
    ///
    /// The dispatch is two-stage: prepare allocates the deterministic
    /// execution id and parks the runner behind its commit gate; commit
    /// performs the final attempt-cancellation checkpoint and produces the
    /// accepted result exactly when conversation ownership commits. A
    /// rolled-back dispatch produces a cancelled result slot for the
    /// originating attempt, never a detached execution.
    async fn dispatch_background(&self, invocation: &ToolInvocation) -> ToolExecutionResult {
        let executor = self.tool_registry().executor(&invocation.tool_id);
        // The attempt's effective ToolEnvironment is captured at prepare
        // time, before the background ownership commit: the detached
        // execution retains exactly this immutable environment for its
        // whole lifetime, even after this attempt terminates and later
        // revisions activate.
        let environment = self.capability.snapshot().effective_environment().clone();
        let Some(mcp_leases) = self.capability.mcp_leases() else {
            return failed_result("the admitted MCP generation is already physically retired");
        };
        match self
            .tool_runtime
            .background()
            .prepare_dispatch_with_mcp_leases(invocation, &executor, environment, mcp_leases)
        {
            Ok(prepared) => {
                match self
                    .tool_runtime
                    .background()
                    .commit_dispatch_owned(prepared, &self.cancellation.signal())
                    .await
                {
                    Ok(BackgroundDispatchOutcome::Accepted { result, .. }) => result,
                    // A rolled-back dispatch (attempt cancellation won at
                    // the ownership boundary) or a refused commit (the
                    // owning conversation runtime is not activated) both
                    // leave no detached execution; the originating attempt
                    // observes a cancelled result slot.
                    Ok(BackgroundDispatchOutcome::RolledBack)
                    | Err(
                        crate::tools::background::BackgroundDispatchError::ConversationInactive {
                            ..
                        },
                    ) => cancelled_result(self.cancellation.reason()),
                    Err(error) => failed_result(&error.to_string()),
                }
            }
            Err(error) => failed_result(&error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests;
