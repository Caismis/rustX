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
//! Native pending/settled interactions are live process-owned observations,
//! not canonical Message Ledger facts and not recovery input. The runtime
//! never emits Runtime Client projection types: every variant
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
//! # The delivery classes
//!
//! The queue carries two delivery classes (Issue #178):
//!
//! - **Reliable** semantic/lifecycle observations: ordered FIFO, non-lossy.
//!   Every pushed observation reaches the consumer exactly once, in push
//!   order. This is the class of every variant except
//!   [`ConversationObservation::SubagentActivity`] and
//!   [`ConversationObservation::ToolProgress`].
//! - **Disposable** observations: latest-value, coalescing, in two keyed
//!   lanes. Subagent activity
//!   ([`ConversationObservation::SubagentActivity`]) is keyed by subagent
//!   identity; live foreground tool progress
//!   ([`ConversationObservation::ToolProgress`]) is keyed by tool call. A
//!   push overwrites the previous unpublished value of its key in place,
//!   so each lane is bounded by the number of active publishers, never by
//!   the number of updates, and a slow consumer provably never slows
//!   reliable publication. A lifecycle snapshot
//!   ([`ConversationObservation::SubagentLifecycle`]) carries the newest
//!   observation projection of its subagent, so it evicts any queued
//!   activity snapshot of that subagent; a tool settlement fact
//!   (`ToolExecutionCompleted`/`ToolExecutionFailed`) likewise evicts the
//!   call's queued live progress: no consumer ever folds a disposable
//!   value older than the reliable fact it already folded.
//!
//! # The worker rendezvous
//!
//! It is also the projection worker's rendezvous point. The worker holds
//! `Arc<PendingObservations>` — never an owning runtime/client handle
//! across an await — so this queue, not the runtime, is what keeps the
//! worker's wait alive. The queue is closed (idempotently) when either the
//! conversation runtime or the Runtime Client adapter is destroyed; closing
//! is the worker's terminal condition.
//!
//! # There is one Runtime Client fold
//!
//! The runtime does **not** fold this vocabulary into a second durable or
//! Runtime Client read model. A Runtime Client host binds before the
//! conversation runtime is activated (Issue #61), and an inactive runtime
//! publishes no observation at all, so the host's primary observation queue
//! carries every observation the runtime ever emits. A bounded local observer
//! may subscribe to the same runtime-owned stream for an existing observation
//! surface (for example the parent's disposable subagent activity projection),
//! but it never becomes a history authority and never replaces the Runtime
//! Client projection.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent::observer::AgentStatusObservation;
use crate::capabilities::{CapabilityAvailability, CapabilitySnapshot};
use crate::durable::TranscriptCursor;
use crate::events::types::RuntimeEvent;
use crate::events::types::RuntimeEventEnvelope;
use crate::message::types::MessageBlock;
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::publication::{PublicationAudit, PublicationFrame, PublicationStreamStart};
use crate::runtime::identity::AttemptId;
use crate::runtime::identity::SubagentId;
use crate::runtime::identity::{ToolCallId, ToolId};
use crate::runtime::inbound::{InboundBatch, InboundItem};
use crate::runtime::interaction::{InteractionOutcome, InteractionRef, RoutedInteraction};
use crate::runtime::subagent::SubagentSnapshot;
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundExecutionSnapshot;
use crate::tools::types::ToolProgress;

