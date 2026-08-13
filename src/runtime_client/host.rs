//! The Runtime Client host: the conversation coordinator and the one
//! linearization owner of externally visible Runtime Client state.
//!
//! [`RuntimeClientHost`] owns the highest-level conversation coordination
//! required by Runtime Client Protocol v1:
//!
//! ```text
//! conversation coordinator = admission/current-attempt coordination
//! AgentExecution           = attempt execution semantics (unchanged)
//! mailbox                  = asynchronous inbound ordering (unchanged)
//! background registry      = background lifecycle authority (unchanged)
//! capability coordinator   = capability authority (unchanged)
//! ```
//!
//! The host does not duplicate any attempt state machine: it coordinates
//! admission, holds the current-attempt handle, and drives attempts
//! asynchronously, while `AgentExecution` remains the settlement
//! authority.
//!
//! # The one synchronization boundary
//!
//! The host guards exactly one state instance with one lock. That lock is
//! the linearization owner of:
//!
//! - the Runtime Client projection (snapshot read model, cursor
//!   allocation, event publication, bounded replay, subscribers);
//! - the canonical conversation history between attempts;
//! - the current-attempt slot (publication/removal);
//! - attachment admission/detach;
//! - inbound admission decisions and shutdown.
//!
//! All observer callbacks converge on this one boundary, so snapshot,
//! cursor, subscription, admission, cancellation, and terminal settlement
//! linearize against each other by synchronization, never by timing.
//!
//! # The lock-order graph
//!
//! ```text
//!   HostState ──────────────► ConversationInboundMailbox
//!       │                              │
//!       │                              │
//!       ▼                              ▼
//!   PendingObservations ◄──────────────┘
//!       ▲          ▲
//!       │          └────────────── CapabilityCoordinator (state lock)
//!       └─────────────────────────  ConversationBackgroundRegistry
//! ```
//!
//! Exactly three rules make the graph acyclic, and each is structural
//! rather than conventional:
//!
//! 1. **`PendingObservations` is a leaf.** It owns one mutex over a
//!    `VecDeque` plus a `Notify`, and it calls nothing. No lock can be
//!    acquired beneath it.
//! 2. **No authoritative subsystem ever acquires `HostState`.** The
//!    mailbox, the background registry, and the capability coordinator all
//!    fire their observers *while their own lock is held*, so
//!    [`HostObserver`] converts each of those callbacks into a
//!    `PendingObservations::push` — an immutable append plus a wakeup.
//!    There is therefore no `subsystem -> HostState` edge to pair with the
//!    `HostState -> mailbox` edge below. This also means subscriber
//!    notification can never block authoritative runtime state: publishing
//!    happens under `HostState`, which no authoritative commit path ever
//!    waits on.
//! 3. **`HostState -> mailbox` is the only downward edge.** It exists in
//!    exactly one place, [`HostInner::admit_next_attempt`], which drains
//!    the mailbox under the host lock so the drain fact, the canonical
//!    history commits, and the attempt publication linearize together. The
//!    drain fires `on_drained` into the leaf queue, never back into the
//!    host lock.
//!
//! The [`AgentExecutionObserver`] callbacks are the one seam that applies
//! directly under `HostState`. That is sound and is *not* an exception to
//! rule 2: `AgentExecution` is owned exclusively by its attempt task and
//! holds no lock of its own when it observes, so the callback introduces
//! no incoming edge. Applying directly keeps streaming deltas on the
//! caller's thread instead of behind a task hop.
//!
//! Every host lock acquisition goes through [`HostInner::lock_state`],
//! which drains `PendingObservations` first. Queued observations therefore
//! fold in enqueue order, ahead of whatever the acquiring caller is about
//! to do, so the total order of externally visible transitions is the
//! order in which authoritative subsystems committed them.
//!
//! # The ownership graph
//!
//! ```text
//!   semantic owner ────────────────► Arc<HostInner>
//!   (RuntimeClientHost and its clones, RuntimeAttachment,
//!    RuntimeClientEndpoint, EventSubscription, a running attempt task)
//!
//!   HostInner ──► authoritative subsystems (tool runtime, mailbox,
//!                 capability coordinator)
//!             ──► projection state (HostState)
//!             ──► Arc<PendingObservations>
//!
//!   authoritative subsystem ──► Arc<HostObserver>
//!   HostObserver ─────────────► Weak<HostInner>
//!
//!   observation worker ───────► Weak<HostInner>
//!                         ────► Arc<PendingObservations>
//! ```
//!
//! > **Observation edges are non-owning with respect to
//! > `RuntimeClientHost`. No observer and no observation worker may extend
//! > `HostInner`'s lifetime.**
//!
//! Installing an observation seam therefore does not create a cycle: a
//! subsystem owns the observer, but the observer only *observes* the host.
//! When the last semantic owner is released, `HostInner` is destroyed at
//! that release — not at process exit — even while the subsystems, their
//! observer `Arc`s, and the worker task still exist.
//!
//! Teardown is one step: [`HostInner`]'s `Drop` closes
//! [`PendingObservations`], which is the worker's terminal condition. It
//! takes no host lock, joins nothing, and publishes nothing.
//!
//! A running attempt task is a deliberate, *bounded* strong owner: an
//! admitted attempt must reach settlement, and the task releases the host
//! when it does.
//!
//! # Canonical history ownership
//!
//! Between attempts the host owns the canonical conversation history (the
//! loop owns its private working copy during an attempt). At attempt
//! settlement the host replaces its history with the authoritative
//! `AgentExecutionResult.messages` under the one lock, and the projection
//! read model is verified against it. The projection mirror is never an
//! independent mutable history.
//!
//! # Detach is not cancellation
//!
//! Detaching an attachment changes only attachment state. It never
//! cancels, settles, or mutates semantic runtime work: the current
//! attempt, conversation-owned background executions, mailbox contents,
//! canonical conversation state, and capability state are untouched.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use chrono_tz::Tz;

use super::projection::{
    Observation, RuntimeClientProjection, SubscriberPoll, background_view, capability_view,
};
use super::snapshot::{RuntimeClientAttemptPhase, RuntimeClientSnapshot};
use super::types::{
    AttachmentId, RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RuntimeClientCursor, RuntimeClientError,
    RuntimeClientProtocolEvent, RuntimeClientResult,
};
use crate::agent::cancellation::AgentCancellation;
use crate::agent::observer::{AgentExecutionObserver, AgentStatusObservation};
use crate::agent::{AgentExecution, AgentExecutionRequest};
use crate::capabilities::{CapabilityCoordinator, CapabilityObserver};
use crate::context::checkpoint::ContextCheckpointStore;
use crate::context::tokens::TokenEstimator;
use crate::context::{AgentStatusComposer, ContextRuntime, SessionContextPolicy};
use crate::events::types::RuntimeEvent;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::invocation::ModelInvocationError;
use crate::model::session::{AttemptModelSnapshot, SessionModelConfig, SessionModelState};
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolExecutionId};
use crate::runtime::inbound::{
    ConversationInboundMailbox, FreshInboundTurn, InboundBatch, InboundItem, InboundObserver,
    InitialTurnTrigger,
};
use crate::runtime::types::{CancellationReason, RuntimeClock, SystemClock};
use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundObserver};
use crate::tools::runtime::ConversationToolRuntime;

/// The one Runtime Client host construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostConstructionError {
    /// The capability coordinator and the conversation tool runtime do not
    /// share the same conversation/workspace ownership domain.
    OwnershipMismatch {
        /// The capability conversation owner.
        capability_conversation: ConversationId,
        /// The tool runtime conversation owner.
        runtime_conversation: ConversationId,
    },
    /// The context engine configuration is impossible.
    Context(String),
    /// The conversation tool runtime identity (or the capability
    /// coordinator identity) is already bound to a Runtime Client host.
    ///
    /// Protocol v1 binds one runtime identity to at most one
    /// [`RuntimeClientHost`] for that identity's lifetime, so cloning a
    /// runtime bundle never yields a second bindable identity and dropping
    /// the bound host never makes it bindable again. Reconnect replaces the
    /// attachment, not the host.
    RuntimeClientAlreadyBound {
        /// The conversation whose runtime identity is already bound.
        conversation_id: ConversationId,
    },
}

impl core::fmt::Display for HostConstructionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OwnershipMismatch {
                capability_conversation,
                runtime_conversation,
            } => write!(
                f,
                "capability owner {capability_conversation} does not match tool runtime owner {runtime_conversation}"
            ),
            Self::Context(message) => write!(f, "context configuration failed: {message}"),
            Self::RuntimeClientAlreadyBound { conversation_id } => write!(
                f,
                "the runtime identity of conversation {conversation_id} is already bound to a Runtime Client host"
            ),
        }
    }
}

impl std::error::Error for HostConstructionError {}

/// Whether one model snapshot can run under the session context policy.
///
/// A model whose context window cannot accommodate the policy reserve plus
/// the model output budget can never run an attempt. Rejecting it when the
/// session model is *constructed* or *set* is what keeps the per-attempt
/// context runtime construction infallible at admission, where there is no
/// caller left to report a failure to.
///
/// # Errors
///
/// Returns the engine configuration error.
fn validate_context_policy(
    policy: &SessionContextPolicy,
    model: &AttemptModelSnapshot,
) -> Result<(), crate::context::ContextError> {
    if policy.summary_output_cap == Some(0) {
        return Err(crate::context::ContextError::new(
            crate::context::ContextErrorKind::InvalidConfiguration,
            "summary_output_cap must be positive when present",
        ));
    }
    policy
        .config_for_window(model.primary().context_window())
        .soft_input_limit(model.primary().max_output_tokens())?;
    let summary = match policy.summary_output_cap {
        Some(cap) => model.summary_invocation().with_output_cap(cap),
        None => model.summary_invocation().clone(),
    };
    policy
        .config_for_window(summary.context_window())
        .soft_input_limit(summary.max_output_tokens())?;
    Ok(())
}

/// Projects a model-resolution failure into the protocol error model.
///
/// Resolution errors never carry credential material: the catalog names an
/// environment variable at most.
fn invalid_model(error: &ModelInvocationError) -> RuntimeClientError {
    RuntimeClientError::InvalidModelConfiguration {
        message: error.to_string(),
    }
}

/// The shared context-plane pieces of the host.
///
/// These are the **session-owned static** pieces: the token estimator, the
/// checkpoint store, the Agent Status composer, and the context policy
/// (reserve tokens, keep-recent target, summary output cap). They persist
/// across attempts, and the model path and the Runtime Client projection
/// share one composer.
///
/// The context *window* is deliberately absent: it belongs to the model, so
/// each attempt derives its [`ContextRuntime`] from this policy plus that
/// attempt's immutable model snapshot. No window captured at process start
/// can survive a session model change.
#[derive(Clone)]
pub struct RuntimeClientContextConfig {
    /// The static session-owned context policy.
    pub policy: SessionContextPolicy,
    /// The deterministic token estimator.
    pub estimator: Arc<dyn TokenEstimator>,
    /// The checkpoint store.
    pub checkpoint_store: Arc<dyn ContextCheckpointStore>,
    /// The Agent Status composer shared by the model path and the Runtime
    /// Client projection.
    pub status_composer: AgentStatusComposer,
}

/// The construction-time configuration of one Runtime Client host.
///
/// # One conversation authority
///
/// There is deliberately no `conversation_id` field: the
/// [`ConversationToolRuntime`] is the single authority for the conversation
/// identity at this boundary, and the host derives its identity from
/// [`ConversationToolRuntime::conversation_id`]. A host whose conversation
/// identity disagrees with the runtime it coordinates is therefore not
/// representable, rather than rejected by an equality check.
pub struct RuntimeClientHostConfig {
    /// The agent executed by attempts of this runtime.
    pub agent_id: AgentId,
    /// The session's authoritative model state: the binding registry plus
    /// the initial desired configuration, already resolved and validated.
    ///
    /// This is the one model authority of the conversation. Attempts freeze
    /// snapshots of it; a client updates it through `model_set`; nothing
    /// else in the process resolves a provider binding.
    pub model: SessionModelState,
    /// The per-conversation IANA timezone, when known.
    pub timezone: Option<Tz>,
    /// The shared context-plane pieces.
    pub context: RuntimeClientContextConfig,
    /// The conversation tool runtime (owns the canonical mailbox and the
    /// authoritative background registry).
    pub tool_runtime: ConversationToolRuntime,
    /// The capability coordinator (owns the active capability snapshot).
    pub capability: CapabilityCoordinator,
    /// The runtime clock stamping client-submitted inbound messages; the
    /// system clock is used when omitted.
    pub clock: Option<Arc<dyn RuntimeClock>>,
    /// The canonical conversation history the host starts from.
    pub initial_messages: Vec<MessageBlock>,
    /// The bounded pre-M8 replay retention; the default is used when
    /// omitted.
    pub replay_limit: Option<usize>,
}

