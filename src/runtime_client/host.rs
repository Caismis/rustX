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
use std::sync::{Arc, Mutex, MutexGuard};

use chrono_tz::Tz;
use tokio::sync::mpsc;

use super::projection::{Observation, RuntimeClientProjection, background_view, capability_view};
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
use crate::context::{AgentStatusComposer, ContextEngine, ContextRuntime, ContextSummarizer};
use crate::events::types::RuntimeEvent;
use crate::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::adapter::ModelAdapter;
use crate::model::types::{ModelProtocol, ReasoningEffort};
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
        }
    }
}

impl std::error::Error for HostConstructionError {}

/// The shared context-plane pieces of the host.
///
/// Every attempt builds a fresh [`ContextRuntime`] from these shared
/// pieces, so the engine, the summarizer, the checkpoint store, and the
/// Agent Status composer are exactly the instances the host was
/// constructed with: checkpoints and status providers persist across
/// attempts, and the model path and the Runtime Client projection share
/// one composer.
#[derive(Clone)]
pub struct RuntimeClientContextConfig {
    /// The deterministic context engine.
    pub engine: ContextEngine,
    /// The provider-neutral summary service.
    pub summarizer: Arc<dyn ContextSummarizer>,
    /// The checkpoint store.
    pub checkpoint_store: Arc<dyn ContextCheckpointStore>,
    /// The Agent Status composer shared by the model path and the Runtime
    /// Client projection.
    pub status_composer: AgentStatusComposer,
}

/// The construction-time configuration of one Runtime Client host.
pub struct RuntimeClientHostConfig {
    /// The conversation this runtime serves.
    pub conversation_id: ConversationId,
    /// The agent executed by attempts of this runtime.
    pub agent_id: AgentId,
    /// The provider model identifier of every attempt.
    pub model: String,
    /// The canonical protocol of every attempt.
    pub protocol: ModelProtocol,
    /// The reasoning effort of every attempt.
    pub reasoning: ReasoningEffort,
    /// The effective maximum output tokens of every attempt.
    pub max_output_tokens: u32,
    /// The per-conversation IANA timezone, when known.
    pub timezone: Option<Tz>,
    /// The model adapter of every attempt.
    pub adapter: Arc<dyn ModelAdapter>,
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

/// The bounded pending-observation queue of the mailbox seam.
///
/// The mailbox observer fires while the mailbox lock is held and can
/// therefore never take the host lock directly (the host drains the
/// mailbox under its own lock). It queues here instead; the worker task
/// and every host lock acquisition apply the queue under the host lock,
/// preserving total order (the queue preserves push order).
struct PendingObservations {
    /// The FIFO observation queue.
    queue: Mutex<VecDeque<Observation>>,
    /// Wakes the worker task on every push.
    notify: tokio::sync::Notify,
}

impl PendingObservations {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn push(&self, observation: Observation) {
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
}

/// The shared host state.
pub(crate) struct HostInner {
    conversation_id: ConversationId,
    agent_id: AgentId,
    model: String,
    protocol: ModelProtocol,
    reasoning: ReasoningEffort,
    max_output_tokens: u32,
    timezone: Option<Tz>,
    adapter: Arc<dyn ModelAdapter>,
    context: RuntimeClientContextConfig,
    tool_runtime: ConversationToolRuntime,
    mailbox: ConversationInboundMailbox,
    capability: CapabilityCoordinator,
    clock: Arc<dyn RuntimeClock>,
    /// The one synchronization boundary.
    state: Mutex<HostState>,
    /// The mailbox-observation queue (see [`PendingObservations`]).
    pending: PendingObservations,
    /// Whether the projection worker task was spawned.
    worker_started: AtomicBool,
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

    /// Builds one fresh `ContextRuntime` from the shared pieces.
    fn context_runtime(&self) -> ContextRuntime<'static> {
        ContextRuntime::with_status_composer(
            self.context.engine.clone(),
            self.context.summarizer.clone(),
            self.context.checkpoint_store.clone(),
            self.context.status_composer.clone(),
        )
    }

