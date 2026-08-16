//! The conversation runtime coordinator (Issue #61): the semantic owner of
//! conversation coordination for one conversation.
//!
//! [`ConversationRuntime`] owns the semantic conversation/runtime state that
//! used to live inside the old Runtime Client host:
//!
//! ```text
//! conversation identity / agent identity
//! authoritative mutable session model state
//! between-attempt canonical ConversationState
//! RequestHistory (frozen non-history request facts)
//! attempt-id allocation
//! the current-attempt slot and its cancellation handle
//! attempt admission (the ONE admission owner)
//! ordinary inbound acceptance/admission coordination
//! the ConversationInboundMailbox active-process relationship
//! ConversationToolRuntime / ConversationBackgroundRegistry
//! CapabilityCoordinator
//! context/request assembly dependencies (policy, estimator, status composer)
//! the shutdown/admission gate
//! attempt settlement handoff back into conversation state
//! ```
//!
//! The Runtime Client (see `crate::runtime_client`) is a projection + control
//! + attachment adapter over this coordinator. It observes runtime
//! observations and forwards control requests; it never owns the semantic
//! state above.
//!
//! # The one admission owner
//!
//! Any producer may publish ordinary inbound work through the conversation
//! inbound boundary (the [`ConversationInboundMailbox`](crate::runtime::inbound::ConversationInboundMailbox)
//! of the owned tool runtime). Only [`ConversationRuntime`] admits the next
//! [`AgentExecution`](crate::agent::AgentExecution):
//!
//! ```text
//! producer publishes ordinary inbound
//!         |
//!         v
//! ConversationInboundMailbox
//!         |
//!         v
//! runtime wake (Notify)                     <- idle async inbound wakes itself
//!         |
//!         v
//! coordinator worker: admit_next_attempt
//! ```
//!
//! A producer never starts an attempt itself: the coordinator worker is the
//! single admission trigger for enqueue-driven admission, and the
//! coordinator's own settlement handoff is the second (internal) trigger.
//! Both run the same [`admit_next_attempt`](ConversationRuntime) linearization.
//!
//! # Admission linearization
//!
//! All of the following share the one coordinator state lock:
//!
//! ```text
//! idle observation (no current attempt, gate open)
//! finite inbound selection (one watermark-bounded mailbox drain)
//! canonical commit of the drained inbound
//! attempt-id allocation
//! model snapshot freeze (SessionModelState::snapshot)
//! current-attempt publication
//! ```
//!
//! There is therefore at most one active [`AgentExecution`] per
//! conversation, one finite inbound batch is admitted exactly once (either
//! consumed by the active attempt at its safe boundary, or used to create
//! the next attempt after settlement — never both), and a session model
//! update linearizes either before an admission (observed by that attempt)
//! or after it (affects only future attempts). See the deterministic race
//! regressions in the test module.
//!
//! # Lifecycle: construction, optional client binding, activation
//!
//! A conversation runtime has exactly two lifecycle states, and the
//! transition between them is the one explicit composition boundary:
//!
//! ```text
//! ConversationRuntime::new(..)          -> inactive
//!     [optional] RuntimeClientHost::new(..)   binds the client adapter
//! ConversationRuntime::activate()       -> active: semantic execution may begin
//! ```
//!
//! An **inactive** runtime is inert, and this is enforced, not merely
//! documented. Once a `ConversationRuntime` owns its semantic subsystems,
//! the inactive phase admits no conversation-semantic mutation at all:
//!
//! ```text
//! ConversationRuntime constructed
//!     |
//!     |  inactive composition phase
//!     |    no inbound admission        (mailbox refuses enqueue)
//!     |    no model mutation           (model_set: ModelUpdateError::Inactive)
//!     |    no shutdown transition      (shutdown:  ShutdownError::Inactive)
//!     |    no background dispatch commit (registry: BackgroundDispatchError::ConversationInactive)
//!     |    no active capability commit (coordinator: CapabilityCommitError::ConversationInactive)
//!     |
//! [optional RuntimeClientHost bootstrap]
//!     |
//! ConversationRuntime::activate()      <- the one freeze/open boundary
//!     |
//! all runtime semantic mutations may begin
//! ```
//!
//! The lifecycle gates are small shared pieces of state, never coordinator
//! callbacks: the mailbox's own admission flag (set by `bind_inactive` at
//! construction and `activate` at activation), and the capability
//! coordinator's runtime-owned activation flag. The background registry
//! reads the mailbox admission flag of its own resources under its own
//! lock section. `activate` flips them all under the one coordinator lock,
//! so a subsystem commit and an activation always linearize: a commit that
//! observes the pre-activation state is refused, one that observes the
//! post-activation state is a real post-activation transition.
//!
//! Binding a Runtime Client host is a **pre-activation** composition
//! decision, not a hot operation: a host bind after activation is refused
//! with the typed [`RuntimeBootstrapError::RuntimeAlreadyActivated`]. A
//! headless runtime (Issue #60 subagents, every zero-client regression)
//! simply never constructs a host. Runtime Client *attachments* remain
//! fully dynamic after activation — attach, detach, reattach — because
//! attachment lifetime and host-binding lifetime are different axes.
//!
//! # Observation bridge
//!
//! The coordinator publishes every semantically meaningful transition as a
//! runtime-owned [`ConversationObservation`](crate::runtime::observation::ConversationObservation)
//! into the shared leaf
//! [`PendingObservations`](crate::runtime::observation::PendingObservations)
//! queue. The Runtime Client projection folds that queue under its own
//! synchronization boundary, translating the semantic observations into
//! its snapshot/cursor read model (see `RuntimeClientProjection`), so
//! snapshot/cursor reads remain linearizable. The runtime keeps no second
//! fold of that vocabulary. A conversation with zero Runtime Client
//! attachments runs the exact same admission/execution path; the
//! observation queue simply has no consumer.
//!
//! # The bootstrap cut
//!
//! [`ConversationRuntime::install_observation_bridge`] is the one runtime
//! handshake a Runtime Client adapter uses at construction. It runs
//! entirely under the one coordinator lock, refuses an already-activated
//! runtime under that same lock, installs the observation queue and every
//! subsystem observation seam, and captures the bootstrap seed:
//!
//! ```text
//! T0  coordinator lock; reject if activated; install the queue;
//!     capture shutting_down / canonical messages / session model
//! T1  background registry: install observer + capture snapshots  (one background section)
//! T2  mailbox:             install observer + capture pending    (one mailbox section)
//! R   capability:          install observer + capture snapshot   (one capability section)
//!     coordinator lock released
//! ```
//!
//! > **Invariant.** The bootstrap cut `R` is a real global state of the
//! > runtime: the initial snapshot contains every projected runtime fact
//! > committed through `R`, every projected transition after `R` is
//! > delivered exactly once through the live observation stream in
//! > semantic publication order, and no transition before `R` is published
//! > as a post-`R` event.
//!
//! Every captured value is still the authority's live value at `R`:
//!
//! - the coordinator-owned facts cannot move — every mutator
//!   (`model_set`, `shutdown`, `submit_inbound`, admission, settlement)
//!   takes the coordinator lock, which is held across `[T0, R]`;
//! - the background registry refuses `commit_dispatch` while its mailbox is
//!   bound inactive, so no background record exists across `[T0, R]` and
//!   none can be created;
//! - the mailbox refuses `enqueue` while its bound runtime is inactive, so
//!   the pending queue is frozen across `[T0, R]`;
//! - the capability coordinator refuses a runtime-owned `commit` before
//!   activation, and the capability snapshot is captured *at* `R`.
//!
//! And because each authority's observer installation shares one lock
//! section with its own seed capture, no transition can be both seeded and
//! queued, and none can be neither.
//!
//! Activation is what actually opens the stream: since an inactive runtime
//! publishes nothing, `R` coincides with the activation point and the live
//! stream carries *every* observation the runtime ever emits. The first
//! allocated `RuntimeClientCursor` is therefore always a real
//! post-activation semantic transition — bootstrap state never fabricates
//! a live event.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use chrono_tz::Tz;

use crate::agent::cancellation::AgentCancellation;
use crate::agent::observer::{AgentExecutionObserver, AgentStatusObservation};
use crate::agent::{AgentExecution, AgentExecutionRequest};
use crate::capabilities::{CapabilityCoordinator, CapabilityObserver, CapabilitySnapshot};
use crate::context::tokens::TokenEstimator;
use crate::context::{AgentStatusComposer, ContextRuntime, SessionContextPolicy};
use crate::conversation::ConversationState;
use crate::events::types::RuntimeEvent;
use crate::message::types::{MessageBlock, UserContentBlock};
use crate::model::catalog::ModelCatalogView;
use crate::model::session::{
    AttemptModelSnapshot, SessionModelConfig, SessionModelState, SessionModelView,
};
use crate::model::{ModelRequest, RequestIdentity, invocation::ModelInvocationError};
use crate::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolExecutionId};
use crate::runtime::inbound::{
    ConversationInboundMailbox, FreshInboundTurn, InboundBatch, InboundItem, InboundObserver,
    InboundSequence, InitialTurnTrigger, MailboxError,
};
use crate::runtime::observation::{ConversationObservation, PendingObservations};
use crate::runtime::request_history::RequestHistory;
use crate::runtime::types::{CancellationReason, RuntimeClock, SystemClock};
use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundObserver};
use crate::tools::runtime::ConversationToolRuntime;

/// The one conversation runtime construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationRuntimeError {
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
    /// The initial canonical messages do not form a valid conversation
    /// state (for example a duplicate `MessageId`).
    InvalidInitialConversation(String),
    /// The conversation tool runtime identity (or the capability
    /// coordinator identity) is already bound to a conversation runtime.
    ///
    /// One conversation identity is bound to at most one
    /// [`ConversationRuntime`] for that identity's lifetime, so cloning a
    /// runtime bundle never yields a second bindable identity and dropping
    /// the bound runtime never makes it bindable again.
    RuntimeAlreadyBound {
        /// The conversation whose runtime identity is already bound.
        conversation_id: ConversationId,
    },
    /// No Tokio execution runtime is current at construction.
    ///
    /// The admission worker must exist before the runtime is usable:
    /// ordinary inbound producers (native `mailbox.enqueue` included) wake
    /// that worker, and a runtime constructed outside an execution runtime
    /// would silently never admit anything. Construction therefore fails
    /// explicitly instead of creating a partially active coordinator.
    NoExecutionRuntime,
}

