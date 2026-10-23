//! The Runtime Client host: the projection + control + attachment adapter
//! over the conversation runtime coordinator (Issue #61).
//!
//! [`RuntimeClientHost`] is the native projection/control owner reused by App Server. It
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
//! +--> App Server native host/attachment operations -> stdio / WebSocket -> clients
//! |
//! +--> RuntimeClientEndpoint -> child-owned read-only inspection socket
//! ```
//!
//! `RuntimeClientEndpoint` is the typed internal request endpoint and child
//! inspection protocol endpoint. The production Runtime Client JSONL consumer
//! is the child-owned read-only inspection socket. TUI uses App Server.
//! The host does not forward Session intent.
//!
//! The host owns:
//!
//! - one control attachment plus any number of explicitly read-only
//!   observation attachments;
//! - the Runtime Client projection (snapshot read model, cursor allocation,
//!   bounded replay, subscribers) and its linearization boundary;
//! - the root publication-admission check for child interactions, limited to
//!   the lifetime of the bound runtime projection, independently of clients;
//! - protocol adaptation: request dispatch, `model_set`/`shutdown`/
//!   `cancel_current_attempt` forwarding and inbound publish forwarding;
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
//! - SessionCatalog/SessionGraph ownership (`SessionController` owns it and
//!   App Server addresses it directly, never through this host);
//! - cancellation terminal settlement (`AgentExecution` remains the attempt
//!   execution/terminal authority);
//! - background/subagent lifecycle.
//!
//! # Observation handoff
//!
//! Native owners deliver complete same-commit receipts and immutable actor
//! sources through the installed CPU publication sink. The runtime-owned
//! observation fan-out retains
//! [`PendingObservations`](crate::runtime::observation::PendingObservations)
//! for indexed journal readiness and finite disposable activity. Its bootstrap
//! handshake runs over an inert runtime and captures every subsystem seam at
//! one native cut. The initial seed publishes nothing and allocates no cursor.
//! Ancillary Session observers read finite aggregate watches and retain no
//! independent body queue or history authority.
//!
//! The worker prepares indexed source evidence outside the Projection lock.
//! Public checkpoint, domain, page, value and content reads capture the already
//! published seed and perform bounded source reads after releasing that lock.
//! They never drain queued observations or publish a cursor. Attachment and
//! registration bookkeeping also acquires only the raw Projection lock.
//! A checkpoint at C and replay after C share this single cursor authority.
//!
//! # The lock-order graph
//!
//! ```text
//! App Server route / registration -> attachment -> Projection (bookkeeping)
//! Source preparation -> Projection (native fold and cursor publication)
//! Native subsystem -> observation fan-out -> pending queue
//! Staged publication -> fan-out enqueue, unlock -> source preparation -> Projection
//! ```
//!
//! Source preparation never acquires an App Server route, registration, or
//! attachment lock. Registration operations never acquire source preparation
//! or perform Store I/O. The publication callback runs after durable staging
//! and after releasing fan-out dispatch locks; it commits the finite native
//! display transition before returning to the existing User Release owner.
//! Physical readiness and Flush remain outside all these boundaries.
//!
//! # Lifetime
//!
//! The host retains the runtime for control and seed reads inside the resident
//! composition. External attachment/endpoint/subscription handles are weak;
//! the runtime also holds only weak references through its installed observer
//! seams. Releasing the last resident host handle closes the
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
//!
//! # Module structure
//!
//! [`ClientInner`] stays the one host state owner; its inherent implementation
//! is grouped by interface. Children own no lock, task or cancellation authority.
//!
//! - lifecycle: `construction` (host construction), `bootstrap` and
//!   `durable_bootstrap` (projection bootstrap from durable authorities),
//!   `worker` (projection worker admission, presentation reads, drain);
//! - projection: `sources` (display-source preparation off the projection
//!   lock) and `publication` (publication gate and native commit linearization);
//! - read: `read` (content/collection/value reads at an exact cut),
//!   `checkpoint` (pages and off-lock materialization), `history` (Trace,
//!   transcript, window and request-history reads), `agents` (Agent reads);
//! - subscription: `subscription` (event observation over the replay ring);
//! - control: `control` (attachment admission and forwarded control requests).

