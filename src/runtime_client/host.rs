//! The Runtime Client host: the projection + control + attachment adapter
//! over the conversation runtime coordinator (Issue #61).
//!
//! [`RuntimeClientHost`] is the Runtime Client protocol boundary. It
//! observes and controls the
//! [`ConversationRuntime`](crate::runtime::conversation_runtime::ConversationRuntime)
//! of the same conversation; it does **not** own the conversation runtime:
//!
//! ```text
//! ConversationRuntime semantic facts/observations
//!         |
//!         v
//! RuntimeClientProjection (snapshot / cursor / replay / subscribers)
//!         |
//!         v
//! RuntimeClientHost (attachment / protocol control adapter)
//!         |
//!         v
//! RuntimeClientEndpoint -> transports (stdio / future WS) -> TUI
//! ```
//!
//! The host owns:
//!
//! - one control attachment plus any number of explicitly read-only
//!   observation attachments;
//! - the Runtime Client projection (snapshot read model, cursor allocation,
//!   bounded replay, subscribers) and its linearization boundary;
//! - the root publication-admission check for child interactions, limited to
//!   the synchronized presence of the control attachment;
//! - protocol adaptation: request dispatch, `model_set`/`shutdown`/
//!   `cancel_current_attempt` forwarding, native Session intent forwarding,
//!   and inbound publish forwarding;
//! - transport-independent client subscriptions.
//!
//! The host does **not** own:
//!
//! - canonical conversation state (the coordinator owns `ConversationState`
//!   between attempts);
//! - session model authority (the coordinator freezes attempt snapshots at
//!   admission);
//! - attempt admission (the coordinator is the one admission owner);
//! - mailbox semantic sequencing (the coordinator owns the
//!   mailbox/admission relationship);
//! - `ConversationToolRuntime` / `CapabilityCoordinator` semantic ownership;
//! - child interaction waiters, pending state, audit, cancellation,
//!   settlement, or execution authority;
//! - SessionCatalog/SessionGraph ownership (the optional native Session
//!   control seam forwards to `LocalSessionSupervisor`);
//! - cancellation terminal settlement (`AgentExecution` remains the attempt
//!   execution/terminal authority);
//! - background/subagent lifecycle.
//!
//! # Observation handoff
//!
//! The conversation runtime publishes every semantically meaningful
//! transition as a runtime-owned
//! [`ConversationObservation`](crate::runtime::observation::ConversationObservation)
//! into the runtime-owned observation fan-out, whose primary queue is the
//! [`PendingObservations`](crate::runtime::observation::PendingObservations)
//! queue used by this projection. The runtime installs that fan-out through
//! its bootstrap handshake at host construction (see
//! `ConversationRuntime::install_observation_bridge`). The handshake runs
//! over an inert, not-yet-activated runtime and captures the bootstrap
//! snapshot and every subsystem observation seam at one global cut, so
//! the projection's initial seed and the live observation stream cover
//! the runtime's history with no gap and no duplication — and the seed
//! itself publishes nothing and allocates no cursor. Existing bounded local
//! observers may receive a separate fan-out queue without becoming a second
//! projection or history authority.
//!
//! Every host lock acquisition drains that queue first, so queued
//! observations fold in enqueue order, ahead of whatever the acquiring
//! caller is about to do. The projection fold, cursor allocation, and
//! event publication therefore share the one host synchronization
//! boundary with snapshot reads, subscription polls, and attachment
//! admission, and the snapshot/cursor invariant holds by synchronization:
//!
//! > A snapshot returned at cursor C contains all Runtime Client state
//! > through C, and a subscription after C observes every subsequently
//! > published event or fails explicitly with `resync_required`.
//!
//! # The lock-order graph
//!
//! ```text
//!   ClientState ─────────────► PendingObservations (fan-out leaf)
//!       ▲
//!       │  (never; see below)
//!   coordinator ─────────────► observation fan-out ─► PendingObservations
//!   mailbox / background / capability ─► observation fan-out
//! ```
//!
//! No authoritative subsystem ever acquires `ClientState`. The mailbox, the
//! background registry, the capability coordinator, and the agent attempt
//! task all fire their observers while their own boundary is held, so
//! [`ClientObserver`] is never used: the conversation runtime's own
//! observers (see `crate::runtime::conversation_runtime::RuntimeObserver`)
//! append to the runtime-owned fan-out instead.
//! There is therefore no `subsystem -> ClientState` edge to pair with any
//! `ClientState -> mailbox` call on the host surface, and subscriber
//! notification can never block authoritative runtime state.
//!
//! # Lifetime
//!
//! The host is a **non-owning observer** of the conversation runtime in
//! every direction: the host holds an `Arc<ConversationRuntime>` (control +
//! seed reads), while the runtime holds only a `Weak` reference through its
//! installed observation seams. Releasing the last host handle closes the
//! primary observation queue (the projection worker's terminal condition);
//! releasing the last runtime handle closes the queue and the admission
//! wake gate. A detached or absent Runtime Client never stops the
//! conversation: admission, execution, settlement, and canonical state all
//! belong to the coordinator and run identically with zero attachments.
//!
//! # Detach is not cancellation
//!
//! Detaching an attachment changes only attachment state. It never
//! cancels, settles, or mutates semantic runtime work: the current
//! attempt, conversation-owned background executions, mailbox contents,
//! canonical conversation state, and capability state are untouched.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg_attr(not(test), allow(unused_imports))]
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use super::projection::{RuntimeClientProjection, SubscriberPoll, background_view, subagent_view};
use super::snapshot::{
    RuntimeClientTranscriptCursor, RuntimeClientTranscriptPage, transcript_page_view,
};
use super::types::{
    AttachmentId, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor, RuntimeClientError,
    RuntimeClientProtocolEvent, RuntimeClientResult, RuntimeClientSessionRequest,
};
use crate::durable::{
    ConversationStore, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT, TRANSCRIPT_PAGE_LIMIT_MAX,
};
use crate::model::catalog::{ModelCapabilities, ModelCatalogView, ModelRef};
use crate::model::invocation::ModelInvocationView;
use crate::model::session::{
    SessionModelConfig, SessionModelView, SummaryModelPolicy, SummaryModelView,
};
use crate::model::types::ModelProtocol;
use crate::model::{ModelRequest, RequestIdentity};
use crate::runtime::conversation_runtime::{
    CancelAttemptError, ConversationRuntime, InboundAdmissionError, ManualCompactionError,
    ModelUpdateError, RuntimeBootstrapError, RuntimeResourceReloadError,
};
use crate::runtime::identity::{ConversationId, ToolExecutionId};
use crate::runtime::interaction::{InteractionRef, InteractionResponse, RoutedInteractionError};
use crate::runtime::observation::PendingObservations;
use crate::runtime::request_history::{RequestHistory, RequestHistoryError};
use crate::runtime::subagent::InteractionPublicationAuthority;

/// The one Runtime Client host construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostConstructionError {
    /// The conversation runtime identity is already bound to a Runtime
    /// Client host.
    ///
    /// The Runtime Client protocol binds one runtime identity to at most one
    /// [`RuntimeClientHost`] for that identity's lifetime, so cloning a
    /// runtime never yields a second bindable identity and dropping the
    /// bound host never makes it bindable again. Reconnect replaces the
    /// attachment, not the host.
    RuntimeClientAlreadyBound {
        /// The conversation whose runtime identity is already bound.
        conversation_id: ConversationId,
    },
    /// An observation bridge is already installed over the conversation
    /// runtime (a previous headless observation consumer), so the host
    /// cannot establish its own projection handshake.
    ///
    /// Unreachable through the production composition path (the binding
    /// claim gates it); reported typed so a failed construction releases
    /// the binding claim instead of leaving a claimed-but-broken runtime.
    ObservationBridgeAlreadyInstalled {
        /// The conversation whose runtime already has a bridge.
        conversation_id: ConversationId,
    },
    /// The conversation runtime was already activated.
    ///
    /// Binding a Runtime Client host is a **pre-activation** composition
    /// decision (Issue #61). A host binds while the runtime is inert, so
    /// its initial snapshot is the runtime's real state at the activation
    /// cut; there is no supported hot installation of a first host over a
    /// runtime that has already begun semantic execution.
    RuntimeAlreadyActivated {
        /// The conversation whose runtime is already activated.
        conversation_id: ConversationId,
    },
    /// The native durable authority could not provide a coherent bootstrap
    /// snapshot for the client projection.
    Durable(String),
}

impl core::fmt::Display for HostConstructionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RuntimeClientAlreadyBound { conversation_id } => write!(
                f,
                "the runtime identity of conversation {conversation_id} is already bound to a Runtime Client host"
            ),
            Self::ObservationBridgeAlreadyInstalled { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} already has an observation bridge installed"
            ),
            Self::RuntimeAlreadyActivated { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} is already activated; a Runtime Client host binds before activation"
            ),
            Self::Durable(message) => write!(f, "durable conversation bootstrap failed: {message}"),
        }
    }
}

impl std::error::Error for HostConstructionError {}

/// The typed native Session control seam installed by the local product
/// composition. Runtime Client owns only protocol adaptation; the
/// implementation remains in `LocalSessionSupervisor`.
pub type SessionControlFuture =
    Pin<Box<dyn Future<Output = Result<RuntimeClientResult, RuntimeClientError>> + Send>>;

pub trait RuntimeClientSessionControl: Send + Sync {
    /// Handles one native Session intent. The returned future owns any
    /// quiescence await; no host lock is held across it.
    fn handle(&self, request: RuntimeClientSessionRequest) -> SessionControlFuture;

    /// Persists a live model candidate for the active local Session before
    /// the `ConversationRuntime` replaces its authoritative configuration.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error when the product metadata cannot
    /// be durably updated.
    fn persist_model(&self, config: SessionModelConfig) -> Result<(), RuntimeClientError>;

    /// Fences conversation/runtime operations after the native Session owner
    /// has reached its terminal replacement-required state. Read-only native
    /// Session metadata requests are handled through `handle` and may remain
    /// available; ordinary runtime operations must not use the stale runtime.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::SessionRestartRequired`] when this
    /// attachment can no longer use its conversation runtime.
    fn ensure_live(&self) -> Result<(), RuntimeClientError> {
        Ok(())
    }
}

/// The host-owned attachment state.
pub(crate) struct AttachmentState {
    /// The attachment identity.
    attachment_id: AttachmentId,
    /// The registered subscriber of the attachment, when it subscribed.
    subscriber_id: Option<u64>,
}

/// The one synchronized host state (the projection linearization owner).
pub(crate) struct ClientState {
    /// The Runtime Client projection: snapshot read model, cursor,
    /// bounded replay, subscribers.
    projection: RuntimeClientProjection,
    /// The one control-capable Runtime Client attachment. Read-only
    /// inspection attachments are kept separately so observation subscribers
    /// do not compete with execution/control ownership.
    control_attachment: Option<AttachmentState>,
    /// Read-only Runtime Client inspection attachments. They share the one
    /// projection and replay ring, but never acquire control authority.
    read_only_attachments: BTreeMap<AttachmentId, AttachmentState>,
    /// The next attachment identity sequence.
    next_attachment_seq: u64,
}

impl ClientState {
    /// Applies every queued pending observation in queue order.
    fn apply_pending(&mut self, pending: &PendingObservations) {
        for observation in pending.drain() {
            self.projection.apply(observation);
        }
    }
}

#[cfg(test)]
impl ClientInner {
    /// Test-only: folds exactly one queued observation and returns the
    /// snapshot as it stands at that cut, or `None` when nothing is queued.
    ///
    /// The ordinary [`Self::lock_state`] path drains the whole queue before
    /// anything reads the projection, which is what makes a request path
    /// safe — and what makes it useless for proving that no *intermediate*
    /// state is observable. Stepping one observation at a time reproduces
    /// exactly what the projection worker can expose to a subscriber it
    /// wakes between two enqueues.
    pub(crate) fn fold_one_observation(
        &self,
    ) -> Option<(super::snapshot::RuntimeClientSnapshot, RuntimeClientCursor)> {
        let observation = self.pending.pop_one()?;
        let mut guard = self
            .state
            .lock()
            .expect("runtime client host lock poisoned");
        guard.projection.apply(observation);
        Some(guard.projection.snapshot().expect("projection is live"))
    }

    /// Test-only: how many observations are waiting to be folded.
    pub(crate) fn queued_observations(&self) -> usize {
        self.pending.queued()
    }

    /// Test-only: parks the projection worker so the test owns the fold
    /// schedule.
    ///
    /// The worker is spawned when the host is constructed and is woken by
    /// every enqueue, so without this a test that wants to look at an
    /// intermediate cut is racing it: the queue can be drained before the
    /// test reads it. Parking is ordered against every concurrent drain, so
    /// after this returns only [`Self::fold_one_observation`] advances the
    /// projection.
    pub(crate) fn park_projection_worker(&self) {
        self.pending.park();
    }
}

/// The shared Runtime Client host state.
pub(crate) struct ClientInner {
    conversation_id: ConversationId,
    agent_id: crate::runtime::identity::AgentId,
    /// The live conversation runtime this host observes and controls. A
    /// durable inspection host deliberately has no runtime owner here.
    runtime: Option<ConversationRuntime>,
    /// The one durable authority used for transcript/bootstrap reads by both
    /// live and read-only hosts.
    store: Arc<dyn ConversationStore>,
    /// Whether this host is a read-only attachment to durable conversation
    /// state rather than a control adapter over a live runtime.
    read_only: bool,
    /// The bounded replay setting used when a durable inspection projection is
    /// rebuilt from authoritative state.
    replay_limit: usize,
    /// Optional native product Session owner. Low-level conversation hosts
    /// intentionally leave this absent; the local product installs exactly
    /// one supervisor here.
    session_control: Option<Arc<dyn RuntimeClientSessionControl>>,
    /// The one projection synchronization boundary.
    state: Mutex<ClientState>,
    /// The observation queue shared with the conversation runtime (the
    /// projection sink installed at construction).
    pending: Arc<PendingObservations>,
    /// Whether the projection worker task was spawned.
    worker_started: AtomicBool,
}

/// Root-side publication authority installed into the parent subagent
/// registry. It is a weak adapter over the Runtime Client host's attachment
/// lock, not an interaction owner.
struct RootInteractionPublicationAuthority {
    inner: Weak<ClientInner>,
}

impl InteractionPublicationAuthority for RootInteractionPublicationAuthority {
    fn admit(&self, _interaction: &InteractionRef) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.admits_interaction_publication())
    }
}

/// Releasing the last host handle closes the shared observation queue,
/// which is the projection worker's terminal condition.
impl Drop for ClientInner {
    fn drop(&mut self) {
        self.pending.close();
    }
}