impl core::fmt::Display for ConversationRuntimeError {
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
            Self::InvalidInitialConversation(message) => write!(
                f,
                "the initial canonical conversation is invalid: {message}"
            ),
            Self::RuntimeAlreadyBound { conversation_id } => write!(
                f,
                "the conversation runtime identity of {conversation_id} is already bound to a conversation runtime"
            ),
            Self::NoExecutionRuntime => write!(
                f,
                "the conversation runtime requires a Tokio execution runtime at construction"
            ),
        }
    }
}

impl std::error::Error for ConversationRuntimeError {}

/// The shared context-plane pieces of one conversation runtime.
///
/// These are the **session-owned static** pieces: the token estimator, the
/// Agent Status composer, and the context policy (reserve tokens,
/// keep-recent target, summary output cap). They persist across attempts,
/// and the model path and the Runtime Client projection share one composer.
///
/// There is deliberately no separate summary store: compaction lineage is
/// derived from Conversation Surface history, which the one `ConversationState`
/// owns, so no second authority can drift from the authoritative state.
///
/// The context *window* is deliberately absent: it belongs to the model, so
/// each attempt derives its [`ContextRuntime`] from this policy plus that
/// attempt's immutable model snapshot. No window captured at process start
/// can survive a session model change.
#[derive(Clone)]
pub struct ConversationContextConfig {
    /// The static session-owned context policy.
    pub policy: SessionContextPolicy,
    /// The deterministic token estimator.
    pub estimator: Arc<dyn TokenEstimator>,
    /// The Agent Status composer shared by the model path and the Runtime
    /// Client projection.
    pub status_composer: AgentStatusComposer,
}

/// The construction-time configuration of one conversation runtime.
///
/// # One conversation authority
///
/// There is deliberately no `conversation_id` field: the
/// [`ConversationToolRuntime`] is the single authority for the conversation
/// identity at this boundary, and the runtime derives its identity from
/// [`ConversationToolRuntime::conversation_id`]. A runtime whose
/// conversation identity disagrees with the runtime it coordinates is
/// therefore not representable, rather than rejected by an equality check.
pub struct RuntimeConversationConfig {
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
    pub context: ConversationContextConfig,
    /// The conversation tool runtime (owns the canonical mailbox and the
    /// authoritative background registry).
    pub tool_runtime: ConversationToolRuntime,
    /// The capability coordinator (owns the active capability snapshot).
    pub capability: CapabilityCoordinator,
    /// The runtime clock stamping submitted inbound messages; the system
    /// clock is used when omitted.
    pub clock: Option<Arc<dyn RuntimeClock>>,
    /// The canonical conversation history the runtime starts from.
    pub initial_messages: Vec<MessageBlock>,
}

/// The runtime-owned current attempt handle.
///
/// This is a control handle only: the coordinator keeps exactly the
/// cancellation trigger the attempt task runs against so that
/// `cancel_current_attempt` can request cancellation. It does **not** own a
/// second cancellation state machine and never decides a terminal outcome;
/// [`AgentExecution`] remains the attempt execution/terminal authority.
struct CurrentAttempt {
    /// The attempt identity.
    attempt_id: AttemptId,
    /// The attempt cancellation trigger observed by the loop.
    cancellation: AgentCancellation,
}

/// The one synchronized coordinator state (the admission linearization
/// owner).
struct CoordinatorState {
    /// The session's authoritative mutable model state.
    ///
    /// It lives under the *same* lock that owns attempt admission, so a
    /// model update and an attempt admission can never interleave
    /// ambiguously: whichever acquires the lock first linearizes first.
    model: SessionModelState,
    /// Settled frozen non-history request facts, retained beside the
    /// authoritative `ConversationState` rather than copied into messages.
    request_history: RequestHistory,
    /// The one canonical conversation state, owned by the coordinator
    /// **only between attempts**.
    ///
    /// Ownership is structural, not conventional: admission moves the state
    /// out (`take`), so while an attempt runs this slot is `None` and the
    /// coordinator physically cannot mutate a competing copy. Settlement
    /// moves the authoritative state back in.
    conversation: Option<ConversationState>,
    /// Whether the runtime was activated (Issue #61).
    ///
    /// It lives under the admission lock because it is *both* the gate of
    /// semantic execution and the freeze point of the Runtime Client host
    /// binding decision: `activate` sets it and the bootstrap handshake
    /// rejects on it, both under this one lock, so the two can never
    /// interleave ambiguously.
    activated: bool,
    /// The current attempt slot (None = idle).
    current_attempt: Option<CurrentAttempt>,
    /// Whether shutdown was accepted: no further inbound admission, no
    /// further attempt admission; the current attempt continues.
    shutting_down: bool,
    /// The next attempt identity sequence.
    next_attempt_seq: u64,
    /// The next submitted-inbound message identity sequence.
    next_inbound_seq: u64,
}