/// One runtime-owned semantic observation.
///
/// The observation union carries every external state change the Runtime
/// Client projection folds. It is the single entry point of the projection:
/// no call site folds state directly. Every variant carries runtime-owned
/// source types only — the Runtime Client layer owns the translation into
/// its snapshot/event vocabulary.
#[derive(Debug, Clone)]
// The pending-interaction variant deliberately keeps its live request,
// audit envelope, and transcript cursor inline: the enum is constructed on
// rare interaction boundaries, and boxing an envelope would add an
// allocation to the hot observation path. The size spread is by design.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ConversationObservation {
    /// One canonical internal runtime fact of an attempt.
    Event {
        /// The emitting attempt.
        attempt_id: AttemptId,
        /// The canonical fact.
        event: RuntimeEvent,
    },
    /// One context-compaction lifecycle fact not owned by an Agent attempt.
    ///
    /// Manual compaction is runtime maintenance: completion still names the
    /// atomic durable `RuntimeEvent::CompactionCompleted` fact, but its live
    /// publication is delayed until coordinator ownership is restored. Start
    /// and failure are live operation observations. Only compaction event
    /// variants are legal in this lane.
    ManualCompactionEvent {
        /// The compaction lifecycle fact.
        event: RuntimeEvent,
    },
    /// One canonical message commit (the loop's commit observation seam;
    /// the internal committed-message events reference identity only).
    Committed {
        /// The committing attempt, when one is active.
        attempt_id: Option<AttemptId>,
        /// The committed canonical message.
        block: MessageBlock,
        /// The durable transcript position of this message, when visible.
        transcript_cursor: Option<TranscriptCursor>,
    },
    /// One composed Agent Status observation.
    Status(AgentStatusObservation),
    /// One publication stream opened (Issue #108). The frozen identity pins
    /// the stream to the exact request generation that started it.
    PublicationOpened {
        /// The streaming attempt.
        attempt_id: AttemptId,
        /// The frozen publication identity.
        start: PublicationStreamStart,
    },
    /// One durably committed publication frame, observed at its release
    /// point. Nothing here reached a client before its own commit.
    Publication {
        /// The streaming attempt.
        attempt_id: AttemptId,
        /// The committed-for-release frame.
        frame: PublicationFrame,
    },
    /// One publication stream settled as an audit: the released output was
    /// either never accepted as a conversation Assistant message, or never
    /// reached its own durable publication terminal.
    PublicationSettled {
        /// The streaming attempt.
        attempt_id: AttemptId,
        /// The bounded immutable audit.
        audit: Box<PublicationAudit>,
        /// The durable transcript position allocated for this audit.
        transcript_cursor: TranscriptCursor,
    },
    /// One mailbox enqueue (authoritative item + sequence).
    InboundEnqueued(InboundItem),
    /// One mailbox finite drain (authoritative batch).
    InboundDrained(InboundBatch),
    /// One background registry transition snapshot.
    Background(BackgroundExecutionSnapshot),
    /// One live, not-yet-durable foreground tool progress report (Issue
    /// #178). Disposable: latest-value per tool call, coalesced in the
    /// queue, never durable, never model-facing. The canonical fact commits
    /// at batch settlement as `RuntimeEvent::ToolExecutionProgress`.
    ToolProgress {
        /// The owning attempt.
        #[allow(dead_code)]
        // identity carried for consumers; the in-crate folds key on the call
        attempt_id: AttemptId,
        /// The in-flight tool call.
        tool_call_id: ToolCallId,
        /// The executing tool.
        #[allow(dead_code)]
        // identity carried for consumers; the in-crate folds key on the call
        tool_id: ToolId,
        /// The latest bounded progress notification.
        progress: ToolProgress,
    },
    /// One subagent registry lifecycle/identity transition snapshot (Issue
    /// #60, reclassified #178). **Reliable**: ordered FIFO, non-lossy —
    /// every identity/lifecycle/terminal transition reaches the consumer
    /// exactly once, in publication order.
    SubagentLifecycle(SubagentSnapshot),
    /// One reliable retained-workspace resource transition. This is separate
    /// from the logical lifecycle lane: disposing a handoff updates only the
    /// resource projection and never creates another terminal transition.
    SubagentWorkspace(SubagentSnapshot),
    /// One subagent live-activity snapshot (Issue #178). **Disposable**:
    /// latest-value, coalescing, keyed by subagent identity — a push
    /// overwrites the previous unpublished snapshot of the same subagent,
    /// so this lane is bounded by the number of active subagents and never
    /// consumes the queue capacity or ordering authority of the reliable
    /// lane. A `SubagentLifecycle` snapshot carries the newest observation
    /// projection of its subagent and evicts any queued activity snapshot
    /// for it.
    SubagentActivity(SubagentSnapshot),
    /// One activated authoritative capability snapshot, together with the
    /// authoritative per-source availability state at that commit (Issue
    /// #81). The availability may change without a revision swap (a
    /// diagnostic-only change); the snapshot never changes without one.
    ///
    /// A runtime resource reload does **not** publish this variant: it
    /// commits a capability generation and a resource generation together
    /// and publishes both as one [`ConversationObservation::Resources`], so
    /// no consumer can fold half a generation.
    Capability {
        /// The activated immutable capability snapshot.
        snapshot: Arc<CapabilitySnapshot>,
        /// The authoritative availability of every evaluated optional
        /// capability source.
        availability: CapabilityAvailability,
    },
    /// One published immutable runtime resource generation, **including the
    /// capability generation it was built against**.
    ///
    /// This is the complete result of one resource reload and the only
    /// observation a reload emits. The capability half is deliberately not
    /// published separately: the two writes are ordered inside the runtime,
    /// but the consumer folding this queue runs on its own task under its
    /// own lock, so two observations would be two folds and a subscriber
    /// could observe the new capability beside the retired resource
    /// generation — a pairing that never existed. One observation makes the
    /// generation atomic for every consumer by construction rather than by
    /// timing.
    ///
    /// Both halves are still carried explicitly, because the resource
    /// revision and the capability revision move independently: a reload
    /// that only rewrites project instruction files advances the resource
    /// revision and leaves the capability revision exactly where it was.
    Resources {
        /// The published immutable resource generation. Its
        /// `capability()` is the capability snapshot committed by the same
        /// reload.
        snapshot: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
        /// The authoritative availability of every evaluated optional
        /// capability source at that same commit.
        availability: CapabilityAvailability,
    },

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
    /// The runtime approval control state changed. `effective` is the mode
    /// admitted by the current attempt boundary; `pending` is the latest
    /// desired mode when it differs and the runtime is busy.
    ApprovalModeChanged {
        /// The authoritative effective mode.
        effective: ApprovalMode,
        /// The pending desired mode, when reconciliation waits for settlement.
        pending: Option<ApprovalMode>,
        /// The monotonic control-plane revision.
        revision: u64,
    },
    /// Runtime drain began and new semantic admission closed.
    Shutdown,
    /// One native interaction became pending.
    InteractionPending {
        /// The root-facing routed request. The originating coordinator still
        /// owns the pending waiter and settlement.
        interaction: RoutedInteraction,
        /// The originating conversation's requested audit and cursor, when
        /// this is the primary conversation. Child audits stay child-local.
        audit: Option<(RuntimeEventEnvelope, TranscriptCursor)>,
    },
    /// One native interaction reached its terminal rendezvous outcome.
    InteractionSettled {
        /// The full routed identity.
        interaction: InteractionRef,
        /// The terminal outcome delivered to its semantic owner.
        outcome: InteractionOutcome,
        /// The originating conversation's settled audit, when this is the
        /// primary conversation and its commit succeeded.
        audit: Option<(RuntimeEventEnvelope, TranscriptCursor)>,
    },
    /// A child process died while owning one or more live interactions.
    ///
    /// This is a projection removal, not a synthetic interaction settlement:
    /// the originating coordinator died with the child and no terminal
    /// response is invented for the historical audit.
    InteractionRemoved {
        /// The no-longer-actionable routed identity.
        interaction: InteractionRef,
    },
    /// The durable authority (Pending Inbound Inbox / Message Ledger) failed
    /// a storage operation the coordinator must not silently swallow
    /// (Issue #63, Finding 5). The failure consumes that stage's one bounded
    /// retry allowance for the current finite admission cycle and schedules a
    /// re-kick; the allowance remains consumed if the cycle advances to the
    /// other durable stage. Accepted pending work remains intact.
    DurableFailure {
        /// The human-readable storage failure description.
        #[allow(dead_code)]
        // surfaced by tests; the projection folds it without a client event
        message: String,
    },
    /// The durable authority failed persistently — a second failure of either
    /// transient admission stage in the same finite cycle, or an immediately
    /// non-transient durable
    /// failure (a semantic contract failure, an active attempt's
    /// canonical-write failure, or an exhausted background
    /// terminal-publication budget): the runtime has entered an explicit
    /// degraded state and will not pretend normal operation continues
    /// (Issue #63, Finding 5).
    DurabilityFailed {
        /// The operation that failed persistently.
        operation: String,
        /// The human-readable failure diagnostic.
        diagnostic: String,
    },
}