    /// Spawns the projection worker: applies queued mailbox observations
    /// promptly so subscribed clients observe enqueue/drain facts without
    /// sending requests. The worker holds a weak handle and exits when the
    /// host is dropped.
    fn ensure_worker(self: &Arc<Self>) {
        if self.worker_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                inner.pending.notify.notified().await;
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let _state = inner.lock_state();
            }
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
    ) -> crate::agent::AgentExecutionResult {
        let lease = self.capability.acquire_attempt_lease();
        let observer = HostObserver {
            inner: self.clone(),
        };
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
            model: self.model.clone(),
            protocol: self.protocol,
            reasoning: self.reasoning,
            max_output_tokens: self.max_output_tokens,
        };
        let mut execution = AgentExecution::new(
            request,
            self.adapter.as_ref(),
            lease,
            cancellation,
            self.context_runtime(),
            &self.tool_runtime,
        )
        .expect("host construction validated the ownership domains");
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
        drop(state);
        let inner = Arc::clone(self);
        tokio::spawn(async move {
            let result = inner
                .run_attempt(
                    attempt_id.clone(),
                    initial_messages,
                    Some(fresh),
                    &cancellation,
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
    /// # Errors
    ///
    /// Returns [`HostConstructionError::OwnershipMismatch`] when the
    /// capability coordinator and the conversation tool runtime do not
    /// share the same conversation/workspace ownership domain, and
    /// [`HostConstructionError::Context`] when the context engine
    /// configuration is impossible.
    pub fn new(config: RuntimeClientHostConfig) -> Result<Self, HostConstructionError> {
        let snapshot = config.capability.current_snapshot();
        if snapshot.conversation_id() != config.tool_runtime.conversation_id()
            || snapshot.workspace_root() != config.tool_runtime.workspace().root()
        {
            return Err(HostConstructionError::OwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: config.tool_runtime.conversation_id().clone(),
            });
        }
        let replay_limit = config
            .replay_limit
            .unwrap_or(super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
        let mailbox = config.tool_runtime.mailbox();
        let clock = config
            .clock
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn RuntimeClock>);
        let mut projection = RuntimeClientProjection::new(
            config.conversation_id.clone(),
            config.initial_messages.clone(),
            capability_view(&snapshot),
            replay_limit,
        );
        // Mirror the pre-existing authoritative background records.
        for existing in config.tool_runtime.background().all_snapshots() {
            projection.apply(Observation::Background(existing));
        }
        let inner = Arc::new(HostInner {
            conversation_id: config.conversation_id,
            agent_id: config.agent_id,
            model: config.model,
            protocol: config.protocol,
            reasoning: config.reasoning,
            max_output_tokens: config.max_output_tokens,
            timezone: config.timezone,
            adapter: config.adapter,
            context: config.context,
            tool_runtime: config.tool_runtime,
            mailbox,
            capability: config.capability,
            clock,
            state: Mutex::new(HostState {
                projection,
                canonical_history: config.initial_messages,
                current_attempt: None,
                attachment: None,
                shutting_down: false,
                next_attachment_seq: 0,
                next_attempt_seq: 0,
                next_inbound_seq: 0,
            }),
            pending: PendingObservations::new(),
            worker_started: AtomicBool::new(false),
        });
        let observer: Arc<HostObserver> = Arc::new(HostObserver {
            inner: inner.clone(),
        });
        inner.mailbox.install_observer(observer.clone());
        inner
            .tool_runtime
            .background()
            .install_observer(observer.clone());
        inner.capability.install_observer(observer);
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

    /// Admits one attachment (the `initialize` semantic operation).
    ///
    /// Protocol v1 allows at most one active attachment; a second
    /// simultaneous attach fails deterministically and never evicts the
    /// first. The returned snapshot and cursor are linearized with the
    /// admission under the one synchronization boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version and [`RuntimeClientError::AttachmentInUse`]
    /// when an attachment is active.
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
        state.next_attachment_seq = state.next_attachment_seq.saturating_add(1);
        let attachment_id = AttachmentId::new(format!("attachment-{}", state.next_attachment_seq));
        let (snapshot, cursor) = state.projection.snapshot();
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
    #[must_use]
    pub fn snapshot(&self) -> (RuntimeClientSnapshot, RuntimeClientCursor) {
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
        let (subscriber_id, receiver) = state.projection.subscribe(after_cursor)?;
        state
            .attachment
            .as_mut()
            .expect("the attachment identity was just checked")
            .subscriber_id = Some(subscriber_id);
        Ok((
            EventSubscription { receiver },
            RuntimeClientResult::Subscribed { after_cursor },
        ))
    }

    /// Reads the active capability projection (the one semantic
    /// implementation shared with the snapshot).
    #[must_use]
    pub fn capability(&self) -> RuntimeClientResult {
        let state = self.inner.lock_state();
        RuntimeClientResult::Capability {
            capabilities: state.projection.snapshot_ref().capabilities.clone(),
        }
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
pub(crate) struct HostObserver {
    inner: Arc<HostInner>,
}

impl HostObserver {
    /// Applies one observation directly under the host lock, applying
    /// queued pending observations first so total order is preserved.
    fn apply_direct(&self, observation: Observation) {
        let mut state = self.inner.lock_state();
        state.projection.apply(observation);
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

impl InboundObserver for HostObserver {
    fn on_enqueued(&self, item: &InboundItem) {
        self.inner
            .pending
            .push(Observation::InboundEnqueued(item.clone()));
    }

    fn on_drained(&self, batch: &InboundBatch) {
        self.inner
            .pending
            .push(Observation::InboundDrained(batch.clone()));
    }
}

impl BackgroundObserver for HostObserver {
    fn on_snapshot(&self, snapshot: &BackgroundExecutionSnapshot) {
        self.apply_direct(Observation::Background(snapshot.clone()));
    }
}

impl CapabilityObserver for HostObserver {
    fn on_snapshot(&self, snapshot: &crate::capabilities::CapabilitySnapshot) {
        self.apply_direct(Observation::Capability(capability_view(snapshot)));
    }
}

/// The live delivery handle of one event subscription.
///
/// The subscription observes the retained replay gap followed by every
/// subsequently published event. When the owning attachment detaches (or
/// re-subscribes), the delivery channel closes: `next` returns `None`.
pub struct EventSubscription {
    receiver: mpsc::UnboundedReceiver<RuntimeClientProtocolEvent>,
}

impl core::fmt::Debug for EventSubscription {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventSubscription").finish()
    }
}

impl EventSubscription {
    /// Receives the next event of the observation stream.
    ///
    /// Returns `None` when the owning attachment detached or replaced the
    /// subscription.
    pub async fn next(&mut self) -> Option<RuntimeClientProtocolEvent> {
        self.receiver.recv().await
    }

    /// Attempts to receive the next event without waiting.
    #[must_use]
    pub fn try_next(&mut self) -> Option<RuntimeClientProtocolEvent> {
        self.receiver.try_recv().ok()
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
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use futures_util::future::BoxFuture;
    use tokio::sync::watch;

    use super::{
        EventSubscription, HostConstructionError, RuntimeClientContextConfig, RuntimeClientHost,
        RuntimeClientHostConfig,
    };
    use crate::context::{
        AgentStatusClock, AgentStatusComposer, AgentStatusFact, AgentStatusRenderContext,
        AgentStatusSectionId, AgentStatusSectionProvider, ContextEngine, ContextError,
        DefaultTokenEstimator, InMemoryCheckpointStore, TokenEstimator,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{
        ContentBlockIndex, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelEventStream};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest, ReasoningEffort};
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
        let engine = ContextEngine::new(
            crate::context::ContextConfig {
                context_window_tokens: 10_000_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            estimator,
        )
        .expect("context engine");
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            conversation_id: conversation_id.clone(),
            agent_id: AgentId::new("agent-a"),
            model: "scripted".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            reasoning: ReasoningEffort::Medium,
            max_output_tokens: 512,
            timezone: None,
            adapter: adapter.clone(),
            context: RuntimeClientContextConfig {
                engine,
                summarizer: Arc::new(super::tests::NeverSummarizes),
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

    /// A no-compaction summarizer (a huge window prevents compaction).
    struct NeverSummarizes;

    impl crate::context::ContextSummarizer for NeverSummarizes {
        fn summarize(
            &self,
            _request: crate::context::summarizer::SummaryRequest,
            _cancellation: CancellationSignal,
        ) -> BoxFuture<'_, Result<String, ContextError>> {
            unreachable!("no compaction occurs under a huge window")
        }
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

    /// Receives events until the predicate matches, with a bounded count.
    async fn receive_until(
        subscription: &mut EventSubscription,
        mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
    ) -> Vec<RuntimeClientProtocolEvent> {
        let mut seen = Vec::new();
        loop {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(10), subscription.next())
                    .await
                    .expect("event stream must not stall")
                    .expect("subscription must stay open");
            let matched = predicate(&event);
            seen.push(event);
            if matched {
                return seen;
            }
        }
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
        let mut subscription = attachment
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
        let events = receive_until(&mut subscription, |event| {
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
        let (snapshot, _) = fixture.host.snapshot();
        assert_eq!(snapshot.messages.len(), 2, "user message + agent message");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        assert_eq!(
            fixture.host.canonical_history(),
            snapshot.messages,
            "the projection mirrors the authoritative canonical history"
        );
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
        let mut subscription = attachment
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
        receive_until(&mut subscription, |event| {
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
        let (snapshot, _) = fixture.host.snapshot();
        assert_eq!(snapshot.inbound.pending.len(), 1);
        assert_eq!(snapshot.inbound.pending[0].message.id, second_id);
        assert_eq!(adapter.requests().len(), 1, "no new turn yet");

        // Release the parked turn: the safe boundary drains the queued
        // message and a second turn observes it.
        release_tx.send(true).expect("release");
        receive_until(&mut subscription, |event| {
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
        let (snapshot, _) = fixture.host.snapshot();
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
        let mut subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&mut subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
        })
        .await;

        // The cancel response is acceptance, not settlement: the attempt
        // is still running when the response returns.
        let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(2),
        });
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::AttemptCancellationAccepted { .. })
        ));
        let (snapshot, _) = fixture.host.snapshot();
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Running
        ));

        // A second cancel while the first is still active is also accepted
        // (cancellation is idempotent at the signal level).
        let second_cancel = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(3),
        });
        assert!(second_cancel.error.is_none());

        // Release the parked model: the adapter observes the attempt
        // cancellation and the loop settles cancelled — exactly once.
        release_tx.send(true).expect("release");
        let events = receive_until(&mut subscription, |event| {
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
        let (snapshot, _) = fixture.host.snapshot();
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
            let (snapshot, _) = fixture.host.snapshot();
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
        let (snapshot, cursor) = fixture.host.snapshot();
        assert!(
            matches!(
                snapshot.attempt.expect("attempt view").phase,
                RuntimeClientAttemptPhase::Running
            ),
            "detach never cancels the attempt"
        );
        // The attempt completes normally after release.
        release_tx.send(true).expect("release");
        let mut subscription = second
            .subscribe_events(cursor)
            .expect("resume from the retained cursor");
        receive_until(&mut subscription, |event| {
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
        let (before, _) = fixture.host.snapshot();
        assert_eq!(before.background.len(), 1);
        assert!(matches!(
            before.background[0].state,
            BackgroundLifecycle::Running
        ));
        assert_eq!(before.inbound.pending.len(), 1);
        attachment.detach();

        // After detach the background execution still runs and the mailbox
        // item still pends.
        let (after, _) = fixture.host.snapshot();
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
        let (final_snapshot, _) = fixture.host.snapshot();
        assert!(matches!(
            final_snapshot.background[0].state,
            BackgroundLifecycle::Succeeded
        ));
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
        let (snapshot, cursor) = snapshot_task.await.expect("snapshot task");
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
        let mut subscription = attachment
            .subscribe_events(cursor)
            .expect("resume after the snapshot cursor");
        let events = receive_until(&mut subscription, |event| {
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
        receive_until(&mut subscription, |event| {
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
        let (before, cursor) = fixture.host.snapshot();
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
        let (after_snapshot, after_cursor) = snapshot_task.await.expect("snapshot task");
        // The transition is already reflected: the cursor advanced and the
        // events are either in the snapshot's state or replayable before
        // it — never lost.
        assert!(after_cursor > cursor, "the transition advanced the cursor");
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach");
        let mut subscription = attachment
            .subscribe_events(cursor)
            .expect("resume from the pre-transition cursor");
        let mut saw_inbound = false;
        loop {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(10), subscription.next())
                    .await
                    .expect("stream must not stall")
                    .expect("subscription stays open");
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
        let mut subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        let events = receive_until(&mut subscription, |event| {
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
        let mut second_subscription = second
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("resume from the retained stream");
        let replayed = receive_until(&mut second_subscription, |event| {
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
        let (snapshot, _) = fixture.host.snapshot();
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
        let (snapshot, _) = fixture.host.snapshot();
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
        let mut subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&mut subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;

        // The detached execution remains visible after the attempt
        // terminated and settles on its own schedule.
        let (snapshot, _) = fixture.host.snapshot();
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
        let mut subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        let events = receive_until(&mut subscription, |event| {
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
        receive_until(&mut subscription, |event| {
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
        let (snapshot, _) = fixture.host.snapshot();
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
        let mut subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&mut subscription, |event| {
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
        receive_until(&mut subscription, |event| {
            matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
        })
        .await;
        attachment.detach();
        let (reattached, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
            .expect("attach after shutdown still works");
        let (snapshot, _) = fixture.host.snapshot();
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
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), composer()).await;
        let other_dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(other_dir.path().join("workspace")).expect("workspace");
        let other_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new("conv-other"),
            other_dir.path().join("workspace"),
            other_dir.path().join("artifacts"),
        )
        .expect("other runtime");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let engine = ContextEngine::new(
            crate::context::ContextConfig {
                context_window_tokens: 1_000_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            estimator,
        )
        .expect("engine");
        let error = RuntimeClientHost::new(RuntimeClientHostConfig {
            conversation_id: ConversationId::new("conv-host"),
            agent_id: AgentId::new("agent-a"),
            model: "scripted".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            reasoning: ReasoningEffort::Medium,
            max_output_tokens: 512,
            timezone: None,
            adapter: fixture.host.inner.adapter.clone(),
            context: RuntimeClientContextConfig {
                engine,
                summarizer: Arc::new(NeverSummarizes),
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
        let engine = ContextEngine::new(
            crate::context::ContextConfig {
                context_window_tokens: 10_000_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            estimator,
        )
        .expect("engine");
        let host = RuntimeClientHost::with_probe(
            RuntimeClientHostConfig {
                conversation_id: conversation_id.clone(),
                agent_id: AgentId::new("agent-a"),
                model: "scripted".to_owned(),
                protocol: ModelProtocol::OpenAiChatCompletions,
                reasoning: ReasoningEffort::Medium,
                max_output_tokens: 512,
                timezone: None,
                adapter: adapter.clone(),
                context: RuntimeClientContextConfig {
                    engine,
                    summarizer: Arc::new(NeverSummarizes),
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
