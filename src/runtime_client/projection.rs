//! The Runtime Client projection: the one linearization owner of externally
//! visible Runtime Client state.
//!
//! [`RuntimeClientProjection`] owns:
//!
//! ```text
//! current snapshot read model
//! RuntimeClientCursor allocation
//! published RuntimeClientEvent sequence
//! bounded in-memory replay/resume state
//! subscriber registration and delivery
//! ```
//!
//! Every externally meaningful runtime transition follows one path:
//!
//! ```text
//! authoritative runtime transition commits
//!         |
//!         v
//! projection fold (update the snapshot read model)
//!         |
//!         +--> allocate the next RuntimeClientCursor
//!         +--> publish the corresponding RuntimeClientEvent
//!         +--> deliver to every subscriber
//! ```
//!
//! Fold, cursor allocation, and publication happen under one
//! synchronization boundary (the Runtime Client host's projection state
//! lock), so the snapshot/cursor invariant holds by synchronization, never
//! by timing luck:
//!
//! > The returned snapshot describes authoritative Runtime Client state at
//! > cursor C. A subscription/resume after C observes every subsequently
//! > published Runtime Client event in that stream, or fails explicitly
//! > with `resync_required`.
//!
//! # Pre-M8 replay strategy and the one retained backlog
//!
//! Replay is a bounded in-memory ring (`cursor -> RuntimeClientEvent`)
//! with an explicit retention limit. That ring is the **sole** retained
//! Runtime Client event backlog: a subscription is a consumed cursor into
//! it, never a second queue.
//!
//! ```text
//! publish  -> allocate cursor -> push into the bounded ring
//!                             -> evict beyond the retention limit
//!                             -> wake every subscriber (edge-triggered)
//!
//! consume  -> subscriber reads the next retained entry after its cursor
//!          -> advances its own cursor
//!          -> falling behind retention yields resync_required
//! ```
//!
//! Consequences, which Issue #38 transport backpressure depends on:
//!
//! - a slow consumer costs one `RuntimeClientCursor`, never buffered
//!   events, so subscriber memory is O(1) and total retained memory is
//!   bounded by `replay_limit`;
//! - a consumer that falls behind retention can never silently skip
//!   events while still looking cursor-contiguous — the next poll is the
//!   explicit [`RuntimeClientError::ResyncRequired`] terminal result;
//! - wakeups are edge-triggered notifications only and carry no payload,
//!   so notification can never grow without bound and never blocks the
//!   publisher.
//!
//! There is no persistence, no Event Journal, and no crash-safe replay
//! claim.
//!
//! # `RuntimeEvent` mapping policy
//!
//! Every internal [`RuntimeEvent`] variant is classified explicitly:
//!
//! ```text
//! PROJECT                         -> one RuntimeClientEvent
//! FOLD INTO CLIENT STATE ONLY     -> snapshot update, no client event
//! INTERNAL / NOT EXPOSED          -> no snapshot change, no client event
//! ```
//!
//! Internal provider/request mechanics ([`ModelRequestStarted`],
//! [`ModelRequestFailed`], [`ModelRetryScheduled`]) and compaction
//! mechanics stay internal unless they express a client-relevant semantic
//! fact. The mapping is defined here, in one place, so internal
//! `RuntimeEvent` evolution cannot silently break Runtime Client Protocol
//! v1.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Notify;