/// The tiny synchronization boundary between the conversation runtime and
/// its observation consumers (the Runtime Client projection).
///
/// This type is the leaf of the lock graph: it owns one mutex over a small
/// state struct plus a `Notify` and calls nothing.
///
/// Two reliable/disposable delivery classes (Issue #178) live side by side
/// behind the one lock: the reliable FIFO of semantic/lifecycle
/// observations, plus two disposable latest-value lanes — subagent activity
/// snapshots keyed by subagent identity, and live (not-yet-durable)
/// foreground tool progress keyed by tool call. Each disposable lane is
/// bounded by the number of active publishers (subagents, in-flight tool
/// calls) — never by the number of updates — so disposable observation
/// traffic provably never consumes queue capacity, synchronization
/// authority, or terminal progress required by the reliable lane.
pub(crate) struct PendingObservations {
    /// The delivery lanes.
    state: Mutex<PendingState>,
    /// Wakes the worker task on every push and on close.
    notify: tokio::sync::Notify,
    /// Set by [`close`](PendingObservations::close). Terminal: no further
    /// observation is accepted and the worker exits.
    closed: AtomicBool,
    /// Test-only worker-exit signal, so worker termination is observable
    /// deterministically instead of by timeout.
    #[cfg(test)]
    worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    /// Test-only park switch. While set, [`drain`](PendingObservations::drain)
    /// yields nothing, so a test can step the queue itself instead of
    /// racing the projection worker. It is read and written only while the
    /// state lock is held, so a worker that has already entered `drain`
    /// either completed before the park or observes it.
    #[cfg(test)]
    parked: AtomicBool,
}