#[cfg(test)]
use publication::commit_display;
mod agents;
pub(crate) mod auxiliary;
mod bootstrap;
mod checkpoint;
mod construction;
mod control;
mod durable_bootstrap;
mod history;
mod publication;
mod read;
mod sources;
mod subscription;
mod worker;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg_attr(not(test), allow(unused_imports))]
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use super::projection::views::background_view;
use super::projection::{RuntimeClientProjection, SubscriberPoll};
use super::types::{
    AttachmentId, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor, RuntimeClientError,
    RuntimeClientProtocolEvent, RuntimeClientResult,
};
use crate::durable::{ConversationStore, TRANSCRIPT_PAGE_LIMIT_MAX};
use crate::model::session::SessionModelConfig;
use crate::model::{ModelRequest, RequestIdentity};
use crate::observation::transcript::{RuntimeClientTranscriptCursor, RuntimeClientTranscriptPage};
use crate::runtime::conversation_runtime::{
    CancelAttemptError, ConversationRuntime, InboundAdmissionError, ManualCompactionError,
    ModelUpdateError, RuntimeBootstrapError,
};
use crate::runtime::identity::{ConversationId, MessageId, ToolExecutionId};
use crate::runtime::interaction::{InteractionRef, InteractionResponse, RoutedInteractionError};
use crate::runtime::observation::{ConversationObservation, PendingObservations};
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
    /// its finite seed captures the runtime's real state at the activation
    /// cut; there is no supported hot installation of a first host over a
    /// runtime that has already begun semantic execution.
    RuntimeAlreadyActivated {
        /// The conversation whose runtime is already activated.
        conversation_id: ConversationId,
    },
    /// The native durable authority could not provide a coherent bootstrap
    /// seed for the client projection.
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

/// The host-owned attachment state.
pub(crate) struct AttachmentState {
    /// The attachment identity.
    attachment_id: AttachmentId,
    /// Shared with the handle and submission permits; detach revokes it before
    /// removing this registration or admitting a successor controller.
    authority: Arc<Mutex<bool>>,
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

#[cfg(test)]
impl ClientInner {
    /// Test-only: waits until the projection has nothing left to publish
    /// without new input — every observation folded, none awaiting its
    /// source release, and the read domains installed. Public reads remain
    /// pure; this barrier waits for the existing worker's publication before
    /// the test chooses its baseline. Join preparation and publication in
    /// their existing lock order before checking raw state: an empty queue
    /// alone does not prove its drained observation has been installed.
    pub(crate) async fn projection_settled(&self) {
        let started = std::time::Instant::now();
        *self.publication_gate.settlement_wait.lock().unwrap() = None;
        let mut turns = self.worker_turns.subscribe();
        loop {
            let worker_turn = *turns.borrow_and_update();
            {
                let _preparation = self
                    .publication_gate
                    .settlement_lock(&self.observation_preparation);
                let _publication = self
                    .publication_gate
                    .settlement_lock(&self.publication_gate.order);
                let guard = self
                    .state
                    .lock()
                    .expect("runtime client host lock poisoned");
                let pending_settled = self.pending.settled();
                let read_cut = guard.projection.read_domain_cut();
                let source_status = match guard.projection.validate_source_prefix() {
                    Ok(()) => "ready",
                    Err(RuntimeClientError::ReadCutExpired) => "pending",
                    Err(RuntimeClientError::ObservationUnavailable { .. }) => "unavailable",
                    Err(_) => "other error",
                };
                if pending_settled && read_cut.is_none() && source_status == "ready" {
                    return;
                }
                *self.publication_gate.settlement_wait.lock().unwrap() = Some(SettlementWait {
                    checked: std::time::Instant::now(),
                    elapsed: started.elapsed(),
                    worker_turn,
                    pending_settled,
                    read_cut,
                    source_status,
                    queue: self.pending.source_progress_debug(),
                });
            }
            turns
                .changed()
                .await
                .expect("the host owns the worker turn signal");
        }
    }