/// The host-owned current attempt handle.
struct CurrentAttempt {
    /// The attempt identity.
    attempt_id: AttemptId,
    /// The attempt cancellation trigger observed by the loop.
    cancellation: AgentCancellation,
}

/// The host-owned attachment state.
pub(crate) struct AttachmentState {
    /// The attachment identity.
    attachment_id: AttachmentId,
    /// The registered subscriber of the attachment, when it subscribed.
    subscriber_id: Option<u64>,
}

/// The one synchronized host state (the linearization owner).
struct HostState {
    /// The Runtime Client projection: snapshot read model, cursor,
    /// bounded replay, subscribers.
    projection: RuntimeClientProjection,
    /// The session's authoritative mutable model state.
    ///
    /// It lives under the *same* lock that owns attempt admission and
    /// projection publication, so a model update and an attempt admission
    /// can never interleave ambiguously: whichever acquires the lock first
    /// linearizes first.
    model: SessionModelState,
    /// The canonical conversation history between attempts. During an
    /// attempt the loop owns its working copy; at settlement the host
    /// replaces this value with the authoritative result messages.
    canonical_history: Vec<MessageBlock>,
    /// The current attempt slot (None = idle).
    current_attempt: Option<CurrentAttempt>,
    /// The at-most-one active attachment of Protocol v1.
    attachment: Option<AttachmentState>,
    /// Whether shutdown was accepted: no further inbound admission, no
    /// further attempt admission; the current attempt continues.
    shutting_down: bool,
    /// The next attachment identity sequence.
    next_attachment_seq: u64,
    /// The next attempt identity sequence.
    next_attempt_seq: u64,
    /// The next client-inbound message identity sequence.
    next_inbound_seq: u64,
}

impl HostState {
    /// Applies every queued pending observation in queue order.
    fn apply_pending(&mut self, pending: &PendingObservations) {
        for observation in pending.drain() {
            self.projection.apply(observation);
        }
    }
}

/// The tiny synchronization boundary between authoritative subsystems and
/// the host.
///
/// The mailbox, the background registry, and the capability coordinator all
/// fire their observers while their own lock is held. None of them may take
/// the host lock from there (see the lock-order graph in the module
/// documentation), so each appends an immutable observation here and wakes
/// the host worker. Every host lock acquisition drains this queue first, so
/// queued observations fold in enqueue order.
///
/// This type is the leaf of the lock graph: it owns one mutex over a
/// `VecDeque` plus a `Notify` and calls nothing.
///
/// It is also the observation worker's rendezvous point. The worker holds
/// `Arc<PendingObservations>` — never `Arc<HostInner>` across an await — so
/// this queue, not the host, is what keeps the worker's wait alive. When
/// `HostInner` is dropped it [`close`](PendingObservations::close)s the
/// queue, which is the worker's terminal condition.
struct PendingObservations {
    /// The FIFO observation queue.
    queue: Mutex<VecDeque<Observation>>,
    /// Wakes the worker task on every push and on close.
    notify: tokio::sync::Notify,
    /// Set exactly once, by `HostInner::drop`. Terminal: no further
    /// observation is accepted and the worker exits.
    closed: AtomicBool,
    /// Test-only worker-exit signal, so worker termination is observable
    /// deterministically instead of by timeout.
    #[cfg(test)]
    worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl PendingObservations {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            #[cfg(test)]
            worker_exit: Mutex::new(None),
        }
    }

    fn push(&self, observation: Observation) {
        if self.closed.load(Ordering::Acquire) {
            // Projection teardown is terminal: never queue an observation
            // that nothing will ever fold.
            return;
        }
        self.queue
            .lock()
            .expect("pending observation queue lock poisoned")
            .push_back(observation);
        self.notify.notify_one();
    }

    fn drain(&self) -> Vec<Observation> {
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
    async fn wait(&self) {
        self.notify.notified().await;
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The terminal close, performed exactly once by `HostInner::drop`.
    ///
    /// No concurrent producer can exist: every producer reaches this queue
    /// through an upgraded `Arc<HostInner>`, and a live upgrade would have
    /// prevented the drop that calls this.
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.queue
            .lock()
            .expect("pending observation queue lock poisoned")
            .clear();
        self.notify.notify_one();
    }

    /// Installs the test-only worker-exit signal.
    #[cfg(test)]
    fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        *self
            .worker_exit
            .lock()
            .expect("worker exit probe lock poisoned") = Some(sender);
    }

    /// Fires the test-only worker-exit signal, once.
    #[cfg(test)]
    fn signal_worker_exit(&self) {
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

/// The shared host state.
pub(crate) struct HostInner {
    conversation_id: ConversationId,
    agent_id: AgentId,
    timezone: Option<Tz>,
    context: RuntimeClientContextConfig,
    tool_runtime: ConversationToolRuntime,
    mailbox: ConversationInboundMailbox,
    capability: CapabilityCoordinator,
    clock: Arc<dyn RuntimeClock>,
    /// The one synchronization boundary.
    state: Mutex<HostState>,
    /// The subsystem-observation queue (see [`PendingObservations`]).
    ///
    /// Shared with the observation worker by `Arc`, so the worker can wait
    /// on it without owning this `HostInner`.
    pending: Arc<PendingObservations>,
    /// Whether the projection worker task was spawned.
    worker_started: AtomicBool,
}

/// Releasing the last semantic owner of a host closes its observation
/// queue, which is the observation worker's terminal condition.
///
/// This is the only teardown action: it takes no host lock (the host is
/// already unreachable), joins nothing, and publishes nothing.
impl Drop for HostInner {
    fn drop(&mut self) {
        self.pending.close();
    }
}

impl HostInner {
    /// Acquires the one synchronization boundary, applying queued pending
    /// observations first so every state read observes every queued fact.
    fn lock_state(&self) -> MutexGuard<'_, HostState> {
        let mut guard = self
            .state
            .lock()
            .expect("runtime client host lock poisoned");
        guard.apply_pending(&self.pending);
        guard
    }

    /// Builds the `ContextRuntime` of one admitted attempt.
    ///
    /// The engine window and the summary invocation both come from that
    /// attempt's immutable model snapshot, so an attempt on a 32k model
    /// never plans compaction with a previously selected 128k window.
    ///
    /// # Errors
    ///
    /// Returns the engine construction error when the session context policy
    /// leaves no positive input budget under this attempt's window.
    fn context_runtime(
        &self,
        model: &AttemptModelSnapshot,
    ) -> Result<ContextRuntime, crate::context::ContextError> {
        ContextRuntime::for_attempt(
            self.context.policy,
            Arc::clone(&self.context.estimator),
            Arc::clone(&self.context.checkpoint_store),
            self.context.status_composer.clone(),
            model,
        )
    }

    /// Spawns the projection worker: folds queued subsystem observations
    /// promptly so subscribed clients observe mailbox, background, and
    /// capability facts without sending requests.
    ///
    /// The worker exists because authoritative subsystems only *enqueue*
    /// (see the lock-order graph): something must take the host lock to
    /// fold what they enqueued. Correctness never depends on the worker —
    /// every host lock acquisition drains the queue first, so a request
    /// path always observes queued facts — only promptness for an idle
    /// subscriber does.
    ///
    /// # Lifetime
    ///
    /// The worker never owns the host. It captures `Weak<HostInner>` plus
    /// an `Arc<PendingObservations>` — the minimal wait state — and it
    /// upgrades the weak handle only inside a folding step, never across
    /// an await. A parked worker therefore holds no strong reference, so it
    /// cannot keep a host alive that has no semantic owner left.
    ///
    /// Termination is deterministic, not timed: dropping the last
    /// `Arc<HostInner>` runs `HostInner::drop`, which closes the pending
    /// queue and wakes the worker; the worker observes the closed queue and
    /// exits. The upgrade check is a second, independent exit path.
    fn ensure_worker(self: &Arc<Self>) {
        // Construction may happen outside a runtime; a later call from a
        // request path spawns the worker instead.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            loop {
                pending.wait().await;
                if pending.is_closed() {
                    break;
                }
                // The strong handle exists only inside this block, so it is
                // never held across the await above.
                {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    let _state = inner.lock_state();
                }
            }
            #[cfg(test)]
            pending.signal_worker_exit();
        });
    }

    /// Runs one attempt to settlement against the coordinator-owned
    /// cancellation trigger (the same handle `cancel_current_attempt`
    /// requests cancellation on).
    async fn run_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        initial_messages: Vec<MessageBlock>,
        fresh: Option<FreshInboundTurn>,
        cancellation: &AgentCancellation,
        model: AttemptModelSnapshot,
    ) -> crate::agent::AgentExecutionResult {
        let lease = self.capability.acquire_attempt_lease();
        let observer = HostObserver::new(self);
        // The context runtime is derived from the frozen snapshot, so the
        // attempt's window, output budget, and summary invocation all agree
        // with the model it was admitted with.
        let context_runtime = self
            .context_runtime(&model)
            .expect("admission validated this model against the session context policy");
        let request = AgentExecutionRequest {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            attempt_id,
            initial_messages,
            initial_turn_trigger: match fresh {
                Some(fresh) => InitialTurnTrigger::FreshInbound(fresh),
                None => InitialTurnTrigger::Continuation,
            },
            timezone: self.timezone,
            model,
        };
        let mut execution = AgentExecution::new(
            request,
            lease,
            cancellation,
            context_runtime,
            &self.tool_runtime,
        )
        // Neither rejection is reachable: `conversation_id` *is* the tool
        // runtime's own identity (the host has no independent conversation
        // authority to disagree with it), and construction validated the
        // coordinator against that same runtime.
        .expect("the host derives its conversation identity from this tool runtime");
        execution.observe(&observer);
        execution.run().await
    }

    /// The settlement path of one attempt: commit the authoritative
    /// history, clear the current-attempt slot, then admit the next
    /// attempt when the mailbox holds pending work.
    ///
    /// The result is consumed by value: its `messages` become the
    /// authoritative canonical history.
    #[allow(clippy::needless_pass_by_value)]
    fn finish_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        result: crate::agent::AgentExecutionResult,
    ) {
        {
            let mut state = self.lock_state();
            debug_assert_eq!(
                state.projection.snapshot_ref().messages,
                result.messages,
                "the projection read model must mirror the authoritative attempt history"
            );
            state.canonical_history = result.messages;
            if state
                .current_attempt
                .as_ref()
                .is_some_and(|current| current.attempt_id == attempt_id)
            {
                state.current_attempt = None;
            }
        }
        self.admit_next_attempt();
    }

    /// Admits one attempt when the runtime is idle and the mailbox holds
    /// pending work.
    ///
    /// Linearization: the idle observation, the finite mailbox drain, the
    /// canonical-history commits, and the current-attempt publication all
    /// share the one host lock (the mailbox drain fires its observer only
    /// into the leaf pending queue, never back into this lock). After the
    /// publication the lock is released and the attempt task is spawned.
    fn admit_next_attempt(self: &Arc<Self>) {
        let mut state = self.lock_state();
        if state.shutting_down || state.current_attempt.is_some() {
            return;
        }
        let Some(batch) = self.mailbox.drain() else {
            return;
        };
        // The drain queued its observation; apply it before committing the
        // drained messages so the client observes the drain fact before
        // the commit facts.
        state.apply_pending(&self.pending);
        let mut fresh_ids = Vec::with_capacity(batch.items().len());
        for item in batch.into_items() {
            let message_id = item.message().id.clone();
            let block = MessageBlock::User(item.into_message());
            state.canonical_history.push(block.clone());
            state.projection.apply(Observation::Committed {
                attempt_id: None,
                block,
            });
            fresh_ids.push(message_id);
        }
        let fresh = FreshInboundTurn::new(fresh_ids)
            .expect("a drained mailbox batch forms one fresh inbound turn");
        let attempt_id = AttemptId::new(format!(
            "{}-attempt-{}",
            self.conversation_id, state.next_attempt_seq
        ));
        state.next_attempt_seq = state.next_attempt_seq.saturating_add(1);
        // The coordinator-owned cancellation handle is the exact trigger
        // `cancel_current_attempt` requests on: the attempt task runs
        // against the same signal, so protocol cancellation always
        // reaches the loop.
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        state.current_attempt = Some(CurrentAttempt {
            attempt_id: attempt_id.clone(),
            cancellation: cancellation.clone(),
        });
        state.projection.apply(Observation::AttemptAdmitted {
            attempt_id: attempt_id.clone(),
        });
        let initial_messages = state.canonical_history.clone();
        // The attempt model snapshot is taken at exactly this admission
        // linearization boundary, under the same lock that publishes the
        // attempt. A `model_set` that linearizes before this point is
        // observed by the attempt; one that linearizes after it affects only
        // future attempts.
        let model = state.model.snapshot();
        state.projection.apply(Observation::AttemptModelFrozen {
            attempt_id: attempt_id.clone(),
            model: Box::new(model.view()),
        });
        drop(state);
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let result = inner
                .run_attempt(
                    attempt_id.clone(),
                    initial_messages,
                    Some(fresh),
                    &cancellation,
                    model,
                )
                .await;
            inner.finish_attempt(attempt_id, result);
        });
    }
}