/// The runtime-owned fan-out for one observation stream.
///
/// The Runtime Client projection is always the primary consumer. A runtime
/// may additionally register a bounded, disposable observation consumer before
/// activation when an existing local observation surface needs the same
/// semantic stream. Each consumer receives its own [`PendingObservations`]
/// queue, so one consumer can never drain, park, or close another consumer's
/// projection input. This is an in-process observation seam, not a transcript
/// channel and not a second durable authority.
pub(crate) struct ObservationFanout {
    /// The primary queue owned by the Runtime Client projection.
    primary: Arc<PendingObservations>,
    /// Serializes publication and close so every consumer sees the same
    /// observation order and no subscriber is closed halfway through a
    /// publication.
    dispatch: Mutex<()>,
    /// Weak subscriber queues. The caller owns the returned queue; a dropped
    /// subscriber is removed on the next publication.
    subscribers: Mutex<Vec<std::sync::Weak<PendingObservations>>>,
}

impl ObservationFanout {
    /// Creates a fan-out with the Runtime Client queue as its primary sink.
    pub(crate) fn new(primary: Arc<PendingObservations>) -> Self {
        Self {
            primary,
            dispatch: Mutex::new(()),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// Returns the primary Runtime Client queue.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn primary(&self) -> Arc<PendingObservations> {
        Arc::clone(&self.primary)
    }

    /// Adds one pre-activation observation consumer.
    ///
    /// The caller must ensure the runtime is still inactive. That lifecycle
    /// check belongs to `ConversationRuntime`, which owns the activation
    /// linearization; this leaf only owns queue fan-out.
    pub(crate) fn subscribe(&self) -> Arc<PendingObservations> {
        let queue = Arc::new(PendingObservations::new());
        self.subscribers
            .lock()
            .expect("observation fan-out lock poisoned")
            .push(Arc::downgrade(&queue));
        queue
    }

    /// Publishes one observation to every currently live consumer.
    pub(crate) fn push(&self, observation: &ConversationObservation) {
        let _dispatch = self
            .dispatch
            .lock()
            .expect("observation fan-out dispatch lock poisoned");
        self.primary.push(observation.clone());
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("observation fan-out lock poisoned");
        subscribers.retain(|subscriber| {
            let Some(queue) = subscriber.upgrade() else {
                return false;
            };
            queue.push(observation.clone());
            !queue.is_closed()
        });
    }

    /// Closes the primary queue and every currently live subscriber.
    pub(crate) fn close(&self) {
        let _dispatch = self
            .dispatch
            .lock()
            .expect("observation fan-out dispatch lock poisoned");
        self.primary.close();
        let subscribers = self
            .subscribers
            .lock()
            .expect("observation fan-out lock poisoned")
            .iter()
            .filter_map(std::sync::Weak::upgrade)
            .collect::<Vec<_>>();
        for subscriber in subscribers {
            subscriber.close();
        }
    }
}

/// The delivery lanes behind the one queue lock.
struct PendingState {
    /// The reliable lane: ordered, non-lossy semantic/lifecycle
    /// observations.
    reliable: VecDeque<ConversationObservation>,
    /// The disposable subagent-activity lane: the latest unpublished
    /// activity snapshot of each subagent that reported one, keyed by
    /// subagent identity.
    latest_activity: BTreeMap<SubagentId, SubagentSnapshot>,
    /// The disposable live-tool-progress lane (Issue #178): the latest
    /// unpublished live progress report of each in-flight foreground tool
    /// call, keyed by tool call. Entries are whole
    /// [`ConversationObservation::ToolProgress`] values so the key never
    /// duplicates the payload's identity fields.
    latest_progress: BTreeMap<ToolCallId, ConversationObservation>,
}

impl PendingObservations {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(PendingState {
                reliable: VecDeque::new(),
                latest_activity: BTreeMap::new(),
                latest_progress: BTreeMap::new(),
            }),
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            #[cfg(test)]
            worker_exit: Mutex::new(None),
            #[cfg(test)]
            parked: AtomicBool::new(false),
        }
    }

    pub(crate) fn push(&self, observation: ConversationObservation) {
        if self.closed.load(Ordering::Acquire) {
            // A closed observation queue is terminal: never queue an
            // observation that nothing will ever fold.
            return;
        }
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        match observation {
            // Disposable: overwrite in place — the queue holds only the
            // latest unpublished activity snapshot per subagent.
            ConversationObservation::SubagentActivity(snapshot) => {
                state
                    .latest_activity
                    .insert(snapshot.subagent_id.clone(), snapshot);
            }
            // Disposable: overwrite in place — the queue holds only the
            // latest unpublished live progress report per tool call.
            ConversationObservation::ToolProgress { .. } => {
                let ConversationObservation::ToolProgress { tool_call_id, .. } = &observation
                else {
                    unreachable!("the match admitted exactly this variant");
                };
                let tool_call_id = tool_call_id.clone();
                state.latest_progress.insert(tool_call_id, observation);
            }
            // Reliable, and authoritative over activity: a lifecycle
            // snapshot carries the newest observation projection of its
            // subagent, so it evicts any queued activity snapshot of that
            // subagent. No consumer ever folds an activity snapshot older
            // than the lifecycle snapshot it already folded.
            ConversationObservation::SubagentLifecycle(snapshot) => {
                state.latest_activity.remove(&snapshot.subagent_id);
                state
                    .reliable
                    .push_back(ConversationObservation::SubagentLifecycle(snapshot));
            }
            ConversationObservation::SubagentWorkspace(snapshot) => {
                state.latest_activity.remove(&snapshot.subagent_id);
                state
                    .reliable
                    .push_back(ConversationObservation::SubagentWorkspace(snapshot));
            }
            // Reliable; a tool settlement fact retires the call's pending
            // live progress: a settled call leaves no stale live report
            // behind.
            ConversationObservation::Event { ref event, .. }
                if matches!(
                    event,
                    RuntimeEvent::ToolExecutionCompleted { .. }
                        | RuntimeEvent::ToolExecutionFailed { .. }
                ) =>
            {
                let (RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }
                | RuntimeEvent::ToolExecutionFailed { tool_call_id, .. }) = event
                else {
                    unreachable!("the match guard admits exactly the two settlement facts")
                };
                state.latest_progress.remove(tool_call_id);
                state.reliable.push_back(observation);
            }
            other => state.reliable.push_back(other),
        }
        drop(state);
        self.notify.notify_one();
    }

    /// Drains everything currently queued, in fold order: the reliable
    /// entries in push order first, then the disposable lanes — the latest
    /// live progress report of each in-flight tool call (in tool-call
    /// identity order), then the latest activity snapshot of each subagent
    /// (in subagent-identity order).
    ///
    /// This ordering is regression-free by construction: every queued
    /// activity entry is strictly newer than any queued lifecycle snapshot
    /// of the same subagent (a lifecycle push evicts it), and every queued
    /// live progress entry belongs to a tool call whose settlement fact is
    /// not queued (a settlement push evicts it).
    pub(crate) fn drain(&self) -> Vec<ConversationObservation> {
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        #[cfg(test)]
        if self.parked.load(Ordering::Acquire) {
            // Parked under the state lock: whatever the projection worker
            // was about to fold, it folds nothing from here on.
            return Vec::new();
        }
        let mut drained: Vec<ConversationObservation> = state.reliable.drain(..).collect();
        drained.extend(std::mem::take(&mut state.latest_progress).into_values());
        drained.extend(
            std::mem::take(&mut state.latest_activity)
                .into_values()
                .map(ConversationObservation::SubagentActivity),
        );
        drained
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
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state.reliable.clear();
        state.latest_activity.clear();
        state.latest_progress.clear();
        drop(state);
        self.notify.notify_one();
    }

    /// Test-only: removes and returns the single oldest queued
    /// observation, so a test can stop between two enqueues and inspect the
    /// consumer's state at exactly that cut. The reliable lane's oldest
    /// entry wins; an empty reliable lane pops the disposable lanes in the
    /// documented drain order (live tool progress in tool-call order, then
    /// subagent activity in subagent-identity order, the latter wrapped as
    /// [`ConversationObservation::SubagentActivity`]).
    #[cfg(test)]
    pub(crate) fn pop_one(&self) -> Option<ConversationObservation> {
        let mut state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        if let Some(observation) = state.reliable.pop_front() {
            return Some(observation);
        }
        if let Some((_, observation)) = state.latest_progress.pop_first() {
            return Some(observation);
        }
        state
            .latest_activity
            .pop_first()
            .map(|(_, snapshot)| ConversationObservation::SubagentActivity(snapshot))
    }

    /// Test-only: stops the projection worker from folding anything, so a
    /// test owns the fold schedule and can inspect every cut of the
    /// observation stream deterministically.
    ///
    /// Parking takes the state lock, so it is ordered against every
    /// concurrent `drain`: a worker either drained before the park or
    /// drains nothing after it. [`pop_one`](PendingObservations::pop_one)
    /// and [`queued`](PendingObservations::queued) deliberately ignore the
    /// park — they are the test's own hands on the queue.
    #[cfg(test)]
    pub(crate) fn park(&self) {
        let _state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        self.parked.store(true, Ordering::Release);
    }

    /// Test-only: lifts the park and wakes the projection worker, so a
    /// backlog that accumulated (coalesced) while parked folds on the
    /// worker's next drain.
    #[cfg(test)]
    pub(crate) fn unpark(&self) {
        {
            let _state = self
                .state
                .lock()
                .expect("pending observation queue lock poisoned");
            self.parked.store(false, Ordering::Release);
        }
        self.notify.notify_one();
    }

    /// Test-only: the number of observations waiting to be folded, across
    /// all delivery lanes.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        let state = self
            .state
            .lock()
            .expect("pending observation queue lock poisoned");
        state.reliable.len() + state.latest_activity.len() + state.latest_progress.len()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::identity::{AgentId, ConversationId, ToolCallId};
    use crate::runtime::subagent::{
        SubagentObservation, SubagentState, SubagentWorkspaceResourceState, WorkspaceSnapshot,
    };

    /// A minimal subagent snapshot carrying only the identity and the
    /// activity revision this suite distinguishes.
    fn subagent_snapshot(subagent_id: &str, revision: u64) -> SubagentSnapshot {
        SubagentSnapshot {
            subagent_id: SubagentId::new(subagent_id),
            child_agent_id: AgentId::new("agent-child"),
            child_conversation_id: ConversationId::new(subagent_id),
            tool_call_id: ToolCallId::new("call-1"),
            agent: "explore".to_owned(),
            definition_digest: "sha256:d1".to_owned(),
            workspace: WorkspaceSnapshot::shared(std::path::PathBuf::from("<shared>")),
            handoff: None,
            workspace_resource_state: SubagentWorkspaceResourceState::None,
            state: SubagentState::Running,
            cancel_reason: None,
            detail: None,
            observation: SubagentObservation {
                revision,
                ..SubagentObservation::default()
            },
            profile: None,
            publication_abandoned: false,
            settled: false,
            started_at: chrono::Utc::now(),
        }
    }

    /// One live tool progress report of `call`, carrying `message`.
    fn live_progress(call: &str, message: &str) -> ConversationObservation {
        ConversationObservation::ToolProgress {
            attempt_id: AttemptId::new("attempt-1"),
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-bash"),
            progress: ToolProgress {
                message: Some(message.to_owned()),
                ..ToolProgress::default()
            },
        }
    }

    /// N activity pushes of one subagent leave exactly one queued entry,
    /// and the drain yields only the latest snapshot.
    #[test]
    fn activity_pushes_of_one_subagent_coalesce_to_the_latest() {
        let queue = PendingObservations::new();
        for revision in 1..=5 {
            queue.push(ConversationObservation::SubagentActivity(
                subagent_snapshot("conv-1-subagent-1", revision),
            ));
        }
        assert_eq!(queue.queued(), 1, "the activity lane holds the latest only");
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        match &drained[0] {
            ConversationObservation::SubagentActivity(snapshot) => {
                assert_eq!(snapshot.subagent_id, SubagentId::new("conv-1-subagent-1"));
                assert_eq!(snapshot.observation.revision, 5, "only the latest survives");
            }
            other => panic!("expected an activity observation, got {other:?}"),
        }
        assert_eq!(queue.queued(), 0);
    }

    /// Activity entries of K subagents coexist: the disposable lane is
    /// bounded by the number of active subagents, not by the number of
    /// activity updates.
    #[test]
    fn activity_entries_coexist_per_subagent() {
        let queue = PendingObservations::new();
        for index in 1..=3 {
            let subagent_id = format!("conv-1-subagent-{index}");
            for revision in 1..=2 {
                queue.push(ConversationObservation::SubagentActivity(
                    subagent_snapshot(&subagent_id, revision),
                ));
            }
        }
        assert_eq!(queue.queued(), 3, "one entry per active subagent");
        let drained = queue.drain();
        assert_eq!(drained.len(), 3);
        for (index, observation) in drained.iter().enumerate() {
            match observation {
                ConversationObservation::SubagentActivity(snapshot) => {
                    let expected = format!("conv-1-subagent-{}", index + 1);
                    assert_eq!(snapshot.subagent_id, SubagentId::new(&expected));
                    assert_eq!(snapshot.observation.revision, 2);
                }
                other => panic!("expected an activity observation, got {other:?}"),
            }
        }
    }

    /// A lifecycle push evicts the queued activity entry of its subagent,
    /// and a newer activity push then queues again: the drain folds the
    /// lifecycle snapshot (reliable, ordered) first and the newer activity
    /// after it — never a stale activity on top of a lifecycle transition.
    #[test]
    fn a_lifecycle_snapshot_evicts_the_queued_activity() {
        let queue = PendingObservations::new();
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv-1-subagent-1", 3),
        ));
        queue.push(ConversationObservation::SubagentLifecycle(
            subagent_snapshot("conv-1-subagent-1", 4),
        ));
        assert_eq!(
            queue.queued(),
            1,
            "the lifecycle push evicted the stale activity"
        );
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv-1-subagent-1", 5),
        ));
        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        match (&drained[0], &drained[1]) {
            (
                ConversationObservation::SubagentLifecycle(lifecycle),
                ConversationObservation::SubagentActivity(activity),
            ) => {
                assert_eq!(lifecycle.observation.revision, 4);
                assert_eq!(activity.observation.revision, 5);
            }
            other => panic!("expected lifecycle then activity, got {other:?}"),
        }
    }

    /// Parked, pushed observations accumulate coalesced; the unpark hands
    /// the backlog to the consumer in one drain.
    #[test]
    fn a_parked_queue_coalesces_and_unpark_releases_the_backlog() {
        let queue = PendingObservations::new();
        queue.park();
        for revision in 1..=4 {
            queue.push(ConversationObservation::SubagentActivity(
                subagent_snapshot("conv-1-subagent-1", revision),
            ));
        }
        queue.push(ConversationObservation::Shutdown);
        assert!(queue.drain().is_empty(), "parked: the worker folds nothing");
        assert_eq!(queue.queued(), 2, "the backlog accumulated coalesced");
        queue.unpark();
        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], ConversationObservation::Shutdown));
        match &drained[1] {
            ConversationObservation::SubagentActivity(snapshot) => {
                assert_eq!(snapshot.observation.revision, 4);
            }
            other => panic!("expected the coalesced activity, got {other:?}"),
        }
    }

    /// Close clears both lanes and refuses further pushes.
    #[test]
    fn close_clears_both_lanes_terminally() {
        let queue = PendingObservations::new();
        queue.push(ConversationObservation::Shutdown);
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv-1-subagent-1", 1),
        ));
        queue.close();
        assert_eq!(queue.queued(), 0);
        queue.push(ConversationObservation::SubagentActivity(
            subagent_snapshot("conv-1-subagent-1", 2),
        ));
        assert_eq!(queue.queued(), 0, "a closed queue accepts nothing");
        assert!(queue.is_closed());
    }

    /// N live progress reports of one tool call coalesce to exactly one
    /// queued entry, and the drain yields only the latest report (after the
    /// reliable entries, in the documented fold order).
    #[test]
    fn live_progress_reports_of_one_call_coalesce_to_the_latest() {
        let queue = PendingObservations::new();
        queue.push(ConversationObservation::Shutdown);
        queue.push(live_progress("call-1", "first"));
        queue.push(live_progress("call-1", "second"));
        queue.push(live_progress("call-1", "third"));
        assert_eq!(queue.queued(), 2, "the progress lane holds the latest only");
        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert!(matches!(drained[0], ConversationObservation::Shutdown));
        match &drained[1] {
            ConversationObservation::ToolProgress {
                tool_call_id,
                progress,
                ..
            } => {
                assert_eq!(*tool_call_id, ToolCallId::new("call-1"));
                assert_eq!(progress.message.as_deref(), Some("third"));
            }
            other => panic!("expected the coalesced live progress, got {other:?}"),
        }
    }

    /// A settled call leaves no stale live progress behind: the durable
    /// settlement fact (reliable) evicts the call's pending live entry.
    #[test]
    fn a_tool_settlement_fact_evicts_the_pending_live_progress() {
        let queue = PendingObservations::new();
        queue.push(live_progress("call-1", "halfway"));
        queue.push(ConversationObservation::Event {
            attempt_id: AttemptId::new("attempt-1"),
            event: RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                result: crate::tools::types::ToolExecutionResult {
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            },
        });
        assert_eq!(queue.queued(), 1, "the settlement evicted the live report");
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(
            matches!(
                drained[0],
                ConversationObservation::Event {
                    event: RuntimeEvent::ToolExecutionCompleted { .. },
                    ..
                }
            ),
            "only the reliable settlement fact folds"
        );
    }
}