    /// Test-only: parks the projection worker so the test owns the fold
    /// schedule.
    ///
    /// The worker is spawned when the host is constructed and is woken by
    /// every enqueue, so without this a test that wants to look at an
    /// intermediate cut is racing it: the queue can be drained before the
    /// test reads it. Parking is ordered against every concurrent drain, so
    /// after this returns only synchronous native commits advance the projection.
    pub(crate) fn park_projection_worker(&self) {
        let _preparation = self
            .observation_preparation
            .lock()
            .expect("test source preparation gate");
        let _publication = self
            .publication_gate
            .order
            .lock()
            .expect("test publication gate");
        self.pending.park();
    }

    pub(crate) fn queued_observation_records(&self) -> usize {
        self.pending.queued()
    }
    #[cfg(test)]
    pub(crate) fn queued_lifecycle_records(&self) -> usize {
        self.pending.queued_lifecycle_records()
    }

    pub(crate) fn unpark_projection_worker(&self) {
        self.pending.unpark();
    }
}

enum TranscriptFieldSource {
    Live(u64),
    Inherited(u64),
}

#[derive(Default)]
struct PublicationGate {
    order: Mutex<()>,
    progress: (Mutex<PublicationProgress>, std::sync::Condvar),
    #[cfg(test)]
    encoding_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    preparation_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    settlement_contention_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    settlement_wait: Mutex<Option<SettlementWait>>,
    /// Runs after a checkpoint requested its Source repair read.
    #[cfg(test)]
    repair_request_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs when a checkpoint's repair must await an in-flight publication.
    #[cfg(test)]
    repair_settle_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    diagnostics: Mutex<ProjectionDiagnostics>,
}

/// Completed display transitions, and whether one is between its fold and
/// its completion. Readers wait on these, never on the publication order.
#[derive(Default)]
struct PublicationProgress {
    generation: u64,
    publishing: bool,
}

/// One display transition in flight; dropping it completes the transition,
/// also when its fold or encoding unwinds.
struct Publishing<'a>(&'a PublicationGate);

#[cfg(test)]
#[derive(Default)]
struct ProjectionDiagnostics {
    worker: Option<ProjectionStage>,
    preparation: Option<ProjectionStage>,
    publication: Option<ProjectionStage>,
    read: Option<(ProjectionStage, super::projection::ReadDomainCut)>,
}

#[cfg(test)]
struct ProjectionStage {
    name: &'static str,
    entered: std::time::Instant,
    thread: std::thread::ThreadId,
}

#[cfg(test)]
impl ProjectionStage {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            entered: std::time::Instant::now(),
            thread: std::thread::current().id(),
        }
    }

    fn describe(&self) -> String {
        format!(
            "{}@{:?} age={:?}",
            self.name,
            self.thread,
            self.entered.elapsed()
        )
    }
}

/// Last failed predicate, retained before cancellation can release native gates.
#[cfg(test)]
struct SettlementWait {
    checked: std::time::Instant,
    elapsed: std::time::Duration,
    worker_turn: u64,
    pending_settled: bool,
    read_cut: Option<super::projection::ReadDomainCut>,
    source_status: &'static str,
    queue: String,
}

struct NativePublicationProjection {
    inner: std::sync::Weak<ClientInner>,
}

type ProjectionReadResult = (
    super::projection::ReadDomainCut,
    Result<super::projection::ReadDomainValues, crate::durable::ConversationStoreError>,
);
type ProjectionReadSlot = tokio::sync::Mutex<Option<tokio::task::JoinHandle<ProjectionReadResult>>>;

#[derive(Default)]
enum ProjectionWorker {
    #[default]
    Vacant,
    Running(tokio::task::JoinHandle<()>),
    // Retained if a shutdown waiter is cancelled while joining the Source read.
    Joined(Result<(), tokio::task::JoinError>),
}