/// The Runtime Client host of one conversation.
///
/// Construct one host per conversation runtime instance; the host installs
/// the observation seams on the mailbox, the background registry, and the
/// capability coordinator exactly once. The host is cheaply cloneable and
/// all clones share one state.
#[derive(Clone)]
pub struct RuntimeClientHost {
    pub(crate) inner: Arc<HostInner>,
}

impl core::fmt::Debug for RuntimeClientHost {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RuntimeClientHost")
            .field("conversation_id", &self.inner.conversation_id)
            .finish()
    }
}

impl RuntimeClientHost {
    /// Creates the host and installs the observation seams.
    ///
    /// # One conversation authority
    ///
    /// The conversation identity of the host *is*
    /// [`ConversationToolRuntime::conversation_id`]. The configuration
    /// carries no conversation id of its own, so the host's identity, the
    /// canonical mailbox, the authoritative background registry, and the
    /// Runtime Client binding identity all name one conversation by
    /// construction. Every conversation-scoped value this host derives —
    /// the projection's conversation, the `initialized` result, generated
    /// inbound message ids, generated attempt ids, and every
    /// [`AgentExecutionRequest`] it issues — uses that one identity.
    ///
    /// The capability coordinator is a *separate* authoritative identity,
    /// so it is still validated explicitly against the runtime.
    ///
    /// # One host per runtime identity
    ///
    /// Construction claims the one-time Runtime Client binding of the
    /// conversation tool runtime and of the capability coordinator. Both are
    /// `Clone`, and every clone shares one binding, so passing a cloned
    /// runtime bundle to a second `new` is rejected with
    /// [`HostConstructionError::RuntimeClientAlreadyBound`] rather than
    /// silently replacing the first host's observation seams.
    ///
    /// The binding lasts for the runtime identity's lifetime and is not
    /// released when the bound host is dropped: reconnect belongs to
    /// attachments (detach, then a fresh
    /// [`RuntimeClientEndpoint`](super::endpoint::RuntimeClientEndpoint)
    /// `initialize`), not to host reconstruction. A new host requires a new
    /// `ConversationToolRuntime` identity.
    ///
    /// # Construction order
    ///
    /// Every fallible validation runs *before* the binding claim, and every
    /// step after it is infallible, so a rejected construction has no
    /// semantic side effect at all and a claimed runtime is never left
    /// unusable. The claim is the ownership-commit boundary.
    ///
    /// # Errors
    ///
    /// Returns [`HostConstructionError::OwnershipMismatch`] when the
    /// capability coordinator and the conversation tool runtime do not
    /// share the same conversation/workspace ownership domain,
    /// [`HostConstructionError::RuntimeClientAlreadyBound`] when either is
    /// already bound to a Runtime Client host, and
    /// [`HostConstructionError::Context`] when the context engine
    /// configuration is impossible.
    pub fn new(config: RuntimeClientHostConfig) -> Result<Self, HostConstructionError> {
        // The one conversation authority at this boundary: every identity
        // this host publishes or derives comes from the tool runtime it
        // coordinates, so host and runtime cannot disagree.
        let conversation_id = config.tool_runtime.conversation_id().clone();

        // ---- Fallible validation: nothing below is observable yet. ----
        let snapshot = config.capability.current_snapshot();
        // The coordinator is a separate authoritative identity, so it is
        // still validated explicitly against the runtime's identity.
        if snapshot.conversation_id() != &conversation_id
            || snapshot.workspace_root() != config.tool_runtime.workspace().root()
        {
            return Err(HostConstructionError::OwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: conversation_id,
            });
        }
        // The initial session model must be able to run under the session
        // context policy. Validating here (and again in `model_set`) is what
        // makes the per-attempt context runtime construction infallible at
        // admission, where there is no caller left to report to.
        validate_context_policy(&config.context.policy, &config.model.snapshot())
            .map_err(|error| HostConstructionError::Context(error.message))?;

        // ---- Ownership commit: the one-time binding claim. ----
        //
        // The runtime identity is claimed first because it is the canonical
        // mailbox/background identity this host coordinates. If the
        // coordinator is already bound, the runtime claim is released again:
        // a rejected construction must leave no trace, and this is the only
        // place a claim is ever released.
        if !config.tool_runtime.claim_runtime_client() {
            return Err(HostConstructionError::RuntimeClientAlreadyBound { conversation_id });
        }
        if !config.capability.claim_runtime_client() {
            config.tool_runtime.release_runtime_client_claim();
            return Err(HostConstructionError::RuntimeClientAlreadyBound { conversation_id });
        }

        // ---- Infallible wiring: from here construction always succeeds. ----
        let replay_limit = config
            .replay_limit
            .unwrap_or(super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
        let mailbox = config.tool_runtime.mailbox();
        let clock = config
            .clock
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn RuntimeClock>);
        let mut projection = RuntimeClientProjection::new(
            conversation_id.clone(),
            config.initial_messages.clone(),
            capability_view(&snapshot),
            config.model.view(),
            replay_limit,
        );
        // Mirror the pre-existing authoritative background records.
        for existing in config.tool_runtime.background().all_snapshots() {
            projection.apply(Observation::Background(existing));
        }
        let inner = Arc::new(HostInner {
            conversation_id,
            agent_id: config.agent_id,
            timezone: config.timezone,
            context: config.context,
            tool_runtime: config.tool_runtime,
            mailbox,
            capability: config.capability,
            clock,
            state: Mutex::new(HostState {
                projection,
                model: config.model,
                canonical_history: config.initial_messages,
                current_attempt: None,
                attachment: None,
                shutting_down: false,
                next_attachment_seq: 0,
                next_attempt_seq: 0,
                next_inbound_seq: 0,
            }),
            pending: Arc::new(PendingObservations::new()),
            worker_started: AtomicBool::new(false),
        });
        let observer: Arc<HostObserver> = Arc::new(HostObserver::new(&inner));
        inner.mailbox.install_observer(observer.clone());
        inner
            .tool_runtime
            .background()
            .install_observer(observer.clone());
        inner.capability.install_observer(observer);
        inner.ensure_worker();
        Ok(Self { inner })
    }

    /// Creates the host with the test-only projection linearization hooks
    /// installed. Only available under `#[cfg(test)]`; never used by
    /// production code.
    #[cfg(test)]
    pub(crate) fn with_probe(
        config: RuntimeClientHostConfig,
        probe: super::test_sync::ProjectionProbe,
    ) -> Result<Self, HostConstructionError> {
        let host = Self::new(config)?;
        host.inner
            .state
            .lock()
            .expect("host lock")
            .projection
            .install_probe(probe);
        Ok(host)
    }

    /// The conversation identity of this host.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.inner.conversation_id
    }

    /// Creates the transport-neutral semantic endpoint of this runtime.
    ///
    /// The endpoint is the boundary a transport (Issue #38 stdio/JSONL,
    /// Issue #36 WebSocket) wraps: it accepts every
    /// [`RuntimeClientRequest`](super::types::RuntimeClientRequest),
    /// including `initialize`, and returns the correlated
    /// [`RuntimeClientResponse`](super::types::RuntimeClientResponse). A
    /// transport therefore never performs protocol negotiation, attachment
    /// admission, identity allocation, or replacement/rejection semantics
    /// itself.
    #[must_use]
    pub fn endpoint(&self) -> super::endpoint::RuntimeClientEndpoint {
        super::endpoint::RuntimeClientEndpoint::new(self.clone())
    }

    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
    ///
    /// This is an internal-shaped primitive, not the semantic protocol
    /// entry point. Transports must go through
    /// [`RuntimeClientEndpoint::handle_request`](super::endpoint::RuntimeClientEndpoint::handle_request)
    /// with an `initialize` request, which owns the orchestration below.
    ///
    /// Protocol v1 allows at most one active attachment; a second
    /// simultaneous attach fails deterministically and never evicts the
    /// first. The returned snapshot and cursor are linearized with the
    /// admission under the one synchronization boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when an
    /// attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub fn attach(
        &self,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        if protocol_version != RUNTIME_CLIENT_PROTOCOL_VERSION_V1 {
            return Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
                requested: protocol_version,
            });
        }
        self.inner.ensure_worker();
        let mut state = self.inner.lock_state();
        if let Some(existing) = &state.attachment {
            return Err(RuntimeClientError::AttachmentInUse {
                existing_attachment_id: existing.attachment_id.clone(),
            });
        }
        let (snapshot, cursor) = state.projection.snapshot()?;
        state.next_attachment_seq = state.next_attachment_seq.saturating_add(1);
        let attachment_id = AttachmentId::new(format!("attachment-{}", state.next_attachment_seq));
        state.attachment = Some(AttachmentState {
            attachment_id: attachment_id.clone(),
            subscriber_id: None,
        });
        drop(state);
        let attachment =
            super::attachment::RuntimeAttachment::new(attachment_id.clone(), self.inner.clone());
        Ok((
            attachment,
            RuntimeClientResult::Initialized {
                attachment_id,
                conversation_id: self.inner.conversation_id.clone(),
                agent_id: self.inner.agent_id.clone(),
                snapshot,
                cursor,
            },
        ))
    }

    /// Releases one attachment. Idempotent: a second detach (or an
    /// attachment drop after an explicit detach) is a no-op. Detach is
    /// never cancellation and never shutdown.
    ///
    /// # Panics
    ///
    /// Panics only if the host lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn detach(&self, attachment_id: &AttachmentId) {
        let mut state = self.inner.lock_state();
        if state
            .attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id)
        {
            let attachment = state
                .attachment
                .take()
                .expect("the attachment identity was just checked");
            if let Some(subscriber_id) = attachment.subscriber_id {
                state.projection.remove_subscriber(subscriber_id);
            }
        }
    }

    /// Submits one inbound user message.
    ///
    /// The runtime owns authoritative metadata: the message identity, the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all runtime-assigned. Success means accepted/admitted, never
    /// assistant-finished. When the runtime is idle, admission starts an
    /// attempt whose first turn observes the message; when an attempt is
    /// running, the message waits in the authoritative mailbox for the
    /// next safe-boundary drain.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidRequest`] for empty content,
    /// [`RuntimeClientError::RuntimeShutdown`] after shutdown, and
    /// [`RuntimeClientError::InvalidState`] for a mailbox admission
    /// failure.
    pub fn submit_inbound(
        &self,
        content: Vec<UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if content.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "inbound content must not be empty".to_owned(),
            });
        }
        let (message_id, timestamp) = {
            let mut state = self.inner.lock_state();
            if state.shutting_down {
                return Err(RuntimeClientError::RuntimeShutdown);
            }
            state.next_inbound_seq = state.next_inbound_seq.saturating_add(1);
            (
                MessageId::new(format!(
                    "{}-inbound-{}",
                    self.inner.conversation_id, state.next_inbound_seq
                )),
                self.inner.clock.now(),
            )
        };
        let message = UserMessageBlock {
            id: message_id.clone(),
            content,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(timestamp),
        };
        let sequence = self.inner.mailbox.enqueue(message).map_err(|error| {
            RuntimeClientError::InvalidState {
                message: error.to_string(),
            }
        })?;
        self.inner.ensure_worker();
        self.inner.admit_next_attempt();
        Ok(RuntimeClientResult::InboundAccepted {
            message_id,
            inbound_sequence: sequence,
        })
    }

    /// Requests cancellation of the current attempt.
    ///
    /// Acceptance is not terminal settlement: actual settlement remains
    /// owned by the Agent Loop and is observed asynchronously. The
    /// deciding observation and the cancellation request share the one
    /// synchronization boundary, so cancel-current, snapshot, the
    /// terminal Runtime Client event, and the next admitted attempt
    /// linearize deterministically.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NoCurrentAttempt`] when no attempt
    /// is currently cancellable.
    ///
    /// # Panics
    ///
    /// Panics only if the host lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        let state = self.inner.lock_state();
        let cancellable = match &state.current_attempt {
            Some(current) => matches!(
                &state.projection.snapshot_ref().attempt,
                Some(attempt)
                    if attempt.attempt_id == current.attempt_id
                        && !matches!(attempt.phase, RuntimeClientAttemptPhase::Settled { .. })
            ),
            None => false,
        };
        if !cancellable {
            return Err(RuntimeClientError::NoCurrentAttempt);
        }
        let current = state
            .current_attempt
            .as_ref()
            .expect("the cancellable attempt exists");
        let attempt_id = current.attempt_id.clone();
        current.cancellation.cancel();
        Ok(RuntimeClientResult::AttemptCancellationAccepted { attempt_id })
    }

    /// Reads the authoritative snapshot and its cursor, linearized
    /// together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted. After that point the projection can no longer
    /// fold authoritative transitions, so the failure is reported
    /// explicitly rather than by handing back a read model that silently
    /// stopped tracking the runtime.
    pub fn snapshot(
        &self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError> {
        let state = self.inner.lock_state();
        state.projection.snapshot()
    }

    /// Subscribes one attachment to the observation stream after a
    /// serviceable cursor.
    ///
    /// The returned subscription receives every subsequently published
    /// event (and the retained replay gap) or fails explicitly with
    /// [`RuntimeClientError::ResyncRequired`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NotAttached`] for an unknown
    /// attachment identity and [`RuntimeClientError::ResyncRequired`] for
    /// an unserviceable cursor.
    ///
    /// # Panics
    ///
    /// Panics only if the host lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn subscribe_events(
        &self,
        attachment_id: &AttachmentId,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(EventSubscription, RuntimeClientResult), RuntimeClientError> {
        self.inner.ensure_worker();
        let mut state = self.inner.lock_state();
        let previous_subscriber = match &state.attachment {
            Some(attachment) if attachment.attachment_id == *attachment_id => {
                attachment.subscriber_id
            }
            _ => return Err(RuntimeClientError::NotAttached),
        };
        if let Some(subscriber_id) = previous_subscriber {
            state.projection.remove_subscriber(subscriber_id);
        }
        let (subscriber_id, notify) = state.projection.subscribe(after_cursor)?;
        state
            .attachment
            .as_mut()
            .expect("the attachment identity was just checked")
            .subscriber_id = Some(subscriber_id);
        drop(state);
        Ok((
            EventSubscription {
                inner: Arc::new(SubscriptionInner {
                    host: self.inner.clone(),
                    subscriber_id,
                    notify,
                }),
            },
            RuntimeClientResult::Subscribed { after_cursor },
        ))
    }

    /// Reads the active capability projection (the one semantic
    /// implementation shared with the snapshot).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the
    /// observation stream is over.
    pub fn capability(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        let state = self.inner.lock_state();
        let snapshot = state.projection.snapshot_ref_checked()?;
        Ok(RuntimeClientResult::Capability {
            capabilities: snapshot.capabilities.clone(),
        })
    }

    /// Reads the safe public model catalog.
    ///
    /// This is the query that makes client-side `models.json` reading
    /// unnecessary: it carries model references, protocols, limits, declared
    /// and effective capabilities, reasoning profile identities with their
    /// semantic enabled state, and the redacted credential *source*. It
    /// never carries a credential value, an adapter, or a provider HTTP
    /// client.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub fn model_catalog(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        let state = self.inner.lock_state();
        state.projection.snapshot_ref_checked()?;
        Ok(RuntimeClientResult::ModelCatalog {
            catalog: state.model.catalog_view(),
        })
    }

    /// Reads the authoritative session model state.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        let state = self.inner.lock_state();
        state.projection.snapshot_ref_checked()?;
        Ok(RuntimeClientResult::Model {
            model: Box::new(state.model.view()),
        })
    }

    /// Replaces the authoritative session model configuration.
    ///
    /// # Linearization
    ///
    /// The whole operation — resolution, validation, state replacement, and
    /// the single projection publication — happens under the one host lock
    /// that also owns attempt admission. An update therefore either
    /// linearizes before an admission (and that attempt observes it) or
    /// after it (and only later attempts observe it). There is no third
    /// possibility and no timing assumption.
    ///
    /// # Transactionality
    ///
    /// A rejected update changes nothing: the session keeps its previous
    /// configuration, no cursor is allocated, and no event is published.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidModelConfiguration`] when the
    /// configuration cannot be resolved against the catalog or cannot run
    /// under the session context policy, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let mut state = self.inner.lock_state();
        state.projection.snapshot_ref_checked()?;
        // Resolve into a scratch copy first: `SessionModelState::apply` is
        // itself transactional, and the context-policy check runs against the
        // *candidate* snapshot before anything is published.
        let mut candidate = state.model.clone();
        candidate
            .apply(config)
            .map_err(|error| invalid_model(&error))?;
        validate_context_policy(&self.inner.context.policy, &candidate.snapshot()).map_err(
            |error| RuntimeClientError::InvalidModelConfiguration {
                message: format!(
                    "the selected model cannot run under the session context policy: {}",
                    error.message
                ),
            },
        )?;
        let view = candidate.view();
        state.model = candidate;
        state.projection.apply(Observation::SessionModelChanged {
            model: Box::new(view.clone()),
        });
        Ok(RuntimeClientResult::ModelSet {
            model: Box::new(view),
        })
    }

    /// Inspects one background execution through the authoritative
    /// registry.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownBackgroundExecution`] for an
    /// unknown execution identity.
    pub fn background_status(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let Some(snapshot) = self.inner.tool_runtime.background().snapshot(execution_id) else {
            return Err(RuntimeClientError::UnknownBackgroundExecution {
                execution_id: execution_id.clone(),
            });
        };
        Ok(RuntimeClientResult::BackgroundStatus {
            execution: background_view(&snapshot),
        })
    }

    /// Requests cancellation of one background execution through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownBackgroundExecution`] for an
    /// unknown execution identity.
    pub fn background_cancel(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let Some(snapshot) = self.inner.tool_runtime.background().cancel(execution_id) else {
            return Err(RuntimeClientError::UnknownBackgroundExecution {
                execution_id: execution_id.clone(),
            });
        };
        Ok(RuntimeClientResult::BackgroundCancelAccepted {
            execution: background_view(&snapshot),
        })
    }

    /// Accepts the local-runtime shutdown request.
    ///
    /// Shutdown is not detach and not cancellation: the current attempt
    /// continues to its settlement, semantic runtime work is never
    /// mutated, and no further inbound admission occurs. The acceptance is
    /// published as the terminal-agnostic [`crate::event::RuntimeClientEvent::RuntimeShutdown`]
    /// observation.
    #[must_use]
    pub fn shutdown(&self) -> RuntimeClientResult {
        let mut state = self.inner.lock_state();
        state.shutting_down = true;
        state.projection.apply(Observation::Shutdown);
        RuntimeClientResult::ShutdownAccepted
    }
}