impl ClientInner {
    /// Acquires the one projection synchronization boundary, applying queued
    /// pending observations first so every state read observes every queued
    /// fact.
    pub(crate) fn lock_state(&self) -> MutexGuard<'_, ClientState> {
        let mut guard = self
            .state
            .lock()
            .expect("runtime client host lock poisoned");
        guard.apply_pending(&self.pending);
        guard
    }

    /// The root publication-admission frontier. The same host mutex that
    /// removes/installs the control attachment on detach/attach is used for
    /// this check, so an admission and a detach have exactly one ordering:
    /// whichever operation acquires this boundary first wins.
    pub(crate) fn admits_interaction_publication(&self) -> bool {
        let state = self.lock_state();
        state.control_attachment.is_some()
    }

    /// Refreshes the bounded transcript bootstrap page from the durable
    /// authority. The projection retains only this read result; it never
    /// owns transcript bodies or an unbounded historical collection.
    fn refresh_transcript_page(&self, state: &mut ClientState) -> Result<(), RuntimeClientError> {
        let page = self
            .store
            .load_transcript_page(None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)
            .map_err(|error| RuntimeClientError::RuntimeFailure {
                message: format!("durable transcript bootstrap failed: {error}"),
            })?;
        let page =
            transcript_page_view(page).map_err(|message| RuntimeClientError::RuntimeFailure {
                message: format!("durable transcript bootstrap is invalid: {message}"),
            })?;
        state.projection.set_transcript_page(page);
        Ok(())
    }

    /// Rebuilds a read-only projection from the durable authorities. This is
    /// the resync boundary for an inspection attachment: no event is replayed
    /// into the live cursor ring, and no previously materialized presentation
    /// value is treated as recovery input.
    fn refresh_durable_projection(
        &self,
        state: &mut ClientState,
    ) -> Result<(), RuntimeClientError> {
        debug_assert!(self.read_only);
        state.projection =
            durable_projection(self.store.as_ref(), self.replay_limit).map_err(|error| {
                RuntimeClientError::RuntimeFailure {
                    message: error.to_string(),
                }
            })?;
        // Replacing the projection invalidates the old subscriber registration
        // just as a fresh attachment would. The caller's next
        // `subscribe_events` request installs a cursor against this rebuilt
        // projection.
        if let Some(attachment) = state.control_attachment.as_mut() {
            attachment.subscriber_id = None;
        }
        for attachment in state.read_only_attachments.values_mut() {
            attachment.subscriber_id = None;
        }
        Ok(())
    }

    /// Spawns the projection worker: folds queued runtime observations
    /// promptly so subscribed clients observe mailbox, background, and
    /// capability facts without sending requests.
    ///
    /// The worker exists because authoritative runtime owners only *enqueue*
    /// (see the lock-order graph in the module documentation): something
    /// must take the host lock to fold what they enqueued. Correctness never
    /// depends on the worker — every host lock acquisition drains the queue
    /// first, so a request path always observes queued facts — only
    /// promptness for an idle subscriber does.
    ///
    /// # Lifetime
    ///
    /// The worker never owns the host. It captures `Weak<ClientInner>` plus
    /// an `Arc<PendingObservations>` — the minimal wait state — and it
    /// upgrades the weak handle only inside a folding step, never across
    /// an await. A parked worker therefore holds no strong reference, so it
    /// cannot keep a host alive that has no semantic owner left.
    ///
    /// Termination is deterministic, not timed: dropping the last
    /// `Arc<ClientInner>` runs `ClientInner::drop`, which closes the pending
    /// queue and wakes the worker; the worker observes the closed queue and
    /// exits. The upgrade check is a second, independent exit path.
    pub(crate) fn ensure_worker(self: &Arc<Self>) {
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

    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
    ///
    /// The Runtime Client host allows one control attachment. A second
    /// simultaneous control attach fails deterministically and never evicts
    /// the first; explicitly read-only inspection attachments use a separate
    /// subscriber set and may coexist with it. The returned snapshot and
    /// cursor are linearized with admission under the one projection
    /// synchronization boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when the
    /// control attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub(crate) fn attach(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.attach_with_mode(protocol_version, false)
    }

    /// Admits one explicitly read-only attachment. Read-only attachments may
    /// coexist with the one control attachment and with one another; they
    /// consume only projection subscription state and never change provider
    /// admission or runtime lifecycle state.
    pub(crate) fn attach_read_only(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.attach_with_mode(protocol_version, true)
    }

    fn attach_with_mode(
        self: &Arc<Self>,
        protocol_version: u16,
        read_only_attachment: bool,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        let read_only_attachment = self.read_only || read_only_attachment;
        self.ensure_session_runtime_live()?;
        if protocol_version != RUNTIME_CLIENT_PROTOCOL_VERSION {
            return Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: RUNTIME_CLIENT_PROTOCOL_VERSION,
                requested: protocol_version,
            });
        }
        self.ensure_worker();
        let mut state = self.lock_state();
        if !read_only_attachment && let Some(existing) = &state.control_attachment {
            return Err(RuntimeClientError::AttachmentInUse {
                existing_attachment_id: existing.attachment_id.clone(),
            });
        }
        if self.read_only {
            self.refresh_durable_projection(&mut state)?;
        } else {
            self.refresh_transcript_page(&mut state)?;
        }
        let (snapshot, cursor) = state.projection.snapshot()?;
        state.next_attachment_seq = state.next_attachment_seq.saturating_add(1);
        let attachment_id = AttachmentId::new(format!("attachment-{}", state.next_attachment_seq));
        let attachment_state = AttachmentState {
            attachment_id: attachment_id.clone(),
            subscriber_id: None,
        };
        if read_only_attachment {
            state
                .read_only_attachments
                .insert(attachment_id.clone(), attachment_state);
        } else {
            state.control_attachment = Some(attachment_state);
        }
        // Keep provider admission under the same host-state lock as the
        // attachment identity. A concurrent detach therefore cannot remove
        // the attachment and then be overwritten by this attach's provider
        // update after the lock is released.
        if !read_only_attachment && let Some(runtime) = self.runtime.as_ref() {
            runtime.set_interaction_provider_available(true);
        }
        drop(state);
        let attachment = super::attachment::RuntimeAttachment::new(
            attachment_id.clone(),
            self.clone(),
            self.read_only || read_only_attachment,
        );
        Ok((
            attachment,
            RuntimeClientResult::Initialized {
                attachment_id,
                conversation_id: self.conversation_id.clone(),
                agent_id: self.agent_id.clone(),
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
    pub(crate) fn detach(&self, attachment_id: &AttachmentId) {
        let mut state = self.lock_state();
        let control = state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id);
        let attachment = if control {
            state
                .control_attachment
                .take()
                .expect("the control attachment identity was just checked")
        } else {
            let Some(attachment) = state.read_only_attachments.remove(attachment_id) else {
                return;
            };
            attachment
        };
        if let Some(subscriber_id) = attachment.subscriber_id {
            state.projection.remove_subscriber(subscriber_id);
        }
        // Detach is non-semantic: it only closes the provider admission
        // gate for future interactions. It does not settle any request
        // already published by the coordinator. Read-only detaches never
        // touch that gate.
        if control && let Some(runtime) = self.runtime.as_ref() {
            runtime.set_interaction_provider_available(false);
        }
    }

    /// Submits one inbound user message through the conversation runtime's
    /// single publish path.
    ///
    /// The runtime owns authoritative metadata: the message identity, the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all runtime-assigned. Success means accepted/published, never
    /// assistant-finished: the runtime wake gate admits the next attempt
    /// when the runtime is idle, and while an attempt is running the
    /// message waits in the authoritative mailbox for the next safe-boundary
    /// drain. The host never admits an attempt itself.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidRequest`] for empty content,
    /// [`RuntimeClientError::RuntimeShutdown`] after shutdown, and
    /// [`RuntimeClientError::InvalidState`] for a mailbox admission
    /// failure.
    pub(crate) fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime");
        let admission = runtime
            .submit_inbound(content)
            .map_err(|error| match error {
                InboundAdmissionError::Shutdown => RuntimeClientError::RuntimeShutdown,
                InboundAdmissionError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                InboundAdmissionError::EmptyContent => RuntimeClientError::InvalidRequest {
                    message: "inbound content must not be empty".to_owned(),
                },
                InboundAdmissionError::DurabilityFailed { message } => {
                    RuntimeClientError::InvalidState { message }
                }
                InboundAdmissionError::Mailbox(error) => RuntimeClientError::InvalidState {
                    message: error.to_string(),
                },
            })?;
        Ok(RuntimeClientResult::InboundAccepted {
            message_id: admission.message_id,
            inbound_sequence: admission.inbound_sequence,
        })
    }

    /// Requests cancellation of the current attempt.
    ///
    /// The deciding observation (the projection's attempt view, drained
    /// under the host lock) and the coordinator's identity-checked
    /// cancellation share the same attempt naming, so the signal is never
    /// delivered to a different attempt. Acceptance is not terminal
    /// settlement: actual settlement remains owned by the Agent Loop and is
    /// observed asynchronously.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NoCurrentAttempt`] when no attempt
    /// is currently cancellable.
    pub(crate) fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let attempt_id = {
            let state = self.lock_state();
            let Some(attempt) = state.projection.snapshot_ref().attempt.as_ref() else {
                return Err(RuntimeClientError::NoCurrentAttempt);
            };
            if matches!(
                attempt.phase,
                super::snapshot::RuntimeClientAttemptPhase::Settled { .. }
            ) {
                return Err(RuntimeClientError::NoCurrentAttempt);
            }
            attempt.attempt_id.clone()
        };
        // The coordinator verifies under its own lock that the named attempt
        // is still the current one, so a settlement/admission race can
        // never cancel a newer attempt.
        match self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .cancel_current_attempt(&attempt_id)
        {
            Ok(attempt_id) => Ok(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }),
            Err(CancelAttemptError::NoCurrentAttempt) => Err(RuntimeClientError::NoCurrentAttempt),
        }
    }

    /// Runs one manual idle context compaction to its terminal result and
    /// returns the authoritative context projection after success.
    pub(crate) async fn compact_context(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .compact_context()
            .await
            .map_err(|error| match error {
                ManualCompactionError::Inactive | ManualCompactionError::Busy => {
                    RuntimeClientError::InvalidState {
                        message: error.to_string(),
                    }
                }
                ManualCompactionError::Shutdown => RuntimeClientError::RuntimeShutdown,
                ManualCompactionError::DurabilityFailed { message }
                | ManualCompactionError::Durable { message } => {
                    RuntimeClientError::InvalidState { message }
                }
                ManualCompactionError::Context(context)
                    if matches!(
                        context.kind,
                        crate::context::ContextErrorKind::NoProgress
                            | crate::context::ContextErrorKind::CannotFit
                    ) =>
                {
                    RuntimeClientError::InvalidState {
                        message: context.message,
                    }
                }
                ManualCompactionError::Context(context) => RuntimeClientError::RuntimeFailure {
                    message: context.message,
                },
            })?;
        let context = self.lock_state().projection.snapshot_ref().context.clone();
        Ok(RuntimeClientResult::ContextCompacted { context })
    }

    /// Atomically reloads one complete resource/capability generation.
    pub(crate) async fn reload_resources(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let reloaded = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .reload_resources()
            .await
            .map_err(|error| match error {
                RuntimeResourceReloadError::Inactive => RuntimeClientError::InvalidState {
                    message: error.to_string(),
                },
                RuntimeResourceReloadError::Shutdown => RuntimeClientError::RuntimeShutdown,
                RuntimeResourceReloadError::Busy { reason } => {
                    RuntimeClientError::ResourceReloadBusy {
                        reason: reason.as_str().to_owned(),
                    }
                }
                RuntimeResourceReloadError::Failed { message } => {
                    RuntimeClientError::RuntimeFailure {
                        message: format!("runtime resource reload failed: {message}"),
                    }
                }
                RuntimeResourceReloadError::PostPublicationSettlementFailed {
                    published,
                    message,
                } => RuntimeClientError::RuntimeFailure {
                    message: format!(
                        "runtime resource reload published resource revision {:?} and capability revision {:?}, but MCP retirement settlement is unproven: {message}",
                        published.resource_revision, published.capability_revision
                    ),
                },
            })?;
        Ok(RuntimeClientResult::ResourcesReloaded {
            resource_revision: reloaded.resource_revision.get(),
            capability_revision: reloaded.capability_revision,
        })
    }

    /// Accepts one typed native interaction response through the
    /// conversation-owned coordinator.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error when the interaction is stale,
    /// already settled, or the response fails bounded validation.
    pub(crate) async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: InteractionResponse,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .respond_interaction(interaction, response)
            .await
            .map(|()| RuntimeClientResult::InteractionResponseAccepted {
                interaction: interaction.clone(),
            })
            .map_err(|error| match error {
                RoutedInteractionError::NotPending { interaction } => {
                    RuntimeClientError::InteractionNotPending { interaction }
                }
                RoutedInteractionError::InvalidResponse { message } => {
                    RuntimeClientError::InteractionInvalidResponse { message }
                }
                RoutedInteractionError::AuditFailed { interaction } => {
                    RuntimeClientError::InteractionAuditFailed { interaction }
                }
            })
    }

    /// Reads the authoritative snapshot and its cursor, linearized
    /// together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted.
    pub(crate) fn snapshot(
        &self,
    ) -> Result<(super::snapshot::RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError>
    {
        self.ensure_session_runtime_live()?;
        let mut state = self.lock_state();
        if self.read_only {
            self.refresh_durable_projection(&mut state)?;
        } else {
            self.refresh_transcript_page(&mut state)?;
        }
        state.projection.snapshot()
    }

    /// Reads one bounded durable transcript page. The transcript cursor is
    /// intentionally distinct from the Runtime Client observation cursor.
    pub(crate) fn transcript_page(
        &self,
        before: Option<RuntimeClientTranscriptCursor>,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        if limit == 0 || limit > TRANSCRIPT_PAGE_LIMIT_MAX {
            return Err(RuntimeClientError::InvalidRequest {
                message: format!(
                    "transcript page limit must be between 1 and {TRANSCRIPT_PAGE_LIMIT_MAX}"
                ),
            });
        }
        let page = self
            .store
            .load_transcript_page(before.map(Into::into), limit)
            .map_err(|error| RuntimeClientError::RuntimeFailure {
                message: format!("durable transcript page failed: {error}"),
            })?;
        let page =
            transcript_page_view(page).map_err(|message| RuntimeClientError::RuntimeFailure {
                message: format!("durable transcript page is invalid: {message}"),
            })?;
        Ok(RuntimeClientResult::TranscriptPage { page })
    }

    /// Returns a durable request-history read handle owned by the
    /// conversation runtime.
    ///
    /// The durable `ConversationStore` owns these snapshots. The returned
    /// value is a read-only handle; each historical read is resolved through
    /// the runtime authority and does not create another conversation or
    /// transcript authority.
    #[must_use]
    pub(crate) fn request_history(&self) -> RequestHistory {
        RequestHistory::new(self.store.clone())
    }

    /// Reconstructs one retained provider-neutral request from durable facts.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub(crate) fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.request_history().reconstruct(identity)
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
    pub(crate) fn subscribe_events(
        self: &Arc<Self>,
        attachment_id: &AttachmentId,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(EventSubscription, RuntimeClientResult), RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        self.ensure_worker();
        let mut state = self.lock_state();
        let previous_subscriber = if state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id)
        {
            state
                .control_attachment
                .as_ref()
                .expect("the control attachment identity was just checked")
                .subscriber_id
        } else {
            state
                .read_only_attachments
                .get(attachment_id)
                .map_or(Err(RuntimeClientError::NotAttached), |attachment| {
                    Ok(attachment.subscriber_id)
                })?
        };
        if let Some(subscriber_id) = previous_subscriber {
            state.projection.remove_subscriber(subscriber_id);
        }
        let (subscriber_id, notify) = state.projection.subscribe(after_cursor)?;
        if state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id)
        {
            state
                .control_attachment
                .as_mut()
                .expect("the control attachment identity was just checked")
                .subscriber_id = Some(subscriber_id);
        } else {
            state
                .read_only_attachments
                .get_mut(attachment_id)
                .expect("the read-only attachment identity was just checked")
                .subscriber_id = Some(subscriber_id);
        }
        drop(state);
        Ok((
            EventSubscription {
                inner: Arc::new(SubscriptionInner {
                    host: self.clone(),
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
    pub(crate) fn capability(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        let state = self.lock_state();
        let snapshot = state.projection.snapshot_ref_checked()?;
        Ok(RuntimeClientResult::Capability {
            capabilities: snapshot.capabilities.clone(),
        })
    }

    /// Reads the safe public model catalog through the conversation
    /// runtime's authoritative session model state.
    ///
    /// It never carries a credential value, an adapter, or a provider HTTP
    /// client.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_catalog(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        let catalog = self.runtime.as_ref().map_or_else(
            || ModelCatalogView { models: Vec::new() },
            super::super::runtime::conversation_runtime::ConversationRuntime::model_catalog,
        );
        Ok(RuntimeClientResult::ModelCatalog { catalog })
    }

    /// Reads the authoritative session model state through the folded
    /// projection, so the value always agrees with the snapshot read model.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        let state = self.lock_state();
        let snapshot = state.projection.snapshot_ref_checked()?;
        Ok(RuntimeClientResult::Model {
            model: Box::new(snapshot.model.clone()),
        })
    }

    /// Replaces the authoritative session model configuration through the
    /// conversation runtime.
    ///
    /// # Linearization
    ///
    /// The runtime performs resolution, validation, and state replacement
    /// under the one coordinator lock that also owns attempt admission. An
    /// update therefore either linearizes before an admission (and that
    /// attempt observes it) or after it (and only later attempts observe
    /// it). There is no third possibility and no timing assumption.
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
    /// under the session context policy, [`RuntimeClientError::InvalidState`]
    /// while the runtime is not yet activated, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub(crate) fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        let control = self.session_control.as_ref().map(Arc::clone);
        let view = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .model_set_with_persistence(config, |config| {
                if let Some(control) = control {
                    control.persist_model(config).map_err(|error| match error {
                        RuntimeClientError::SessionRestartRequired { message } => {
                            ModelUpdateError::SessionRestartRequired { message }
                        }
                        error => ModelUpdateError::PersistenceFailed {
                            message: format!("cannot persist active Session model: {error:?}"),
                        },
                    })?;
                }
                Ok(())
            })
            .map_err(|error| match error {
                ModelUpdateError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                ModelUpdateError::InvalidConfiguration(message) => {
                    RuntimeClientError::InvalidModelConfiguration { message }
                }
                ModelUpdateError::DurabilityFailed { message } => {
                    RuntimeClientError::InvalidState { message }
                }
                ModelUpdateError::PersistenceFailed { message } => {
                    RuntimeClientError::SessionFailure { message }
                }
                ModelUpdateError::SessionRestartRequired { message } => {
                    RuntimeClientError::SessionRestartRequired { message }
                }
            })?;
        Ok(RuntimeClientResult::ModelSet {
            model: Box::new(view),
        })
    }

    /// Requests the authoritative runtime `ApprovalMode` transition.
    pub(crate) fn approval_mode_set(
        &self,
        mode: crate::runtime::ApprovalMode,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        let state = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .approval_mode_set(mode)
            .map_err(|error| match error {
                crate::runtime::ApprovalModeUpdateError::Inactive => {
                    RuntimeClientError::ApprovalModeInactive
                }
                crate::runtime::ApprovalModeUpdateError::DurabilityFailed { message } => {
                    RuntimeClientError::ApprovalModeDurabilityFailed { message }
                }
            })?;
        Ok(RuntimeClientResult::ApprovalModeSet {
            effective_approval_mode: state.effective,
            pending_approval_mode: (state.effective != state.desired).then_some(state.desired),
            revision: state.revision,
        })
    }

    /// Inspects one background execution through the conversation runtime's
    /// authoritative registry.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownBackgroundExecution`] for an
    /// unknown execution identity.
    pub(crate) fn background_status(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        let Some(snapshot) = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.background_status(execution_id))
        else {
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
    pub(crate) fn background_cancel(
        &self,
        execution_id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let Some(snapshot) = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .background_cancel(execution_id)
        else {
            return Err(RuntimeClientError::UnknownBackgroundExecution {
                execution_id: execution_id.clone(),
            });
        };
        Ok(RuntimeClientResult::BackgroundCancelAccepted {
            execution: background_view(&snapshot),
        })
    }

    /// Inspects one subagent child through the conversation runtime's
    /// authoritative registry (Issue #60).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownSubagent`] for an unknown
    /// subagent identity.
    pub(crate) fn subagent_status(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_session_runtime_live()?;
        let Some(snapshot) = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.subagent_status(subagent_id))
        else {
            return Err(RuntimeClientError::UnknownSubagent {
                subagent_id: subagent_id.clone(),
            });
        };
        Ok(RuntimeClientResult::SubagentStatus {
            subagent: subagent_view(&snapshot),
        })
    }

    /// Requests cancellation of one subagent child through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownSubagent`] for an unknown
    /// subagent identity.
    pub(crate) fn subagent_cancel(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let Some(snapshot) = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .subagent_cancel(subagent_id)
        else {
            return Err(RuntimeClientError::UnknownSubagent {
                subagent_id: subagent_id.clone(),
            });
        };
        Ok(RuntimeClientResult::SubagentCancelAccepted {
            subagent: subagent_view(&snapshot),
        })
    }

    /// Disposes one retained terminal subagent workspace through the
    /// runtime-owned workspace/resource lifecycle. The client supplies only
    /// the subagent identity as the disposal target; existing workspace facts
    /// may still be returned in the authoritative subagent projection, but
    /// are never client-selected deletion inputs.
    pub(crate) async fn subagent_workspace_dispose(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let result = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .subagent_workspace_dispose(subagent_id)
            .await
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentWorkspaceDisposalError::UnknownSubagent {
                    subagent_id,
                } => RuntimeClientError::UnknownSubagent { subagent_id },
                crate::runtime::subagent::SubagentWorkspaceDisposalError::NotTerminal { state } => {
                    RuntimeClientError::InvalidState {
                        message: format!(
                            "retained workspace disposal requires a terminal subagent; current state is {state:?}"
                        ),
                    }
                }
                crate::runtime::subagent::SubagentWorkspaceDisposalError::OwnershipMismatch {
                    detail,
                } => RuntimeClientError::SubagentWorkspaceOwnershipMismatch {
                    subagent_id: subagent_id.clone(),
                    message: detail,
                },
                crate::runtime::subagent::SubagentWorkspaceDisposalError::Backend { detail } => {
                    RuntimeClientError::RuntimeFailure { message: detail }
                }
            })?;
        let (snapshot, outcome) = match result {
            crate::runtime::subagent::SubagentWorkspaceDisposal::Disposed(snapshot) => (
                snapshot,
                super::types::RuntimeClientSubagentWorkspaceDisposalOutcome::Disposed,
            ),
            crate::runtime::subagent::SubagentWorkspaceDisposal::AlreadyDisposed(snapshot) => (
                snapshot,
                super::types::RuntimeClientSubagentWorkspaceDisposalOutcome::AlreadyDisposed,
            ),
            crate::runtime::subagent::SubagentWorkspaceDisposal::DisposalPending(snapshot) => (
                snapshot,
                super::types::RuntimeClientSubagentWorkspaceDisposalOutcome::DisposalPending,
            ),
            crate::runtime::subagent::SubagentWorkspaceDisposal::NoRetainedWorkspace(snapshot) => (
                snapshot,
                super::types::RuntimeClientSubagentWorkspaceDisposalOutcome::NoRetainedWorkspace,
            ),
        };
        Ok(RuntimeClientResult::SubagentWorkspaceDisposed {
            subagent: subagent_view(&snapshot),
            outcome,
        })
    }

    /// Drains the conversation runtime and returns only after it is
    /// quiescent. Runtime Client remains a control adapter: the semantic
    /// lifecycle and all settlement ownership remain in `ConversationRuntime`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidState`] while the runtime is
    /// not yet activated: an inert conversation has no runtime lifecycle
    /// to end, so the request is refused and nothing is published.
    pub(crate) async fn shutdown(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .shutdown()
            .await
            .map_err(|error| match error {
                crate::runtime::conversation_runtime::ShutdownError::Inactive => {
                    RuntimeClientError::InvalidState {
                        message: "the conversation runtime is not activated".to_owned(),
                    }
                }
                crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail,
                } => RuntimeClientError::RuntimeFailure {
                    message: format!("runtime-owned shutdown settlement failed: {detail}"),
                },
            })?;
        Ok(RuntimeClientResult::ShutdownCompleted)
    }

    /// Forwards native Session control to the product owner. This adapter
    /// owns no catalog, graph, or selection state itself.
    pub(crate) async fn session_request(
        &self,
        request: RuntimeClientSessionRequest,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let Some(control) = self.session_control.as_ref() else {
            return Err(RuntimeClientError::InvalidState {
                message: "this Runtime Client host is not attached to a local Session product"
                    .to_owned(),
            });
        };
        control.handle(request).await
    }

    fn ensure_session_runtime_live(&self) -> Result<(), RuntimeClientError> {
        self.session_control
            .as_ref()
            .map_or(Ok(()), |control| control.ensure_live())
    }

    /// Fences every semantic mutation from a durable inspection host. The
    /// read-only host owns no coordinator, mailbox, registry, or lifecycle
    /// handle, so an inspection client cannot accidentally become an
    /// execution owner through a control request.
    fn ensure_writable_runtime(&self) -> Result<(), RuntimeClientError> {
        if self.read_only {
            return Err(RuntimeClientError::InvalidState {
                message: "conversation inspection is read-only".to_owned(),
            });
        }
        self.ensure_session_runtime_live()?;
        if self.runtime.is_none() {
            return Err(RuntimeClientError::InvalidState {
                message: "the conversation runtime is unavailable".to_owned(),
            });
        }
        Ok(())
    }
}

