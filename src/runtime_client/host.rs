//! The Runtime Client host: the projection + control + attachment adapter
//! over the conversation runtime coordinator (Issue #61).
//!
//! [`RuntimeClientHost`] is the Runtime Client boundary of Protocol v1. It
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
//! - the one-active-attachment v1 policy;
//! - the Runtime Client projection (snapshot read model, cursor allocation,
//!   bounded replay, subscribers) and its linearization boundary;
//! - protocol adaptation: request dispatch, `model_set`/`shutdown`/
//!   `cancel_current_attempt` forwarding, inbound publish forwarding;
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
//! - cancellation terminal settlement (`AgentExecution` remains the attempt
//!   execution/terminal authority);
//! - background/subagent lifecycle.
//!
//! # Observation handoff
//!
//! The conversation runtime publishes every semantically meaningful
//! transition as a runtime-owned
//! [`ConversationObservation`](crate::runtime::observation::ConversationObservation)
//! into the shared leaf
//! [`PendingObservations`](crate::runtime::observation::PendingObservations)
//! queue, which the runtime installs through its bootstrap handshake at
//! host construction (see
//! `ConversationRuntime::install_observation_bridge`). The handshake runs
//! over an inert, not-yet-activated runtime and captures the bootstrap
//! snapshot and every subsystem observation seam at one global cut, so
//! the projection's initial seed and the live observation stream cover
//! the runtime's history with no gap and no duplication — and the seed
//! itself publishes nothing and allocates no cursor.
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
//!   ClientState ─────────────► PendingObservations (leaf)
//!       ▲
//!       │  (never; see below)
//!   coordinator ─────────────► PendingObservations
//!   mailbox / background / capability ─► PendingObservations
//! ```
//!
//! No authoritative subsystem ever acquires `ClientState`. The mailbox, the
//! background registry, the capability coordinator, and the agent attempt
//! task all fire their observers while their own boundary is held, so
//! [`ClientObserver`] is never used: the conversation runtime's own
//! observers (see `crate::runtime::conversation_runtime::RuntimeObserver`)
//! append to the leaf queue instead.
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
//! shared observation queue (the projection worker's terminal condition);
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

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg_attr(not(test), allow(unused_imports))]
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use super::projection::{RuntimeClientProjection, SubscriberPoll, background_view};
use super::types::{
    AttachmentId, RUNTIME_CLIENT_PROTOCOL_VERSION_V1, RuntimeClientCursor, RuntimeClientError,
    RuntimeClientProtocolEvent, RuntimeClientResult,
};
use crate::model::session::SessionModelConfig;
use crate::model::{ModelRequest, RequestIdentity};
use crate::runtime::conversation_runtime::{
    CancelAttemptError, ConversationRuntime, InboundAdmissionError, ModelUpdateError,
    RuntimeBootstrapError,
};
use crate::runtime::identity::{ConversationId, ToolExecutionId};
use crate::runtime::observation::PendingObservations;
use crate::runtime::request_history::{RequestHistory, RequestHistoryError};

/// The one Runtime Client host construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostConstructionError {
    /// The conversation runtime identity is already bound to a Runtime
    /// Client host.
    ///
    /// Protocol v1 binds one runtime identity to at most one
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
        }
    }
}

impl std::error::Error for HostConstructionError {}

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
    /// The at-most-one active attachment of Protocol v1.
    attachment: Option<AttachmentState>,
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

