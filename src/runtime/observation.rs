//! The runtime-owned semantic observation contract (Issue #61).
//!
//! This module owns the vocabulary and the handoff state through which the
//! conversation runtime publishes every semantically meaningful transition
//! of one conversation:
//!
//! ```text
//! ConversationRuntime semantic facts
//!         |
//!         v
//! ConversationObservation (this module, runtime-owned shapes only)
//!         |
//!         v
//! Runtime Client adapter translates
//!         |
//!         v
//! RuntimeClientProjection (snapshot / cursor / replay / subscribers)
//! ```
//!
//! The runtime never emits Runtime Client projection types: every variant
//! carries runtime-owned source types (`RuntimeEvent`, `InboundItem`,
//! `BackgroundExecutionSnapshot`, the authoritative `CapabilitySnapshot`,
//! the frozen `AttemptModelView`, …). The Runtime Client projection owns
//! the translation into its snapshot/event vocabulary.
//!
//! # The pending observation queue
//!
//! [`PendingObservations`] is the tiny leaf synchronization boundary between
//! the conversation runtime and its observation consumers. The mailbox, the
//! background registry, the capability coordinator, and `AgentExecution`
//! all fire their observers while their own lock is held; the coordinator
//! publishes admission/commit facts under its own lock. None of those
//! producers may take the Runtime Client projection lock, so each appends
//! an immutable observation here and wakes the projection worker. Every
//! projection lock acquisition drains this queue first, so queued
//! observations fold in enqueue order.
//!
//! It is also the projection worker's rendezvous point. The worker holds
//! `Arc<PendingObservations>` — never an owning runtime/client handle
//! across an await — so this queue, not the runtime, is what keeps the
//! worker's wait alive. The queue is closed (idempotently) when either the
//! conversation runtime or the Runtime Client adapter is destroyed; closing
//! is the worker's terminal condition.
//!
//! # The runtime semantic record
//!
//! [`RuntimeSemanticRecord`] is the runtime-owned derived read state of the
//! facts that are *not* readable from a static owner while an attempt is
//! running: the committed canonical messages, the current attempt's
//! execution semantics, the latest composed Agent Status, and the committed
//! compaction statistics. Between attempts the coordinator owns the one
//! mutable `ConversationState` and can serve those facts from it directly;
//! during an attempt that state is structurally moved into `AgentExecution`,
//! so the record is folded from the same observation stream a client would
//! fold, under its own small leaf lock.
//!
//! The record is **never** a second mutable `ConversationState` authority:
//! it is append-only derived read state, cleared at attempt settlement,
//! and exists only so a Runtime Client adapter constructed while an
//! attempt is active receives a coherent initial read model. It contains no
//! Runtime Client types.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent::observer::AgentStatusObservation;
use crate::capabilities::CapabilitySnapshot;
use crate::conversation::SurfaceRevision;
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::model::types::ModelUsage;
use crate::runtime::identity::{AttemptId, MessageId, ToolCallId, ToolId};
use crate::runtime::inbound::{InboundBatch, InboundItem};
use crate::runtime::types::TokenMeasurement;
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolProgress};

/// One runtime-owned semantic observation.
///
/// The observation union carries every external state change the Runtime
/// Client projection folds. It is the single entry point of the projection:
/// no call site folds state directly. Every variant carries runtime-owned
/// source types only — the Runtime Client layer owns the translation into
/// its snapshot/event vocabulary.
#[derive(Debug, Clone)]
pub(crate) enum ConversationObservation {
    /// One canonical internal runtime fact of an attempt.
    Event {
        /// The emitting attempt.
        attempt_id: AttemptId,
        /// The canonical fact.
        event: RuntimeEvent,
    },
    /// One canonical message commit (the loop's commit observation seam;
    /// the internal committed-message events reference identity only).
    Committed {
        /// The committing attempt, when one is active.
        attempt_id: Option<AttemptId>,
        /// The committed canonical message.
        block: MessageBlock,
    },
    /// One composed Agent Status observation.
    Status(AgentStatusObservation),
    /// One mailbox enqueue (authoritative item + sequence).
    InboundEnqueued(InboundItem),
    /// One mailbox finite drain (authoritative batch).
    InboundDrained(InboundBatch),
    /// One background registry transition snapshot.
    Background(BackgroundExecutionSnapshot),
    /// One activated authoritative capability snapshot.
    Capability(Arc<CapabilitySnapshot>),
    /// The coordinator admitted an attempt (before the loop started).
    AttemptAdmitted {
        /// The admitted attempt.
        attempt_id: AttemptId,
    },
    /// The admitted attempt froze its immutable model snapshot.
    ///
    /// Published under the same lock acquisition as `AttemptAdmitted`, so
    /// the attempt read model always carries the model it actually runs
    /// with.
    AttemptModelFrozen {
        /// The admitted attempt.
        attempt_id: AttemptId,
        /// The frozen model view.
        model: Box<AttemptModelView>,
    },
    /// The authoritative session model configuration changed.
    SessionModelChanged {
        /// The redacted session model state after the update.
        model: Box<SessionModelView>,
    },
    /// The runtime accepted shutdown.
    Shutdown,
}