use super::event::{RuntimeClientAttemptFailure, RuntimeClientEvent, RuntimeClientOutcome};
use super::snapshot::{
    AgentStatusView, CapabilityView, ForegroundToolExecution, ForegroundToolState,
    InFlightAssistantMessage, InFlightBlock, InboundDiagnostics, InboundDrainView, InboundItemView,
    RuntimeClientAttempt, RuntimeClientAttemptPhase, RuntimeClientBackgroundExecution,
    RuntimeClientCompactionView, RuntimeClientContextView, RuntimeClientSnapshot,
    RuntimeClientStatusFact, RuntimeClientStatusSection,
};
use super::types::{RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent};
use crate::agent::observer::AgentStatusObservation;
use crate::context::status::{AgentStatusSectionData, render_agent_status};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::{ContentBlockIndex, MessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::runtime::identity::{AttemptId, ConversationId, ToolCallId};
use crate::runtime::inbound::{InboundBatch, InboundItem};
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

/// The default bounded replay retention of the pre-M8 observation stream.
///
/// The ring retains at most this many published events; a resume after an
/// expired cursor fails with `resync_required` and the client repairs with
/// a fresh snapshot. This is an in-memory bound, never a durability claim.
pub const RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT: usize = 4096;

/// One authoritative runtime observation feeding the projection.
///
/// The observation union carries every external state change the
/// projection folds. It is the single entry point of the projection: no
/// call site folds state directly.
#[derive(Debug, Clone)]
pub(crate) enum Observation {
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
    /// One activated capability set.
    Capability(CapabilityView),
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

/// One registered subscriber of the observation stream.
///
/// A subscriber owns **no** event storage: it is a consumed cursor into
/// the one bounded replay ring plus an edge-triggered wakeup handle. A
/// stalled subscriber therefore cannot grow memory and cannot make the
/// publisher block.
struct Subscriber {
    /// The opaque registration identity (attachment-scoped).
    id: u64,
    /// The cursor through which this subscriber already consumed events.
    consumed: RuntimeClientCursor,
    /// The edge-triggered wakeup handle. Carries no payload.
    notify: Arc<Notify>,
}

/// The result of one subscriber poll against the bounded replay ring.
// The event variant is the overwhelmingly common one and is produced once
// per delivered event; boxing it would add an allocation to every delivery
// to shrink a short-lived stack value.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SubscriberPoll {
    /// The next retained event after the subscriber's consumed cursor.
    /// The subscriber's cursor advanced to this event's cursor.
    Event(RuntimeClientProtocolEvent),
    /// The subscriber is caught up: nothing published after its cursor.
    Pending,
    /// The subscription is no longer registered (detach, re-subscribe, or
    /// a dropped handle).
    Closed,
    /// The subscriber fell behind the bounded retention: the events it
    /// still needs were evicted. This is terminal for the subscription and
    /// is reported explicitly instead of silently skipping the gap.
    Lagged {
        /// The cursor the subscriber consumed through.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The cursor space is exhausted; the observation stream is over.
    Exhausted,
}

/// The projection state guarded by the Runtime Client host's one
/// synchronization boundary.
///
/// This struct owns no lock: the Runtime Client host
/// ([`crate::runtime_client::host::RuntimeClientHost`]) guards exactly one
/// instance with its projection state lock, making that lock the one
/// linearization owner of every externally visible transition. The
/// conversation runtime (Issue #61) publishes observations into the shared
/// leaf queue; every acquisition of this lock drains that queue first, so
/// the projection folds the coordinator's commits in order.
pub(crate) struct RuntimeClientProjection {
    /// The cursor of the last published event (0 = nothing published yet).
    cursor: RuntimeClientCursor,
    /// Set when the cursor space is exhausted: publishing stops and
    /// snapshot/subscribe fail explicitly. Never wraps.
    exhausted: bool,
    /// The deterministic snapshot read model.
    snapshot: RuntimeClientSnapshot,
    /// The bounded pre-M8 replay ring, oldest first.
    replay: VecDeque<(RuntimeClientCursor, RuntimeClientEvent)>,
    /// The explicit bounded retention limit.
    replay_limit: usize,
    /// The registered subscribers in registration order.
    subscribers: Vec<Subscriber>,
    /// The next opaque subscriber registration identity.
    next_subscriber_id: u64,
    /// Test-only linearization hooks for controlled race tests.
    #[cfg(test)]
    probe: Option<crate::runtime_client::test_sync::ProjectionProbe>,
}

impl RuntimeClientProjection {
    /// Creates the projection over one conversation with the initial
    /// canonical history and the initial capability view.
    pub(crate) fn new(
        conversation_id: ConversationId,
        initial_messages: Vec<MessageBlock>,
        initial_capabilities: CapabilityView,
        initial_model: SessionModelView,
        replay_limit: usize,
    ) -> Self {
        Self {
            cursor: RuntimeClientCursor::new(0),
            exhausted: false,
            snapshot: RuntimeClientSnapshot {
                conversation_id,
                shutting_down: false,
                messages: initial_messages,
                attempt: None,
                inbound: InboundDiagnostics {
                    pending: Vec::new(),
                    last_drain: None,
                },
                background: Vec::new(),
                status: None,
                context: RuntimeClientContextView::default(),
                capabilities: initial_capabilities,
                model: initial_model,
            },
            replay: VecDeque::new(),
            replay_limit,
            subscribers: Vec::new(),
            next_subscriber_id: 1,
            #[cfg(test)]
            probe: None,
        }
    }

    /// Installs the test-only linearization hooks. Only available under
    /// `#[cfg(test)]`; never used by production code.
    #[cfg(test)]
    pub(crate) fn install_probe(
        &mut self,
        probe: crate::runtime_client::test_sync::ProjectionProbe,
    ) {
        self.probe = Some(probe);
    }

    /// Applies one authoritative observation: fold the snapshot read
    /// model, publish the resulting events, and deliver to subscribers.
    ///
    /// This is the one projection application path. Fold, cursor
    /// allocation, replay retention, and delivery share the caller's
    /// synchronization boundary, so no caller may partially apply an
    /// observation.
    pub(crate) fn apply(&mut self, observation: Observation) {
        if self.exhausted {
            return;
        }
        let published = self.fold(observation);
        #[cfg(test)]
        self.probe_publish_enter();
        for event in published {
            self.publish(event);
        }
    }

    /// Folds one observation into the read model and returns the external
    /// events the observation publishes.
    #[allow(clippy::too_many_lines)]
    fn fold(&mut self, observation: Observation) -> Vec<RuntimeClientEvent> {
        match observation {
            Observation::Event { attempt_id, event } => self.fold_event(&attempt_id, &event),
            Observation::Committed { attempt_id, block } => {
                if matches!(block, MessageBlock::Assistant(_))
                    && let Some(attempt) = &mut self.snapshot.attempt
                {
                    attempt.in_flight = None;
                }
                self.snapshot.messages.push(block.clone());
                vec![RuntimeClientEvent::MessageCommitted {
                    attempt_id,
                    message: block,
                }]
            }
            Observation::Status(observation) => {
                let view = status_view(&observation);
                self.snapshot.status = Some(view.clone());
                vec![RuntimeClientEvent::AgentStatusComposed {
                    attempt_id: observation.attempt_id.clone(),
                    turn: observation.turn,
                    target_message_id: observation.target_message_id.clone(),
                    status: view,
                }]
            }
            Observation::InboundEnqueued(item) => {
                self.snapshot.inbound.pending.push(InboundItemView {
                    sequence: item.sequence(),
                    message: item.message().clone(),
                });
                vec![RuntimeClientEvent::InboundEnqueued {
                    sequence: item.sequence(),
                    message: item.message().clone(),
                }]
            }
            Observation::InboundDrained(batch) => {
                self.snapshot.inbound.pending.clear();
                self.snapshot.inbound.last_drain = Some(InboundDrainView {
                    watermark: batch.watermark(),
                    count: batch.items().len(),
                });
                vec![RuntimeClientEvent::InboundDrained {
                    watermark: batch.watermark(),
                    count: batch.items().len(),
                    message_ids: batch
                        .items()
                        .iter()
                        .map(|item| item.message().id.clone())
                        .collect(),
                }]
            }
            Observation::Background(snapshot) => {
                let view = background_view(&snapshot);
                upsert_background(&mut self.snapshot.background, view.clone());
                vec![RuntimeClientEvent::BackgroundExecutionUpdated { execution: view }]
            }
            Observation::Capability(capabilities) => {
                self.snapshot.capabilities = capabilities.clone();
                vec![RuntimeClientEvent::CapabilityPublished { capabilities }]
            }
            Observation::AttemptAdmitted { attempt_id } => {
                // The model is folded by the `AttemptModelFrozen`
                // observation the admission path publishes immediately
                // after this one, under the same lock acquisition.
                let model = Box::new(self.snapshot.model.to_attempt_view());
                self.snapshot.attempt = Some(RuntimeClientAttempt {
                    attempt_id,
                    phase: RuntimeClientAttemptPhase::Admitted,
                    turn: 0,
                    last_usage: None,
                    in_flight: None,
                    foreground: Vec::new(),
                    model,
                });
                Vec::new()
            }
            Observation::AttemptModelFrozen { attempt_id, model } => {
                if let Some(attempt) = self
                    .snapshot
                    .attempt
                    .as_mut()
                    .filter(|attempt| attempt.attempt_id == attempt_id)
                {
                    attempt.model = model;
                }
                Vec::new()
            }
            Observation::SessionModelChanged { model } => {
                self.snapshot.model = (*model).clone();
                vec![RuntimeClientEvent::SessionModelChanged { model }]
            }
            Observation::Shutdown => {
                self.snapshot.shutting_down = true;
                vec![RuntimeClientEvent::RuntimeShutdown]
            }
        }
    }

    /// The explicit `RuntimeEvent` mapping policy of Runtime Client Protocol
    /// v1.
    ///
    /// Classification (see the module documentation):
    ///
    /// - PROJECT: attempt lifecycle/settlement, streaming assistant
    ///   output, tool-call assembly, foreground tool lifecycle, and
    ///   progress;
    /// - PROJECT: turn counting and final request usage, carrying the exact
    ///   values folded into the attempt view;
    /// - INTERNAL: model request mechanics (`ModelRequestStarted`,
    ///   `ModelRequestFailed`, `ModelRetryScheduled`) and compaction start /
    ///   failure (`CompactionStarted/Failed`);
    /// - PROJECT: committed compaction completion, carrying only context
    ///   metadata from `CompactionCompleted`.
    // The mapping table is one explicit classification policy; identical
    // `Vec::new()` bodies mark intentionally distinct classes (the remaining
    // fold-only/internal observations) that must remain separately
    // documented.
    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn fold_event(
        &mut self,
        attempt_id: &AttemptId,
        event: &RuntimeEvent,
    ) -> Vec<RuntimeClientEvent> {
        match event {
            RuntimeEvent::AttemptStarted { .. } => {
                let model = self.frozen_attempt_model(attempt_id);
                self.snapshot.attempt = Some(RuntimeClientAttempt {
                    attempt_id: attempt_id.clone(),
                    phase: RuntimeClientAttemptPhase::Running,
                    turn: 0,
                    last_usage: None,
                    in_flight: None,
                    foreground: Vec::new(),
                    model: model.clone(),
                });
                self.snapshot.status = None;
                // The published event carries the same frozen model the
                // attempt read model carries, so an incremental subscriber
                // and a snapshot reader agree without inference.
                vec![RuntimeClientEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                    model,
                }]
            }
            RuntimeEvent::AttemptCompleted { finish_reason, .. } => self.settle_attempt(
                attempt_id,
                RuntimeClientOutcome::Completed {
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
                    error: client_failure(error),
                },
            ),
            RuntimeEvent::TurnStarted => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.turn = attempt.turn.saturating_add(1);
                    return vec![RuntimeClientEvent::AttemptTurnUpdated {
                        attempt_id: attempt_id.clone(),
                        turn: attempt.turn,
                    }];
                }
                Vec::new()
            }
            // INTERNAL: model request mechanics never produce client events —
            // the attempt settlement carries the normalized failure.
            RuntimeEvent::TurnCompleted
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ModelRequestFailed { .. }
            | RuntimeEvent::ModelRetryScheduled { .. } => Vec::new(),
            RuntimeEvent::ModelRequestCompleted { usage, .. } => {
                if let Some(usage) = usage
                    && let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id == *attempt_id
                {
                    attempt.last_usage = Some(usage.clone());
                    return vec![RuntimeClientEvent::AttemptUsageUpdated {
                        attempt_id: attempt_id.clone(),
                        usage: usage.clone(),
                    }];
                }
                Vec::new()
            }
            RuntimeEvent::AssistantMessageStarted { message_id } => {
                self.ensure_attempt(attempt_id);
                self.snapshot
                    .attempt
                    .as_mut()
                    .expect("attempt view exists")
                    .in_flight = Some(InFlightAssistantMessage {
                    message_id: message_id.clone(),
                    blocks: Vec::new(),
                });
                vec![RuntimeClientEvent::AssistantMessageStarted {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                }]
            }
            RuntimeEvent::AssistantTextDelta {
                message_id,
                block_index,
                delta,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, delta, TextKind::Text);
                vec![RuntimeClientEvent::AssistantTextDelta {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: delta.clone(),
                }]
            }
            RuntimeEvent::AssistantReasoningDelta {
                message_id,
                block_index,
                delta,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, delta, TextKind::Reasoning);
                vec![RuntimeClientEvent::AssistantReasoningDelta {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: delta.clone(),
                }]
            }
            RuntimeEvent::AssistantRefusalDelta {
                message_id,
                block_index,
                delta,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, delta, TextKind::Refusal);
                vec![RuntimeClientEvent::AssistantRefusalDelta {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    delta: delta.clone(),
                }]
            }
            RuntimeEvent::ToolCallStarted {
                message_id,
                block_index,
                call,
            } => {
                self.ensure_attempt(attempt_id);
                let attempt = self.snapshot.attempt.as_mut().expect("attempt view exists");
                attempt.foreground.push(ForegroundToolExecution {
                    call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                    state: ForegroundToolState::Assembled {
                        arguments: String::new(),
                    },
                });
                push_in_flight_block(
                    &mut attempt.in_flight,
                    InFlightBlock::ToolCall {
                        block_index: *block_index,
                        call_id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                        arguments: String::new(),
                    },
                );
                vec![RuntimeClientEvent::ToolCallStarted {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    call: call.clone(),
                }]
            }
            RuntimeEvent::ToolCallArgumentsDelta {
                message_id,
                block_index,
                call_id,
                arguments_delta,
            } => {
                self.append_arguments(call_id, arguments_delta);
                vec![RuntimeClientEvent::ToolCallArgumentsDelta {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    call_id: call_id.clone(),
                    arguments_delta: arguments_delta.clone(),
                }]
            }
            RuntimeEvent::ToolCallCompleted {
                message_id,
                block_index,
                call,
            } => {
                self.set_assembled(call);
                vec![RuntimeClientEvent::ToolCallAssembled {
                    attempt_id: attempt_id.clone(),
                    message_id: message_id.clone(),
                    block_index: *block_index,
                    call: call.clone(),
                }]
            }
            // The loop does not emit identity-only committed-message
            // events yet (M8 owns the durable ledger); if one ever
            // arrives it folds identity only and publishes nothing.
            RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. } => Vec::new(),
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id == *attempt_id
                    && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, tool_call_id)
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments_of(&slot.state),
                        progress: None,
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionStarted {
                    attempt_id: attempt_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                }]
            }
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id,
                tool_id,
                execution_id,
                progress,
            } => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id == *attempt_id
                    && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, tool_call_id)
                    && let ForegroundToolState::Running { arguments, .. } = &slot.state
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments.clone(),
                        progress: Some(progress.clone()),
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionProgress {
                    attempt_id: attempt_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    execution_id: execution_id.clone(),
                    progress: progress.clone(),
                }]
            }
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                result,
            } => {
                self.settle_foreground(tool_call_id, result.clone());
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    attempt_id: attempt_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    result: result.clone(),
                }]
            }
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id,
                tool_id,
                error,
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
                self.settle_foreground(tool_call_id, result.clone());
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    attempt_id: attempt_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    result,
                }]
            }
            RuntimeEvent::CompactionCompleted {
                generation,
                summary_message_id,
                surface_revision,
                tokens_before,
                estimated_tokens_after,
            } => {
                let compaction = RuntimeClientCompactionView {
                    generation: *generation,
                    summary_message_id: summary_message_id.clone(),
                    surface_revision: *surface_revision,
                    tokens_before: *tokens_before,
                    estimated_tokens_after: *estimated_tokens_after,
                };
                self.snapshot.context.compaction_count = self
                    .snapshot
                    .context
                    .compaction_count
                    .checked_add(1)
                    .expect("Runtime Client compaction count cannot overflow");
                self.snapshot.context.latest_compaction = Some(compaction);
                vec![RuntimeClientEvent::ContextCompacted {
                    attempt_id: attempt_id.clone(),
                    context: self.snapshot.context.clone(),
                }]
            }
            // INTERNAL: compaction start/failure never produce client events.
            RuntimeEvent::CompactionStarted | RuntimeEvent::CompactionFailed { .. } => Vec::new(),
        }
    }

    /// Folds one terminal attempt settlement: the attempt view settles and
    /// exactly one terminal client event is published.
    fn settle_attempt(
        &mut self,
        attempt_id: &AttemptId,
        outcome: RuntimeClientOutcome,
    ) -> Vec<RuntimeClientEvent> {
        if let Some(attempt) = &mut self.snapshot.attempt
            && attempt.attempt_id == *attempt_id
        {
            attempt.phase = RuntimeClientAttemptPhase::Settled {
                outcome: outcome.clone(),
            };
        }
        vec![RuntimeClientEvent::AttemptSettled {
            attempt_id: attempt_id.clone(),
            outcome,
        }]
    }

    /// Ensures the attempt view exists with a running phase (used by unit
    /// tests applying raw event sequences without prior admission).
    fn ensure_attempt(&mut self, attempt_id: &AttemptId) {
        if self
            .snapshot
            .attempt
            .as_ref()
            .is_none_or(|attempt| attempt.attempt_id != *attempt_id)
        {
            let model = self.frozen_attempt_model(attempt_id);
            self.snapshot.attempt = Some(RuntimeClientAttempt {
                attempt_id: attempt_id.clone(),
                phase: RuntimeClientAttemptPhase::Running,
                turn: 0,
                last_usage: None,
                in_flight: None,
                foreground: Vec::new(),
                model,
            });
        }
    }

    /// The model view an attempt froze at admission.
    ///
    /// Rebuilding the attempt view (for example when the loop's
    /// `AttemptStarted` arrives after admission) must never lose the frozen
    /// model: while the same attempt is described, the already-frozen view
    /// wins over live session state.
    fn frozen_attempt_model(&self, attempt_id: &AttemptId) -> Box<AttemptModelView> {
        match self
            .snapshot
            .attempt
            .as_ref()
            .filter(|attempt| attempt.attempt_id == *attempt_id)
        {
            Some(attempt) => attempt.model.clone(),
            None => Box::new(self.snapshot.model.to_attempt_view()),
        }
    }

    /// Appends one text/reasoning/refusal delta to the in-flight block.
    fn append_text(&mut self, block_index: ContentBlockIndex, delta: &str, kind: TextKind) {
        let Some(attempt) = self.snapshot.attempt.as_mut() else {
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
                TextKind::Text => InFlightBlock::Text {
                    block_index,
                    text: delta.to_owned(),
                },
                TextKind::Reasoning => InFlightBlock::Reasoning {
                    block_index,
                    text: delta.to_owned(),
                },
                TextKind::Refusal => InFlightBlock::Refusal {
                    block_index,
                    text: delta.to_owned(),
                },
            });
            return;
        };
        match existing {
            InFlightBlock::Text { text, .. }
            | InFlightBlock::Reasoning { text, .. }
            | InFlightBlock::Refusal { text, .. } => text.push_str(delta),
            InFlightBlock::ToolCall { .. } => {}
        }
    }

    /// Appends one JSON argument fragment to the in-flight tool-call block
    /// and the foreground slot.
    fn append_arguments(&mut self, call_id: &ToolCallId, delta: &str) {
        if let Some(attempt) = self.snapshot.attempt.as_mut()
            && let Some(in_flight) = attempt.in_flight.as_mut()
            && let Some(InFlightBlock::ToolCall { arguments, .. }) = in_flight
                .blocks
                .iter_mut()
                .find(|block| matches!(block, InFlightBlock::ToolCall { call_id: id, .. } if id == call_id))
        {
            arguments.push_str(delta);
        }
        if let Some(attempt) = self.snapshot.attempt.as_mut()
            && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, call_id)
        {
            let arguments = arguments_of(&slot.state);
            slot.state = match slot.state.clone() {
                ForegroundToolState::Assembled { .. } => ForegroundToolState::Assembled {
                    arguments: format!("{arguments}{delta}"),
                },
                ForegroundToolState::Running { progress, .. } => ForegroundToolState::Running {
                    arguments: format!("{arguments}{delta}"),
                    progress,
                },
                ForegroundToolState::Settled { result, .. } => ForegroundToolState::Settled {
                    arguments: format!("{arguments}{delta}"),
                    result,
                },
            };
        }
    }

    /// Replaces a foreground slot's assembled arguments with the fully
    /// assembled canonical call.
    fn set_assembled(&mut self, call: &ToolCall) {
        let Some(attempt) = self.snapshot.attempt.as_mut() else {
            return;
        };
        if let Some(in_flight) = attempt.in_flight.as_mut()
            && let Some(InFlightBlock::ToolCall { arguments, .. }) = in_flight
                .blocks
                .iter_mut()
                .find(|block| matches!(block, InFlightBlock::ToolCall { call_id: id, .. } if id == &call.id))
        {
            *arguments = call.arguments.to_string();
        }
        if let Some(slot) = foreground_slot_mut(&mut attempt.foreground, &call.id) {
            slot.state = match slot.state.clone() {
                ForegroundToolState::Assembled { .. } => ForegroundToolState::Assembled {
                    arguments: call.arguments.to_string(),
                },
                ForegroundToolState::Running { progress, .. } => ForegroundToolState::Running {
                    arguments: call.arguments.to_string(),
                    progress,
                },
                ForegroundToolState::Settled { result, .. } => ForegroundToolState::Settled {
                    arguments: call.arguments.to_string(),
                    result,
                },
            };
        }
    }

    /// Settles one foreground slot with its normalized result.
    fn settle_foreground(&mut self, call_id: &ToolCallId, result: ToolExecutionResult) {
        let Some(attempt) = self.snapshot.attempt.as_mut() else {
            return;
        };
        if let Some(slot) = foreground_slot_mut(&mut attempt.foreground, call_id) {
            let arguments = arguments_of(&slot.state);
            slot.state = ForegroundToolState::Settled { arguments, result };
        }
    }

    /// Publishes one folded client event: allocate the next cursor,
    /// retain the bounded replay entry, and wake every subscriber.
    ///
    /// Waking is edge-triggered and payload-free: no event copy is queued
    /// per subscriber, so publication is O(subscribers) pointer work with
    /// no per-subscriber memory and no possibility of blocking on a slow
    /// consumer.
    #[allow(clippy::needless_pass_by_value)] // the owned event is retained in the ring
    fn publish(&mut self, event: RuntimeClientEvent) {
        let next = self.cursor.get().checked_add(1);
        let Some(next_value) = next else {
            // Explicit exhaustion: the cursor never wraps, publication
            // stops, and every read path fails with `projection_exhausted`
            // instead of silently dropping the observation.
            self.exhausted = true;
            self.wake_subscribers();
            return;
        };
        self.cursor = RuntimeClientCursor::new(next_value);
        self.replay.push_back((self.cursor, event));
        while self.replay.len() > self.replay_limit {
            self.replay.pop_front();
        }
        self.wake_subscribers();
    }

    /// Wakes every registered subscriber. Notification carries no payload
    /// and never blocks the publisher.
    fn wake_subscribers(&self) {
        for subscriber in &self.subscribers {
            subscriber.notify.notify_one();
        }
    }

    /// The snapshot and its cursor, linearized together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted: after that point the projection can no longer
    /// fold authoritative transitions, so returning a stale read model
    /// would silently hide dropped observations.
    pub(crate) fn snapshot(
        &self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError> {
        #[cfg(test)]
        self.probe_snapshot_enter();
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        Ok((self.snapshot.clone(), self.cursor))
    }

    /// The oldest cursor a subscription can still serve: one before the
    /// oldest retained replay entry, or the current cursor when the ring
    /// is empty.
    fn earliest_serviceable(&self) -> RuntimeClientCursor {
        match self.replay.front() {
            Some((cursor, _)) => RuntimeClientCursor::new(cursor.get().saturating_sub(1)),
            None => self.cursor,
        }
    }

    /// Registers a subscriber consuming from `after_cursor` and returns its
    /// registration identity and wakeup handle.
    ///
    /// No events are copied into the subscriber: the bounded replay ring is
    /// the one retained backlog and the subscriber is a cursor into it.
    /// Serviceability is checked at registration under the caller's
    /// synchronization boundary, so a resume after a serviceable cursor has
    /// no gap and an expired cursor fails explicitly with
    /// `resync_required`.
    pub(crate) fn subscribe(
        &mut self,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(u64, Arc<Notify>), RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        let earliest = self.earliest_serviceable();
        if after_cursor > self.cursor || after_cursor < earliest {
            return Err(RuntimeClientError::ResyncRequired {
                after_cursor,
                earliest_serviceable: earliest,
            });
        }
        let subscriber_id = self.next_subscriber_id;
        self.next_subscriber_id = self.next_subscriber_id.saturating_add(1);
        let notify = Arc::new(Notify::new());
        self.subscribers.push(Subscriber {
            id: subscriber_id,
            consumed: after_cursor,
            notify: notify.clone(),
        });
        // A subscriber registered behind the current cursor already has
        // retained work: arm its first wakeup so a parked consumer never
        // waits for the next publication to observe the existing gap.
        if after_cursor < self.cursor {
            notify.notify_one();
        }
        Ok((subscriber_id, notify))
    }

    /// Polls one registered subscriber for the next retained event after
    /// its consumed cursor, advancing that cursor on delivery.
    ///
    /// This is the one delivery path: exactly one retained ring entry is
    /// handed out per call, so a consumer can never observe a
    /// non-contiguous cursor sequence. If the events the subscriber still
    /// needs were evicted, the poll reports [`SubscriberPoll::Lagged`]
    /// without advancing the cursor — the condition is stable and terminal
    /// until the client re-subscribes.
    pub(crate) fn poll_subscriber(&mut self, subscriber_id: u64) -> SubscriberPoll {
        if self.exhausted {
            return SubscriberPoll::Exhausted;
        }
        let earliest = self.earliest_serviceable();
        let Some(subscriber) = self
            .subscribers
            .iter_mut()
            .find(|subscriber| subscriber.id == subscriber_id)
        else {
            return SubscriberPoll::Closed;
        };
        let consumed = subscriber.consumed;
        if consumed >= self.cursor {
            return SubscriberPoll::Pending;
        }
        if consumed < earliest {
            return SubscriberPoll::Lagged {
                after_cursor: consumed,
                earliest_serviceable: earliest,
            };
        }
        let Some((cursor, event)) = self
            .replay
            .iter()
            .find(|(cursor, _)| *cursor > consumed)
            .map(|(cursor, event)| (*cursor, event.clone()))
        else {
            // Unreachable while `consumed < self.cursor` and
            // `consumed >= earliest_serviceable`; treated as lag rather
            // than as a silent skip.
            return SubscriberPoll::Lagged {
                after_cursor: consumed,
                earliest_serviceable: earliest,
            };
        };
        subscriber.consumed = cursor;
        SubscriberPoll::Event(RuntimeClientProtocolEvent { cursor, event })
    }

    /// Removes one registered subscriber (attachment detach, re-subscribe,
    /// or a dropped subscription handle) and wakes it so a parked consumer
    /// observes the closure instead of waiting forever.
    pub(crate) fn remove_subscriber(&mut self, subscriber_id: u64) {
        if let Some(index) = self
            .subscribers
            .iter()
            .position(|subscriber| subscriber.id == subscriber_id)
        {
            let subscriber = self.subscribers.remove(index);
            subscriber.notify.notify_one();
        }
    }

    /// The current cursor (host observability/tests).
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> RuntimeClientCursor {
        self.cursor
    }

    /// Forces the cursor near exhaustion (overflow tests).
    #[cfg(test)]
    pub(crate) fn force_cursor_for_test(&mut self, value: u64) {
        self.cursor = RuntimeClientCursor::new(value);
    }

    /// The current snapshot read model reference (host/tests).
    pub(crate) fn snapshot_ref(&self) -> &RuntimeClientSnapshot {
        &self.snapshot
    }

    /// The current snapshot read model reference, failing explicitly once
    /// the observation stream is exhausted.
    pub(crate) fn snapshot_ref_checked(
        &self,
    ) -> Result<&RuntimeClientSnapshot, RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        Ok(&self.snapshot)
    }

    /// The retained replay ring (tests).
    #[cfg(test)]
    #[allow(dead_code)] // asserted by the bounded-replay regression tests
    pub(crate) fn replay_len(&self) -> usize {
        self.replay.len()
    }

    #[cfg(test)]
    fn probe_publish_enter(&self) {
        if let Some(probe) = &self.probe {
            probe.publish_enter();
        }
    }

    #[cfg(test)]
    fn probe_snapshot_enter(&self) {
        if let Some(probe) = &self.probe {
            probe.snapshot_enter();
        }
    }
}