/// The shared Runtime Client host state.
pub(crate) struct ClientInner {
    conversation_id: ConversationId,
    agent_id: crate::runtime::identity::AgentId,
    /// The conversation runtime this host observes and controls.
    runtime: ConversationRuntime,
    /// The one projection synchronization boundary.
    state: Mutex<ClientState>,
    /// The observation queue shared with the conversation runtime (the
    /// projection sink installed at construction).
    pending: Arc<PendingObservations>,
    /// Whether the projection worker task was spawned.
    worker_started: AtomicBool,
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
    /// Protocol v1 allows at most one active attachment; a second
    /// simultaneous attach fails deterministically and never evicts the
    /// first. The returned snapshot and cursor are linearized with the
    /// admission under the one projection synchronization boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when an
    /// attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub(crate) fn attach(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        if protocol_version != RUNTIME_CLIENT_PROTOCOL_VERSION_V1 {
            return Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
                requested: protocol_version,
            });
        }
        self.ensure_worker();
        let mut state = self.lock_state();
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
            super::attachment::RuntimeAttachment::new(attachment_id.clone(), self.clone());
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
        let admission = self
            .runtime
            .submit_inbound(content)
            .map_err(|error| match error {
                InboundAdmissionError::Shutdown => RuntimeClientError::RuntimeShutdown,
                InboundAdmissionError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                InboundAdmissionError::EmptyContent => RuntimeClientError::InvalidRequest {
                    message: "inbound content must not be empty".to_owned(),
                },
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
        match self.runtime.cancel_current_attempt(&attempt_id) {
            Ok(attempt_id) => Ok(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }),
            Err(CancelAttemptError::NoCurrentAttempt) => Err(RuntimeClientError::NoCurrentAttempt),
        }
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
        let state = self.lock_state();
        state.projection.snapshot()
    }

    /// Returns the immutable in-memory request facts retained by the
    /// conversation runtime.
    ///
    /// The runtime owns these snapshots after attempt settlement. The
    /// returned value is a read-only clone of the request-fact collection;
    /// it does not create another conversation or transcript authority.
    #[must_use]
    pub(crate) fn request_history(&self) -> RequestHistory {
        self.runtime.request_history()
    }

    /// Reconstructs one retained provider-neutral request from its frozen
    /// snapshot and the exact historical Surface revisions in the runtime's
    /// authoritative `ConversationState`.
    ///
    /// # Errors
    ///
    /// Returns [`RequestHistoryError::ConversationUnavailable`] while the
    /// single `ConversationState` is owned by a running attempt, or a
    /// lookup / historical reconstruction error for an unknown or invalid
    /// request.
    pub(crate) fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.runtime.reconstruct_request(identity)
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
        self.ensure_worker();
        let mut state = self.lock_state();
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
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        Ok(RuntimeClientResult::ModelCatalog {
            catalog: self.runtime.model_catalog(),
        })
    }

    /// Reads the authoritative session model state through the folded
    /// projection, so the value always agrees with the snapshot read model.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
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
    /// under the session context policy, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub(crate) fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        let view = self
            .runtime
            .model_set(config)
            .map_err(|error| match error {
                ModelUpdateError::InvalidConfiguration(message) => {
                    RuntimeClientError::InvalidModelConfiguration { message }
                }
            })?;
        Ok(RuntimeClientResult::ModelSet {
            model: Box::new(view),
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
        let Some(snapshot) = self.runtime.background_status(execution_id) else {
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
        let Some(snapshot) = self.runtime.background_cancel(execution_id) else {
            return Err(RuntimeClientError::UnknownBackgroundExecution {
                execution_id: execution_id.clone(),
            });
        };
        Ok(RuntimeClientResult::BackgroundCancelAccepted {
            execution: background_view(&snapshot),
        })
    }

    /// Accepts the local-runtime shutdown request through the conversation
    /// runtime.
    ///
    /// Shutdown is not detach and not cancellation: the current attempt
    /// continues to its settlement, semantic runtime work is never mutated,
    /// and no further inbound admission occurs.
    #[must_use]
    pub(crate) fn shutdown(&self) -> RuntimeClientResult {
        self.runtime.shutdown();
        RuntimeClientResult::ShutdownAccepted
    }
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
    /// headless observation bridge already exists over the runtime.
    pub fn new(config: RuntimeClientHostConfig) -> Result<Self, HostConstructionError> {
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
        };

        // ---- Infallible wiring: from here construction always succeeds. ----
        //
        // The projection mirrors the runtime's authoritative seed exactly
        // — canonical history, session model, capability snapshot, pending
        // inbound, and pre-existing background executions — entirely as
        // snapshot state. No seeded fact is routed through
        // `RuntimeClientProjection::apply`, so bootstrap allocates no
        // cursor and publishes no event: the first cursor belongs to a
        // real post-activation transition.
        let mut projection = RuntimeClientProjection::new(
            seed.conversation_id.clone(),
            seed.messages.clone(),
            super::projection::capability_view(&seed.capabilities),
            seed.model.clone(),
            replay_limit,
        );
        projection.bootstrap(&seed);
        let inner = Arc::new(ClientInner {
            conversation_id: seed.conversation_id,
            agent_id: config.runtime.agent_id().clone(),
            runtime: config.runtime,
            state: Mutex::new(ClientState {
                projection,
                attachment: None,
                next_attachment_seq: 0,
            }),
            pending,
            worker_started: AtomicBool::new(false),
        });
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
        super::endpoint::RuntimeClientEndpoint::new(self.clone())
    }

    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
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
        self.inner.attach(protocol_version)
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

    /// Returns the immutable in-memory request facts retained by the
    /// conversation runtime.
    #[must_use]
    pub fn request_history(&self) -> RequestHistory {
        self.inner.request_history()
    }

    /// Reconstructs one retained provider-neutral request from its frozen
    /// snapshot and the exact historical Surface revisions in the runtime's
    /// authoritative `ConversationState`.
    ///
    /// # Errors
    ///
    /// Returns [`RequestHistoryError::ConversationUnavailable`] while the
    /// single `ConversationState` is owned by a running attempt, or a
    /// lookup / historical reconstruction error.
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

    /// Accepts the local-runtime shutdown request.
    #[must_use]
    pub fn shutdown(&self) -> RuntimeClientResult {
        self.inner.shutdown()
    }

    /// Convenience for tests: host-level conversation-state accessors that
    /// forward to the conversation runtime.
    #[cfg(test)]
    pub(crate) fn host_ledger(&self) -> Option<Vec<crate::message::types::MessageBlock>> {
        self.inner.runtime.coordinator_ledger()
    }

    /// The runtime's active Surface identities, or `None` while an attempt
    /// owns the conversation state.
    #[cfg(test)]
    pub(crate) fn host_active_ids(&self) -> Option<Vec<crate::runtime::identity::MessageId>> {
        self.inner.runtime.coordinator_active_ids()
    }

    #[cfg(test)]
    #[allow(dead_code)] // used by the race regression tests
    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner.runtime.has_current_attempt()
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
        self.inner.runtime.weak_inner()
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
        self.inner.runtime.install_worker_exit_probe(sender);
    }

    /// The conversation runtime this host observes and controls.
    #[cfg(test)]
    pub(crate) fn runtime(&self) -> &ConversationRuntime {
        &self.inner.runtime
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
    /// The bounded pre-M8 replay retention; the default is used when
    /// omitted.
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

    use futures_util::future::BoxFuture;
    use tokio::sync::watch;

    use super::{EventDelivery, EventSubscription, RuntimeClientHost, RuntimeClientHostConfig};
    use crate::context::{
        AgentStatusClock, AgentStatusComposer, AgentStatusFact, AgentStatusRenderContext,
        AgentStatusSectionId, AgentStatusSectionProvider, ContextError, DefaultTokenEstimator,
        TokenEstimator,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{
        ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelEventStream};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::conversation_runtime::{
        ConversationContextConfig, ConversationRuntime, CoordinatorProbe, RuntimeConversationConfig,
    };
    use crate::runtime::identity::{AgentId, ConversationId, ToolCallId, ToolId};
    use crate::runtime::types::RuntimeClock;
    use crate::runtime_client::event::RuntimeClientEvent;
    use crate::runtime_client::host::HostConstructionError;
    use crate::runtime_client::snapshot::RuntimeClientAttemptPhase;
    use crate::runtime_client::types::{
        RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent, RuntimeClientRequest,
        RuntimeClientResult,
    };
    use crate::scripted_suites::support::model::scripted_session_model;
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
            timezone: None,
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_composer: composer,
            },
            tool_runtime,
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
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
                timezone: None,
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    status_composer: composer(),
                },
                tool_runtime,
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
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

    /// Submitting while idle admits and runs the attempt through the
    /// conversation runtime's single admission path; the admission response
    /// is accepted, not finished; the attempt settles and the canonical
    /// history is committed.
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

        // The first model request observed the admitted Runtime Agent Status
        // fact through canonical history.
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].messages.iter().any(|message| {
            matches!(message, MessageBlock::User(user) if user.id == message_id)
        }));
        assert!(requests[0].messages.iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user)
                    if user.kind
                        == crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus,
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

        // Request facts survive the AgentExecutionResult transfer. Mutate
        // the live session configuration after settlement and reconstruct
        // from the retained snapshot plus the runtime-owned historical
        // Surface; neither current configuration nor a live contributor is
        // consulted.
        let requests = adapter.requests();
        let history = fixture.host.request_history();
        assert_eq!(history.snapshots().len(), 1);
        let retained = history.snapshots()[0].clone();
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
            history.get(&retained.identity),
            Some(&retained),
            "request history lookup is identity-based and immutable"
        );
    }

    /// A composed runtime retains every actual primary request, including
    /// an overflow retry, after the `AgentExecutionResult` has been
    /// transferred and dropped. The retry keeps the pending fresh inbound
    /// visible while both request facts remain reconstructable from their
    /// own Surface revisions.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn settled_host_retains_distinct_overflow_request_snapshots() {
        let (adapter, fixture) = host_fixture(
            vec![
                one_turn_stop(),
                vec![GatedStep::Emit(ModelEvent::Failed {
                    error: ModelError {
                        kind: ModelErrorKind::ContextWindowExceeded,
                        message: "context window exceeded".to_owned(),
                        retry_after_ms: None,
                        provider_code: None,
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
        assert_eq!(history.snapshots().len(), 3);
        assert_eq!(history.snapshots()[0].identity.retry_number, 0);
        assert_eq!(history.snapshots()[1].identity.retry_number, 0);
        assert_eq!(history.snapshots()[2].identity.retry_number, 1);
        assert_eq!(
            history.snapshots()[1].identity.attempt_id,
            history.snapshots()[2].identity.attempt_id
        );
        assert_eq!(
            history.snapshots()[1].context_generation,
            history.snapshots()[2].context_generation,
            "overflow retry keeps the one admitted context generation"
        );
        assert_ne!(
            history.snapshots()[1].surface_revision,
            history.snapshots()[2].surface_revision,
            "compaction gives the retry its own historical Surface revision"
        );

        let provider_requests = adapter.requests();
        assert_eq!(
            provider_requests.len(),
            4,
            "three primary requests plus summary"
        );
        for (snapshot, request) in history.snapshots().iter().zip([
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
                    MessageBlock::User(user) if user.id.as_str() == "conv-host-inbound-2"
                )
            }),
            "the retry still observes the pending fresh inbound"
        );
    }

    /// Waits until the runtime owns the conversation state again and its
    /// Message Ledger equals the expected records.
    async fn await_canonical_history(host: &RuntimeClientHost, expected: &[MessageBlock]) {
        tokio::time::timeout(std::time::Duration::from_secs(120), async {
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

    /// Waits for the post-settlement transfer of frozen request facts to the
    /// runtime-owned append-only history.
    async fn await_request_history_len(host: &RuntimeClientHost, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(120), async {
            loop {
                if host.request_history().snapshots().len() == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request history transfer must settle");
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
            composer(),
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
        else {
            panic!("accepted dispatch");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("background runner started");
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
        release.notify_one();
        fixture
            .runtime
            .tool_runtime()
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

    /// Detach never changes canonical conversation state, and a detached
    /// conversation keeps admitting asynchronous inbound through the one
    /// coordinator path (Test 2).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_does_not_affect_conversation_or_future_async_admission() {
        let (adapter, fixture) =
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
            content: submit_content("first"),
        });
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
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
        tool_started
            .wait_for(|started| *started)
            .await
            .expect("the parking tool started");
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
                MessageBlock::System(_) => "system",
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
                "user",
                "assistant",
                "user",
                "user",
                "assistant",
            ],
            "one authoritative history, including one canonical Runtime \
             context fact for each fresh inbound step, extended across the \
             tool turn, the safe-boundary drain, and both attempts"
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

    /// Releasing the last semantic owner destroys the host adapter and the
    /// conversation runtime, and terminates both workers — deterministically,
    /// and without depending on process exit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn releasing_the_last_owner_destroys_the_host_and_exits_the_workers() {
        let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
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
        else {
            panic!("accepted dispatch");
        };
        started
            .wait_for(|started| *started)
            .await
            .expect("background runner started");
        release.notify_one();
        runtime
            .tool_runtime()
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        // The registry publishes its terminal notification into the
        // authoritative mailbox; the runtime wake gate admits it into a
        // second attempt, which settles immediately (no scripts). Waiting
        // for its request-history transfer makes the runtime provably idle
        // before the capability commit below.
        await_request_history_len(&host, 2).await;
        write_probe_skill(&dir.path().join("workspace"), "lifetime-skill");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");

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
        let (_adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
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
        release.notify_one();
        fixture
            .runtime
            .tool_runtime()
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_capability_commit_never_waits_on_the_host_lock() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
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
        // held: there is no `ClientState -> capability` edge.
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
            .runtime
            .tool_runtime()
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
            .runtime
            .tool_runtime()
            .background()
            .wait_until_terminal(&execution_id)
            .await
            .expect("terminal");
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    }

    /// Agent Status is admitted from the exact same composition the model
    /// path consumes: the client event's rendered text equals the canonical
    /// Runtime context fact sent in the model request.
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
            .messages
            .iter()
            .find_map(|message| match message {
                MessageBlock::User(user)
                    if user.kind
                        == crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus,
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
    #[allow(clippy::too_many_lines)] // one complete shutdown lifecycle
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
        let repeated = attachment.handle_request(RuntimeClientRequest::Shutdown {
            id: crate::runtime_client::RequestId::new(5),
        });
        assert!(matches!(
            repeated.result,
            Some(RuntimeClientResult::ShutdownAccepted)
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

        // The current attempt continues to settlement; detach still works.
        release_tx.send(true).expect("release");
        receive_until(&subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        attachment.detach();
        let (reattached, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn human_and_runtime_inbound_share_one_admission_path() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), composer()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        // The native Runtime producer publishes first (a background-style
        // terminal notification), then the human submit.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("conv-host-async-1", "runtime"))
            .expect("runtime enqueue");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("human"),
        });

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
            .filter_map(|message| match message {
                MessageBlock::User(user)
                    if user.kind == crate::message::types::InboundKind::Message =>
                {
                    Some(user.id.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            inbound_ids,
            vec!["conv-host-async-1", "conv-host-inbound-1"],
            "both producers sequence through one mailbox in order"
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
            },
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
        let requests = adapter.requests();
        assert_eq!(
            requests.len(),
            2,
            "exactly one next attempt (never two, never zero)"
        );
        assert!(
            requests[1].messages.iter().any(|message| matches!(
                message,
                MessageBlock::User(user) if user.id.as_str() == "conv-host-async-2"
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
            host_fixture(vec![script, one_turn_stop()], tools, composer()).await;
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
        // Both sibling tool calls start (the loop executes the batch); A
        // parks.
        a_started
            .wait_for(|started| *started)
            .await
            .expect("tool A started");
        b_started
            .wait_for(|started| *started)
            .await
            .expect("tool B started");

        // An async inbound arrives while the sibling batch is in flight.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("conv-host-async-2", "during batch"))
            .expect("async enqueue");

        // Release both tools; the batch settles structurally, the safe
        // boundary drains the inbound into the next turn.
        a_release.notify_one();
        b_release.notify_one();
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
            .filter_map(|message| match message {
                MessageBlock::Tool(_) => Some("tool"),
                MessageBlock::User(user)
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
                message,
                MessageBlock::User(user) if user.id.as_str() == "conv-host-async-2"
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
            },
        )
        .await;
        let (attachment_a, _) = fixture_a
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
            composer(),
        )
        .await;
        let (attachment_b, _) = fixture_b
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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

        // A capability commit lands mid-attempt: the coordinator rejects it
        // deterministically — the attempt's lease pins the revision.
        write_probe_skill(
            fixture.runtime.tool_runtime().workspace().root(),
            "mid-attempt-skill",
        );
        let candidate = fixture
            .coordinator
            .prepare_candidate()
            .await
            .expect("prepare");
        let rejected = fixture.coordinator.commit(candidate);
        assert!(
            matches!(
                rejected,
                Err(crate::capabilities::CapabilityCommitError::Busy)
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

        // After settlement the same commit succeeds; the admitted attempt's
        // request facts still carry the pre-commit revision, and the
        // projection observes the post-commit revision.
        let candidate = fixture
            .coordinator
            .prepare_candidate()
            .await
            .expect("prepare");
        let committed = fixture
            .coordinator
            .commit(candidate)
            .expect("commit after settlement");
        assert!(committed.revision() > revision_at_admission);
        let history = fixture.host.request_history();
        assert_eq!(
            history.snapshots()[0].capability_revision,
            revision_at_admission,
            "the later capability change never retroactively mutates the admitted attempt"
        );
        let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
        assert_eq!(snapshot.capabilities.revision, committed.revision());
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
            timezone: None,
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_composer: composer(),
            },
            tool_runtime,
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
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
                timezone: None,
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    status_composer: composer(),
                },
                tool_runtime,
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
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
            timezone: None,
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_composer: composer(),
            },
            tool_runtime,
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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

    /// Test 1 — bootstrap state never allocates a live cursor event.
    ///
    /// A background execution and a real capability activation exist
    /// *before* the Runtime Client host binds. The initial snapshot must
    /// carry both, the snapshot cursor must still be the bootstrap cursor
    /// `0`, and no `BackgroundExecutionUpdated` / `CapabilityPublished`
    /// event may exist merely because the state pre-existed. The first
    /// real post-bootstrap transition then receives cursor `1`.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn bootstrap_state_allocates_no_live_cursor_event() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;

        // Pre-existing background state, dispatched through the real
        // background path before any host binds.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
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

        // Pre-existing capability state: a real activation.
        write_probe_skill(&fixture.workspace, "pdf");
        let candidate = fixture
            .coordinator
            .prepare_candidate()
            .await
            .expect("prepare with the probe skill");
        let activated = fixture.coordinator.commit(candidate).expect("commit");
        assert_eq!(activated.revision().get(), 1, "a real semantic activation");

        // Bind the host over the inert runtime.
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host binds before activation");

        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert_eq!(
            cursor,
            RuntimeClientCursor::new(0),
            "bootstrap state allocates no cursor"
        );
        assert_eq!(
            snapshot.background.len(),
            1,
            "the pre-existing background execution is seeded"
        );
        assert_eq!(snapshot.background[0].execution_id, execution_id);
        assert_eq!(
            snapshot.capabilities.revision.get(),
            1,
            "the pre-existing capability activation is seeded"
        );
        assert!(
            snapshot.attempt.is_none() && snapshot.status.is_none(),
            "an inert runtime has no attempt and no composed status"
        );
        assert_eq!(snapshot.context.compaction_count, 0);

        // A subscription from the bootstrap cursor observes nothing at
        // all until a real transition happens: no synthetic replay of the
        // seeded state exists.
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe from the bootstrap cursor");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "the bootstrap seed published no event"
        );

        // The first real post-bootstrap transition receives cursor 1.
        fixture.runtime.activate();
        release.notify_waiters();
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
            "the first cursor belongs to a real post-bootstrap transition"
        );
        // The terminal background settlement publishes its canonical
        // notification enqueue and then the registry transition — both
        // real post-activation facts.
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeClientEvent::BackgroundExecutionUpdated { .. }
                ))
                .count(),
            1,
            "exactly one real background transition, never a seeded replay"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.event, RuntimeClientEvent::CapabilityPublished { .. })),
            "the seeded capability activation is never re-published"
        );
    }

    /// A session model update that linearizes before the bootstrap cut is
    /// part of the seed and is never re-applied as a live observation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pre_bootstrap_model_transition_is_seeded_not_duplicated() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;

        // The transition linearizes before any host exists: it is folded
        // into the runtime's authoritative state with no consumer.
        fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "before-cut"))
            .expect("model transition before the cut");

        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");
        fixture.runtime.activate();

        // The seed contains the transition...
        let (snapshot, cursor) = host.snapshot().expect("snapshot");
        assert_eq!(
            snapshot.model.configured.request_params.get("before-cut"),
            Some(&serde_json::json!("changed")),
            "the pre-cut transition must be part of the seed"
        );
        assert_eq!(
            cursor,
            RuntimeClientCursor::new(0),
            "the seed allocated no cursor"
        );
        // ...and it is not re-applied as an extra live event: drive one
        // real turn and assert no SessionModelChanged publication exists.
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
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
        assert!(
            !events.iter().any(|event| {
                matches!(event.event, RuntimeClientEvent::SessionModelChanged { .. })
            }),
            "the pre-cut transition must never be re-published"
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
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
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