/// The tiny synchronization boundary between the conversation runtime and
/// its observation consumers (the Runtime Client projection).
///
/// This type is the leaf of the lock graph: it owns one mutex over a
/// `VecDeque` plus a `Notify` and calls nothing.
pub(crate) struct PendingObservations {
    /// The FIFO observation queue.
    queue: Mutex<VecDeque<ConversationObservation>>,
    /// Wakes the worker task on every push and on close.
    notify: tokio::sync::Notify,
    /// Set by [`close`](PendingObservations::close). Terminal: no further
    /// observation is accepted and the worker exits.
    closed: AtomicBool,
    /// Test-only worker-exit signal, so worker termination is observable
    /// deterministically instead of by timeout.
    #[cfg(test)]
    worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl PendingObservations {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            #[cfg(test)]
            worker_exit: Mutex::new(None),
        }
    }

    pub(crate) fn push(&self, observation: ConversationObservation) {
        if self.closed.load(Ordering::Acquire) {
            // A closed observation queue is terminal: never queue an
            // observation that nothing will ever fold.
            return;
        }
        self.queue
            .lock()
            .expect("pending observation queue lock poisoned")
            .push_back(observation);
        self.notify.notify_one();
    }

    pub(crate) fn drain(&self) -> Vec<ConversationObservation> {
        let mut queue = self
            .queue
            .lock()
            .expect("pending observation queue lock poisoned");
        queue.drain(..).collect()
    }

    /// Waits for the next push or for close.
    ///
    /// `Notify::notify_one` stores one permit even with no waiter, so a
    /// push or a close between two waits is never missed.
    pub(crate) async fn wait(&self) {
        self.notify.notified().await;
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The terminal close, performed when either owner is destroyed.
    ///
    /// Idempotent: the second close is a no-op. No concurrent producer can
    /// exist after the last owner drops: every producer reaches this queue
    /// through a live owner handle.
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.queue
            .lock()
            .expect("pending observation queue lock poisoned")
            .clear();
        self.notify.notify_one();
    }

    /// Installs the test-only worker-exit signal.
    #[cfg(test)]
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        *self
            .worker_exit
            .lock()
            .expect("worker exit probe lock poisoned") = Some(sender);
    }

    /// Fires the test-only worker-exit signal, once.
    #[cfg(test)]
    pub(crate) fn signal_worker_exit(&self) {
        if let Some(sender) = self
            .worker_exit
            .lock()
            .expect("worker exit probe lock poisoned")
            .take()
        {
            let _ = sender.send(());
        }
    }
}

/// The externally meaningful phase of one attempt's runtime semantics.
///
/// This is the runtime-owned semantic phase, folded from the attempt's own
/// events; the Runtime Client projection translates it into its attempt
/// view at adapter bootstrap.
#[derive(Debug, Clone)]
pub(crate) enum RuntimeAttemptPhase {
    /// The coordinator admitted the attempt; the loop has not started yet.
    Admitted,
    /// The attempt is executing.
    Running,
    /// The attempt settled; the terminal outcome is final and absorbing.
    Settled {
        /// The platform-level terminal settlement.
        outcome: AttemptOutcome,
    },
}

/// The accumulated in-flight output of one streaming Assistant message.
///
/// The runtime-owned semantic mirror of the streaming repair state; the
/// Runtime Client projection translates it into its in-flight view at
/// adapter bootstrap.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeInFlightMessage {
    /// The provisional message identity.
    pub message_id: MessageId,
    /// The ordered content blocks assembled so far.
    pub blocks: Vec<RuntimeInFlightBlock>,
}