/// Rebuilds the redacted session-model view needed by the ordinary Runtime
/// Client snapshot from the newest durable Request Snapshot. A completed or
/// failed child may no longer have a live model catalog, so the historical
/// request invocation is the only honest model authority available to an
/// inspector. Conversations with no started model request receive a
/// deliberately inert display model.
fn durable_model_view(
    store: &dyn ConversationStore,
) -> Result<SessionModelView, HostConstructionError> {
    const REQUEST_SNAPSHOT_PAGE_LIMIT: usize = 256;

    let mut latest = None;
    let mut after_sequence = None;
    loop {
        let page = store
            .read_request_snapshots(after_sequence, REQUEST_SNAPSHOT_PAGE_LIMIT)
            .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
        let next_sequence = page.next_sequence;
        latest = page.snapshots.into_iter().last().or(latest);
        if next_sequence.is_none() || next_sequence == after_sequence {
            break;
        }
        after_sequence = next_sequence;
    }

    let Some(snapshot) = latest else {
        let model = ModelRef::parse("inspection/durable")
            .expect("the fixed durable inspection model reference is valid");
        let capabilities = ModelCapabilities::text_only(false, false);
        let invocation = ModelInvocationView {
            model: model.clone(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            context_window: 0,
            model_max_output_tokens: 0,
            max_output_tokens: 0,
            reasoning_profile: None,
            reasoning_enabled: false,
            request_params: serde_json::Map::new(),
            capabilities: capabilities.clone(),
            declared_capabilities: capabilities,
        };
        return Ok(SessionModelView {
            configured: SessionModelConfig::of(model),
            effective: invocation,
            summary: SummaryModelView::Session,
        });
    };

    // Request Snapshots intentionally persist the provider-facing model
    // identifier, not the catalog's provider-qualified ModelRef. A child
    // inspector has no live catalog to recover that qualification from, so
    // retain the historical invocation values while using the inert display
    // identity already used for conversations with no request history.
    let model = ModelRef::parse(&snapshot.invocation.model).unwrap_or_else(|_| {
        ModelRef::parse("inspection/durable")
            .expect("the fixed durable inspection model reference is valid")
    });
    let invocation = ModelInvocationView {
        model: model.clone(),
        protocol: snapshot.invocation.protocol,
        context_window: snapshot.context_window_tokens,
        model_max_output_tokens: snapshot.invocation.max_output_tokens,
        max_output_tokens: snapshot.invocation.max_output_tokens,
        reasoning_profile: snapshot.reasoning_profile.clone(),
        reasoning_enabled: snapshot.reasoning_enabled,
        request_params: snapshot.invocation.request_params.clone(),
        capabilities: snapshot.invocation.capabilities.clone(),
        declared_capabilities: snapshot.invocation.capabilities.clone(),
    };
    Ok(SessionModelView {
        configured: SessionModelConfig {
            model,
            reasoning_profile: snapshot.reasoning_profile,
            request_params: snapshot.invocation.request_params,
            max_output_tokens: Some(snapshot.invocation.max_output_tokens),
            summary_model: SummaryModelPolicy::Session,
        },
        effective: invocation,
        summary: SummaryModelView::Session,
    })
}

/// Builds the ordinary Runtime Client projection from one conversation's
/// durable authorities. Durable journal facts update the read model only;
/// they do not become live Runtime Client events or consume the live cursor
/// domain.
fn durable_projection(
    store: &dyn ConversationStore,
    replay_limit: usize,
) -> Result<RuntimeClientProjection, HostConstructionError> {
    const DURABLE_EVENT_PAGE_LIMIT: usize = 256;

    let conversation_id = store.conversation_id().clone();
    let head = store
        .load_head()
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    let messages = store
        .load_surface_snapshot(head.revision)
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    let canonical = store
        .load_canonical()
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    let transcript = store
        .load_transcript_page(None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    let model = durable_model_view(store)?;
    let capabilities = super::snapshot::CapabilityView {
        revision: crate::runtime::identity::CapabilityRevision::new(0),
        tools: Vec::new(),
        available_tools: Vec::new(),
        skills: Vec::new(),
        sources: Vec::new(),
    };
    let mut projection =
        RuntimeClientProjection::new(conversation_id, messages, capabilities, model, replay_limit);
    projection.set_transcript_page(
        transcript_page_view(transcript).map_err(HostConstructionError::Durable)?,
    );

    let mut after_sequence = None;
    loop {
        let page = store
            .read_events(after_sequence, DURABLE_EVENT_PAGE_LIMIT)
            .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
        let next_sequence = page.next_sequence;
        for event in &page.events {
            let committed_message_id = match &event.event {
                crate::events::types::RuntimeEvent::AssistantMessageCommitted { message_id }
                | crate::events::types::RuntimeEvent::ToolMessageCommitted { message_id, .. } => {
                    Some(message_id)
                }
                _ => None,
            };
            if let (Some(attempt_id), Some(message_id)) =
                (event.attempt_id.as_ref(), committed_message_id)
                && let Some(message) = canonical.iter().find(|message| message.id() == message_id)
            {
                projection.bootstrap_durable_message(attempt_id, message);
            }
            projection.bootstrap_durable_event(event);
        }
        if page.events.is_empty() || next_sequence == after_sequence {
            break;
        }
        let Some(next_sequence) = next_sequence else {
            break;
        };
        after_sequence = Some(next_sequence);
    }

    Ok(projection)
}

/// The Runtime Client host of one conversation.
///
/// Construct one host per conversation runtime instance; the host installs
/// the projection sink on the conversation runtime exactly once and claims
/// the one-time Runtime Client binding. The host is cheaply cloneable and
/// all clones share one state.
#[derive(Clone)]
pub struct RuntimeClientHost {
    pub(crate) inner: Arc<ClientInner>,
}

impl core::fmt::Debug for RuntimeClientHost {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RuntimeClientHost")
            .field("conversation_id", &self.inner.conversation_id)
            .finish()
    }
}

impl RuntimeClientHost {
    /// Creates the host over one conversation runtime.
    ///
    /// # One conversation authority
    ///
    /// The conversation identity of the host *is*
    /// [`ConversationRuntime::conversation_id`]. The configuration carries
    /// no conversation id of its own, so the host's identity, the
    /// projection's conversation, and the `initialized` result all name the
    /// conversation of the runtime it observes.
    ///
    /// # One host per runtime identity
    ///
    /// Construction claims the one-time Runtime Client binding of the
    /// conversation tool runtime and of the capability coordinator. A
    /// second construction over the same runtime is rejected with
    /// [`HostConstructionError::RuntimeClientAlreadyBound`] rather than
    /// silently installing a second projection sink.
    ///
    /// The binding lasts for the runtime identity's lifetime and is not
    /// released when the bound host is dropped: reconnect belongs to
    /// attachments (detach, then a fresh
    /// [`RuntimeClientEndpoint`](super::endpoint::RuntimeClientEndpoint)
    /// `initialize`), not to host reconstruction.
    ///
    /// # Lifecycle
    ///
    /// A host binds **before** its conversation runtime is activated. The
    /// composition constructs the runtime, optionally binds this host, and
    /// then calls [`ConversationRuntime::activate`]; binding after
    /// activation is refused with
    /// [`HostConstructionError::RuntimeAlreadyActivated`]. A headless
    /// runtime never constructs a host at all.
    ///
    /// # Bootstrap linearization
    ///
    /// After the binding claim the host performs exactly one fallible
    /// step: the runtime's observation bridge handshake
    /// ([`ConversationRuntime::install_observation_bridge`]), which
    /// installs the observation queue and every subsystem seam and
    /// captures the bootstrap snapshot at one global cut, under the one
    /// coordinator lock and over an inert runtime. The projection then
    /// mirrors that snapshot as pure seed state — publishing nothing and
    /// allocating no cursor — so the initial state plus the live
    /// observation stream is exactly one complete projection, with no lost
    /// transition, no duplicate, and no synthetic event for state that
    /// already existed. If the handshake fails, the binding claim is
    /// released and the failure is reported typed; a failed construction
    /// never leaves a claimed-but-invalid binding.
    ///
    /// # Errors
    ///
    /// Returns [`HostConstructionError::RuntimeClientAlreadyBound`] when
    /// the runtime identity is already bound to a Runtime Client host,
    /// [`HostConstructionError::RuntimeAlreadyActivated`] when the runtime
    /// has already been activated, and
    /// [`HostConstructionError::ObservationBridgeAlreadyInstalled`] when a
    /// headless observation bridge already exists over the runtime, or
    /// [`HostConstructionError::Durable`] when native durable bootstrap fails.
    pub fn new(config: RuntimeClientHostConfig) -> Result<Self, HostConstructionError> {
        Self::construct(config, None)
    }

    /// Creates a read-only Runtime Client host over a known conversation's
    /// durable store.
    ///
    /// This is the generic conversation-attachment primitive used by child
    /// inspection. It does not construct a [`ConversationRuntime`], install
    /// an observation bridge, claim a mailbox, or acquire an execution
    /// lifecycle. The store supplies the current Surface/transcript and the
    /// Event Journal is folded into the same ordinary Runtime Client
    /// projection used by a live host. No child-specific transcript payload
    /// or parent-side history is introduced.
    ///
    /// The attachment linearizes at the durable reads performed here. A
    /// concurrent writer may advance the append-only authorities while those
    /// reads occur; a later `snapshot_get`/fresh attachment repairs from the
    /// store again, and no live event cursor is fabricated for the read-only
    /// view.
    ///
    /// # Errors
    ///
    /// Returns [`HostConstructionError::Durable`] when the durable
    /// conversation cannot provide a coherent current Surface, transcript,
    /// request-model, or Event Journal bootstrap.
    pub fn new_durable(
        store: Arc<dyn ConversationStore>,
        replay_limit: Option<usize>,
    ) -> Result<Self, HostConstructionError> {
        let replay_limit =
            replay_limit.unwrap_or(super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
        let conversation_id = store.conversation_id().clone();
        let projection = durable_projection(store.as_ref(), replay_limit)?;

        let pending = Arc::new(PendingObservations::new());
        let inner = Arc::new(ClientInner {
            conversation_id,
            agent_id: crate::runtime::identity::AgentId::new(format!(
                "inspection:{}",
                store.conversation_id()
            )),
            runtime: None,
            store,
            read_only: true,
            replay_limit,
            session_control: None,
            state: Mutex::new(ClientState {
                projection,
                control_attachment: None,
                read_only_attachments: BTreeMap::new(),
                next_attachment_seq: 0,
            }),
            pending,
            worker_started: AtomicBool::new(false),
        });
        // No authoritative runtime can enqueue observations into this host,
        // but using the normal worker setup keeps attachment/subscription
        // lifetime semantics identical to a live host.
        inner.ensure_worker();
        Ok(Self { inner })
    }

    /// Creates a host over one runtime and installs the native Session
    /// control seam used by the local product composition.
    ///
    /// # Errors
    ///
    /// Returns [`HostConstructionError`] when the runtime is already bound,
    /// activated, or cannot provide a coherent observation bootstrap.
    pub fn new_with_session_control(
        config: RuntimeClientHostConfig,
        session_control: Arc<dyn RuntimeClientSessionControl>,
    ) -> Result<Self, HostConstructionError> {
        Self::construct(config, Some(session_control))
    }

    fn construct(
        config: RuntimeClientHostConfig,
        session_control: Option<Arc<dyn RuntimeClientSessionControl>>,
    ) -> Result<Self, HostConstructionError> {
        // ---- Ownership commit: the one-time binding claim. ----
        //
        // The claim is the linearization point that gates every later
        // step: a second construction fails here before touching the
        // runtime, and this construction is the only one that can proceed.
        if !config.runtime.claim_client_binding() {
            return Err(HostConstructionError::RuntimeClientAlreadyBound {
                conversation_id: config.runtime.conversation_id().clone(),
            });
        }

        // ---- The one fallible step after the claim: the bridge handshake.
        //
        // The runtime installs the observation queue and every subsystem
        // observation seam and captures the bootstrap snapshot at one
        // global cut. On failure the claim is released: a rejected
        // construction must leave no trace.
        let replay_limit = config
            .replay_limit
            .unwrap_or(super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
        let store = config.runtime.tool_runtime().durable_store();
        let pending = Arc::new(PendingObservations::new());
        let seed = match config
            .runtime
            .install_observation_bridge(Arc::clone(&pending))
        {
            Ok(seed) => seed,
            Err(RuntimeBootstrapError::BridgeAlreadyInstalled { conversation_id }) => {
                config.runtime.release_client_binding();
                return Err(HostConstructionError::ObservationBridgeAlreadyInstalled {
                    conversation_id,
                });
            }
            Err(RuntimeBootstrapError::RuntimeAlreadyActivated { conversation_id }) => {
                config.runtime.release_client_binding();
                return Err(HostConstructionError::RuntimeAlreadyActivated { conversation_id });
            }
            Err(RuntimeBootstrapError::Durable(message)) => {
                config.runtime.release_client_binding();
                return Err(HostConstructionError::Durable(message));
            }
        };

        // ---- Infallible wiring: from here construction always succeeds. ----
        //
        // The projection mirrors the runtime's authoritative seed exactly
        // — current Surface working set, session model, capability snapshot, and
        // pending inbound — entirely as snapshot state. No seeded fact is
        // routed through `RuntimeClientProjection::apply`, so bootstrap
        // allocates no cursor and publishes no event: the first cursor
        // belongs to a real post-activation transition. (The background
        // seed is provably empty by the ownership-transfer invariant: a
        // `ConversationRuntime` is constructed only over a pristine
        // tool-runtime background plane, and the transfer then refuses
        // dispatch commits while its mailbox is bound inactive.)
        let mut projection = RuntimeClientProjection::new(
            seed.conversation_id.clone(),
            seed.messages.clone(),
            super::projection::capability_view(&seed.capabilities, &seed.capability_availability),
            seed.model.clone(),
            replay_limit,
        );
        projection.bootstrap(&seed);
        let inner = Arc::new(ClientInner {
            conversation_id: seed.conversation_id,
            agent_id: config.runtime.agent_id().clone(),
            runtime: Some(config.runtime),
            store,
            read_only: false,
            replay_limit,
            session_control,
            state: Mutex::new(ClientState {
                projection,
                control_attachment: None,
                read_only_attachments: BTreeMap::new(),
                next_attachment_seq: 0,
            }),
            pending,
            worker_started: AtomicBool::new(false),
        });
        if let Some(runtime) = inner.runtime.as_ref() {
            runtime.install_interaction_publication_authority(Arc::new(
                RootInteractionPublicationAuthority {
                    inner: Arc::downgrade(&inner),
                },
            ));
        }
        // Only the projection worker: activating the conversation runtime
        // is the composition's explicit next step, never a side effect of
        // binding a client.
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
        if self.inner.read_only {
            super::endpoint::RuntimeClientEndpoint::new_read_only(self.clone())
        } else {
            super::endpoint::RuntimeClientEndpoint::new(self.clone())
        }
    }

    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when the
    /// control attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub fn attach(
        &self,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.inner.attach(protocol_version)
    }

    /// Admits one explicitly read-only attachment to the live projection.
    ///
    /// The attachment can read the same projection as the Runtime Client
    /// control owner, including disposable live state, but every semantic
    /// mutation is rejected before it reaches the conversation runtime.
    pub(crate) fn attach_read_only(
        &self,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.inner.attach_read_only(protocol_version)
    }

    /// Releases one attachment. Idempotent. Detach is never cancellation
    /// and never shutdown.
    pub fn detach(&self, attachment_id: &AttachmentId) {
        self.inner.detach(attachment_id);
    }

    /// Submits one inbound user message through the conversation runtime's
    /// single publish path.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidRequest`] for empty content,
    /// [`RuntimeClientError::RuntimeShutdown`] after shutdown, and
    /// [`RuntimeClientError::InvalidState`] for a mailbox admission
    /// failure.
    pub fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.submit_inbound(content)
    }

    /// Requests cancellation of the current attempt.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NoCurrentAttempt`] when no attempt
    /// is currently cancellable.
    pub fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.cancel_current_attempt()
    }

    /// Manually compacts the current context while the runtime is idle.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle, busy, context, or durability error. A
    /// successful response means the canonical compaction already committed.
    pub async fn compact_context(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.compact_context().await
    }

    /// Reloads current runtime resources for future attempts only.
    ///
    /// # Errors
    ///
    /// Returns a typed busy result while an attempt, interaction, compaction,
    /// or another reload owns the semantic boundary.
    pub async fn reload_resources(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.reload_resources().await
    }

    /// Responds to one live native interaction through Runtime Client
    /// semantics. The response is finite and cannot replace tool arguments.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InteractionNotPending`] for a stale or
    /// settled identity, or [`RuntimeClientError::InteractionInvalidResponse`]
    /// for a response that fails bounded validation.
    pub async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: InteractionResponse,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.respond_interaction(interaction, response).await
    }

    /// Reads the authoritative snapshot and its cursor, linearized
    /// together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted.
    pub fn snapshot(
        &self,
    ) -> Result<(super::snapshot::RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError>
    {
        self.inner.snapshot()
    }

    /// Returns a durable request-history read handle owned by the
    /// conversation runtime.
    #[must_use]
    pub fn request_history(&self) -> RequestHistory {
        self.inner.request_history()
    }

    /// Reads one bounded durable transcript page. `before` is exclusive and
    /// means that the returned page contains only older transcript entries.
    /// This cursor is independent from the live Runtime Client event cursor.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error for an invalid limit, a durable
    /// read failure, or an invalid resolved transcript item.
    pub fn transcript_page(
        &self,
        before: Option<RuntimeClientTranscriptCursor>,
        limit: usize,
    ) -> Result<RuntimeClientTranscriptPage, RuntimeClientError> {
        match self.inner.transcript_page(before, limit)? {
            RuntimeClientResult::TranscriptPage { page } => Ok(page),
            other => Err(RuntimeClientError::RuntimeFailure {
                message: format!("unexpected transcript page result: {other:?}"),
            }),
        }
    }

    /// Reconstructs one retained provider-neutral request from durable facts.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.inner.reconstruct_request(identity)
    }

    /// Subscribes one attachment to the observation stream after a
    /// serviceable cursor.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NotAttached`] for an unknown
    /// attachment identity and [`RuntimeClientError::ResyncRequired`] for
    /// an unserviceable cursor.
    pub fn subscribe_events(
        &self,
        attachment_id: &AttachmentId,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(EventSubscription, RuntimeClientResult), RuntimeClientError> {
        self.inner.subscribe_events(attachment_id, after_cursor)
    }

    /// Reads the active capability projection.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the
    /// observation stream is over.
    pub fn capability(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.capability()
    }

    /// Reads the safe public model catalog.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub fn model_catalog(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.model_catalog()
    }

    /// Reads the authoritative session model state through the folded
    /// projection.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.model_get()
    }

    /// Replaces the authoritative session model configuration through the
    /// conversation runtime.
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
        self.inner.model_set(config)
    }

    /// Requests the runtime `ApprovalMode` transition through the authoritative
    /// runtime control plane.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error when the runtime is inactive,
    /// durably failed, or its projection is no longer serviceable.
    pub fn approval_mode_set(
        &self,
        mode: crate::runtime::ApprovalMode,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.approval_mode_set(mode)
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
        self.inner.background_status(execution_id)
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
        self.inner.background_cancel(execution_id)
    }

    /// Inspects one subagent child through the authoritative registry
    /// (Issue #60).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownSubagent`] for an unknown
    /// subagent identity.
    pub fn subagent_status(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.subagent_status(subagent_id)
    }

    /// Requests cancellation of one subagent child through the
    /// authoritative registry. Acceptance and eventual settlement remain
    /// distinct.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnknownSubagent`] for an unknown
    /// subagent identity.
    pub fn subagent_cancel(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.subagent_cancel(subagent_id)
    }

    /// Disposes one exact retained workspace owned by a terminal subagent.
    /// The operation is asynchronous because physical Git verification and
    /// removal belong to the runtime/workspace plane.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error when the request is not writable,
    /// the subagent is not eligible, ownership cannot be proven, or disposal
    /// fails in the backend.
    pub async fn subagent_workspace_dispose(
        &self,
        subagent_id: &crate::runtime::identity::SubagentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.subagent_workspace_dispose(subagent_id).await
    }

    /// Drains the local runtime and completes only at runtime quiescence.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidState`] while the runtime is
    /// not yet activated.
    pub async fn shutdown(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.shutdown().await
    }

    /// Convenience for tests: host-level conversation-state accessors that
    /// forward to the conversation runtime.
    #[cfg(test)]
    pub(crate) fn host_ledger(&self) -> Option<Vec<crate::message::types::MessageBlock>> {
        self.inner
            .runtime
            .as_ref()
            .and_then(ConversationRuntime::coordinator_ledger)
    }

    /// The runtime's active Surface identities, or `None` while an attempt
    /// owns the conversation state.
    #[cfg(test)]
    pub(crate) fn host_active_ids(&self) -> Option<Vec<crate::runtime::identity::MessageId>> {
        self.inner
            .runtime
            .as_ref()
            .and_then(ConversationRuntime::coordinator_active_ids)
    }

    #[cfg(test)]
    #[allow(dead_code)] // used by the race regression tests
    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner
            .runtime
            .as_ref()
            .is_some_and(ConversationRuntime::has_current_attempt)
    }

    /// A non-owning handle to the shared host state, for lifetime tests.
    #[cfg(test)]
    pub(crate) fn weak_inner(&self) -> Weak<ClientInner> {
        Arc::downgrade(&self.inner)
    }

    /// A non-owning handle to the shared conversation runtime state, for
    /// lifetime tests.
    #[cfg(test)]
    pub(crate) fn weak_runtime_inner(
        &self,
    ) -> Weak<crate::runtime::conversation_runtime::RuntimeInner> {
        self.inner
            .runtime
            .as_ref()
            .expect("live host has a conversation runtime")
            .weak_inner()
    }

    /// Installs the deterministic worker-exit signal of the projection
    /// worker, for lifetime tests.
    #[cfg(test)]
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        self.inner.pending.install_worker_exit_probe(sender);
    }

    /// Installs the deterministic worker-exit signal of the admission
    /// worker, for lifetime tests.
    #[cfg(test)]
    pub(crate) fn install_admission_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        self.inner
            .runtime
            .as_ref()
            .expect("live host has a conversation runtime")
            .install_worker_exit_probe(sender);
    }

    /// The conversation runtime this host observes and controls.
    #[cfg(test)]
    pub(crate) fn runtime(&self) -> &ConversationRuntime {
        self.inner
            .runtime
            .as_ref()
            .expect("live host has a conversation runtime")
    }
}

/// The construction-time configuration of one Runtime Client host.
pub struct RuntimeClientHostConfig {
    /// The conversation runtime this host observes and controls.
    ///
    /// This is the one conversation authority of the runtime; the host
    /// derives its identity, its snapshot seed, and every control outcome
    /// from it.
    pub runtime: ConversationRuntime,
    /// The bounded projection replay retention; the default is used when
    /// omitted. This cache is not the durable Event Journal.
    pub replay_limit: Option<usize>,
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
    host: Arc<ClientInner>,
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tokio::sync::watch;