/// The observation seam implementations bridging the authoritative
/// runtime owners into the one projection boundary.
///
/// Two shapes exist, and which one a callback uses is decided purely by
/// whether the calling subsystem holds its own lock (see the lock-order
/// graph in the module documentation):
///
/// - [`HostObserver::enqueue`] — for the mailbox, the background registry,
///   and the capability coordinator, all of which fire while holding their
///   authoritative lock. The observation is appended to the leaf queue and
///   the host worker is woken. These paths never acquire `HostState`.
/// - [`HostObserver::apply_direct`] — for `AgentExecution`, which holds no
///   lock when it observes.
///
/// # Lifetime
///
/// The observer is **non-owning**. Authoritative subsystems keep it alive
/// (`Arc<dyn InboundObserver>` and friends are unchanged), but it holds
/// only a `Weak<HostInner>`, so the edge
/// `HostInner -> subsystem -> Arc<HostObserver> -> HostInner` is broken:
/// installing an observation seam never extends a host's lifetime.
///
/// Every callback upgrades the weak handle and returns without publishing
/// when the upgrade fails — the Runtime Client projection simply no longer
/// exists. That is never an error for the subsystem: the mailbox, the
/// background registry, and the capability coordinator stay authoritative
/// whether or not a projection is observing them. The upgrade is transient
/// and confined to the callback, so an observer can neither resurrect nor
/// prolong a host.
pub(crate) struct HostObserver {
    host: Weak<HostInner>,
}

impl HostObserver {
    /// Creates the non-owning observer of one host.
    fn new(host: &Arc<HostInner>) -> Self {
        Self {
            host: Arc::downgrade(host),
        }
    }

    /// Applies one observation directly under the host lock, applying
    /// queued pending observations first so total order is preserved.
    ///
    /// Only legal from a caller that holds no authoritative subsystem
    /// lock.
    fn apply_direct(&self, observation: Observation) {
        let Some(inner) = self.host.upgrade() else {
            return;
        };
        let mut state = inner.lock_state();
        state.projection.apply(observation);
    }

    /// Appends one observation to the leaf queue and wakes the host
    /// worker, without acquiring the host lock.
    ///
    /// This is the only shape legal from a subsystem observer that fires
    /// while its authoritative lock is held.
    fn enqueue(&self, observation: Observation) {
        let Some(inner) = self.host.upgrade() else {
            return;
        };
        inner.pending.push(observation);
    }
}

impl AgentExecutionObserver for HostObserver {
    fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent) {
        self.apply_direct(Observation::Event {
            attempt_id: attempt_id.clone(),
            event: event.clone(),
        });
    }

    fn observe_committed(&self, attempt_id: &AttemptId, block: &MessageBlock) {
        self.apply_direct(Observation::Committed {
            attempt_id: Some(attempt_id.clone()),
            block: block.clone(),
        });
    }

    fn observe_status(&self, observation: &AgentStatusObservation) {
        self.apply_direct(Observation::Status(observation.clone()));
    }
}

// The mailbox fires `on_enqueued`/`on_drained` while the mailbox lock is
// held, and `admit_next_attempt` drains the mailbox under the host lock:
// taking the host lock here would close the cycle. Enqueue only.
impl InboundObserver for HostObserver {
    fn on_enqueued(&self, item: &InboundItem) {
        self.enqueue(Observation::InboundEnqueued(item.clone()));
    }

    fn on_drained(&self, batch: &InboundBatch) {
        self.enqueue(Observation::InboundDrained(batch.clone()));
    }
}

// The registry fires `on_snapshot` while the registry lock is held, and
// `background_status`/`background_cancel` call into the registry from the
// host surface. Enqueue only, so no `HostState -> registry` ordering
// discipline is ever required of a caller.
impl BackgroundObserver for HostObserver {
    fn on_snapshot(&self, snapshot: &BackgroundExecutionSnapshot) {
        self.enqueue(Observation::Background(snapshot.clone()));
    }
}

// The coordinator fires `on_snapshot` while the capability state lock is
// held, with an attempt commit blocked behind it. Enqueue only, so an
// authoritative capability commit never waits on the host lock.
impl CapabilityObserver for HostObserver {
    fn on_snapshot(&self, snapshot: &crate::capabilities::CapabilitySnapshot) {
        self.enqueue(Observation::Capability(capability_view(snapshot)));
    }
}

/// One delivery of the Runtime Client observation stream.
///
/// Delivery is explicit in all four terminal shapes: a client can never
/// confuse "nothing yet" with "the stream ended" or, critically, with
/// "events were skipped".
// The event variant is the overwhelmingly common one and is produced once
// per delivered event; boxing it would add an allocation to every delivery
// to shrink a short-lived stack value.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum EventDelivery {
    /// The next event, at its published cursor. Cursors delivered to one
    /// subscription are strictly contiguous within the retained stream.
    Event(RuntimeClientProtocolEvent),
    /// Nothing has been published after the subscription's cursor yet.
    /// Only [`EventSubscription::try_next`] returns this.
    Pending,
    /// The subscription was released (detach, re-subscription, or a
    /// dropped handle). The stream is over.
    Closed,
    /// The subscriber fell behind the bounded retention: the events it
    /// still needed were evicted from the replay ring. This is reported
    /// explicitly instead of skipping the gap, and it is stable — the
    /// client must re-subscribe (or take a fresh snapshot) to continue.
    ResyncRequired {
        /// The cursor the subscription consumed through.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The cursor space is exhausted; nothing further will be published.
    Exhausted,
}

/// The shared registration of one event subscription.
///
/// Dropping the last handle removes the registration from the projection,
/// which is the same release an explicit detach performs.
struct SubscriptionInner {
    /// The host whose projection owns the registration.
    host: Arc<HostInner>,
    /// The opaque registration identity.
    subscriber_id: u64,
    /// The edge-triggered wakeup handle of this subscriber.
    notify: Arc<tokio::sync::Notify>,
}