/// One ordered block of an in-flight Assistant message.
#[derive(Debug, Clone)]
pub(crate) enum RuntimeInFlightBlock {
    /// Accumulated text of one output block.
    Text {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated text.
        text: String,
    },
    /// Accumulated reasoning of one output block.
    Reasoning {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated reasoning text.
        text: String,
    },
    /// Accumulated refusal of one output block.
    Refusal {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated refusal text.
        text: String,
    },
    /// One tool call being assembled.
    ToolCall {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The tool-call identity.
        call_id: ToolCallId,
        /// The canonical tool identity.
        tool_id: ToolId,
        /// The model-facing tool name.
        name: String,
        /// The accumulated JSON argument fragments.
        arguments: String,
    },
}

/// The foreground tool execution semantics of one logical tool call.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeForegroundExecution {
    /// The logical tool-call identity.
    pub call_id: ToolCallId,
    /// The canonical tool identity.
    pub tool_id: ToolId,
    /// The model-facing tool name at call time.
    pub name: String,
    /// The semantic execution state.
    pub state: RuntimeForegroundState,
}

/// The semantic state of one foreground tool execution.
#[derive(Debug, Clone)]
pub(crate) enum RuntimeForegroundState {
    /// The call is known and its arguments are assembled; execution has
    /// not started.
    Assembled {
        /// The assembled JSON arguments.
        arguments: String,
    },
    /// The execution is running.
    Running {
        /// The assembled JSON arguments.
        arguments: String,
        /// The latest bounded progress, when any.
        progress: Option<ToolProgress>,
    },
    /// The execution settled with its normalized result.
    Settled {
        /// The assembled JSON arguments.
        arguments: String,
        /// The normalized execution result.
        result: ToolExecutionResult,
    },
}

/// The runtime-owned semantics of the current attempt.
///
/// Folded from the attempt's own observation stream; the Runtime Client
/// projection translates this into its attempt view at adapter bootstrap.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeAttemptSemantics {
    /// The attempt identity.
    pub attempt_id: AttemptId,
    /// The semantic attempt phase.
    pub phase: RuntimeAttemptPhase,
    /// The number of completed turns.
    pub turn: u32,
    /// The latest normalized usage of a completed model request, when any.
    pub last_usage: Option<ModelUsage>,
    /// The in-flight Assistant output, when a message is streaming.
    pub in_flight: Option<RuntimeInFlightMessage>,
    /// The foreground tool executions of the attempt in call-assembly
    /// order.
    pub foreground: Vec<RuntimeForegroundExecution>,
    /// The immutable model snapshot this attempt was admitted with; `Some`
    /// from the admission's model freeze onward (the freeze observation
    /// follows the admission observation under the same coordinator lock,
    /// so no bootstrap can observe the intermediate state).
    pub model: Option<Box<AttemptModelView>>,
}

/// The runtime-owned committed compaction semantics.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeCompactionSemantics {
    /// The number of committed compaction completions folded so far.
    pub count: u64,
    /// The latest committed compaction facts, when compaction occurred.
    pub latest: Option<RuntimeCompactionFacts>,
}

/// The semantic facts of one committed compaction.
///
/// Every field is derived from already-committed conversation state; the
/// committed summary content itself is an ordinary canonical Ledger fact.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeCompactionFacts {
    /// The compaction generation maintained in the current Conversation
    /// Surface head.
    pub generation: u64,
    /// The identity of the committed canonical compaction summary message.
    pub summary_message_id: MessageId,
    /// The Conversation Surface revision established by the rewrite.
    pub surface_revision: SurfaceRevision,
    /// The pre-compaction input measurement and its provenance.
    pub tokens_before: TokenMeasurement,
    /// The deterministic estimate of the rebuilt request context.
    pub estimated_tokens_after: u64,
}

/// The runtime-owned derived semantic read state of one conversation.
///
/// See the module documentation for the ownership rationale. The record
/// is guarded by its own small leaf lock and is updated by
/// `RuntimeInner::observe` at every publication point; it is cleared at
/// attempt settlement, where the authoritative `ConversationState` becomes
/// directly readable again.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeSemanticRecord {
    /// The committed canonical messages of the active attempt, in commit
    /// order (including the drained inbound committed at admission).
    pub messages: Vec<MessageBlock>,
    /// The current attempt semantics, when an attempt is active.
    pub attempt: Option<RuntimeAttemptSemantics>,
    /// The latest composed Agent Status observation, when one exists.
    pub status: Option<AgentStatusObservation>,
    /// The committed compaction statistics.
    pub compaction: RuntimeCompactionSemantics,
}

