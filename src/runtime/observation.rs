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
//! It is also the projection worker's rendezvous point. The worker holds
//! `Arc<PendingObservations>` — never an owning runtime/client handle
//! across an await — so this queue, not the runtime, is what keeps the
//! worker's wait alive. The queue is closed (idempotently) when either the
//! conversation runtime or the Runtime Client adapter is destroyed; closing
//! is the worker's terminal condition.
//!
//! # There is exactly one fold
//!
//! The runtime does **not** fold this vocabulary into a second read model.
//! A Runtime Client host binds before the conversation runtime is
//! activated (Issue #61), and an inactive runtime publishes no observation
//! at all, so the observation queue — when a consumer exists — carries
//! every observation the runtime ever emits. The client projection is
//! therefore the one and only fold of this stream, and no runtime-side
//! mirror of the client attempt view exists.

use std::collections::VecDeque;
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
use crate::runtime::identity::InteractionId;
use crate::runtime::inbound::{InboundBatch, InboundItem};
use crate::runtime::interaction::{InteractionOutcome, InteractionRequest};
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundExecutionSnapshot;

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
    /// One subagent registry transition snapshot (Issue #60).
    Subagent(crate::runtime::subagent::SubagentSnapshot),
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
        /// The live pending request.
        request: InteractionRequest,
        /// The requested audit committed before the prompt was released.
        audit: RuntimeEventEnvelope,
        /// The durable transcript position allocated for this audit.
        transcript_cursor: TranscriptCursor,
    },
    /// One native interaction reached its terminal rendezvous outcome.
    InteractionSettled {
        /// The interaction identity.
        interaction_id: InteractionId,
        /// The terminal outcome delivered to its semantic owner.
        outcome: InteractionOutcome,
        /// The durable settled audit, when its commit succeeded. `None` is
        /// the fail-closed unavailable outcome and creates no historical item.
        audit: Option<(RuntimeEventEnvelope, TranscriptCursor)>,
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
    /// Test-only park switch. While set, [`drain`](PendingObservations::drain)
    /// yields nothing, so a test can step the queue itself instead of
    /// racing the projection worker. It is read and written only while the
    /// queue lock is held, so a worker that has already entered `drain`
    /// either completed before the park or observes it.
    #[cfg(test)]
    parked: AtomicBool,
}

impl PendingObservations {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
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
        #[cfg(test)]
        if self.parked.load(Ordering::Acquire) {
            // Parked under the queue lock: whatever the projection worker
            // was about to fold, it folds nothing from here on.
            return Vec::new();
        }
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

    /// Test-only: removes and returns the single oldest queued
    /// observation, so a test can stop between two enqueues and inspect the
    /// consumer's state at exactly that cut.
    #[cfg(test)]
    pub(crate) fn pop_one(&self) -> Option<ConversationObservation> {
        self.queue
            .lock()
            .expect("pending observation queue lock poisoned")
            .pop_front()
    }

    /// Test-only: stops the projection worker from folding anything, so a
    /// test owns the fold schedule and can inspect every cut of the
    /// observation stream deterministically.
    ///
    /// Parking takes the queue lock, so it is ordered against every
    /// concurrent `drain`: a worker either drained before the park or
    /// drains nothing after it. [`pop_one`](PendingObservations::pop_one)
    /// and [`queued`](PendingObservations::queued) deliberately ignore the
    /// park — they are the test's own hands on the queue.
    #[cfg(test)]
    pub(crate) fn park(&self) {
        let _queue = self
            .queue
            .lock()
            .expect("pending observation queue lock poisoned");
        self.parked.store(true, Ordering::Release);
    }

    /// Test-only: the number of observations waiting to be folded.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.queue
            .lock()
            .expect("pending observation queue lock poisoned")
            .len()
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