impl Drop for SubscriptionInner {
    fn drop(&mut self) {
        let mut state = self.host.lock_state();
        state.projection.remove_subscriber(self.subscriber_id);
    }
}

/// The live delivery handle of one event subscription.
///
/// The handle owns **no** event buffer. It is a registration identity plus
/// a wakeup handle over the projection's one bounded replay ring: reads
/// pull the next retained event by cursor under the host lock. A stalled
/// consumer therefore costs one cursor, never a growing queue, and can
/// never make the runtime drop events silently — falling behind retention
/// surfaces as [`EventDelivery::ResyncRequired`].
///
/// Cloning shares one registration; the registration is released when the
/// last clone drops.
#[derive(Clone)]
pub struct EventSubscription {
    inner: Arc<SubscriptionInner>,
}

impl core::fmt::Debug for EventSubscription {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventSubscription")
            .field("subscriber_id", &self.inner.subscriber_id)
            .finish()
    }
}

impl EventSubscription {
    /// Polls the projection once for the next retained event.
    fn poll(&self) -> EventDelivery {
        let mut state = self.inner.host.lock_state();
        match state.projection.poll_subscriber(self.inner.subscriber_id) {
            SubscriberPoll::Event(event) => EventDelivery::Event(event),
            SubscriberPoll::Pending => EventDelivery::Pending,
            SubscriberPoll::Closed => EventDelivery::Closed,
            SubscriberPoll::Lagged {
                after_cursor,
                earliest_serviceable,
            } => EventDelivery::ResyncRequired {
                after_cursor,
                earliest_serviceable,
            },
            SubscriberPoll::Exhausted => EventDelivery::Exhausted,
        }
    }

    /// Waits for the next delivery of the observation stream.
    ///
    /// Never returns [`EventDelivery::Pending`]: it parks on the
    /// subscriber's wakeup handle until an event, a closure, a lag, or
    /// exhaustion is observable. Parking holds no lock.
    pub async fn next(&self) -> EventDelivery {
        loop {
            match self.poll() {
                EventDelivery::Pending => {}
                delivery => return delivery,
            }
            // `Notify::notify_one` stores one permit even with no waiter,
            // so a publication between the poll above and this await is
            // never missed.
            self.inner.notify.notified().await;
        }
    }

    /// Polls for the next delivery without waiting.
    #[must_use]
    pub fn try_next(&self) -> EventDelivery {
        self.poll()
    }
}

/// Convenience for tests: the host's canonical history accessor.
#[cfg(test)]
impl RuntimeClientHost {
    pub(crate) fn canonical_history(&self) -> Vec<MessageBlock> {
        self.inner
            .state
            .lock()
            .expect("host lock")
            .canonical_history
            .clone()
    }