impl RuntimeSemanticRecord {
    /// Folds one observation into the record.
    ///
    /// The fold mirrors the attempt-view semantics of the Runtime Client
    /// projection fold over the same observation vocabulary; the two run
    /// over disjoint time ranges (the record for the pre-bootstrap attempt
    /// history, the projection for the post-bootstrap live stream), and
    /// the bootstrap translation maps the record into the projection at a
    /// single cut.
    pub(crate) fn fold(&mut self, observation: &ConversationObservation) {
        match observation {
            ConversationObservation::Event { attempt_id, event } => {
                self.fold_event(attempt_id, event);
            }
            ConversationObservation::Committed { block, .. } => {
                self.messages.push(block.clone());
                if matches!(block, MessageBlock::Assistant(_))
                    && let Some(attempt) = &mut self.attempt
                {
                    attempt.in_flight = None;
                }
            }
            ConversationObservation::Status(observation) => {
                self.status = Some(observation.clone());
            }
            ConversationObservation::AttemptAdmitted { attempt_id } => {
                self.attempt = Some(RuntimeAttemptSemantics {
                    attempt_id: attempt_id.clone(),
                    phase: RuntimeAttemptPhase::Admitted,
                    turn: 0,
                    last_usage: None,
                    in_flight: None,
                    foreground: Vec::new(),
                    model: None,
                });
            }
            ConversationObservation::AttemptModelFrozen { attempt_id, model } => {
                if let Some(attempt) = self
                    .attempt
                    .as_mut()
                    .filter(|attempt| attempt.attempt_id == *attempt_id)
                {
                    attempt.model = Some(model.clone());
                }
            }
            ConversationObservation::InboundEnqueued(_)
            | ConversationObservation::InboundDrained(_)
            | ConversationObservation::Background(_)
            | ConversationObservation::Capability(_)
            | ConversationObservation::SessionModelChanged { .. }
            | ConversationObservation::Shutdown => {}
        }
    }