/// The runtime admission worker's wake boundary.
///
/// A leaf: one `Notify` plus a closed flag, owned by the worker task and the
/// runtime. The mailbox observer wakes it on every enqueue, so an idle
/// conversation admits asynchronous inbound without any client request.
struct WakeGate {
    /// The wake signal.
    notify: tokio::sync::Notify,
    /// Set by the runtime's `Drop`. Terminal for the worker.
    closed: AtomicBool,
    /// Test-only worker-exit signal.
    #[cfg(test)]
    worker_exit: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl WakeGate {
    fn new() -> Self {
        Self {
            notify: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            #[cfg(test)]
            worker_exit: Mutex::new(None),
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
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

/// Test-only synchronization hooks of the conversation coordinator.
///
/// Every hook is an armed gate that parks exactly one operation and then
/// releases/disarms itself, so a test can construct exact interleavings
/// without timing assumptions:
///
/// - `admission_gate`: parked at the entrance of `admit_next_attempt`,
///   **before** the coordinator lock is acquired, so a competing publish
///   can still enqueue while the admission is gated. A gated admission
///   runs when the gate is released; a later admission (for example the
///   settlement handoff) passes unarmed.
/// - `settlement_gate`: parked inside `finish_attempt` after the
///   conversation state is restored and the current-attempt slot is
///   cleared, **before** the next-admission handoff, so an enqueue during
///   the park provably races the settlement-to-next-attempt boundary.
///
/// All synchronization is `std` (mutex + condvar) because the coordinator
/// boundary is a `std` mutex critical section; the parking blocks the OS
/// thread, so the race tests run on a multi-threaded runtime. These hooks
/// exist only under `#[cfg(test)]` and are never installed by production
/// code.
#[cfg(test)]
#[derive(Debug, Default)]
#[allow(clippy::struct_field_names)]
pub(crate) struct CoordinatorProbe {
    /// Parks the next admission when armed.
    pub(crate) admission_gate: Option<Arc<Gate>>,
    /// Parks the next settlement handoff when armed.
    pub(crate) settlement_gate: Option<Arc<Gate>>,
}

/// One two-phase gate of a coordinator boundary (test-only).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Gate {
    state: Mutex<GateState>,
    condvar: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct GateState {
    armed: bool,
    entered: bool,
    proceed: bool,
}

#[cfg(test)]
impl Gate {
    /// Signals that the boundary was entered; when armed, parks until
    /// [`Gate::release`]. An unarmed gate never blocks.
    pub(crate) fn enter(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        if !state.armed {
            return;
        }
        state.entered = true;
        self.condvar.notify_all();
        while !state.proceed {
            state = self
                .condvar
                .wait(state)
                .expect("coordinator probe wait poisoned");
        }
        state.armed = false;
    }

    /// Arms the gate: the next [`Gate::enter`] parks.
    pub(crate) fn arm(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        state.armed = true;
        state.entered = false;
        state.proceed = false;
    }

    /// Blocks until the boundary was entered.
    pub(crate) fn wait_entered(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        while !state.entered {
            state = self
                .condvar
                .wait(state)
                .expect("coordinator probe wait poisoned");
        }
    }

    /// Releases a parked boundary and disarms the gate.
    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("coordinator probe lock poisoned");
        state.proceed = true;
        self.condvar.notify_all();
    }
}

/// The shared state of one conversation runtime.
pub(crate) struct RuntimeInner {
    conversation_id: ConversationId,
    agent_id: AgentId,
    timezone: Option<Tz>,
    context: ConversationContextConfig,
    tool_runtime: ConversationToolRuntime,
    mailbox: ConversationInboundMailbox,
    capability: CapabilityCoordinator,
    clock: Arc<dyn RuntimeClock>,
    /// The Tokio execution runtime this conversation was constructed in.
    ///
    /// Captured (and validated) at construction so
    /// [`ConversationRuntime::activate`] spawns the admission worker
    /// unconditionally, from any thread, with no `Handle::try_current`
    /// dependency at the activation call site.
    executor: tokio::runtime::Handle,
    /// The one admission synchronization boundary.
    state: Mutex<CoordinatorState>,
    /// The runtime admission worker's wake boundary.
    wake: Arc<WakeGate>,
    /// Whether the admission worker task was spawned.
    worker_started: AtomicBool,
    /// The observation queue shared with the Runtime Client projection;
    /// set exactly once when a projection consumer installs itself through
    /// [`RuntimeInner::install_observation_bridge`].
    pending: std::sync::OnceLock<Arc<PendingObservations>>,
    /// Settlement signal: fired once per attempt settlement handoff, so
    /// headless drivers await the authoritative state transfer
    /// deterministically instead of by polling.
    settlement: tokio::sync::Notify,
    /// Test-only coordinator synchronization hooks.
    #[cfg(test)]
    probe: Mutex<Option<CoordinatorProbe>>,
}

/// Releasing the last semantic owner of a conversation runtime closes its
/// observation queue (if one was installed) and its admission wake gate.
/// The admission worker's terminal condition is the closed wake gate.
impl Drop for RuntimeInner {
    fn drop(&mut self) {
        self.wake.close();
        if let Some(pending) = self.pending.get() {
            pending.close();
        }
    }
}

impl RuntimeInner {
    /// Acquires the one admission synchronization boundary.
    fn lock_state(&self) -> MutexGuard<'_, CoordinatorState> {
        self.state
            .lock()
            .expect("conversation runtime lock poisoned")
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
            self.context.status_composer.clone(),
            model,
        )
    }

    /// Publishes one semantic observation into the shared leaf queue,
    /// when a projection consumer exists and the queue is open.
    ///
    /// This is a leaf publication: the runtime keeps no second fold of the
    /// observation vocabulary. Because the queue is installed while the
    /// runtime is still inactive (see
    /// [`RuntimeInner::install_observation_bridge`]) and an inactive
    /// runtime publishes nothing, an installed consumer observes every
    /// observation this runtime ever emits.
    fn observe(&self, observation: ConversationObservation) {
        if let Some(pending) = self.pending.get() {
            pending.push(observation);
        }
    }

    /// Installs the observation bridge and captures the bootstrap seed at
    /// one global cut `R`.
    ///
    /// The whole handshake runs under the one coordinator lock, which is
    /// also what rejects an already-activated runtime atomically against
    /// [`ConversationRuntime::activate`]. See the module documentation
    /// ("The bootstrap cut") for the coherence proof; in short, the
    /// coordinator facts are frozen by the held lock, and every subsystem
    /// semantic commit is lifecycle-gated: the background registry refuses
    /// `commit_dispatch` while its mailbox is bound inactive, the mailbox
    /// refuses inbound while inactive, and the capability coordinator
    /// refuses a runtime-owned `commit` before activation — with the
    /// capability snapshot itself captured at `R`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeBootstrapError::RuntimeAlreadyActivated`] when the
    /// runtime was already activated and
    /// [`RuntimeBootstrapError::BridgeAlreadyInstalled`] when an
    /// observation bridge already exists.
    fn install_observation_bridge(
        self: &Arc<Self>,
        queue: Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        // The one coordinator lock is held across every phase below: it is
        // the freeze that makes the combined seed one real global state.
        let state = self.lock_state();
        // Binding a Runtime Client host is a pre-activation composition
        // decision. Rejecting here, under the lock `activate` also takes,
        // is what makes the lifecycle boundary structural rather than
        // conventional.
        if state.activated {
            return Err(RuntimeBootstrapError::RuntimeAlreadyActivated {
                conversation_id: self.conversation_id.clone(),
            });
        }
        if self.pending.set(queue).is_err() {
            return Err(RuntimeBootstrapError::BridgeAlreadyInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        }
        // ---- T0: the coordinator-owned facts ----
        //
        // An inactive runtime never moved its conversation state into an
        // attempt, so the authoritative canonical history is right here.
        let messages = state
            .conversation
            .as_ref()
            .expect("an inactive runtime owns its conversation state")
            .ledger()
            .audit_records()
            .to_vec();
        let shutting_down = state.shutting_down;
        let model = state.model.view();
        let observer: Arc<RuntimeObserver> = Arc::new(RuntimeObserver::new(self));
        // ---- T1: the background registry (frozen: the registry refuses
        //          commits while its mailbox is bound inactive) ----
        let background = self
            .tool_runtime
            .background()
            .install_observer_and_snapshots(observer.clone());
        // ---- T2: the mailbox (frozen: an inactive conversation refuses
        //          inbound) ----
        let inbound_pending = self.mailbox.install_observer_and_pending(observer.clone());
        // ---- R: the capability coordinator, the cut itself ----
        let capabilities = self.capability.install_observer_and_snapshot(observer);
        drop(state);
        Ok(RuntimeBootstrapSnapshot {
            conversation_id: self.conversation_id.clone(),
            shutting_down,
            messages,
            model,
            inbound_pending,
            background,
            capabilities,
        })
    }

    /// Spawns the admission worker: admits the next attempt whenever the
    /// wake gate fires (any mailbox enqueue), so idle asynchronous inbound
    /// is admitted without any client request.
    ///
    /// Spawning uses the execution-runtime handle captured at
    /// construction, so activation from any thread starts the worker.
    ///
    /// # Lifetime
    ///
    /// The worker never owns the runtime. It captures `Weak<RuntimeInner>`
    /// plus an `Arc<WakeGate>` — the minimal wait state — and it upgrades
    /// the weak handle only inside an admission step, never across an
    /// await. A parked worker therefore holds no strong reference, so it
    /// cannot keep a runtime alive that has no semantic owner left.
    ///
    /// Termination is deterministic, not timed: dropping the last
    /// `Arc<RuntimeInner>` runs `RuntimeInner::drop`, which closes the wake
    /// gate and wakes the worker; the worker observes the closed gate and
    /// exits. The upgrade check is a second, independent exit path.
    fn ensure_worker(self: &Arc<Self>) {
        if self.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let wake = Arc::clone(&self.wake);
        // The mailbox's shared admission wake: every ordinary inbound
        // enqueue notifies it at its publication linearization point, so an
        // idle conversation admits asynchronous inbound without any client
        // request.
        let mailbox_wake = self.mailbox.wake();
        // The execution runtime validated at construction, so activation
        // spawns the worker unconditionally: it can neither panic on a
        // missing runtime nor silently leave a conversation that never
        // admits anything.
        self.executor.spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = wake.notify.notified() => {}
                    () = mailbox_wake.notified() => {}
                }
                if wake.is_closed() {
                    break;
                }
                // The strong handle exists only inside this block, so it is
                // never held across the await above.
                {
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    inner.admit_next_attempt();
                }
            }
            #[cfg(test)]
            wake.signal_worker_exit();
        });
    }

    /// Runs one attempt to settlement against the coordinator-owned
    /// cancellation trigger (the same handle `cancel_current_attempt`
    /// requests cancellation on).
    async fn run_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        conversation: ConversationState,
        fresh: Option<FreshInboundTurn>,
        cancellation: &AgentCancellation,
        model: AttemptModelSnapshot,
    ) -> crate::agent::AgentExecutionResult {
        let lease = self.capability.acquire_attempt_lease();
        let observer = RuntimeObserver::new(self);
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
            conversation,
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
            // The identity lifecycle configuration: enter every step, defer
            // no context. The runtime has no native pre-step policy or
            // tool-result observer consumer, exactly as it has no certified
            // context contributor yet (`ContextRuntime::for_attempt`). A
            // configured owner arrives with the consumer that needs it, not
            // as speculative plumbing.
            crate::agent::AttemptLifecycle::inert(),
        )
        // Neither rejection is reachable: `conversation_id` *is* the tool
        // runtime's own identity (the runtime has no independent
        // conversation authority to disagree with it), and construction
        // validated the coordinator against that same runtime.
        .expect("the conversation runtime derives its identity from this tool runtime");
        execution.observe(&observer);
        execution.run().await
    }

    /// The settlement path of one attempt: transfer the authoritative
    /// conversation state back to the coordinator, clear the current-attempt
    /// slot, then hand off to the next admission when the mailbox holds
    /// pending work.
    ///
    /// The result is consumed by value: its `conversation` becomes the
    /// coordinator's authoritative conversation state again.
    #[allow(clippy::needless_pass_by_value)]
    fn finish_attempt(
        self: &Arc<Self>,
        attempt_id: AttemptId,
        result: crate::agent::AgentExecutionResult,
    ) {
        {
            let mut state = self.lock_state();
            state
                .request_history
                .append(result.request_snapshots)
                .expect("each admitted request identity is transferred exactly once");
            state.conversation = Some(result.conversation);
            if state
                .current_attempt
                .as_ref()
                .is_some_and(|current| current.attempt_id == attempt_id)
            {
                state.current_attempt = None;
            }
            self.settlement.notify_one();
            // Test-only gate: the conversation state is restored and the
            // current-attempt slot is cleared, but the next-admission
            // handoff has not run yet. An enqueue during this park
            // deterministically races the settlement boundary. The gate
            // parks only when armed and disarms after one park.
            #[cfg(test)]
            if let Some(probe) = self
                .probe
                .lock()
                .expect("coordinator probe lock poisoned")
                .as_ref()
                && let Some(gate) = &probe.settlement_gate
            {
                gate.enter();
            }
        }
        self.admit_next_attempt();
    }

    /// Admits one attempt when the runtime is idle and the mailbox holds
    /// pending work.
    ///
    /// This is the **one** attempt-admission authority. Every producer —
    /// the Runtime Client human submit path, runtime/agent inbound,
    /// background completion, future subagent/fleet/external producers —
    /// publishes through the inbound mailbox and is admitted here or is
    /// consumed by the active attempt at its safe boundary.
    ///
    /// # Linearization
    ///
    /// The activation observation, the idle observation, the
    /// shutdown/admission-gate observation, the
    /// finite mailbox drain, the canonical-history commits, the attempt-id
    /// allocation, the model snapshot freeze, and the current-attempt
    /// publication all share the one coordinator lock. The mailbox drain
    /// fires its observer only into the leaf pending queue, never back into
    /// this lock. After the publication the lock is released and the
    /// attempt task is spawned, so at most one active [`AgentExecution`]
    /// exists per conversation.
    fn admit_next_attempt(self: &Arc<Self>) {
        // Test-only gate: parks before the coordinator lock, so a competing
        // publish can still enqueue while the admission is gated. The gate
        // parks only when armed and disarms after one park.
        #[cfg(test)]
        if let Some(probe) = self
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            && let Some(gate) = &probe.admission_gate
        {
            gate.enter();
        }
        let mut state = self.lock_state();
        if !state.activated || state.shutting_down || state.current_attempt.is_some() {
            return;
        }
        let Some(batch) = self.mailbox.drain() else {
            return;
        };
        // Ownership transfer: the coordinator hands its conversation state
        // to the attempt. From here until settlement the coordinator holds
        // `None` and the attempt is the single mutable conversation
        // authority.
        let mut conversation = state
            .conversation
            .take()
            .expect("the coordinator owns the conversation state while idle");
        let mut fresh_ids = Vec::with_capacity(batch.items().len());
        for item in batch.into_items() {
            let block = MessageBlock::User(item.into_message());
            let message_id = conversation
                .commit(block.clone())
                .expect("a mailbox-assigned inbound identity is unique");
            self.observe(ConversationObservation::Committed {
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
        // against the same signal, so protocol cancellation always reaches
        // the loop.
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        state.current_attempt = Some(CurrentAttempt {
            attempt_id: attempt_id.clone(),
            cancellation: cancellation.clone(),
        });
        self.observe(ConversationObservation::AttemptAdmitted {
            attempt_id: attempt_id.clone(),
        });
        // The attempt model snapshot is taken at exactly this admission
        // linearization boundary, under the same lock that publishes the
        // attempt. A `model_set` that linearizes before this point is
        // observed by the attempt; one that linearizes after it affects only
        // future attempts.
        let model = state.model.snapshot();
        self.observe(ConversationObservation::AttemptModelFrozen {
            attempt_id: attempt_id.clone(),
            model: Box::new(model.view()),
        });
        drop(state);
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let result = inner
                .run_attempt(
                    attempt_id.clone(),
                    conversation,
                    Some(fresh),
                    &cancellation,
                    model,
                )
                .await;
            inner.finish_attempt(attempt_id, result);
        });
    }
}