/// The shared Runtime Client host state.
pub(crate) struct ClientInner {
    pub(super) conversation_id: ConversationId,
    agent_id: crate::runtime::identity::AgentId,
    /// The live conversation runtime this host observes and controls. A
    /// durable inspection host deliberately has no runtime owner here.
    runtime: Option<ConversationRuntime>,
    /// The one durable authority used for transcript/bootstrap reads by both
    /// live and read-only hosts.
    store: Arc<dyn ConversationStore>,
    read_store: Arc<dyn ConversationStore>,
    /// Physical child inspection sockets/SQL workers across all root connections.
    pub(super) inspection_capacity: Arc<tokio::sync::Semaphore>,
    /// Whether this host is a read-only attachment to durable conversation
    /// state rather than a control adapter over a live runtime.
    read_only: bool,
    /// The one projection synchronization boundary.
    state: Arc<Mutex<ClientState>>,
    /// Serializes preparation and native folds; metadata I/O happens before
    /// acquiring the Projection lock. Lock order: preparation -> Projection.
    observation_preparation: Arc<Mutex<()>>,
    publication_gate: Arc<PublicationGate>,
    /// The observation queue shared with the conversation runtime (the
    /// projection sink installed at construction).
    pending: Arc<PendingObservations>,
    /// Whether the projection worker task was spawned.
    worker_started: AtomicBool,
    worker: tokio::sync::Mutex<ProjectionWorker>,
    projection_read: Arc<ProjectionReadSlot>,
    #[cfg(test)]
    read_domain_hook: Arc<Mutex<Option<ReadDomainHook>>>,
    #[cfg(test)]
    trace_cut_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    publication_commit_hook: Mutex<Option<PublicationCommitHook>>,
    #[cfg(test)]
    audit_commit_hook: Mutex<Option<AuditCommitHook>>,
    #[cfg(test)]
    snapshot_read_hook: Mutex<Option<Box<dyn FnMut() + Send>>>,
    #[cfg(test)]
    cancellation_receipt_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    manual_compaction_response_gate: Mutex<Option<ManualCompactionResponseGate>>,
    /// Test-only: advanced after every projection worker turn.
    #[cfg(test)]
    worker_turns: Arc<tokio::sync::watch::Sender<u64>>,
}

#[cfg(test)]
type ManualCompactionResponseGate = (
    tokio::sync::oneshot::Sender<crate::runtime::conversation_runtime::ManualCompactionOutcome>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(test)]
type ReadDomainHook =
    Box<dyn FnOnce() -> Result<(), crate::durable::ConversationStoreError> + Send>;

#[cfg(test)]
type AuditCommitHook = Box<dyn FnOnce(&ClientInner) + Send>;

#[cfg(test)]
type PublicationCommitHook = Box<dyn FnMut(&ClientInner, &[ConversationObservation], bool) + Send>;

#[cfg(test)]
fn publication_frame(
    observations: &[ConversationObservation],
) -> Option<&crate::publication::PublicationFrame> {
    observations
        .iter()
        .find_map(|observation| match observation {
            ConversationObservation::Publication { frame, .. } => Some(frame),
            ConversationObservation::ObservationBatch(observations)
            | ConversationObservation::JournalBatch { observations, .. } => {
                publication_frame(observations)
            }
            ConversationObservation::Published { observation, .. }
            | ConversationObservation::CollectionProof { observation, .. }
            | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
                publication_frame(std::slice::from_ref(observation.as_ref()))
            }
            _ => None,
        })
}