    /// Folds one attempt execution fact into the record's attempt semantics.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn fold_event(&mut self, attempt_id: &AttemptId, event: &RuntimeEvent) {
        match event {
            RuntimeEvent::AttemptStarted { .. } => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.phase = RuntimeAttemptPhase::Running;
                    attempt.turn = 0;
                }
                self.status = None;
            }
            RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
            | RuntimeEvent::AttemptFailed { .. } => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.phase = RuntimeAttemptPhase::Settled {
                        outcome: AttemptOutcome::from_terminal_event(event)
                            .expect("a terminal attempt event maps to exactly one outcome"),
                    };
                }
            }
            RuntimeEvent::TurnStarted => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.turn = attempt.turn.saturating_add(1);
                }
            }
            RuntimeEvent::TurnCompleted
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::ModelRetryScheduled { .. } => {}
            RuntimeEvent::ModelRequestCompleted { usage, .. } => {
                if let Some(usage) = usage
                    && let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.last_usage = Some(usage.clone());
                }
            }
            RuntimeEvent::AssistantMessageStarted { message_id } => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.in_flight = Some(RuntimeInFlightMessage {
                        message_id: message_id.clone(),
                        blocks: Vec::new(),
                    });
                }
            }
            RuntimeEvent::AssistantTextDelta {
                block_index, delta, ..
            } => {
                self.append_text(*block_index, delta, RuntimeTextKind::Text);
            }
            RuntimeEvent::AssistantReasoningDelta {
                block_index, delta, ..
            } => {
                self.append_text(*block_index, delta, RuntimeTextKind::Reasoning);
            }
            RuntimeEvent::AssistantRefusalDelta {
                block_index, delta, ..
            } => {
                self.append_text(*block_index, delta, RuntimeTextKind::Refusal);
            }
            RuntimeEvent::ToolCallStarted {
                block_index, call, ..
            } => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.foreground.push(RuntimeForegroundExecution {
                        call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                        state: RuntimeForegroundState::Assembled {
                            arguments: String::new(),
                        },
                    });
                    push_in_flight_block(
                        &mut attempt.in_flight,
                        RuntimeInFlightBlock::ToolCall {
                            block_index: *block_index,
                            call_id: call.id.clone(),
                            tool_id: call.tool_id.clone(),
                            name: call.name.clone(),
                            arguments: String::new(),
                        },
                    );
                }
            }
            RuntimeEvent::ToolCallArgumentsDelta {
                call_id,
                arguments_delta,
                ..
            } => {
                self.append_arguments(call_id, arguments_delta);
            }
            RuntimeEvent::ToolCallCompleted { call, .. } => {
                self.set_assembled(call);
            }
            // The loop does not emit identity-only committed-message
            // events yet (M8 owns the durable ledger); they carry no
            // attempt-view fact either way.
            RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. } => {}
            RuntimeEvent::ToolExecutionStarted { tool_call_id, .. } => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                    && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, tool_call_id)
                {
                    slot.state = RuntimeForegroundState::Running {
                        arguments: arguments_of(&slot.state),
                        progress: None,
                    };
                }
            }
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id,
                progress,
                ..
            } => {
                if let Some(attempt) = &mut self.attempt
                    && attempt.attempt_id == *attempt_id
                    && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, tool_call_id)
                    && let RuntimeForegroundState::Running { arguments, .. } = &slot.state
                {
                    slot.state = RuntimeForegroundState::Running {
                        arguments: arguments.clone(),
                        progress: Some(progress.clone()),
                    };
                }
            }
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } => {
                self.settle_foreground(tool_call_id, result.clone());
            }
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                error,
                ..
            } => {
                let result = ToolExecutionResult {
                    status: ToolExecutionStatus::Failed {
                        error: error.clone(),
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                };
                self.settle_foreground(tool_call_id, result);
            }
            RuntimeEvent::CompactionCompleted {
                generation,
                summary_message_id,
                surface_revision,
                tokens_before,
                estimated_tokens_after,
            } => {
                self.compaction.count = self
                    .compaction
                    .count
                    .checked_add(1)
                    .expect("compaction count cannot overflow");
                self.compaction.latest = Some(RuntimeCompactionFacts {
                    generation: *generation,
                    summary_message_id: summary_message_id.clone(),
                    surface_revision: *surface_revision,
                    tokens_before: *tokens_before,
                    estimated_tokens_after: *estimated_tokens_after,
                });
            }
            RuntimeEvent::CompactionStarted | RuntimeEvent::CompactionFailed { .. } => {}
        }
    }

    /// Appends one text/reasoning/refusal delta to the in-flight block.
    fn append_text(&mut self, block_index: ContentBlockIndex, delta: &str, kind: RuntimeTextKind) {
        let Some(attempt) = self.attempt.as_mut() else {
            return;
        };
        let Some(in_flight) = attempt.in_flight.as_mut() else {
            return;
        };
        let Some(existing) = in_flight
            .blocks
            .iter_mut()
            .find(|block| block_index_of(block) == block_index)
        else {
            in_flight.blocks.push(match kind {
                RuntimeTextKind::Text => RuntimeInFlightBlock::Text {
                    block_index,
                    text: delta.to_owned(),
                },
                RuntimeTextKind::Reasoning => RuntimeInFlightBlock::Reasoning {
                    block_index,
                    text: delta.to_owned(),
                },
                RuntimeTextKind::Refusal => RuntimeInFlightBlock::Refusal {
                    block_index,
                    text: delta.to_owned(),
                },
            });
            return;
        };
        match existing {
            RuntimeInFlightBlock::Text { text, .. }
            | RuntimeInFlightBlock::Reasoning { text, .. }
            | RuntimeInFlightBlock::Refusal { text, .. } => text.push_str(delta),
            RuntimeInFlightBlock::ToolCall { .. } => {}
        }
    }

    /// Appends one JSON argument fragment to the in-flight tool-call block
    /// and the foreground slot.
    fn append_arguments(&mut self, call_id: &ToolCallId, delta: &str) {
        if let Some(attempt) = self.attempt.as_mut()
            && let Some(in_flight) = attempt.in_flight.as_mut()
            && let Some(RuntimeInFlightBlock::ToolCall { arguments, .. }) = in_flight
                .blocks
                .iter_mut()
                .find(|block| matches!(block, RuntimeInFlightBlock::ToolCall { call_id: id, .. } if id == call_id))
        {
            arguments.push_str(delta);
        }
        if let Some(attempt) = self.attempt.as_mut()
            && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, call_id)
        {
            let arguments = arguments_of(&slot.state);
            slot.state = match slot.state.clone() {
                RuntimeForegroundState::Assembled { .. } => RuntimeForegroundState::Assembled {
                    arguments: format!("{arguments}{delta}"),
                },
                RuntimeForegroundState::Running { progress, .. } => {
                    RuntimeForegroundState::Running {
                        arguments: format!("{arguments}{delta}"),
                        progress,
                    }
                }
                RuntimeForegroundState::Settled { result, .. } => RuntimeForegroundState::Settled {
                    arguments: format!("{arguments}{delta}"),
                    result,
                },
            };
        }
    }

    /// Replaces a foreground slot's assembled arguments with the fully
    /// assembled canonical call.
    fn set_assembled(&mut self, call: &ToolCall) {
        let Some(attempt) = self.attempt.as_mut() else {
            return;
        };
        if let Some(in_flight) = attempt.in_flight.as_mut()
            && let Some(RuntimeInFlightBlock::ToolCall { arguments, .. }) = in_flight
                .blocks
                .iter_mut()
                .find(|block| matches!(block, RuntimeInFlightBlock::ToolCall { call_id: id, .. } if id == &call.id))
        {
            *arguments = call.arguments.to_string();
        }
        if let Some(slot) = foreground_slot_mut(&mut attempt.foreground, &call.id) {
            slot.state = match slot.state.clone() {
                RuntimeForegroundState::Assembled { .. } => RuntimeForegroundState::Assembled {
                    arguments: call.arguments.to_string(),
                },
                RuntimeForegroundState::Running { progress, .. } => {
                    RuntimeForegroundState::Running {
                        arguments: call.arguments.to_string(),
                        progress,
                    }
                }
                RuntimeForegroundState::Settled { result, .. } => RuntimeForegroundState::Settled {
                    arguments: call.arguments.to_string(),
                    result,
                },
            };
        }
    }

    /// Settles one foreground slot with its normalized result.
    fn settle_foreground(&mut self, call_id: &ToolCallId, result: ToolExecutionResult) {
        let Some(attempt) = self.attempt.as_mut() else {
            return;
        };
        if let Some(slot) = foreground_slot_mut(&mut attempt.foreground, call_id) {
            let arguments = arguments_of(&slot.state);
            slot.state = RuntimeForegroundState::Settled { arguments, result };
        }
    }
}