/// The conversation runtime coordinator of one conversation.
///
/// Construct one runtime per conversation instance; the runtime installs
/// the observation seams on the mailbox, the background registry, and the
/// capability coordinator exactly once, and spawns its admission worker.
/// The runtime is cheaply cloneable and all clones share one state.
#[derive(Clone)]
pub struct ConversationRuntime {
    pub(crate) inner: Arc<RuntimeInner>,
}

impl core::fmt::Debug for ConversationRuntime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConversationRuntime")
            .field("conversation_id", &self.inner.conversation_id)
            .finish()
    }
}

impl ConversationRuntime {
    /// Creates the runtime in its **inactive** lifecycle state.
    ///
    /// An inactive runtime is inert: its mailbox refuses inbound, it has
    /// no admission worker, it admits no attempt, and it publishes no
    /// observation. The composition may now optionally bind a
    /// `RuntimeClientHost` over it, and must then call
    /// [`ConversationRuntime::activate`] before semantic execution can
    /// begin. A headless composition simply activates directly.
    ///
    /// # One conversation authority
    ///
    /// The conversation identity of the runtime *is*
    /// [`ConversationToolRuntime::conversation_id`]. The configuration
    /// carries no conversation id of its own, so the runtime's identity, the
    /// canonical mailbox, the authoritative background registry, and the
    /// Runtime Client binding identity all name one conversation by
    /// construction. Every conversation-scoped value this runtime derives —
    /// generated inbound message ids, generated attempt ids, and every
    /// [`AgentExecutionRequest`] it issues — uses that one identity.
    ///
    /// The capability coordinator is a *separate* authoritative identity,
    /// so it is still validated explicitly against the runtime.
    ///
    /// # Construction order
    ///
    /// Every fallible validation runs before the runtime exists, so a
    /// rejected construction has no semantic side effect at all.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationRuntimeError::OwnershipMismatch`] when the
    /// capability coordinator and the conversation tool runtime do not
    /// share the same conversation/workspace ownership domain,
    /// [`ConversationRuntimeError::Context`] when the context engine
    /// configuration is impossible, and
    /// [`ConversationRuntimeError::InvalidInitialConversation`] when the
    /// initial canonical messages are invalid.
    pub fn new(config: RuntimeConversationConfig) -> Result<Self, ConversationRuntimeError> {
        // The one conversation authority at this boundary: every identity
        // this runtime publishes or derives comes from the tool runtime it
        // coordinates, so runtime and tool runtime cannot disagree.
        let conversation_id = config.tool_runtime.conversation_id().clone();

        // ---- Fallible validation: nothing below is observable yet. ----
        let snapshot = config.capability.current_snapshot();
        // The coordinator is a separate authoritative identity, so it is
        // still validated explicitly against the runtime's identity.
        if snapshot.conversation_id() != &conversation_id
            || snapshot.workspace_root() != config.tool_runtime.workspace().root()
        {
            return Err(ConversationRuntimeError::OwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: conversation_id,
            });
        }
        // The initial session model must be able to run under the session
        // context policy. Validating here (and again in `model_set`) is what
        // makes the per-attempt context runtime construction infallible at
        // admission, where there is no caller left to report to.
        validate_context_policy(&config.context.policy, &config.model.snapshot())
            .map_err(|error| ConversationRuntimeError::Context(error.message))?;
        // The bootstrap conversation state is built here, in the fallible
        // section: a rejected bootstrap leaves no runtime behind.
        let conversation = ConversationState::from_messages(config.initial_messages.clone())
            .map_err(|error| {
                ConversationRuntimeError::InvalidInitialConversation(error.to_string())
            })?;
        // Activation spawns the admission worker, and a runtime with no
        // worker would silently never admit anything. The execution
        // runtime is required — and captured — here, still in the fallible
        // section and before any claim, so `activate` spawns the worker
        // unconditionally and the impossible composition is rejected as
        // early as it can be seen.
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return Err(ConversationRuntimeError::NoExecutionRuntime);
        };

        // ---- Ownership commit: the one-time coordinator binding claim. ----
        //
        // The runtime identity is claimed first because it is the canonical
        // mailbox/background identity this runtime coordinates. If the
        // coordinator is already bound, the runtime claim is released again:
        // a rejected construction must leave no trace, and this is the only
        // place a claim is ever released.
        if !config.tool_runtime.claim_conversation_runtime() {
            return Err(ConversationRuntimeError::RuntimeAlreadyBound { conversation_id });
        }
        if !config.capability.claim_conversation_runtime() {
            config.tool_runtime.release_conversation_runtime_claim();
            return Err(ConversationRuntimeError::RuntimeAlreadyBound { conversation_id });
        }

        // ---- Infallible wiring: from here construction always succeeds. ----
        let mailbox = config.tool_runtime.mailbox();
        // The conversation is inert until `activate`: its mailbox refuses
        // inbound, so nothing can be admitted and nothing can be observed
        // while the optional Runtime Client host binds.
        mailbox.bind_inactive();
        let clock = config
            .clock
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn RuntimeClock>);
        let inner = Arc::new(RuntimeInner {
            conversation_id,
            agent_id: config.agent_id,
            timezone: config.timezone,
            context: config.context,
            tool_runtime: config.tool_runtime,
            mailbox,
            capability: config.capability,
            clock,
            executor,
            state: Mutex::new(CoordinatorState {
                model: config.model,
                request_history: RequestHistory::default(),
                conversation: Some(conversation),
                activated: false,
                current_attempt: None,
                shutting_down: false,
                next_attempt_seq: 0,
                next_inbound_seq: 0,
            }),
            wake: Arc::new(WakeGate::new()),
            worker_started: AtomicBool::new(false),
            pending: std::sync::OnceLock::new(),
            settlement: tokio::sync::Notify::new(),
            #[cfg(test)]
            probe: Mutex::new(None),
        });
        Ok(Self { inner })
    }

    /// Creates the runtime with the test-only coordinator synchronization
    /// hooks installed. Only available under `#[cfg(test)]`; never used by
    /// production code.
    #[cfg(test)]
    pub(crate) fn with_probe(
        config: RuntimeConversationConfig,
        probe: CoordinatorProbe,
    ) -> Result<Self, ConversationRuntimeError> {
        let runtime = Self::new(config)?;
        *runtime
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned") = Some(probe);
        Ok(runtime)
    }

    /// The conversation identity of this runtime.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.inner.conversation_id
    }

    /// The agent executed by attempts of this runtime.
    #[must_use]
    pub fn agent_id(&self) -> &AgentId {
        &self.inner.agent_id
    }

    /// The one conversation tool runtime of this runtime.
    #[must_use]
    pub fn tool_runtime(&self) -> &ConversationToolRuntime {
        &self.inner.tool_runtime
    }

    /// The shared session-owned context plane of this runtime.
    ///
    /// The context policy, the token estimator, and the Agent Status
    /// composer persist across attempts; each attempt derives its
    /// [`ContextRuntime`](crate::context::ContextRuntime) from this plane
    /// plus that attempt's frozen model snapshot.
    #[must_use]
    pub fn context_config(&self) -> &ConversationContextConfig {
        &self.inner.context
    }

    /// The one capability coordinator of this runtime.
    #[must_use]
    pub fn capability(&self) -> &CapabilityCoordinator {
        &self.inner.capability
    }

    /// Installs the observation bridge shared with the Runtime Client
    /// adapter (or a headless observer) and captures the semantic
    /// bootstrap snapshot, as one linearizable handoff.
    ///
    /// See the module documentation ("Adapter bootstrap linearization")
    /// for the cut contract: every authority contributes its seed under
    /// its own lock in the same section that installs its observation
    /// seam, and the queue is installed under the same section that
    /// captures the coordinator-owned facts and the runtime semantic
    /// record, so a transition can never be lost between a seed and the
    /// live observation stream and can never be applied twice.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeBootstrapError::BridgeAlreadyInstalled`] when an
    /// observation bridge was already installed (a previous headless
    /// consumer). The one-time Runtime Client binding claim guarantees
    /// this cannot happen for a production adapter, and a failed host
    /// construction releases that claim again.
    pub(crate) fn install_observation_bridge(
        &self,
        queue: Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        self.inner.install_observation_bridge(queue)
    }

    /// Claims the one-time Runtime Client binding of the tool runtime and of
    /// the capability coordinator.
    ///
    /// Protocol v1 binds one runtime identity to at most one Runtime Client
    /// adapter for that identity's lifetime, so cloning a runtime never
    /// yields a second bindable identity and dropping the bound adapter
    /// never makes it bindable again. Reconnect replaces the attachment,
    /// not the adapter.
    pub(crate) fn claim_client_binding(&self) -> bool {
        if !self.inner.tool_runtime.claim_runtime_client() {
            return false;
        }
        if !self.inner.capability.claim_runtime_client() {
            self.inner.tool_runtime.release_runtime_client_claim();
            return false;
        }
        true
    }

    /// Releases a Runtime Client binding claimed by a host construction
    /// that then failed.
    ///
    /// This exists only so a rejected `RuntimeClientHost::new` (whose
    /// observation bridge install failed after the claim) leaves no trace;
    /// it is never called on host drop, and a successfully constructed
    /// host never releases its binding.
    pub(crate) fn release_client_binding(&self) {
        self.inner.tool_runtime.release_runtime_client_claim();
        self.inner.capability.release_runtime_client_claim();
    }

    /// Activates the runtime: semantic execution may begin.
    ///
    /// This is the one explicit lifecycle boundary of Issue #61. Before
    /// it, the runtime is inert and a `RuntimeClientHost` may bind over
    /// it; at it, the Runtime Client host-binding decision is frozen, the
    /// mailbox opens, and the admission worker starts. Activation is the
    /// bootstrap cut a bound Runtime Client projection is seeded against.
    ///
    /// Runtime Client *attachments* remain fully dynamic afterwards: this
    /// boundary freezes only which adapter (if any) observes the runtime,
    /// never how long a client stays attached.
    ///
    /// Activating twice is a no-op; the first activation wins.
    pub fn activate(&self) {
        {
            let mut state = self.inner.lock_state();
            if state.activated {
                return;
            }
            state.activated = true;
            // Under the same lock section, so an admission, a bootstrap
            // handshake, or a runtime-owned subsystem commit can never
            // observe a half-activated runtime: the mailbox opens and the
            // capability coordinator's runtime-owned commit gate opens at
            // the same linearization point.
            self.inner.mailbox.activate();
            self.inner.capability.activate_conversation();
        }
        self.inner.ensure_worker();
        // Any inbound published before activation (there can be none: the
        // mailbox refused it) and any inbound racing this activation is
        // admitted here rather than depending on a wake permit.
        self.inner.admit_next_attempt();
    }

    /// Whether this runtime was activated.
    #[must_use]
    pub fn is_activated(&self) -> bool {
        self.inner.lock_state().activated
    }

    /// Submits one ordinary inbound user message.
    ///
    /// The runtime owns authoritative metadata: the message identity, the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all runtime-assigned. Success means accepted/published, never
    /// assistant-finished: the runtime wake gate admits the next attempt
    /// when the runtime is idle, and while an attempt is running the message
    /// waits in the authoritative mailbox for the next safe-boundary drain.
    ///
    /// # Errors
    ///
    /// Returns [`InboundAdmissionError::Inactive`] before activation,
    /// [`InboundAdmissionError::Shutdown`] after shutdown,
    /// [`InboundAdmissionError::EmptyContent`] for empty content, and
    /// [`InboundAdmissionError::Mailbox`] for a mailbox admission failure.
    pub fn submit_inbound(
        &self,
        content: Vec<UserContentBlock>,
    ) -> Result<InboundAdmission, InboundAdmissionError> {
        if content.is_empty() {
            return Err(InboundAdmissionError::EmptyContent);
        }
        let (message_id, timestamp) = {
            let mut state = self.inner.lock_state();
            if !state.activated {
                return Err(InboundAdmissionError::Inactive);
            }
            if state.shutting_down {
                return Err(InboundAdmissionError::Shutdown);
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
        let message = crate::message::types::UserMessageBlock {
            id: message_id.clone(),
            content,
            source: crate::message::types::UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: Some(timestamp),
        };
        let sequence = self
            .inner
            .mailbox
            .enqueue(message)
            .map_err(InboundAdmissionError::Mailbox)?;
        Ok(InboundAdmission {
            message_id,
            inbound_sequence: sequence,
        })
    }

    /// Requests cancellation of one named current attempt.
    ///
    /// Acceptance is not terminal settlement: actual settlement remains
    /// owned by the Agent Loop and is observed asynchronously. The
    /// attempt-id check under the coordinator lock guarantees that the
    /// cancellation signal is never delivered to a *different* attempt that
    /// was admitted after the named one settled.
    ///
    /// # Errors
    ///
    /// Returns [`CancelAttemptError::NoCurrentAttempt`] when no attempt
    /// with the given identity is currently cancellable.
    pub fn cancel_current_attempt(
        &self,
        attempt_id: &AttemptId,
    ) -> Result<AttemptId, CancelAttemptError> {
        let state = self.inner.lock_state();
        let Some(current) = state
            .current_attempt
            .as_ref()
            .filter(|current| current.attempt_id == *attempt_id)
        else {
            return Err(CancelAttemptError::NoCurrentAttempt);
        };
        current.cancellation.cancel();
        Ok(current.attempt_id.clone())
    }

    /// Reads the authoritative session model catalog.
    #[must_use]
    pub fn model_catalog(&self) -> ModelCatalogView {
        self.inner.lock_state().model.catalog_view()
    }

    /// Replaces the authoritative session model configuration.
    ///
    /// # Lifecycle
    ///
    /// A model update is a live semantic mutation: it is refused with the
    /// typed [`ModelUpdateError::Inactive`] while the runtime is inactive
    /// and consumes nothing. Construction-time model configuration belongs
    /// in the [`SessionModelState`] supplied to
    /// [`ConversationRuntime::new`](Self::new), not in a live mutation of a
    /// runtime that has not started.
    ///
    /// # Linearization
    ///
    /// The whole operation — resolution, validation, and state replacement
    /// — happens under the one coordinator lock that also owns attempt
    /// admission. An update therefore either linearizes before an admission
    /// (and that attempt observes it) or after it (and only later attempts
    /// observe it). There is no third possibility and no timing assumption.
    ///
    /// # Transactionality
    ///
    /// A rejected update changes nothing: the session keeps its previous
    /// configuration and no observation is published.
    ///
    /// # Errors
    ///
    /// Returns [`ModelUpdateError::Inactive`] before activation and
    /// [`ModelUpdateError::InvalidConfiguration`] when the configuration
    /// cannot be resolved against the catalog or cannot run under the
    /// session context policy.
    pub fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<SessionModelView, ModelUpdateError> {
        let mut state = self.inner.lock_state();
        if !state.activated {
            return Err(ModelUpdateError::Inactive);
        }
        // Resolve into a scratch copy first: `SessionModelState::apply` is
        // itself transactional, and the context-policy check runs against the
        // *candidate* snapshot before anything is published.
        let mut candidate = state.model.clone();
        candidate
            .apply(config)
            .map_err(|error| invalid_model(&error))?;
        validate_context_policy(&self.inner.context.policy, &candidate.snapshot()).map_err(
            |error| {
                ModelUpdateError::InvalidConfiguration(format!(
                    "the selected model cannot run under the session context policy: {}",
                    error.message
                ))
            },
        )?;
        let view = candidate.view();
        state.model = candidate;
        self.inner
            .observe(ConversationObservation::SessionModelChanged {
                model: Box::new(view.clone()),
            });
        Ok(view)
    }

    /// The authoritative session model view.
    #[must_use]
    pub fn model_view(&self) -> SessionModelView {
        self.inner.lock_state().model.view()
    }

    /// The authoritative session model configuration (test convenience).
    #[cfg(test)]
    pub(crate) fn model_config(&self) -> SessionModelConfig {
        self.inner.lock_state().model.config().clone()
    }

    /// Accepts the local-runtime shutdown request.
    ///
    /// Shutdown is not detach and not cancellation: the current attempt
    /// continues to its settlement, semantic runtime work is never mutated,
    /// and no further inbound admission occurs. The acceptance is published
    /// as the [`ConversationObservation::Shutdown`] observation.
    ///
    /// # Lifecycle
    ///
    /// Shutdown is a live semantic mutation: shutting down a conversation
    /// that has never activated is refused with the typed
    /// [`ShutdownError::Inactive`] and consumes nothing — no shutting-down
    /// state, no cursor, no [`ConversationObservation::Shutdown`] event. An
    /// inactive conversation has no runtime lifecycle to end.
    ///
    /// # Errors
    ///
    /// Returns [`ShutdownError::Inactive`] before activation. After
    /// activation shutdown is accepted (idempotently) and never fails.
    pub fn shutdown(&self) -> Result<(), ShutdownError> {
        let mut state = self.inner.lock_state();
        if !state.activated {
            return Err(ShutdownError::Inactive);
        }
        if !state.shutting_down {
            state.shutting_down = true;
            self.inner.observe(ConversationObservation::Shutdown);
        }
        Ok(())
    }

    /// Returns the immutable in-memory request facts retained by this
    /// runtime.
    ///
    /// The runtime owns these snapshots after attempt settlement. The
    /// returned value is a read-only clone of the request-fact collection;
    /// it does not create another conversation or transcript authority.
    #[must_use]
    pub fn request_history(&self) -> RequestHistory {
        self.inner.lock_state().request_history.clone()
    }

    /// Reconstructs one retained provider-neutral request from its frozen
    /// snapshot and the exact historical Surface revisions in the runtime's
    /// authoritative `ConversationState`.
    ///
    /// While an attempt is running, that single `ConversationState` is moved
    /// into the attempt and this read is explicitly unavailable. Once the
    /// attempt settles, the same state returns to the runtime and
    /// reconstruction is again available without consulting live
    /// configuration or sources.
    ///
    /// # Errors
    ///
    /// Returns
    /// [`RequestHistoryError::ConversationUnavailable`](crate::runtime::request_history::RequestHistoryError::ConversationUnavailable)
    /// while the single `ConversationState` is owned by a running attempt,
    /// or a lookup / historical reconstruction error for an unknown or
    /// invalid request.
    pub fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, crate::runtime::request_history::RequestHistoryError> {
        let state = self.inner.lock_state();
        let conversation = state
            .conversation
            .as_ref()
            .ok_or(crate::runtime::request_history::RequestHistoryError::ConversationUnavailable)?;
        state.request_history.reconstruct(identity, conversation)
    }

    /// Inspects one background execution through the authoritative
    /// registry.
    #[must_use]
    pub fn background_status(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        self.inner.tool_runtime.background().snapshot(execution_id)
    }

    /// Requests cancellation of one background execution through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct.
    #[must_use]
    pub fn background_cancel(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Option<BackgroundExecutionSnapshot> {
        self.inner.tool_runtime.background().cancel(execution_id)
    }

    /// The settlement handoff signal of this runtime: fired once per
    /// attempt settlement, so a headless driver can await the
    /// authoritative state transfer deterministically instead of by
    /// polling.
    #[must_use]
    pub fn settlement_signal(&self) -> &tokio::sync::Notify {
        &self.inner.settlement
    }
}

/// The one runtime observation-bridge failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeBootstrapError {
    /// An observation bridge was already installed over this runtime.
    ///
    /// The one-time Runtime Client binding claim makes this unreachable
    /// for a production adapter; it exists so a rejected host construction
    /// over a runtime already observed by a headless consumer fails typed
    /// and releases its claim instead of silently sharing a queue.
    BridgeAlreadyInstalled {
        /// The conversation whose runtime is already bridged.
        conversation_id: ConversationId,
    },
    /// The runtime was already activated.
    ///
    /// Binding a Runtime Client host is a pre-activation composition
    /// decision (Issue #61): the bootstrap seed is only coherent while the
    /// runtime is inert, so a late bind is refused rather than
    /// approximated.
    RuntimeAlreadyActivated {
        /// The conversation whose runtime is already activated.
        conversation_id: ConversationId,
    },
}

impl core::fmt::Display for RuntimeBootstrapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BridgeAlreadyInstalled { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} already has an observation bridge installed"
            ),
            Self::RuntimeAlreadyActivated { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} is already activated; a Runtime Client host binds before activation"
            ),
        }
    }
}

