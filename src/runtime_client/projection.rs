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
//! # Projection replay cache and the one retained backlog
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
//! The ring is not durable persistence and is never a recovery input. Durable
//! Event Journal reads and current Surface bootstrap remain `ConversationStore`
//! responsibilities.
//!
//! Live native interactions follow the same fold. A pending Approval or
//! Questionnaire is snapshot state and a terminal interaction observation
//! removes it; neither becomes canonical conversation history. The projection
//! never answers an interaction or infers an outcome from attachment loss.
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
//! `RuntimeEvent` evolution cannot silently break the Runtime Client protocol.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Notify;

use super::event::{RuntimeClientAttemptFailure, RuntimeClientEvent, RuntimeClientOutcome};
use super::snapshot::{
    AGENT_STATUS_WINDOW, AgentStatusOpportunityView, AgentStatusView, CapabilityView,
    ForegroundToolExecution, ForegroundToolState, FreshInboundStatusOpportunityView,
    InFlightAssistantMessage, InFlightBlock, InboundDiagnostics, InboundDrainView, InboundItemView,
    PostToolBatchStatusOpportunityView, RuntimeClientAttempt, RuntimeClientAttemptPhase,
    RuntimeClientBackgroundExecution, RuntimeClientCompactionView, RuntimeClientContextView,
    RuntimeClientSnapshot, RuntimeClientStatusSection, RuntimeClientTodoStatusTask,
    RuntimeClientTranscriptCursor,
};
use super::types::{RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent};
use crate::agent::observer::AgentStatusObservation;
use crate::context::status::{AgentStatusSectionData, render_agent_status};
use crate::events::types::{AttemptFailure, RuntimeEvent};
use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::publication::{PublicationFrame, PublicationPayload};
use crate::runtime::identity::{AttemptId, ConversationId, MessageId, ToolCallId};
use crate::runtime::inbound::InboundItem;
use crate::runtime::observation::ConversationObservation;
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

/// The default bounded replay retention of the projection observation stream.
///
/// The ring retains at most this many published events; a resume after an
/// expired cursor fails with `resync_required` and the client repairs with
/// a fresh snapshot. This is an in-memory bound, never a durability claim.
pub const RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT: usize = 4096;

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