/// The text block kinds folded into the in-flight message.
#[derive(Debug, Clone, Copy)]
enum RuntimeTextKind {
    Text,
    Reasoning,
    Refusal,
}

/// The block index of one in-flight block.
fn block_index_of(block: &RuntimeInFlightBlock) -> ContentBlockIndex {
    match block {
        RuntimeInFlightBlock::Text { block_index, .. }
        | RuntimeInFlightBlock::Reasoning { block_index, .. }
        | RuntimeInFlightBlock::Refusal { block_index, .. }
        | RuntimeInFlightBlock::ToolCall { block_index, .. } => *block_index,
    }
}

/// Appends one in-flight block maintaining block-index order.
fn push_in_flight_block(
    in_flight: &mut Option<RuntimeInFlightMessage>,
    block: RuntimeInFlightBlock,
) {
    let Some(message) = in_flight.as_mut() else {
        return;
    };
    let index = block_index_of(&block);
    if !message
        .blocks
        .iter()
        .any(|existing| block_index_of(existing) == index)
    {
        message.blocks.push(block);
        message
            .blocks
            .sort_by_key(|existing| block_index_of(existing).get());
    }
}

/// The mutable foreground slot of one logical call.
fn foreground_slot_mut<'a>(
    foreground: &'a mut [RuntimeForegroundExecution],
    call_id: &ToolCallId,
) -> Option<&'a mut RuntimeForegroundExecution> {
    foreground.iter_mut().find(|slot| slot.call_id == *call_id)
}

/// The arguments string of one foreground state.
fn arguments_of(state: &RuntimeForegroundState) -> String {
    match state {
        RuntimeForegroundState::Assembled { arguments }
        | RuntimeForegroundState::Running { arguments, .. }
        | RuntimeForegroundState::Settled { arguments, .. } => arguments.clone(),
    }
}