impl std::error::Error for RuntimeBootstrapError {}

/// The semantic facts a Runtime Client adapter seeds from at construction:
/// the runtime-owned bootstrap snapshot captured by
/// [`ConversationRuntime::install_observation_bridge`] at the global cut
/// `R`.
///
/// Every field is a runtime-owned semantic source type or runtime-owned
/// immutable view; the Runtime Client layer owns the translation into its
/// snapshot read model.
///
/// There is deliberately no attempt / Agent Status / compaction field: a
/// runtime binds its Runtime Client host while inactive, so no attempt has
/// ever run, no status has been composed, and no compaction has occurred.
/// Every one of those facts arrives through the live observation stream.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeBootstrapSnapshot {
    /// The conversation identity.
    pub conversation_id: ConversationId,
    /// Whether the runtime accepted shutdown.
    pub shutting_down: bool,
    /// The authoritative canonical history at the cut, read from the
    /// coordinator-owned `ConversationState`.
    pub messages: Vec<MessageBlock>,
    /// The authoritative session model view.
    pub model: SessionModelView,
    /// The currently pending inbound items, in mailbox sequence order.
    pub inbound_pending: Vec<InboundItem>,
    /// The authoritative background execution records at the cut.
    ///
    /// Provably empty under the Issue #61 lifecycle: the background
    /// registry refuses `commit_dispatch` while its mailbox is bound
    /// inactive, so no record can exist when the bridge is installed. The
    /// seed is captured anyway, in the same registry section that installs
    /// the observer, so the handshake is one coherent cut for whatever
    /// state exists.
    pub background: Vec<BackgroundExecutionSnapshot>,
    /// The active authoritative capability snapshot.
    pub capabilities: Arc<crate::capabilities::CapabilitySnapshot>,
}