/// The result of applying one foreground settlement to the projection.
///
/// The live execution event and canonical `ToolMessage` commit both call the
/// same helper. `AlreadySettled` is the explicit deduplication rule: a
/// foreground slot transitions to `Settled` at most once, and a later physical
/// event cannot overwrite the canonical result that won. A missing slot
/// preserves the existing event mapping for raw lifecycle observations while
/// never inventing a foreground slot on a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForegroundSettlement {
    Applied,
    AlreadySettled,
    Missing,
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
    /// The bounded projection replay ring, oldest first.
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
    /// Creates the projection over one conversation with the current Surface
    /// working set and the initial capability view.
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
                effective_approval_mode: ApprovalMode::Policy,
                pending_approval_mode: None,
                approval_mode_revision: 0,
                durability_failure: None,
                messages: initial_messages,
                transcript: super::snapshot::RuntimeClientTranscriptPage::default(),
                attempt: None,
                inbound: InboundDiagnostics {
                    pending: Vec::new(),
                    last_drain: None,
                },
                pending_interactions: Vec::new(),
                background: Vec::new(),
                subagents: Vec::new(),
                statuses: Vec::new(),
                context: RuntimeClientContextView::default(),
                capabilities: initial_capabilities,
                resources: super::snapshot::RuntimeClientResourcesView::default(),
                model: initial_model,
                todos: crate::tools::todo::TodoSnapshot::empty(),
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

    /// Seeds the projection from the runtime-owned bootstrap snapshot.
    ///
    /// The conversation runtime captured every fact of this snapshot at
    /// one global cut `R` while it was still inactive (see
    /// `ConversationRuntime::install_observation_bridge`); this method
    /// owns the translation of the semantic source types into the client
    /// snapshot read model.
    ///
    /// **Nothing here publishes and nothing here allocates a cursor.**
    /// The seed *is* the state at cursor 0 — current Surface, session
    /// model, the startup capability snapshot, and pending inbound — all
    /// installed as snapshot state rather than replayed through
    /// [`RuntimeClientProjection::apply`], so state that existed before
    /// the bootstrap cut can never fabricate a live event. The background
    /// seed is provably empty by the ownership-transfer invariant (a
    /// `ConversationRuntime` is constructed only over a pristine
    /// tool-runtime background plane, and the transfer then refuses
    /// dispatch commits while its mailbox is bound inactive) and is still
    /// installed from the same seed for one coherent cut. Every
    /// transition after `R` arrives through the live observation stream
    /// and gets the first real cursor.
    pub(crate) fn bootstrap(
        &mut self,
        seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    ) {
        self.snapshot.transcript = super::snapshot::transcript_page_view(seed.transcript.clone())
            .expect("runtime bootstrap transcript is valid");
        self.snapshot.shutting_down = seed.shutting_down;
        self.snapshot.effective_approval_mode = seed.approval_mode.effective;
        self.snapshot.pending_approval_mode = (seed.approval_mode.effective
            != seed.approval_mode.desired)
            .then_some(seed.approval_mode.desired);
        self.snapshot.approval_mode_revision = seed.approval_mode.revision;
        self.snapshot.inbound.pending =
            seed.inbound_pending.iter().map(inbound_item_view).collect();
        self.snapshot
            .pending_interactions
            .clone_from(&seed.pending_interactions);
        for existing in &seed.background {
            upsert_background(&mut self.snapshot.background, background_view(existing));
        }
        for existing in &seed.subagents {
            upsert_subagent(&mut self.snapshot.subagents, subagent_view(existing));
        }
        self.snapshot.resources = resources_view(&seed.resources);
        self.snapshot.todos = seed.todos.clone();
        // An inactive runtime has never admitted an attempt, composed an
        // Agent Status, or compacted, so `attempt`, `statuses`, and
        // `context` keep their empty initial values by construction.
    }

    /// Folds one durable Event Journal fact into a read-only conversation
    /// attachment without publishing a live Runtime Client event.
    ///
    /// Durable inspection has no observation bridge and therefore no live
    /// cursor stream to replay. The journal is still the child conversation's
    /// execution-history authority, so a fresh attachment folds its bounded
    /// execution facts into the ordinary snapshot projection before the
    /// client is initialized. Message bodies are intentionally absent here:
    /// committed messages are loaded separately from the child Message
    /// Ledger/transcript authority.
    pub(crate) fn bootstrap_durable_event(
        &mut self,
        envelope: &crate::events::types::RuntimeEventEnvelope,
    ) {
        if self.exhausted {
            return;
        }
        let event = &envelope.event;
        match event {
            RuntimeEvent::CompactionStarted
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::CompactionFailed { .. } => {
                self.fold_compaction_event(envelope.attempt_id.as_ref(), event);
            }
            _ => {
                let Some(attempt_id) = envelope.attempt_id.as_ref().or(match event {
                    RuntimeEvent::AttemptStarted { attempt_id }
                    | RuntimeEvent::AttemptCompleted { attempt_id, .. }
                    | RuntimeEvent::AttemptCancelled { attempt_id, .. }
                    | RuntimeEvent::AttemptTimedOut { attempt_id, .. }
                    | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
                    | RuntimeEvent::AttemptFailed { attempt_id, .. } => Some(attempt_id),
                    _ => None,
                }) else {
                    // Journal facts without attempt attribution either have
                    // no client snapshot representation or are represented
                    // by the durable transcript itself.
                    return;
                };
                self.fold_event(attempt_id, event);
            }
        }
    }

    /// Seeds the normal foreground-tool read model from one canonical child
    /// message while replaying the child's durable Event Journal. The
    /// assistant message remains in the Message Ledger; this only gives the
    /// projection the same call slots that live publication frames create
    /// before `ToolExecutionStarted`/`ToolExecutionCompleted` facts arrive.
    ///
    /// No message is copied into another durable authority and no client event
    /// is published by this bootstrap-only operation.
    pub(crate) fn bootstrap_durable_message(
        &mut self,
        attempt_id: &AttemptId,
        message: &MessageBlock,
    ) {
        let MessageBlock::Assistant(assistant) = message else {
            if let MessageBlock::Tool(tool) = message {
                let _ = self.settle_foreground(attempt_id, &tool.tool_call_id, tool.result.clone());
            }
            return;
        };
        self.ensure_attempt(attempt_id);
        let attempt = self
            .snapshot
            .attempt
            .as_mut()
            .expect("durable message bootstrap creates the attempt view");
        for block in &assistant.content {
            let AssistantContentBlock::ToolCall(call) = block else {
                continue;
            };
            if attempt
                .foreground
                .iter()
                .any(|slot| slot.call_id == call.id)
            {
                continue;
            }
            attempt.foreground.push(ForegroundToolExecution {
                call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
                state: ForegroundToolState::Assembled {
                    arguments: call.arguments.to_string(),
                },
            });
        }
    }

    /// Applies one authoritative observation: fold the snapshot read
    /// model, publish the resulting events, and deliver to subscribers.
    ///
    /// This is the one projection application path. Fold, cursor
    /// allocation, replay retention, and delivery share the caller's
    /// synchronization boundary, so no caller may partially apply an
    /// observation.
    pub(crate) fn apply(&mut self, observation: ConversationObservation) {
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
    fn fold(&mut self, observation: ConversationObservation) -> Vec<RuntimeClientEvent> {
        match observation {
            ConversationObservation::Event { attempt_id, event } => {
                self.fold_event(&attempt_id, &event)
            }
            ConversationObservation::ManualCompactionEvent { event } => {
                self.fold_compaction_event(None, &event)
            }
            ConversationObservation::Committed {
                attempt_id,
                block,
                transcript_cursor,
            } => {
                // A canonical ToolMessage is the authoritative result fact.
                // It repairs an accepted foreground slot that has not seen a
                // live execution settlement (notably BeforeStart, which must
                // never fabricate started/completed facts). A slot transitions
                // to Settled at most once: the live event path and this commit
                // path share the same idempotent helper, and only the winner
                // publishes ToolExecutionSettled.
                let mut events = Vec::with_capacity(2);
                if let (Some(attempt_id), MessageBlock::Tool(tool)) = (attempt_id.as_ref(), &block)
                    && self.settle_foreground(attempt_id, &tool.tool_call_id, tool.result.clone())
                        == ForegroundSettlement::Applied
                {
                    events.push(RuntimeClientEvent::ToolExecutionSettled {
                        attempt_id: attempt_id.clone(),
                        tool_call_id: tool.tool_call_id.clone(),
                        tool_id: tool.tool_id.clone(),
                        result: tool.result.clone(),
                    });
                }
                if matches!(block, MessageBlock::Assistant(_))
                    && let Some(attempt) = &mut self.snapshot.attempt
                {
                    attempt.in_flight = None;
                }
                // The task list is derived from canonical results, exactly
                // as the runtime's own list is: a committed `todo` result
                // *is* the list moving. A result whose payload does not
                // decode is left out rather than allowed to replace a good
                // list with a broken one — the runtime writes these, so an
                // undecodable one is a defect, not a list.
                if let Some(Ok(todos)) = crate::tools::todo::published_snapshot(&block) {
                    self.snapshot.todos = todos;
                }
                self.snapshot.messages.push(block.clone());
                let transcript_cursor = transcript_cursor.map(RuntimeClientTranscriptCursor::from);
                events.push(RuntimeClientEvent::MessageCommitted {
                    attempt_id,
                    message: block,
                    transcript_cursor,
                });
                events
            }
            // The three publication observations replace what used to be
            // per-delta Event Journal facts (Issue #108). The client-facing
            // vocabulary is unchanged: a frame folds into exactly the
            // streaming event a delta used to publish, and every one of them
            // is already durably committed for release before it arrives.
            ConversationObservation::PublicationOpened { attempt_id, start } => {
                self.ensure_attempt(&attempt_id);
                self.snapshot
                    .attempt
                    .as_mut()
                    .expect("attempt view exists")
                    .in_flight = Some(InFlightAssistantMessage {
                    message_id: start.message_id.clone(),
                    blocks: Vec::new(),
                });
                vec![RuntimeClientEvent::AssistantMessageStarted {
                    attempt_id,
                    message_id: start.message_id,
                }]
            }
            ConversationObservation::Publication { attempt_id, frame } => {
                self.fold_publication_frame(&attempt_id, &frame)
            }
            // An audit is a derived transcript item, but it is not a
            // canonical Message Ledger message and never becomes an
            // in-flight message. It only closes the in-flight read model of a
            // stream that will never be canonically accepted.
            ConversationObservation::PublicationSettled {
                attempt_id,
                audit,
                transcript_cursor,
            } => {
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id == attempt_id
                {
                    attempt.in_flight = None;
                }
                let transcript_cursor = RuntimeClientTranscriptCursor::from(transcript_cursor);
                vec![RuntimeClientEvent::AssistantPublicationSettled {
                    attempt_id,
                    audit,
                    transcript_cursor,
                }]
            }
            // A composed status joins the bounded window in composition
            // order. It replaces nothing: an earlier composition is a
            // historical fact of this conversation and stays exactly where
            // the runtime put it.
            //
            // Placement is projected, never reconstructed: the observation
            // already carries the durable identity its opportunity froze, so
            // nothing here consults the conversation's current transcript
            // position. Doing that would sample a frontier that unrelated
            // durable activity may have advanced since the composition was
            // determined.
            //
            // Retention is owned here and published as part of the
            // transition. The event carries the exact eviction this
            // admission caused, so an incremental fold reproduces this
            // window instead of applying a second retention policy of its
            // own — which is what makes a live fold and a snapshot repair at
            // the same cursor agree past the window bound.
            ConversationObservation::Status(observation) => {
                let view = status_view(&observation);
                let evicted_status_message_id =
                    admit_status(&mut self.snapshot.statuses, view.clone());
                vec![RuntimeClientEvent::AgentStatusComposed {
                    attempt_id: observation.attempt_id.clone(),
                    turn: observation.turn,
                    status: view,
                    evicted_status_message_id,
                }]
            }
            ConversationObservation::InboundEnqueued(item) => {
                self.snapshot.inbound.pending.push(inbound_item_view(&item));
                let transcript_cursor = item
                    .transcript_cursor()
                    .map(RuntimeClientTranscriptCursor::from);
                vec![RuntimeClientEvent::InboundEnqueued {
                    sequence: item.sequence(),
                    message: item.message().clone(),
                    transcript_cursor,
                }]
            }
            ConversationObservation::InboundDrained(batch) => {
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
            ConversationObservation::InteractionPending { interaction, audit } => {
                upsert_interaction(&mut self.snapshot.pending_interactions, interaction.clone());
                let mut events = vec![RuntimeClientEvent::InteractionPending { interaction }];
                if let Some((audit, transcript_cursor)) = audit {
                    let audit_view = super::snapshot::interaction_requested_view(audit)
                        .expect("runtime interaction requested audit is valid");
                    let transcript_cursor = RuntimeClientTranscriptCursor::from(transcript_cursor);
                    events.push(RuntimeClientEvent::InteractionAuditRequested {
                        audit: Box::new(audit_view),
                        transcript_cursor,
                    });
                }
                events
            }
            ConversationObservation::InteractionSettled {
                interaction,
                outcome,
                audit,
            } => {
                self.snapshot
                    .pending_interactions
                    .retain(|pending| pending.interaction != interaction);
                let mut events = vec![RuntimeClientEvent::InteractionSettled {
                    interaction,
                    outcome,
                }];
                if let Some(audit) = audit {
                    let (audit, transcript_cursor) = audit;
                    let audit_view = super::snapshot::interaction_settled_view(audit)
                        .expect("runtime interaction settled audit is valid");
                    let transcript_cursor = RuntimeClientTranscriptCursor::from(transcript_cursor);
                    events.push(RuntimeClientEvent::InteractionAuditSettled {
                        audit: Box::new(audit_view),
                        transcript_cursor,
                    });
                }
                events
            }
            ConversationObservation::InteractionRemoved { interaction } => {
                self.snapshot
                    .pending_interactions
                    .retain(|pending| pending.interaction != interaction);
                vec![RuntimeClientEvent::InteractionRemoved { interaction }]
            }
            ConversationObservation::Background(snapshot) => {
                let view = background_view(&snapshot);
                upsert_background(&mut self.snapshot.background, view.clone());
                vec![RuntimeClientEvent::BackgroundExecutionUpdated { execution: view }]
            }
            ConversationObservation::ToolProgress {
                attempt_id,
                tool_call_id,
                tool_id,
                progress,
            } => {
                // Live-only, latest-value (Issue #178): the report is folded
                // directly into the running foreground slot owned by this
                // projection. It is never written to the Event Journal. A
                // read-only attachment therefore sees the same disposable
                // state as the child runtime's ordinary Runtime Client
                // projection, while durable bootstrap still reconstructs
                // only committed progress facts.
                if let Some(attempt) = &mut self.snapshot.attempt
                    && attempt.attempt_id == attempt_id
                    && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, &tool_call_id)
                    && let ForegroundToolState::Running { arguments, .. } = &slot.state
                {
                    slot.state = ForegroundToolState::Running {
                        arguments: arguments.clone(),
                        progress: Some(progress.clone()),
                    };
                }
                vec![RuntimeClientEvent::ToolExecutionProgress {
                    attempt_id,
                    tool_call_id,
                    tool_id,
                    execution_id: None,
                    progress,
                }]
            }
            ConversationObservation::SubagentLifecycle(snapshot) => {
                // Both subagent delivery classes fold identically: the
                // whole-view upsert is unconditional last-write-wins, and
                // the queue's lane rules (a lifecycle push evicts queued
                // activity of the same subagent) already guarantee no fold
                // ever observes an activity snapshot older than the
                // lifecycle snapshot it already folded.
                let view = subagent_view(&snapshot);
                upsert_subagent(&mut self.snapshot.subagents, view.clone());
                let mut events = vec![RuntimeClientEvent::SubagentUpdated {
                    subagent: Box::new(view),
                }];
                if snapshot.state.is_terminal() {
                    let removed: Vec<_> = self
                        .snapshot
                        .pending_interactions
                        .iter()
                        .filter(|pending| {
                            pending.interaction.conversation_id == snapshot.child_conversation_id
                        })
                        .map(|pending| pending.interaction.clone())
                        .collect();
                    self.snapshot.pending_interactions.retain(|pending| {
                        pending.interaction.conversation_id != snapshot.child_conversation_id
                    });
                    events.extend(
                        removed.into_iter().map(|interaction| {
                            RuntimeClientEvent::InteractionRemoved { interaction }
                        }),
                    );
                }
                events
            }
            ConversationObservation::SubagentWorkspace(snapshot) => {
                // A retained-workspace disposal is a reliable resource
                // projection update. It must not repeat the terminal
                // lifecycle branch or remove pending child interactions as a
                // second logical terminal transition.
                let view = subagent_view(&snapshot);
                upsert_subagent(&mut self.snapshot.subagents, view.clone());
                vec![RuntimeClientEvent::SubagentUpdated {
                    subagent: Box::new(view),
                }]
            }
            ConversationObservation::SubagentActivity(snapshot) => {
                let view = subagent_view(&snapshot);
                upsert_subagent(&mut self.snapshot.subagents, view.clone());
                vec![RuntimeClientEvent::SubagentUpdated {
                    subagent: Box::new(view),
                }]
            }
            ConversationObservation::Capability {
                snapshot,
                availability,
            } => {
                // The projection owns the translation of the authoritative
                // capability snapshot and availability into the client
                // capability view. One event covers both an executable
                // revision swap and an availability-only change; the view's
                // revision discriminates them.
                let capabilities = capability_view(&snapshot, &availability);
                self.snapshot.capabilities = capabilities.clone();
                vec![RuntimeClientEvent::CapabilityUpdated { capabilities }]
            }
            ConversationObservation::Resources {
                snapshot,
                availability,
            } => {
                // One reload, one fold, one event. The runtime publishes the
                // resource generation and the capability generation it was
                // built against as a single observation precisely so both
                // views move together: this arm updates the snapshot
                // completely before returning, so no `snapshot()` call can
                // land between the two halves and read a pairing that never
                // existed.
                //
                // The published event carries both halves for the same
                // reason. Two events would be two cursors, and a client
                // that maintains its projection from the event stream would
                // sit at the first one holding the new capability
                // generation beside the retired resource generation.
                // Adjacent is not atomic; one event is.
                let capabilities = capability_view(snapshot.capability(), &availability);
                let resources = resources_view(&snapshot);
                self.snapshot.capabilities = capabilities.clone();
                self.snapshot.resources = resources.clone();
                vec![RuntimeClientEvent::ResourceGenerationUpdated {
                    capabilities,
                    resources,
                }]
            }
            ConversationObservation::AttemptAdmitted { attempt_id } => {
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
            ConversationObservation::AttemptModelFrozen { attempt_id, model } => {
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
            ConversationObservation::SessionModelChanged { model } => {
                self.snapshot.model = (*model).clone();
                vec![RuntimeClientEvent::SessionModelChanged { model }]
            }
            ConversationObservation::ApprovalModeChanged {
                effective,
                pending,
                revision,
            } => {
                self.snapshot.effective_approval_mode = effective;
                self.snapshot.pending_approval_mode = pending;
                self.snapshot.approval_mode_revision = revision;
                vec![RuntimeClientEvent::ApprovalModeChanged {
                    effective_approval_mode: effective,
                    pending_approval_mode: pending,
                    revision,
                }]
            }
            ConversationObservation::Shutdown => {
                self.snapshot.shutting_down = true;
                vec![RuntimeClientEvent::RuntimeShutdown]
            }
            ConversationObservation::DurableFailure { .. } => {
                // A transient durable-authority failure is recorded and
                // re-kicked; it does not yet change the externally visible
                // runtime health.
                Vec::new()
            }
            ConversationObservation::DurabilityFailed {
                operation,
                diagnostic,
            } => {
                self.snapshot.durability_failure =
                    Some(super::snapshot::RuntimeDurabilityFailure {
                        operation: operation.clone(),
                        diagnostic: diagnostic.clone(),
                    });
                vec![RuntimeClientEvent::RuntimeDurabilityFailed {
                    operation,
                    diagnostic,
                }]
            }
        }
    }

    /// Folds one durably committed publication frame into the in-flight read
    /// model and publishes its client event.
    ///
    /// The tool-call frames are model **proposals**: they update the
    /// in-flight assembly view, and the `Assembled` foreground state records
    /// exactly that — a call the model proposed, not one the Tool Plane
    /// started. Only `ToolExecutionStarted` moves a slot to `Running`.
    fn fold_publication_frame(
        &mut self,
        attempt_id: &AttemptId,
        frame: &PublicationFrame,
    ) -> Vec<RuntimeClientEvent> {
        let message_id = frame.message_id.clone();
        match &frame.payload {
            PublicationPayload::TextSuffix {
                block_index,
                suffix,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, suffix, TextKind::Text);
                vec![RuntimeClientEvent::AssistantTextDelta {
                    attempt_id: attempt_id.clone(),
                    message_id,
                    block_index: *block_index,
                    delta: suffix.clone(),
                }]
            }
            PublicationPayload::ReasoningSuffix {
                block_index,
                suffix,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, suffix, TextKind::Reasoning);
                vec![RuntimeClientEvent::AssistantReasoningDelta {
                    attempt_id: attempt_id.clone(),
                    message_id,
                    block_index: *block_index,
                    delta: suffix.clone(),
                }]
            }
            PublicationPayload::RefusalSuffix {
                block_index,
                suffix,
            } => {
                self.ensure_attempt(attempt_id);
                self.append_text(*block_index, suffix, TextKind::Refusal);
                vec![RuntimeClientEvent::AssistantRefusalDelta {
                    attempt_id: attempt_id.clone(),
                    message_id,
                    block_index: *block_index,
                    delta: suffix.clone(),
                }]
            }
            PublicationPayload::ProposedToolCallStarted { block_index, call } => {
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
                    message_id,
                    block_index: *block_index,
                    call: call.clone(),
                }]
            }
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index,
                call_id,
                suffix,
            } => {
                self.append_arguments(call_id, suffix);
                vec![RuntimeClientEvent::ToolCallArgumentsDelta {
                    attempt_id: attempt_id.clone(),
                    message_id,
                    block_index: *block_index,
                    call_id: call_id.clone(),
                    arguments_delta: suffix.clone(),
                }]
            }
            PublicationPayload::ProposedToolCallCompleted { block_index, call } => {
                self.set_assembled(call);
                vec![RuntimeClientEvent::ToolCallAssembled {
                    attempt_id: attempt_id.clone(),
                    message_id,
                    block_index: *block_index,
                    call: call.clone(),
                }]
            }
            // The terminal-only frame carries the publication terminal
            // transition and no visible payload, so it publishes nothing.
            PublicationPayload::TerminalOnly => Vec::new(),
        }
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
    /// - INTERNAL: model request mechanics (`ModelRequestStarted`,
    ///   `ModelRequestFailed`, `ModelRetryScheduled`);
    /// - PROJECT: compaction start/failure and committed completion, carrying
    ///   attempt attribution when automatic and no attempt identity when
    ///   manual.
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
            | RuntimeEvent::ModelRetryScheduled { .. }
            | RuntimeEvent::AgentStatusEmitted { .. } => Vec::new(),
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
                    && attempt.attempt_id == *attempt_id
                    && let Some(slot) = foreground_slot_mut(&mut attempt.foreground, tool_call_id)
                    && !matches!(&slot.state, ForegroundToolState::Settled { .. })
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
                if self.settle_foreground(attempt_id, tool_call_id, result.clone())
                    == ForegroundSettlement::AlreadySettled
                {
                    return Vec::new();
                }
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
                    managed_output: None,
                };
                if self.settle_foreground(attempt_id, tool_call_id, result.clone())
                    == ForegroundSettlement::AlreadySettled
                {
                    return Vec::new();
                }
                vec![RuntimeClientEvent::ToolExecutionSettled {
                    attempt_id: attempt_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    result,
                }]
            }
            RuntimeEvent::CompactionStarted
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
            | RuntimeEvent::SubagentOwnershipCommitted { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::SubagentWorkspaceDisposalStarted { .. }
            | RuntimeEvent::SubagentWorkspaceDisposalSettled { .. } => Vec::new(),
            // The interaction requested/settled facts are durable audit
            // evidence (Issue #109). The client already learns the live
            // pending/settled transitions from the coordinator's own
            // observation seam, so replaying the Journal fact here would
            // publish the same prompt twice under two different vocabularies.
            RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. } => {
                Vec::new()
            }
            // Workflow lifecycle/join facts are best-effort observability;
            // the successful child value and native terminal pair are the
            // separate durable authority. The Runtime Client continues to
            // project the bounded parent Tool call/result and does not expose
            // workflow-local values or child transcripts as a second
            // conversation surface.
            RuntimeEvent::WorkflowStarted { .. }
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

    /// Folds the shared automatic/manual compaction lifecycle. Manual
    /// maintenance carries no attempt identity; both paths update the exact
    /// same context read model and publish the same client vocabulary.
    fn fold_compaction_event(
        &mut self,
        attempt_id: Option<&AttemptId>,
        event: &RuntimeEvent,
    ) -> Vec<RuntimeClientEvent> {
        match event {
            RuntimeEvent::CompactionStarted => {
                self.snapshot.context.compaction_in_progress = true;
                vec![RuntimeClientEvent::ContextCompactionStarted {
                    attempt_id: attempt_id.cloned(),
                }]
            }
            RuntimeEvent::CompactionFailed { error } => {
                self.snapshot.context.compaction_in_progress = false;
                vec![RuntimeClientEvent::ContextCompactionFailed {
                    attempt_id: attempt_id.cloned(),
                    error: error.clone(),
                }]
            }
            RuntimeEvent::CompactionCompleted {
                generation,
                summary_message_id,
                surface_revision,
                tokens_before,
                estimated_tokens_after,
            } => {
                self.snapshot.context.compaction_in_progress = false;
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
                    attempt_id: attempt_id.cloned(),
                    context: self.snapshot.context.clone(),
                }]
            }
            _ => unreachable!("only compaction events enter the compaction fold"),
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
            self.snapshot.context.compaction_in_progress = false;
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
    fn settle_foreground(
        &mut self,
        attempt_id: &AttemptId,
        call_id: &ToolCallId,
        result: ToolExecutionResult,
    ) -> ForegroundSettlement {
        let Some(attempt) = self
            .snapshot
            .attempt
            .as_mut()
            .filter(|attempt| attempt.attempt_id == *attempt_id)
        else {
            return ForegroundSettlement::Missing;
        };
        let Some(slot) = foreground_slot_mut(&mut attempt.foreground, call_id) else {
            return ForegroundSettlement::Missing;
        };
        if matches!(&slot.state, ForegroundToolState::Settled { .. }) {
            return ForegroundSettlement::AlreadySettled;
        }
        let arguments = arguments_of(&slot.state);
        slot.state = ForegroundToolState::Settled { arguments, result };
        ForegroundSettlement::Applied
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

    /// Replaces only the derived durable transcript page with a fresh
    /// authoritative read. The Surface projection and Runtime Client cursor
    /// remain untouched: transcript paging is a separate durable cursor
    /// domain and is not a live observation event.
    pub(crate) fn set_transcript_page(
        &mut self,
        page: super::snapshot::RuntimeClientTranscriptPage,
    ) {
        self.snapshot.transcript = page;
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

/// One pending inbound item of the diagnostics view.
fn inbound_item_view(item: &InboundItem) -> InboundItemView {
    InboundItemView {
        sequence: item.sequence(),
        message: item.message().clone(),
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
/// authoritative capability snapshot and the coordinator-owned
/// availability state (Issue #81).
///
/// The tool catalog preserves registry order; the Skill catalog is
/// ordered by Skill name (the two snapshot lists derive from one sorted
/// package order); the source list preserves the deterministic
/// source-identity order of the authoritative availability map. No
/// executors, environment paths, or dependency internals ever appear.
pub(crate) fn capability_view(
    snapshot: &crate::capabilities::CapabilitySnapshot,
    availability: &crate::capabilities::CapabilityAvailability,
) -> CapabilityView {
    let project_tool =
        |definition: &crate::tools::types::ToolDefinition| super::snapshot::RuntimeClientTool {
            id: definition.id.clone(),
            name: definition.name.clone(),
            description: definition.description.clone(),
            input_schema: definition.input_schema.clone(),
            execution_policy: definition.execution_policy,
            concurrency_policy: definition.concurrency_policy,
            approval_policy: definition.approval_policy,
            replay_policy: definition.replay_policy,
            origin: definition.origin.clone(),
        };
    let tools = snapshot
        .tool_registry()
        .definitions()
        .iter()
        .map(&project_tool)
        .collect();
    let available_tools = snapshot
        .available_tools()
        .definitions()
        .iter()
        .map(project_tool)
        .collect();
    let skills = snapshot
        .skills()
        .catalog_entries()
        .iter()
        .zip(snapshot.skills().visible_bindings())
        .map(|(entry, binding)| super::snapshot::RuntimeClientSkill {
            id: binding.skill_id.clone(),
            version_id: binding.version_id.clone(),
            name: entry.name.clone(),
            description: entry.description.clone(),
            location: entry.location.clone(),
        })
        .collect();
    let sources = availability
        .iter()
        .map(|(source, state)| super::snapshot::CapabilitySourceView {
            source: match source {
                crate::capabilities::CapabilitySourceId::Mcp(server_id) => {
                    super::snapshot::CapabilitySourceDescriptor::Mcp {
                        server_id: server_id.clone(),
                    }
                }
            },
            state: match state {
                crate::capabilities::CapabilitySourceState::Ready => {
                    super::snapshot::CapabilitySourceStateView::Ready
                }
                crate::capabilities::CapabilitySourceState::Unavailable { reason } => {
                    super::snapshot::CapabilitySourceStateView::Unavailable {
                        reason: reason.clone(),
                    }
                }
            },
        })
        .collect();
    CapabilityView {
        revision: snapshot.revision(),
        tools,
        available_tools,
        skills,
        sources,
    }
}

/// Builds the client-visible resource projection from one immutable
/// runtime resource generation.
///
/// Identity and provenance only: the path the runtime read and the exact
/// byte length it loaded. The content stays where it already is — on disk,
/// and inside the runtime's own Effective System Prompt assembly — because
/// a conversation projection is not a second copy of request input.
pub(crate) fn resources_view(
    resources: &crate::runtime::resources::RuntimeResourceSnapshot,
) -> super::snapshot::RuntimeClientResourcesView {
    super::snapshot::RuntimeClientResourcesView {
        revision: resources.revision(),
        context_files: resources
            .project_context_files()
            .iter()
            .map(|file| super::snapshot::RuntimeClientContextFile {
                path: file.path.display().to_string(),
                bytes: file.content.len() as u64,
            })
            .collect(),
        agent_profile: resources.agent_profile().is_some(),
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

/// Projects one authoritative subagent registry snapshot into the external
/// Runtime Client shape (Issue #60).
pub(crate) fn subagent_view(
    snapshot: &crate::runtime::subagent::SubagentSnapshot,
) -> super::snapshot::RuntimeClientSubagent {
    super::snapshot::RuntimeClientSubagent {
        subagent_id: snapshot.subagent_id.clone(),
        child_agent_id: snapshot.child_agent_id.clone(),
        child_conversation_id: snapshot.child_conversation_id.clone(),
        agent: snapshot.agent.clone(),
        definition_digest: snapshot.definition_digest.clone(),
        state: snapshot.state,
        detail: snapshot.detail.clone(),
        observation: snapshot.observation.clone(),
        execution_profile: snapshot.profile.clone(),
        started_at: snapshot.started_at,
        workspace: super::snapshot::RuntimeClientSubagentWorkspace {
            logical_workspace: snapshot.workspace.logical_workspace.clone(),
            isolation: match &snapshot.workspace.isolation {
                crate::runtime::subagent::WorkspaceIsolation::Shared => {
                    super::snapshot::RuntimeClientWorkspaceIsolation::Shared
                }
                crate::runtime::subagent::WorkspaceIsolation::GitWorktree(worktree) => {
                    super::snapshot::RuntimeClientWorkspaceIsolation::GitWorktree {
                        source_repository_root: worktree.source_repository_root.clone(),
                        repository_relative_workspace: worktree
                            .repository_relative_workspace
                            .clone(),
                        physical_worktree_root: worktree.physical_worktree_root.clone(),
                        base_commit: worktree.base_commit.clone(),
                        branch: worktree.branch.clone(),
                        parent_had_uncommitted_changes: worktree.parent_had_uncommitted_changes,
                    }
                }
            },
            resource_state: snapshot.workspace_resource_state,
            handoff: snapshot.handoff.as_ref().map(|handoff| {
                super::snapshot::RuntimeClientWorkspaceHandoff {
                    logical_workspace: handoff.logical_workspace.clone(),
                    physical_worktree_root: handoff.physical_worktree_root.clone(),
                    branch: handoff.branch.clone(),
                    base_commit: handoff.base_commit.clone(),
                    head_commit: handoff.head_commit.clone(),
                    dirty: handoff.dirty,
                }
            }),
        },
    }
}

fn upsert_subagent(
    subagents: &mut Vec<super::snapshot::RuntimeClientSubagent>,
    view: super::snapshot::RuntimeClientSubagent,
) {
    if let Some(existing) = subagents
        .iter_mut()
        .find(|entry| entry.subagent_id == view.subagent_id)
    {
        *existing = view;
    } else {
        subagents.push(view);
    }
}

/// Inserts or replaces one live native interaction projection, preserving
/// deterministic coordinator identity order.
fn upsert_interaction(
    interactions: &mut Vec<crate::runtime::interaction::RoutedInteraction>,
    request: crate::runtime::interaction::RoutedInteraction,
) {
    if let Some(existing) = interactions
        .iter_mut()
        .find(|existing| existing.interaction == request.interaction)
    {
        *existing = request;
    } else {
        interactions.push(request);
        interactions.sort_by(|left, right| left.interaction.cmp(&right.interaction));
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
            } => RuntimeClientStatusSection::Temporal {
                current_time: *current_time,
                timezone: *timezone,
            },
            AgentStatusSectionData::BackgroundExecution {
                executions,
                omitted_count,
            } => RuntimeClientStatusSection::BackgroundExecutions {
                executions: executions.iter().map(background_view).collect(),
                omitted_count: *omitted_count,
            },
            AgentStatusSectionData::Todo { presentation } => RuntimeClientStatusSection::Todo {
                current: presentation.current.as_ref().map(todo_status_task_view),
                tasks: presentation
                    .tasks
                    .iter()
                    .map(todo_status_task_view)
                    .collect(),
                active_count: presentation.active_count,
                blocked_count: presentation.blocked_count,
                completed_count: presentation.completed_count,
                deleted_count: presentation.deleted_count,
                omitted_count: presentation.omitted_count,
            },
        })
        .collect();
    let fresh_inbound = observation
        .opportunities
        .fresh_inbound
        .as_ref()
        .map(|fresh| FreshInboundStatusOpportunityView {
            target_message_id: fresh.target_message_id.clone(),
        });
    // The placement fact of a `PostToolBatch` composition is carried by the
    // observation, frozen by the Agent Loop at the tool batch's own commit.
    // It is copied, never derived.
    let post_tool_batch =
        observation
            .opportunities
            .post_tool_batch
            .map(|_| PostToolBatchStatusOpportunityView {
                transcript_anchor: observation
                    .post_tool_batch_anchor
                    .map(RuntimeClientTranscriptCursor::from),
            });
    AgentStatusView {
        attempt_id: observation.attempt_id.clone(),
        turn: observation.turn,
        status_message_id: observation.status_message_id.clone(),
        opportunities: AgentStatusOpportunityView {
            fresh_inbound,
            post_tool_batch,
        },
        rendered: render_agent_status(&observation.status),
        sections,
    }
}

/// Admits one composed status into the bounded projection window and returns
/// the composition that admission pushed out, when the window was full.
///
/// Identity is the composed status message: re-observing the same
/// composition is idempotent rather than a second historical fact, and an
/// admission that does nothing evicts nothing.
///
/// This is the **only** retention decision in the system. The returned
/// eviction is published with the composition as one transition, so a client
/// folding incrementally reproduces this window without ever knowing
/// [`AGENT_STATUS_WINDOW`]. Because the window is only ever grown by one
/// before it is trimmed, one admission can retire at most one composition,
/// and the transition is fully described by a single optional identity.
fn admit_status(statuses: &mut Vec<AgentStatusView>, view: AgentStatusView) -> Option<MessageId> {
    if statuses
        .iter()
        .any(|existing| existing.status_message_id == view.status_message_id)
    {
        return None;
    }
    statuses.push(view);
    if statuses.len() > AGENT_STATUS_WINDOW {
        return Some(statuses.remove(0).status_message_id);
    }
    None
}

fn todo_status_task_view(
    task: &crate::context::status::TodoStatusTask,
) -> RuntimeClientTodoStatusTask {
    RuntimeClientTodoStatusTask {
        id: task.id,
        subject: task.subject.clone(),
        active_form: task.active_form.clone(),
        status: task.status,
        blocked: task.blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeClientProjection, SubscriberPoll};
    use crate::agent::{
        AgentExecution, AgentExecutionObserver, AgentExecutionRequest, AgentStatusObservation,
    };
    use crate::context::{
        AgentStatus, AgentStatusEngine, AgentStatusOpportunitySet, CompactionBudgets,
        ContextEngine, ContextError, ContextErrorKind, ContextRuntime,
        FreshInboundStatusOpportunity, PostToolBatchStatusOpportunity,
    };
    use crate::conversation::ConversationState;
    use crate::events::interaction::{InteractionSettlement, InteractionSubject};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::ModelAdapter;
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::publication::{PublicationFrame, PublicationPayload};
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, EventId, InteractionId, MessageId, RequestId,
        ToolCallId, ToolId, TurnId,
    };
    use crate::runtime::inbound::{ConversationInboundMailbox, InitialTurnTrigger};
    use crate::runtime::interaction::{
        ApprovalDecision, InteractionKind, InteractionOutcome, InteractionRef, InteractionRequest,
        InteractionResponse, InteractionSource, RoutedInteraction,
    };
    use crate::runtime::observation::ConversationObservation;
    use crate::runtime::types::{CancellationReason, TokenMeasurement, TokenMeasurementSource};
    use crate::runtime_client::event::{RuntimeClientEvent, RuntimeClientOutcome};
    use crate::runtime_client::snapshot::{
        AGENT_STATUS_WINDOW, AgentStatusOpportunityView, AgentStatusView, ForegroundToolState,
        InFlightBlock, RuntimeClientAttemptPhase, RuntimeClientStatusSection,
        RuntimeClientTranscriptCursor,
    };
    use crate::runtime_client::types::RuntimeClientCursor;
    use crate::scripted_suites::support::context::{
        FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
    };
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep};
    use crate::tools::executor::ToolRegistry;
    use crate::tools::types::{
        ToolCall, ToolCallStart, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
        ToolProgress,
    };
    use std::sync::{Arc, Mutex};

    fn attempt() -> AttemptId {
        AttemptId::new("attempt-1")
    }

    fn interaction_audit(
        request: &InteractionRequest,
        event_id: &str,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(event_id),
            sequence: 1,
            conversation_id: request.conversation_id.clone(),
            attempt_id: Some(request.attempt_id.clone()),
            turn_id: Some(TurnId::new(request.turn.to_string())),
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("timestamp")
                .with_timezone(&chrono::Utc),
            event,
        }
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
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            model_view(),
            64,
        )
    }

    fn apply_event(projection: &mut RuntimeClientProjection, event: RuntimeEvent) {
        projection.apply(event_observation(event));
    }

    /// Opens the representative publication stream on a projection.
    fn apply_publication_open(projection: &mut RuntimeClientProjection) {
        projection.apply(ConversationObservation::PublicationOpened {
            attempt_id: attempt(),
            start: stream_start(),
        });
    }

    /// Releases one already-committed publication frame into a projection.
    fn apply_frame(
        projection: &mut RuntimeClientProjection,
        sequence: u64,
        payload: PublicationPayload,
    ) {
        projection.apply(ConversationObservation::Publication {
            attempt_id: attempt(),
            frame: frame(sequence, payload),
        });
    }

    fn text_frame(text: &str) -> PublicationPayload {
        PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: text.to_owned(),
        }
    }

    fn event_observation(event: RuntimeEvent) -> ConversationObservation {
        ConversationObservation::Event {
            attempt_id: attempt(),
            event,
        }
    }

    /// One composed status observation for the placement suites.
    ///
    /// `post_tool_batch_anchor` is the placement fact the Agent Loop froze at
    /// the tool batch commit; it is supplied by the caller exactly as the
    /// loop supplies it, never derived from projection state.
    fn status_observation(
        status_message_id: &str,
        turn: u32,
        opportunities: AgentStatusOpportunitySet,
        post_tool_batch_anchor: Option<crate::durable::TranscriptCursor>,
    ) -> AgentStatusObservation {
        AgentStatusObservation {
            attempt_id: attempt(),
            turn,
            status_message_id: MessageId::new(status_message_id),
            opportunities,
            post_tool_batch_anchor,
            status: AgentStatus {
                generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                sections: Vec::new(),
            },
        }
    }

    /// One `FreshInbound`-only composition: placed by the exact inbound
    /// message identity, which needs no transcript position.
    fn fresh_inbound_status(
        status_message_id: &str,
        turn: u32,
        target: &str,
    ) -> AgentStatusObservation {
        status_observation(
            status_message_id,
            turn,
            AgentStatusOpportunitySet {
                fresh_inbound: Some(FreshInboundStatusOpportunity {
                    target_message_id: MessageId::new(target),
                }),
                post_tool_batch: None,
            },
            None,
        )
    }

    /// One `PostToolBatch`-only composition, carrying the durable position
    /// its settled tool batch froze.
    fn post_tool_batch_status(
        status_message_id: &str,
        turn: u32,
        batch_cursor: u64,
    ) -> AgentStatusObservation {
        status_observation(
            status_message_id,
            turn,
            AgentStatusOpportunitySet {
                fresh_inbound: None,
                post_tool_batch: Some(PostToolBatchStatusOpportunity),
            },
            Some(crate::durable::TranscriptCursor::new(batch_cursor)),
        )
    }

    /// The transcript-position placement one composition publishes, if any.
    fn published_anchor(view: &AgentStatusView) -> Option<RuntimeClientTranscriptCursor> {
        view.opportunities
            .post_tool_batch
            .and_then(|batch| batch.transcript_anchor)
    }

    /// Commits one visible canonical message at a durable transcript
    /// position.
    fn commit_at(projection: &mut RuntimeClientProjection, id: &str, cursor: u64) {
        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: MessageBlock::User(UserMessageBlock {
                id: MessageId::new(id),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "hello".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(cursor)),
        });
    }

    /// A `FreshInbound` composition is placed by the exact inbound message
    /// identity its opportunity carries, and publishes no transcript
    /// position: a message identity is the stronger fact, and nothing later
    /// can move it.
    #[test]
    fn fresh_inbound_status_is_placed_by_message_identity() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        commit_at(&mut projection, "user-1", 7);
        projection.apply(ConversationObservation::Status(fresh_inbound_status(
            "status-1", 1, "user-1",
        )));

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.statuses.len(), 1);
        assert_eq!(
            snapshot.statuses[0]
                .opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id,
            MessageId::new("user-1")
        );
        assert_eq!(
            published_anchor(&snapshot.statuses[0]),
            None,
            "a FreshInbound composition needs no transcript position"
        );

        // A later commit changes nothing about an already-composed status.
        commit_at(&mut projection, "user-2", 9);
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.statuses[0]
                .opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id,
            MessageId::new("user-1")
        );
    }

    /// A `PostToolBatch` composition publishes exactly the placement the
    /// Agent Loop froze at its tool batch commit — **not** whatever durable
    /// position the conversation happens to have reached when the projection
    /// folds the observation.
    ///
    /// This is the invariant that a fold-time frontier violated. Inbound
    /// acceptance is an independent durable boundary and may legally commit
    /// between the composition and its observation; the interleaving below is
    /// exactly that case, folded in the order it can really occur.
    #[test]
    fn post_tool_batch_status_keeps_the_anchor_its_batch_froze() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        commit_at(&mut projection, "user-1", 3);
        commit_at(&mut projection, "tool-result", 5);

        // Unrelated durable transcript activity lands before the status
        // observation reaches the projection.
        commit_at(&mut projection, "unrelated-inbound", 6);

        projection.apply(ConversationObservation::Status(post_tool_batch_status(
            "status-post-batch",
            2,
            5,
        )));

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let status = snapshot.statuses.last().expect("composed status");
        assert!(status.opportunities.fresh_inbound.is_none());
        assert_eq!(
            published_anchor(status),
            Some(RuntimeClientTranscriptCursor::new(5)),
            "the anchor is the tool batch's own frozen position, not the newest durable one"
        );
    }

    /// A new attempt is not a retraction: an earlier composition stays in the
    /// window, at its own placement, and a later one is added beside it.
    #[test]
    fn a_later_attempt_neither_removes_nor_relocates_an_earlier_status() {
        let mut projection = projection();
        commit_at(&mut projection, "user-1", 1);
        projection.apply(ConversationObservation::Status(fresh_inbound_status(
            "status-1", 1, "user-1",
        )));
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        commit_at(&mut projection, "tool-result", 4);
        projection.apply(ConversationObservation::Status(post_tool_batch_status(
            "status-2", 1, 4,
        )));

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot
                .statuses
                .iter()
                .map(|status| status.status_message_id.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["status-1".to_owned(), "status-2".to_owned()]
        );
        assert_eq!(
            snapshot.statuses[0]
                .opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id,
            MessageId::new("user-1")
        );
        assert_eq!(
            published_anchor(&snapshot.statuses[1]),
            Some(RuntimeClientTranscriptCursor::new(4))
        );
    }

    /// Identity is the composed status message: observing one composition
    /// twice describes one historical fact, evicts nothing, and the window
    /// stays bounded.
    #[test]
    fn the_agent_status_window_is_identity_keyed_and_bounded() {
        let mut projection = projection();
        let mut client = LiveStatusClient::attach(&mut projection);
        let observation = post_tool_batch_status("status-1", 1, 1);
        projection.apply(ConversationObservation::Status(observation.clone()));
        projection.apply(ConversationObservation::Status(observation));
        client.drain(&mut projection);
        assert_eq!(
            client.evictions,
            vec![None, None],
            "neither the first admission nor its replay evicts anything"
        );
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.statuses.len(), 1);

        for index in 0..AGENT_STATUS_WINDOW + 5 {
            projection.apply(ConversationObservation::Status(post_tool_batch_status(
                &format!("status-fill-{index}"),
                1,
                u64::try_from(index).expect("test index fits a cursor") + 2,
            )));
        }
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.statuses.len(), AGENT_STATUS_WINDOW);
        assert_eq!(
            snapshot.statuses.last().expect("newest").status_message_id,
            MessageId::new(format!("status-fill-{}", AGENT_STATUS_WINDOW + 4))
        );
    }

    /// A continuously subscribed client that folds status-window
    /// transitions exactly as the TUI reducer does.
    ///
    /// It is deliberately dumb: it removes the identity the runtime says its
    /// own admission evicted, then appends by identity. It does not know
    /// `AGENT_STATUS_WINDOW`, does not count entries, and makes no retention
    /// decision of its own.
    struct LiveStatusClient {
        subscriber: u64,
        statuses: Vec<AgentStatusView>,
        evictions: Vec<Option<MessageId>>,
    }

    impl LiveStatusClient {
        fn attach(projection: &mut RuntimeClientProjection) -> Self {
            let (subscriber, _notify) = projection
                .subscribe(RuntimeClientCursor::new(0))
                .expect("serviceable cursor");
            Self {
                subscriber,
                statuses: Vec::new(),
                evictions: Vec::new(),
            }
        }

        /// Drains everything published so far and folds it.
        fn drain(&mut self, projection: &mut RuntimeClientProjection) {
            loop {
                match projection.poll_subscriber(self.subscriber) {
                    SubscriberPoll::Event(event) => {
                        let RuntimeClientEvent::AgentStatusComposed {
                            status,
                            evicted_status_message_id,
                            ..
                        } = event.event
                        else {
                            continue;
                        };
                        self.evictions.push(evicted_status_message_id.clone());
                        if let Some(evicted) = evicted_status_message_id {
                            self.statuses
                                .retain(|existing| existing.status_message_id != evicted);
                        }
                        if !self
                            .statuses
                            .iter()
                            .any(|existing| existing.status_message_id == status.status_message_id)
                        {
                            self.statuses.push(status);
                        }
                    }
                    SubscriberPoll::Pending => return,
                    other => panic!("the status client lost its subscription: {other:?}"),
                }
            }
        }
    }

    /// **The bounded-window convergence invariant.**
    ///
    /// Folding every live event through cursor `C` and replacing state from
    /// the authoritative snapshot at cursor `C` must reconstruct the same
    /// semantic presentation state — past the retention bound as well as
    /// below it.
    ///
    /// The client here is deliberately dumb: it applies the eviction each
    /// event carries and appends by identity. It does not know
    /// `AGENT_STATUS_WINDOW`, does not count, and makes no retention
    /// decision, which is the entire point — with two retention owners this
    /// fold and the snapshot would diverge from the 65th distinct
    /// composition onwards.
    #[test]
    fn the_status_window_converges_between_live_fold_and_snapshot_repair() {
        let mut projection = projection();
        let mut live = LiveStatusClient::attach(&mut projection);
        let overshoot = 7;

        for index in 0..AGENT_STATUS_WINDOW + overshoot {
            let cursor = u64::try_from(index).expect("test index fits a cursor") + 1;
            projection.apply(ConversationObservation::Status(post_tool_batch_status(
                &format!("status-{index}"),
                1,
                cursor,
            )));
            live.drain(&mut projection);
        }
        let client = live.statuses;

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.statuses.len(),
            AGENT_STATUS_WINDOW,
            "the runtime window stays bounded"
        );
        assert_eq!(
            client, snapshot.statuses,
            "the live fold reconstructs the authoritative window exactly"
        );
        assert_eq!(
            client
                .iter()
                .map(|status| status.status_message_id.as_str().to_owned())
                .collect::<Vec<_>>(),
            (overshoot..AGENT_STATUS_WINDOW + overshoot)
                .map(|index| format!("status-{index}"))
                .collect::<Vec<_>>(),
            "retention and order are the runtime's, applied verbatim"
        );
        assert_eq!(
            client.last().map(|status| status.status_message_id.clone()),
            snapshot
                .statuses
                .last()
                .map(|status| status.status_message_id.clone()),
            "the newest composition agrees on both paths"
        );
        for (folded, authoritative) in client.iter().zip(&snapshot.statuses) {
            assert_eq!(
                published_anchor(folded),
                published_anchor(authoritative),
                "placement survives the fold unchanged"
            );
        }
    }

    /// Replay combined with eviction: re-observing a composition the window
    /// already holds must not displace another one.
    ///
    /// This is the failure a client-side retention rule invites — a second
    /// "append then trim" would drop the oldest entry on every replay, while
    /// the runtime's own window would not move at all.
    #[test]
    fn replaying_a_composition_evicts_nothing_from_a_full_window() {
        let mut projection = projection();
        let mut live = LiveStatusClient::attach(&mut projection);
        let mut observations = Vec::new();

        for index in 0..AGENT_STATUS_WINDOW + 3 {
            let cursor = u64::try_from(index).expect("test index fits a cursor") + 1;
            let observation = post_tool_batch_status(&format!("status-{index}"), 1, cursor);
            observations.push(observation.clone());
            projection.apply(ConversationObservation::Status(observation));
            live.drain(&mut projection);
        }
        let (before, _) = projection.snapshot().expect("snapshot");
        let evictions_before = live.evictions.len();

        // Replay the newest composition and one that is still retained.
        for observation in [
            observations.last().expect("newest").clone(),
            observations[AGENT_STATUS_WINDOW].clone(),
        ] {
            projection.apply(ConversationObservation::Status(observation));
            live.drain(&mut projection);
        }
        assert_eq!(
            live.evictions[evictions_before..],
            [None, None],
            "a replay is not an admission and evicts nothing"
        );

        let (after, _) = projection.snapshot().expect("snapshot");
        assert_eq!(after.statuses, before.statuses, "replay moves no window");
        assert_eq!(
            live.statuses, after.statuses,
            "the client fold agrees after replay"
        );
    }

    // -----------------------------------------------------------------------
    // The PostToolBatch anchor race.
    //
    // The Agent Loop's status linearization is:
    //
    // ```text
    // canonical ToolResult batch commits          <- placement determined here
    //   -> pending PostToolBatch opportunity
    //   -> next primary-step preparation
    //   -> Agent Status composed
    //   -> durable model-turn-start commit
    //   -> observe_status                          <- placement observed here
    // ```
    //
    // Inbound acceptance is an independent durable boundary and may commit
    // anywhere in that window. The test below parks the loop at
    // `observe_status`, durably accepts an unrelated inbound message while it
    // is parked, folds that acceptance into the Runtime Client projection, and
    // only then releases the status. Anything that derived placement at the
    // fold would produce the inbound's cursor; the frozen fact does not.
    // -----------------------------------------------------------------------

    /// An observer that folds every execution fact into a Runtime Client
    /// projection, but parks the first `PostToolBatch` status observation
    /// before it reaches the projection.
    ///
    /// The park is a real rendezvous: `observe_status` is a synchronous
    /// Agent Loop callback, so blocking in it blocks the loop itself. The
    /// controlling task therefore provably runs *between* the composition and
    /// its publication — not merely at some convenient moment.
    struct StatusParkObserver {
        projection: Arc<Mutex<RuntimeClientProjection>>,
        /// Every observed canonical commit with a durable position.
        commits: Mutex<Vec<(MessageId, crate::durable::TranscriptCursor)>>,
        /// Set when the loop is parked inside `observe_status`.
        parked: tokio::sync::watch::Sender<bool>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        released: Mutex<bool>,
    }

    impl StatusParkObserver {
        fn install(
            projection: Arc<Mutex<RuntimeClientProjection>>,
        ) -> (
            Arc<Self>,
            tokio::sync::watch::Receiver<bool>,
            std::sync::mpsc::Sender<()>,
        ) {
            let (parked, parked_rx) = tokio::sync::watch::channel(false);
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            (
                Arc::new(Self {
                    projection,
                    commits: Mutex::new(Vec::new()),
                    parked,
                    release: Mutex::new(release_rx),
                    released: Mutex::new(false),
                }),
                parked_rx,
                release_tx,
            )
        }

        fn apply(&self, observation: ConversationObservation) {
            self.projection
                .lock()
                .expect("projection lock")
                .apply(observation);
        }

        /// The durable position of the last committed tool message: the
        /// canonical `ToolResult` batch's own transcript frontier.
        fn tool_batch_cursor(&self) -> crate::durable::TranscriptCursor {
            *self
                .commits
                .lock()
                .expect("commit lock")
                .iter()
                .filter(|(id, _)| id.as_str().contains("-tool-"))
                .map(|(_, cursor)| cursor)
                .max()
                .expect("the attempt committed a canonical ToolResult batch")
        }
    }

    impl AgentExecutionObserver for StatusParkObserver {
        fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent) {
            self.apply(ConversationObservation::Event {
                attempt_id: attempt_id.clone(),
                event: event.clone(),
            });
        }

        fn observe_committed(
            &self,
            attempt_id: &AttemptId,
            block: &MessageBlock,
            transcript_cursor: Option<crate::durable::TranscriptCursor>,
        ) {
            if let Some(cursor) = transcript_cursor {
                self.commits
                    .lock()
                    .expect("commit lock")
                    .push((crate::conversation::message_id_of(block), cursor));
            }
            self.apply(ConversationObservation::Committed {
                attempt_id: Some(attempt_id.clone()),
                block: block.clone(),
                transcript_cursor,
            });
        }

        fn observe_status(&self, observation: &AgentStatusObservation) {
            let park = observation.opportunities.post_tool_batch.is_some()
                && observation.opportunities.fresh_inbound.is_none()
                && !*self.released.lock().expect("release flag lock");
            if park {
                self.parked.send_replace(true);
                // Blocks the Agent Loop here, before the Runtime Client can
                // fold anything about this composition.
                let _ = self.release.lock().expect("release lock").recv();
                *self.released.lock().expect("release flag lock") = true;
            }
            self.apply(ConversationObservation::Status(observation.clone()));
        }

        fn observe_publication_opened(
            &self,
            attempt_id: &AttemptId,
            start: &crate::publication::PublicationStreamStart,
        ) {
            self.apply(ConversationObservation::PublicationOpened {
                attempt_id: attempt_id.clone(),
                start: start.clone(),
            });
        }

        fn observe_publication(
            &self,
            attempt_id: &AttemptId,
            frame: &crate::publication::PublicationFrame,
        ) {
            self.apply(ConversationObservation::Publication {
                attempt_id: attempt_id.clone(),
                frame: frame.clone(),
            });
        }

        fn observe_publication_settled(
            &self,
            attempt_id: &AttemptId,
            audit: &crate::publication::PublicationAudit,
            transcript_cursor: crate::durable::TranscriptCursor,
        ) {
            self.apply(ConversationObservation::PublicationSettled {
                attempt_id: attempt_id.clone(),
                audit: Box::new(audit.clone()),
                transcript_cursor,
            });
        }
    }

    /// The controlling half of the anchor race.
    ///
    /// Everything here runs strictly inside the park: the Agent Loop is
    /// blocked in `observe_status`, between the composition and its
    /// publication, for the whole body of this function. Returns the tool
    /// batch's own durable position and the later position the unrelated
    /// inbound was accepted at.
    async fn drive_anchor_race(
        observer: Arc<StatusParkObserver>,
        projection: Arc<Mutex<RuntimeClientProjection>>,
        mailbox: ConversationInboundMailbox,
        mut parked: tokio::sync::watch::Receiver<bool>,
        release: std::sync::mpsc::Sender<()>,
    ) -> (
        crate::durable::TranscriptCursor,
        crate::durable::TranscriptCursor,
    ) {
        parked
            .wait_for(|reached| *reached)
            .await
            .expect("the status park stays open");
        let batch_cursor = observer.tool_batch_cursor();

        // An independent durable boundary: a new inbound message is accepted,
        // and the durable transcript advances past the batch.
        mailbox
            .enqueue(UserMessageBlock {
                id: MessageId::new("unrelated-inbound"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "and another thing".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: Some(chrono::Utc::now()),
            })
            .expect("durable inbound acceptance");
        let batch = mailbox
            .select_pending_batch()
            .expect("select pending")
            .expect("the accepted inbound is pending");
        let item = batch
            .items()
            .iter()
            .find(|item| item.message().id == MessageId::new("unrelated-inbound"))
            .expect("the accepted inbound is in the pending batch")
            .clone();
        let inbound_cursor = item
            .transcript_cursor()
            .expect("a visible inbound message is a durable transcript item");
        assert!(
            inbound_cursor > batch_cursor,
            "the unrelated inbound is durably later than the tool batch"
        );

        // Fold the acceptance into the Runtime Client *before* the status.
        projection
            .lock()
            .expect("projection lock")
            .apply(ConversationObservation::InboundEnqueued(item));
        let (mid_flight, _) = projection
            .lock()
            .expect("projection lock")
            .snapshot()
            .expect("snapshot");
        assert!(
            mid_flight.statuses.is_empty(),
            "the parked composition has not been published yet"
        );
        assert_eq!(
            mid_flight
                .inbound
                .pending
                .last()
                .expect("the unrelated inbound is folded")
                .message
                .id,
            MessageId::new("unrelated-inbound"),
            "the projection has already folded the later durable position"
        );

        release.send(()).expect("release the parked status");
        (batch_cursor, inbound_cursor)
    }

    /// **The `PostToolBatch` placement invariant.**
    ///
    /// A `PostToolBatch` Agent Status is anchored to the durable transcript
    /// frontier of the tool batch that caused the composition. Later
    /// unrelated durable transcript activity cannot move that anchor.
    ///
    /// The interleaving is forced, not hoped for:
    ///
    /// ```text
    /// 1. canonical ToolResult batch commits at cursor N
    /// 2. the PostToolBatch opportunity freezes N
    /// 3. the loop PARKS in observe_status, before the projection sees it
    /// 4. an unrelated inbound is durably accepted at cursor N+1
    /// 5. its InboundEnqueued observation is folded
    /// 6. the park is RELEASED and the status is folded
    /// 7. the published anchor is still N
    /// ```
    ///
    /// Step 5 is asserted before step 6 runs, so the projection provably held
    /// the later position when it folded the composition. There are no
    /// sleeps and no timing assumptions anywhere: every ordering above is a
    /// rendezvous.
    /// The scripted provider of the anchor race: one turn that calls `todo`,
    /// then plain answers.
    ///
    /// The tool is `todo` on purpose. The Todo module is the one status module
    /// eligible for a `PostToolBatch` opportunity, so this batch both settles
    /// a canonical `ToolResult` batch and makes the next primary step compose
    /// a `PostToolBatch`-only status.
    fn anchor_race_model() -> Arc<FakeModel> {
        let call = crate::tools::types::ToolCall {
            id: ToolCallId::new("call-anchor-1"),
            tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
            name: "todo".to_owned(),
            arguments: serde_json::json!({ "action": "create", "subject": "Anchor the status" }),
        };
        let arguments_json =
            serde_json::to_string(&call.arguments).expect("serialize todo arguments");
        let stop_turn = || {
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "done".to_owned(),
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]
        };
        Arc::new(FakeModel::new(vec![
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::ToolCallStarted {
                    block_index: ContentBlockIndex::new(0),
                    call: crate::tools::types::ToolCallStart {
                        id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                    },
                }),
                FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                    block_index: ContentBlockIndex::new(0),
                    call_id: call.id.clone(),
                    arguments_delta: arguments_json,
                }),
                FakeStep::Emit(ModelEvent::ToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call,
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::ToolCalls,
                    usage: None,
                }),
            ],
            stop_turn(),
            stop_turn(),
        ]))
    }

    /// A context runtime with a real Agent Status engine and no compaction
    /// pressure, so the only status decision under test is placement.
    fn anchor_race_context_runtime() -> ContextRuntime {
        ContextRuntime::with_scripted_summarizer(
            ContextEngine::new(
                crate::context::ContextConfig {
                    context_window_tokens: 200_000,
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                },
                Arc::new(ScriptedEstimator::new(1, 0, 0)),
            )
            .expect("valid test context engine"),
            Arc::new(FakeContextSummarizer::new(Vec::new())),
            AgentStatusEngine::default(),
            CompactionBudgets::new(1, 1, 1_000_000),
        )
    }

    /// The attempt request of the anchor race: a pure continuation, so the
    /// only status opportunity this attempt can produce is `PostToolBatch`.
    fn anchor_race_request(
        conversation_id: ConversationId,
        model: Arc<FakeModel>,
    ) -> AgentExecutionRequest {
        let adapter: Arc<dyn ModelAdapter> = model;
        AgentExecutionRequest {
            agent_id: AgentId::new("agent-a"),
            conversation_id,
            attempt_id: AttemptId::new("attempt-anchor"),
            conversation: ConversationState::from_messages(vec![MessageBlock::User(
                UserMessageBlock {
                    id: MessageId::new("user-1"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "track the work".to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: InboundKind::Message,
                    timestamp: None,
                },
            )])
            .expect("bootstrap conversation"),
            initial_turn_trigger: InitialTurnTrigger::Continuation,
            model: crate::scripted_suites::support::attempt_model_with_window(
                adapter,
                "status-anchor",
                200_000,
                64,
            ),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn a_later_inbound_cannot_relocate_a_prepared_post_tool_batch_status() {
        let fixture = crate::scripted_suites::common::native_fixture();
        let capability = crate::scripted_suites::common::capability_lease(
            fixture.registry.clone(),
            &fixture.runtime,
        )
        .await;
        let request = anchor_race_request(
            fixture.runtime.conversation_id().clone(),
            anchor_race_model(),
        );
        let projection = Arc::new(Mutex::new(projection()));
        let (observer, parked, release) = StatusParkObserver::install(projection.clone());

        let controller = tokio::spawn(drive_anchor_race(
            observer.clone(),
            projection.clone(),
            fixture.mailbox.clone(),
            parked,
            release,
        ));

        let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
        let mut execution = AgentExecution::new(
            request,
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            anchor_race_context_runtime(),
            &fixture.runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.observe(observer.as_ref());
        let _result = execution.run().await;
        let (batch_cursor, inbound_cursor) = controller.await.expect("controller task");

        let (snapshot, _) = projection
            .lock()
            .expect("projection lock")
            .snapshot()
            .expect("snapshot");
        let status = snapshot
            .statuses
            .iter()
            .find(|status| status.opportunities.post_tool_batch.is_some())
            .expect("the attempt composed a PostToolBatch status");
        assert_eq!(
            published_anchor(status),
            Some(RuntimeClientTranscriptCursor::from(batch_cursor)),
            "the status keeps the position its own tool batch froze"
        );
        assert_ne!(
            published_anchor(status),
            Some(RuntimeClientTranscriptCursor::from(inbound_cursor)),
            "an unrelated inbound accepted before the fold cannot relocate the status"
        );
    }

    #[test]
    fn agent_status_opportunity_view_allows_absent_fresh_inbound() {
        let observation = AgentStatusObservation {
            attempt_id: attempt(),
            turn: 1,
            status_message_id: MessageId::new("status-1"),
            opportunities: AgentStatusOpportunitySet::default(),
            post_tool_batch_anchor: None,
            status: AgentStatus {
                generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                sections: Vec::new(),
            },
        };

        let view = super::status_view(&observation);
        assert!(view.opportunities.fresh_inbound.is_none());

        let encoded = serde_json::to_value(&view.opportunities).expect("serialize opportunity");
        assert_eq!(encoded, serde_json::json!({}));
        let decoded: AgentStatusOpportunityView =
            serde_json::from_value(encoded).expect("deserialize opportunity");
        assert_eq!(decoded, view.opportunities);
    }

    #[test]
    fn agent_status_opportunity_view_publishes_one_combined_observation() {
        let status_message_id = MessageId::new("status-combined");
        let observation = AgentStatusObservation {
            attempt_id: attempt(),
            turn: 2,
            status_message_id: status_message_id.clone(),
            opportunities: AgentStatusOpportunitySet {
                fresh_inbound: Some(FreshInboundStatusOpportunity {
                    target_message_id: MessageId::new("fresh-inbound"),
                }),
                post_tool_batch: Some(PostToolBatchStatusOpportunity),
            },
            post_tool_batch_anchor: Some(crate::durable::TranscriptCursor::new(6)),
            status: AgentStatus {
                generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
                sections: Vec::new(),
            },
        };

        let view = super::status_view(&observation);
        assert_eq!(view.status_message_id, status_message_id);
        assert_eq!(
            view.opportunities
                .fresh_inbound
                .as_ref()
                .expect("FreshInbound view")
                .target_message_id
                .clone(),
            MessageId::new("fresh-inbound")
        );
        assert_eq!(
            published_anchor(&view),
            Some(RuntimeClientTranscriptCursor::new(6)),
            "the PostToolBatch half carries the placement its batch froze"
        );
        let encoded = serde_json::to_value(&view).expect("serialize combined status view");
        assert_eq!(
            encoded["opportunities"]["post_tool_batch"],
            serde_json::json!({ "transcript_anchor": 6 })
        );
        assert!(
            encoded.get("transcript_anchor").is_none(),
            "placement lives on the opportunity that froze it, never on the view itself"
        );
        assert_eq!(
            encoded["status_message_id"],
            serde_json::json!("status-combined")
        );
    }

    #[test]
    fn todo_status_section_round_trips_the_wire_shape() {
        let wire = serde_json::json!({
            "type": "todo",
            "current": {
                "id": 7,
                "subject": "Review the boundary",
                "active_form": "Reviewing the boundary",
                "status": "in_progress",
                "blocked": false
            },
            "tasks": [{
                "id": 8,
                "subject": "Write the regression",
                "status": "pending",
                "blocked": true
            }],
            "active_count": 2,
            "blocked_count": 1,
            "completed_count": 3,
            "deleted_count": 1,
            "omitted_count": 0
        });
        let section: RuntimeClientStatusSection =
            serde_json::from_value(wire.clone()).expect("decode the Runtime Client Todo section");
        assert_eq!(
            serde_json::to_value(section).expect("encode the Runtime Client Todo section"),
            wire
        );
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
                    available_tools: Vec::new(),
                    skills: Vec::new(),
                    sources: Vec::new(),
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
            projection.apply(ConversationObservation::Event {
                attempt_id: attempt_id.clone(),
                event: event.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_committed(
            &self,
            attempt_id: &AttemptId,
            block: &MessageBlock,
            transcript_cursor: Option<crate::durable::TranscriptCursor>,
        ) {
            if matches!(
                block,
                MessageBlock::User(user) if user.kind.is_compaction_summary()
            ) {
                self.facts
                    .lock()
                    .expect("order facts lock")
                    .push(CompactionOrderFact::SummaryLedgerCommitted);
            }
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::Committed {
                attempt_id: Some(attempt_id.clone()),
                block: block.clone(),
                transcript_cursor,
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_status(&self, observation: &AgentStatusObservation) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::Status(observation.clone()));
            self.drain_client_events(&mut projection);
        }

        fn observe_publication_opened(
            &self,
            attempt_id: &AttemptId,
            start: &crate::publication::PublicationStreamStart,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::PublicationOpened {
                attempt_id: attempt_id.clone(),
                start: start.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_publication(
            &self,
            attempt_id: &AttemptId,
            frame: &crate::publication::PublicationFrame,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::Publication {
                attempt_id: attempt_id.clone(),
                frame: frame.clone(),
            });
            self.drain_client_events(&mut projection);
        }

        fn observe_publication_settled(
            &self,
            attempt_id: &AttemptId,
            audit: &crate::publication::PublicationAudit,
            transcript_cursor: crate::durable::TranscriptCursor,
        ) {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::PublicationSettled {
                attempt_id: attempt_id.clone(),
                audit: Box::new(audit.clone()),
                transcript_cursor,
            });
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
        crate::scripted_suites::common::DurableExecutionAudit,
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
            AgentStatusEngine::default(),
            CompactionBudgets::new(1, 1, 1_000_000),
        );
        let tool_runtime = crate::scripted_suites::common::tool_runtime("projection-order");
        let store = tool_runtime.durable_store();
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
            model: model_snapshot,
        };
        let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
        let observer = Arc::new(EventPathObserver::new(initial_messages, facts.clone()));
        let mut execution = AgentExecution::new(
            request,
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime");
        execution.observe(observer.as_ref());
        let result = execution.run().await;
        let result = crate::scripted_suites::common::durable_agent_result(result, store.as_ref());
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
            result.event_history.last(),
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
                .event_history
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
                        && user.kind.is_compaction_summary()
            )),
            "the committed runtime summary is an ordinary canonical ledger fact"
        );
    }

    /// A failed compaction publishes its live start/failure lifecycle and
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
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionStarted))
        );
        assert!(
            result
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
        );
        assert!(
            result
                .event_history
                .iter()
                .all(|event| { !matches!(event, RuntimeEvent::CompactionCompleted { .. }) })
        );
        assert!(matches!(
            result.event_history.last(),
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
                    if user.kind.is_compaction_summary())),
            "a failed compaction never commits a canonical summary"
        );
        let snapshot = observer.snapshot();
        assert!(!snapshot.context.compaction_in_progress);
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
                if attempt_id.as_ref() == Some(&attempt())
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
            managed_output: None,
        }
    }

    fn cancelled_result(
        reason: CancellationReason,
        phase: ToolCancellationPhase,
    ) -> ToolExecutionResult {
        ToolExecutionResult {
            status: ToolExecutionStatus::Cancelled { reason, phase },
            content: Vec::new(),
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        }
    }

    fn apply_assembled_call(projection: &mut RuntimeClientProjection, call: &ToolCall) {
        apply_frame(
            projection,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        );
        apply_frame(
            projection,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
            },
        );
        apply_frame(
            projection,
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
        );
    }

    fn stream_start() -> crate::publication::PublicationStreamStart {
        crate::publication::PublicationStreamStart {
            stream_id: crate::runtime::identity::PublicationStreamId::new("attempt-1-pub-1"),
            attempt_id: attempt(),
            turn_id: crate::runtime::identity::TurnId::new("1"),
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            message_id: MessageId::new("msg-1"),
        }
    }

    fn frame(sequence: u64, payload: PublicationPayload) -> PublicationFrame {
        PublicationFrame {
            stream_id: stream_start().stream_id,
            message_id: MessageId::new("msg-1"),
            sequence,
            payload,
        }
    }

    /// The representative attempt sequence: streaming publication, a tool
    /// call, execution, and terminal settlement.
    ///
    /// Streaming assembly arrives as durably committed publication frames
    /// (Issue #108); only the surrounding execution facts are Event Journal
    /// observations.
    fn representative_sequence() -> Vec<ConversationObservation> {
        let mut observations = vec![
            event_observation(RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            }),
            event_observation(RuntimeEvent::TurnStarted),
            event_observation(RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                model: "scripted".to_owned(),
            }),
            ConversationObservation::PublicationOpened {
                attempt_id: attempt(),
                start: stream_start(),
            },
        ];
        for (sequence, payload) in [
            PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "hello ".to_owned(),
            },
            PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(0),
                suffix: "world".to_owned(),
            },
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(1),
                call: ToolCallStart {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                },
            },
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(1),
                call_id: ToolCallId::new("call_1"),
                suffix: "{}".to_owned(),
            },
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(1),
                call: ToolCall {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            },
        ]
        .into_iter()
        .enumerate()
        {
            observations.push(ConversationObservation::Publication {
                attempt_id: attempt(),
                frame: frame(sequence as u64, payload),
            });
        }
        observations.extend([
            event_observation(RuntimeEvent::ModelRequestCompleted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
            event_observation(RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
            }),
            event_observation(RuntimeEvent::ToolExecutionProgress {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                execution_id: None,
                progress: ToolProgress {
                    message: Some("half way".to_owned()),
                    completed: Some(1.0),
                    total: Some(2.0),
                },
            }),
            event_observation(RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                result: success_result(),
            }),
            event_observation(RuntimeEvent::TurnCompleted),
            event_observation(RuntimeEvent::AttemptCompleted {
                attempt_id: attempt(),
                finish_reason: ModelFinishReason::Stop,
            }),
        ]);
        observations
    }

    /// The projection is deterministic: applying the same representative
    /// sequence twice produces identical event sequences and identical
    /// snapshots.
    #[test]
    fn representative_sequence_projects_deterministically() {
        let mut first = projection();
        for observation in representative_sequence() {
            first.apply(observation);
        }
        let mut second = projection();
        for observation in representative_sequence() {
            second.apply(observation);
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

    /// Model-request mechanics stay internal; compaction start/failure and
    /// committed completion are projected as runtime-owned lifecycle facts.
    #[test]
    fn model_request_events_stay_internal_and_compaction_lifecycle_is_projected() {
        let mut projection = projection();
        for event in [
            RuntimeEvent::ModelRequestStarted {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                model: "m".to_owned(),
            },
            RuntimeEvent::ModelRequestFailed {
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                error: ModelError {
                    kind: ModelErrorKind::RateLimit,
                    message: "retry".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                    retry_after_ms: Some(10),
                    provider_code: Some("rate_limit_exceeded".to_owned()),
                    context_overflow: None,
                    malformed_tool_proposal: None,
                },
                usage: None,
            },
            RuntimeEvent::ModelRetryScheduled {
                failed_request_id: RequestId::new("request:9:attempt-1:1:1:0"),
                retry_number: 1,
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
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0].event,
            RuntimeClientEvent::ContextCompactionStarted { .. }
        ));
        assert!(matches!(
            &events[1].event,
            RuntimeClientEvent::ContextCompacted { context, .. }
                if context.compaction_count == 1
        ));
        assert!(matches!(
            &events[2].event,
            RuntimeClientEvent::ContextCompactionFailed { error, .. } if error == "boom"
        ));
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(3));
        assert!(!snapshot.context.compaction_in_progress);
        assert_eq!(snapshot.context.compaction_count, 1);
        assert!(snapshot.attempt.is_none());
    }

    #[test]
    fn manual_compaction_projects_without_fabricating_an_attempt_identity() {
        let mut projection = projection();
        projection.apply(ConversationObservation::ManualCompactionEvent {
            event: RuntimeEvent::CompactionStarted,
        });
        projection.apply(ConversationObservation::ManualCompactionEvent {
            event: RuntimeEvent::CompactionCompleted {
                generation: 1,
                summary_message_id: MessageId::new("conv-1-summary-1"),
                surface_revision: crate::conversation::SurfaceRevision::new(2),
                tokens_before: TokenMeasurement {
                    input_tokens: 8_400,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 1_900,
            },
        });

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert!(matches!(
            &events[0].event,
            RuntimeClientEvent::ContextCompactionStarted { attempt_id: None }
        ));
        assert!(matches!(
            &events[1].event,
            RuntimeClientEvent::ContextCompacted {
                attempt_id: None,
                context,
            } if !context.compaction_in_progress && context.compaction_count == 1
        ));
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
                request_id: RequestId::new("request:9:attempt-1:1:1:0"),
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
        projection.apply(ConversationObservation::Shutdown);

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
                        retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                        retry_after_ms: Some(5_000),
                        provider_code: Some("rate_limit_exceeded".to_owned()),
                        context_overflow: None,
                        malformed_tool_proposal: None,
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
        apply_publication_open(&mut projection);
        apply_frame(&mut projection, 0, text_frame("hello "));
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
        apply_frame(&mut projection, 1, text_frame("world"));
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
        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: committed.clone(),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
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

    /// A durable commit cursor travels with the live observation even when
    /// observations arrive in the opposite order. The Runtime Client event
    /// cursor records that delivery order only; it is deliberately not the
    /// transcript order input.
    #[test]
    fn live_committed_observations_carry_durable_cursors_across_reordering() {
        let mut projection = projection();
        let message_a = compactable_user("message-a");
        let message_b = compactable_user("message-b");

        projection.apply(ConversationObservation::Committed {
            attempt_id: None,
            block: message_b,
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(11)),
        });
        projection.apply(ConversationObservation::Committed {
            attempt_id: None,
            block: message_a,
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(10)),
        });

        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(
            events
                .iter()
                .map(|event| event.cursor.get())
                .collect::<Vec<_>>(),
            vec![1, 2],
            "Runtime Client cursors follow observation delivery"
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match &event.event {
                    RuntimeClientEvent::MessageCommitted {
                        transcript_cursor, ..
                    } => transcript_cursor.map(RuntimeClientTranscriptCursor::get),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![11, 10],
            "each live fact carries its own durable transcript cursor"
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
        for (sequence, call) in [
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
        ]
        .into_iter()
        .enumerate()
        {
            apply_frame(
                &mut projection,
                sequence as u64,
                PublicationPayload::ProposedToolCallStarted {
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
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
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

    /// The Runtime Client projection carries the canonical cancellation phase
    /// verbatim and preserves it through its wire round trip.
    #[test]
    fn issue136_foreground_cancellation_phase_round_trips_through_projection() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_cancelled"),
            tool_id: ToolId::new("tool-cancelled"),
            name: "cancelled".to_owned(),
            arguments: serde_json::json!({"value": 1}),
        };
        apply_frame(
            &mut projection,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        );
        apply_frame(
            &mut projection,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
            },
        );
        apply_frame(
            &mut projection,
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        );
        let result = ToolExecutionResult {
            status: ToolExecutionStatus::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
                phase: ToolCancellationPhase::DuringExecution,
            },
            content: Vec::new(),
            duration_ms: 3,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        };
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        );

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
        let ForegroundToolState::Settled {
            result: projected, ..
        } = &foreground[0].state
        else {
            panic!("the cancelled call must be settled");
        };
        assert_eq!(projected, &result);
        let encoded = serde_json::to_value(&snapshot).expect("snapshot JSON");
        assert_eq!(
            encoded["attempt"]["foreground"][0]["state"]["result"]["status"]["phase"],
            "during_execution"
        );
        let decoded: crate::runtime_client::snapshot::RuntimeClientSnapshot =
            serde_json::from_value(encoded).expect("snapshot round trip");
        assert_eq!(decoded, snapshot);
    }

    /// The Runtime Client projection carries the canonical `OutcomeUnknown`
    /// result verbatim — bounded `detail` included — and preserves it through
    /// its wire round trip, never flattening it into a known `failed`.
    #[test]
    fn issue202_outcome_unknown_flows_through_projection_unchanged() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_outcome_unknown"),
            tool_id: ToolId::new("tool-outcome-unknown"),
            name: "outcome_unknown".to_owned(),
            arguments: serde_json::json!({"value": 1}),
        };
        apply_frame(
            &mut projection,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        );
        apply_frame(
            &mut projection,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
            },
        );
        apply_frame(
            &mut projection,
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: call.clone(),
            },
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        );
        let result = ToolExecutionResult {
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "executor lost contact after dispatch".to_owned(),
            },
            content: Vec::new(),
            duration_ms: 3,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        };
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        );

        let (snapshot, _) = projection.snapshot().expect("snapshot");
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
        let ForegroundToolState::Settled {
            result: projected, ..
        } = &foreground[0].state
        else {
            panic!("the outcome-unknown call must be settled");
        };
        assert_eq!(projected, &result);
        let encoded = serde_json::to_value(&snapshot).expect("snapshot JSON");
        let status = &encoded["attempt"]["foreground"][0]["state"]["result"]["status"];
        assert_eq!(status["type"], "outcome_unknown");
        assert_eq!(status["detail"], "executor lost contact after dispatch");
        let decoded: crate::runtime_client::snapshot::RuntimeClientSnapshot =
            serde_json::from_value(encoded).expect("snapshot round trip");
        assert_eq!(decoded, snapshot);
    }

    /// A canonical `ToolMessage` settles an accepted call that never emitted a
    /// live execution lifecycle event. The commit publishes exactly one
    /// equivalent client settlement and preserves it against late raw facts.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn issue136_before_start_commit_repairs_foreground_slot_once() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_before_start"),
            tool_id: ToolId::new("tool-before-start"),
            name: "before_start".to_owned(),
            arguments: serde_json::json!({"value": 1}),
        };
        apply_assembled_call(&mut projection, &call);
        let result = cancelled_result(
            CancellationReason::RuntimeShutdown,
            ToolCancellationPhase::BeforeStart,
        );
        let committed = MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("message-before-start"),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        });
        let (_, before_commit) = projection.snapshot().expect("snapshot before commit");

        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: committed.clone(),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
        });
        let commit_events = collect(&mut projection, before_commit);
        assert_eq!(commit_events.len(), 2);
        assert_eq!(
            commit_events
                .iter()
                .filter(|event| {
                    matches!(event.event, RuntimeClientEvent::ToolExecutionSettled { .. })
                })
                .count(),
            1,
            "the canonical commit emits one foreground settlement"
        );
        assert!(
            !commit_events.iter().any(|event| matches!(
                event.event,
                RuntimeClientEvent::ToolExecutionStarted { .. }
            ))
        );
        assert!(matches!(
            &commit_events[0].event,
            RuntimeClientEvent::ToolExecutionSettled {
                tool_call_id,
                result: projected,
                ..
            } if tool_call_id == &call.id && projected == &result
        ));
        assert!(matches!(
            &commit_events[1].event,
            RuntimeClientEvent::MessageCommitted {
                message,
                ..
            } if message == &committed
        ));

        let (snapshot, _) = projection.snapshot().expect("settled snapshot");
        let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
        assert_eq!(foreground.len(), 1);
        assert!(matches!(
            &foreground[0].state,
            ForegroundToolState::Settled {
                result: projected,
                ..
            } if projected == &result
        ));

        apply_event(
            &mut projection,
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt(),
                reason: CancellationReason::RuntimeShutdown,
            },
        );
        let (settled_snapshot, after_attempt) = projection.snapshot().expect("terminal snapshot");
        assert!(matches!(
            settled_snapshot
                .attempt
                .as_ref()
                .expect("attempt view")
                .phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: RuntimeClientOutcome::Cancelled {
                    reason: CancellationReason::RuntimeShutdown,
                }
            }
        ));
        assert!(matches!(
            &settled_snapshot.attempt.as_ref().expect("attempt view").foreground[0].state,
            ForegroundToolState::Settled { result: projected, .. } if projected == &result
        ));

        // A late physical result cannot reopen this slot or emit a second
        // client-visible settlement after the canonical commit won.
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: success_result(),
            },
        );
        assert!(collect(&mut projection, after_attempt).is_empty());
        let (final_snapshot, _) = projection.snapshot().expect("final snapshot");
        assert!(matches!(
            &final_snapshot.attempt.as_ref().expect("attempt view").foreground[0].state,
            ForegroundToolState::Settled { result: projected, .. } if projected == &result
        ));
    }

    /// A live `DuringExecution` settlement already closes the slot; its later
    /// canonical `ToolMessage` commit is history only and emits no duplicate
    /// foreground settlement.
    #[test]
    fn issue136_live_settlement_is_not_duplicated_by_tool_message_commit() {
        let mut projection = projection();
        apply_event(
            &mut projection,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        );
        let call = ToolCall {
            id: ToolCallId::new("call_during_execution"),
            tool_id: ToolId::new("tool-during-execution"),
            name: "during_execution".to_owned(),
            arguments: serde_json::json!({}),
        };
        apply_assembled_call(&mut projection, &call);
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
        );
        let result = cancelled_result(
            CancellationReason::ParentCancelled,
            ToolCancellationPhase::DuringExecution,
        );
        apply_event(
            &mut projection,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            },
        );
        let (_, before_commit) = projection.snapshot().expect("snapshot before commit");
        projection.apply(ConversationObservation::Committed {
            attempt_id: Some(attempt()),
            block: MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("message-during-execution"),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: result.clone(),
            }),
            transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
        });

        let commit_events = collect(&mut projection, before_commit);
        assert_eq!(commit_events.len(), 1);
        assert!(matches!(
            &commit_events[0].event,
            RuntimeClientEvent::MessageCommitted { .. }
        ));
        let (snapshot, _) = projection.snapshot().expect("snapshot after commit");
        assert!(matches!(
            &snapshot.attempt.as_ref().expect("attempt view").foreground[0].state,
            ForegroundToolState::Settled { result: projected, .. } if projected == &result
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
        for index in 0..10u64 {
            apply_frame(&mut projection, index, text_frame(&index.to_string()));
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
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            model_view(),
            4,
        );
        apply_publication_open(&mut projection);
        for index in 0..20u64 {
            apply_frame(&mut projection, index, text_frame(&index.to_string()));
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
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            model_view(),
            limit,
        );
        apply_publication_open(&mut projection);
        let (stalled, _notify) = projection
            .subscribe(RuntimeClientCursor::new(1))
            .expect("the current cursor is serviceable");

        // The subscriber never polls while 200 events are published.
        for index in 0..200u64 {
            apply_frame(&mut projection, index, text_frame(&index.to_string()));
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
        for step in 1..=3u64 {
            apply_frame(&mut projection, 199 + step, text_frame("x"));
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
        apply_frame(&mut projection, 0, text_frame("x"));
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
        let drained = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("batch");
        let _ = (first, second);
        // The authoritative items fold through the enqueue observations.
        for entry in drained.items() {
            projection.apply(ConversationObservation::InboundEnqueued(entry.clone()));
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
        projection.apply(ConversationObservation::InboundDrained(drained));
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert!(snapshot.inbound.pending.is_empty());
        let last_drain = snapshot.inbound.last_drain.expect("drain recorded");
        assert_eq!(last_drain.watermark.get(), 2);
        assert_eq!(last_drain.count, 2);
    }

    /// Native interactions are live Runtime Client projection facts: a
    /// pending event adds exactly one deterministic snapshot entry, and its
    /// terminal event removes that entry without touching canonical messages.
    #[test]
    fn interaction_pending_and_settled_fold_into_snapshot_and_events() {
        let mut projection = projection();
        let request = InteractionRequest {
            id: InteractionId::new("attempt-1-interaction-1"),
            conversation_id: ConversationId::new("conv-1"),
            attempt_id: attempt(),
            turn: 2,
            kind: InteractionKind::Approval {
                call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                tool_name: "alpha".to_owned(),
                origin: crate::tools::types::ToolOrigin::Builtin,
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({"path": "original"}),
                reason: "native policy".to_owned(),
            },
        };
        let requested_audit = interaction_audit(
            &request,
            "event-requested",
            RuntimeEvent::InteractionRequested {
                interaction_id: request.id.clone(),
                subject: InteractionSubject::Approval {
                    call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-alpha"),
                    tool_name: "alpha".to_owned(),
                    arguments_digest: "0".repeat(64),
                    reason: "native policy".to_owned(),
                },
            },
        );
        projection.apply(ConversationObservation::InteractionPending {
            interaction: RoutedInteraction {
                interaction: InteractionRef::new(
                    request.conversation_id.clone(),
                    request.id.clone(),
                ),
                source: InteractionSource::Primary,
                request: request.clone(),
            },
            audit: Some((requested_audit, crate::durable::TranscriptCursor::new(1))),
        });
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.pending_interactions,
            vec![RoutedInteraction {
                interaction: InteractionRef::new(
                    request.conversation_id.clone(),
                    request.id.clone(),
                ),
                source: InteractionSource::Primary,
                request: request.clone(),
            }]
        );
        assert_eq!(cursor, RuntimeClientCursor::new(2));
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert!(matches!(
            events.first().map(|event| &event.event),
            Some(RuntimeClientEvent::InteractionPending { interaction })
                if interaction.request == request
        ));

        let outcome = InteractionOutcome::Responded {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        };
        let settled_audit = interaction_audit(
            &request,
            "event-settled",
            RuntimeEvent::InteractionSettled {
                interaction_id: request.id.clone(),
                settlement: InteractionSettlement::Approved,
            },
        );
        projection.apply(ConversationObservation::InteractionSettled {
            interaction: InteractionRef::new(request.conversation_id.clone(), request.id.clone()),
            outcome: outcome.clone(),
            audit: Some((settled_audit, crate::durable::TranscriptCursor::new(2))),
        });
        let (snapshot, cursor) = projection.snapshot().expect("snapshot");
        assert!(snapshot.pending_interactions.is_empty());
        assert_eq!(cursor, RuntimeClientCursor::new(4));
        let events = collect(&mut projection, RuntimeClientCursor::new(2));
        assert!(matches!(
            events.first().map(|event| &event.event),
            Some(RuntimeClientEvent::InteractionSettled {
                interaction,
                outcome: event_outcome,
            }) if interaction
                == &InteractionRef::new(request.conversation_id.clone(), request.id.clone())
                && event_outcome == &outcome
        ));

        // The projection never turns a live prompt into a historical audit;
        // only the durable requested/settled audit observations do that.
        assert!(
            projection
                .snapshot()
                .expect("snapshot")
                .0
                .messages
                .is_empty()
        );
    }

    /// The root projection is a set of independently addressed routed
    /// interactions. Primary and child prompts can coexist, and removing or
    /// settling one pair never selects or changes another pair.
    #[test]
    fn routed_interactions_keep_multiple_sources_and_removals_independent() {
        let mut projection = projection();
        let approval = |conversation: &str, id: &str| InteractionRequest {
            id: InteractionId::new(id),
            conversation_id: ConversationId::new(conversation),
            attempt_id: attempt(),
            turn: 1,
            kind: InteractionKind::Approval {
                call_id: ToolCallId::new(format!("{id}-call")),
                tool_id: ToolId::new("tool-alpha"),
                tool_name: "alpha".to_owned(),
                origin: crate::tools::types::ToolOrigin::Builtin,
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({"id": id}),
                reason: "independent prompt".to_owned(),
            },
        };
        let primary = RoutedInteraction::primary(approval("conv-primary", "primary-a"));
        let child_a_request = approval("conv-child-a", "child-a");
        let child_a = RoutedInteraction::subagent(
            crate::runtime::identity::SubagentId::new("conv-primary-subagent-1"),
            child_a_request.conversation_id.clone(),
            crate::runtime::subagent::catalog::SubagentName::parse("explore").expect("agent name"),
            child_a_request,
        );
        let other_child_request = approval("conv-child-b", "child-b");
        let child_b = RoutedInteraction::subagent(
            crate::runtime::identity::SubagentId::new("conv-primary-subagent-2"),
            other_child_request.conversation_id.clone(),
            crate::runtime::subagent::catalog::SubagentName::parse("explore").expect("agent name"),
            other_child_request,
        );

        for interaction in [primary.clone(), child_a.clone(), child_b.clone()] {
            projection.apply(ConversationObservation::InteractionPending {
                interaction,
                audit: None,
            });
        }
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.pending_interactions.len(), 3);
        assert_eq!(
            snapshot
                .pending_interactions
                .iter()
                .map(|interaction| interaction.interaction.clone())
                .collect::<Vec<_>>(),
            vec![
                child_a.interaction.clone(),
                child_b.interaction.clone(),
                primary.interaction.clone(),
            ]
        );

        projection.apply(ConversationObservation::InteractionRemoved {
            interaction: child_a.interaction.clone(),
        });
        let (snapshot, _) = projection.snapshot().expect("snapshot after child removal");
        assert_eq!(
            snapshot
                .pending_interactions
                .iter()
                .map(|interaction| interaction.interaction.clone())
                .collect::<Vec<_>>(),
            vec![child_b.interaction.clone(), primary.interaction.clone()]
        );

        projection.apply(ConversationObservation::InteractionSettled {
            interaction: child_b.interaction.clone(),
            outcome: InteractionOutcome::Responded {
                response: InteractionResponse::Approval {
                    decision: ApprovalDecision::Allow,
                },
            },
            audit: None,
        });
        let (snapshot, _) = projection.snapshot().expect("snapshot after settlement");
        assert_eq!(snapshot.pending_interactions, vec![primary]);
        assert!(snapshot.messages.is_empty());
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
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                execution_id: crate::runtime::identity::ToolExecutionId::new("exec_1"),
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                state: BackgroundLifecycle::Running,
                progress: None,
                result: None,
            },
        ));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                execution_id: crate::runtime::identity::ToolExecutionId::new("exec_1"),
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                state: BackgroundLifecycle::Succeeded,
                progress: None,
                result: Some(success_result()),
            },
        ));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                execution_id: crate::runtime::identity::ToolExecutionId::new("exec_2"),
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                state: BackgroundLifecycle::Starting,
                progress: None,
                result: None,
            },
        ));
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.background.len(), 2);
        assert_eq!(snapshot.background[0].execution_id.as_str(), "exec_1");
        assert_eq!(snapshot.background[0].state, BackgroundLifecycle::Succeeded);
        assert!(snapshot.background[0].result.is_some());
        assert_eq!(snapshot.background[1].execution_id.as_str(), "exec_2");
    }

    /// Issue #202: the projection carries the honest background terminal
    /// lifecycle vocabulary verbatim. `OutcomeUnknown` and `TimedOut` are
    /// first-class terminal states and are never collapsed into `Failed`,
    /// in neither the folded view nor the published event, on or off the
    /// wire.
    #[test]
    fn issue202_background_terminal_states_project_verbatim() {
        use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
        let mut projection = projection();
        let outcome_unknown_result = ToolExecutionResult {
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "cancellation raced the executor after dispatch".to_owned(),
            },
            ..success_result()
        };
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                execution_id: crate::runtime::identity::ToolExecutionId::new("exec_unknown"),
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                state: BackgroundLifecycle::OutcomeUnknown,
                progress: None,
                result: Some(outcome_unknown_result),
            },
        ));
        projection.apply(ConversationObservation::Background(
            BackgroundExecutionSnapshot {
                execution_id: crate::runtime::identity::ToolExecutionId::new("exec_timed_out"),
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".to_owned(),
                state: BackgroundLifecycle::TimedOut,
                progress: None,
                result: Some(ToolExecutionResult {
                    status: ToolExecutionStatus::TimedOut,
                    ..success_result()
                }),
            },
        ));

        // The folded view keeps the typed terminal states verbatim.
        let (snapshot, _) = projection.snapshot().expect("snapshot");
        assert_eq!(snapshot.background.len(), 2);
        assert_eq!(
            snapshot.background[0].state,
            BackgroundLifecycle::OutcomeUnknown
        );
        assert_eq!(snapshot.background[1].state, BackgroundLifecycle::TimedOut);

        // The published events carry the same verbatim states.
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        let [unknown_event, timed_out_event] = events.as_slice() else {
            panic!("two BackgroundExecutionUpdated events: {events:?}");
        };
        let RuntimeClientEvent::BackgroundExecutionUpdated {
            execution: unknown_view,
        } = &unknown_event.event
        else {
            panic!(
                "a BackgroundExecutionUpdated event: {:?}",
                unknown_event.event
            );
        };
        let RuntimeClientEvent::BackgroundExecutionUpdated {
            execution: timed_out_view,
        } = &timed_out_event.event
        else {
            panic!(
                "a BackgroundExecutionUpdated event: {:?}",
                timed_out_event.event
            );
        };
        assert_eq!(unknown_view.state, BackgroundLifecycle::OutcomeUnknown);
        assert_eq!(timed_out_view.state, BackgroundLifecycle::TimedOut);

        // On the wire the states serialize as their typed snake_case names,
        // never "failed".
        let unknown_json = serde_json::to_value(unknown_view).expect("execution JSON");
        assert_eq!(unknown_json["state"], "outcome_unknown");
        let timed_out_json = serde_json::to_value(timed_out_view).expect("execution JSON");
        assert_eq!(timed_out_json["state"], "timed_out");
    }

    /// Issue #178: a subagent registry snapshot folds into the whole-view
    /// `SubagentUpdated` event carrying the live activity projection and
    /// the redacted execution profile, and the snapshot repair path serves
    /// the same enriched view.
    #[test]
    fn subagent_observations_fold_the_activity_projection_into_the_view() {
        use crate::runtime::subagent::{
            SubagentActivity, SubagentActivityCounters, SubagentExecutionProfile,
            SubagentObservation, SubagentSnapshot, SubagentState, SubagentWorkspaceResourceState,
            WorkspaceSnapshot,
        };

        fn snapshot(observation: SubagentObservation) -> SubagentSnapshot {
            SubagentSnapshot {
                subagent_id: crate::runtime::identity::SubagentId::new("conv-1-subagent-1"),
                child_agent_id: AgentId::new("agent-child"),
                child_conversation_id: ConversationId::new("conv-1-subagent-1"),
                tool_call_id: ToolCallId::new("call-1"),
                agent: "explore".to_owned(),
                definition_digest: "sha256:d1".to_owned(),
                workspace: WorkspaceSnapshot::shared(std::path::PathBuf::from(
                    "<shared-workspace>",
                )),
                handoff: None,
                workspace_resource_state: SubagentWorkspaceResourceState::None,
                state: SubagentState::Running,
                detail: None,
                observation,
                profile: Some(SubagentExecutionProfile {
                    model: "local/model".to_owned(),
                    reasoning_profile: None,
                    reasoning_enabled: false,
                }),
                publication_abandoned: false,
                settled: false,
                started_at: chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
                    .expect("timestamp")
                    .with_timezone(&chrono::Utc),
            }
        }

        let mut projection = projection();
        let observation = SubagentObservation {
            revision: 2,
            activity: SubagentActivity::Tool {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                progress: None,
            },
            counters: SubagentActivityCounters {
                tool_executions: 1,
                ..SubagentActivityCounters::default()
            },
            ..SubagentObservation::default()
        };
        projection.apply(ConversationObservation::SubagentLifecycle(snapshot(
            observation.clone(),
        )));
        // The disposable activity lane folds identically (unconditional
        // last-write-wins whole-view upsert).
        let newer = SubagentObservation {
            revision: 3,
            ..observation.clone()
        };
        projection.apply(ConversationObservation::SubagentActivity(snapshot(
            newer.clone(),
        )));

        // Both events carry the enriched whole view; the latest wins.
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        let [_, event] = events.as_slice() else {
            panic!("two SubagentUpdated events: {events:?}");
        };
        let RuntimeClientEvent::SubagentUpdated { subagent } = &event.event else {
            panic!("a SubagentUpdated event: {:?}", event.event);
        };
        assert_eq!(subagent.observation, newer);
        assert_eq!(
            subagent.execution_profile,
            Some(SubagentExecutionProfile {
                model: "local/model".to_owned(),
                reasoning_profile: None,
                reasoning_enabled: false,
            })
        );
        assert_eq!(
            subagent.started_at,
            chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
                .expect("timestamp")
                .with_timezone(&chrono::Utc)
        );

        // The snapshot repair path re-reads the same enriched view.
        let (repaired, _) = projection.snapshot().expect("snapshot");
        assert_eq!(repaired.subagents.len(), 1);
        assert_eq!(repaired.subagents[0].observation, newer);
        assert_eq!(
            repaired.subagents[0].execution_profile,
            subagent.execution_profile
        );
    }
}