/// The text block kinds folded into the in-flight message.
#[derive(Debug, Clone, Copy)]
enum TextKind {
    Text,
    Reasoning,
    Refusal,
}

/// The block index of one in-flight block.
fn block_index_of(block: &InFlightBlock) -> ContentBlockIndex {
    match block {
        InFlightBlock::Text { block_index, .. }
        | InFlightBlock::Reasoning { block_index, .. }
        | InFlightBlock::Refusal { block_index, .. }
        | InFlightBlock::ToolCall { block_index, .. } => *block_index,
    }
}

/// Appends one in-flight block maintaining block-index order.
fn push_in_flight_block(in_flight: &mut Option<InFlightAssistantMessage>, block: InFlightBlock) {
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
    foreground: &'a mut [ForegroundToolExecution],
    call_id: &ToolCallId,
) -> Option<&'a mut ForegroundToolExecution> {
    foreground.iter_mut().find(|slot| slot.call_id == *call_id)
}

/// The arguments string of one foreground state.
fn arguments_of(state: &ForegroundToolState) -> String {
    match state {
        ForegroundToolState::Assembled { arguments }
        | ForegroundToolState::Running { arguments, .. }
        | ForegroundToolState::Settled { arguments, .. } => arguments.clone(),
    }
}

/// Projects one internal attempt failure into its external shape,
/// dropping provider-specific fields.
fn client_failure(failure: &AttemptFailure) -> RuntimeClientAttemptFailure {
    match failure {
        AttemptFailure::Model { error } => RuntimeClientAttemptFailure::Model {
            kind: error.kind.clone(),
            message: error.message.clone(),
            retry_after_ms: error.retry_after_ms,
        },
        AttemptFailure::Runtime { error } => RuntimeClientAttemptFailure::Runtime {
            error: error.clone(),
        },
    }
}