/// The accepted identity of one submitted inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAdmission {
    /// The runtime-assigned message identity.
    pub message_id: MessageId,
    /// The mailbox-assigned inbound sequence.
    pub inbound_sequence: InboundSequence,
}

/// An inbound publish failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundAdmissionError {
    /// The runtime was not activated: an inert conversation accepts no
    /// inbound work.
    Inactive,
    /// The runtime accepted shutdown: no further inbound admission occurs.
    Shutdown,
    /// Inbound content must not be empty.
    EmptyContent,
    /// The authoritative mailbox rejected the message.
    Mailbox(MailboxError),
}

impl core::fmt::Display for InboundAdmissionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Inactive => f.write_str("the conversation runtime is not activated"),
            Self::Shutdown => f.write_str("the conversation runtime is shutting down"),
            Self::EmptyContent => f.write_str("inbound content must not be empty"),
            Self::Mailbox(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for InboundAdmissionError {}

/// A cancellation request failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelAttemptError {
    /// No attempt with the given identity is currently cancellable.
    NoCurrentAttempt,
}

/// A session model update failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelUpdateError {
    /// The runtime has not been activated: a live semantic mutation of an
    /// inert conversation is refused and consumes nothing.
    Inactive,
    /// The configuration cannot be resolved against the catalog or cannot
    /// run under the session context policy.
    InvalidConfiguration(String),
}

/// A runtime shutdown failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    /// The runtime has not been activated: an inert conversation has no
    /// runtime lifecycle to end, so the request is refused and nothing is
    /// published.
    Inactive,
}

/// Projects a model-resolution failure into a descriptive message.
///
/// Resolution errors never carry credential material: the catalog names an
/// environment variable at most.
fn invalid_model(error: &ModelInvocationError) -> ModelUpdateError {
    ModelUpdateError::InvalidConfiguration(error.to_string())
}

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

/// The observation seam implementation bridging the authoritative runtime
/// owners into the shared leaf observation queue.
///
/// [`RuntimeObserver::push`] serves the mailbox, the background registry,
/// the capability coordinator, and `AgentExecution`, all of which fire
/// while holding (or being owned by) their authoritative boundary. The
/// observation is appended to the leaf queue and the projection worker is
/// woken — nothing else. These paths never acquire the coordinator lock or
/// the Runtime Client projection lock, and the runtime performs no fold of
/// its own.
///
/// # Lifetime
///
/// The observer is **non-owning**. Authoritative subsystems keep it alive
/// (`Arc<dyn InboundObserver>` and friends are unchanged), but it holds
/// only a `Weak<RuntimeInner>`, so the edge
/// `RuntimeInner -> subsystem -> Arc<RuntimeObserver> -> RuntimeInner` is
/// broken: installing an observation seam never extends a runtime's
/// lifetime.
///
/// Every callback upgrades the weak handle and returns without publishing
/// when the upgrade fails — the observation consumer simply no longer
/// exists. That is never an error for the subsystem: the mailbox, the
/// background registry, and the capability coordinator stay authoritative
/// whether or not a runtime observes them. The upgrade is transient and
/// confined to the callback, so an observer can neither resurrect nor
/// prolong a runtime.
pub(crate) struct RuntimeObserver {
    inner: Weak<RuntimeInner>,
}

impl RuntimeObserver {
    /// Creates the non-owning observer of one runtime.
    ///
    /// Installed by [`RuntimeInner::install_observation_bridge`] on behalf
    /// of the Runtime Client adapter (or a headless observation consumer);
    /// a conversation runtime with zero attachments has no subsystem
    /// observation seams and runs the identical admission/execution path.
    pub(crate) fn new(inner: &Arc<RuntimeInner>) -> Self {
        Self {
            inner: Arc::downgrade(inner),
        }
    }

    /// Appends one observation to the leaf queue, without acquiring the
    /// coordinator lock or the Runtime Client projection lock.
    ///
    /// This is the only shape legal from a subsystem observer that fires
    /// while its authoritative lock is held.
    pub(crate) fn push(&self, observation: ConversationObservation) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        inner.observe(observation);
    }
}

impl AgentExecutionObserver for RuntimeObserver {
    fn observe_event(&self, attempt_id: &AttemptId, event: &RuntimeEvent) {
        self.push(ConversationObservation::Event {
            attempt_id: attempt_id.clone(),
            event: event.clone(),
        });
    }

    fn observe_committed(&self, attempt_id: &AttemptId, block: &MessageBlock) {
        self.push(ConversationObservation::Committed {
            attempt_id: Some(attempt_id.clone()),
            block: block.clone(),
        });
    }

    fn observe_status(&self, observation: &AgentStatusObservation) {
        self.push(ConversationObservation::Status(observation.clone()));
    }
}

// The mailbox fires `on_enqueued`/`on_drained` while the mailbox lock is
// held, and `admit_next_attempt` drains the mailbox under the coordinator
// lock: taking the coordinator lock here would close the cycle. Enqueue
// into the leaf queue only; admission wakeup is the mailbox's own shared
// wake handle (see [`ConversationInboundMailbox::wake`]), which fires at
// the same publication linearization point whether or not any observer is
// installed.
impl InboundObserver for RuntimeObserver {
    fn on_enqueued(&self, item: &InboundItem) {
        self.push(ConversationObservation::InboundEnqueued(item.clone()));
    }

    fn on_drained(&self, batch: &InboundBatch) {
        self.push(ConversationObservation::InboundDrained(batch.clone()));
    }
}

// The registry fires `on_snapshot` while the registry lock is held. Push
// only, so no `coordinator -> registry` ordering discipline is ever
// required of a caller.
impl BackgroundObserver for RuntimeObserver {
    fn on_snapshot(&self, snapshot: &BackgroundExecutionSnapshot) {
        self.push(ConversationObservation::Background(snapshot.clone()));
    }
}