/// Root-side publication authority installed into the parent subagent
/// registry. It is a weak adapter over runtime projection binding, independent
/// of external attachments, and is not an interaction owner.
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
        self.read_store.close_presentation_reader();
        self.pending.close();
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
    /// The conversation identity of this host.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.inner.conversation_id
    }

    /// Creates the transport-neutral semantic endpoint of this runtime.
    ///
    /// This is an in-process typed request surface; no process serves it as a
    /// client protocol (App Server's stdio and WebSocket transports wrap
    /// `AppServerConnection`). This endpoint accepts every
    /// [`RuntimeClientRequest`](super::types::RuntimeClientRequest),
    /// including `initialize`, and returns the correlated
    /// [`RuntimeClientResponse`](super::types::RuntimeClientResponse). A
    /// transport therefore never performs protocol negotiation, attachment
    /// admission, identity allocation, or replacement/rejection semantics
    /// itself.
    #[must_use]
    pub fn endpoint(&self) -> super::endpoint::RuntimeClientEndpoint {
        if self.inner.read_only {
            super::endpoint::RuntimeClientEndpoint::new_read_only(self)
        } else {
            super::endpoint::RuntimeClientEndpoint::new(self)
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
    #[cfg(test)]
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
    /// successful response means the typed checkpoint already committed.
    pub async fn compact_context(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.compact_context(request_id).await
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
    pub(crate) fn published_read_cut(
        &self,
    ) -> Result<crate::observation::cut::CheckpointReadCut, RuntimeClientError> {
        let state = self.inner.state.lock().expect("runtime client state lock");
        state.projection.validate_published_source()?;
        let cut = state.projection.checkpoint_read_cut();
        if cut.source_cut.is_none() {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(cut)
    }

    /// # Errors
    ///
    /// Returns a native source/cut error when the captured checkpoint cannot be served.
    /// A failed derived Source may request one worker retry within the shared
    /// checkpoint deadline. This synchronous call must leave the existing
    /// worker runnable (use a blocking lane when called from an async executor).
    pub fn checkpoint(
        &self,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.inner.checkpoint()
    }
    /// Materializes bounded Trace descriptors at the same native checkpoint cut.
    /// # Errors
    /// Invalid selections, expired sources, and exhausted cuts are explicit.
    pub fn checkpoint_with_trace(
        &self,
        records: &[crate::observation::trace::TraceCursor],
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.inner.checkpoint_with_trace(records)
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

    /// Replaces a standalone conversation's in-memory model configuration.
    /// Durable Session runtimes reject this control; their native Session owner
    /// must commit settings, retained binding, and runtime authority together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidModelConfiguration`] when the
    /// configuration cannot be resolved against the catalog or cannot run
    /// under the session context policy, [`RuntimeClientError::InvalidState`]
    /// for a durable Session or inactive runtime, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.model_set(config)
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

    /// Injects one derived Source failure at the current native dependency fence.
    #[cfg(test)]
    pub(crate) fn fail_projection_read_for_test(&self, message: &str) {
        commit_display(
            &self.inner.state,
            &self.inner.publication_gate,
            |projection| {
                projection.request_read_domains_for_test();
                let cut = projection.read_domain_fence();
                assert!(projection.install_read_domains(
                    &cut,
                    Err(crate::durable::ConversationStoreError::InvalidReference(
                        message.into()
                    ),),
                    true
                ));
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn gate_projection_read_for_test(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        cancelled: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        let reader = self.inner.read_store.clone();
        *self.inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
            reader.park_presentation_read_until_cancelled(entered);
            let _ = cancelled.send(());
            let _ = release.recv();
            Ok(())
        }));
        self.inner
            .lock_state()
            .projection
            .request_read_domains_for_test();
        self.inner
            .pending
            .push(ConversationObservation::JournalSourceReady);
    }

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

    /// Waits until the projection has nothing left to publish without new
    /// input (see [`ClientInner::projection_settled`]).
    #[cfg(test)]
    pub(crate) async fn projection_settled(&self) {
        self.inner.projection_settled().await;
    }

    /// Installs the deterministic worker-exit signal of the projection
    /// worker, for lifetime tests.
    #[cfg(test)]
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        self.inner.pending.install_worker_exit_probe(sender);
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
    /// The native source is unavailable; no cursor is consumed or skipped.
    Unavailable { message: String },
    /// The cursor space is exhausted; nothing further will be published.
    Exhausted,
}

/// The shared registration of one event subscription.
///
/// Dropping the last handle removes the registration from the projection,
/// which is the same release an explicit detach performs.
struct SubscriptionInner {
    auxiliary: Option<crate::runtime::auxiliary::observer::AuxiliarySubscription>,
    /// The host whose projection owns the registration.
    host: Weak<ClientInner>,
    /// The opaque registration identity.
    subscriber_id: u64,
    /// The edge-triggered wakeup handle of this subscriber.
    notify: Arc<tokio::sync::Notify>,
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

#[cfg(test)]
mod tests;