/// Projects one authoritative background registry snapshot into the
/// external Runtime Client shape.
pub(crate) fn background_view(
    snapshot: &BackgroundExecutionSnapshot,
) -> RuntimeClientBackgroundExecution {
    RuntimeClientBackgroundExecution {
        execution_id: snapshot.execution_id.clone(),
        tool_id: snapshot.tool_id.clone(),
        tool_name: snapshot.tool_name.clone(),
        state: snapshot.state,
        progress: snapshot.progress.clone(),
        result: snapshot.result.clone(),
    }
}

/// Builds the deterministic active capability projection from the
/// authoritative capability snapshot.
///
/// The tool catalog preserves registry order; the Skill catalog is
/// ordered by Skill name (the two snapshot lists derive from one sorted
/// package order). No executors, environment paths, or dependency
/// internals ever appear.
pub(crate) fn capability_view(
    snapshot: &crate::capabilities::CapabilitySnapshot,
) -> CapabilityView {
    let tools = snapshot
        .tool_registry()
        .definitions()
        .iter()
        .map(|definition| super::snapshot::RuntimeClientTool {
            id: definition.id.clone(),
            name: definition.name.clone(),
            description: definition.description.clone(),
            input_schema: definition.input_schema.clone(),
            execution_policy: definition.execution_policy,
            concurrency_policy: definition.concurrency_policy,
            replay_policy: definition.replay_policy,
            origin: definition.origin.clone(),
        })
        .collect();
    let skills = snapshot
        .catalog_entries()
        .iter()
        .zip(snapshot.skills().bindings())
        .map(|(entry, binding)| super::snapshot::RuntimeClientSkill {
            id: binding.skill_id.clone(),
            version_id: binding.version_id.clone(),
            name: entry.name.clone(),
            description: entry.description.clone(),
        })
        .collect();
    CapabilityView {
        revision: snapshot.revision(),
        tools,
        skills,
    }
}