    #[allow(dead_code)] // used by the race regression tests
    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("host lock")
            .current_attempt
            .is_some()
    }

    /// A non-owning handle to the shared host state, for lifetime tests.
    pub(crate) fn weak_inner(&self) -> Weak<HostInner> {
        Arc::downgrade(&self.inner)
    }

    /// Installs the deterministic worker-exit signal, for lifetime tests.
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        self.inner.pending.install_worker_exit_probe(sender);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use futures_util::future::BoxFuture;
    use tokio::sync::watch;

    use super::{
        EventDelivery, EventSubscription, HostConstructionError, RuntimeClientContextConfig,
        RuntimeClientHost, RuntimeClientHostConfig,
    };
    use crate::context::{
        AgentStatusClock, AgentStatusComposer, AgentStatusFact, AgentStatusRenderContext,
        AgentStatusSectionId, AgentStatusSectionProvider, ContextError, DefaultTokenEstimator,
        InMemoryCheckpointStore, TokenEstimator,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{
        ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelEventStream};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::fixture::scripted_session_model;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{AgentId, ConversationId, ToolCallId, ToolId};
    use crate::runtime::types::RuntimeClock;
    use crate::runtime_client::event::RuntimeClientEvent;
    use crate::runtime_client::snapshot::RuntimeClientAttemptPhase;
    use crate::runtime_client::types::{
        RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent, RuntimeClientRequest,
        RuntimeClientResult,
    };
    use crate::tools::background::{BackgroundDispatchOutcome, BackgroundLifecycle};
    use crate::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
    use crate::tools::types::{
        ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
        ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolReplayPolicy,
    };

    /// One scripted step of the gated adapter.
    enum GatedStep {
        /// Yield one canonical model event.
        Emit(ModelEvent),
        /// Wait until the shared watch releases, then continue without
        /// yielding; if the attempt cancellation fires first, fail with a
        /// cancelled model error like a real adapter.
        ParkUntilReleased(watch::Receiver<bool>),
    }

    /// A scripted cancellation-aware model adapter: one script per
    /// invocation, with deterministic park points.
    struct GatedAdapter {
        scripts: Mutex<VecDeque<VecDeque<GatedStep>>>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl GatedAdapter {
        fn new(scripts: Vec<Vec<GatedStep>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into_iter().map(VecDeque::from).collect()),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().expect("requests lock").clone()
        }
    }

    impl ModelAdapter for GatedAdapter {
        fn protocol(&self) -> ModelProtocol {
            ModelProtocol::OpenAiChatCompletions
        }

        fn stream(
            &self,
            request: ModelRequest,
            cancellation: CancellationSignal,
        ) -> ModelEventStream {
            self.requests.lock().expect("requests lock").push(request);
            let script = self
                .scripts
                .lock()
                .expect("scripts lock")
                .pop_front()
                .unwrap_or_default();
            Box::pin(futures_util::stream::unfold(
                (script, cancellation),
                |(mut script, cancellation)| async move {
                    loop {
                        match script.pop_front() {
                            None => return None,
                            Some(GatedStep::Emit(event)) => {
                                return Some((event, (script, cancellation)));
                            }
                            Some(GatedStep::ParkUntilReleased(mut release)) => {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        return Some((ModelEvent::Failed {
                                            error: ModelError {
                                                kind: ModelErrorKind::Cancelled,
                                                message: "cancelled while parked".to_owned(),
                                                retry_after_ms: None,
                                                provider_code: None,
                                            },
                                        }, (VecDeque::new(), cancellation)));
                                    }
                                    result = release.wait_for(|released| *released) => {
                                        result.expect("release channel stays open");
                                    }
                                }
                            }
                        }
                    }
                },
            ))
        }
    }

    fn model_release() -> (watch::Sender<bool>, watch::Receiver<bool>) {
        watch::channel(false)
    }

    /// A parking background executor: starts, waits for the release
    /// notify, then settles with a fixed result.
    struct ParkingBackgroundTool {
        #[allow(dead_code)] // the definition documents the tool identity
        definition: ToolDefinition,
        started: watch::Sender<bool>,
        release: Arc<tokio::sync::Notify>,
    }

    impl ParkingBackgroundTool {
        fn new() -> (Self, watch::Receiver<bool>, Arc<tokio::sync::Notify>) {
            let (started, started_rx) = watch::channel(false);
            let release = Arc::new(tokio::sync::Notify::new());
            (
                Self {
                    definition: ToolDefinition {
                        id: ToolId::new("tool-bg"),
                        name: "bg".to_owned(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type": "object"}),
                        execution_policy: ToolExecutionPolicy::ModelSelectable,
                        concurrency_policy: ToolConcurrencyPolicy::Sequential,
                        replay_policy: ToolReplayPolicy::Never,
                        origin: ToolOrigin::Builtin,
                    },
                    started,
                    release: release.clone(),
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for ParkingBackgroundTool {
        fn execute<'a>(
            &'a self,
            _invocation: ToolInvocation,
            _context: ToolExecutionContext<'a>,
        ) -> BoxFuture<'a, ToolExecutionResult> {
            self.started.send_replace(true);
            let release = self.release.clone();
            Box::pin(async move {
                release.notified().await;
                ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                }
            })
        }
    }

    #[allow(dead_code)] // used by fixtures of the background tests
    fn success() -> ToolExecutionResult {
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
        }
    }

    /// A fixed deterministic status clock.
    #[derive(Debug, Clone, Copy)]
    struct FixedStatusClock;

    impl AgentStatusClock for FixedStatusClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("fixed clock")
                .with_timezone(&chrono::Utc)
        }
    }

    /// A fixed deterministic runtime clock.
    #[derive(Debug, Clone, Copy)]
    struct FixedRuntimeClock;

    impl RuntimeClock for FixedRuntimeClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("fixed clock")
                .with_timezone(&chrono::Utc)
        }
    }

    /// An extension provider recording its facts.
    struct RecordingProvider;

    impl AgentStatusSectionProvider for RecordingProvider {
        fn section_id(&self) -> AgentStatusSectionId {
            AgentStatusSectionId::new("recording")
        }

        fn section(
            &self,
            _context: &AgentStatusRenderContext,
        ) -> Result<Option<Vec<AgentStatusFact>>, ContextError> {
            Ok(Some(vec![AgentStatusFact {
                label: "extension".to_owned(),
                value: "fact".to_owned(),
            }]))
        }
    }

    /// A host fixture over one conversation with the given adapter scripts
    /// and tool registry.
    struct HostFixture {
        _dir: tempfile::TempDir,
        host: RuntimeClientHost,
        coordinator: crate::capabilities::CapabilityCoordinator,
    }

    async fn host_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        composer: AgentStatusComposer,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv-host");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            conversation_id.clone(),
            &workspace,
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(tools),
                mcp_servers: Vec::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("prepare candidate");
        coordinator.commit(candidate).expect("commit candidate");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter.clone()),
            timezone: None,
            context: RuntimeClientContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
                status_composer: composer,
            },
            tool_runtime,
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            replay_limit: None,
        })
        .expect("host");
        (
            adapter,
            HostFixture {
                _dir: dir,
                host,
                coordinator,
            },
        )
    }

    /// The default status composer over the fixed clock.
    fn composer() -> AgentStatusComposer {
        AgentStatusComposer::new(Arc::new(FixedStatusClock))
    }

    fn inbound_text(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: crate::runtime::identity::MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse")
                    .with_timezone(&chrono::Utc),
            ),
        }
    }

    fn submit_content(text: &str) -> Vec<UserContentBlock> {
        vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })]
    }

    fn one_turn_stop() -> Vec<GatedStep> {
        vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    }

    /// The outer liveness guard of the event-stream helpers.
    ///
    /// Waiting for an event is exact: the subscription wakes on
    /// publication. This bounds only the total wall time of one
    /// `receive_until` call, so a genuine regression fails with a message
    /// instead of hanging. It is deliberately far larger than any
    /// scheduling delay a loaded runner can produce, and it is a whole-call
    /// budget rather than a per-event bound — a single scheduling stall can
    /// never fail a correct run.
    const STREAM_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

    /// Receives events until the predicate matches.
    async fn receive_until(
        subscription: &EventSubscription,
        mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
    ) -> Vec<RuntimeClientProtocolEvent> {
        tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
            let mut seen = Vec::new();
            loop {
                let delivery = subscription.next().await;
                let EventDelivery::Event(event) = delivery else {
                    panic!("subscription must stay open and contiguous, got {delivery:?}");
                };
                let matched = predicate(&event);
                seen.push(event);
                if matched {
                    return seen;
                }
            }
        })
        .await
        .expect("the observation stream must not stall")
    }

    /// First attachment succeeds, the second concurrent attachment is
    /// rejected deterministically without evicting the first, detach
    /// permits a later attachment with a distinct identity, and request
    /// ids are attachment-scoped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attachment_lifecycle_and_request_id_scope() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let (first, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("first attach");
        let RuntimeClientResult::Initialized {
            attachment_id,
            cursor,
            ..
        } = &initialized
        else {
            panic!("initialized result");
        };
        let first_id = attachment_id.clone();
        assert_eq!(*cursor, RuntimeClientCursor::new(0));

        let second = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1);
        assert!(matches!(
            second,
            Err(RuntimeClientError::AttachmentInUse {
                existing_attachment_id,
            }) if existing_attachment_id == first_id
        ));

        // The rejection never evicts the first attachment: its requests
        // still work.
        let response = first.handle_request(RuntimeClientRequest::SnapshotGet {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert_eq!(response.id.get(), 1);
        assert!(response.error.is_none());
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::Snapshot { .. })
        ));

        // Incompatible protocol version fails explicitly.
        let bad = fixture.host.attach(9);
        assert!(matches!(
            bad,
            Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: 1,
                requested: 9,
            })
        ));

        // Explicit detach releases the attachment; a fresh attachment has
        // a distinct identity and a fresh request-id scope.
        first.detach();
        let (second_attachment, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach after detach");
        let RuntimeClientResult::Initialized {
            attachment_id: second_id,
            ..
        } = initialized
        else {
            panic!("initialized result");
        };
        assert_ne!(first_id, second_id);
        let response = second_attachment.handle_request(RuntimeClientRequest::SnapshotGet {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert_eq!(response.id.get(), 1, "request ids are attachment-scoped");
        assert!(response.error.is_none());
    }

    /// Dropping the attachment releases it (RAII detach semantics).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_attachment_detaches_it() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        drop(attachment);
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach after drop");
        assert!(
            second
                .handle_request(RuntimeClientRequest::SnapshotGet {
                    id: crate::runtime_client::RequestId::new(1),
                })
                .error
                .is_none()
        );
    }

    /// A detached handle rejects requests deterministically.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detached_handle_rejects_requests() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        attachment.detach();
        let response = attachment.handle_request(RuntimeClientRequest::SnapshotGet {
            id: crate::runtime_client::RequestId::new(7),
        });
        assert_eq!(response.id.get(), 7);
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::NotAttached)
        ));
    }

    /// Submitting while idle starts an attempt whose first turn observes
    /// the message; the admission response is accepted, not finished; the
    /// attempt settles and the canonical history is committed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submit_when_idle_admits_and_runs_the_attempt() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), composer()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("hello"),
        });
        let RuntimeClientResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } = response.result.expect("accepted")
        else {
            panic!("accepted result");
        };
        assert_eq!(inbound_sequence.get(), 1);
        assert_eq!(message_id.as_str(), "conv-host-inbound-1");

        // The attempt settles asynchronously; the subscription observes the
        // terminal settlement exactly once.
        let mut settled = 0;
        let events = receive_until(&subscription, |event| {
            if matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }) {
                settled += 1;
                return true;
            }
            false
        })
        .await;
        let settled_events: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }))
            .collect();
        assert_eq!(settled_events.len(), 1);
        assert_eq!(settled, 1);

        // The first model request observed the admitted message with Agent
        // Status.
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].messages.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.id == message_id)
        }));
        assert!(requests[0].agent_status.is_some());

        // The snapshot carries the committed canonical history and the
        // settled attempt.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.messages.len(), 2, "user message + agent message");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        // The terminal settlement event is emitted by the loop, and the
        // authoritative canonical history is committed by the host's
        // settlement path immediately afterwards. Observing the commit is a
        // wait on that exact condition, never a delay.
        await_canonical_history(&fixture.host, &snapshot.messages).await;
    }

    /// Waits until the host's authoritative canonical history equals the
    /// expected value.
    ///
    /// The host commits it in `finish_attempt`, just after the Agent Loop
    /// emitted the attempt's terminal event, so a test that synchronized on
    /// the terminal event may still observe the previous value once. This
    /// yields to the runtime until the commit is visible; the outer timeout
    /// only bounds a pathological stall.
    async fn await_canonical_history(host: &RuntimeClientHost, expected: &[MessageBlock]) {
        tokio::time::timeout(std::time::Duration::from_secs(120), async {
            loop {
                if host.canonical_history() == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the projection mirrors the authoritative canonical history");
    }

    /// Submitting while an attempt is running queues the message in the
    /// authoritative mailbox; the running attempt drains it at its next
    /// safe boundary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submit_while_busy_queues_for_the_next_drain() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = host_fixture(
            vec![
                // Turn 1 parks in its stream; after release it completes
                // with Stop and the safe boundary drains the queued
                // message into a second turn.
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::Emit(ModelEvent::TextDelta {
                        block_index: ContentBlockIndex::new(0),
                        text: "working".to_owned(),
                    }),
                    GatedStep::ParkUntilReleased(release_rx),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            composer(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        let first = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        assert!(matches!(
            first.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ));

        // Wait until the first request is in flight (the adapter has been
        // asked) so the second submit provably arrives while busy.
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AssistantTextDelta { .. })
        })
        .await;
        assert_eq!(adapter.requests().len(), 1);

        let second = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        let RuntimeClientResult::InboundAccepted {
            message_id: second_id,
            inbound_sequence,
        } = second.result.expect("accepted")
        else {
            panic!("accepted result");
        };
        assert_eq!(inbound_sequence.get(), 2);

        // While the attempt is parked, the second message remains pending
        // in the authoritative mailbox diagnostics.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.inbound.pending.len(), 1);
        assert_eq!(snapshot.inbound.pending[0].message.id, second_id);
        assert_eq!(adapter.requests().len(), 1, "no new turn yet");

        // Release the parked turn: the safe boundary drains the queued
        // message and a second turn observes it.
        release_tx.send(true).expect("release");
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let requests = adapter.requests();
        assert_eq!(requests.len(), 2, "the drained batch opens a second turn");
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| matches!(message, MessageBlock::User(user) if user.id == second_id))
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(
            snapshot.inbound.pending.is_empty(),
            "the queued message was drained exactly once"
        );
        assert!(
            snapshot.messages.iter().any(|message| {
                matches!(message, MessageBlock::User(user) if user.id == second_id)
            }),
            "the drained message committed to canonical history"
        );
    }

    /// Cancelling the current attempt: the acceptance response is not
    /// terminal settlement; the actual runtime settlement is observed
    /// exactly once after the model releases.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_current_attempt_acceptance_is_not_settlement() {
        let (release_tx, release_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            composer(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
        })
        .await;

        // The cancel response is acceptance, never terminal settlement.
        //
        // `AttemptCancellationAccepted` is itself the exact proof: it is
        // returned only when the deciding observation — under the one host
        // lock — found a non-settled attempt, and a settled attempt yields
        // `NoCurrentAttempt` instead. Nothing after this point may assert
        // that the attempt is *still* running: cancellation is precisely
        // what makes the loop settle, so settlement is free to overtake any
        // later observation the test makes. Asserting otherwise would be a
        // scheduler assumption, not an invariant.
        let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(2),
        });
        let Some(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }) = response.result
        else {
            panic!("cancellation of a running attempt is accepted, got {response:?}");
        };
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        let view = snapshot.attempt.expect("attempt view");
        assert_eq!(
            view.attempt_id, attempt_id,
            "acceptance names the attempt the snapshot describes"
        );
        assert!(
            matches!(
                view.phase,
                RuntimeClientAttemptPhase::Running | RuntimeClientAttemptPhase::Settled { .. }
            ),
            "an accepted cancellation leaves the attempt running or already settled"
        );

        // A second cancel is idempotent at the signal level: it is accepted
        // again while the attempt is still cancellable, and reports
        // `no_current_attempt` once settlement won the race. Both are
        // correct; neither is a scheduler assumption.
        let second_cancel = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(3),
        });
        assert!(
            second_cancel.error.is_none()
                || matches!(
                    second_cancel.error,
                    Some(RuntimeClientError::NoCurrentAttempt)
                ),
            "a second cancel is accepted or reports no cancellable attempt, got {second_cancel:?}"
        );

        // Release the parked model so it can finish. The adapter's park is
        // biased on the attempt cancellation, so it may already have
        // observed it and dropped its release handle — the release is a
        // fallback, never part of the ordering proof.
        let _ = release_tx.send(true);
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let settled: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event.event,
                    RuntimeClientEvent::AttemptSettled {
                        outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled { .. },
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(
            settled.len(),
            1,
            "terminal cancellation observed exactly once"
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled { .. }
            }
        ));
    }

    /// With no attempt running, cancel returns a deterministic typed
    /// error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_with_no_attempt_fails_typed() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::NoCurrentAttempt)
        ));
    }

    /// Detaching an attachment never cancels the active attempt: the
    /// attempt continues to settlement after detach, and a reattached
    /// client observes the terminal settlement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_is_not_cancellation() {
        let (release_tx, release_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            composer(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        // Wait for the attempt to be running.
        loop {
            let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
            if snapshot
                .attempt
                .as_ref()
                .is_some_and(|attempt| matches!(attempt.phase, RuntimeClientAttemptPhase::Running))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        // Detach while the attempt is parked.
        attachment.detach();
        // A reattached client sees the attempt still running.
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("reattach");
        let (snapshot, cursor) = fixture.host.snapshot().expect("snapshot");
        assert!(
            matches!(
                snapshot.attempt.expect("attempt view").phase,
                RuntimeClientAttemptPhase::Running
            ),
            "detach never cancels the attempt"
        );
        // The attempt completes normally after release.
        release_tx.send(true).expect("release");
        let subscription = second
            .subscribe_events(cursor)
            .expect("resume from the retained cursor");
        receive_until(&subscription, |event| {
            matches!(
                event.event,
                RuntimeClientEvent::AttemptSettled {
                    outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed { .. },
                    ..
                }
            )
        })
        .await;
    }

    /// Detaching never cancels conversation-owned background work and
    /// never drains mailbox contents.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_never_mutates_background_or_mailbox_state() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        // Dispatch one detached background execution directly through the
        // authoritative registry.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    call_id: ToolCallId::new("call-bg"),
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
        else {
            panic!("accepted dispatch");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("background runner started");
        // One pending mailbox item must survive detach untouched.
        fixture
            .host
            .inner
            .mailbox
            .enqueue(inbound_text("msg-pending", "kept"))
            .expect("enqueue");

        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let (before, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(before.background.len(), 1);
        assert!(matches!(
            before.background[0].state,
            BackgroundLifecycle::Running
        ));
        assert_eq!(before.inbound.pending.len(), 1);
        attachment.detach();

        // After detach the background execution still runs and the mailbox
        // item still pends.
        let (after, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(after.background.len(), 1);
        assert!(
            matches!(after.background[0].state, BackgroundLifecycle::Running),
            "detach never cancels background work"
        );
        assert_eq!(
            after.inbound.pending.len(),
            1,
            "detach never drains mailbox contents"
        );

        // The execution settles normally after release.
        release.notify_one();
        fixture
            .host
            .inner
            .tool_runtime
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        let (final_snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(matches!(
            final_snapshot.background[0].state,
            BackgroundLifecycle::Succeeded
        ));
    }

    /// There is exactly one authoritative mutable canonical history owner
    /// at a time, and ownership transfers at the attempt boundaries.
    ///
    /// This test inspects the host's own `canonical_history` — the thing
    /// that would be the second authority if one existed — at each phase:
    ///
    /// ```text
    /// idle        host history == []                     (host owns)
    /// admission   host history == [user]  -> moved into AgentExecution
    /// running     host history UNCHANGED  while the loop commits more
    /// settlement  host history == the execution's final messages
    /// ```
    ///
    /// The "running" step is the load-bearing one: the loop commits an
    /// agent message and the projection mirrors it, while the host's copy
    /// provably does not move. The host is therefore not a competing
    /// mutable authority whose agreement happens to be checked at the end.
    // One ownership lifecycle observed end to end: splitting it would lose
    // the phase-to-phase continuity that is the whole point.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn canonical_history_has_one_owner_at_a_time() {
        // Turn 1 calls a tool that parks. The loop therefore commits the
        // agent message (the model stream ended) and then blocks in tool
        // execution: exactly the "running, history already grown" window.
        let (tool, mut tool_started, release) = ParkingBackgroundTool::new();
        let definition = ToolDefinition {
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            ..tool.definition.clone()
        };
        let mut tools = ToolRegistry::new();
        tools
            .register(definition.clone(), Arc::new(tool))
            .expect("register the parking tool");
        let call_id = ToolCallId::new("call-park");
        let script = vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call_id.clone(),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call_id.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: call_id,
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        let (adapter, fixture) = host_fixture(
            vec![script, one_turn_stop(), one_turn_stop()],
            tools,
            composer(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        // Idle: the host owns canonical history, and it is the projection's
        // only source.
        assert!(fixture.host.canonical_history().is_empty());
        assert!(
            fixture
                .host
                .snapshot()
                .expect("snapshot")
                .0
                .messages
                .is_empty()
        );

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });

        // Running: the loop committed the agent message (observable on the
        // stream) and is now parked inside tool execution, so the attempt
        // provably has not settled.
        receive_until(&subscription, |event| {
            matches!(
                &event.event,
                RuntimeClientEvent::MessageCommitted { message, .. }
                    if matches!(message, MessageBlock::Agent(_))
            )
        })
        .await;
        tool_started
            .wait_for(|started| *started)
            .await
            .expect("the parking tool started");
        let during = fixture.host.canonical_history();
        assert_eq!(
            during.len(),
            1,
            "the host's history holds only the admitted turn: the attempt owns the rest"
        );
        assert!(matches!(during[0], MessageBlock::User(_)));
        let (mirror, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            mirror.messages.len(),
            2,
            "the projection mirrors the attempt's committed history"
        );
        assert!(fixture.host.has_current_attempt());

        // An inbound message arriving now stays mailbox-owned: the host
        // does not append it to a competing history.
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        assert_eq!(
            fixture.host.canonical_history().len(),
            1,
            "a busy-path submission never mutates canonical history"
        );

        // Releasing the tool lets the attempt finish its tool turn and then
        // drain the mailbox at its safe boundary. The drained message joins
        // the *execution's* history — the loop commits it — and the attempt
        // continues rather than settling.
        release.notify_one();
        let settlement = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let committed_during_attempt: Vec<&MessageBlock> = settlement
            .iter()
            .filter_map(|event| match &event.event {
                RuntimeClientEvent::MessageCommitted { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        assert!(
            committed_during_attempt
                .iter()
                .any(|message| matches!(message, MessageBlock::Tool(_))),
            "the loop committed the tool message"
        );
        assert!(
            committed_during_attempt
                .iter()
                .any(|message| matches!(message, MessageBlock::User(_))),
            "the safe-boundary drain committed the queued inbound message into the attempt"
        );
        // (The host's copy is deliberately not read here: settlement has
        // begun, so the ownership transfer may already have happened. The
        // "frozen while running" assertion above is the load-bearing proof
        // that the host never mutated a competing copy during the attempt.)

        // A third submission while idle: its admission is the deterministic
        // proof that settlement transferred the execution's final history
        // back to the host — admission only happens once `finish_attempt`
        // cleared the attempt slot, and it starts from `canonical_history`.
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(3),
            content: submit_content("third"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;

        // The next attempt's first request began from exactly the previous
        // attempt's committed history.
        let requests = adapter.requests();
        assert_eq!(
            requests.len(),
            3,
            "two turns in the first attempt, one in the second"
        );
        let previous_committed = requests[1].messages.len();
        assert!(
            requests[2].messages.len() > previous_committed,
            "the next attempt started from the previous committed history \
             ({} vs {previous_committed} messages)",
            requests[2].messages.len()
        );

        // The externally visible history is one coherent sequence across
        // the tool turn, the safe-boundary drain, and both attempts. The
        // `debug_assert_eq!` in `finish_attempt` additionally verified, at
        // each of the two settlements above, that the projection mirror
        // equals the authoritative `AgentExecutionResult.messages`.
        let (final_snapshot, _) = fixture.host.snapshot().expect("snapshot");
        let roles: Vec<&str> = final_snapshot
            .messages
            .iter()
            .map(|message| match message {
                MessageBlock::User(_) => "user",
                MessageBlock::Agent(_) => "agent",
                MessageBlock::Tool(_) => "tool",
                MessageBlock::System(_) => "system",
            })
            .collect();
        assert_eq!(
            roles,
            vec!["user", "agent", "tool", "user", "agent", "user", "agent"],
            "one authoritative history, extended across the tool turn, the \
             safe-boundary drain, and both attempts"
        );
    }

    /// Blocks off the runtime until the worker-exit signal arrives.
    ///
    /// The signal itself is the correctness proof: it fires only on the
    /// worker's terminal path. The timeout is an outer liveness guard so a
    /// regression fails loudly instead of hanging.
    async fn await_worker_exit(receiver: std::sync::mpsc::Receiver<()>) {
        tokio::task::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the observation worker must terminate after the host is released");
        })
        .await
        .expect("worker exit task");
    }

    /// Writes one discoverable Skill package, making the next capability
    /// candidate a real (non-no-op) commit.
    fn write_probe_skill(workspace: &std::path::Path, name: &str) {
        let skill = workspace.join(".agents").join("skills").join(name);
        std::fs::create_dir_all(&skill).expect("skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: \"a probe skill\"\n---\nbody\n"),
        )
        .expect("SKILL.md");
    }

    /// Releasing the last semantic owner destroys `HostInner` and
    /// terminates the observation worker — deterministically, and without
    /// depending on process exit.
    ///
    /// All three subsystem observation seams are exercised first, so every
    /// `Arc<HostObserver>` is installed and live at the moment the host is
    /// released. Under the old strong-`Arc` observer this test could not
    /// pass: `HostInner -> subsystem -> Arc<HostObserver> -> HostInner` was
    /// a cycle, and the worker held a strong handle across its await.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn releasing_the_last_owner_destroys_the_host_and_exits_the_worker() {
        let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let HostFixture {
            _dir: dir,
            host,
            coordinator,
        } = fixture;

        let weak = host.weak_inner();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        host.install_worker_exit_probe(exit_tx);

        // Seam 1: the mailbox observer fires under the mailbox lock.
        host.inner
            .mailbox
            .enqueue(inbound_text("msg-lifetime", "queued"))
            .expect("enqueue");
        // Seam 2: the background registry observer fires under the registry
        // lock.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = host
            .inner
            .tool_runtime
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    call_id: ToolCallId::new("call-lifetime"),
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = host
            .inner
            .tool_runtime
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
        else {
            panic!("accepted dispatch");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("background runner started");
        release.notify_one();
        host.inner
            .tool_runtime
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        // Seam 3: the capability observer fires under the coordinator lock.
        write_probe_skill(&dir.path().join("workspace"), "lifetime-skill");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");

        // Every seam has fired and the projection folded them.
        let (before, _) = host.snapshot().expect("snapshot");
        // (The settled background execution also posts its terminal
        // notification into the same authoritative mailbox.)
        assert!(
            before
                .inbound
                .pending
                .iter()
                .any(|item| item.message.id.as_str() == "msg-lifetime")
        );
        assert_eq!(before.background.len(), 1);
        assert!(
            before
                .capabilities
                .skills
                .iter()
                .any(|skill| skill.name == "lifetime-skill")
        );

        // Release the one semantic owner. The subsystems, their observer
        // `Arc`s, and the worker task all still exist.
        drop(host);

        // The worker terminated on its own terminal condition. Once it has,
        // no strong reference can exist anywhere.
        await_worker_exit(exit_rx).await;
        assert_eq!(
            weak.strong_count(),
            0,
            "no strong reference to the host remains"
        );
        assert!(
            weak.upgrade().is_none(),
            "HostInner is destroyed, not merely unreachable"
        );

        // The authoritative subsystems outlived the projection, as they
        // must.
        drop(coordinator);
        drop(dir);
    }

    /// A surviving authoritative subsystem handle neither retains nor
    /// resurrects the host: its observer no-ops, and its own transitions
    /// still succeed.
    ///
    /// This is the property that makes projection observation distinct from
    /// subsystem ownership.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_surviving_subsystem_handle_never_retains_the_host() {
        let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let HostFixture {
            _dir: dir,
            host,
            coordinator,
        } = fixture;

        // Clone subsystem handles out of the host, exactly as an embedder
        // legitimately may.
        let mailbox = host.inner.mailbox.clone();
        let registry = host.inner.tool_runtime.background().clone();
        let weak = host.weak_inner();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        host.install_worker_exit_probe(exit_tx);

        drop(host);
        await_worker_exit(exit_rx).await;
        assert!(weak.upgrade().is_none(), "the host is gone");

        // Authoritative mailbox transition: the observer's upgrade fails and
        // the seam no-ops, but the mailbox is unaffected.
        let sequence = mailbox
            .enqueue(inbound_text("msg-after", "still authoritative"))
            .expect("the mailbox remains authoritative without a projection");
        assert_eq!(sequence.get(), 1);
        let batch = mailbox.drain().expect("the drain still works");
        assert_eq!(batch.items().len(), 1);

        // Authoritative background transition: same.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &ToolInvocation {
                    call_id: ToolCallId::new("call-after"),
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } =
            registry.commit_dispatch(prepared, &CancellationSignal::new())
        else {
            panic!("accepted dispatch");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("background runner started");
        release.notify_one();
        registry
            .wait_until_terminal(&execution_id)
            .await
            .expect("the registry still settles executions");

        // Authoritative capability transition: same.
        write_probe_skill(&dir.path().join("workspace"), "after-skill");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        let committed = coordinator
            .commit(candidate)
            .expect("the coordinator remains authoritative");
        assert!(
            committed
                .catalog_entries()
                .iter()
                .any(|entry| entry.name == "after-skill")
        );

        // None of those transitions resurrected the host.
        assert_eq!(weak.strong_count(), 0);
        assert!(
            weak.upgrade().is_none(),
            "an observation seam can never resurrect a destroyed host"
        );
    }

    /// Attachment detach is not host destruction: the host survives, the
    /// attachment slot is released, and a fresh endpoint initializes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_releases_the_attachment_but_never_the_host() {
        let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let weak = fixture.host.weak_inner();

        let endpoint = fixture.host.endpoint();
        let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        });
        assert!(response.error.is_none());

        // Dropping the endpoint detaches only that attachment.
        drop(endpoint);
        assert!(
            weak.upgrade().is_some(),
            "detach is not host destruction while a semantic owner remains"
        );

        // The host is still usable and the slot is free.
        fixture
            .host
            .snapshot()
            .expect("the host still serves reads");
        let reconnected = fixture.host.endpoint();
        let response = reconnected.handle_request(RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
        });
        assert!(
            matches!(
                response.result,
                Some(RuntimeClientResult::Initialized { .. })
            ),
            "reconnect remains possible while the host is owned"
        );

        // Only releasing the host itself ends its lifetime.
        drop(reconnected);
        drop(fixture);
        assert!(weak.upgrade().is_none(), "the host ends with its owner");
    }

    /// The lock-order invariant, made structurally testable: an
    /// authoritative background registry transition **completes** while the
    /// host lock is held by someone else.
    ///
    /// The interleaving is established with barriers, not timing:
    ///
    /// ```text
    /// T1: snapshot()  -> takes HostState -> parks on the armed probe gate
    /// T2: registry.cancel(id)
    ///       -> takes the registry lock
    ///       -> fires on_snapshot (registry lock still held)
    ///       -> returns
    /// test: recv() the completion token   <-- asserted BEFORE releasing T1
    /// test: release the gate; T1 finishes
    /// ```
    ///
    /// The token arriving before the release is the proof: the observer
    /// never acquired `HostState`, so there is no `registry -> HostState`
    /// edge to pair with any `HostState -> registry` call on the host
    /// surface (`background_status`, `background_cancel`). If the observer
    /// took the host lock instead, T2 could not have completed here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_background_transition_completes_while_the_host_lock_is_held() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    call_id: ToolCallId::new("call-bg"),
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
        else {
            panic!("accepted dispatch");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("background runner started");

        // T1 takes the host lock and parks inside it.
        probe.arm_snapshot();
        let parked_host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || parked_host.snapshot());
        probe.wait_snapshot_entered();

        // T2 commits an authoritative registry transition and reports
        // completion.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let registry = fixture.host.inner.tool_runtime.background().clone();
        let cancel_id = execution_id.clone();
        let transition = tokio::task::spawn_blocking(move || {
            let snapshot = registry.cancel(&cancel_id).expect("known execution");
            done_tx.send(()).expect("the test still listens");
            snapshot
        });

        // The proof: completion is observable while T1 still holds the host
        // lock.
        done_rx
            .recv()
            .expect("an authoritative registry transition never waits on the host lock");
        let cancelled = transition.await.expect("transition task");
        assert!(matches!(cancelled.state, BackgroundLifecycle::Cancelling));

        probe.release_snapshot();
        snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");

        // The observation was not lost by being enqueued: the next host
        // lock acquisition folds it.
        release.notify_one();
        fixture
            .host
            .inner
            .tool_runtime
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.background.len(), 1);
        assert_eq!(snapshot.background[0].execution_id, execution_id);
    }

    /// The same lock-order invariant for the capability coordinator, with a
    /// stronger barrier: the coordinator is parked *inside* `commit`, with
    /// its state lock held, and the host lock is taken from another thread
    /// while it is parked.
    ///
    /// ```text
    /// T1: coordinator.commit(candidate)
    ///       -> takes the capability state lock
    ///       -> parks on the commit-boundary hook (lock still held)
    /// test: snapshot() from this thread -> takes HostState, returns
    /// test: release the hook; commit fires on_snapshot and completes
    /// ```
    ///
    /// The host lock being acquirable while the capability lock is held
    /// rules out `HostState -> capability`; the commit completing without
    /// the host lock (asserted below while nothing holds it, and by the
    /// background test's mirror interleaving) rules out the reverse edge.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_capability_commit_never_waits_on_the_host_lock() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
        let (before, _) = fixture.host.snapshot().expect("snapshot");

        // A non-noop candidate: one discoverable Skill package.
        let workspace = fixture
            .host
            .inner
            .tool_runtime
            .workspace()
            .root()
            .to_path_buf();
        let skill = workspace.join(".agents").join("skills").join("probe-skill");
        std::fs::create_dir_all(&skill).expect("skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: probe-skill\ndescription: \"a probe skill\"\n---\nbody\n",
        )
        .expect("SKILL.md");
        let candidate = fixture
            .coordinator
            .prepare_candidate()
            .await
            .expect("prepare candidate");

        // T1 parks inside commit while holding the capability state lock.
        let hook = Arc::new(crate::capabilities::test_sync::CommitBoundaryHook::default());
        fixture
            .coordinator
            .install_commit_boundary_hook(hook.clone());
        let committing = fixture.coordinator.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let commit_task = tokio::task::spawn_blocking(move || {
            let snapshot = committing.commit(candidate);
            done_tx.send(()).expect("the test still listens");
            snapshot
        });
        hook.wait_entered();

        // The host lock is acquirable while the capability state lock is
        // held: there is no `HostState -> capability` edge.
        let (during, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            during.capabilities.revision, before.capabilities.revision,
            "the uncommitted candidate is not observable"
        );

        // Release: the commit fires its observer with the capability lock
        // still held and completes without ever taking the host lock.
        hook.proceed();
        done_rx
            .recv()
            .expect("an authoritative capability commit never waits on the host lock");
        let committed = commit_task
            .await
            .expect("commit task")
            .expect("commit succeeds");
        assert!(committed.revision() > before.capabilities.revision);

        // The enqueued observation folds at the next host lock acquisition.
        let (after, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(after.capabilities.revision, committed.revision());
        assert!(
            after
                .capabilities
                .skills
                .iter()
                .any(|entry| entry.name == "probe-skill"),
            "the capability projection folded the committed activation"
        );
    }

    /// The exact snapshot/cursor race: a transition concurrent with a
    /// snapshot is either already reflected in the snapshot at its cursor
    /// or observed after that cursor — never lost.
    ///
    /// Interleaving A (snapshot wins): the snapshot linearizes first and
    /// the concurrent transition is observed by a resume after the
    /// snapshot's cursor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_cursor_race_snapshot_wins() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), vec![one_turn_stop()]).await;
        probe.arm_snapshot();
        let snapshot_probe = probe.clone();
        let host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || host.snapshot());
        snapshot_probe.wait_snapshot_entered();
        // The concurrent transition: a submit whose admission linearizes
        // after the parked snapshot.
        let submitting = fixture.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("racing"))
                .expect("accepted")
        });
        // The submit's enqueue pushed its observation and its admission is
        // blocked on the host lock the snapshot holds.
        snapshot_probe.release_snapshot();
        let (snapshot, cursor) = snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        let accepted = submit_task.await.expect("submit task");
        assert!(matches!(
            accepted,
            RuntimeClientResult::InboundAccepted { .. }
        ));
        assert!(snapshot.inbound.pending.is_empty());
        // The transition is observed after C: resume and receive the
        // admission events, never a gap.
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("resume after the snapshot cursor");
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::InboundEnqueued { .. })
        })
        .await;
        assert!(
            events.iter().all(|event| event.cursor > cursor),
            "every resumed event is strictly after the snapshot cursor"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RuntimeClientEvent::InboundEnqueued { .. }))
        );
        // Drain the attempt so the fixture settles cleanly.
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
    }

    /// Interleaving B (publish wins): the concurrent transition
    /// linearizes before the snapshot, so the snapshot at its cursor
    /// already reflects it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_cursor_race_publish_wins() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), vec![one_turn_stop()]).await;
        // Baseline: an idle host at some cursor C.
        let (before, cursor) = fixture.host.snapshot().expect("snapshot");
        assert!(before.inbound.pending.is_empty());

        probe.arm_publish();
        let probe_task = probe.clone();
        let submitting = fixture.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("racing"))
                .expect("accepted")
        });
        // The submission publishes its first client event while holding
        // the projection lock; park there.
        probe_task.wait_publish_entered();
        let probe_snapshot = probe.clone();
        let snapshot_host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || snapshot_host.snapshot());
        // Release the publication; the snapshot then acquires the lock.
        probe_snapshot.release_publish();
        let _accepted = submit_task.await.expect("submit task");
        let (after_snapshot, after_cursor) = snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        // The transition is already reflected: the cursor advanced and the
        // events are either in the snapshot's state or replayable before
        // it — never lost.
        assert!(after_cursor > cursor, "the transition advanced the cursor");
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("resume from the pre-transition cursor");
        let mut saw_inbound = false;
        loop {
            let delivery = tokio::time::timeout(STREAM_LIVENESS_GUARD, subscription.next())
                .await
                .expect("stream must not stall");
            let EventDelivery::Event(event) = delivery else {
                panic!("subscription stays open and contiguous, got {delivery:?}");
            };
            if matches!(event.event, RuntimeClientEvent::InboundEnqueued { .. }) {
                saw_inbound = true;
            }
            if matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }) {
                break;
            }
        }
        assert!(saw_inbound, "the transition event is on the stream");
        assert!(
            after_snapshot.messages.iter().any(|message| matches!(
                message,
                MessageBlock::User(user) if user.id.as_str() == "conv-host-inbound-1"
            )),
            "the snapshot reflects the transition state"
        );
    }

    /// The bounded replay/resync contract: a serviceable resume has no
    /// gap, an expired cursor returns `resync_required`, and a fresh
    /// snapshot repairs all state. The cursor survives detach (it belongs
    /// to the observation stream, not the attachment).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn replay_resync_and_cursor_survival() {
        let (_, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), composer()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let terminal_cursor = events.last().expect("terminal event").cursor;

        // Detach: the cursor is stream-owned and survives.
        attachment.detach();
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("reattach");
        let second_subscription = second
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("resume from the retained stream");
        let replayed = receive_until(&second_subscription, |event| {
            event.cursor == terminal_cursor
        })
        .await;
        assert_eq!(
            replayed.last().expect("terminal replayed").cursor,
            terminal_cursor,
            "the full retained stream is replayable after reconnect"
        );
        assert!(
            replayed
                .windows(2)
                .all(|pair| pair[0].cursor < pair[1].cursor)
        );

        // A cursor ahead of the stream is unserviceable.
        let error = second
            .subscribe_events(RuntimeClientCursor::new(terminal_cursor.get() + 100))
            .expect_err("ahead of the stream");
        assert!(matches!(error, RuntimeClientError::ResyncRequired { .. }));
    }

    /// The background lifecycle projection: Starting/Running/Cancelling/
    /// terminal transitions project from the authoritative registry, the
    /// protocol cancel is acceptance-not-settlement, and terminal records
    /// stay visible after detach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_lifecycle_projection_and_protocol_cancel() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    call_id: ToolCallId::new("call-bg"),
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
        else {
            panic!("accepted");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("runner started");

        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(matches!(
            snapshot.background[0].state,
            BackgroundLifecycle::Running
        ));

        // Protocol cancel: acceptance carries the Cancelling snapshot,
        // never the terminal result.
        let response = attachment.handle_request(RuntimeClientRequest::BackgroundCancel {
            id: crate::runtime_client::RequestId::new(1),
            execution_id: execution_id.clone(),
        });
        let RuntimeClientResult::BackgroundCancelAccepted { execution } =
            response.result.expect("accepted")
        else {
            panic!("cancel accepted result");
        };
        assert_eq!(execution.execution_id, execution_id);
        assert!(matches!(execution.state, BackgroundLifecycle::Cancelling));

        // Unknown executions fail explicitly.
        let unknown = attachment.handle_request(RuntimeClientRequest::BackgroundStatus {
            id: crate::runtime_client::RequestId::new(2),
            execution_id: crate::runtime::identity::ToolExecutionId::new("exec_99"),
        });
        assert!(matches!(
            unknown.error,
            Some(RuntimeClientError::UnknownBackgroundExecution { .. })
        ));

        // Settlement: the cancellation winner canonicalizes the terminal
        // result to Cancelled.
        release.notify_one();
        let terminal = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        assert_eq!(terminal.state, BackgroundLifecycle::Cancelled);
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.background[0].state, BackgroundLifecycle::Cancelled);
        assert!(snapshot.background[0].result.is_some());
    }

    /// Detached background work stays visible after the originating
    /// attempt terminates.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_survives_attempt_termination() {
        let (_, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), composer()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    call_id: ToolCallId::new("call-bg"),
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
        else {
            panic!("accepted");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("runner started");

        // Run one attempt to completion.
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;

        // The detached execution remains visible after the attempt
        // terminated and settles on its own schedule.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        assert_eq!(snapshot.background.len(), 1);
        assert!(matches!(
            snapshot.background[0].state,
            BackgroundLifecycle::Running
        ));
        release.notify_one();
        let terminal = fixture
            .host
            .inner
            .tool_runtime
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    }

    /// Agent Status is projected from the exact same composition the model
    /// path consumes: the client event's rendered text equals the model
    /// request's rendered attachment, and the structured extension facts
    /// are preserved.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn agent_status_projection_shares_one_composition() {
        let mut composer = composer();
        composer
            .register(Arc::new(RecordingProvider))
            .expect("register provider");
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), composer).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AgentStatusComposed { .. })
        })
        .await;
        let status_event = events
            .iter()
            .find_map(|event| match &event.event {
                RuntimeClientEvent::AgentStatusComposed { status, .. } => Some(status),
                _ => None,
            })
            .expect("status event");
        // The model request is recorded slightly after the status
        // observation; wait for the attempt to settle so the request is
        // provably recorded.
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1);
        let model_rendered = requests[0]
            .agent_status
            .as_ref()
            .expect("model path carries Agent Status")
            .rendered
            .clone();
        assert_eq!(
            status_event.rendered, model_rendered,
            "the client view derives from the same composition as the model path"
        );
        assert!(status_event.sections.iter().any(|section| matches!(
            section,
            crate::runtime_client::snapshot::RuntimeClientStatusSection::Facts { facts }
                if facts.iter().any(|fact| fact.label == "extension" && fact.value == "fact")
        )));
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.status.expect("status view").rendered,
            model_rendered
        );
    }

    /// Shutdown is distinct from detach: it stops further admission, the
    /// current attempt continues, and detach remains available.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_is_not_detach_and_not_cancellation() {
        let (release_tx, release_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            composer(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
        })
        .await;

        let response = attachment.handle_request(RuntimeClientRequest::Shutdown {
            id: crate::runtime_client::RequestId::new(2),
        });
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::ShutdownAccepted)
        ));

        // Further admission fails explicitly.
        let submit = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(3),
            content: submit_content("too late"),
        });
        assert!(matches!(
            submit.error,
            Some(RuntimeClientError::RuntimeShutdown)
        ));

        // The current attempt continues to settlement; detach still works.
        release_tx.send(true).expect("release");
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        attachment.detach();
        let (reattached, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach after shutdown still works");
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        assert!(
            reattached
                .handle_request(RuntimeClientRequest::SnapshotGet {
                    id: crate::runtime_client::RequestId::new(1),
                })
                .error
                .is_none()
        );
    }

    /// Host construction rejects mismatched capability/tool-runtime
    /// ownership domains.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_construction_validates_ownership() {
        let (adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let other_dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(other_dir.path().join("workspace")).expect("workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("conv-other"),
            other_dir.path().join("workspace"),
            other_dir.path().join("artifacts"),
        )
        .expect("other runtime");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let error = RuntimeClientHost::new(RuntimeClientHostConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            timezone: None,
            context: RuntimeClientContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
                status_composer: composer(),
            },
            tool_runtime: other_runtime,
            capability: fixture.coordinator.clone(),
            clock: None,
            initial_messages: Vec::new(),
            replay_limit: None,
        })
        .expect_err("mismatched ownership is rejected");
        assert!(matches!(
            error,
            HostConstructionError::OwnershipMismatch { .. }
        ));
    }

    /// Builds a host with the projection linearization probe installed.
    async fn host_fixture_probe(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv-host");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            conversation_id.clone(),
            &workspace,
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                mcp_servers: Vec::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let host = RuntimeClientHost::with_probe(
            RuntimeClientHostConfig {
                agent_id: AgentId::new("agent-a"),
                model: crate::model::fixture::scripted_session_model(adapter.clone()),
                timezone: None,
                context: RuntimeClientContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
                    status_composer: composer(),
                },
                tool_runtime,
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
                replay_limit: None,
            },
            (*probe).clone(),
        )
        .expect("host");
        (
            adapter,
            HostFixture {
                _dir: dir,
                host,
                coordinator,
            },
        )
    }
}