    use super::{EventDelivery, EventSubscription, RuntimeClientHost, RuntimeClientHostConfig};
    use crate::context::{
        AgentStatusClock, AgentStatusConfig, AgentStatusEngine, DefaultTokenEstimator,
        TokenEstimator,
    };
    use crate::conversation::SurfaceRevision;
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::local_runtime::session::SessionPersistentState;
    use crate::local_runtime::{CurrentRuntimeConfig, LocalSessionSupervisor, SessionCatalog};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelStream};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::conversation_runtime::{
        ConversationContextConfig, ConversationRuntime, ConversationRuntimeError, CoordinatorProbe,
        InboundAdmissionError, ModelUpdateError, RuntimeConversationConfig,
    };
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, EventId, MessageId, ToolCallId, ToolExecutionId, ToolId,
    };
    use crate::runtime::request_history::RequestHistory;
    use crate::runtime::types::RuntimeClock;
    use crate::runtime_client::endpoint::RuntimeClientEndpoint;
    use crate::runtime_client::event::RuntimeClientEvent;
    use crate::runtime_client::host::HostConstructionError;
    use crate::runtime_client::snapshot::RuntimeClientAttemptPhase;
    use crate::runtime_client::types::{
        RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent, RuntimeClientRequest,
        RuntimeClientResult,
    };
    use crate::scripted_suites::support::model::scripted_session_model;
    use crate::tools::background::{
        BackgroundDispatchError, BackgroundDispatchOutcome, BackgroundExecutionSnapshot,
        BackgroundLifecycle, ConversationBackgroundRegistry,
    };
    use crate::tools::deadline::ToolProgressCapability;
    use crate::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
    use crate::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
        ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolReplayPolicy,
        ToolResultContent,
    };

    fn test_resources(
        capability: &crate::capabilities::CapabilityCoordinator,
    ) -> Arc<crate::runtime::RuntimeResourceSnapshot> {
        Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
            crate::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            capability.current_snapshot(),
        ))
    }

    fn test_resource_loader(
        capability: &crate::capabilities::CapabilityCoordinator,
    ) -> Arc<dyn crate::runtime::RuntimeResourceLoader> {
        Arc::new(crate::runtime::FilesystemRuntimeResourceLoader::new(
            capability.current_snapshot().workspace_root(),
        ))
    }

    fn request_snapshots(history: &RequestHistory) -> Vec<crate::model::RequestSnapshot> {
        let mut snapshots = Vec::new();
        let mut cursor = None;
        loop {
            let page = history.page(cursor, 32).expect("request snapshot page");
            if page.snapshots.is_empty() {
                break;
            }
            cursor = page.next_sequence;
            snapshots.extend(page.snapshots);
        }
        snapshots
    }

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
        request_count: Arc<watch::Sender<usize>>,
    }

    impl GatedAdapter {
        fn new(scripts: Vec<Vec<GatedStep>>) -> Self {
            let (request_count, _receiver) = watch::channel(0);
            Self {
                scripts: Mutex::new(scripts.into_iter().map(VecDeque::from).collect()),
                requests: Arc::new(Mutex::new(Vec::new())),
                request_count: Arc::new(request_count),
            }
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().expect("requests lock").clone()
        }

        fn request_count(&self) -> watch::Receiver<usize> {
            self.request_count.subscribe()
        }
    }

    impl ModelAdapter for GatedAdapter {
        fn protocol(&self) -> ModelProtocol {
            ModelProtocol::OpenAiChatCompletions
        }

        fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
            let request_count = {
                let mut requests = self.requests.lock().expect("requests lock");
                requests.push(request);
                requests.len()
            };
            self.request_count.send_replace(request_count);
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
                                return Some((
                                    crate::model::adapter::ModelStreamItem::Event(event),
                                    (script, cancellation),
                                ));
                            }
                            Some(GatedStep::ParkUntilReleased(mut release)) => {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        return Some((crate::model::adapter::ModelStreamItem::Event(ModelEvent::Failed {
                                            error: ModelError {
                                                kind: ModelErrorKind::Cancelled,
                                                message: "cancelled while parked".to_owned(),
                                                retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                                                retry_after_ms: None,
                                                provider_code: None,
                                                context_overflow: None,
                                                malformed_tool_proposal: None,
                                                timeout_phase: None,
                                                generation: None,
                                            },
                                        }), (VecDeque::new(), cancellation)));
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

    /// A parking background executor: its returned future reports entry,
    /// waits on a durable release state, then settles with a fixed result.
    struct ParkingBackgroundTool {
        #[allow(dead_code)] // the definition documents the tool identity
        definition: ToolDefinition,
        started: watch::Sender<bool>,
        release: watch::Sender<bool>,
        execution_gate: Option<watch::Receiver<bool>>,
    }

    impl ParkingBackgroundTool {
        fn new() -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
            Self::build(None)
        }

        /// Builds a fixture whose returned future waits at an explicit gate
        /// before it observes `release`. This is used by the lost-wakeup
        /// regression to force release-before-wait ordering.
        fn new_with_execution_gate() -> (
            Self,
            watch::Receiver<bool>,
            watch::Sender<bool>,
            watch::Sender<bool>,
        ) {
            let (execution_gate, execution_gate_rx) = watch::channel(false);
            let (tool, started, release) = Self::build(Some(execution_gate_rx));
            (tool, started, release, execution_gate)
        }

        fn build(
            execution_gate: Option<watch::Receiver<bool>>,
        ) -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
            let (started, started_rx) = watch::channel(false);
            let (release, _release_rx) = watch::channel(false);
            (
                Self {
                    definition: ToolDefinition {
                        id: ToolId::new("tool-bg"),
                        name: "bg".to_owned(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type": "object"}),
                        execution_policy: ToolExecutionPolicy::ModelSelectable,
                        concurrency_policy: ToolConcurrencyPolicy::Sequential,
                        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
                        replay_policy: ToolReplayPolicy::Never,
                        origin: ToolOrigin::Builtin,
                    },
                    started,
                    release: release.clone(),
                    execution_gate,
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for ParkingBackgroundTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            let started = self.started.clone();
            let mut execution_gate = self.execution_gate.clone();
            let mut release = self.release.subscribe();
            crate::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    // This signal is published by the returned future, so
                    // observing it means the deterministic execution gate has
                    // actually been entered rather than merely returned by
                    // `start`.
                    started.send_replace(true);
                    if let Some(execution_gate) = execution_gate.as_mut() {
                        execution_gate
                            .wait_for(|entered| *entered)
                            .await
                            .expect("execution gate stays open");
                    }
                    release
                        .wait_for(|released| *released)
                        .await
                        .expect("release channel stays open");
                    ToolExecutionResult {
                        status: ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        managed_output: None,
                    }
                }),
                context.cancellation.clone(),
            )
        }

        fn progress_capability(&self) -> ToolProgressCapability {
            ToolProgressCapability::None
        }
    }

    /// Bounds only the terminal wait in background-runtime tests. The
    /// release and start orderings are established by watch state; this is
    /// fail-fast containment for a broken fixture or registry invariant, not
    /// a synchronization primitive.
    const BACKGROUND_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

    async fn await_background_started(
        started: &mut watch::Receiver<bool>,
        description: &'static str,
    ) {
        tokio::time::timeout(
            BACKGROUND_LIVENESS_GUARD,
            started.wait_for(|is_started| *is_started),
        )
        .await
        .unwrap_or_else(|_| panic!("{description}: start wait exceeded liveness guard"))
        .expect("start channel stays open");
    }

    async fn await_background_terminal(
        registry: &ConversationBackgroundRegistry,
        execution_id: &ToolExecutionId,
        description: &'static str,
    ) -> BackgroundExecutionSnapshot {
        tokio::time::timeout(
            BACKGROUND_LIVENESS_GUARD,
            registry.wait_until_terminal(execution_id),
        )
        .await
        .unwrap_or_else(|_| panic!("{description}: terminal wait exceeded liveness guard"))
        .unwrap_or_else(|| panic!("{description}: execution disappeared before terminal state"))
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

    /// A fixture over one conversation: the conversation runtime
    /// coordinator, its Runtime Client host adapter, and the scripted
    /// adapter driving attempts.
    struct HostFixture {
        _dir: tempfile::TempDir,
        host: RuntimeClientHost,
        runtime: ConversationRuntime,
        coordinator: crate::capabilities::CapabilityCoordinator,
    }

    /// Builds the conversation runtime + host over one conversation with
    /// the given adapter scripts and tool registry.
    async fn host_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        status_engine: AgentStatusEngine,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        host_fixture_with_native_tools(scripts, tools, status_engine, false).await
    }

    /// Builds the conversation runtime + host, optionally activating the
    /// real native tool plane. Skill projection tests opt into this variant
    /// so the fixture exercises the normal native composition and its
    /// mandatory Read capability.
    async fn host_fixture_with_native_tools(
        scripts: Vec<Vec<GatedStep>>,
        mut tools: ToolRegistry,
        status_engine: AgentStatusEngine,
        include_native_tools: bool,
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
        if include_native_tools {
            crate::tools::register_native_tools(
                &mut tools,
                crate::tools::NativeToolResources {
                    subagent_catalog: crate::runtime::subagent::SubagentCatalog::empty(),
                    background: tool_runtime.background().clone(),
                    subagents: None,
                },
                crate::tools::NativeToolPolicies::default(),
            )
            .expect("register native tools");
        }
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(tools),
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace.join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: std::collections::BTreeMap::new(),
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
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine,
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        })
        .expect("conversation runtime");
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        })
        .expect("runtime client host");
        // The explicit lifecycle boundary: the host bound over the inert
        // runtime, so semantic execution may begin now.
        runtime.activate();
        (
            adapter,
            HostFixture {
                _dir: dir,
                host,
                runtime,
                coordinator,
            },
        )
    }

    /// Builds a fixture whose conversation runtime carries the given
    /// coordinator synchronization hooks.
    async fn host_fixture_with_runtime_probe(
        scripts: Vec<Vec<GatedStep>>,
        probe: CoordinatorProbe,
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
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace.join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let runtime = ConversationRuntime::with_probe(
            RuntimeConversationConfig {
                agent_id: AgentId::new("agent-a"),
                model: scripted_session_model(adapter.clone()),
                approval_mode: crate::runtime::ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    status_engine: status_engine(),
                },
                tool_runtime,
                resources: test_resources(&coordinator),
                resource_loader: test_resource_loader(&coordinator),
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: None,
            },
            probe,
        )
        .expect("conversation runtime with probe");
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        })
        .expect("runtime client host");
        // The explicit lifecycle boundary: the host bound over the inert
        // runtime, so semantic execution may begin now.
        runtime.activate();
        (
            adapter,
            HostFixture {
                _dir: dir,
                host,
                runtime,
                coordinator,
            },
        )
    }

    /// The default status engine over the fixed clock.
    fn status_engine() -> AgentStatusEngine {
        AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(FixedStatusClock))
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

    fn durable_event(
        conversation_id: &ConversationId,
        event_id: &str,
        attempt_id: &AttemptId,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(event_id),
            sequence: 0,
            conversation_id: conversation_id.clone(),
            attempt_id: Some(attempt_id.clone()),
            turn_id: None,
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("fixed event timestamp")
                .with_timezone(&chrono::Utc),
            event,
        }
    }

    fn durable_tool_result() -> ToolExecutionResult {
        ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![ToolResultContent::Text(TextBlock {
                text: "child tool output".to_owned(),
            })],
            duration_ms: 7,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        }
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

    struct DurableHistoryFixture {
        conversation_id: ConversationId,
        store: Arc<SqliteConversationStore>,
        user: MessageBlock,
        assistant: MessageBlock,
        tool: MessageBlock,
        call: ToolCall,
        tool_result: ToolExecutionResult,
    }

    fn durable_history_fixture() -> DurableHistoryFixture {
        let conversation_id = ConversationId::new("conversation-child-inspection");
        let store = Arc::new(
            SqliteConversationStore::in_memory(conversation_id.clone()).expect("child store"),
        );
        let user = MessageBlock::User(inbound_text("child-user", "inspect this child"));
        let call = ToolCall {
            id: ToolCallId::new("child-call-1"),
            tool_id: ToolId::new("tool-read"),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "README.md"}),
        };
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("child-assistant"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "I will inspect the file.".to_owned(),
                }),
                AssistantContentBlock::ToolCall(call.clone()),
            ],
        });
        let tool_result = durable_tool_result();
        let tool = MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("child-tool-result"),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: tool_result.clone(),
        });
        append_durable_history(
            &store,
            &conversation_id,
            &assistant,
            &tool,
            &call,
            &tool_result,
        );
        DurableHistoryFixture {
            conversation_id,
            store,
            user,
            assistant,
            tool,
            call,
            tool_result,
        }
    }

    fn append_durable_history(
        store: &SqliteConversationStore,
        conversation_id: &ConversationId,
        assistant: &MessageBlock,
        tool: &MessageBlock,
        call: &ToolCall,
        tool_result: &ToolExecutionResult,
    ) {
        let user = MessageBlock::User(inbound_text("child-user", "inspect this child"));
        store
            .initialize(std::slice::from_ref(&user))
            .expect("child seed");
        let attempt_id = AttemptId::new("child-attempt-1");
        store
            .append_event(durable_event(
                conversation_id,
                "child-attempt-started",
                &attempt_id,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            ))
            .expect("attempt start");
        store
            .append_canonical_with_event(
                assistant,
                durable_event(
                    conversation_id,
                    "child-assistant-committed",
                    &attempt_id,
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: assistant.id().clone(),
                    },
                ),
            )
            .expect("assistant commit");
        store
            .append_event(durable_event(
                conversation_id,
                "child-tool-started",
                &attempt_id,
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
            ))
            .expect("tool start");
        store
            .append_event(durable_event(
                conversation_id,
                "child-tool-completed",
                &attempt_id,
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    result: tool_result.clone(),
                },
            ))
            .expect("tool completion");
        store
            .append_canonical_with_event(
                tool,
                durable_event(
                    conversation_id,
                    "child-tool-committed",
                    &attempt_id,
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: tool.id().clone(),
                        tool_call_id: call.id.clone(),
                    },
                ),
            )
            .expect("tool result commit");
        store
            .append_event(durable_event(
                conversation_id,
                "child-attempt-completed",
                &attempt_id,
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt_id.clone(),
                    finish_reason: ModelFinishReason::Stop,
                },
            ))
            .expect("attempt completion");
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
    const STREAM_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

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

    /// A durable attachment reads the child's canonical Ledger and Event
    /// Journal through the ordinary Runtime Client snapshot. It has no live
    /// runtime to mutate, and detaching it leaves both authorities byte-for-
    /// byte semantically unchanged.
    #[test]
    fn durable_attachment_replays_child_history_without_writing_or_copying() {
        let fixture = durable_history_fixture();

        let canonical_before = fixture
            .store
            .load_canonical()
            .expect("canonical before inspection");
        let events_before = fixture
            .store
            .read_events(None, 64)
            .expect("journal before inspection")
            .events;
        let host = RuntimeClientHost::new_durable(fixture.store.clone(), Some(16))
            .expect("inspection host");
        assert!(host.inner.runtime.is_none(), "inspection owns no runtime");

        let (attachment, initialized) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach child conversation");
        let RuntimeClientResult::Initialized {
            conversation_id: attached_id,
            snapshot,
            cursor,
            ..
        } = initialized
        else {
            panic!("durable attach must initialize the ordinary snapshot");
        };
        assert_eq!(attached_id, fixture.conversation_id);
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert_eq!(
            snapshot.messages,
            vec![
                fixture.user.clone(),
                fixture.assistant.clone(),
                fixture.tool.clone()
            ]
        );
        assert!(snapshot.transcript.entries.iter().any(|entry| {
            matches!(
                &entry.item,
                crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message { message }
                    if message == &fixture.assistant
            )
        }));
        assert!(snapshot.transcript.entries.iter().any(|entry| {
            matches!(
                &entry.item,
                crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message { message }
                    if message == &fixture.tool
            )
        }));
        let attempt = snapshot.attempt.expect("durable attempt history");
        assert!(matches!(
            attempt.phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed {
                    finish_reason: ModelFinishReason::Stop,
                }
            }
        ));
        assert!(matches!(
            attempt.foreground.as_slice(),
            [crate::runtime_client::snapshot::ForegroundToolExecution {
                call_id,
                state: crate::runtime_client::snapshot::ForegroundToolState::Settled { result, .. },
                ..
            }] if call_id == &fixture.call.id && result == &fixture.tool_result
        ));

        let rejected = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("must not mutate child"),
        });
        assert!(matches!(
            rejected.error,
            Some(RuntimeClientError::InvalidState { message })
                if message == "conversation inspection is read-only"
        ));
        attachment.detach();
        assert_eq!(
            fixture
                .store
                .load_canonical()
                .expect("canonical after inspection"),
            canonical_before
        );
        assert_eq!(
            fixture
                .store
                .read_events(None, 64)
                .expect("journal after inspection")
                .events,
            events_before
        );
    }

    /// A `snapshot_get` on a still-open durable attachment repairs a projection
    /// after journal state advances out of band. The read-only cursor remains
    /// at zero because durable history is bootstrap/resync input, never a live
    /// observation stream.
    #[test]
    fn durable_attachment_resyncs_cancelled_child_after_runtime_disappears() {
        let conversation_id = ConversationId::new("conversation-cancelled-child");
        let store = Arc::new(
            SqliteConversationStore::in_memory(conversation_id.clone()).expect("child store"),
        );
        let partial_user = MessageBlock::User(inbound_text(
            "cancelled-child-user",
            "the cancelled child started this durable turn",
        ));
        store
            .initialize(std::slice::from_ref(&partial_user))
            .expect("partial child seed");
        let host = RuntimeClientHost::new_durable(store.clone(), None).expect("inspection host");
        let (attachment, initialized) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("initial attach");
        let RuntimeClientResult::Initialized { snapshot, .. } = initialized else {
            panic!("durable attach must initialize");
        };
        assert_eq!(snapshot.messages, vec![partial_user]);
        assert!(snapshot.attempt.is_none());

        let attempt_id = AttemptId::new("cancelled-attempt");
        store
            .append_event(durable_event(
                &conversation_id,
                "cancelled-attempt-started",
                &attempt_id,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            ))
            .expect("attempt start");
        store
            .append_event(durable_event(
                &conversation_id,
                "cancelled-attempt-cancelled",
                &attempt_id,
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt_id.clone(),
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                },
            ))
            .expect("attempt cancellation");

        let (repaired, cursor) = host.snapshot().expect("durable resync");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(matches!(
            repaired.attempt.expect("cancelled attempt").phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled {
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                }
            }
        ));
        attachment.detach();
    }

    /// First attachment succeeds, the second concurrent attachment is
    /// rejected deterministically without evicting the first, detach
    /// permits a later attachment with a distinct identity, and request
    /// ids are attachment-scoped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attachment_lifecycle_and_request_id_scope() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (first, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION);
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
        let bad = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1);
        assert!(matches!(
            bad,
            Err(RuntimeClientError::UnsupportedProtocolVersion {
                requested,
                ..
            }) if requested == crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1
        ));

        // Explicit detach releases the attachment; a fresh attachment has
        // a distinct identity and a fresh request-id scope.
        first.detach();
        let (second_attachment, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

    /// Read-only inspection attachments share the live projection without
    /// competing with the one control owner or changing provider admission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn read_only_attachments_coexist_with_the_control_attachment() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (control, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("control attachment");
        let (inspection_one, _) = fixture
            .host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("first read-only attachment");
        let (inspection_two, _) = fixture
            .host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("second read-only attachment");
        assert_ne!(
            inspection_one.attachment_id(),
            inspection_two.attachment_id()
        );

        let rejection = inspection_one.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert!(matches!(
            rejection.error,
            Some(RuntimeClientError::InvalidState { message })
                if message == "conversation inspection is read-only"
        ));
        assert!(
            inspection_two
                .handle_request(RuntimeClientRequest::SnapshotGet {
                    id: crate::runtime_client::RequestId::new(2),
                })
                .error
                .is_none()
        );
        assert!(
            control
                .handle_request(RuntimeClientRequest::SnapshotGet {
                    id: crate::runtime_client::RequestId::new(3),
                })
                .error
                .is_none(),
            "read-only attachments do not displace control ownership"
        );
    }

    /// Dropping the attachment releases it (RAII detach semantics).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_attachment_detaches_it() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        drop(attachment);
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

    /// Submitting while idle admits and runs the attempt through the
    /// conversation runtime's single admission path; the admission response
    /// is accepted, not finished; the attempt settles and the canonical
    /// history is committed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submit_when_idle_admits_and_runs_the_attempt() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

        // The first model request observed the admitted Runtime Agent Status
        // fact through canonical history.
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].messages.iter().any(|message| {
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == message_id)
        }));
        assert!(requests[0].messages.iter().any(|message| {
            matches!(
                message,
                crate::model::ModelInputMessage::Canonical(MessageBlock::User(user))
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    )
            )
        }));

        // The snapshot carries the committed canonical history and the
        // settled attempt.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.messages.len(),
            3,
            "user message + admitted Agent Status + Assistant message"
        );
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        // The terminal settlement event is emitted by the loop, and the
        // authoritative canonical history is committed by the runtime's
        // settlement path immediately afterwards. Observing the commit is a
        // wait on that exact condition, never a delay.
        await_canonical_history(&fixture.host, &snapshot.messages).await;

        // Request facts remain in the durable ConversationStore after the
        // AgentExecutionResult transfer. Mutate the live session
        // configuration after settlement and reconstruct from the durable
        // snapshot plus its historical Surface; neither current
        // configuration nor a live contributor is consulted.
        let requests = adapter.requests();
        let history = fixture.host.request_history();
        let snapshots = request_snapshots(&history);
        assert_eq!(snapshots.len(), 1);
        let retained = snapshots[0].clone();
        let mut live_config = fixture.runtime.model_config();
        live_config.request_params.insert(
            "live_mutation".to_owned(),
            serde_json::json!("changed-after-settlement"),
        );
        fixture
            .host
            .model_set(live_config)
            .expect("live model mutation remains valid");
        let reconstructed = fixture
            .host
            .reconstruct_request(&retained.identity)
            .expect("retained request reconstructs after settlement");
        assert_eq!(reconstructed, requests[0]);
        assert_eq!(
            history.get(&retained.identity).unwrap(),
            Some(retained),
            "request history lookup is identity-based and immutable"
        );
    }

    /// A composed runtime keeps every actual primary request, including an
    /// overflow retry, reconstructable in the durable `ConversationStore`
    /// after the `AgentExecutionResult` has been transferred and dropped. The
    /// retry keeps the pending fresh inbound visible while both request facts
    /// remain reconstructable from their own Surface revisions.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one durable overflow/retry scenario
    async fn settled_host_retains_distinct_overflow_request_snapshots() {
        let (adapter, fixture) = host_fixture(
            vec![
                one_turn_stop(),
                vec![GatedStep::Emit(ModelEvent::Failed {
                    error: ModelError {
                        kind: ModelErrorKind::ContextWindowExceeded,
                        message: "context window exceeded".to_owned(),
                        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
                    },
                })],
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::Emit(ModelEvent::TextDelta {
                        block_index: ContentBlockIndex::new(0),
                        text: "historical summary".to_owned(),
                    }),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        await_request_history_len(&fixture.host, 1).await;

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        await_request_history_len(&fixture.host, 3).await;

        let history = fixture.host.request_history();
        let snapshots = request_snapshots(&history);
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].identity.retry_number, 0);
        assert_eq!(snapshots[1].identity.retry_number, 0);
        assert_eq!(snapshots[2].identity.retry_number, 1);
        assert_eq!(
            snapshots[1].identity.attempt_id,
            snapshots[2].identity.attempt_id
        );
        assert_eq!(
            snapshots[1].context_generation, snapshots[2].context_generation,
            "overflow retry keeps the one admitted context generation"
        );
        assert_ne!(
            snapshots[1].surface_revision, snapshots[2].surface_revision,
            "compaction gives the retry its own historical Surface revision"
        );

        let provider_requests = adapter.requests();
        assert_eq!(
            provider_requests.len(),
            4,
            "three primary requests plus summary"
        );
        for (snapshot, request) in snapshots.iter().zip([
            &provider_requests[0],
            &provider_requests[1],
            &provider_requests[3],
        ]) {
            assert_eq!(
                fixture
                    .host
                    .reconstruct_request(&snapshot.identity)
                    .expect("settled historical request reconstructs"),
                *request
            );
        }
        assert!(
            provider_requests[3].messages.iter().any(|message| {
                matches!(
                    message,
                    crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id.as_str() == "conv-host-inbound-2"
                )
            }),
            "the retry still observes the pending fresh inbound"
        );
    }

    /// Waits until the runtime owns the conversation state again and its
    /// Message Ledger equals the expected records.
    async fn await_canonical_history(host: &RuntimeClientHost, expected: &[MessageBlock]) {
        tokio::time::timeout(std::time::Duration::from_mins(2), async {
            loop {
                if host.host_ledger().as_deref() == Some(expected) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the projection mirrors the authoritative canonical history");
    }

    /// Waits for the durable request-fact read to expose the expected count.
    async fn await_request_history_len(host: &RuntimeClientHost, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_mins(2), async {
            loop {
                if request_snapshots(&host.request_history()).len() == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request history transfer must settle");
    }

    /// Waits until the fake provider has actually received the expected
    /// number of provider-neutral requests. Request snapshots are durable
    /// before adapter invocation, so request-history visibility alone is not
    /// a sufficient synchronization point for provider-side assertions.
    async fn await_adapter_request_count(adapter: &GatedAdapter, expected: usize) {
        let mut count = adapter.request_count();
        tokio::time::timeout(
            std::time::Duration::from_mins(2),
            count.wait_for(|actual| *actual >= expected),
        )
        .await
        .expect("provider invocation must settle")
        .expect("provider request-count signal must stay open");
    }

    /// Seeds one real settled user turn and returns the exact retained
    /// Surface revision selected for a fork/tree transition. The loop waits
    /// on the runtime's durable head, not on a scheduling delay.
    async fn await_text_boundary(
        runtime: &ConversationRuntime,
        adapter: &GatedAdapter,
        text: &str,
    ) -> (SurfaceRevision, MessageId, Vec<MessageBlock>) {
        let message_id = runtime
            .submit_inbound(submit_content(text))
            .expect("boundary turn accepted")
            .message_id;
        await_adapter_request_count(adapter, 1).await;
        tokio::time::timeout(std::time::Duration::from_mins(2), async {
            loop {
                let (revision, messages) = runtime
                    .historical_head_snapshot()
                    .expect("historical head snapshot");
                let found_user = messages.iter().any(
                    |message| matches!(message, MessageBlock::User(user) if user.id == message_id),
                );
                let found_assistant = messages
                    .iter()
                    .any(|message| matches!(message, MessageBlock::Assistant(_)));
                if found_user && found_assistant {
                    return (revision, message_id.clone(), messages);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("settled boundary must become durable")
    }

    fn catalog_conversation(
        catalog_root: &std::path::Path,
        session_id: &str,
        conversation_id: &crate::runtime::identity::ConversationId,
    ) -> Vec<MessageBlock> {
        let path = catalog_root
            .join("sessions")
            .join(session_id)
            .join("conversations")
            .join(conversation_id.as_str())
            .join("conversation.sqlite");
        SqliteConversationStore::open(conversation_id.clone(), &path)
            .expect("destination conversation store")
            .load_canonical()
            .expect("destination canonical history")
    }

    /// Accepts one durable Pending Inbound prompt into the catalog-owned
    /// conversation store of the active Session — the exact durable boundary
    /// that makes a Session used and resume-visible. The fixture runtime
    /// owns a *separate* store file for the same `ConversationId`, so a
    /// runtime-side `submit_inbound` never reaches the catalog's
    /// classification authority; Session-lifecycle assertions must write
    /// through the catalog's own store.
    fn accept_catalog_pending_inbound(catalog_root: &std::path::Path, text: &str) {
        let catalog = SessionCatalog::open_existing(catalog_root)
            .expect("open catalog")
            .expect("catalog exists");
        let (session_id, node, _) = catalog.active_lineage().expect("active lineage");
        let store = SqliteConversationStore::open(
            node.conversation_id.clone(),
            &catalog.database_path(&session_id, &node.conversation_id),
        )
        .expect("catalog conversation store");
        store
            .accept_inbound(crate::durable::InboundDraft {
                message_id: None,
                source: UserSource::Human,
                kind: InboundKind::Message,
                content: submit_content(text),
                timestamp: chrono::Utc::now(),
                correlation: None,
            })
            .expect("accept catalog pending inbound");
    }

    /// Submitting while an attempt is running queues the message in the
    /// authoritative mailbox; the running attempt drains it at its next
    /// safe boundary. An enqueue during an active attempt never creates a
    /// second `AgentExecution`.
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
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
        assert!(fixture.host.has_current_attempt());

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
        // in the authoritative mailbox diagnostics, and no second
        // AgentExecution exists.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.inbound.pending.len(), 1);
        assert_eq!(snapshot.inbound.pending[0].message.id, second_id);
        assert_eq!(adapter.requests().len(), 1, "no second AgentExecution yet");

        // Release the parked turn: the safe boundary drains the queued
        // message and a second turn observes it within the SAME attempt.
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
                .any(|message| matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == second_id))
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
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

        // A second cancel is idempotent at the signal level.
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

        // Release the parked model so it can finish.
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
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
    /// never drains mailbox contents: the mailbox is drained only by the
    /// conversation runtime's admission/safe-boundary authority, never by
    /// the client boundary (Test 2).
    #[allow(clippy::too_many_lines)] // one complete detach lifecycle
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_never_mutates_background_or_mailbox_state() {
        let (release_tx, release_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::ParkUntilReleased(release_rx),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        // Dispatch one detached background execution directly through the
        // authoritative registry.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
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
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;
        // One mailbox item admitted by the runtime's idle wakeup: the first
        // attempt starts and parks in its model stream.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("msg-first", "admitted"))
            .expect("enqueue");
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
        // A second mailbox item stays pending while the attempt runs.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("msg-pending", "kept"))
            .expect("enqueue");

        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (before, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(before.background.len(), 1);
        assert!(matches!(
            before.background[0].state,
            BackgroundLifecycle::Running
        ));
        assert_eq!(before.inbound.pending.len(), 1);
        assert_eq!(before.inbound.pending[0].message.id.as_str(), "msg-pending");
        attachment.detach();

        // After detach the background execution still runs and the mailbox
        // item still pends; the running attempt is untouched.
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
        assert!(
            after.attempt.as_ref().is_some_and(|attempt| {
                matches!(attempt.phase, RuntimeClientAttemptPhase::Running)
            }),
            "detach never cancels the active attempt"
        );

        // Releasing the model lets the attempt settle: the safe boundary
        // drains the pending mailbox item into a second turn within the
        // same attempt.
        release_tx.send(true).expect("release");
        await_request_history_len(&fixture.host, 2).await;
        let (settled, _) = fixture.host.snapshot().expect("snapshot");
        assert!(
            settled.inbound.pending.is_empty(),
            "the runtime drained the pending item at the safe boundary"
        );

        // The background execution settles normally after release.
        release.send_replace(true);
        await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "detach background execution",
        )
        .await;
        let (final_snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(matches!(
            final_snapshot.background[0].state,
            BackgroundLifecycle::Succeeded
        ));
    }

    /// Detach never changes canonical conversation state, and a detached
    /// conversation keeps admitting asynchronous inbound through the one
    /// coordinator path (Test 2).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_does_not_affect_conversation_or_future_async_admission() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        fixture.runtime.settlement_signal().notified().await;
        let (before, _) = fixture.host.snapshot().expect("snapshot");
        let ledger_before = fixture
            .runtime
            .coordinator_ledger()
            .expect("settled ledger");

        // Detach: nothing semantic changes.
        attachment.detach();
        let (after_detach, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            after_detach.messages, before.messages,
            "detach changes nothing"
        );
        assert_eq!(
            fixture
                .runtime
                .coordinator_ledger()
                .expect("settled ledger"),
            ledger_before,
            "detach never mutates canonical conversation state"
        );

        // A purely asynchronous enqueue with no attachment admits exactly
        // one further attempt through the runtime wake gate.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("conv-host-async-2", "async after detach"))
            .expect("async enqueue");
        await_request_history_len(&fixture.host, 2).await;
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(
            adapter.requests().len(),
            2,
            "the detached conversation admitted the next attempt"
        );
    }

    /// There is exactly one authoritative mutable conversation-state owner
    /// at a time, and ownership transfers at the attempt boundaries.
    ///
    /// Since Issue #54 the ownership is *structural*: admission moves the
    /// one `ConversationState` out of the coordinator, so while an attempt
    /// runs the coordinator holds nothing at all and physically cannot
    /// mutate a competing copy.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn canonical_history_has_one_owner_at_a_time() {
        // Turn 1 calls a tool that parks. The loop therefore commits the
        // Assistant message (the model stream ended) and then blocks in tool
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
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        // Idle: the runtime owns the conversation state, and it is the
        // projection's only source.
        assert_eq!(
            fixture.host.host_ledger(),
            Some(Vec::new()),
            "an idle runtime owns an empty conversation state"
        );
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

        // Running: the loop committed the Assistant message (observable on the
        // stream) and is now parked inside tool execution, so the attempt
        // provably has not settled.
        receive_until(&subscription, |event| {
            matches!(
                &event.event,
                RuntimeClientEvent::MessageCommitted { message, .. }
                    if matches!(message, MessageBlock::Assistant(_))
            )
        })
        .await;
        await_background_started(&mut tool_started, "the parking tool started").await;
        assert!(
            fixture.host.host_ledger().is_none(),
            "the attempt owns the conversation state; the runtime holds nothing"
        );
        assert!(
            fixture.host.host_active_ids().is_none(),
            "there is no runtime-side surface to compete with the attempt's"
        );
        let (mirror, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            mirror.messages.len(),
            3,
            "the projection mirrors the attempt's committed history"
        );
        assert!(fixture.host.has_current_attempt());

        // An inbound message arriving now stays mailbox-owned: the runtime
        // does not append it to a competing history.
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        assert!(
            fixture.host.host_ledger().is_none(),
            "a busy-path submission never gives the runtime a competing conversation state"
        );

        // Releasing the tool lets the attempt finish its tool turn and then
        // drain the mailbox at its safe boundary. The drained message joins
        // the *execution's* history — the loop commits it — and the attempt
        // continues rather than settling.
        release.send_replace(true);
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

        // A third submission while idle: its admission is the deterministic
        // proof that settlement transferred the execution's final
        // conversation state back to the runtime — admission only happens
        // once `finish_attempt` restored the state and cleared the attempt
        // slot.
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
        // the tool turn, the safe-boundary drain, and both attempts.
        let (final_snapshot, _) = fixture.host.snapshot().expect("snapshot");
        let roles: Vec<&str> = final_snapshot
            .messages
            .iter()
            .map(|message| match message {
                MessageBlock::User(_) => "user",
                MessageBlock::Assistant(_) => "assistant",
                MessageBlock::Tool(_) => "tool",
            })
            .collect();
        assert_eq!(
            roles,
            vec![
                "user",
                "user",
                "assistant",
                "tool",
                "user",
                "assistant",
                "user",
                "assistant",
            ],
            "one authoritative history, with only eligible canonical Runtime \
             context facts, extended across the tool turn, the safe-boundary \
             drain, and both attempts"
        );
    }

    /// Blocks off the runtime until the worker-exit signal arrives.
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

    /// A resource reload is one generation or nothing — in the snapshot the
    /// projection folds *and* in the event stream a client folds. Every cut
    /// of both is checked, so a consumer can never see the new capability
    /// generation beside the retired resource generation.
    ///
    /// The window this closes is real and not a lock-ordering detail: the
    /// projection worker folds on its own task, takes only the projection
    /// lock, and is woken by *every* enqueue. Two enqueues under the runtime
    /// state lock are still two folds, and the worker can be scheduled
    /// between them. Two *events* have the same defect one level out: they
    /// occupy two cursors, and a client that maintains its own projection
    /// incrementally sits at the first one holding a pairing the runtime
    /// never had. The reload therefore publishes exactly one observation
    /// carrying the whole generation, which folds into exactly one event.
    ///
    /// The projection worker is parked for the duration, so the fold
    /// schedule belongs to this test rather than to the scheduler: without
    /// that, the worker may drain the queue before the assertions below run
    /// and the test would pass by luck instead of by construction.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_resource_reload_never_exposes_a_half_published_generation() {
        let (_adapter, fixture) =
            host_fixture_with_native_tools(Vec::new(), ToolRegistry::new(), status_engine(), true)
                .await;
        let HostFixture {
            _dir: dir,
            host,
            runtime,
            coordinator,
        } = fixture;

        // An attached client watching the event stream from the baseline
        // cut, exactly as an incremental consumer would.
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        // Fold everything the composition produced, so the queue holds the
        // reload's observations and nothing else.
        let (baseline, baseline_cursor) = host.snapshot().expect("snapshot");
        let subscription = attachment
            .subscribe_events(baseline_cursor)
            .expect("subscribe from the snapshot cursor");
        let inner = host.weak_inner().upgrade().expect("host is live");
        assert_eq!(
            inner.queued_observations(),
            0,
            "the baseline snapshot drained the queue"
        );

        // From here the test owns every fold.
        inner.park_projection_worker();

        // A reload that genuinely moves both halves of the generation.
        write_probe_skill(&dir.path().join("workspace"), "generation-skill");
        let reloaded = runtime.reload_resources().await.expect("resource reload");
        assert!(
            reloaded.capability_revision > baseline.capabilities.revision,
            "the reload advanced the capability generation"
        );
        assert!(
            reloaded.resource_revision > baseline.resources.revision,
            "the reload advanced the resource generation"
        );

        // The whole generation is one enqueue. On a publication that
        // committed the capability half separately this is 2, and the cut
        // below is reachable.
        assert_eq!(
            inner.queued_observations(),
            1,
            "a reload publishes its complete generation as one observation"
        );

        // Nothing is visible before that one fold: the runtime committed
        // the capability generation already, and the projection still shows
        // the previous pair — not a mixture of the two.
        let (before, before_cursor) = host_projection_snapshot(&inner);
        assert_eq!(before.capabilities.revision, baseline.capabilities.revision);
        assert_eq!(before.resources.revision, baseline.resources.revision);
        assert_eq!(before_cursor, baseline_cursor, "no event was published yet");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "an unfolded observation publishes nothing"
        );

        // Step the queue one observation at a time and check every cut.
        let mut cuts = 0;
        while let Some((snapshot, _)) = inner.fold_one_observation() {
            cuts += 1;
            assert_eq!(
                (snapshot.capabilities.revision, snapshot.resources.revision),
                (reloaded.capability_revision, reloaded.resource_revision),
                "cut {cuts} exposed a generation pairing that never existed"
            );
            assert!(
                snapshot
                    .capabilities
                    .skills
                    .iter()
                    .any(|skill| skill.name == "generation-skill"),
                "cut {cuts} advanced the revision without the generation's skills"
            );
        }
        assert_eq!(cuts, 1, "there is exactly one cut to check");

        // The same property one level out: the client stream carries the
        // whole generation at a single cursor, so no incremental fold of it
        // can produce a half-published pairing either.
        let mut delivered = Vec::new();
        while let EventDelivery::Event(event) = subscription.try_next() {
            delivered.push(event);
        }
        assert_eq!(
            delivered.len(),
            1,
            "one generation is one cursor: {delivered:?}"
        );
        let RuntimeClientEvent::ResourceGenerationUpdated {
            capabilities,
            resources,
        } = &delivered[0].event
        else {
            panic!("the reload publishes its generation: {delivered:?}");
        };
        assert_eq!(
            (capabilities.revision, resources.revision),
            (reloaded.capability_revision, reloaded.resource_revision),
            "the one event carries both halves of the committed generation"
        );
        assert!(
            capabilities
                .skills
                .iter()
                .any(|skill| skill.name == "generation-skill"),
            "the published capability half is the one the reload composed"
        );

        drop(host);
        drop(runtime);
        drop(coordinator);
        drop(dir);
    }

    /// Reads the projection without draining the pending queue, so a test
    /// can look at the state a consumer would see at an exact cut.
    fn host_projection_snapshot(
        inner: &Arc<super::ClientInner>,
    ) -> (
        crate::runtime_client::snapshot::RuntimeClientSnapshot,
        RuntimeClientCursor,
    ) {
        inner
            .state
            .lock()
            .expect("runtime client host lock poisoned")
            .projection
            .snapshot()
            .expect("projection is live")
    }

    /// Releasing the last semantic owner destroys the host adapter and the
    /// conversation runtime, and terminates both workers — deterministically,
    /// and without depending on process exit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn releasing_the_last_owner_destroys_the_host_and_exits_the_workers() {
        let (_adapter, fixture) =
            host_fixture_with_native_tools(Vec::new(), ToolRegistry::new(), status_engine(), true)
                .await;
        let HostFixture {
            _dir: dir,
            host,
            runtime,
            coordinator,
        } = fixture;

        let weak = host.weak_inner();
        let weak_runtime = host.weak_runtime_inner();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        host.install_worker_exit_probe(exit_tx);
        let (runtime_exit_tx, runtime_exit_rx) = std::sync::mpsc::channel();
        host.install_admission_worker_exit_probe(runtime_exit_tx);

        // Exercise all three subsystem observation seams so every
        // `Arc<RuntimeObserver>` is installed and live at the moment the
        // runtime is released. The mailbox enqueue is admitted by the idle
        // wakeup and settles (the fixture has no model scripts, so the
        // attempt fails immediately); the request-history transfer proves
        // the attempt reached settlement and the runtime is idle again.
        host.runtime()
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("msg-lifetime", "queued"))
            .expect("enqueue");
        await_request_history_len(&host, 1).await;
        runtime.settlement_signal().notified().await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = runtime
            .tool_runtime()
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
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;
        release.send_replace(true);
        await_background_terminal(
            runtime.tool_runtime().background(),
            &execution_id,
            "host lifetime background execution",
        )
        .await;
        // The registry publishes its terminal notification into the
        // authoritative mailbox; the runtime wake gate admits it into a
        // second attempt, which settles immediately (no scripts). Waiting
        // for its request-history transfer makes the runtime provably idle
        // before the capability commit below.
        await_request_history_len(&host, 2).await;
        runtime.settlement_signal().notified().await;
        write_probe_skill(&dir.path().join("workspace"), "lifetime-skill");
        runtime
            .reload_resources()
            .await
            .expect("the runtime owns capability publication");

        // Every seam has fired and the projection folded them.
        let (before, _) = host.snapshot().expect("snapshot");
        assert!(
            before.messages.iter().any(|message| matches!(
                message,
                MessageBlock::User(user) if user.id.as_str() == "msg-lifetime"
            )),
            "the admitted mailbox enqueue committed to canonical history"
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
        // `Arc`s, and both worker tasks all still exist.
        drop(host);
        drop(runtime);

        // Both workers terminated on their own terminal conditions.
        await_worker_exit(exit_rx).await;
        await_worker_exit(runtime_exit_rx).await;
        assert_eq!(
            weak.strong_count(),
            0,
            "no strong reference to the host remains"
        );
        assert!(weak.upgrade().is_none(), "the host adapter is destroyed");
        assert_eq!(
            weak_runtime.strong_count(),
            0,
            "no strong reference to the conversation runtime remains"
        );
        assert!(
            weak_runtime.upgrade().is_none(),
            "the conversation runtime is destroyed, not merely unreachable"
        );

        // The authoritative subsystems outlived the projection, as they
        // must.
        drop(coordinator);
        drop(dir);
    }

    /// A surviving authoritative subsystem handle neither retains nor
    /// resurrects the runtime: its observer no-ops, and its own transitions
    /// still succeed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_surviving_subsystem_handle_never_retains_the_host() {
        let (_adapter, fixture) =
            host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let HostFixture {
            _dir: dir,
            host,
            runtime,
            coordinator,
        } = fixture;

        // Clone subsystem handles out of the runtime, exactly as an embedder
        // legitimately may.
        let mailbox = runtime.tool_runtime().mailbox();
        let registry = runtime.tool_runtime().background().clone();
        let weak = host.weak_inner();
        let weak_runtime = host.weak_runtime_inner();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        host.install_worker_exit_probe(exit_tx);

        drop(host);
        drop(runtime);
        await_worker_exit(exit_rx).await;
        assert!(weak.upgrade().is_none(), "the host is gone");
        assert!(
            weak_runtime.upgrade().is_none(),
            "the conversation runtime is gone"
        );

        // Authoritative mailbox transition: the observer's upgrade fails and
        // the seam no-ops, but the mailbox is unaffected.
        let sequence = mailbox
            .enqueue(inbound_text("msg-after", "still authoritative"))
            .expect("the mailbox remains authoritative without a runtime");
        assert_eq!(sequence.get(), 1);
        let batch = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("the select still works");
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
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;
        release.send_replace(true);
        await_background_terminal(
            &registry,
            &execution_id,
            "surviving registry background execution",
        )
        .await;

        // A claimed coordinator remains runtime-owned even after the runtime
        // and host are dropped; a surviving clone cannot bypass the resource
        // publication owner.
        write_probe_skill(&dir.path().join("workspace"), "after-skill");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        assert_eq!(
            coordinator.commit(candidate),
            Err(crate::capabilities::CapabilityCommitError::RuntimePublicationRequired),
            "a claimed coordinator cannot publish outside its runtime owner"
        );

        // None of those transitions resurrected the runtime or the host.
        assert_eq!(weak.strong_count(), 0);
        assert_eq!(weak_runtime.strong_count(), 0);
        assert!(
            weak.upgrade().is_none() && weak_runtime.upgrade().is_none(),
            "an observation seam can never resurrect a destroyed runtime"
        );
    }

    /// Attachment detach is not host destruction: the host survives, the
    /// attachment slot is released, and a fresh endpoint initializes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_releases_the_attachment_but_never_the_host() {
        let (_adapter, fixture) =
            host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let weak = fixture.host.weak_inner();

        let endpoint = fixture.host.endpoint();
        let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
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
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
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
    /// host (projection) lock is held by someone else.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_background_transition_completes_while_the_host_lock_is_held() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
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
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;

        // T1 takes the host lock and parks inside it.
        probe.arm_snapshot();
        let parked_host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || parked_host.snapshot());
        probe.wait_snapshot_entered();

        // T2 commits an authoritative registry transition and reports
        // completion.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let registry = fixture.runtime.tool_runtime().background().clone();
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
        release.send_replace(true);
        await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "snapshot-fold background execution",
        )
        .await;
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.background.len(), 1);
        assert_eq!(snapshot.background[0].execution_id, execution_id);
    }

    /// The same lock-order invariant for the capability coordinator, with a
    /// stronger barrier: the runtime-owned reload is parked *inside* the
    /// capability publication boundary, with its state lock held, and the
    /// host lock is taken while it is parked.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_capability_commit_never_waits_on_the_host_lock() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) =
            host_fixture_probe_with_native_tools(probe.clone(), Vec::new(), true).await;
        let (before, _) = fixture.host.snapshot().expect("snapshot");

        // A non-noop candidate: one discoverable Skill package.
        let workspace = fixture
            .runtime
            .tool_runtime()
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
        // T1 parks inside the runtime-owned commit while holding the
        // capability state lock.
        let hook = Arc::new(crate::capabilities::test_sync::CommitBoundaryHook::default());
        fixture
            .coordinator
            .install_commit_boundary_hook(hook.clone());
        let reloading = fixture.runtime.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reload_task = tokio::spawn(async move {
            let result = reloading.reload_resources().await;
            done_tx.send(()).expect("the test still listens");
            result
        });
        hook.wait_entered();

        // The host lock is acquirable while the capability state lock is
        // held: there is no `ClientState -> capability` edge.
        let (during, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            during.capabilities.revision, before.capabilities.revision,
            "the uncommitted candidate is not observable"
        );

        // Release: the runtime publication fires its observer with the
        // capability lock still held and completes without ever taking the
        // host lock.
        hook.proceed();
        done_rx
            .recv()
            .expect("an authoritative runtime publication never waits on the host lock");
        let committed = reload_task
            .await
            .expect("reload task")
            .expect("reload succeeds");
        assert!(committed.capability_revision > before.capabilities.revision);

        // The enqueued observation folds at the next host lock acquisition.
        let (after, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(after.capabilities.revision, committed.capability_revision);
        assert!(
            after
                .capabilities
                .skills
                .iter()
                .any(|entry| entry.name == "probe-skill"),
            "the capability projection folded the committed activation"
        );
    }

    /// Issue #81 follow-up: an availability-only capability commit — the
    /// Python plane becomes unavailable while the committed executable set
    /// is unchanged — never advances `CapabilityRevision`, yet a
    /// continuously attached client still observes it as one
    /// `ResourceGenerationUpdated` event whose capability view reports the
    /// unchanged revision. No `snapshot_get` polling is needed to discover
    /// the transition.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_availability_only_commit_publishes_a_capability_update_without_a_revision_swap() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (_, cursor) = fixture.host.snapshot().expect("snapshot");
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("subscribe from the snapshot cursor");
        let revision_before = fixture.coordinator.current_snapshot().revision();

        // Break one Python tool package *without* changing the executable
        // set: the workspace had no Python packages, and a malformed
        // package can never contribute one.
        let workspace = fixture
            .runtime
            .tool_runtime()
            .workspace()
            .root()
            .to_path_buf();
        let package = workspace.join(".agents").join("tools").join("broken-tool");
        std::fs::create_dir_all(&package).expect("package dir");
        // A package without `requirements.txt` is rejected in place by the
        // discovery pass.
        std::fs::write(
            package.join("server.py"),
            "from fastmcp import FastMCP\nmcp = FastMCP('broken')\n",
        )
        .expect("broken package source");
        let committed = fixture
            .runtime
            .reload_resources()
            .await
            .expect("an availability-only reload succeeds");
        assert_eq!(
            committed.capability_revision, revision_before,
            "an availability-only change never fabricates a revision"
        );

        let events = receive_until(&subscription, |event| {
            matches!(
                event.event,
                RuntimeClientEvent::ResourceGenerationUpdated { .. }
            )
        })
        .await;
        let Some(RuntimeClientEvent::ResourceGenerationUpdated { capabilities, .. }) =
            events.last().map(|event| &event.event)
        else {
            panic!("the capability update event is published: {events:?}");
        };
        assert_eq!(
            capabilities.revision, revision_before,
            "the event reports the unchanged executable revision"
        );
        assert!(
            capabilities.sources.iter().any(|source| {
                matches!(
                    source,
                    crate::runtime_client::snapshot::CapabilitySourceView {
                        source: crate::runtime_client::snapshot::CapabilitySourceDescriptor::Mcp { server_id, .. },
                        state: crate::runtime_client::snapshot::CapabilitySourceStateView::Unavailable { .. },
                    } if server_id.as_str() == "python:broken-tool"
                )
            }),
            "the event carries the typed unavailable state: {:?}",
            capabilities.sources
        );
        // The folded snapshot agrees with the event stream.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.capabilities.revision, revision_before);
        assert_eq!(snapshot.capabilities.sources, capabilities.sources);
    }

    /// The runtime resource generation is a client-visible fact of its own:
    /// the project instruction files the runtime actually loaded travel in
    /// the snapshot, and a reload that discovers a new one publishes a
    /// `ResourceGenerationUpdated` event carrying it.
    ///
    /// This is deliberately not folded into the capability view. The reload
    /// below changes no executable capability at all — it adds an
    /// `AGENTS.md` — so a client that read only the capability half of the
    /// event would still believe no project instructions were loaded.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resource_reload_publishes_the_loaded_project_context_files() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (initial, cursor) = fixture.host.snapshot().expect("snapshot");
        assert!(
            initial.resources.context_files.is_empty(),
            "the fixture starts with no project instructions"
        );
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("subscribe from the snapshot cursor");

        let workspace = fixture
            .runtime
            .tool_runtime()
            .workspace()
            .root()
            .to_path_buf();
        let instructions = workspace.join("AGENTS.md");
        std::fs::write(&instructions, "project authority").expect("write AGENTS.md");
        let committed = fixture
            .runtime
            .reload_resources()
            .await
            .expect("the reload succeeds");

        let events = receive_until(&subscription, |event| {
            matches!(
                event.event,
                RuntimeClientEvent::ResourceGenerationUpdated { .. }
            )
        })
        .await;
        let Some(RuntimeClientEvent::ResourceGenerationUpdated { resources, .. }) =
            events.last().map(|event| &event.event)
        else {
            panic!("the resource update event is published: {events:?}");
        };
        assert_eq!(resources.revision, committed.resource_revision);
        assert!(
            resources
                .context_files
                .iter()
                .any(|file| std::path::Path::new(&file.path) == instructions
                    && file.bytes == "project authority".len() as u64),
            "the published generation names the file it loaded: {:?}",
            resources.context_files
        );

        // The folded snapshot agrees with the event stream.
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(&snapshot.resources, resources);
    }

    /// The exact snapshot/cursor race, interleaving A (snapshot wins): the
    /// snapshot linearizes first and the concurrent transition is observed
    /// by a resume after the snapshot's cursor.
    ///
    /// The runtime admission gate makes the interleaving exact: the parked
    /// snapshot drains the projection before the submit exists, and the
    /// admission commit is released only after the snapshot returned.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_cursor_race_snapshot_wins() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (_, fixture) = host_fixture_probe_with_runtime_gate(
            probe.clone(),
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                settlement_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            },
        )
        .await;
        admission_gate.arm();
        probe.arm_snapshot();
        let snapshot_probe = probe.clone();
        let host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || host.snapshot());
        snapshot_probe.wait_snapshot_entered();

        // The concurrent transition: a submit whose admission is gated
        // until after the snapshot returns.
        let submitting = fixture.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("racing"))
                .expect("accepted")
        });
        submit_task.await.expect("submit task");
        // The admission worker parks at the runtime gate: the transition
        // has not committed.
        admission_gate.wait_entered();

        snapshot_probe.release_snapshot();
        let (snapshot, cursor) = snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        assert!(snapshot.inbound.pending.is_empty());

        // Release the admission: the transition commits and is observed
        // after C. Resume and receive the admission events, never a gap.
        admission_gate.release();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

    /// The exact snapshot/cursor race, interleaving B (publish wins): the
    /// concurrent transition linearizes before the snapshot, so the
    /// snapshot at its cursor already reflects it.
    ///
    /// The runtime admission gate and the projection publish gate make the
    /// interleaving exact: the admission commits while the publish of its
    /// observations is parked, and the snapshot acquires the projection
    /// lock only after the fold completed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_cursor_race_publish_wins() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (_, fixture) = host_fixture_probe_with_runtime_gate(
            probe.clone(),
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                settlement_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            },
        )
        .await;
        // Baseline: an idle host at some cursor C.
        let (before, cursor) = fixture.host.snapshot().expect("snapshot");
        assert!(before.inbound.pending.is_empty());

        // Submit; the admission parks at the runtime gate before committing.
        admission_gate.arm();
        let submitting = fixture.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("racing"))
                .expect("accepted")
        });
        let _accepted = submit_task.await.expect("submit task");
        admission_gate.wait_entered();

        // Fold the enqueue observation (which the submit already pushed)
        // into the projection, so the publish gate below parks only on the
        // admission commit's publications.
        let (folded, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(folded.inbound.pending.len(), 1);

        // Release the admission while the projection publish gate is armed:
        // the fold of the commit observations parks at the gate.
        probe.arm_publish();
        admission_gate.release();
        probe.wait_publish_entered();
        let probe_snapshot = probe.clone();
        let snapshot_host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || snapshot_host.snapshot());
        // Release the publication; the snapshot then acquires the lock and
        // drains everything the commit published.
        probe_snapshot.release_publish();
        let (after_snapshot, after_cursor) = snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        assert!(after_cursor > cursor, "the transition advanced the cursor");
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
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
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted");
        };
        await_background_started(&mut started, "runner started").await;

        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

        // Settlement: the executor raced past the cancellation request and
        // proved success. Cancellation intent owns the request fact and its
        // reason, never the physical outcome (Issue #202), so the execution
        // settles as `Succeeded`, not `Cancelled`.
        release.send_replace(true);
        let terminal = await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "background execution settlement after a racing cancel",
        )
        .await;
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.background[0].state, BackgroundLifecycle::Succeeded);
        assert!(snapshot.background[0].result.is_some());
    }

    /// Detached background work stays visible after the originating
    /// attempt terminates.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_survives_attempt_termination() {
        let (_, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
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
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted");
        };
        await_background_started(&mut started, "runner started").await;

        // Run one attempt to completion.
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
        release.send_replace(true);
        let terminal = await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "detached background execution",
        )
        .await;
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    }

    /// Agent Status is admitted from the exact same composition the model
    /// path consumes: the client event's rendered text equals the canonical
    /// Runtime context fact sent in the model request.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn agent_status_projection_shares_one_composition() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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
        let status_index = events
            .iter()
            .position(|event| matches!(event.event, RuntimeClientEvent::AgentStatusComposed { .. }))
            .expect("status event index");
        let committed_index = events
            .iter()
            .position(|event| {
                matches!(
                    &event.event,
                    RuntimeClientEvent::MessageCommitted { message, .. }
                        if matches!(
                            message,
                            MessageBlock::User(user)
                                if matches!(
                                    &user.kind,
                                    crate::message::types::InboundKind::Context(
                                        crate::message::types::ContextKind::AgentStatus(_)
                                    )
                                )
                        )
                )
            })
            .expect("canonical status commit event");
        assert!(
            committed_index < status_index,
            "canonical Agent Status publication precedes structured observation"
        );
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
            .messages
            .iter()
            .find_map(|message| match message.as_canonical() {
                Some(MessageBlock::User(user))
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    ) =>
                {
                    user.content.first().and_then(|content| match content {
                        crate::message::types::UserContentBlock::Text(text) => {
                            Some(text.text.clone())
                        }
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("model path carries canonical Agent Status");
        assert_eq!(
            status_event.rendered, model_rendered,
            "the client view derives from the same composition as the model path"
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        let committed_status = snapshot
            .messages
            .iter()
            .find_map(|message| match message {
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    ) =>
                {
                    Some(user)
                }
                _ => None,
            })
            .expect("canonical status message");
        assert_eq!(status_event.status_message_id, committed_status.id);
        assert!(
            status_event.opportunities.fresh_inbound.is_some(),
            "the current FreshInbound-only producer still populates its opportunity"
        );
        assert!(snapshot.messages.iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user)
                    if user.id
                        == status_event
                            .opportunities
                            .fresh_inbound
                            .as_ref()
                            .expect("FreshInbound is populated by the current producer")
                            .target_message_id
            )
        }));
        assert!(matches!(
            status_event.sections.first(),
            Some(crate::runtime_client::snapshot::RuntimeClientStatusSection::Temporal { .. })
        ));
        assert_eq!(
            snapshot
                .statuses
                .last()
                .expect("status view")
                .rendered
                .clone(),
            model_rendered
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resource_reload_dispatches_through_the_async_runtime_client_control() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");

        let response = attachment
            .handle_request_async(RuntimeClientRequest::ReloadResources {
                id: crate::runtime_client::RequestId::new(1),
            })
            .await;
        assert!(response.error.is_none());
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::ResourcesReloaded {
                resource_revision: 2,
                capability_revision,
            }) if capability_revision == fixture.coordinator.current_snapshot().revision()
        ));
        assert!(
            fixture
                .runtime
                .coordinator_ledger()
                .expect("idle canonical history")
                .is_empty(),
            "reload creates no conversation history"
        );
    }

    /// Shutdown is distinct from detach: it drains the current attempt to
    /// quiescence, and detach remains available afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one complete shutdown lifecycle
    async fn shutdown_is_not_detach_and_reaches_quiescence() {
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
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
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

        release_tx
            .send(true)
            .expect("release current model request");
        let response = attachment
            .handle_request_async(RuntimeClientRequest::Shutdown {
                id: crate::runtime_client::RequestId::new(2),
            })
            .await;
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::ShutdownCompleted)
        ));

        let (after_shutdown, _) = fixture.host.snapshot().expect("snapshot after shutdown");
        assert!(after_shutdown.shutting_down);
        let first_shutdown_events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::RuntimeShutdown)
        })
        .await;
        assert_eq!(
            first_shutdown_events
                .iter()
                .filter(|event| matches!(event.event, RuntimeClientEvent::RuntimeShutdown))
                .count(),
            1
        );
        let repeated = attachment
            .handle_request_async(RuntimeClientRequest::Shutdown {
                id: crate::runtime_client::RequestId::new(5),
            })
            .await;
        assert!(matches!(
            repeated.result,
            Some(RuntimeClientResult::ShutdownCompleted)
        ));
        let mut duplicate_shutdown = false;
        loop {
            match subscription.try_next() {
                EventDelivery::Event(event) => {
                    duplicate_shutdown |=
                        matches!(event.event, RuntimeClientEvent::RuntimeShutdown);
                }
                EventDelivery::Pending => break,
                delivery => panic!("subscription remains open after repeat: {delivery:?}"),
            }
        }
        assert!(
            !duplicate_shutdown,
            "repeated shutdown publishes no duplicate fact"
        );
        let snapshot_response = attachment.handle_request(RuntimeClientRequest::SnapshotGet {
            id: crate::runtime_client::RequestId::new(4),
        });
        let Some(RuntimeClientResult::Snapshot { snapshot, .. }) = snapshot_response.result else {
            panic!("snapshot_get returns the shutdown state: {snapshot_response:?}");
        };
        assert!(snapshot.shutting_down);

        // Further admission fails explicitly.
        let submit = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(3),
            content: submit_content("too late"),
        });
        assert!(matches!(
            submit.error,
            Some(RuntimeClientError::RuntimeShutdown)
        ));

        // The current attempt settled before shutdown completed; detach still
        // works independently of runtime lifetime.
        attachment.detach();
        let (reattached, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach after shutdown still works");
        let RuntimeClientResult::Initialized { snapshot, .. } = initialized else {
            panic!("fresh initialize returns a snapshot");
        };
        assert!(snapshot.shutting_down);
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

    /// One human inbound through the Runtime Client and one Runtime/Agent
    /// inbound through the native publisher reach the same coordinator
    /// admission path: one finite batch, mailbox order preserved, exactly
    /// one attempt (Test 3).
    ///
    /// The admission gate makes the interleaving exact: the async enqueue
    /// starts an admission that parks before the coordinator lock, so the
    /// human submit provably lands in the mailbox before the finite drain.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn human_and_runtime_inbound_share_one_admission_path() {
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (adapter, fixture) = host_fixture_probe_with_runtime_gate(
            Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default()),
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                settlement_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            },
        )
        .await;
        admission_gate.arm();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        // The native Runtime producer publishes first (a background-style
        // terminal notification), waking the admission worker; the worker
        // parks at the runtime gate before the coordinator lock, so the
        // human submit below provably lands before the finite drain.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("conv-host-async-1", "runtime"))
            .expect("runtime enqueue");
        admission_gate.wait_entered();
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("human"),
        });

        // Release: one admission drains both messages in mailbox order.
        admission_gate.release();
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;

        // Exactly one attempt observed both messages in mailbox order.
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1, "one admission, one attempt");
        let inbound_ids: Vec<&str> = requests[0]
            .messages
            .iter()
            .filter_map(|message| match message.as_canonical() {
                Some(MessageBlock::User(user))
                    if user.kind == crate::message::types::InboundKind::Message =>
                {
                    Some(user.id.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            inbound_ids,
            vec!["conv-host-async-1", "conv-host-inbound-2"],
            "both producers sequence through one durable sequence domain in order"
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(
            snapshot.inbound.pending.is_empty(),
            "the batch was consumed exactly once"
        );
    }

    /// An enqueue racing attempt settlement loses nothing and creates at
    /// most one next attempt (Test 6).
    ///
    /// The settlement gate parks `finish_attempt` after the conversation
    /// state is restored and the current-attempt slot is cleared, before
    /// the next-admission handoff; the enqueue during that park provably
    /// races the settlement boundary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn enqueue_racing_settlement_admits_exactly_one_next_attempt() {
        let settlement_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (adapter, fixture) = host_fixture_with_runtime_probe(
            vec![one_turn_stop(), one_turn_stop()],
            CoordinatorProbe {
                admission_gate: None,
                settlement_gate: Some(settlement_gate.clone()),
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            },
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");

        // Run the first attempt; its settlement handoff parks at the gate
        // after the conversation restore and the slot clear.
        settlement_gate.arm();
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        settlement_gate.wait_entered();

        // An ordinary async enqueue lands while the settlement handoff is
        // parked (the gate holds the coordinator lock after the
        // conversation restore and the slot clear, so the test never reads
        // coordinator state while it is parked).
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("conv-host-async-2", "racing settlement"))
            .expect("async enqueue");

        // Release the handoff: exactly one next attempt consumes the
        // inbound; the settlement path never consumes it again. Waiting on
        // the request-history transfer makes both settlements provable
        // before the assertions below.
        settlement_gate.release();
        await_request_history_len(&fixture.host, 2).await;
        await_adapter_request_count(&adapter, 2).await;
        let requests = adapter.requests();
        assert_eq!(
            requests.len(),
            2,
            "exactly one next attempt (never two, never zero)"
        );
        assert!(
            requests[1].messages.iter().any(|message| matches!(
                message.as_canonical(),
                Some(MessageBlock::User(user)) if user.id.as_str() == "conv-host-async-2"
            )),
            "the racing inbound was consumed exactly once by the next attempt"
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert!(
            snapshot.inbound.pending.is_empty(),
            "no second consumption of the same batch"
        );
    }

    /// The safe-boundary tool-batch invariant (Test 8): with sibling tool
    /// calls A and B, an async inbound arriving while A executes is never
    /// interleaved between the tool results — the full sibling structural
    /// settlement lands before the inbound enters model-visible context.
    #[allow(clippy::too_many_lines)] // one complete tool-batch lifecycle
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn safe_boundary_keeps_sibling_tool_results_together() {
        use crate::runtime::identity::ToolCallId;
        let (tool_a, mut a_started, a_release) = ParkingBackgroundTool::new();
        let (tool_b, mut b_started, b_release) = ParkingBackgroundTool::new();
        let definition_a = ToolDefinition {
            id: ToolId::new("tool-a"),
            name: "a".to_owned(),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Parallel,
            ..tool_a.definition.clone()
        };
        let definition_b = ToolDefinition {
            id: ToolId::new("tool-b"),
            name: "b".to_owned(),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Parallel,
            ..tool_b.definition.clone()
        };
        let mut tools = ToolRegistry::new();
        tools
            .register(definition_a.clone(), Arc::new(tool_a))
            .expect("register a");
        tools
            .register(definition_b.clone(), Arc::new(tool_b))
            .expect("register b");
        let call_a = ToolCallId::new("call-a");
        let call_b = ToolCallId::new("call-b");
        let script = vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call_a.clone(),
                    tool_id: definition_a.id.clone(),
                    name: definition_a.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call_a.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: call_a.clone(),
                    tool_id: definition_a.id.clone(),
                    name: definition_a.name.clone(),
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(1),
                call: crate::tools::types::ToolCallStart {
                    id: call_b.clone(),
                    tool_id: definition_b.id.clone(),
                    name: definition_b.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(1),
                call_id: call_b.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(1),
                call: crate::tools::types::ToolCall {
                    id: call_b.clone(),
                    tool_id: definition_b.id.clone(),
                    name: definition_b.name.clone(),
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        let (adapter, fixture) =
            host_fixture(vec![script, one_turn_stop()], tools, status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        // Both sibling tool calls start (the loop executes the batch); A
        // parks.
        await_background_started(&mut a_started, "tool A started").await;
        await_background_started(&mut b_started, "tool B started").await;

        // An async inbound arrives while the sibling batch is in flight.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("conv-host-async-2", "during batch"))
            .expect("async enqueue");

        // Release both tools; the batch settles structurally, the safe
        // boundary drains the inbound into the next turn.
        a_release.send_replace(true);
        b_release.send_replace(true);
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let requests = adapter.requests();
        assert_eq!(requests.len(), 2, "tool turn + drained-inbound turn");
        // The second request replays the whole conversation; the model-
        // visible tail must be ToolResult A, ToolResult B, then the drained
        // inbound — the sibling structural settlement always lands before
        // the inbound enters model-visible context.
        let roles: Vec<&str> = requests[1]
            .messages
            .iter()
            .filter_map(|message| match message.as_canonical() {
                Some(MessageBlock::Tool(_)) => Some("tool"),
                Some(MessageBlock::User(user))
                    if user.kind == crate::message::types::InboundKind::Message =>
                {
                    Some("user")
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            &roles[roles.len() - 3..],
            &["tool", "tool", "user"],
            "ToolResult A, ToolResult B, then the inbound — never interleaved"
        );
        assert!(
            requests[1].messages.iter().any(|message| matches!(
                message.as_canonical(),
                Some(MessageBlock::User(user)) if user.id.as_str() == "conv-host-async-2"
            )),
            "the drained inbound is the async one"
        );
    }

    /// Model configuration freezes at the admission boundary (Test 10):
    /// an update that linearizes before admission is observed by the
    /// admitted attempt; one that linearizes after admission affects only
    /// future attempts.
    #[allow(clippy::too_many_lines)] // two full freeze interleavings
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn model_update_freezes_at_admission() {
        // Interleaving A: the update linearizes before the admission.
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (adapter_a, fixture_a) = host_fixture_with_runtime_probe(
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                settlement_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            },
        )
        .await;
        let (attachment_a, _) = fixture_a
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription_a = attachment_a
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        admission_gate.arm();
        let submitting = fixture_a.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("first"))
                .expect("accepted")
        });
        let _ = submit_task.await.expect("submit task");
        admission_gate.wait_entered();

        // The model update linearizes while the admission is gated.
        let mut updated = fixture_a.runtime.model_config();
        updated.request_params.insert(
            "frozen_probe".to_owned(),
            serde_json::json!("updated-before-admission"),
        );
        fixture_a
            .host
            .model_set(updated)
            .expect("model update accepted");

        admission_gate.release();
        receive_until(&subscription_a, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let requests = adapter_a.requests();
        assert_eq!(
            requests[0].request_params().get("frozen_probe"),
            Some(&serde_json::json!("updated-before-admission")),
            "the admitted attempt observes the pre-admission update"
        );

        // Interleaving B: the update linearizes after the admission.
        let (release_b_tx, release_b_rx) = model_release();
        let (adapter_b, fixture_b) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_b_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment_b, _) = fixture_b
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription_b = attachment_b
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment_b.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        // The attempt is provably admitted (its model stream is parked).
        receive_until(&subscription_b, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
        })
        .await;

        // The update lands mid-attempt: the running attempt keeps its
        // frozen snapshot.
        let mut updated = fixture_b.runtime.model_config();
        updated.request_params.insert(
            "frozen_probe".to_owned(),
            serde_json::json!("updated-after-admission"),
        );
        fixture_b
            .host
            .model_set(updated)
            .expect("model update accepted");

        release_b_tx.send(true).expect("release");
        receive_until(&subscription_b, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let requests = adapter_b.requests();
        assert!(
            requests[0].request_params().get("frozen_probe").is_none(),
            "the admitted attempt never observes the post-admission update"
        );

        // A later attempt observes it.
        attachment_b.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        receive_until(&subscription_b, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        let requests = adapter_b.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].request_params().get("frozen_probe"),
            Some(&serde_json::json!("updated-after-admission")),
            "a future attempt observes the update"
        );
    }

    /// Capability revision immutability (Test 11): an active attempt's
    /// lease pins the capability revision, so the coordinator rejects a
    /// mid-attempt commit (`Busy`); after settlement the same commit
    /// succeeds and the admitted attempt's request facts still carry the
    /// revision it was admitted with.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn capability_revision_is_frozen_at_admission() {
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
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        let revision_at_admission = fixture.runtime.capability().current_snapshot().revision();

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        // The attempt is provably admitted (its model stream is parked).
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
        })
        .await;

        // A resource reload lands mid-attempt: the runtime rejects it
        // deterministically — the attempt's lease pins the revision.
        write_probe_skill(
            fixture.runtime.tool_runtime().workspace().root(),
            "mid-attempt-skill",
        );
        let rejected = fixture.runtime.reload_resources().await;
        assert!(
            matches!(
                rejected,
                Err(crate::runtime::RuntimeResourceReloadError::Busy {
                    reason: crate::runtime::RuntimeResourceReloadBusyReason::Attempt
                })
            ),
            "an active attempt lease blocks capability mutation"
        );

        // Release: the attempt settles normally with its frozen lease.
        release_tx.send(true).expect("release");
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        await_request_history_len(&fixture.host, 1).await;

        // After settlement the same reload succeeds; the admitted attempt's
        // request facts still carry the pre-commit revision, and the
        // projection observes the post-commit revision.
        let committed = fixture
            .runtime
            .reload_resources()
            .await
            .expect("reload after settlement");
        assert!(committed.capability_revision > revision_at_admission);
        let history = fixture.host.request_history();
        assert_eq!(
            request_snapshots(&history)[0].capability_revision,
            revision_at_admission,
            "the later capability change never retroactively mutates the admitted attempt"
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.capabilities.revision,
            committed.capability_revision
        );
    }

    /// Builds a host with the projection linearization probe installed.
    async fn host_fixture_probe(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        host_fixture_probe_with_native_tools(probe, scripts, false).await
    }

    /// Probe fixture variant with the real native tool plane activated.
    async fn host_fixture_probe_with_native_tools(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
        include_native_tools: bool,
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
        let mut tools = ToolRegistry::new();
        if include_native_tools {
            crate::tools::register_native_tools(
                &mut tools,
                crate::tools::NativeToolResources {
                    subagent_catalog: crate::runtime::subagent::SubagentCatalog::empty(),
                    background: tool_runtime.background().clone(),
                    subagents: None,
                },
                crate::tools::NativeToolPolicies::default(),
            )
            .expect("register native tools");
        }
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(tools),
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace.join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: crate::scripted_suites::support::model::scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: status_engine(),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        })
        .expect("conversation runtime");
        let host = RuntimeClientHost::with_probe(
            RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
            (*probe).clone(),
        )
        .expect("host");
        runtime.activate();
        (
            adapter,
            HostFixture {
                _dir: dir,
                host,
                runtime,
                coordinator,
            },
        )
    }

    /// Builds a host whose runtime carries both the projection probe and
    /// the coordinator synchronization hooks.
    async fn host_fixture_probe_with_runtime_gate(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
        runtime_probe: CoordinatorProbe,
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
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace.join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let runtime = ConversationRuntime::with_probe(
            RuntimeConversationConfig {
                agent_id: AgentId::new("agent-a"),
                model: crate::scripted_suites::support::model::scripted_session_model(
                    adapter.clone(),
                ),
                approval_mode: crate::runtime::ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    status_engine: status_engine(),
                },
                tool_runtime,
                resources: test_resources(&coordinator),
                resource_loader: test_resource_loader(&coordinator),
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: None,
            },
            runtime_probe,
        )
        .expect("conversation runtime with probe");
        let host = RuntimeClientHost::with_probe(
            RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
            (*probe).clone(),
        )
        .expect("host");
        runtime.activate();
        (
            adapter,
            HostFixture {
                _dir: dir,
                host,
                runtime,
                coordinator,
            },
        )
    }

    /// A fixture over one conversation runtime **without** a Runtime
    /// Client host, so a test controls host construction itself (the
    /// Issue #61 bootstrap regressions).
    struct RuntimeOnlyFixture {
        _dir: tempfile::TempDir,
        runtime: ConversationRuntime,
        coordinator: crate::capabilities::CapabilityCoordinator,
        workspace: std::path::PathBuf,
    }

    /// Builds the conversation runtime alone (no host), with the given
    /// scripts, tool registry, and optional coordinator probe.
    async fn runtime_only_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        probe: Option<CoordinatorProbe>,
    ) -> (Arc<GatedAdapter>, RuntimeOnlyFixture) {
        runtime_only_fixture_with_conversation_id(
            scripts,
            tools,
            probe,
            ConversationId::new("conv-host"),
        )
        .await
    }

    /// Builds the same runtime-only fixture with an explicit conversation
    /// identity. Session-aware tests use the catalog's deterministic initial
    /// identity instead of coupling the runtime to the old fixture label.
    async fn runtime_only_fixture_with_conversation_id(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        probe: Option<CoordinatorProbe>,
        conversation_id: ConversationId,
    ) -> (Arc<GatedAdapter>, RuntimeOnlyFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
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
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace.join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let config = RuntimeConversationConfig {
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: status_engine(),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        };
        let runtime = match probe {
            Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
            None => ConversationRuntime::new(config).expect("runtime"),
        };
        (
            adapter,
            RuntimeOnlyFixture {
                _dir: dir,
                runtime,
                coordinator,
                workspace,
            },
        )
    }

    /// Builds the real Runtime Client/session-owner boundary over a scripted
    /// runtime. The catalog is deliberately separate from the fixture's
    /// private conversation store: this test exercises the ownership seam,
    /// while the supervisor still gets the same `ConversationId` identity.
    async fn local_session_endpoint(
        scripts: Vec<Vec<GatedStep>>,
        probe: Option<CoordinatorProbe>,
    ) -> (
        Arc<GatedAdapter>,
        RuntimeOnlyFixture,
        RuntimeClientEndpoint,
        Arc<LocalSessionSupervisor>,
        tempfile::TempDir,
        CurrentRuntimeConfig,
    ) {
        let (adapter, fixture) = runtime_only_fixture_with_conversation_id(
            scripts,
            ToolRegistry::new(),
            probe,
            ConversationId::new("conversation-1"),
        )
        .await;
        let catalog_root = tempfile::tempdir().expect("catalog root");
        let config = CurrentRuntimeConfig::from_jsonc_slice(
            br#"{
              "agentId": "agent-a",
              "model": {"model": "scripted/scripted"},
              "context": {"reserveTokens": 0, "keepRecentTokens": 0}
            }"#,
        )
        .expect("current runtime config");
        let catalog = SessionCatalog::create(
            catalog_root.path(),
            &SessionPersistentState {
                model: config.model.clone(),
            },
        )
        .expect("catalog");
        let supervisor = Arc::new(LocalSessionSupervisor::new(catalog, config.model.clone()));
        let host = RuntimeClientHost::new_with_session_control(
            RuntimeClientHostConfig {
                runtime: fixture.runtime.clone(),
                replay_limit: None,
            },
            supervisor.clone(),
        )
        .expect("session-aware host");
        fixture.runtime.activate();
        supervisor
            .install_runtime(fixture.runtime.clone())
            .await
            .expect("install runtime");
        (
            adapter,
            fixture,
            RuntimeClientEndpoint::new(host),
            supervisor,
            catalog_root,
            config,
        )
    }

    fn initialize_endpoint(endpoint: &RuntimeClientEndpoint) {
        let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        });
        assert!(response.error.is_none(), "initialize failed: {response:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn catalog_publication_outcomes_fence_the_runtime_client_typed() {
        for post_rename in [false, true] {
            let (_adapter, _fixture, endpoint, supervisor, catalog_root, _config) =
                local_session_endpoint(Vec::new(), None).await;
            initialize_endpoint(&endpoint);
            let initial = endpoint
                .handle_request_async(RuntimeClientRequest::SessionGet {
                    id: crate::runtime_client::RequestId::new(10),
                })
                .await;
            let Some(RuntimeClientResult::Session { session: initial }) = initial.result else {
                panic!("initial metadata must be readable: {initial:?}");
            };
            // `/new` is a semantic no-op over an untouched empty shell, so
            // the transition this test fences needs the root Session to own
            // durable user work first. Durable Pending Inbound acceptance in
            // the catalog-owned store is exactly that boundary.
            accept_catalog_pending_inbound(catalog_root.path(), "root work");
            if post_rename {
                supervisor.arm_catalog_write_fault_after_rename().await;
            } else {
                supervisor.arm_catalog_write_fault_before_rename().await;
            }

            let response = endpoint
                .handle_request_async(RuntimeClientRequest::SessionNew {
                    id: crate::runtime_client::RequestId::new(2),
                })
                .await;
            if post_rename {
                assert!(matches!(
                    response.result,
                    Some(RuntimeClientResult::SessionCommittedRestartRequired {
                        editor_content: None,
                        ..
                    })
                ));
                assert!(
                    response.error.is_none(),
                    "committed transition is typed success"
                );
            } else {
                assert!(
                    matches!(
                        response.error,
                        Some(RuntimeClientError::SessionRestartRequired { .. })
                    ),
                    "unexpected pre-commit transition response: {response:?}"
                );
            }

            // A repeated replacement request is fenced by the absorbing
            // supervisor state, rather than being treated as another normal
            // Session failure.
            let duplicate = endpoint
                .handle_request_async(RuntimeClientRequest::SessionNew {
                    id: crate::runtime_client::RequestId::new(3),
                })
                .await;
            assert!(matches!(
                duplicate.error,
                Some(RuntimeClientError::SessionRestartRequired { .. })
            ));

            let stale_snapshot = endpoint.handle_request(RuntimeClientRequest::SnapshotGet {
                id: crate::runtime_client::RequestId::new(4),
            });
            assert!(matches!(
                stale_snapshot.error,
                Some(RuntimeClientError::SessionRestartRequired { .. })
            ));

            let same_node = endpoint
                .handle_request_async(RuntimeClientRequest::SessionSelect {
                    id: crate::runtime_client::RequestId::new(11),
                    session_id: initial.id,
                    node_id: Some(initial.active_node),
                })
                .await;
            assert!(matches!(
                same_node.error,
                Some(RuntimeClientError::SessionRestartRequired { .. })
            ));

            let list = endpoint
                .handle_request_async(RuntimeClientRequest::SessionList {
                    id: crate::runtime_client::RequestId::new(5),
                    query: None,
                    offset: 0,
                    limit: crate::local_runtime::SESSION_LIST_PAGE_LIMIT,
                })
                .await;
            let Some(RuntimeClientResult::SessionList { sessions, .. }) = list.result else {
                panic!("bounded metadata remains readable: {list:?}");
            };
            assert_eq!(
                sessions.len(),
                1,
                "the used source is the only resume-visible Session either way"
            );

            if post_rename {
                let reopened = SessionCatalog::open_existing(catalog_root.path())
                    .expect("open visible catalog")
                    .expect("reopen visible catalog");
                assert_eq!(
                    reopened.persisted_session_ids().len(),
                    2,
                    "post-rename failure still leaves the new catalog visible"
                );
                assert_eq!(
                    reopened
                        .list_page(None, 0, crate::local_runtime::SESSION_LIST_PAGE_LIMIT)
                        .expect("reopened page")
                        .sessions
                        .len(),
                    1,
                    "the published empty shell is durable, not resume history"
                );
            }
        }
    }

    /// `/new` asks for an empty Session, and an untouched empty active
    /// Session already is one. The command is then a semantic no-op — same
    /// Session, same node, same conversation, byte-identical catalog, live
    /// runtime — while the same command over a used Session remains a real
    /// switch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)]
    async fn session_new_over_an_unused_session_is_a_semantic_noop() {
        let (_adapter, _fixture, endpoint, _supervisor, catalog_root, _config) =
            local_session_endpoint(Vec::new(), None).await;
        initialize_endpoint(&endpoint);
        let initial = endpoint
            .handle_request_async(RuntimeClientRequest::SessionGet {
                id: crate::runtime_client::RequestId::new(10),
            })
            .await;
        let Some(RuntimeClientResult::Session { session: initial }) = initial.result else {
            panic!("initial metadata must be readable: {initial:?}");
        };
        let catalog_path = catalog_root.path().join("sessions").join("catalog.json");
        let catalog_before = std::fs::read(&catalog_path).expect("catalog before /new");

        let noop = endpoint
            .handle_request_async(RuntimeClientRequest::SessionNew {
                id: crate::runtime_client::RequestId::new(2),
            })
            .await;
        let Some(RuntimeClientResult::SessionChanged {
            session,
            editor_content,
            restart_required,
        }) = noop.result
        else {
            panic!("session_new over an unused Session must succeed: {noop:?}");
        };
        assert!(noop.error.is_none());
        assert!(
            !restart_required,
            "reusing the empty shell replaces no runtime"
        );
        assert_eq!(editor_content, None);
        assert_eq!(session.id, initial.id, "no new SessionId");
        assert_eq!(
            session.active_node, initial.active_node,
            "no new SessionNodeId"
        );
        assert_eq!(
            session.active_conversation_id, initial.active_conversation_id,
            "no new ConversationId"
        );
        assert_eq!(
            std::fs::read(&catalog_path).expect("catalog after /new"),
            catalog_before,
            "the no-op publishes no catalog row and moves no ordinal"
        );

        // The runtime was not quiesced to exchange one empty shell for
        // another: a repeated `/new` no-ops again instead of hitting the
        // absorbing replacement fence, and ordinary submission through the
        // same live runtime still works.
        let repeated = endpoint
            .handle_request_async(RuntimeClientRequest::SessionNew {
                id: crate::runtime_client::RequestId::new(3),
            })
            .await;
        assert!(
            matches!(
                repeated.result,
                Some(RuntimeClientResult::SessionChanged {
                    restart_required: false,
                    ..
                })
            ),
            "a repeated /new over the still-unused Session no-ops: {repeated:?}"
        );
        let submitted = endpoint
            .handle_request_async(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(4),
                content: submit_content("real work"),
            })
            .await;
        assert!(
            matches!(
                submitted.result,
                Some(RuntimeClientResult::InboundAccepted { .. })
            ),
            "the same live runtime still accepts work: {submitted:?}"
        );

        // Durable Pending Inbound acceptance — in the catalog-owned store,
        // the classification authority — made the Session used: `/resume`
        // lists it immediately, and `/new` is a real switch again.
        accept_catalog_pending_inbound(catalog_root.path(), "real work");
        let list = endpoint
            .handle_request_async(RuntimeClientRequest::SessionList {
                id: crate::runtime_client::RequestId::new(5),
                query: None,
                offset: 0,
                limit: crate::local_runtime::SESSION_LIST_PAGE_LIMIT,
            })
            .await;
        let Some(RuntimeClientResult::SessionList { sessions, .. }) = list.result else {
            panic!("the used Session is resume-visible: {list:?}");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, initial.id);

        let switched = endpoint
            .handle_request_async(RuntimeClientRequest::SessionNew {
                id: crate::runtime_client::RequestId::new(6),
            })
            .await;
        let Some(RuntimeClientResult::SessionChanged {
            session: new_session,
            restart_required,
            ..
        }) = switched.result
        else {
            panic!("session_new over a used Session must switch: {switched:?}");
        };
        assert!(restart_required);
        assert_ne!(new_session.id, initial.id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fork_pre_rename_failure_is_terminal_without_committed_draft() {
        let prompt = "fork-draft-exact-pre-7f3b";
        let (adapter, fixture, endpoint, supervisor, catalog_root, _config) =
            local_session_endpoint(vec![one_turn_stop()], None).await;
        initialize_endpoint(&endpoint);
        let (revision, message_id, source_messages) =
            await_text_boundary(&fixture.runtime, &adapter, prompt).await;
        // The runtime-side turn lives in the fixture store; the catalog's
        // own store is the classification authority, so the source Session
        // owns durable user work there too.
        accept_catalog_pending_inbound(catalog_root.path(), "source work");
        let source = SessionCatalog::open_existing(catalog_root.path())
            .expect("open source catalog")
            .expect("source catalog")
            .active_snapshot()
            .expect("source snapshot");

        supervisor.arm_catalog_write_fault_before_rename().await;
        let response = endpoint
            .handle_request_async(RuntimeClientRequest::SessionFork {
                id: crate::runtime_client::RequestId::new(20),
                surface_revision: revision,
                message_id,
            })
            .await;
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::SessionRestartRequired { .. })
        ));
        assert!(
            response.result.is_none(),
            "pre-commit failure has no transition result"
        );

        let reopened = SessionCatalog::open_existing(catalog_root.path())
            .expect("open source")
            .expect("reopen source");
        assert_eq!(
            reopened
                .active_snapshot()
                .expect("active source after failure")
                .id,
            source.id
        );
        assert_eq!(
            reopened
                .list_page(None, 0, crate::local_runtime::SESSION_LIST_PAGE_LIMIT)
                .expect("source page")
                .sessions
                .len(),
            1,
            "the prepared destination is not catalog-visible before rename"
        );
        assert_eq!(
            fixture
                .runtime
                .historical_head_snapshot()
                .expect("source history after failed fork")
                .1,
            source_messages,
            "source lineage remains unchanged"
        );

        let duplicate = endpoint
            .handle_request_async(RuntimeClientRequest::SessionFork {
                id: crate::runtime_client::RequestId::new(21),
                surface_revision: revision,
                message_id: MessageId::new(prompt),
            })
            .await;
        assert!(matches!(
            duplicate.error,
            Some(RuntimeClientError::SessionRestartRequired { .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fork_post_rename_failure_carries_exact_uncommitted_editor_payload() {
        let prompt = "fork-draft-exact-7f3b";
        let (adapter, fixture, endpoint, supervisor, catalog_root, _config) =
            local_session_endpoint(vec![one_turn_stop()], None).await;
        initialize_endpoint(&endpoint);
        let (revision, message_id, source_messages) =
            await_text_boundary(&fixture.runtime, &adapter, prompt).await;
        // The runtime-side turn lives in the fixture store; the catalog's
        // own store is the classification authority, so the source Session
        // owns durable user work there too.
        accept_catalog_pending_inbound(catalog_root.path(), "source work");
        let source = SessionCatalog::open_existing(catalog_root.path())
            .expect("open source catalog")
            .expect("source catalog")
            .active_snapshot()
            .expect("source snapshot");

        supervisor.arm_catalog_write_fault_after_rename().await;
        let response = endpoint
            .handle_request_async(RuntimeClientRequest::SessionFork {
                id: crate::runtime_client::RequestId::new(30),
                surface_revision: revision,
                message_id,
            })
            .await;
        let Some(RuntimeClientResult::SessionCommittedRestartRequired {
            session,
            editor_content,
            diagnostic,
        }) = response.result
        else {
            panic!("post-commit fork must carry a typed transition result: {response:?}");
        };
        assert!(response.error.is_none());
        assert!(diagnostic.contains("durability is uncertain"));
        assert_eq!(editor_content, Some(submit_content(prompt)));

        let reopened = SessionCatalog::open_existing(catalog_root.path())
            .expect("open fork")
            .expect("reopen fork");
        let authoritative = reopened.active_snapshot().expect("authoritative fork");
        assert_eq!(authoritative.id.as_str(), session.id);
        assert_ne!(
            authoritative.active_conversation_id,
            source.active_conversation_id
        );
        assert_eq!(
            reopened
                .list_page(None, 0, crate::local_runtime::SESSION_LIST_PAGE_LIMIT)
                .expect("fork page")
                .sessions
                .len(),
            2
        );

        let destination = catalog_conversation(
            catalog_root.path(),
            session.id.as_str(),
            &session.active_conversation_id,
        );
        assert!(!destination.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.content.iter().any(|content| {
                matches!(content, UserContentBlock::Text(text) if text.text == prompt)
            }))
        }));
        assert_eq!(
            fixture
                .runtime
                .historical_head_snapshot()
                .expect("source history after committed fork")
                .1,
            source_messages,
            "source lineage remains unchanged"
        );

        let duplicate = endpoint
            .handle_request_async(RuntimeClientRequest::SessionNew {
                id: crate::runtime_client::RequestId::new(31),
            })
            .await;
        assert!(matches!(
            duplicate.error,
            Some(RuntimeClientError::SessionRestartRequired { .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tree_post_rename_failure_carries_exact_uncommitted_editor_payload() {
        let prompt = "tree-draft-exact-7f3b";
        let (adapter, fixture, endpoint, supervisor, catalog_root, _config) =
            local_session_endpoint(vec![one_turn_stop()], None).await;
        initialize_endpoint(&endpoint);
        let (revision, message_id, source_messages) =
            await_text_boundary(&fixture.runtime, &adapter, prompt).await;
        let source = SessionCatalog::open_existing(catalog_root.path())
            .expect("open source catalog")
            .expect("source catalog")
            .active_snapshot()
            .expect("source snapshot");

        supervisor.arm_catalog_write_fault_after_rename().await;
        let response = endpoint
            .handle_request_async(RuntimeClientRequest::SessionTreeBranch {
                id: crate::runtime_client::RequestId::new(40),
                surface_revision: revision,
                message_id,
            })
            .await;
        let Some(RuntimeClientResult::SessionCommittedRestartRequired {
            session,
            editor_content,
            diagnostic,
        }) = response.result
        else {
            panic!("post-commit tree branch must carry a typed transition result: {response:?}");
        };
        assert!(response.error.is_none());
        assert!(diagnostic.contains("durability is uncertain"));
        assert_eq!(editor_content, Some(submit_content(prompt)));
        assert_eq!(session.id, source.id.as_str());
        assert_ne!(session.active_node, source.active_node.as_str());

        let reopened = SessionCatalog::open_existing(catalog_root.path())
            .expect("open tree")
            .expect("reopen tree");
        let authoritative = reopened.active_snapshot().expect("authoritative tree node");
        assert_eq!(authoritative.id, source.id);
        assert_eq!(authoritative.active_node.as_str(), session.active_node);
        assert_eq!(authoritative.node_count, 2);
        let destination = catalog_conversation(
            catalog_root.path(),
            session.id.as_str(),
            &session.active_conversation_id,
        );
        assert!(!destination.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.content.iter().any(|content| {
                matches!(content, UserContentBlock::Text(text) if text.text == prompt)
            }))
        }));
        assert_eq!(
            fixture
                .runtime
                .historical_head_snapshot()
                .expect("source history after committed tree branch")
                .1,
            source_messages,
            "source node remains unchanged"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn model_catalog_visibility_uncertainty_fences_the_live_runtime_typed() {
        let (_adapter, fixture, endpoint, supervisor, catalog_root, _config) =
            local_session_endpoint(Vec::new(), None).await;
        initialize_endpoint(&endpoint);
        let before = fixture.runtime.model_config();
        let candidate = marked_model_config(&fixture.runtime, "post-rename");
        supervisor.arm_catalog_write_fault_after_rename().await;

        let response = endpoint.handle_request(RuntimeClientRequest::ModelSet {
            id: crate::runtime_client::RequestId::new(2),
            config: Box::new(candidate.clone()),
        });
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::SessionRestartRequired { .. })
        ));
        assert_eq!(
            fixture.runtime.model_config(),
            before,
            "the live runtime candidate is not installed after catalog publication uncertainty"
        );

        let reopened = SessionCatalog::open_existing(catalog_root.path())
            .expect("open catalog")
            .expect("reopen catalog");
        let (_, _, reopened_config) = reopened.active_lineage().expect("active lineage");
        assert_eq!(
            reopened_config.model, candidate,
            "the catalog crossed visibility even though its durability barrier was uncertain"
        );
        let stale = endpoint.handle_request(RuntimeClientRequest::SnapshotGet {
            id: crate::runtime_client::RequestId::new(3),
        });
        assert!(matches!(
            stale.error,
            Some(RuntimeClientError::SessionRestartRequired { .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn session_switch_waits_for_real_attempt_task_quiescence_before_catalog_visibility() {
        let (release_tx, release_rx) = model_release();
        let attempt_exit_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        attempt_exit_gate.arm();
        let (adapter, fixture, _endpoint, supervisor, catalog_root, _config) =
            local_session_endpoint(
                vec![vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::ParkUntilReleased(release_rx),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ]],
                Some(CoordinatorProbe {
                    attempt_exit_gate: Some(attempt_exit_gate.clone()),
                    ..CoordinatorProbe::default()
                }),
            )
            .await;
        // `/new` no-ops over an untouched empty shell, so the catalog's
        // active Session must own durable user work before the switch below
        // is a real transition at all.
        accept_catalog_pending_inbound(catalog_root.path(), "catalog work");
        let before = SessionCatalog::open_existing(catalog_root.path())
            .expect("open catalog before switch")
            .expect("catalog before switch")
            .active_snapshot()
            .expect("active snapshot before switch");

        fixture
            .runtime
            .submit_inbound(submit_content("unsettled turn"))
            .expect("real turn accepted");
        await_adapter_request_count(&adapter, 1).await;

        let switch_supervisor = supervisor.clone();
        let switch = tokio::spawn(async move { switch_supervisor.new_session().await });
        let entered = {
            let gate = attempt_exit_gate.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
        };
        entered.await.expect("attempt exit gate entered");

        let observed = SessionCatalog::open_existing(catalog_root.path())
            .expect("observe catalog while old task is unsettled")
            .expect("catalog while old task is unsettled")
            .active_snapshot()
            .expect("active snapshot while old task is unsettled");
        assert_eq!(observed.id, before.id);
        assert!(!switch.is_finished(), "switch waits for quiescence");

        // The provider is cancellation-aware; releasing the model gate is
        // not what completes the switch. The attempt-exit gate is the exact
        // runtime ownership boundary that shutdown must await.
        let _ = release_tx.send(true);
        let release_exit = {
            let gate = attempt_exit_gate.clone();
            tokio::task::spawn_blocking(move || gate.release())
        };
        release_exit.await.expect("release attempt task exit");
        let result = switch.await.expect("switch task");
        assert!(result.is_ok(), "quiescence precedes successful publication");
        let after = SessionCatalog::open_existing(catalog_root.path())
            .expect("open catalog after switch")
            .expect("reopen catalog after switch")
            .active_snapshot()
            .expect("active snapshot after switch");
        assert_ne!(after.id, before.id);
    }

    /// A marker-bearing alternate session model configuration.
    fn marked_model_config(
        runtime: &ConversationRuntime,
        marker: &str,
    ) -> crate::model::session::SessionModelConfig {
        let mut config = runtime.model_config();
        config
            .request_params
            .insert(marker.to_owned(), serde_json::json!("changed"));
        config
    }

    /// Test B1 — the interactive composition: construct the runtime, bind
    /// the Runtime Client host over the inert runtime, activate, and run a
    /// real turn end to end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn interactive_pre_activation_bind_runs_a_real_turn() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        assert!(
            !fixture.runtime.is_activated(),
            "a freshly constructed runtime is inert"
        );

        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("a host binds before activation");
        fixture.runtime.activate();
        assert!(fixture.runtime.is_activated());

        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (_, cursor) = host.snapshot().expect("snapshot");
        let subscription = attachment.subscribe_events(cursor).expect("subscribe");
        attachment
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(1),
                content: submit_content("drive a turn"),
            })
            .result
            .expect("accepted");
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }))
                .count(),
            1,
            "exactly one terminal settlement"
        );
        assert_eq!(adapter.requests().len(), 1, "the real provider path ran");
    }

    /// Test B2 — the headless composition: construct the runtime, bind no
    /// host at all, activate, and run a real turn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn headless_activation_runs_without_any_client_host() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        fixture.runtime.activate();

        fixture
            .runtime
            .submit_inbound(submit_content("headless"))
            .expect("accepted");
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(adapter.requests().len(), 1, "the real provider path ran");
        assert!(
            !fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "no Runtime Client host ever existed"
        );
    }

    /// Test B3 — a Runtime Client host bind after activation is refused
    /// with the explicit lifecycle error: no panic, no partial bridge, and
    /// the one-time client binding claim is not consumed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn late_host_bind_after_activation_is_rejected_typed() {
        let (adapter, fixture) = runtime_only_fixture(
            vec![one_turn_stop(), one_turn_stop()],
            ToolRegistry::new(),
            None,
        )
        .await;
        fixture.runtime.activate();
        // Semantic execution really started before the bind attempt.
        fixture
            .runtime
            .submit_inbound(submit_content("start executing"))
            .expect("accepted");
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(adapter.requests().len(), 1);

        let rejected = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        });
        match rejected {
            Err(HostConstructionError::RuntimeAlreadyActivated { conversation_id }) => {
                assert_eq!(conversation_id.as_str(), "conv-host");
            }
            _ => panic!("a post-activation host bind must fail typed"),
        }
        // Transactional: the rejected construction consumed no binding
        // claim and installed no bridge.
        assert!(
            !fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "the tool runtime binding claim was not consumed"
        );
        assert!(
            !fixture.runtime.capability().is_runtime_client_bound(),
            "the capability binding claim was not consumed"
        );
        // The runtime keeps executing normally afterwards.
        fixture
            .runtime
            .submit_inbound(submit_content("keep going"))
            .expect("accepted");
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(
            adapter.requests().len(),
            2,
            "the runtime keeps executing after the rejected bind"
        );
    }

    /// Test B4 — attachments stay dynamic after activation: attach, detach
    /// while an attempt is running, and reattach, without ever affecting
    /// semantic execution.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attachments_remain_dynamic_after_activation() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = runtime_only_fixture(
            vec![
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
            None,
        )
        .await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host binds before activation");
        fixture.runtime.activate();

        let (first, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (_, cursor) = host.snapshot().expect("snapshot");
        let subscription = first.subscribe_events(cursor).expect("subscribe");
        first
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(1),
                content: submit_content("first"),
            })
            .result
            .expect("accepted");
        // Wait for a real in-flight streaming fact through the client
        // projection, then detach mid-attempt.
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AssistantTextDelta { .. })
        })
        .await;
        first.detach();

        // The detached attempt still settles canonically.
        release_tx.send(true).expect("release the parked attempt");
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(adapter.requests().len(), 1, "the attempt ran to settlement");

        // Reattach: a fresh attachment over the same host and the same
        // semantic owners, and the projection observed the settlement it
        // was detached for.
        let (second, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("reattach");
        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert!(
            matches!(
                snapshot.attempt.as_ref().map(|attempt| &attempt.phase),
                Some(RuntimeClientAttemptPhase::Settled { .. })
            ),
            "the detach never altered semantic execution"
        );
        let subscription = second.subscribe_events(cursor).expect("subscribe");
        second
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(2),
                content: submit_content("second"),
            })
            .result
            .expect("accepted");
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        assert_eq!(adapter.requests().len(), 2, "the reattached turn ran");
    }

    /// Test C + D + E — no runtime-owned semantic commit can cross the
    /// bootstrap while the runtime is inactive, so cursor 0 is genuinely
    /// stable until `activate()`.
    ///
    /// The host binds over an inert runtime whose tool-runtime background
    /// plane is pristine by the ownership-transfer invariant (construction
    /// requires no prepared dispatch and no committed record, and the
    /// transfer then refuses dispatch commits while the mailbox is bound
    /// inactive): an inbound submit, a background dispatch commit, and a
    /// capability commit are all refused typed and consume nothing; the
    /// snapshot stays at cursor 0 with the startup capability revision
    /// seeded, and a subscription from cursor 0 stays `Pending`. After
    /// `activate()` the first real transition receives cursor 1.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pre_activation_semantic_commits_cannot_cross_the_bootstrap() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;

        // Bind the host over the inert runtime. The startup capability
        // revision (committed before the runtime existed, during
        // composition) is legitimate bootstrap state; nothing else is.
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host binds before activation");

        // Exercise every legal pre-activation operation against the
        // conversation-bound subsystems: each is refused typed and
        // consumes nothing.
        assert!(matches!(
            fixture.runtime.submit_inbound(submit_content("early")),
            Err(InboundAdmissionError::Inactive)
        ));

        // A background dispatch can prepare (that is pure preparation) but
        // its ownership commit is refused: no record, no runner start.
        let (tool, mut started, release, execution_gate) =
            ParkingBackgroundTool::new_with_execution_gate();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let registry = fixture.runtime.tool_runtime().background().clone();
        let prepared = registry
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
            .expect("preparation is allowed before activation");
        let refused = registry.commit_dispatch(prepared, &CancellationSignal::new());
        assert!(
            matches!(
                refused,
                Err(crate::tools::background::BackgroundDispatchError::ConversationInactive { .. })
            ),
            "a background ownership commit before activation is refused typed: {refused:?}"
        );
        assert!(
            registry.all_snapshots().is_empty(),
            "the refused commit published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never began");
        assert!(
            adapter.requests().is_empty(),
            "the rejected pre-activation inbound never starts a model request"
        );

        // A capability commit on the runtime-owned coordinator is refused
        // typed: the active revision stays the startup one.
        write_probe_skill(&fixture.workspace, "pdf");
        let candidate = fixture
            .coordinator
            .prepare_candidate()
            .await
            .expect("prepare is allowed before activation");
        let refused = fixture.coordinator.commit(candidate);
        assert_eq!(
            refused,
            Err(crate::capabilities::CapabilityCommitError::RuntimePublicationRequired),
            "a runtime-owned capability commit cannot bypass resource publication"
        );

        // The bootstrap snapshot is exactly the startup state at cursor 0.
        // (The startup capability commit during composition was a no-op
        // against the empty candidate, so the seeded revision is 0.)
        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert_eq!(
            snapshot.capabilities.revision.get(),
            0,
            "the startup capability revision is seeded"
        );
        assert!(
            snapshot.background.is_empty(),
            "no background record exists at bootstrap"
        );
        assert!(snapshot.attempt.is_none() && snapshot.statuses.is_empty());
        assert_eq!(snapshot.context.compaction_count, 0);

        // A subscription from the bootstrap cursor observes nothing at all
        // until a real post-activation transition happens.
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe from the bootstrap cursor");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "cursor 0 stays Pending until activation"
        );

        // Activation opens every gate at once. The first real transition
        // — the resource/capability publication — receives cursor 1, the next — the
        // background dispatch commit — cursor 2.
        fixture.runtime.activate();
        let activated = fixture
            .runtime
            .reload_resources()
            .await
            .expect("a runtime-owned resource reload succeeds after activation");
        assert_eq!(
            activated.capability_revision.get(),
            1,
            "the first real activation"
        );
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(
                registry
                    .prepare_dispatch(
                        &ToolInvocation {
                            call_id: ToolCallId::new("call-bg-2"),
                            tool_id: ToolId::new("tool-bg"),
                            tool_name: "bg".to_owned(),
                            mode: ToolInvocationMode::Background,
                            arguments: serde_json::json!({}),
                        },
                        &executor,
                        crate::tools::environment::ToolEnvironment::new(),
                    )
                    .expect("prepare"),
                &CancellationSignal::new(),
            )
            .expect("dispatch commits after activation")
        else {
            panic!("accepted dispatch");
        };
        let events = receive_until(&subscription, |event| {
            matches!(
                event.event,
                RuntimeClientEvent::BackgroundExecutionUpdated { .. }
            )
        })
        .await;
        assert_eq!(
            events[0].cursor,
            RuntimeClientCursor::new(1),
            "the first cursor belongs to a real post-activation transition"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeClientEvent::ResourceGenerationUpdated { .. }
                ))
                .count(),
            1,
            "the post-activation resource generation is published exactly once"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeClientEvent::BackgroundExecutionUpdated { .. }
                ))
                .count(),
            1,
            "the post-activation background commit is published exactly once, never seeded"
        );
        // The conversation-owned runner really starts and settles after
        // activation. The explicit execution gate deliberately keeps the
        // returned future before its release wait: the release state is
        // published first, then the future is allowed to observe it. An
        // edge-triggered `Notify::notify_waiters()` fixture would lose this
        // signal and hang at terminal settlement.
        await_background_started(&mut started, "the post-activation runner starts").await;
        release.send_replace(true);
        execution_gate.send_replace(true);
        await_background_terminal(&registry, &execution_id, "activated background execution").await;
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "one background record settles once"
        );
    }

    /// Regression for the PR #70 test-fixture lost wakeup: release the
    /// background execution while its returned future is intentionally held
    /// before the old `Notify::notified()` wait point. Durable release state
    /// must still settle the one committed record exactly once, without a
    /// second registry record or an invented admission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_release_before_future_wait_cannot_be_lost() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, mut started, release, execution_gate) =
            ParkingBackgroundTool::new_with_execution_gate();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-lost-wakeup"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted dispatch");
        };

        await_background_started(&mut started, "lost-wakeup regression runner").await;
        // This is the critical ordering: release is durable before the
        // returned future is allowed to reach its wait point.
        release.send_replace(true);
        execution_gate.send_replace(true);

        let terminal =
            await_background_terminal(&registry, &execution_id, "lost-wakeup regression").await;
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "no duplicate background record"
        );
        assert!(
            terminal.result.is_some(),
            "the single execution has one result"
        );
    }

    /// The standalone pre-runtime pieces of one ownership-transfer test:
    /// the tool runtime and the capability coordinator exist over one
    /// conversation, but no `ConversationRuntime` owns them yet.
    struct OwnershipFixture {
        _dir: tempfile::TempDir,
        adapter: Arc<GatedAdapter>,
        tool_runtime: crate::tools::runtime::ConversationToolRuntime,
        coordinator: crate::capabilities::CapabilityCoordinator,
    }

    /// Builds the standalone pieces exactly like the runtime fixtures, but
    /// stops before the `ConversationRuntime` construction.
    async fn ownership_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
    ) -> OwnershipFixture {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv-claim");
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
                tool_activation: crate::capabilities::ToolActivationPolicy::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig {
                    automatic_roots: vec![workspace.join(".agents/skills")],
                    explicit_paths: Vec::new(),
                },
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        OwnershipFixture {
            _dir: dir,
            adapter,
            tool_runtime,
            coordinator,
        }
    }

    /// The `RuntimeConversationConfig` over one ownership fixture.
    fn claim_config(fixture: &OwnershipFixture) -> RuntimeConversationConfig {
        RuntimeConversationConfig {
            agent_id: AgentId::new("agent-claim"),
            model: scripted_session_model(fixture.adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: status_engine(),
            },
            tool_runtime: fixture.tool_runtime.clone(),
            resources: test_resources(&fixture.coordinator),
            resource_loader: test_resource_loader(&fixture.coordinator),
            capability: fixture.coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        }
    }

    /// A background invocation for the ownership-transfer tests.
    fn claim_background_invocation(call_id: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: ToolCallId::new(call_id),
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".to_owned(),
            mode: ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        }
    }

    /// The ownership transfer rejects a tool runtime whose background plane
    /// already holds a **committed** standalone execution: typed
    /// `ToolRuntimeNotQuiescent`, no coordinator claim consumed, no
    /// capability claim consumed, the mailbox stays standalone/unbound, and
    /// the detached execution continues under standalone semantics.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn conversation_runtime_claim_rejects_committed_standalone_background_work() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-committed"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("a standalone commit succeeds");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the standalone runner starts").await;
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "the standalone execution is committed and running"
        );

        // The ownership transfer is refused typed...
        let refused = ConversationRuntime::new(claim_config(&fixture))
            .expect_err("a tool runtime with committed background work is not claimable");
        assert_eq!(
            refused,
            ConversationRuntimeError::ToolRuntimeNotQuiescent {
                conversation_id: ConversationId::new("conv-claim"),
            }
        );

        // ...and consumed nothing: no coordinator claim, no capability
        // claim, and the mailbox remains standalone.
        assert!(
            !fixture.tool_runtime.is_conversation_runtime_bound(),
            "the failed claim consumed no tool-runtime ownership"
        );
        assert!(
            !fixture.coordinator.is_conversation_runtime_bound(),
            "the failed claim consumed no capability ownership"
        );
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-1", "still standalone"))
            .expect("the mailbox remains standalone/unbound");

        // The detached execution keeps its standalone semantics and
        // settles normally.
        release.send_replace(true);
        let terminal =
            await_background_terminal(&registry, &execution_id, "standalone background execution")
                .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the standalone execution settles normally"
        );
    }

    /// The ownership transfer rejects a tool runtime with a **prepared but
    /// not committed** dispatch: typed `ToolRuntimeNotQuiescent`, no claim
    /// consumed, the mailbox stays standalone, and the prepared handle
    /// keeps its standalone semantics — dropping it rolls the dispatch back
    /// with no fabricated record. Once the background plane is pristine
    /// again, a fresh construction of the same identity succeeds, proving
    /// the failed claim did not consume the one-time ownership.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn conversation_runtime_claim_rejects_a_prepared_standalone_dispatch() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, started, _release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-prepared"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        assert!(
            registry.all_snapshots().is_empty(),
            "preparation publishes no record"
        );

        // The ownership transfer is refused typed and consumes nothing.
        let refused = ConversationRuntime::new(claim_config(&fixture))
            .expect_err("a staged dispatch makes the background plane non-quiescent");
        assert_eq!(
            refused,
            ConversationRuntimeError::ToolRuntimeNotQuiescent {
                conversation_id: ConversationId::new("conv-claim"),
            }
        );
        assert!(!fixture.tool_runtime.is_conversation_runtime_bound());
        assert!(!fixture.coordinator.is_conversation_runtime_bound());
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-2", "still standalone"))
            .expect("the mailbox remains standalone/unbound");

        // The prepared handle stays valid under standalone semantics:
        // dropping it rolls the dispatch back and fabricates no record.
        drop(prepared);
        assert!(
            registry.all_snapshots().is_empty(),
            "the rolled-back dispatch published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never begins");

        // The one-time claim was not consumed by the failed construction:
        // a fresh claim of the same identity succeeds once the background
        // plane is pristine again.
        let runtime = ConversationRuntime::new(claim_config(&fixture))
            .expect("a fresh claim succeeds after the prepared dispatch rolled back");
        assert!(
            fixture.tool_runtime.is_conversation_runtime_bound(),
            "the retried construction owns the identity"
        );
        assert!(!runtime.is_activated(), "the runtime is still inactive");
        runtime.activate();
    }

    /// Interleaving A of the ownership-transfer race: a standalone
    /// background commit parked exactly at its ownership-commit boundary
    /// (holding the registry synchronization lock) beats the racing
    /// `ConversationRuntime::new`. The claim provably linearizes after the
    /// committed record, fails typed `ToolRuntimeNotQuiescent`, and
    /// consumes nothing — the mailbox stays standalone and the detached
    /// execution stays valid.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_commit_racing_the_runtime_claim_wins_and_construction_fails_typed() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-race-a"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        registry.install_commit_boundary_hook(hook.clone());

        // The commit enters its critical section and parks there, holding
        // the registry lock at the ownership-commit boundary.
        let commit_registry = registry.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            commit_registry.commit_dispatch(prepared, &CancellationSignal::new())
        });
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.wait_entered())
                .await
                .expect("the commit entered the ownership boundary");
        }

        // Race the runtime claim: the rendezvous marker proves the claim
        // thread is in flight while the commit is parked, and because the
        // commit holds the registry lock, the claim's quiescence
        // observation necessarily linearizes *after* the commit's record
        // publication.
        let (marker_tx, marker_rx) = std::sync::mpsc::sync_channel(0);
        let claim_config = claim_config(&fixture);
        let claim_task = tokio::task::spawn_blocking(move || {
            marker_tx.send(()).expect("the claim is in flight");
            ConversationRuntime::new(claim_config)
        });
        marker_rx
            .recv()
            .expect("the claim thread started while the commit was parked");

        // Release the boundary: the commit wins, publishes the record, and
        // only then may the claim acquire the registry lock.
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.proceed())
                .await
                .expect("the commit boundary was released");
        }
        let outcome = commit_task
            .await
            .expect("commit outcome")
            .expect("the standalone commit succeeds");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the standalone runner starts").await;

        let refused = claim_task
            .await
            .expect("claim outcome")
            .expect_err("the claim linearizes after the committed background record");
        assert_eq!(
            refused,
            ConversationRuntimeError::ToolRuntimeNotQuiescent {
                conversation_id: ConversationId::new("conv-claim"),
            }
        );
        assert!(!fixture.tool_runtime.is_conversation_runtime_bound());
        assert!(!fixture.coordinator.is_conversation_runtime_bound());
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-3", "still standalone"))
            .expect("the mailbox remains standalone/unbound");

        release.send_replace(true);
        let terminal = await_background_terminal(
            &registry,
            &execution_id,
            "racing standalone background execution",
        )
        .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the racing standalone execution settles normally"
        );
    }

    /// Interleaving B of the ownership-transfer race: the runtime ownership
    /// transfer linearizes first, and a background commit that arrives
    /// afterwards fails typed `ConversationInactive` — no record published,
    /// the prepared runner rolls back, and the runtime remains inert until
    /// `activate()`. Ordinary capability commits are independently rejected
    /// because live publication belongs to resource reload; after activation,
    /// a fresh background dispatch commits normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_claim_racing_the_background_commit_wins_and_commit_fails_inactive() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();

        // The ownership transfer completes first: the runtime exists,
        // inactive, with its mailbox bound inactive.
        let runtime = ConversationRuntime::new(claim_config(&fixture))
            .expect("the ownership transfer wins the race");
        assert!(
            fixture.tool_runtime.is_conversation_runtime_bound(),
            "the runtime owns the tool runtime identity"
        );
        assert!(
            fixture.coordinator.is_conversation_runtime_bound(),
            "the runtime owns the capability identity"
        );
        assert!(!runtime.is_activated(), "the runtime is still inactive");

        // A background commit that linearizes after the transfer is refused
        // typed: no record, no runner start, the prepared dispatch rolls
        // back completely.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-race-b"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("preparation is still allowed");
        let refused = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect_err("a commit after the transfer observes the inactive runtime");
        assert_eq!(
            refused,
            BackgroundDispatchError::ConversationInactive {
                conversation_id: ConversationId::new("conv-claim"),
            }
        );
        assert!(
            registry.all_snapshots().is_empty(),
            "the refused commit published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never begins");

        // Activation is the single semantic-open boundary: a fresh dispatch
        // commits normally afterwards.
        runtime.activate();
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-race-b-2"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare after activation");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("a post-activation commit succeeds");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the post-activation runner starts").await;
        release.send_replace(true);
        let terminal = await_background_terminal(
            &registry,
            &execution_id,
            "post-activation background execution",
        )
        .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the post-activation execution settles normally"
        );
    }

    /// Transactional construction: when the capability claim fails after
    /// the tool-runtime ownership transfer, the transfer is rolled back to
    /// its exact previous standalone state — the coordinator claim is
    /// cleared and the mailbox is unbound again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_capability_claim_rolls_back_the_tool_runtime_ownership_transfer() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        // Consume the capability identity's one-time claim *before*
        // construction, so the capability claim inside `new` fails after
        // the tool-runtime transfer already succeeded.
        assert!(
            fixture
                .coordinator
                .claim_conversation_runtime(&crate::runtime::types::ConversationLifecycle::new())
                .is_some()
        );

        let refused = ConversationRuntime::new(claim_config(&fixture))
            .expect_err("a claimed capability identity rejects construction");
        assert_eq!(
            refused,
            ConversationRuntimeError::RuntimeAlreadyBound {
                conversation_id: ConversationId::new("conv-claim"),
            }
        );
        assert!(
            !fixture.tool_runtime.is_conversation_runtime_bound(),
            "the failed construction released the tool-runtime claim"
        );
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-4", "still standalone"))
            .expect("the rolled-back mailbox accepts standalone inbound");
    }

    /// The activation regression: `ConversationRuntime::activate` performs
    /// one shared `Inactive -> Running` lifecycle transition, and every
    /// runtime-owned semantic boundary observes exactly that transition.
    ///
    /// The activation gate parks `activate` before the lifecycle
    /// transition: while parked, a background commit, an ordinary capability
    /// commit, and a mailbox enqueue are refused typed (consuming nothing);
    /// after the gate is released, live capability mutation uses resource
    /// reload and the other operations observe `Running` and follow normal
    /// semantics. The park
    /// proves both sides against the *one* shared decision — the mailbox,
    /// the background registry, and the capability coordinator can never
    /// observe contradictory lifecycle states, because there is only one
    /// lifecycle authority to observe.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn activation_is_one_shared_lifecycle_transition() {
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (_adapter, fixture) = runtime_only_fixture(
            vec![one_turn_stop(), one_turn_stop()],
            ToolRegistry::new(),
            Some(CoordinatorProbe {
                admission_gate: None,
                settlement_gate: None,
                activation_gate: Some(gate.clone()),
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            }),
        )
        .await;
        let registry = fixture.runtime.tool_runtime().background().clone();
        let coordinator = fixture.coordinator.clone();
        gate.arm();

        // Park `activate` exactly before the lifecycle transition: while
        // the park holds, the conversation is provably still Inactive.
        let runtime = fixture.runtime.clone();
        let activate_task = tokio::task::spawn_blocking(move || runtime.activate());
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .expect("activate entered the gate");
        }

        // Pre-side: every runtime-owned semantic commit observes Inactive
        // and is refused typed, consuming nothing.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-activation-pre"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let refused = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect_err("a pre-transition background commit is refused");
        assert_eq!(
            refused,
            BackgroundDispatchError::ConversationInactive {
                conversation_id: ConversationId::new("conv-host"),
            }
        );
        assert!(
            registry.all_snapshots().is_empty(),
            "the refused commit published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never begins");

        let refused = coordinator
            .commit(coordinator.prepare_candidate().await.expect("prepare"))
            .expect_err("a pre-transition capability commit is refused");
        assert_eq!(
            refused,
            crate::capabilities::CapabilityCommitError::RuntimePublicationRequired
        );

        let refused = fixture
            .runtime
            .submit_inbound(submit_content("early"))
            .expect_err("a pre-transition inbound is refused");
        assert_eq!(refused, InboundAdmissionError::Inactive);

        // A real capability candidate for the post-transition commit.
        write_probe_skill(&fixture.workspace, "pdf");

        // Release: the one lifecycle transition commits.
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.release())
                .await
                .expect("the activation gate was released");
        }
        activate_task.await.expect("activate completes");
        assert!(fixture.runtime.is_activated());

        // Post-side: the same operations observe Running and follow the
        // normal running semantics.
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-activation-post"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("a post-transition background commit succeeds")
        else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the post-transition runner starts").await;

        let reloaded = fixture
            .runtime
            .reload_resources()
            .await
            .expect("a post-transition resource/capability reload succeeds");
        assert_eq!(
            reloaded.capability_revision.get(),
            1,
            "the first live capability revision"
        );

        fixture
            .runtime
            .submit_inbound(submit_content("late"))
            .expect("a post-transition inbound is accepted");
        fixture.runtime.settlement_signal().notified().await;

        // Settle the background execution cleanly.
        release.send_replace(true);
        let terminal = await_background_terminal(
            &registry,
            &execution_id,
            "cross-subsystem background execution",
        )
        .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the post-transition execution settles normally"
        );
    }

    /// The real-time ordered cross-subsystem regression: live capability
    /// publication must use the runtime resource boundary even when a
    /// background ownership commit has already observed `Running` and is
    /// parked in its own commit section.
    ///
    /// The registry commit-boundary hook parks a background commit after it
    /// has already observed `Running` inside its critical section; a
    /// resource reload that begins afterwards is parked at the capability
    /// publication boundary, then a second reload follows the background
    /// completion. The parks and task joins prove the real-time ordering with
    /// no timing assumptions.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_semantic_commits_cannot_disagree_across_activation() {
        let (_adapter, fixture) = runtime_only_fixture(
            vec![one_turn_stop(), one_turn_stop()],
            ToolRegistry::new(),
            None,
        )
        .await;
        let registry = fixture.runtime.tool_runtime().background().clone();
        let coordinator = fixture.coordinator.clone();
        fixture.runtime.activate();

        // Real resource generations for the two post-activation commits.
        write_probe_skill(&fixture.workspace, "pdf");

        // Prepare a background dispatch and park its commit at the
        // registry ownership-commit boundary: the commit has already
        // observed the shared lifecycle (Running) inside its critical
        // section when the hook fires.
        let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        registry.install_commit_boundary_hook(hook.clone());
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-epoch-b"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let commit_registry = registry.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            commit_registry.commit_dispatch(prepared, &CancellationSignal::new())
        });
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.wait_entered())
                .await
                .expect("the background commit entered its boundary after observing Running");
        }

        // A resource reload that begins now — real-time after the background
        // commit's lifecycle observation — owns both capability and resource
        // publication. Park its capability half to make the ordering exact.
        let capability_hook =
            Arc::new(crate::capabilities::test_sync::CommitBoundaryHook::default());
        coordinator.install_commit_boundary_hook(capability_hook.clone());
        let reload_runtime = fixture.runtime.clone();
        let reload_task = tokio::spawn(async move { reload_runtime.reload_resources().await });
        capability_hook.wait_entered();
        capability_hook.proceed();
        let committed = reload_task
            .await
            .expect("reload task")
            .expect("the runtime-owned publication observes Running");
        assert_eq!(committed.capability_revision.get(), 1);

        // The background commit completes successfully.
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.proceed())
                .await
                .expect("the background commit boundary was released");
        }
        let outcome = commit_task
            .await
            .expect("commit outcome")
            .expect("the background commit succeeds after observing Running");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the runner starts").await;

        // The old contradiction shape: B completed successfully, then C
        // begins through a second resource publication.
        write_probe_skill(&fixture.workspace, "docx");
        let committed = fixture
            .runtime
            .reload_resources()
            .await
            .expect("a second resource publication remains live");
        assert_eq!(committed.capability_revision.get(), 2);

        // Settle the background execution cleanly.
        release.send_replace(true);
        let terminal = await_background_terminal(
            &registry,
            &execution_id,
            "activation-order background execution",
        )
        .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the background execution settles normally"
        );
    }

    /// The host-binding vs activation race: `RuntimeClientHost::new` and
    /// `ConversationRuntime::activate` race against the same lifecycle
    /// transition, serialized by the one coordinator lock the transition
    /// commits under. This interleaving proves "host wins": the host
    /// binds while `activate` is parked before the lifecycle transition,
    /// completes with the bootstrap seed at cursor 0, and the transition
    /// then commits — the first cursor belongs to a real post-activation
    /// transition. The activation-wins interleaving is proven by
    /// `late_host_bind_after_activation_is_rejected_typed`.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_bind_racing_activation_has_one_clean_linearization() {
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (_adapter, fixture) = runtime_only_fixture(
            vec![one_turn_stop()],
            ToolRegistry::new(),
            Some(CoordinatorProbe {
                admission_gate: None,
                settlement_gate: None,
                activation_gate: Some(gate.clone()),
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                background_failure_gate: None,
            }),
        )
        .await;
        gate.arm();

        // Park `activate` before the lifecycle transition: the
        // conversation is provably still Inactive, so the host bind wins
        // the race and completes with the full bootstrap seed.
        let runtime = fixture.runtime.clone();
        let activate_task = tokio::task::spawn_blocking(move || runtime.activate());
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .expect("activate entered the gate");
        }
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("the host binds before the lifecycle transition");
        assert!(
            fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "the successful bind consumed the one-time claim"
        );
        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(
            snapshot.background.is_empty(),
            "the inert runtime contributes no background seed"
        );

        // Release the transition: activation commits and the one-time
        // post-activation kick runs.
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.release())
                .await
                .expect("the activation gate was released");
        }
        activate_task.await.expect("activate completes");
        assert!(fixture.runtime.is_activated());

        // The first cursor belongs to a real post-activation transition.
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe from the bootstrap cursor");
        host.submit_inbound(submit_content("first transition"))
            .expect("accepted");
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        assert_eq!(
            events[0].cursor,
            RuntimeClientCursor::new(1),
            "the first cursor is a real post-activation transition"
        );
    }

    /// Concurrent `activate` calls are idempotent: exactly one call
    /// commits the lifecycle transition (`Inactive -> Running` CAS) and
    /// performs the one-time post-transition work — worker spawn and the
    /// admission kick — and every other call observes `Running` and returns
    /// without changing anything. A single inbound item therefore admits
    /// exactly one attempt, never two.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_activation_is_idempotent_and_creates_one_worker() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let runtime = fixture.runtime.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let a = {
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                runtime.activate();
            })
        };
        let b = {
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                runtime.activate();
            })
        };
        barrier.wait();
        a.await.expect("activate a");
        b.await.expect("activate b");
        assert!(fixture.runtime.is_activated());

        // One inbound item admits exactly one attempt: a duplicated
        // activation kick can never admit a second attempt from one item,
        // and a duplicated worker is structurally impossible (one CAS
        // winner, one `worker_started` guard).
        fixture
            .runtime
            .submit_inbound(submit_content("one item"))
            .expect("accepted");
        fixture.runtime.settlement_signal().notified().await;
        assert_eq!(
            adapter.requests().len(),
            1,
            "exactly one attempt from one activation epoch"
        );
    }

    /// Test A — a model mutation while the runtime is inactive is rejected
    /// typed and consumes nothing: the model is unchanged, the host
    /// snapshot stays at cursor 0, and no `SessionModelChanged` event
    /// exists. After activation the same update succeeds and is delivered
    /// exactly once with the first real cursor.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn model_set_while_inactive_is_rejected_and_consumes_nothing() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");

        // The live mutation is refused typed while inactive...
        let refused = fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "early"))
            .expect_err("a model update while inactive is refused");
        assert_eq!(refused, ModelUpdateError::Inactive);

        // ...consumes nothing: the model is unchanged, the snapshot is the
        // bootstrap state at cursor 0, and no event exists.
        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(
            snapshot
                .model
                .configured
                .request_params
                .get("early")
                .is_none(),
            "the rejected update left the model unchanged"
        );
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "the rejected update published no event"
        );

        // After activation the same update succeeds and receives the first
        // real cursor, exactly once.
        fixture.runtime.activate();
        fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "early"))
            .expect("the update succeeds after activation");
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. })
        })
        .await;
        assert_eq!(
            events[0].cursor,
            RuntimeClientCursor::new(1),
            "the first cursor belongs to the real post-activation transition"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeClientEvent::SessionModelChanged { .. }
                ))
                .count(),
            1,
            "the post-activation update is delivered exactly once"
        );
    }

    /// Test B — a shutdown while the runtime is inactive is rejected typed
    /// and is non-semantic: the runtime is not marked shutting down, the
    /// snapshot stays at cursor 0, and no `RuntimeShutdown` event exists.
    /// After activation shutdown retains the existing runtime semantics.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_while_inactive_is_rejected_and_consumes_nothing() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");

        assert_eq!(
            fixture.runtime.shutdown().await,
            Err(crate::runtime::conversation_runtime::ShutdownError::Inactive),
            "a shutdown while inactive is refused typed"
        );
        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(
            !snapshot.shutting_down,
            "the refused shutdown never marked the runtime shutting down"
        );
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "the refused shutdown published no event"
        );

        // After activation shutdown completes its one semantic drain,
        // publishes the drain observation exactly once, and gates inbound
        // afterwards.
        fixture.runtime.activate();
        fixture
            .runtime
            .shutdown()
            .await
            .expect("accepted after activation");
        let events = receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::RuntimeShutdown)
        })
        .await;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.event, RuntimeClientEvent::RuntimeShutdown))
                .count(),
            1,
            "the post-activation shutdown publishes exactly one event"
        );
        assert_eq!(
            fixture.runtime.submit_inbound(submit_content("late")),
            Err(InboundAdmissionError::Shutdown)
        );
    }

    /// A model update that linearizes after activation — that is, after
    /// the bootstrap cut — arrives through the live observation stream
    /// exactly once and is never lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn post_activation_model_transition_is_delivered_exactly_once() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");
        fixture.runtime.activate();

        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe from the bootstrap cursor");

        fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "after-cut"))
            .expect("model transition after the cut");

        let (snapshot, _cursor) = host.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.model.configured.request_params.get("after-cut"),
            Some(&serde_json::json!("changed")),
            "the post-cut transition must be visible in the projection"
        );
        let mut session_model_events = 0;
        receive_until(&subscription, |event| {
            if matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. }) {
                session_model_events += 1;
            }
            matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. })
        })
        .await;
        assert_eq!(
            session_model_events, 1,
            "the post-cut transition is delivered exactly once"
        );
    }

    /// Failed host construction never leaves a claimed-but-invalid
    /// binding: when the observation bridge handshake fails (a previous
    /// headless bridge exists), the one-time client binding claim is
    /// released again and the failure is typed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_host_construction_releases_the_client_binding() {
        let (_adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
        // A headless observation bridge already exists over the runtime.
        let queue = Arc::new(crate::runtime::observation::PendingObservations::new());
        fixture
            .runtime
            .install_observation_bridge(queue)
            .expect("headless bridge");

        let rejected = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        });
        match rejected {
            Err(HostConstructionError::ObservationBridgeAlreadyInstalled { conversation_id }) => {
                assert_eq!(conversation_id.as_str(), "conv-host");
            }
            _ => panic!("the bridge conflict must fail typed"),
        }
        // The failed construction released the binding claim: no
        // claimed-but-invalid binding remains.
        assert!(
            !fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "the tool runtime binding was released"
        );
        assert!(
            !fixture.runtime.capability().is_runtime_client_bound(),
            "the capability binding was released"
        );
    }
}