/// Inserts or replaces one background view, preserving execution
/// allocation order.
fn upsert_background(
    background: &mut Vec<RuntimeClientBackgroundExecution>,
    view: RuntimeClientBackgroundExecution,
) {
    if let Some(existing) = background
        .iter_mut()
        .find(|entry| entry.execution_id == view.execution_id)
    {
        *existing = view;
    } else {
        background.push(view);
    }
}

/// Builds the structured external status view from the exact composed
/// status observation; the rendered representation derives from the same
/// composition through the one canonical renderer.
pub(crate) fn status_view(observation: &AgentStatusObservation) -> AgentStatusView {
    let sections = observation
        .status
        .sections
        .iter()
        .map(|section| match &section.data {
            AgentStatusSectionData::Temporal {
                current_time,
                timezone,
                inbound_message_time,
            } => RuntimeClientStatusSection::Temporal {
                current_time: *current_time,
                timezone: *timezone,
                inbound_message_time: *inbound_message_time,
            },
            AgentStatusSectionData::BackgroundExecution { executions } => {
                RuntimeClientStatusSection::BackgroundExecutions {
                    executions: executions.iter().map(background_view).collect(),
                }
            }
            AgentStatusSectionData::Facts { facts } => RuntimeClientStatusSection::Facts {
                facts: facts
                    .iter()
                    .map(|fact| RuntimeClientStatusFact {
                        label: fact.label.clone(),
                        value: fact.value.clone(),
                    })
                    .collect(),
            },
        })
        .collect();
    AgentStatusView {
        attempt_id: observation.attempt_id.clone(),
        turn: observation.turn,
        target_message_id: observation.target_message_id.clone(),
        rendered: render_agent_status(&observation.status),
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::{Observation, RuntimeClientProjection, SubscriberPoll};
    use crate::agent::{
        AgentExecution, AgentExecutionObserver, AgentExecutionRequest, AgentStatusObservation,
    };
    use crate::context::{
        AgentStatusComposer, CompactionBudgets, ContextEngine, ContextError, ContextErrorKind,
        ContextRuntime,
    };
    use crate::conversation::ConversationState;
    use crate::events::types::RuntimeEvent;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
        UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::ModelAdapter;
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId,
    };
    use crate::runtime::inbound::{ConversationInboundMailbox, InitialTurnTrigger};
    use crate::runtime::types::{CancellationReason, TokenMeasurement, TokenMeasurementSource};
    use crate::runtime_client::event::{RuntimeClientEvent, RuntimeClientOutcome};
    use crate::runtime_client::snapshot::{
        ForegroundToolState, InFlightBlock, RuntimeClientAttemptPhase,
    };
    use crate::runtime_client::types::RuntimeClientCursor;
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep};
    use crate::tools::executor::ToolRegistry;
    use crate::tools::types::{
        ToolCall, ToolCallStart, ToolExecutionResult, ToolExecutionStatus, ToolProgress,
    };
    use std::sync::{Arc, Mutex};

    fn attempt() -> AttemptId {
        AttemptId::new("attempt-1")
    }

    /// A deterministic session model view for projection unit tests.
    fn model_view() -> crate::model::session::SessionModelView {
        crate::scripted_suites::support::model::scripted_session_model(std::sync::Arc::new(
            crate::scripted_suites::support::model::NullAdapter,
        ))
        .view()
    }

    fn projection() -> RuntimeClientProjection {
        RuntimeClientProjection::new(
            ConversationId::new("conv-1"),
            Vec::new(),
            crate::runtime_client::snapshot::CapabilityView {
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                skills: Vec::new(),
            },
            model_view(),
            64,
        )
    }

    fn apply_event(projection: &mut RuntimeClientProjection, event: RuntimeEvent) {
        projection.apply(Observation::Event {
            attempt_id: attempt(),
            event,
        });
    }

    fn collect(
        projection: &mut RuntimeClientProjection,
        after: RuntimeClientCursor,
    ) -> Vec<crate::runtime_client::types::RuntimeClientProtocolEvent> {
        let (id, _notify) = projection.subscribe(after).expect("serviceable cursor");
        let mut events = Vec::new();
        loop {
            match projection.poll_subscriber(id) {
                SubscriberPoll::Event(event) => events.push(event),
                SubscriberPoll::Pending => break,
                other => panic!("unexpected subscriber poll: {other:?}"),
            }
        }
        projection.remove_subscriber(id);
        events
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CompactionOrderFact {
        /// The canonical runtime compaction summary joined the Message
        /// Ledger (observed at its commit linearization point).
        SummaryLedgerCommitted,
        RuntimeCompactionCompleted,
        ClientContextCompacted,
        ClientAttemptSettled,
    }

    struct EventPathObserver {
        projection: Mutex<RuntimeClientProjection>,
        subscriber: u64,
        facts: Arc<Mutex<Vec<CompactionOrderFact>>>,
    }

    impl EventPathObserver {
        fn new(
            initial_messages: Vec<MessageBlock>,
            facts: Arc<Mutex<Vec<CompactionOrderFact>>>,
        ) -> Self {
            let mut projection = RuntimeClientProjection::new(
                ConversationId::new("projection-order"),
                initial_messages,
                crate::runtime_client::snapshot::CapabilityView {
                    revision: crate::runtime::identity::CapabilityRevision::new(1),
                    tools: Vec::new(),
                    skills: Vec::new(),
                },
                model_view(),
                64,
            );
            let (subscriber, _notify) = projection
                .subscribe(RuntimeClientCursor::new(0))
                .expect("subscribe projection observer");
            Self {
                projection: Mutex::new(projection),
                subscriber,
                facts,
            }
        }

        fn drain_client_events(&self, projection: &mut RuntimeClientProjection) {
            loop {
                match projection.poll_subscriber(self.subscriber) {
                    SubscriberPoll::Event(event) => match event.event {
                        RuntimeClientEvent::ContextCompacted { .. } => self
                            .facts
                            .lock()
                            .expect("order facts lock")
                            .push(CompactionOrderFact::ClientContextCompacted),
                        RuntimeClientEvent::AttemptSettled { .. } => self
                            .facts
                            .lock()
                            .expect("order facts lock")
                            .push(CompactionOrderFact::ClientAttemptSettled),
                        _ => {}
                    },
                    SubscriberPoll::Pending => return,
                    other => panic!("projection observer lost its subscription: {other:?}"),
                }
            }
        }

        fn snapshot(&self) -> crate::runtime_client::snapshot::RuntimeClientSnapshot {
            self.projection
                .lock()
                .expect("projection lock")
                .snapshot()
                .expect("projection snapshot")
                .0
        }
    }

    impl AgentExecutionObserver for EventPathObserver {
        fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent) {
            if matches!(event, RuntimeEvent::CompactionCompleted { .. }) {
                self.facts
                    .lock()
                    .expect("order facts lock")
                    .push(CompactionOrderFact::RuntimeCompactionCompleted);
            }
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(Observation::Event {
                attempt_id: attempt_id.clone(),
                event: event.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_committed(&self, attempt_id: &AttemptId, block: &MessageBlock) {
            if matches!(
                block,
                MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
            ) {
                self.facts
                    .lock()
                    .expect("order facts lock")
                    .push(CompactionOrderFact::SummaryLedgerCommitted);
            }
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(Observation::Committed {
                attempt_id: Some(attempt_id.clone()),
                block: block.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_status(&self, observation: &AgentStatusObservation) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(Observation::Status(observation.clone()));
            self.drain_client_events(&mut projection);
        }
    }

    fn compactable_user(id: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "history".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    async fn run_compaction_order_case(
        fail_summary: bool,
    ) -> (
        crate::agent::AgentExecutionResult,
        Arc<EventPathObserver>,
        Arc<Mutex<Vec<CompactionOrderFact>>>,
    ) {
        let facts = Arc::new(Mutex::new(Vec::new()));
        let model = Arc::new(FakeModel::new(vec![vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]]));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let model_snapshot = crate::scripted_suites::support::attempt_model_with_window(
            adapter,
            "projection-order",
            200,
            1,
        );
        let initial_messages = vec![compactable_user("old-1"), compactable_user("old-2")];
        let engine = ContextEngine::new(
            crate::context::ContextConfig {
                context_window_tokens: 200,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(ScriptedEstimator::new(120, 0, 0)),
        )
        .expect("valid test context engine");
        let summary_step = if fail_summary {
            FakeSummaryStep::Fail(ContextError::new(
                ContextErrorKind::SummaryFailed,
                "scripted summary failure",
            ))
        } else {
            FakeSummaryStep::Return("summary".to_owned())
        };
        let runtime = ContextRuntime::with_scripted_summarizer(
            engine,
            Arc::new(FakeContextSummarizer::new(vec![summary_step])),
            AgentStatusComposer::default(),
            CompactionBudgets::new(1, 1, 1_000_000),
        );
        let tool_runtime = crate::scripted_suites::common::tool_runtime("projection-order");
        let capability =
            crate::scripted_suites::common::capability_lease(ToolRegistry::new(), &tool_runtime)
                .await;
        let request = AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id: ConversationId::new("projection-order"),
            attempt_id: AttemptId::new("attempt-order"),
            conversation: ConversationState::from_messages(initial_messages.clone())
                .expect("bootstrap conversation"),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            timezone: None,
            model: model_snapshot,
        };
        let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
        let observer = Arc::new(EventPathObserver::new(initial_messages, facts.clone()));
        let mut execution = AgentExecution::new(
            request,
            capability.into_lease(),
            &cancellation,
            runtime,
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.observe(observer.as_ref());
        let result = execution.run().await;
        (result, observer, facts)
    }

    /// A successful committed compaction has one ordered fact path: the
    /// canonical summary Ledger commit, the canonical completion event, the
    /// Runtime Client event, and terminal settlement.
    ///
    /// The ordering is the client-visible invariant: no `ContextCompacted`
    /// may imply success before the summary Ledger record and the new
    /// Surface revision exist.
    #[tokio::test]
    async fn committed_compaction_order_is_canonical_and_terminal_last() {
        let (result, observer, facts) = run_compaction_order_case(false).await;
        assert_eq!(
            *facts.lock().expect("order facts lock"),
            vec![
                CompactionOrderFact::SummaryLedgerCommitted,
                CompactionOrderFact::RuntimeCompactionCompleted,
                CompactionOrderFact::ClientContextCompacted,
                CompactionOrderFact::ClientAttemptSettled,
            ]
        );
        assert!(matches!(
            result.events.last(),
            Some(RuntimeEvent::AttemptCompleted { .. })
        ));
        assert_eq!(result.conversation.surface().compaction_generation(), 1);
        let snapshot = observer.snapshot();
        assert_eq!(snapshot.context.compaction_count, 1);
        let latest = snapshot
            .context
            .latest_compaction
            .expect("latest compaction");
        assert_eq!(latest.generation, 1);
        assert_eq!(
            latest.surface_revision,
            result
                .events
                .iter()
                .find_map(|event| match event {
                    RuntimeEvent::CompactionCompleted {
                        surface_revision, ..
                    } => Some(*surface_revision),
                    _ => None,
                })
                .expect("one compaction completion"),
            "the client view carries the revision the rewrite established"
        );
        assert!(
            snapshot.messages.iter().any(|message| matches!(
                message,
                MessageBlock::User(user)
                    if user.id == latest.summary_message_id
                        && user.kind == InboundKind::CompactionSummary
            )),
            "the committed runtime summary is an ordinary canonical ledger fact"
        );
    }

    /// A failed compaction emits the existing compaction failure and
    /// terminal settlement, but never commits a canonical summary and never
    /// emits canonical or client completion.
    #[tokio::test]
    async fn failed_compaction_has_no_summary_commit_or_client_event() {
        let (result, observer, facts) = run_compaction_order_case(true).await;
        assert_eq!(
            *facts.lock().expect("order facts lock"),
            vec![CompactionOrderFact::ClientAttemptSettled]
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionStarted))
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
        );
        assert!(
            result
                .events
                .iter()
                .all(|event| { !matches!(event, RuntimeEvent::CompactionCompleted { .. }) })
        );
        assert!(matches!(
            result.events.last(),
            Some(RuntimeEvent::AttemptFailed { .. })
        ));
        assert_eq!(result.conversation.surface().compaction_generation(), 0);
        assert!(
            result
                .conversation
                .ledger()
                .audit_records()
                .iter()
                .all(|message| !matches!(message, MessageBlock::User(user)
                    if user.kind == InboundKind::CompactionSummary)),
            "a failed compaction never commits a canonical summary"
        );
        let snapshot = observer.snapshot();
        assert_eq!(snapshot.context.compaction_count, 0);
        assert!(snapshot.context.latest_compaction.is_none());
    }

    /// Canonical completion events advance the external context view exactly
    /// once per committed generation and preserve token-measurement
    /// provenance.
    #[test]
    fn committed_compactions_are_visible_without_exposing_summary_content() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::CompactionCompleted {
                generation: 1,
                summary_message_id: MessageId::new("conv-1-summary-1"),
                surface_revision: crate::conversation::SurfaceRevision::new(3),
                tokens_before: TokenMeasurement {
                    input_tokens: 4800,
                    source: TokenMeasurementSource::ProviderReported,
                },
                estimated_tokens_after: 1700,
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::CompactionCompleted {
                generation: 2,
                summary_message_id: MessageId::new("conv-1-summary-2"),
                surface_revision: crate::conversation::SurfaceRevision::new(6),
                tokens_before: TokenMeasurement {
                    input_tokens: 4700,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 1800,
            },
        );

        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.messages, Vec::new());
        assert_eq!(snapshot.context.compaction_count, 2);
        let latest = snapshot
            .context
            .latest_compaction
            .expect("latest compaction");
        assert_eq!(latest.generation, 2);
        assert_eq!(
            latest.tokens_before.source,
            TokenMeasurementSource::Estimated
        );
        assert_eq!(latest.estimated_tokens_after, 1800);

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0].event,
            RuntimeClientEvent::ContextCompacted { attempt_id, context }
                if attempt_id == &attempt()
                    && context.compaction_count == 1
                    && context.latest_compaction.as_ref().is_some_and(|view| view.generation == 1)
        ));
        assert!(matches!(
            &events[1].event,
            RuntimeClientEvent::ContextCompacted { context, .. }
                if context.compaction_count == 2
                    && context.latest_compaction.as_ref().is_some_and(|view| view.generation == 2)
        ));
        assert_eq!(cursor, RuntimeClientCursor::new(2));
    }

    fn success_result() -> ToolExecutionResult {
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
        }
    }

    /// The representative attempt sequence: streaming assembly, a tool
    /// call, execution, and terminal settlement.
    fn representative_sequence() -> Vec<RuntimeEvent> {
        vec![
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
            RuntimeEvent::TurnStarted,
            RuntimeEvent::ModelRequestStarted {
                model: "scripted".to_owned(),
            },
            RuntimeEvent::AssistantMessageStarted {
                message_id: MessageId::new("msg-1"),
            },
            RuntimeEvent::AssistantTextDelta {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(0),
                delta: "hello ".to_owned(),
            },
            RuntimeEvent::AssistantTextDelta {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(0),
                delta: "world".to_owned(),
            },
            RuntimeEvent::ToolCallStarted {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(1),
                call: ToolCallStart {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                },
            },
            RuntimeEvent::ToolCallArgumentsDelta {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(1),
                call_id: ToolCallId::new("call_1"),
                arguments_delta: "{}".to_owned(),
            },
            RuntimeEvent::ToolCallCompleted {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(1),
                call: ToolCall {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            },
            RuntimeEvent::ModelRequestCompleted {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            },
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
            },
            RuntimeEvent::ToolExecutionProgress {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                execution_id: None,
                progress: ToolProgress {
                    message: Some("half way".to_owned()),
                    completed: Some(1.0),
                    total: Some(2.0),
                },
            },
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                result: success_result(),
            },
            RuntimeEvent::TurnCompleted,
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt(),
                finish_reason: ModelFinishReason::Stop,
            },
        ]
    }

    /// The projection is deterministic: applying the same representative
    /// sequence twice produces identical event sequences and identical
    /// snapshots.
    #[test]
    fn representative_sequence_projects_deterministically() {
        let mut first = projection();
        for event in representative_sequence() {
            apply_event(&mut first, event);
        }
        let mut second = projection();
        for event in representative_sequence() {
            apply_event(&mut second, event);
        }
        let first_events = collect(&mut first, RuntimeClientCursor::new(0));
        let second_events = collect(&mut second, RuntimeClientCursor::new(0));
        assert_eq!(first_events, second_events);
        assert!(!first_events.is_empty(), "the sequence publishes events");
        let (first_snapshot, first_cursor) = first.snapshot().expect("snapshot");
        let (second_snapshot, second_cursor) = second.snapshot().expect("snapshot");
        assert_eq!(first_snapshot, second_snapshot);
        assert_eq!(first_cursor, second_cursor);
    }

    /// Model-request mechanics and compaction start/failure stay internal;
    /// committed compaction completion is projected from its canonical event.
    #[test]
    fn internal_events_are_not_exposed_but_committed_compaction_is_projected() {
        let mut projection = projection();
        for event in [
            RuntimeEvent::ModelRequestStarted {
                model: "m".to_owned(),
            },
            RuntimeEvent::ModelRequestFailed {
                error: ModelError {
                    kind: ModelErrorKind::RateLimit,
                    message: "retry".to_owned(),
                    retry_after_ms: Some(10),
                    provider_code: Some("rate_limit_exceeded".to_owned()),
                },
            },
            RuntimeEvent::ModelRetryScheduled {
                attempt_number: 1,
                retry_delay_ms: Some(5),
            },
            RuntimeEvent::CompactionStarted,
            RuntimeEvent::CompactionCompleted {
                generation: 1,
                summary_message_id: MessageId::new("conv-1-summary-1"),
                surface_revision: crate::conversation::SurfaceRevision::new(3),
                tokens_before: TokenMeasurement {
                    input_tokens: 1,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 1,
            },
            RuntimeEvent::CompactionFailed {
                error: "boom".to_owned(),
            },
            RuntimeEvent::TurnCompleted,
        ] {
            apply_event(&mut projection, event);
        }
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].event,
            RuntimeClientEvent::ContextCompacted { context, .. }
                if context.compaction_count == 1
        ));
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(1));
        assert_eq!(snapshot.context.compaction_count, 1);
        assert!(snapshot.attempt.is_none());
    }

    /// Turn and usage observations publish the exact values folded into the
    /// attempt view, so incremental subscribers stay synchronized with a
    /// snapshot without polling.
    #[test]
    fn turn_and_usage_events_agree_with_the_attempt_view() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(&mut projection, RuntimeEvent::TurnStarted);
        apply_event(&mut projection, RuntimeEvent::TurnCompleted);
        apply_event(
            &mut projection,
            RuntimeEvent::ModelRequestCompleted {
                finish_reason: ModelFinishReason::Stop,
                usage: Some(ModelUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                    details: None,
                }),
            },
        );
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].event,
            RuntimeClientEvent::AttemptTurnUpdated {
                attempt_id: attempt(),
                turn: 1,
            }
        );
        assert_eq!(
            events[1].event,
            RuntimeClientEvent::AttemptUsageUpdated {
                attempt_id: attempt(),
                usage: ModelUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                    details: None,
                },
            }
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let attempt_view = snapshot.attempt.expect("attempt view exists");
        assert_eq!(attempt_view.turn, 1);
        assert_eq!(
            attempt_view
                .last_usage
                .as_ref()
                .map(|usage| usage.input_tokens),
            Some(10)
        );
    }

    #[test]
    fn shutdown_folds_into_the_snapshot_and_publishes_the_runtime_fact() {
        let mut projection = projection();
        projection.apply(Observation::Shutdown);

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, RuntimeClientEvent::RuntimeShutdown);

        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert!(snapshot.shutting_down);
        assert_eq!(cursor, RuntimeClientCursor::new(1));
    }

    /// Exactly one terminal settlement exists per attempt, and the
    /// terminal client event carries the platform outcome.
    #[test]
    fn terminal_settlement_is_unique() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt(),
                reason: CancellationReason::UserRequested,
            },
        );
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }))
                .count(),
            1,
            "exactly one AttemptSettled"
        );
        assert_eq!(
            events.last().expect("terminal event").event,
            RuntimeClientEvent::AttemptSettled {
                attempt_id: attempt(),
                outcome: RuntimeClientOutcome::Cancelled {
                    reason: CancellationReason::UserRequested,
                },
            }
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
    }

    /// Provider-specific failure fields never leak: the projected failure
    /// drops the provider error code.
    #[test]
    fn provider_specific_fields_do_not_leak() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptFailed {
                attempt_id: attempt(),
                error: crate::events::types::AttemptFailure::Model {
                    error: ModelError {
                        kind: ModelErrorKind::RateLimit,
                        message: "retries exhausted".to_owned(),
                        retry_after_ms: Some(5_000),
                        provider_code: Some("rate_limit_exceeded".to_owned()),
                    },
                },
            },
        );
        let events = collect(&mut projection, RuntimeClientCursor::new(1));
        let settled = events
            .iter()
            .find_map(|protocol| match &protocol.event {
                RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome),
                _ => None,
            })
            .expect("terminal settlement");
        let value = serde_json::to_value(settled).expect("serialize outcome");
        assert_eq!(value["type"], "failed");
        let text = serde_json::to_string(settled).expect("serialize");
        assert!(
            !text.contains("provider_code"),
            "provider-specific fields never leak: {text}"
        );
        assert!(text.contains("rate_limit"));
        assert!(text.contains("retries exhausted"));
        assert!(text.contains("5000"));
    }

    /// Streamed message state folds correctly into the in-flight repair
    /// view, and committing the message clears the in-flight state.
    #[test]
    fn streamed_message_state_folds_correctly() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AssistantMessageStarted {
                message_id: MessageId::new("msg-1"),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AssistantTextDelta {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(0),
                delta: "hello ".to_owned(),
            },
        );
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        let in_flight = snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.in_flight.as_ref())
            .expect("in-flight message");
        assert_eq!(
            in_flight.blocks,
            vec![InFlightBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: "hello ".to_owned(),
            }]
        );
        // Resume after C delivers exactly the remaining delta, never a
        // duplicate of the accumulated text.
        apply_event(
            &mut projection,
            RuntimeEvent::AssistantTextDelta {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(0),
                delta: "world".to_owned(),
            },
        );
        let resumed = collect(&mut projection, cursor);
        assert_eq!(resumed.len(), 1, "exactly one delta is published after C");
        assert!(matches!(
            &resumed[0].event,
            RuntimeClientEvent::AssistantTextDelta { delta, .. } if delta == "world"
        ));
        // The canonical commit clears the in-flight repair state.
        let committed = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-1"),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: "hello world".to_owned(),
            })],
        });
        projection.apply(Observation::Committed {
            attempt_id: Some(attempt()),
            block: committed.clone(),
        });
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.messages, vec![committed]);
        assert!(
            snapshot
                .attempt
                .as_ref()
                .and_then(|attempt| attempt.in_flight.as_ref())
                .is_none()
        );
    }

    /// Foreground tool state keeps canonical logical identities even when
    /// physical completion order is reversed.
    #[test]
    fn foreground_tool_identity_survives_reversed_completion() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        for call in [
            ToolCallStart {
                id: ToolCallId::new("call_a"),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
            },
            ToolCallStart {
                id: ToolCallId::new("call_b"),
                tool_id: ToolId::new("tool-beta"),
                name: "beta".to_owned(),
            },
        ] {
            apply_event(
                &mut projection,
                RuntimeEvent::ToolCallStarted {
                    message_id: MessageId::new("msg-1"),
                    block_index: ContentBlockIndex::new(0),
                    call,
                },
            );
        }
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_a"),
                tool_id: ToolId::new("tool-alpha"),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_b"),
                tool_id: ToolId::new("tool-beta"),
            },
        );
        // B settles first physically.
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_b"),
                tool_id: ToolId::new("tool-beta"),
                result: success_result(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_a"),
                tool_id: ToolId::new("tool-alpha"),
                result: success_result(),
            },
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let foreground = &snapshot.attempt.expect("attempt view").foreground;
        assert_eq!(foreground.len(), 2);
        assert_eq!(foreground[0].call_id, ToolCallId::new("call_a"));
        assert_eq!(foreground[1].call_id, ToolCallId::new("call_b"));
        assert!(matches!(
            foreground[0].state,
            ForegroundToolState::Settled { .. }
        ));
        assert!(matches!(
            foreground[1].state,
            ForegroundToolState::Settled { .. }
        ));
    }

    /// A resume after a serviceable cursor replays exactly the retained
    /// gap, in order, without duplicates or gaps.
    #[test]
    fn resume_after_a_serviceable_cursor_has_no_gap() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        for index in 0..10 {
            apply_event(
                &mut projection,
                RuntimeEvent::AssistantTextDelta {
                    message_id: MessageId::new("msg-1"),
                    block_index: ContentBlockIndex::new(0),
                    delta: format!("{index}"),
                },
            );
        }
        let (_, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(11));
        let events = collect(&mut projection, RuntimeClientCursor::new(3));
        let cursors: Vec<u64> = events.iter().map(|event| event.cursor.get()).collect();
        assert_eq!(cursors, vec![4, 5, 6, 7, 8, 9, 10, 11]);
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].cursor < pair[1].cursor)
        );
    }

    /// An expired cursor (evicted from the bounded ring) and a cursor
    /// ahead of the stream both fail explicitly with `resync_required`;
    /// a fresh snapshot still repairs all state.
    #[test]
    fn unserviceable_cursors_fail_with_resync_required() {
        let mut projection = RuntimeClientProjection::new(
            ConversationId::new("conv-1"),
            Vec::new(),
            crate::runtime_client::snapshot::CapabilityView {
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                skills: Vec::new(),
            },
            model_view(),
            4,
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AssistantMessageStarted {
                message_id: MessageId::new("msg-1"),
            },
        );
        for index in 0..20 {
            apply_event(
                &mut projection,
                RuntimeEvent::AssistantTextDelta {
                    message_id: MessageId::new("msg-1"),
                    block_index: ContentBlockIndex::new(0),
                    delta: format!("{index}"),
                },
            );
        }
        let error = projection
            .subscribe(RuntimeClientCursor::new(1))
            .expect_err("the cursor was evicted from the bounded ring");
        match error {
            crate::runtime_client::types::RuntimeClientError::ResyncRequired {
                after_cursor,
                earliest_serviceable,
            } => {
                assert_eq!(after_cursor, RuntimeClientCursor::new(1));
                assert_eq!(earliest_serviceable, RuntimeClientCursor::new(17));
            }
            other => panic!("expected resync_required, got {other:?}"),
        }
        let error = projection
            .subscribe(RuntimeClientCursor::new(100))
            .expect_err("a cursor ahead of the stream is unserviceable");
        assert!(matches!(
            error,
            crate::runtime_client::types::RuntimeClientError::ResyncRequired { .. }
        ));
        // A fresh snapshot repairs every externally visible fact.
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(21));
        let in_flight = snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.in_flight.as_ref())
            .expect("in-flight view repaired");
        assert!(matches!(
            &in_flight.blocks[0],
            InFlightBlock::Text { text, .. } if text == "012345678910111213141516171819"
        ));
    }

    /// A stalled subscriber introduces no second backlog: the bounded
    /// replay ring is the only retained storage, its size never exceeds the
    /// retention limit no matter how far behind the consumer is, and the
    /// consumer is told explicitly that it fell behind rather than being
    /// handed a silently non-contiguous stream.
    #[test]
    fn a_stalled_subscriber_is_bounded_and_never_silently_skips() {
        let limit = 4;
        let mut projection = RuntimeClientProjection::new(
            ConversationId::new("conv-1"),
            Vec::new(),
            crate::runtime_client::snapshot::CapabilityView {
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                skills: Vec::new(),
            },
            model_view(),
            limit,
        );
        apply_event(
            &mut projection,
            RuntimeEvent::AssistantMessageStarted {
                message_id: MessageId::new("msg-1"),
            },
        );
        let (stalled, _notify) = projection
            .subscribe(RuntimeClientCursor::new(1))
            .expect("the current cursor is serviceable");

        // The subscriber never polls while 200 events are published.
        for index in 0..200 {
            apply_event(
                &mut projection,
                RuntimeEvent::AssistantTextDelta {
                    message_id: MessageId::new("msg-1"),
                    block_index: ContentBlockIndex::new(0),
                    delta: format!("{index}"),
                },
            );
            assert!(
                projection.replay_len() <= limit,
                "the one retained backlog stays within its bound at every publication"
            );
        }
        assert_eq!(
            projection.replay_len(),
            limit,
            "retention is the only storage that grew, and only to its bound"
        );

        // Publication cost per subscriber is one wakeup, so the stalled
        // subscriber holds nothing: it is still sitting at its original
        // cursor.
        let poll = projection.poll_subscriber(stalled);
        let SubscriberPoll::Lagged {
            after_cursor,
            earliest_serviceable,
        } = poll
        else {
            panic!("a subscriber past retention must lag explicitly, got {poll:?}");
        };
        assert_eq!(after_cursor, RuntimeClientCursor::new(1));
        assert_eq!(earliest_serviceable, RuntimeClientCursor::new(197));
        assert_eq!(
            projection.poll_subscriber(stalled),
            poll,
            "the lag verdict is stable and never partially advances the cursor"
        );

        // A consumer that keeps up observes strictly contiguous cursors,
        // one event per poll, and then parks.
        let (live, _notify) = projection
            .subscribe(projection.cursor())
            .expect("the current cursor is serviceable");
        let base = projection.cursor().get();
        for step in 1..=3 {
            apply_event(
                &mut projection,
                RuntimeEvent::AssistantTextDelta {
                    message_id: MessageId::new("msg-1"),
                    block_index: ContentBlockIndex::new(0),
                    delta: "x".to_owned(),
                },
            );
            let poll = projection.poll_subscriber(live);
            let SubscriberPoll::Event(event) = poll else {
                panic!("the live subscriber must receive, got {poll:?}");
            };
            assert_eq!(event.cursor.get(), base + step);
        }
        assert_eq!(projection.poll_subscriber(live), SubscriberPoll::Pending);

        // Removal is observable, never an indefinite park.
        projection.remove_subscriber(live);
        assert_eq!(projection.poll_subscriber(live), SubscriberPoll::Closed);
    }

    /// Cursor overflow fails explicitly and never wraps: publishing stops,
    /// subscriptions fail with a typed error, and reads no longer hand back
    /// a read model that silently stopped folding transitions.
    #[test]
    fn cursor_overflow_fails_explicitly() {
        let mut projection = projection();
        projection.force_cursor_for_test(u64::MAX - 1);
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        assert_eq!(projection.cursor(), RuntimeClientCursor::new(u64::MAX));
        apply_event(&mut projection, RuntimeEvent::TurnStarted);
        apply_event(
            &mut projection,
            RuntimeEvent::AssistantTextDelta {
                message_id: MessageId::new("msg-1"),
                block_index: ContentBlockIndex::new(0),
                delta: "x".to_owned(),
            },
        );
        assert_eq!(
            projection.cursor(),
            RuntimeClientCursor::new(u64::MAX),
            "the cursor never wraps"
        );
        assert!(matches!(
            projection.subscribe(RuntimeClientCursor::new(0)),
            Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
        ));
        // Reads fail explicitly too: after exhaustion the projection can no
        // longer fold authoritative transitions, and returning the stale
        // read model would hide exactly that.
        assert!(matches!(
            projection.snapshot(),
            Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
        ));
        assert!(matches!(
            projection.snapshot_ref_checked(),
            Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
        ));
    }

    /// Mailbox observations fold into the inbound diagnostics view:
    /// enqueues appear as pending in inbound-sequence order and a finite
    /// drain clears them and records the watermark.
    #[test]
    fn inbound_diagnostics_fold_from_mailbox_observations() {
        let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
        let item = |id: &str| UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: id.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse")
                    .with_timezone(&chrono::Utc),
            ),
        };
        let first = mailbox.enqueue(item("m1")).expect("enqueue");
        let second = mailbox.enqueue(item("m2")).expect("enqueue");

        let mut projection = projection();
        let drained = mailbox.drain().expect("batch");
        let _ = (first, second);
        // The authoritative items fold through the enqueue observations.
        for entry in drained.items() {
            projection.apply(Observation::InboundEnqueued(entry.clone()));
        }
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.inbound.pending.len(), 2);
        assert_eq!(
            snapshot.inbound.pending[0].sequence.get(),
            1,
            "pending items follow inbound sequence order"
        );
        assert_eq!(snapshot.inbound.pending[1].sequence.get(), 2);

        // The finite drain of the same batch clears the pending view and
        // records the watermark.
        projection.apply(Observation::InboundDrained(drained));
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert!(snapshot.inbound.pending.is_empty());
        let last_drain = snapshot.inbound.last_drain.expect("drain recorded");
        assert_eq!(last_drain.watermark.get(), 2);
        assert_eq!(last_drain.count, 2);
    }

    /// The snapshot and its cursor linearize: the returned cursor is the
    /// cursor of the returned snapshot.
    #[test]
    fn snapshot_and_cursor_are_linearized() {
        let mut projection = projection();
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(snapshot.messages.is_empty());
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(1));
        assert!(snapshot.attempt.is_some());
    }

    /// Background registry observations fold into the background section
    /// preserving allocation order and terminal retention.
    #[test]
    fn background_observations_fold_in_allocation_order() {
        use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
        let mut projection = projection();
        projection.apply(Observation::Background(BackgroundExecutionSnapshot {
            execution_id: crate::runtime::identity::ToolExecutionId::new("exec_1"),
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".to_owned(),
            state: BackgroundLifecycle::Running,
            progress: None,
            result: None,
        }));
        projection.apply(Observation::Background(BackgroundExecutionSnapshot {
            execution_id: crate::runtime::identity::ToolExecutionId::new("exec_1"),
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".to_owned(),
            state: BackgroundLifecycle::Succeeded,
            progress: None,
            result: Some(success_result()),
        }));
        projection.apply(Observation::Background(BackgroundExecutionSnapshot {
            execution_id: crate::runtime::identity::ToolExecutionId::new("exec_2"),
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".to_owned(),
            state: BackgroundLifecycle::Starting,
            progress: None,
            result: None,
        }));
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.background.len(), 2);
        assert_eq!(snapshot.background[0].execution_id.as_str(), "exec_1");
        assert_eq!(snapshot.background[0].state, BackgroundLifecycle::Succeeded);
        assert!(snapshot.background[0].result.is_some());
        assert_eq!(snapshot.background[1].execution_id.as_str(), "exec_2");
    }
}