// The coordinator fires `on_snapshot` while the capability state lock is
// held, with an attempt commit blocked behind it. Push only, so an
// authoritative capability commit never waits on the coordinator lock or
// the Runtime Client projection lock. The observation carries the
// authoritative `CapabilitySnapshot`; the Runtime Client projection owns
// the translation into its capability view.
impl CapabilityObserver for RuntimeObserver {
    fn on_snapshot(&self, snapshot: &CapabilitySnapshot) {
        self.push(ConversationObservation::Capability(Arc::new(
            snapshot.clone(),
        )));
    }
}

#[cfg(test)]
impl ConversationRuntime {
    /// The runtime-owned Message Ledger records, or `None` while an attempt
    /// owns the conversation state.
    pub(crate) fn coordinator_ledger(&self) -> Option<Vec<MessageBlock>> {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .conversation
            .as_ref()
            .map(|conversation| conversation.ledger().audit_records().to_vec())
    }

    /// The runtime-owned active Surface identities, or `None` while an
    /// attempt owns the conversation state.
    pub(crate) fn coordinator_active_ids(&self) -> Option<Vec<MessageId>> {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .conversation
            .as_ref()
            .map(|conversation| conversation.active_ids().to_vec())
    }

    #[allow(dead_code)] // used by the race regression tests
    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime lock")
            .current_attempt
            .is_some()
    }

    /// A non-owning handle to the shared runtime state, for lifetime tests.
    pub(crate) fn weak_inner(&self) -> Weak<RuntimeInner> {
        Arc::downgrade(&self.inner)
    }

    /// Installs the deterministic admission-worker exit signal, for
    /// lifetime tests.
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        self.inner.wake.install_worker_exit_probe(sender);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        CancelAttemptError, ConversationContextConfig, ConversationRuntime,
        ConversationRuntimeError, CoordinatorProbe, InboundAdmissionError, PendingObservations,
        RuntimeConversationConfig,
    };
    use crate::context::{AgentStatusComposer, DefaultTokenEstimator, TokenEstimator};
    use crate::message::content::TextBlock;
    use crate::message::types::{MessageBlock, UserContentBlock, UserSource};
    use crate::model::adapter::ModelAdapter;
    use crate::runtime::identity::{AgentId, ConversationId};
    use crate::runtime::observation::ConversationObservation;
    use crate::scripted_suites::support::fake::{FakeModel, FakeStep};
    use crate::scripted_suites::support::model::scripted_session_model;

    /// A headless runtime fixture: the conversation runtime with zero
    /// Runtime Client attachments, over a scripted model adapter and an
    /// optional runtime-owned observation bridge the test folds itself.
    struct HeadlessFixture {
        _dir: tempfile::TempDir,
        runtime: ConversationRuntime,
        model: Arc<FakeModel>,
        pending: Option<Arc<PendingObservations>>,
    }

    fn text_content(text: &str) -> Vec<UserContentBlock> {
        vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })]
    }

    fn one_turn_script() -> Vec<FakeStep> {
        use crate::message::types::ContentBlockIndex;
        use crate::model::event::ModelEvent;
        use crate::model::finish::ModelFinishReason;
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
    }

    /// Builds the conversation runtime of one headless fixture.
    async fn headless_runtime(
        dir: &tempfile::TempDir,
        scripts: Vec<Vec<FakeStep>>,
        base_tool_registry: Option<crate::tools::executor::ToolRegistry>,
        probe: Option<CoordinatorProbe>,
    ) -> (ConversationRuntime, Arc<FakeModel>) {
        let conversation_id = ConversationId::new("conv-headless");
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
                base_tool_registry: Arc::new(base_tool_registry.unwrap_or_default()),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let model = Arc::new(FakeModel::new(scripts));
        let adapter: Arc<dyn ModelAdapter> = model.clone();
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let config = RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter),
            timezone: None,
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_composer: AgentStatusComposer::default(),
            },
            tool_runtime,
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
        };
        let runtime = match probe {
            Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
            None => ConversationRuntime::new(config).expect("runtime"),
        };
        (runtime, model)
    }

    /// A headless fixture whose runtime has the runtime-owned observation
    /// bridge installed (no Runtime Client exists): the test folds
    /// `ConversationObservation`s from the shared queue.
    async fn headless_fixture_with(probe: Option<CoordinatorProbe>) -> HeadlessFixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let (runtime, model) = headless_runtime(&dir, vec![one_turn_script()], None, probe).await;
        let pending = Arc::new(PendingObservations::new());
        runtime
            .install_observation_bridge(pending.clone())
            .expect("first bridge install");
        // The bridge installed over the inert runtime; activation is the
        // cut, exactly as in the interactive composition.
        runtime.activate();
        HeadlessFixture {
            _dir: dir,
            runtime,
            model,
            pending: Some(pending),
        }
    }

    async fn headless_fixture() -> HeadlessFixture {
        headless_fixture_with(None).await
    }

    /// A runtime-source inbound message with runtime-assigned metadata.
    fn runtime_inbound_message(id: &str) -> crate::message::types::UserMessageBlock {
        crate::message::types::UserMessageBlock {
            id: crate::runtime::identity::MessageId::new(id),
            content: text_content("async"),
            source: UserSource::Runtime,
            kind: crate::message::types::InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse")
                    .with_timezone(&chrono::Utc),
            ),
        }
    }

    /// The headless full turn: a conversation runtime with zero Runtime
    /// Client attachments publishes ordinary inbound, runs a real
    /// `AgentExecution` through the real Context Assembly / provider path,
    /// and settles canonically.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn headless_full_turn_without_any_runtime_client() {
        let fixture = headless_fixture().await;

        let admission = fixture
            .runtime
            .submit_inbound(text_content("hello"))
            .expect("accepted");
        assert_eq!(admission.message_id.as_str(), "conv-headless-inbound-1");
        assert_eq!(admission.inbound_sequence.get(), 1);

        // The terminal settlement is observable through the runtime-owned
        // observation queue: the Agent Loop emits exactly one terminal
        // event, then the settlement handoff restores the conversation.
        let terminal = await_observation(
            fixture
                .pending
                .as_ref()
                .expect("the bridged fixture carries its queue"),
            |observation| {
                matches!(
                    observation,
                    ConversationObservation::Event { event, .. } if is_terminal_event(event)
                )
            },
        )
        .await;
        assert_eq!(
            count_terminal(&terminal),
            1,
            "exactly one terminal settlement event"
        );

        // The canonical conversation committed the inbound message and the
        // assistant reply: the same real AgentExecution path an interactive
        // runtime uses.
        let ledger = await_settled_ledger(&fixture.runtime).await;
        let roles: Vec<&str> = ledger
            .iter()
            .map(|message| match message {
                MessageBlock::User(_) => "user",
                MessageBlock::Assistant(_) => "assistant",
                MessageBlock::Tool(_) => "tool",
                MessageBlock::System(_) => "system",
            })
            .collect();
        assert_eq!(
            roles,
            vec!["user", "user", "assistant"],
            "inbound + admitted Agent Status + assistant reply"
        );
        // The real provider path observed the inbound message.
        let requests = fixture.model.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].messages.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.id == admission.message_id)
        }));
        // The attempt was admitted exactly once and the runtime is idle
        // again.
        assert!(!fixture.runtime.has_current_attempt());
        assert_eq!(fixture.runtime.request_history().snapshots().len(), 1);
    }

    /// The headless real tool cycle (Issue #61): a conversation runtime
    /// with **zero** observation bridge and zero Runtime Client
    /// attachments executes a real tool turn end to end:
    ///
    /// ```text
    /// inbound -> AgentExecution -> provider ToolCall
    ///   -> real ConversationToolRuntime execution
    ///   -> canonical ToolResult commit
    ///   -> same attempt's next model step (observing the ToolResult)
    ///   -> final assistant result -> terminal settlement
    ///   -> ConversationRuntime regains the authoritative ConversationState
    /// ```
    ///
    /// The production `AgentExecution` / Context Assembly / `ToolRuntime` /
    /// `CapabilityCoordinator` path is used unchanged; the test observes
    /// only the runtime-owned observation stream and authoritative state.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn headless_tool_cycle_with_zero_attachments() {
        use crate::message::types::{ContentBlockIndex, InboundKind};
        use crate::model::event::ModelEvent;
        use crate::model::finish::ModelFinishReason;
        use crate::runtime::identity::{ToolCallId, ToolId};
        use crate::scripted_suites::support::fake::{FakeTool, success_result};
        use crate::tools::executor::ToolRegistry;
        use crate::tools::types::{
            ToolDefinition, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
        };

        // The real tool: registered in the base registry the capability
        // coordinator composes over, executed by the production
        // ConversationToolRuntime path.
        let definition = ToolDefinition {
            id: ToolId::new("tool-echo"),
            name: "echo".to_owned(),
            description: "echoes the call arguments".to_owned(),
            input_schema: serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: crate::tools::types::ToolConcurrencyPolicy::default(),
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        };
        let tool = FakeTool::new(definition.clone(), success_result("echoed"));
        let calls = tool.calls();
        let mut registry = ToolRegistry::new();
        tool.register(&mut registry);

        let dir = tempfile::tempdir().expect("temp dir");
        let call_id = "call-echo";
        let scripts = vec![
            // Turn 1: one real tool call.
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::ToolCallStarted {
                    block_index: ContentBlockIndex::new(0),
                    call: crate::tools::types::ToolCallStart {
                        id: ToolCallId::new(call_id),
                        tool_id: definition.id.clone(),
                        name: definition.name.clone(),
                    },
                }),
                FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                    block_index: ContentBlockIndex::new(0),
                    call_id: ToolCallId::new(call_id),
                    arguments_delta: "{\"text\":\"hi\"}".to_owned(),
                }),
                FakeStep::Emit(ModelEvent::ToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call: crate::tools::types::ToolCall {
                        id: ToolCallId::new(call_id),
                        tool_id: definition.id.clone(),
                        name: definition.name.clone(),
                        arguments: serde_json::json!({"text": "hi"}),
                    },
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::ToolCalls,
                    usage: None,
                }),
            ],
            // Turn 2: the final assistant answer.
            vec![
                FakeStep::Emit(ModelEvent::Started),
                FakeStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "tool worked".to_owned(),
                }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ],
        ];
        // Zero observation bridge, zero client: the runtime runs the exact
        // production path with no consumer attached.
        let (runtime, model) = headless_runtime(&dir, scripts, Some(registry), None).await;
        runtime.activate();

        let admission = runtime
            .submit_inbound(text_content("use echo"))
            .expect("accepted");

        // Deterministic settlement handoff: the settlement signal fires
        // once when the authoritative ConversationState returns to the
        // runtime.
        runtime.settlement_signal().notified().await;
        let ledger = runtime.coordinator_ledger().expect("settled");

        // The real tool executed exactly once under the canonical ToolCall
        // identity.
        let invocations = calls.borrow().clone();
        assert_eq!(invocations.len(), 1, "exactly one real tool execution");
        assert_eq!(invocations[0].tool_name, "echo");
        assert_eq!(invocations[0].arguments, serde_json::json!({"text": "hi"}));

        // Structural ordering: inbound user + Agent Status + the Assistant
        // ToolCall message + the canonical ToolResult + the final
        // assistant reply.
        let roles: Vec<&str> = ledger
            .iter()
            .map(|message| match message {
                MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary => {
                    "summary"
                }
                MessageBlock::User(_) => "user",
                MessageBlock::Assistant(_) => "assistant",
                MessageBlock::Tool(_) => "tool",
                MessageBlock::System(_) => "system",
            })
            .collect();
        assert_eq!(
            roles,
            vec!["user", "user", "assistant", "tool", "assistant"],
            "inbound + status + tool call + canonical tool result + final reply"
        );
        // The canonical ToolResult commits under the real call identity.
        let tool_message = ledger
            .iter()
            .find_map(|message| match message {
                MessageBlock::Tool(tool) => Some(tool),
                _ => None,
            })
            .expect("a ToolMessage was committed");
        assert_eq!(tool_message.tool_call_id.as_str(), call_id);
        assert_eq!(tool_message.tool_id.as_str(), "tool-echo");

        // Two real primary provider requests; the second observes the
        // canonical ToolResult.
        let requests = model.requests();
        assert_eq!(requests.len(), 2, "two primary provider requests");
        assert!(requests[0].messages.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.id == admission.message_id)
        }));
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| matches!(message, MessageBlock::Tool(tool) if tool.tool_call_id.as_str() == call_id)),
            "the second provider request observes the canonical ToolResult"
        );

        // Terminal settlement happened exactly once and the runtime owns
        // the authoritative state again, with the request facts retained.
        assert!(!runtime.has_current_attempt());
        let history = runtime.request_history();
        assert_eq!(history.snapshots().len(), 2, "both requests retained");
        // The coordinator owns the one authoritative ConversationState
        // again: the runtime keeps no second derived read model to reset.
        assert!(runtime.coordinator_ledger().is_some());
    }

    /// The idle async wakeup: an idle conversation runtime with a purely
    /// asynchronous inbound enqueue admits exactly one attempt without any
    /// client command.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn idle_async_inbound_wakes_the_runtime_without_a_client_request() {
        let fixture = headless_fixture().await;

        // A purely async producer: a Runtime-source message enqueued
        // directly into the authoritative mailbox. No client exists at all.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(runtime_inbound_message("conv-headless-async-1"))
            .expect("enqueue");

        // No client command: the runtime wake gate admits exactly one
        // attempt.
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(
            fixture.model.requests().len(),
            1,
            "exactly one attempt was admitted"
        );
        let ledger = fixture.runtime.coordinator_ledger().expect("settled");
        assert!(
            ledger.iter().any(|message| matches!(
                message,
                MessageBlock::User(user) if user.id.as_str() == "conv-headless-async-1"
            )),
            "the asynchronous inbound was consumed exactly once"
        );
    }

    /// The async-wake vs client-submit race: while an admission is gated,
    /// an async enqueue and a client submit both land in the mailbox; the
    /// single admission drains one finite batch in mailbox order and
    /// admits exactly one attempt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn async_wake_vs_client_submit_admits_exactly_one_attempt() {
        let gate = Arc::new(super::Gate::default());
        let fixture = headless_fixture_with(Some(CoordinatorProbe {
            admission_gate: Some(gate.clone()),
            settlement_gate: None,
        }))
        .await;
        gate.arm();

        // The async producer enqueues first; the wake gate starts an
        // admission that parks before the coordinator lock.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(runtime_inbound_message("conv-headless-async-1"))
            .expect("async enqueue");
        gate.wait_entered();

        // The client submit races the gated admission: it can still publish
        // because the admission parks before the coordinator lock.
        fixture
            .runtime
            .submit_inbound(text_content("client"))
            .expect("client submit accepted");

        // Release: one admission drains both messages in mailbox order.
        gate.release();
        let observations = await_observation(
            fixture
                .pending
                .as_ref()
                .expect("the bridged fixture carries its queue"),
            |observation| {
                matches!(
                    observation,
                    ConversationObservation::Event { event, .. } if is_terminal_event(event)
                )
            },
        )
        .await;

        assert_eq!(
            fixture.model.requests().len(),
            1,
            "at most one active attempt"
        );
        let ledger = await_settled_ledger(&fixture.runtime).await;
        let inbound: Vec<&str> = ledger
            .iter()
            .filter_map(|message| match message {
                MessageBlock::User(user) => Some(user.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(inbound[0], "conv-headless-async-1");
        assert!(inbound.contains(&"conv-headless-inbound-1"));
        // Exactly one finite drain was observed, carrying both messages in
        // mailbox order.
        let drained: Vec<_> = observations
            .iter()
            .filter_map(|observation| match observation {
                ConversationObservation::InboundDrained(batch) => Some(batch),
                _ => None,
            })
            .collect();
        assert_eq!(drained.len(), 1, "one finite inbound batch");
        assert_eq!(drained[0].items().len(), 2, "both messages in one batch");
    }

    /// Waits until an observation satisfying the predicate reaches the
    /// queue; returns every observation drained up to and including it.
    async fn await_observation(
        pending: &PendingObservations,
        mut predicate: impl FnMut(&ConversationObservation) -> bool,
    ) -> Vec<ConversationObservation> {
        let mut folded = Vec::new();
        loop {
            for observation in pending.drain() {
                let matched = predicate(&observation);
                folded.push(observation);
                if matched {
                    return folded;
                }
            }
            pending.wait().await;
        }
    }

    /// Waits for the settlement handoff (the runtime-owned settlement
    /// signal) and returns the authoritative ledger.
    async fn await_settled_ledger(runtime: &ConversationRuntime) -> Vec<MessageBlock> {
        runtime.settlement_signal().notified().await;
        runtime
            .coordinator_ledger()
            .expect("the settlement handoff restored the conversation state")
    }

    /// Whether an internal runtime event is one of the terminal settlement
    /// events of an attempt.
    fn is_terminal_event(event: &crate::events::types::RuntimeEvent) -> bool {
        matches!(
            event,
            crate::events::types::RuntimeEvent::AttemptCompleted { .. }
                | crate::events::types::RuntimeEvent::AttemptCancelled { .. }
                | crate::events::types::RuntimeEvent::AttemptTimedOut { .. }
                | crate::events::types::RuntimeEvent::AttemptLimitExceeded { .. }
                | crate::events::types::RuntimeEvent::AttemptFailed { .. }
        )
    }

    fn count_terminal(observations: &[ConversationObservation]) -> usize {
        observations
            .iter()
            .filter(|observation| {
                matches!(
                    observation,
                    ConversationObservation::Event { event, .. } if is_terminal_event(event)
                )
            })
            .count()
    }

    /// A runtime whose capability coordinator and tool runtime disagree on
    /// ownership is rejected at construction.
    #[test]
    fn construction_validates_ownership() {
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv-a");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            conversation_id.clone(),
            &workspace,
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");
        let other = ConversationId::new("conv-b");
        let other_workspace = dir.path().join("workspace-other");
        std::fs::create_dir_all(&other_workspace).expect("other workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            other.clone(),
            &other_workspace,
            dir.path().join("artifacts-other"),
        )
        .expect("other tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: other_runtime.conversation_id().clone(),
                workspace: other_runtime.workspace().clone(),
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: other_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let error = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
            timezone: None,
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_composer: AgentStatusComposer::default(),
            },
            tool_runtime,
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
        })
        .expect_err("mismatched ownership is rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::OwnershipMismatch { .. }
        ));
    }

    /// Construction outside a Tokio execution runtime fails with the typed
    /// activation error instead of silently creating a coordinator whose
    /// admission worker may never exist.
    #[test]
    fn construction_outside_execution_runtime_fails_typed() {
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "this test runs on a plain thread"
        );
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv-no-tokio");
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
                base_tool_registry: Arc::new(crate::tools::executor::ToolRegistry::new()),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let error = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(Arc::new(FakeModel::new(Vec::new()))),
            timezone: None,
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_composer: AgentStatusComposer::default(),
            },
            tool_runtime,
            capability: coordinator,
            clock: None,
            initial_messages: Vec::new(),
        })
        .expect_err("construction outside Tokio is rejected");
        assert!(matches!(
            error,
            ConversationRuntimeError::NoExecutionRuntime
        ));
    }

    /// Shutdown gates further inbound admission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_gates_further_admission() {
        let fixture = headless_fixture().await;
        fixture
            .runtime
            .shutdown()
            .expect("accepted after activation");
        assert!(matches!(
            fixture.runtime.submit_inbound(text_content("late")),
            Err(InboundAdmissionError::Shutdown)
        ));
    }

    /// Cancelling an unknown attempt identity fails explicitly and never
    /// cancels a different attempt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_unknown_attempt_fails_typed() {
        let fixture = headless_fixture().await;
        assert_eq!(
            fixture
                .runtime
                .cancel_current_attempt(&crate::runtime::identity::AttemptId::new("ghost")),
            Err(CancelAttemptError::NoCurrentAttempt)
        );
    }
}
